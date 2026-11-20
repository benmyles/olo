//! The gateway against a fake provider: never a real account or service.

use olo_sync::Mutex;
use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use olo_token_pool::{
    gateway::{self, Shared},
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use serde_json::{Value, json};

/// What the fake provider saw: (token, path, had the client's key).
type Seen = Arc<Mutex<Vec<(String, String, bool)>>>;

async fn serve(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

fn token(h: &HeaderMap) -> String {
    h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_owned()
}

/// A fake Claude: "full" is out of quota, "stale" is refused until
/// refreshed (to "fresh"), anything else answers in a stream.
fn fake(seen: Seen) -> Router {
    let s2 = seen.clone();
    Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let seen = seen.clone();
                async move {
                    let t = token(&h);
                    seen.lock().unwrap().push((t.clone(), "/v1/messages".into(), h.contains_key("x-api-key")));
                    assert!(h.get("anthropic-beta").unwrap().to_str().unwrap().contains("oauth-2025-04-20"));
                    match t.as_str() {
                        "full" => Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3600).to_string())
                            .body(Body::from("{}"))
                            .unwrap(),
                        "stale" => StatusCode::UNAUTHORIZED.into_response(),
                        _ => Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("anthropic-ratelimit-unified-5h-utilization", "0.25")
                            .body(Body::from(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7}}}\n\n\
                                 data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hello\"}}\n\n\
                                 data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3}}\n\n",
                            ))
                            .unwrap(),
                    }
                }
            }),
        )
        .route(
            "/oauth",
            post(move |body: axum::body::Bytes| {
                let seen = s2.clone();
                async move {
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    seen.lock().unwrap().push((v["refresh_token"].as_str().unwrap().into(), "/oauth".into(), false));
                    axum::Json(json!({"access_token": "fresh", "refresh_token": "r2", "expires_in": 3600}))
                }
            }),
        )
        .route("/api/oauth/usage", get(|| async { axum::Json(json!({"five_hour": {"utilization": 10.0, "resets_at": now() + 1000}})) }))
}

fn account(id: &str, created: i64, resets: i64) -> Account {
    let mut a = Account::new(id.into(), id.into(), Provider::Claude);
    a.created = created;
    a.windows.push(olo_token_pool::model::Window { name: "five_hour".into(), label: "5-hour".into(), used: 10.0, resets_at: Some(now() + resets), models: vec![] });
    a
}

fn cred(t: &str) -> Credential {
    Credential { access: t.into(), refresh: Some(format!("r-{t}")), expires_at: Some(now() + 3600), account_id: None }
}

