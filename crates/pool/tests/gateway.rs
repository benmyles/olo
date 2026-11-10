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
    a.credits.push(olo_token_pool::model::ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 2, usable: true, program: Some("cedar_ember".into()) });
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
/// on a network that went is (`olo_net`), and the gateway asks again on
/// the network there is now, so the client gets its answer, not an error;
/// with no network at all, it waits for one, then asks.
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

    // Changed under it: asked again, on the new network.
    let started = std::time::Instant::now();
    let pending = tokio::spawn(ask());
    until(1, asked.clone()).await;
    slow.store(false, SeqCst);
    let cut = olo_net::pretend_change(true, Some(up_addr.port()));
    assert!(cut >= 1, "the gateway's connection upstream was cut");
    let r = pending.await.unwrap();
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
