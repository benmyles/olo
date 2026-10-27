//! The gateway against a fake provider: never a real account or service.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use tabula_token_pool::{
    gateway::{self, Shared},
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};

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
    a.windows.push(tabula_token_pool::model::Window { name: "five_hour".into(), label: "5-hour".into(), used: 10.0, resets_at: Some(now() + resets), models: vec![] });
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
    let up = Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth") };
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
        .header("authorization", "Bearer tabula-pool")
        .json(&json!({"model": "claude-opus-5-5", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // The refreshed "b" answered, after "a" was out.
    assert_eq!(r.headers()["x-tabula-pool-account"], "b");
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