#[tokio::test(flavor = "multi_thread")]
async fn drains_and_fails_over() {
    let seen: Seen = Default::default();
    let up_addr = serve(fake(seen.clone())).await;
    let base = format!("http://{up_addr}");
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    // "a" expires soonest but is out; "b" next but its sign-in is stale;
    // "c" last.
    let mut saved = Saved::default();
    for (id, t, resets) in [("a", "full", 100), ("b", "stale", 200), ("c", "ok", 300)] {
        saved.accounts.push(account(id, 1, resets));
        saved.credentials.insert(id.into(), cred(t));
    }
    let shared = Shared::new(saved, None, up, || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let client = reqwest::Client::new();
    let r = client
        .post(format!("http://{addr}/claude/v1/messages"))
        .header("x-api-key", "client-key")
        .header("authorization", "Bearer olo-pool")
        .json(&json!({"model": "claude-opus-5-5", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // The refreshed "b" answered, after "a" was out.
    assert_eq!(r.headers()["x-olo-pool-account"], "b");
    let body = r.text().await.unwrap();
    assert!(body.contains("hello"));
    let calls = seen.lock().unwrap().clone();
    let tokens: Vec<&str> = calls.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(tokens, ["full", "stale", "r-stale", "fresh"]);
    assert!(calls.iter().all(|c| !c.2), "the client's own key reached the provider");

    // The usage, and only the usage, was kept; "a" waits for its reset.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let p = shared.snapshot();
    let b = p.account("b").unwrap();
    assert_eq!((b.total.requests, b.total.input_tokens, b.total.output_tokens), (1, 7, 3));
    assert_eq!(b.windows[0].used, 25.0);
    assert_eq!(p.saved.credentials["b"].refresh.as_deref(), Some("r2"));
    assert!(p.account("a").unwrap().cooldown_until.is_some());
    assert_eq!(p.in_flight.values().sum::<u32>(), 0);
    // Each try is in its account's strip, with how it ended.
    use olo_token_pool::model::Outcome;
    assert_eq!(p.account("a").unwrap().recent.iter().map(|r| r.outcome).collect::<Vec<_>>(), [Outcome::RateLimited]);
    assert_eq!(b.recent.iter().map(|r| (r.outcome, r.usage.output_tokens)).collect::<Vec<_>>(), [(Outcome::Completed, 3)]);
    assert!(p.open.is_empty());
    assert!(!serde_json::to_string(&gateway::status_json(&p)).unwrap().contains("fresh"));

    // A web page can't use it.
    let r = client.post(format!("http://{addr}/claude/v1/messages")).header("origin", "https://example.invalid").body("{}").send().await.unwrap();
    assert_eq!(r.status(), 403);

    // With every account out, the client is told when to come back.
    shared.set_enabled("b", false);
    shared.set_enabled("c", false);
    let r = client.post(format!("http://{addr}/claude/v1/messages")).json(&json!({"model": "claude-opus-5-5"})).send().await.unwrap();
    assert_eq!(r.status(), 429);
    let wait: i64 = r.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!(wait > 3000 && wait <= 3600, "{wait}");
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["type"], "rate_limit_error");

    // Reading quota refreshes the windows from the report.
    shared.sync(Some("c")).await;
    assert_eq!(shared.snapshot().account("c").unwrap().windows[0].used, 10.0);
}

/// One account, out of quota, with a reset credit: the request redeems it
/// (once, for the account's organization) and is then served.
#[tokio::test(flavor = "multi_thread")]
async fn redeems_a_credit_when_all_are_out() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let reset = Arc::new(AtomicBool::new(false));
    let redeemed = Arc::new(AtomicUsize::new(0));
    let (r1, r2, n) = (reset.clone(), reset.clone(), redeemed.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let reset = r1.clone();
                async move {
                    if reset.load(Ordering::SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap()
                    } else {
                        Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3600).to_string())
                            .body(Body::from("{}"))
                            .unwrap()
                    }
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>, body: axum::body::Bytes| {
                let (reset, n) = (r2.clone(), n.clone());
                async move {
                    assert_eq!(org, "org-1");
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!((v["grant_id"].as_str(), v["program"].as_str()), (Some("g1"), Some("cedar_ember")));
                    assert_eq!(v["request_id"].as_str().unwrap().len(), 36);
                    n.fetch_add(1, Ordering::SeqCst);
                    reset.store(true, Ordering::SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        )
        .route("/api/oauth/usage", get(|| async { axum::Json(json!({"five_hour": {"utilization": 1.0, "resets_at": now() + 18000}})) }));
    let base = format!("http://{}", serve(fake).await);
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    let mut saved = Saved::default();
    let mut a = account("a", 1, 100);
    a.org = Some("org-1".into());
    a.credits.push(olo_token_pool::model::ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 2, usable: true, program: Some("cedar_ember".into()), ..Default::default() });
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("ok"));
    let shared = Shared::new(saved, None, up, || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let client = reqwest::Client::new();
    let r = client.post(format!("http://{addr}/claude/v1/messages")).json(&json!({"model": "claude-opus-5-5"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(redeemed.load(Ordering::SeqCst), 1);
    let p = shared.snapshot();
    let a = p.account("a").unwrap();
    // Read again from the provider afterwards (this fake reports none left).
    assert!(a.credits.iter().all(|c| c.remaining <= 1));
    assert_eq!((a.redeeming.is_none(), a.cooldown_until), (true, None));
}

/// Quota by model (Ben, 2026-11-08: "make sure too that token pooler
/// switches accounts when the model in use is the one that the limit hits
/// like it needs to be quota-model aware"). "a"'s report says its Fable
/// week is used up: a Fable request skips it (never tried) and an Opus one
/// still uses it; "c" is refused for Fable when it's tried (its Fable
/// meter, says the claim): the request goes on to "b", and "c" waits for
/// Fable only.
#[tokio::test(flavor = "multi_thread")]
async fn switches_by_the_model_whose_meter_ran_out() {
    type Asked = Arc<Mutex<Vec<(String, String)>>>;
    let asked: Asked = Default::default();
    let seen = asked.clone();
    let iso = |secs: i64| chrono::DateTime::from_timestamp(now() + secs, 0).unwrap().to_rfc3339();
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap, body: axum::body::Bytes| {
                let seen = seen.clone();
                async move {
                    let (t, model) = (token(&h), serde_json::from_slice::<Value>(&body).unwrap()["model"].as_str().unwrap_or("").to_owned());
                    seen.lock().unwrap().push((t.clone(), model.clone()));
                    if t == "c" && model.contains("fable") {
                        return Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "seven_day_fable")
                            .header("anthropic-ratelimit-unified-reset", (now() + 7200).to_string())
                            .body(Body::from("{}"))
                            .unwrap();
                    }
                    Response::builder().header("content-type", "text/event-stream").body(Body::from("data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n")).unwrap()
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| async move {
                // Each account's own: "a" first to reset, then "c", then "b";
                // "a"'s Fable week used up.
                let (session, fable) = match token(&h).as_str() {
                    "a" => (100, 100.0),
                    "c" => (200, 20.0),
                    _ => (300, 10.0),
                };
                axum::Json(json!({
                    "five_hour": {"utilization": 10.0, "resets_at": now() + session},
                    "limits": [{"kind": "weekly_scoped", "group": "weekly", "percent": fable, "resets_at": iso(3 * 86400), "scope": {"model": {"display_name": "Fable"}}}],
                }))
            }),
        );
    let base = format!("http://{}", serve(fake).await);
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    let mut saved = Saved::default();
    for (id, created) in [("a", 1), ("b", 2), ("c", 3)] {
        saved.accounts.push(account(id, created, 1000));
        saved.credentials.insert(id.into(), Credential { access: id.into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    }
    let shared = Shared::new(saved, None, up, || {});
    for id in ["a", "b", "c"] {
        shared.sync(Some(id)).await;
    }
    let fable = shared.snapshot().account("a").unwrap().windows.iter().find(|w| w.label == "Weekly Fable").cloned().unwrap();
    assert_eq!((fable.used, fable.models.as_slice()), (100.0, ["fable".to_owned()].as_slice()), "the report's Fable week, for Fable");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let client = reqwest::Client::new();
    let ask = |model: &'static str| {
        let (client, addr) = (client.clone(), addr);
        async move {
            let r = client.post(format!("http://{addr}/claude/v1/messages")).json(&json!({"model": model, "messages": []})).send().await.unwrap();
            let who = r.headers().get("x-olo-pool-account").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
            (r.status().as_u16(), who)
        }
    };

    // Fable: not "a" (its Fable week is out), not "c" (refused for Fable):
    // "b"; "a" never asked.
    assert_eq!(ask("claude-fable-5-1").await, (200, "b".to_owned()));
    let tried: Vec<(String, String)> = asked.lock().unwrap().drain(..).collect();
    assert_eq!(tried, [("c".to_owned(), "claude-fable-5-1".to_owned()), ("b".to_owned(), "claude-fable-5-1".to_owned())]);
    // Opus: "a" still, first to reset; and "c" was held back for Fable only.
    assert_eq!(ask("claude-opus-5-5").await, (200, "a".to_owned()));
    let p = shared.snapshot();
    let c = p.account("c").unwrap();
    assert!(c.cooldown_until.is_none_or(|t| t <= now()) && c.model_cooldowns.get("fable").is_some_and(|&t| t > now()), "{:?} {:?}", c.cooldown_until, c.model_cooldowns);
    // Fable again: straight to "b", neither "a" nor "c" tried.
    asked.lock().unwrap().clear();
    assert_eq!(ask("claude-fable-5-1").await, (200, "b".to_owned()));
    assert_eq!(asked.lock().unwrap().iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["b"]);
    // And with "b" off too, Fable waits (until "c"'s Fable resets) while
    // Opus is served.
    shared.set_enabled("b", false);
    assert_eq!(ask("claude-fable-5-1").await.0, 429);
    assert_eq!(ask("claude-opus-5-5").await, (200, "a".to_owned()));
}

/// The network changes while a request waits for its answer (Ben,
/// 2026-11-09: a cable pulled, Wi-Fi left): its connection is cut, as one
/// on a network that went is (`olo_net`). The host may have it already, so
/// the pool never sends it twice (Codex's review reproduced the double):
/// it answers 502 once there's a network again, and the client asks again
/// on it, as Claude Code's and Codex's own retries do; with no network at
/// all, it waits for one before answering.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_goes_on_across_a_network_change() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    let asked = Arc::new(AtomicUsize::new(0));
    let slow = Arc::new(AtomicBool::new(true));
    let fake = Router::new().route(
        "/v1/messages",
        post({
            let (asked, slow) = (asked.clone(), slow.clone());
            move || {
                let (asked, slow) = (asked.clone(), slow.clone());
                async move {
                    asked.fetch_add(1, SeqCst);
                    // (Thinking: long enough for the network to change.)
                    if slow.load(SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }
                    Response::builder().header("content-type", "text/event-stream").body(Body::from("data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hello\"}}\n\n")).unwrap()
                }
            }
        }),
    );
    let up_addr = serve(fake).await;
    let base = format!("http://{up_addr}");
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    let mut saved = Saved::default();
    saved.accounts.push(account("a", 1, 100));
    saved.credentials.insert("a".into(), cred("ok"));
    let shared = Shared::new(saved, None, up, || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let ask = move || async move { reqwest::Client::new().post(format!("http://{addr}/claude/v1/messages")).json(&json!({"model": "claude-opus-5-5"})).send().await.unwrap() };
    let until = |n: usize, asked: Arc<AtomicUsize>| async move {
        let t = std::time::Instant::now();
        while asked.load(SeqCst) < n {
            assert!(t.elapsed().as_secs() < 10, "asked {} times, not {n}", asked.load(SeqCst));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    // Changed under it: never sent twice; the client's own retry, on the
    // new network.
    let started = std::time::Instant::now();
    let pending = tokio::spawn(ask());
    until(1, asked.clone()).await;
    slow.store(false, SeqCst);
    let cut = olo_net::pretend_change(true, Some(up_addr.port()));
    assert!(cut >= 1, "the gateway's connection upstream was cut");
    let r = pending.await.unwrap();
    assert_eq!(r.status(), 502);
    assert_eq!(asked.load(SeqCst), 1, "the pool sent it twice");
    let r = ask().await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("hello"));
    assert_eq!(asked.load(SeqCst), 2);
    println!("answered {:?} after it was sent, the network changing under it", started.elapsed());

    // No network: it waits for one, then asks.
    slow.store(true, SeqCst);
    let pending = tokio::spawn(ask());
    until(3, asked.clone()).await;
    slow.store(false, SeqCst);
    olo_net::pretend_change(false, Some(up_addr.port()));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!pending.is_finished(), "it waits for the network, not fails");
    assert_eq!(asked.load(SeqCst), 3);
    olo_net::pretend_change(true, None);
    let r = pending.await.unwrap();
    assert_eq!(r.status(), 502);
    assert_eq!(asked.load(SeqCst), 3, "the pool sent it twice");
    let r = ask().await;
    assert_eq!(r.status(), 200);
    assert_eq!(asked.load(SeqCst), 4);
    assert_eq!(shared.snapshot().in_flight.values().sum::<u32>(), 0);
}

/// ChatGPT refuses a compressed Codex request with a bare 400 (Ben,
/// 2026-11-09: a conversation whose history sat just past 3 × 128 KiB
/// failed every turn): the gateway sends it once more with its JSON
/// decoded, which is served. A request refused plain isn't sent again, nor
/// one refused both ways a third time.
#[tokio::test(flavor = "multi_thread")]
async fn a_compressed_codex_request_refused_goes_again_plain() {
    // What reached the fake: (content-encoding, the body).
    type Got = Arc<Mutex<Vec<(Option<String>, Vec<u8>)>>>;
    let got: Got = Default::default();
    let refuse_plain = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fake = Router::new().route(
        "/responses",
        post({
            let (got, refuse_plain) = (got.clone(), refuse_plain.clone());
            move |h: HeaderMap, body: axum::body::Bytes| {
                let (got, refuse_plain) = (got.clone(), refuse_plain.clone());
                async move {
                    let coding = h.get("content-encoding").and_then(|v| v.to_str().ok()).map(str::to_owned);
                    got.lock().unwrap().push((coding.clone(), body.to_vec()));
                    if coding.is_some() || refuse_plain.load(std::sync::atomic::Ordering::SeqCst) {
                        return Response::builder().status(400).header("content-type", "application/json").body(Body::from(r#"{"detail":"Bad Request"}"#)).unwrap();
                    }
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from("data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":2}}}\n\n"))
                        .unwrap()
                }
            }
        }),
    );
    let up_addr = serve(fake).await;
    let base = format!("http://{up_addr}");
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "a".into(), Provider::Codex);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("ok"));
    let shared = Shared::new(saved, None, up, || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    // Just past 3 × 128 KiB, as Codex sends it: zstd.
    let json = serde_json::to_vec(&json!({"model": "gpt-6-astra", "input": [{"type": "message", "role": "user", "content": "x".repeat(3 * 131_072 + 2_701)}]})).unwrap();
    let packed = zstd::encode_all(&json[..], 3).unwrap();
    let client = reqwest::Client::new();
    let send = |body: Vec<u8>, coding: Option<&'static str>| {
        let mut req = client.post(format!("http://{addr}/codex/v1/responses")).header("content-type", "application/json").body(body);
        if let Some(c) = coding {
            req = req.header("content-encoding", c);
        }
        req.send()
    };
    let r = send(packed.clone(), Some("zstd")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("response.completed"));
    {
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "sent once compressed, once plain");
        assert_eq!((got[0].0.as_deref(), got[0].1 == packed), (Some("zstd"), true));
        assert_eq!((got[1].0.as_deref(), got[1].1 == json), (None, true), "the same JSON, decoded");
    }
    // Counted once, as served.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let p = shared.snapshot();
    let a = p.account("a").unwrap();
    assert_eq!((a.total.requests, a.total.input_tokens, a.total.output_tokens), (1, 9, 2));

    // Refused plain too: the client gets the 400, after two tries.
    got.lock().unwrap().clear();
    refuse_plain.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = send(packed.clone(), Some("zstd")).await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.text().await.unwrap(), r#"{"detail":"Bad Request"}"#);
    assert_eq!(got.lock().unwrap().len(), 2);
    // Sent plain and refused: once.
    got.lock().unwrap().clear();
    let r = send(json.clone(), None).await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(got.lock().unwrap().len(), 1);
}

/// A fake Claude for the claims below: `/v1/messages` serves an account
/// once it's been reset (else a 429 naming its 5-hour meter), and names
/// the host it was sent to; its usage read says so too, its grants
/// `grants` (those not used); each claim is written down (organization,
/// grant, request id), answered by `answer` after `delay`.
struct Claims {
    reset: Mutex<Vec<String>>,
    made: Mutex<Vec<(String, String, String)>>,
    hosts: Mutex<Vec<String>>,
}

type Answer = Arc<dyn Fn(&str, &str, usize) -> (u16, Value) + Send + Sync>;

fn claims_fake(state: Arc<Claims>, grants: Vec<(&'static str, &'static str)>, delay: std::time::Duration, answer: Answer) -> Router {
    let (s1, s2, s3) = (state.clone(), state.clone(), state.clone());
    Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let s = s1.clone();
                async move {
                    s.hosts.lock().unwrap().push(h.get("host").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned());
                    if s.reset.lock().unwrap().contains(&token(&h)) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap()
                    } else {
                        Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3600).to_string())
                            .body(Body::from("{}"))
                            .unwrap()
                    }
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>, body: axum::body::Bytes| {
                let (s, answer) = (s2.clone(), answer.clone());
                async move {
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    let (grant, request) = (v["grant_id"].as_str().unwrap().to_owned(), v["request_id"].as_str().unwrap().to_owned());
                    let n = {
                        let mut made = s.made.lock().unwrap();
                        made.push((org.clone(), grant.clone(), request));
                        made.iter().filter(|m| m.0 == org).count()
                    };
                    tokio::time::sleep(delay).await;
                    let (status, v) = answer(&org, &grant, n);
                    if v["result"] == "reset" {
                        // (Its account's token: the organization's name, "org-" taken off.)
                        s.reset.lock().unwrap().push(org.trim_start_matches("org-").to_owned());
                    }
                    (StatusCode::from_u16(status).unwrap(), axum::Json(v))
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| {
                let s = s3.clone();
                let grants = grants.clone();
                async move {
                    let t = token(&h);
                    let reset = s.reset.lock().unwrap().contains(&t);
                    let used: Vec<String> = s.made.lock().unwrap().iter().filter(|m| m.0 == format!("org-{t}") && reset).map(|m| m.1.clone()).collect();
                    let grants: Vec<Value> = grants
                        .iter()
                        .filter(|(who, id)| *who == t && !used.contains(&id.to_string()))
                        .map(|(_, id)| json!({"id": id, "resets_left": 1, "usable_now": true, "use_requires_limit": false, "clears": ["five_hour"], "ends_at": now() + 86400 + id.len() as i64}))
                        .collect();
                    axum::Json(json!({
                        "five_hour": {"utilization": if reset { 1.0 } else { 100.0 }, "resets_at": now() + 18000},
                        "cedar_ember": {"eligible": true, "grants": grants},
                    }))
                }
            }),
        )
}

/// Accounts out of their 5-hour quota, each with the grants named for it
/// in `grants`; a gateway for them in front of `fake`.
async fn out_of_quota(ids: &[&str], grants: &[(&'static str, &'static str)], fake: Router) -> (Arc<Shared>, SocketAddr, String) {
    let base = format!("http://{}", serve(fake).await);
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth"), origin: None };
    let mut saved = Saved::default();
    for (n, id) in ids.iter().enumerate() {
        let mut a = account(id, n as i64 + 1, 18000);
        a.windows[0].used = 100.0;
        a.org = Some(format!("org-{id}"));
        for (who, g) in grants.iter().filter(|(who, _)| who == id) {
            let _ = who;
            a.credits.push(olo_token_pool::model::ResetCredit {
                id: (*g).into(),
                expires_at: Some(now() + 86400 + g.len() as i64 + n as i64),
                remaining: 1,
                usable: true,
                program: Some("cedar_ember".into()),
                clears: vec!["five_hour".into()],
                needs_limit: false,
                ..Default::default()
            });
        }
        saved.accounts.push(a);
        saved.credentials.insert((*id).into(), Credential { access: (*id).into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    }
    let shared = Shared::new(saved, None, up, || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(shared.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (shared, addr, base.trim_start_matches("http://").to_owned())
}

async fn ask_claude(addr: SocketAddr) -> (u16, String) {
    let r = reqwest::Client::new().post(format!("http://{addr}/claude/v1/messages")).json(&json!({"model": "claude-opus-5-5", "messages": []})).send().await.unwrap();
    let who = r.headers().get("x-olo-pool-account").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    (r.status().as_u16(), who)
}

/// Two requests turned away together, every account out: one reset claimed,
/// and both served by it (Codex's review: each claimed one, two credits
/// for what one gave back). Sent to the gateway's own address, each went
/// to the host naming the host's own name, not the gateway's.
#[tokio::test(flavor = "multi_thread")]
async fn two_turned_away_together_claim_one_reset() {
    let state = Arc::new(Claims { reset: Default::default(), made: Default::default(), hosts: Default::default() });
    let fake = claims_fake(state.clone(), vec![("a", "ga"), ("b", "gb")], std::time::Duration::from_millis(300), Arc::new(|_, _, _| (200, json!({"result": "reset"}))));
    let (_shared, addr, host) = out_of_quota(&["a", "b"], &[("a", "ga"), ("b", "gb")], fake).await;
    let (one, two) = tokio::join!(ask_claude(addr), async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        ask_claude(addr).await
    });
    assert_eq!((one, two), ((200, "a".to_owned()), (200, "a".to_owned())));
    let made = state.made.lock().unwrap().clone();
    assert_eq!(made.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["ga"], "one reset for both");
    assert!(state.hosts.lock().unwrap().iter().all(|h| *h == host), "{:?} not {host}", state.hosts.lock().unwrap());
}

/// A handover holds every provider's claims while the new copy starts:
/// one being made is waited for (not past the limit given), and one
/// asked for while they're held, once the copy has retired, isn't made
/// (Codex's third review: a claim under way across a handover could be
/// made again by the new copy).
#[tokio::test(flavor = "multi_thread")]
async fn a_handover_waits_for_a_claim_and_none_starts_across_it() {
    let state = Arc::new(Claims { reset: Default::default(), made: Default::default(), hosts: Default::default() });
    let fake = claims_fake(state.clone(), vec![("a", "ga")], std::time::Duration::from_millis(600), Arc::new(|_, _, _| (200, json!({"result": "reset"}))));
    let (shared, _addr, _host) = out_of_quota(&["a"], &[("a", "ga")], fake).await;
    let claiming = shared.clone();
    let claim = tokio::spawn(async move { claiming.redeem("a").await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // (From a thread of its own, as the kit's upgrade thread is.)
    let s = shared.clone();
    let (soon, later) = tokio::task::spawn_blocking(move || {
        let soon = s.claims_closed(std::time::Duration::from_millis(50)).is_some();
        let later = s.claims_closed(std::time::Duration::from_secs(5));
        (soon, later)
    })
    .await
    .unwrap();
    assert!(!soon, "the gates taken while a claim was under way");
    let held = later.expect("the gates, once the claim was made");
    assert!(claim.await.unwrap().is_ok());
    // Held, the copy retires: a claim asked for now isn't made.
    shared.drain.retire();
    let asked = shared.clone();
    let after = tokio::spawn(async move { asked.redeem("a").await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(held);
    assert!(after.await.unwrap().is_err(), "a claim made by a copy that handed over");
    assert_eq!(state.made.lock().unwrap().len(), 1);
}

/// A claim left unclear (the provider answered 503: it may have gone
/// through): no other account's credit is claimed meanwhile (that could pay
/// twice for one reset), and the same claim, under the same request id, is
/// made again first.
#[tokio::test(flavor = "multi_thread")]
async fn an_unclear_claim_is_made_again_before_any_other() {
    let state = Arc::new(Claims { reset: Default::default(), made: Default::default(), hosts: Default::default() });
    let answer: Answer = Arc::new(|org, _, n| if org == "org-a" && n == 1 { (503, json!({"error": "busy"})) } else { (200, json!({"result": "reset"})) });
    let fake = claims_fake(state.clone(), vec![("a", "ga"), ("b", "gb")], std::time::Duration::ZERO, answer);
    let (shared, addr, _) = out_of_quota(&["a", "b"], &[("a", "ga"), ("b", "gb")], fake).await;
    assert_eq!(ask_claude(addr).await.0, 429);
    assert_eq!(ask_claude(addr).await.0, 429);
    {
        let made = state.made.lock().unwrap();
        assert_eq!(made.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["org-a"], "another account's credit claimed while one claim was unclear");
    }
    assert!(shared.snapshot().account("a").unwrap().redeeming.is_some(), "the unclear claim forgotten");
    // Asked again (by hand here): the same claim, the same id; then served.
    shared.redeem("a").await.unwrap();
    let made = state.made.lock().unwrap().clone();
    assert_eq!(made.len(), 2);
    assert_eq!((&made[0].1, &made[0].2), (&made[1].1, &made[1].2), "made again under another id");
    assert_eq!(ask_claude(addr).await, (200, "a".to_owned()));
    assert!(state.made.lock().unwrap().iter().all(|m| m.0 == "org-a"));
}

/// One account, two grants that would let the request through: the first
/// refused (nothing spent), the second is claimed (Codex's review: the
/// search ended at the first).
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_grant_then_the_same_accounts_next() {
    let state = Arc::new(Claims { reset: Default::default(), made: Default::default(), hosts: Default::default() });
    let answer: Answer = Arc::new(|_, grant, _| if grant == "g1" { (200, json!({"result": "cooldown"})) } else { (200, json!({"result": "reset"})) });
    let fake = claims_fake(state.clone(), vec![("a", "g1"), ("a", "g22")], std::time::Duration::ZERO, answer);
    let (_shared, addr, _) = out_of_quota(&["a"], &[("a", "g1"), ("a", "g22")], fake).await;
    assert_eq!(ask_claude(addr).await, (200, "a".to_owned()));
    let made = state.made.lock().unwrap().clone();
    assert_eq!(made.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["g1", "g22"]);
}
use std::time::Duration;

fn claimed_grants(state: &Claims) -> Vec<String> {
    state.made.lock().unwrap().iter().map(|r| r.1.clone()).collect()
}

fn claims_state() -> Arc<Claims> {
    Arc::new(Claims { reset: Default::default(), made: Default::default(), hosts: Default::default() })
}

/// While a claim is unclear, no credit is claimed by any way in: by hand
/// on another account (Codex's third look: `redeem("b")` claimed B's).
#[tokio::test(flavor = "multi_thread")]
async fn no_claim_by_hand_while_one_is_unclear() {
    let state = claims_state();
    let grants = [("a", "ga"), ("b", "gb")];
    let fake = claims_fake(state.clone(), grants.to_vec(), Duration::ZERO, Arc::new(|org, _, _| if org == "org-a" { (503, json!({})) } else { (200, json!({"result": "reset"})) }));
    let (s, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    assert_eq!(ask_claude(addr).await.0, 429);
    assert!(s.snapshot().account("a").unwrap().redeeming.is_some());
    assert!(s.redeem("b").await.is_err(), "claimed by hand while another was unclear");
    assert_eq!(claimed_grants(&state), ["ga"]);
}

/// An unclear claim that can't be made again now (its sign-in can't be
/// renewed) stops claims, never lets another account's through (Codex's
/// third look: B's was claimed with A's still pending).
#[tokio::test(flavor = "multi_thread")]
async fn an_unclear_claim_that_cant_go_again_stops_claims() {
    let state = claims_state();
    let grants = [("a", "ga"), ("b", "gb")];
    let fake = claims_fake(state.clone(), grants.to_vec(), Duration::ZERO, Arc::new(|_, _, _| (200, json!({"result": "reset"}))));
    let (s, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    {
        let mut p = s.pool.lock().unwrap();
        let a = p.account_mut("a").unwrap();
        a.redeeming = Some(("ga".into(), "prior-id".into()));
        a.redeem_tried = Some(now() - 20);
        let c = p.saved.credentials.get_mut("a").unwrap();
        c.expires_at = Some(now() - 10);
        c.refresh = None;
    }
    assert_eq!(ask_claude(addr).await.0, 429);
    assert!(s.snapshot().account("a").unwrap().redeeming.is_some());
    assert!(claimed_grants(&state).is_empty(), "{:?}", claimed_grants(&state));
}

/// A claim refused as the account wasn't out after all (`not_limited`):
/// read again, that account serves, and no other credit is claimed
/// (Codex's third look: B's was).
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_that_frees_the_account_claims_nothing_more() {
    let state = claims_state();
    let grants = [("a", "ga"), ("b", "gb")];
    let st = state.clone();
    let fake = claims_fake(
        state.clone(),
        grants.to_vec(),
        Duration::ZERO,
        Arc::new(move |org, _, _| {
            if org == "org-a" {
                st.reset.lock().unwrap().push("a".into());
                (200, json!({"result": "not_limited"}))
            } else {
                (200, json!({"result": "reset"}))
            }
        }),
    );
    let (_s, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    assert_eq!(ask_claude(addr).await, (200, "a".to_owned()));
    assert_eq!(claimed_grants(&state), ["ga"]);
}

/// A 429 for a request sent before an account's reset, coming after it,
/// doesn't put the account back at its limit, so no second credit is
/// claimed (Codex's third look: B's was, though A had been reset).
#[tokio::test(flavor = "multi_thread")]
async fn a_rejection_from_before_a_reset_is_stale() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let state = claims_state();
    let grants = [("a", "ga"), ("b", "gb")];
    let fallback = claims_fake(state.clone(), grants.to_vec(), Duration::ZERO, Arc::new(|_, _, _| (200, json!({"result": "reset"}))));
    let (began, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
    let signals = (began.clone(), release.clone());
    let counter = Arc::new(AtomicUsize::new(0));
    let st = state.clone();
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let (began, release) = signals.clone();
                let st = st.clone();
                let n = counter.fetch_add(1, SeqCst);
                async move {
                    let ready = st.reset.lock().unwrap().contains(&token(&h));
                    // (The first, held until another request has had A reset.)
                    if n == 0 {
                        began.notify_one();
                        tokio::time::timeout(Duration::from_secs(4), release.notified()).await.unwrap();
                    }
                    if ready {
                        return axum::Json(json!({})).into_response();
                    }
                    Response::builder()
                        .status(429)
                        .header("anthropic-ratelimit-unified-status", "rejected")
                        .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                        .header("anthropic-ratelimit-unified-reset", (now() + 18000).to_string())
                        .body(Body::from("{}"))
                        .unwrap()
                }
            }),
        )
        .fallback_service(fallback);
    let (s, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    s.pool.lock().unwrap().account_mut("a").unwrap().windows[0].used = 10.0;
    let slow = tokio::spawn(ask_claude(addr));
    tokio::time::timeout(Duration::from_secs(4), began.notified()).await.unwrap();
    assert_eq!(ask_claude(addr).await, (200, "a".to_owned()));
    release.notify_one();
    assert_eq!(slow.await.unwrap(), (200, "a".to_owned()));
    assert_eq!(claimed_grants(&state), ["ga"]);
}

/// A redirect goes back to the client, as Codex would get it (it follows
/// it itself): the pool never follows it, nor sends the request again when
/// where it points can't be reached (Codex's third look: sent twice).
#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_goes_back_to_the_client() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let fake = Router::new().route(
        "/backend-api/codex/responses",
        post(move || {
            count.fetch_add(1, SeqCst);
            async move { Response::builder().status(307).header("Location", "http://127.0.0.1:1/elsewhere").body(Body::empty()).unwrap() }
        }),
    );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    saved.accounts.push(Account::new("a".into(), "a".into(), Provider::Codex));
    saved.credentials.insert("a".into(), cred("sample"));
    let s = Shared::new(saved, None, Upstreams::fake(&base), || {});
    let r = gateway::serve(s, Provider::Codex, "responses".into(), None, hyper::Method::POST, HeaderMap::new(), Default::default(), axum::body::Bytes::from_static(b"{}")).await;
    assert_eq!((r.status().as_u16(), calls.load(SeqCst)), (307, 1));
}

/// Answers with nothing in them end at once: an empty one's connection is
/// kept, a HEAD's (its length the representation's) completes; and usage
/// is counted only from an answer that decoded whole (one cut short of its
/// gzip trailer counts none).
#[tokio::test(flavor = "multi_thread")]
async fn empty_answers_end_and_only_whole_ones_count() {
    use olo_token_pool::model::Outcome;
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(br#"{"usage":{"output_tokens":7}}"#).unwrap();
    let mut cut = enc.finish().unwrap();
    cut.truncate(cut.len() - 8);
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let bytes = cut.clone();
                async move { Response::builder().header("content-encoding", "gzip").header("content-type", "application/json").body(Body::from(bytes)).unwrap() }
            })
            .head(|| async { Response::builder().header("content-length", 47).body(Body::empty()).unwrap() }),
        )
        .route("/v1/empty", post(|| async { Response::builder().header("content-length", 0).body(Body::empty()).unwrap() }));
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    saved.accounts.push(account("a", 1, 18000));
    saved.credentials.insert("a".into(), cred("sample"));
    let s = Shared::new(saved, None, Upstreams::fake(&base), || {});
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(s.clone(), addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let c = reqwest::Client::builder().no_gzip().no_deflate().no_brotli().no_zstd().build().unwrap();
    let url = format!("http://{addr}/claude/v1/messages");
    let last = || {
        let p = s.snapshot();
        p.served.back().map(|v| (v.request.outcome, v.request.usage.output_tokens))
    };
    c.post(format!("http://{addr}/claude/v1/empty")).body("{}").send().await.unwrap().bytes().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(s.exact().idle(), 1, "an empty answer's connection not kept");
    let r = c.head(&url).send().await.unwrap();
    assert_eq!(r.headers().get("content-length").and_then(|v| v.to_str().ok()), Some("47"));
    assert!(r.bytes().await.unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(last(), Some((Outcome::Completed, 0)));
    let r = c.post(&url).body("{}").send().await.unwrap();
    assert!(!r.bytes().await.unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(last().map(|(_, tokens)| tokens), Some(0), "usage counted from an answer that didn't decode whole");
}

/// After a refusal, the account that can serve now serves, whichever ranks
/// first (Codex's third look: the one ranked first had been tried, and the
/// request was answered 429 though B could serve).
#[tokio::test(flavor = "multi_thread")]
async fn after_a_refusal_whoever_can_serve_does() {
    let state = claims_state();
    let st = state.clone();
    let grants = [("b", "gb")];
    let fake = claims_fake(
        state.clone(),
        grants.to_vec(),
        Duration::ZERO,
        Arc::new(move |_, _, _| {
            st.reset.lock().unwrap().push("b".into());
            (200, json!({"result": "not_limited"}))
        }),
    );
    let (s, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    {
        let mut p = s.pool.lock().unwrap();
        let a = p.account_mut("a").unwrap();
        a.windows[0].used = 10.0;
        a.windows[0].resets_at = Some(now() + 7200);
        let c = p.saved.credentials.get_mut("a").unwrap();
        c.expires_at = Some(now() - 1);
        c.refresh = Some("fixture".into());
    }
    assert_eq!(ask_claude(addr).await, (200, "b".to_owned()));
}

/// An answer for a sign-in replaced while it came: its cookies go to the
/// old sign-in's jar, never the new one's (Codex's third look: the new jar
/// got the old answer's cookie).
#[tokio::test(flavor = "multi_thread")]
async fn an_old_sign_ins_cookies_stay_its_own() {
    let (arrived, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
    let signals = (arrived.clone(), release.clone());
    let fake = Router::new().route(
        "/v1/messages",
        post(move || {
            let (arrived, release) = signals.clone();
            async move {
                arrived.notify_one();
                tokio::time::timeout(Duration::from_secs(4), release.notified()).await.unwrap();
                Response::builder().header("set-cookie", "marker=old; Path=/v1").body(Body::from("{}")).unwrap()
            }
        }),
    );
    let (s, addr, _) = out_of_quota(&["a"], &[], fake).await;
    s.pool.lock().unwrap().account_mut("a").unwrap().windows[0].used = 10.0;
    let request = tokio::spawn(ask_claude(addr));
    tokio::time::timeout(Duration::from_secs(4), arrived.notified()).await.unwrap();
    s.reauth("a", cred("replacement"));
    release.notify_one();
    let _ = request.await.unwrap();
    assert_eq!(olo_token_pool::likeness::cookie(&s.jar("a"), "api.anthropic.com", "/v1/messages"), None);
}

/// The account id named escaped (its first character as `1`) is
/// replaced whole, where it is: the person's words that name it untouched
/// (Codex's third look: the escape's last digit was taken for the id's
/// first, and the id came out starting with ':').
#[tokio::test(flavor = "multi_thread")]
async fn an_escaped_account_id_is_replaced_whole() {
    let id = "escaped";
    let (theirs, ours) = ("11111111-2222-3333-4444-555555555555", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let fake = Router::new().route("/v1/messages", post(|body: axum::body::Bytes| async move { body }));
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    saved.accounts.push(account(id, 1, 100));
    saved.credentials.insert(id.into(), cred("fixture"));
    let s = Shared::new(saved, None, Upstreams::fake(&base), || {});
    s.keep_capture(
        id,
        olo_token_pool::capture::Capture {
            provider: Provider::Claude,
            version: "0".into(),
            installed: String::new(),
            account: id.into(),
            at: now(),
            requests: vec![],
            account_uuid: Some(ours.into()),
            hellos: vec![],
        },
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = gateway::router(s, addr.port());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let inner = format!(r#"{{"account_uuid":"1{}"}}"#, &theirs[1..]);
    let body = json!({"metadata": {"user_id": inner}, "messages": [theirs]}).to_string();
    let v: Value = reqwest::Client::new().post(format!("http://{addr}/claude/v1/messages")).header("content-type", "application/json").body(body).send().await.unwrap().json().await.unwrap();
    let named: Value = serde_json::from_str(v["metadata"]["user_id"].as_str().unwrap()).unwrap();
    assert_eq!(named["account_uuid"], ours);
    assert_eq!(v["messages"][0], theirs);
}

/// A capture's client is started with every descriptor of the pool's
/// marked to close on exec, not closed before it: closed, the channel by
/// which a failed start is told went too, and the start looked to work
/// (Codex's third look: `spawn` was Ok and the child aborted).
#[test]
fn a_failed_start_is_told_with_descriptors_marked() {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/usr/bin/true");
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Err(std::io::Error::from_raw_os_error(libc::EINVAL))
        });
    }
    assert_eq!(cmd.spawn().err().and_then(|e| e.raw_os_error()), Some(libc::EINVAL));
}

/// Each account's own reads go on connections of its own, a capture or
/// not (Codex's third look: two accounts' quota reads shared one).
#[tokio::test(flavor = "multi_thread")]
async fn each_accounts_reads_on_its_own_connection() {
    use axum::extract::ConnectInfo;
    let seen = Arc::new(Mutex::new(Vec::<(String, SocketAddr)>::new()));
    let log = seen.clone();
    let fake = Router::new().route(
        "/api/oauth/usage",
        get(move |ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push((token(&headers), peer));
                axum::Json(json!({"five_hour": {"utilization": 10.0, "resets_at": now() + 7200}}))
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, fake.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    let mut saved = Saved::default();
    for id in ["a", "b"] {
        saved.accounts.push(account(id, 1, 3600));
        saved.credentials.insert(id.into(), cred(id));
    }
    let s = Shared::new(saved, None, Upstreams::fake(&format!("http://{addr}")), || {});
    s.sync(Some("a")).await;
    s.sync(Some("b")).await;
    let rows = seen.lock().unwrap().clone();
    let socket = |who: &str| rows.iter().filter(|(t, _)| t == who).map(|(_, p)| *p).collect::<std::collections::HashSet<_>>();
    assert!(socket("a").is_disjoint(&socket("b")), "two accounts' reads on one connection: {rows:?}");
}
