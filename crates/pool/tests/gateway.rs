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

/// Usage credits come last (Ben, 2026-11-26: "instead of switching to
/// another account or using a reset token pool is letting the clients
/// use usage credits first"). Claude goes on answering an account whose
/// allowance is out when it has usage credits: 200, its headers saying
/// the meter is "rejected" and overage "allowed". The pool took only a
/// 429 for a meter running out, so such an account stayed the one chosen
/// and every request after was paid for.
///
/// "a" is chosen first and answers so. That one answer goes back (it is
/// answered, and paid for); from the next request on "b" serves. When
/// "b" is refused, "a"'s reset credit is claimed before any credits are
/// spent. With the reset spent and both out again, and only then, the
/// request goes to "a" on its credits. And when "a" refuses too, the
/// client is told every account is out.
#[tokio::test(flavor = "multi_thread")]
async fn usage_credits_come_after_another_account_and_a_reset() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // How each account answers now: 0 within its allowance, 1 from usage
    // credits, 2 refused. And how many answers were paid for from credits.
    let state = Arc::new([AtomicUsize::new(1), AtomicUsize::new(0)]);
    let (paid_for, resets) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (s1, s2, n1, n2) = (state.clone(), state.clone(), paid_for.clone(), resets.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let (state, paid_for) = (s1.clone(), n1.clone());
                async move {
                    let who = if token(&h) == "a" { 0 } else { 1 };
                    let limits = |status: &str| {
                        Response::builder()
                            .header("anthropic-ratelimit-unified-status", status)
                            .header("anthropic-ratelimit-unified-overage-status", "allowed")
                            .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3600).to_string())
                    };
                    match state[who].load(Ordering::SeqCst) {
                        0 => limits("allowed").header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap(),
                        1 => {
                            paid_for.fetch_add(1, Ordering::SeqCst);
                            limits("rejected").header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap()
                        }
                        _ => limits("rejected").status(429).body(Body::from("{}")).unwrap(),
                    }
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>| {
                let (state, resets) = (s2.clone(), n2.clone());
                async move {
                    assert_eq!(org, "org-a");
                    resets.fetch_add(1, Ordering::SeqCst);
                    state[0].store(0, Ordering::SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        )
        .route("/api/oauth/usage", get(|| async { axum::Json(json!({"seven_day": {"utilization": 40.0, "resets_at": now() + 90000}})) }));
    // (Both with allowance as far as the pool knows; "a" first, the older.)
    let (shared, addr, _) = out_of_quota(&["a", "b"], &[("a", "g1")], fake).await;
    shared.pool.lock().unwrap().saved.accounts.iter_mut().for_each(|a| a.windows[0].used = 10.0);
    let paid_until = |id: &str| shared.snapshot().account(id).unwrap().paid_until("opus", now());
    // 1. "a" answers, from its usage credits: once, and the pool knows.
    assert_eq!(ask_claude(addr).await, (200, "a".into()));
    assert_eq!(paid_for.load(Ordering::SeqCst), 1);
    assert!(paid_until("a").is_some_and(|until| until > now() + 3000), "{:?}", paid_until("a"));
    assert!(shared.snapshot().account("a").unwrap().windows.iter().any(|w| w.name == "five_hour" && w.used >= 100.0), "the meter that ran out is marked so");
    // 2. From here on "b", which has allowance: nothing more paid for.
    for _ in 0..3 {
        assert_eq!(ask_claude(addr).await, (200, "b".into()));
    }
    assert_eq!((paid_for.load(Ordering::SeqCst), resets.load(Ordering::SeqCst)), (1, 0));
    // 3. "b" is refused: "a"'s reset is claimed, before any credits. "a"
    // answers within its allowance again, and is no longer taken as paid.
    state[1].store(2, Ordering::SeqCst);
    assert_eq!(ask_claude(addr).await, (200, "a".into()));
    assert_eq!((paid_for.load(Ordering::SeqCst), resets.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(paid_until("a"), None);
    // 4. "a" runs out again, its reset spent: its first answer from
    // credits (as in 1), then, "b" still refused and nothing to claim,
    // the last resort: "a" on its credits.
    state[0].store(1, Ordering::SeqCst);
    assert_eq!(ask_claude(addr).await, (200, "a".into()));
    assert_eq!(paid_for.load(Ordering::SeqCst), 2);
    assert!(paid_until("a").is_some());
    assert_eq!(ask_claude(addr).await, (200, "a".into()));
    assert_eq!((paid_for.load(Ordering::SeqCst), resets.load(Ordering::SeqCst)), (3, 1), "on credits only once nothing else could serve");
    // 5. "b" has allowance again: back to "b" at once, though "a" was the
    // one in use.
    state[1].store(0, Ordering::SeqCst);
    {
        let mut p = shared.pool.lock().unwrap();
        let b = p.account_mut("b").unwrap();
        b.cooldown_until = None;
        b.windows.iter_mut().for_each(|w| w.used = 10.0);
    }
    assert_eq!(ask_claude(addr).await, (200, "b".into()));
    assert_eq!(paid_for.load(Ordering::SeqCst), 3);
    // 5b. On a host's pool that keeps "a" (`kept_on: None`): "b" refused
    // again and "a" has a reset credit "g2". Because this host IS "a"'s
    // keeper, it claims "g2" itself instead of spending usage credits or
    // waiting for a desktop that may be off.
    state[1].store(2, Ordering::SeqCst);
    shared.set_hosted();
    {
        let mut p = shared.pool.lock().unwrap();
        let a = p.account_mut("a").unwrap();
        a.redeem_tried = None;
        a.credits = vec![olo_token_pool::model::ResetCredit {
            id: "g2".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["five_hour".into()],
            ..Default::default()
        }];
    }
    // (A host's pool answers through its proxy alone, which hands the
    // request to the gateway so.)
    let through_the_proxy = || async {
        let mut h = HeaderMap::new();
        h.insert("content-type", "application/json".parse().unwrap());
        h.insert("authorization", "Bearer the-clients-own".parse().unwrap());
        let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
        let r = gateway::serve(shared.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
        let (status, who) = (r.status().as_u16(), r.headers().get("x-olo-pool-account").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned());
        let said = axum::body::to_bytes(r.into_body(), 1 << 20).await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        (status, who, (), said)
    };
    let (status, who, _, said) = through_the_proxy().await;
    assert_eq!((status, who.as_str()), (200, "a"), "hosted keeper must claim its own reset: {said}");
    assert_eq!((paid_for.load(Ordering::SeqCst), resets.load(Ordering::SeqCst)), (3, 2));
    assert_eq!(paid_until("a"), None);
    // 5c. On a borrower pool whose account is kept elsewhere (`kept_on: Some`)
    // and whose keeper can't be reached: "a" runs out onto credits again and
    // has a reset credit waiting on its keeper. No usage credits are spent
    // ahead of it: the client is told all are out and a reset waits. With no
    // reset anywhere, the last resort as before.
    state[0].store(1, Ordering::SeqCst);
    let (status, who, ..) = through_the_proxy().await;
    assert_eq!((status, who.as_str()), (200, "a"));
    assert_eq!(paid_for.load(Ordering::SeqCst), 4);
    assert!(paid_until("a").is_some());
    {
        let mut p = shared.pool.lock().unwrap();
        let a = p.account_mut("a").unwrap();
        a.kept_on = Some("other-host".into());
        a.redeem_tried = None;
        a.credits = vec![olo_token_pool::model::ResetCredit {
            id: "g3".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["five_hour".into()],
            ..Default::default()
        }];
    }
    let (status, _, _, said) = through_the_proxy().await;
    assert!(status == 429 && said.contains("reset credit waits"), "{status}: {said}");
    assert_eq!((paid_for.load(Ordering::SeqCst), resets.load(Ordering::SeqCst)), (4, 2), "neither credits spent nor a reset claimed by an unreachable borrower");
    shared.pool.lock().unwrap().account_mut("a").unwrap().credits.clear();
    let (status, who, ..) = through_the_proxy().await;
    assert_eq!((status, who.as_str()), (200, "a"));
    assert_eq!(paid_for.load(Ordering::SeqCst), 5);
    // 6. Both refuse outright ("a"'s credits ran out too): the client is
    // told every account is out.
    state[0].store(2, Ordering::SeqCst);
    state[1].store(2, Ordering::SeqCst);
    let (status, _, _, said) = through_the_proxy().await;
    assert!(status == 429 && said.contains("out of quota") && !said.contains("reset credit waits"), "{status}: {said}");
    assert_eq!(paid_for.load(Ordering::SeqCst), 5);
    // (And the status says which account is answered from credits.)
    let status: Value = reqwest::get(format!("http://{addr}/pool/status")).await.unwrap().json().await.unwrap();
    let a = status["accounts"].as_array().unwrap().iter().find(|a| a["id"] == "a").unwrap().clone();
    assert!(a["paid_until"].as_i64().is_some_and(|until| until > now()), "{a}");
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

/// End-to-end reproduction of why `olo-pool-host` missed Claude Code resets
/// (Ben, 2026-11-27):
/// 1) Hosted keeper (`set_hosted()`, `kept_on: None`) had no capture, and
///    Anthropic rejects `cedar_ember` with `eligible: false, surface` unless
///    `User-Agent` is `claude-cli/... (external, cli)`.
/// 2) Grants with `use_requires_limit: true` can be cached with
///    `usable_now: false, unavailable_reason: "needs_limit"` or absent from a
///    stale cache until exhaustion refreshes credits.
/// 3) Missing `account.org` is healed from `/api/oauth/profile` before
///    claiming `/api/organizations/{org}/reset_rate_limits`.
#[tokio::test(flavor = "multi_thread")]
async fn hosted_keeper_discovers_claude_resets_heals_org_and_claims_on_exhaustion() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    let reset = Arc::new(AtomicBool::new(false));
    let redeemed = Arc::new(AtomicUsize::new(0));
    let (r1, r2, r3, n) = (reset.clone(), reset.clone(), reset.clone(), redeemed.clone());
    let is_cli_ua = |h: &HeaderMap| h.get("user-agent").and_then(|v| v.to_str().ok()).is_some_and(|ua| ua.starts_with("claude-cli/") && ua.contains("(external, cli)"));
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let reset = r1.clone();
                async move {
                    if reset.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":5,"output_tokens":7}}"#)).unwrap()
                    } else {
                        Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "seven_day")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3 * 86400).to_string())
                            .body(Body::from("{}"))
                            .unwrap()
                    }
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| {
                let reset = r2.clone();
                async move {
                    let is_reset = reset.load(SeqCst);
                    let cedar = if !is_cli_ua(&h) {
                        json!({"eligible": false, "ineligible_reason": "surface", "grants": []})
                    } else if is_reset {
                        json!({"eligible": true, "at_limit": false, "grants": []})
                    } else {
                        json!({
                            "eligible": true,
                            "at_limit": false,
                            "grants": [{
                                "id": "full-reset-2611",
                                "type": "full",
                                "label": "Full Reset",
                                "resets_left": 1,
                                "paused": false,
                                "usable_now": false,
                                "use_requires_limit": true,
                                "unavailable_reason": "needs_limit",
                                "clears": ["five_hour", "seven_day"],
                                "ends_at": now() + 86400
                            }]
                        })
                    };
                    axum::Json(json!({
                        "seven_day": {"utilization": if is_reset { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": cedar
                    }))
                }
            }),
        )
        .route(
            "/api/oauth/profile",
            get(move |h: HeaderMap| async move {
                let ua = h.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
                assert!(ua.starts_with("claude-code/"), "profile request must carry Claude Code UA, got {ua:?}");
                assert!(!h.contains_key("anthropic-beta"), "profile request must not carry anthropic-beta");
                axum::Json(json!({"organization": {"uuid": "org-healed"}, "account": {"uuid": "acct-1"}}))
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>, h: HeaderMap, body: axum::body::Bytes| {
                let (reset, n) = (r3.clone(), n.clone());
                async move {
                    assert!(is_cli_ua(&h), "reset request must carry Claude CLI UA");
                    assert_eq!(h.get_all("content-type").iter().count(), 1, "uncaptured reset POST must carry exactly one Content-Type header");
                    assert_eq!(org, "org-healed");
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!((v["grant_id"].as_str(), v["program"].as_str()), (Some("full-reset-2611"), Some("cedar_ember")));
                    n.fetch_add(1, SeqCst);
                    reset.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "Work".into(), Provider::Claude);
    a.created = 1;
    a.org = None; // missing org -> must heal via /api/oauth/profile
    a.windows = vec![olo_token_pool::model::Window { name: "seven_day".into(), label: "Weekly".into(), used: 95.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("ok"));
    let shared = Shared::new(saved, None, Upstreams::fake(&base), || {});
    shared.set_hosted();

    // Send through proxy path (as `olo-pool-host` does) with no capture and empty cached credits.
    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(shared.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r.status(), 200, "hosted keeper must discover, heal org, claim needs_limit reset, and succeed");
    assert_eq!(redeemed.load(SeqCst), 1);
    let p = shared.snapshot();
    let a = p.account("a").unwrap();
    assert_eq!(a.org.as_deref(), Some("org-healed"));
    assert!(a.redeeming.is_none());
    assert!(a.credits.is_empty());
}

/// If a Claude account is missing `org` and `/api/oauth/profile` cannot heal it,
/// `redeem_locked` must fail cleanly before setting `a.redeeming` so the error
/// does not strand a pending claim and block every other Claude account.
#[tokio::test(flavor = "multi_thread")]
async fn missing_org_does_not_strand_redeeming_and_allows_next_account_to_claim() {
    let state = claims_state();
    let grants = [("a", "ga"), ("b", "gb")];
    let fake = claims_fake(state.clone(), grants.to_vec(), Duration::ZERO, Arc::new(|_, _, _| (200, json!({"result": "reset"})))).route("/api/oauth/profile", get(|| async { StatusCode::NOT_FOUND }));
    let (shared, addr, _) = out_of_quota(&["a", "b"], &grants, fake).await;
    shared.pool.lock().unwrap().account_mut("a").unwrap().org = None;

    assert_eq!(ask_claude(addr).await, (200, "b".to_owned()));
    assert_eq!(claimed_grants(&state), ["gb"]);
    assert!(shared.snapshot().account("a").unwrap().redeeming.is_none(), "missing org must never leave a stranded redeeming lock");
}

/// If every cached reset credit candidate fails definitively (e.g. refused),
/// `redeem_kept_locked` performs one bounded stale-credit refresh so another
/// account with a newly available credit can still rescue the request.
#[tokio::test(flavor = "multi_thread")]
async fn refused_cached_candidate_triggers_stale_refresh_and_claims_next_account() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let b_reset = Arc::new(AtomicBool::new(false));
    let (br1, br2) = (b_reset.clone(), b_reset.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let br = br1.clone();
                async move {
                    if token(&h) == "b" && br.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":2,"output_tokens":3}}"#)).unwrap()
                    } else {
                        Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "seven_day")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3 * 86400).to_string())
                            .body(Body::from("{}"))
                            .unwrap()
                    }
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| {
                let br = br2.clone();
                async move {
                    let who = token(&h);
                    let is_reset = who == "b" && br.load(SeqCst);
                    let grants = if who == "b" && !is_reset { json!([{"id": "gb-fresh", "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400}]) } else { json!([]) };
                    axum::Json(json!({
                        "seven_day": {"utilization": if is_reset { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": grants}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>| {
                let b_reset = b_reset.clone();
                async move {
                    if org == "org-a" {
                        (StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"message": "grant already spent"}}))).into_response()
                    } else {
                        b_reset.store(true, SeqCst);
                        axum::Json(json!({"result": "reset"})).into_response()
                    }
                }
            }),
        );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![olo_token_pool::model::Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits = vec![olo_token_pool::model::ResetCredit {
        id: "ga-stale".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: true,
        program: Some("cedar_ember".into()),
        clears: vec!["seven_day".into()],
        ..Default::default()
    }];
    let mut b = Account::new("b".into(), "B".into(), Provider::Claude);
    b.created = 2;
    b.org = Some("org-b".into());
    b.windows = vec![olo_token_pool::model::Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    // b starts with empty cached credits -> discovered only if stale refresh runs after a's refusal!
    saved.accounts.extend([a, b]);
    saved.credentials.insert("a".into(), cred("a"));
    saved.credentials.insert("b".into(), cred("b"));
    let shared = Shared::new(saved, None, Upstreams::fake(&base), || {});

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(shared.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r.status(), 200, "refusal on cached account a must refresh stale credits on b and succeed");
    assert_eq!(r.headers().get("x-olo-pool-account").and_then(|v| v.to_str().ok()), Some("B"));
}

/// A pre-send TCP connection refusal before bytes leave the pool must classify
/// as `Failure::NotSent` (`Claimed::NotNow`) and clear a newly persisted
/// `a.redeeming` instead of stranding the provider claim lock.
#[tokio::test(flavor = "multi_thread")]
async fn presend_connect_error_does_not_strand_redeeming() {
    // Bind and immediately drop a loopback listener so the port refuses connections immediately.
    let closed_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![olo_token_pool::model::Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits = vec![olo_token_pool::model::ResetCredit {
        id: "ga".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: true,
        program: Some("cedar_ember".into()),
        clears: vec!["seven_day".into()],
        ..Default::default()
    }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("a"));
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://127.0.0.1:{closed_port}")), || {});

    let err = shared.redeem_credit("a", None, gateway::Why::Asked).await.expect_err("connect refusal must fail");
    assert!(matches!(err, gateway::Claimed::NotNow(_)), "pre-send connect failure must be NotNow, got {err:?}");
    assert!(shared.snapshot().account("a").unwrap().redeeming.is_none(), "pre-send connect error must not strand a.redeeming");
}

/// A narrow `five_hour` reset must NOT clear `a.paid` when `a.paid.meter` is
/// `"seven_day"` (nor clear `seven_day` itself).
#[tokio::test(flavor = "multi_thread")]
async fn partial_five_hour_reset_preserves_seven_day_paid_mark() {
    let fake = Router::new().route("/api/organizations/{org}/reset_rate_limits", post(|| async { axum::Json(json!({"result": "reset"})) })).route(
        "/api/oauth/usage",
        get(|| async {
            axum::Json(json!({
                "five_hour": {"utilization": 0.0, "resets_at": now() + 5 * 3600},
                "seven_day": {"utilization": 100.0, "resets_at": now() + 3 * 86400},
                "cedar_ember": {"eligible": true, "grants": []}
            }))
        }),
    );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![
        olo_token_pool::model::Window { name: "five_hour".into(), label: "5-hour".into(), used: 100.0, resets_at: Some(now() + 3 * 3600), models: vec![] },
        olo_token_pool::model::Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] },
    ];
    a.paid = Some(olo_token_pool::model::PaidFor { until: now() + 3 * 86400, meter: "seven_day".into() });
    a.credits = vec![olo_token_pool::model::ResetCredit {
        id: "g-5h".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: true,
        program: Some("cedar_ember".into()),
        clears: vec!["five_hour".into()],
        ..Default::default()
    }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("a"));
    let shared = Shared::new(saved, None, Upstreams::fake(&base), || {});

    shared.redeem_credit("a", Some("g-5h"), gateway::Why::Asked).await.unwrap();
    let snap = shared.snapshot();
    let after = snap.account("a").unwrap();
    assert_eq!(after.paid.as_ref().map(|p| p.meter.as_str()), Some("seven_day"), "five_hour reset must not clear seven_day paid mark");
    assert_eq!(after.windows.iter().find(|w| w.name == "five_hour").unwrap().used, 0.0);
    assert_eq!(after.windows.iter().find(|w| w.name == "seven_day").unwrap().used, 100.0);
}

/// When Codex returns 429 with `Retry-After` only (no window headers), the
/// cooldown has no exhausted window in cache; `refresh_credits_if_stale` must
/// also fetch Codex usage so the exhausted weekly window + credit are seen and
/// claimed automatically.
#[tokio::test(flavor = "multi_thread")]
async fn codex_cooldown_without_exhausted_window_fetches_usage_and_claims_reset() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let reset = Arc::new(AtomicBool::new(false));
    let (r1, r2, r3) = (reset.clone(), reset.clone(), reset.clone());
    let fake = Router::new()
        .route(
            "/backend-api/codex/responses",
            post(move || {
                let reset = r1.clone();
                async move {
                    if reset.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":4,"output_tokens":6}}"#)).unwrap()
                    } else {
                        Response::builder().status(429).header("retry-after", "3600").body(Body::from("{}")).unwrap()
                    }
                }
            }),
        )
        .route(
            "/backend-api/wham/usage",
            get(move || {
                let reset = r2.clone();
                async move {
                    let is_reset = reset.load(SeqCst);
                    axum::Json(json!({
                        "rate_limit": {
                            "secondary_window": {
                                "used_percent": if is_reset { 0.0 } else { 100.0 },
                                "limit_window_seconds": 604800,
                                "reset_at": now() + 3 * 86400
                            }
                        }
                    }))
                }
            }),
        )
        .route(
            "/backend-api/wham/rate-limit-reset-credits",
            get(move || {
                let reset = r3.clone();
                async move {
                    if reset.load(SeqCst) {
                        axum::Json(json!({"available_count": 0, "credits": []}))
                    } else {
                        axum::Json(json!({
                            "available_count": 1,
                            "credits": [{"id": "cx-1", "expires_at": now() + 86400}]
                        }))
                    }
                }
            }),
        )
        .route(
            "/backend-api/wham/rate-limit-reset-credits/consume",
            post(move || {
                let reset = reset.clone();
                async move {
                    reset.store(true, SeqCst);
                    axum::Json(json!({"code": "reset"}))
                }
            }),
        );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    let mut a = Account::new("cx".into(), "Codex".into(), Provider::Codex);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("cx".into(), Credential { access: "cx-tok".into(), refresh: Some("r".into()), expires_at: Some(now() + 3600), account_id: Some("acct-cx".into()) });
    let shared = Shared::new(saved, None, Upstreams::fake(&base), || {});

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "gpt-6.2-astra", "input": []}).to_string());
    let r = gateway::serve(shared, Provider::Codex, "responses".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r.status(), 200, "Codex cooldown with Retry-After must fetch usage + reset credit and succeed");
}

/// Regression for Astra R2 #7: an older slow `sync` response finishing after a
/// newer `sync` discovered a grant must NOT overwrite the newer read's credits.
#[tokio::test(flavor = "multi_thread")]
async fn older_concurrent_sync_cannot_overwrite_newer_credit_read() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use tokio::sync::Notify;

    let credit_calls = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let (cc, fs, rf) = (credit_calls.clone(), first_started.clone(), release_first.clone());

    let fake = Router::new().route(
        "/api/oauth/usage",
        get(move |axum::extract::RawQuery(q): axum::extract::RawQuery| {
            let (cc, fs, rf) = (cc.clone(), fs.clone(), rf.clone());
            async move {
                if q.as_deref().is_some_and(|s| s.contains("cedar_ember")) {
                    let n = cc.fetch_add(1, SeqCst);
                    if n == 0 {
                        fs.notify_one();
                        rf.notified().await;
                        // Older slow response returns empty grants.
                        return axum::Json(json!({"seven_day": {"utilization": 50.0, "resets_at": now() + 3 * 86400}, "cedar_ember": {"eligible": true, "grants": []}}));
                    }
                    // Newer response returns 1 fresh grant.
                    return axum::Json(json!({
                        "seven_day": {"utilization": 50.0, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": [{"id": "g-new", "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400}]}
                    }));
                }
                axum::Json(json!({"seven_day": {"utilization": 50.0, "resets_at": now() + 3 * 86400}}))
            }
        }),
    );
    let base = format!("http://{}", serve(fake).await);
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), cred("a"));
    let shared = Shared::new(saved, None, Upstreams::fake(&base), || {});

    let s1 = shared.clone();
    let slow = tokio::spawn(async move { s1.sync(Some("a")).await });
    first_started.notified().await;
    // Complete a newer sync while the older one is still waiting on its credits call.
    shared.sync(Some("a")).await;
    assert_eq!(shared.snapshot().account("a").unwrap().credits.len(), 1);
    // Release the older response: it MUST NOT wipe the newer grant!
    release_first.notify_one();
    slow.await.unwrap();
    assert_eq!(shared.snapshot().account("a").unwrap().credits.len(), 1, "older sync must never overwrite newer credits");
}
