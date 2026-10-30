//! The real clients through the pool's proxy, signed in with fake
//! subscriptions, against a fake provider: never a real account, and off
//! the network (`OLO_POOL_NO_TUNNEL`: any host but the providers' is
//! refused). Ignored unless asked for, with the client to run:
//!
//! ```text
//! CLAUDE_BIN=$(readlink -f ~/.local/bin/claude) CODEX_BIN=$(readlink -f ~/.local/bin/codex) \
//!   cargo test -p olo-token-pool --test clients -- --ignored --nocapture
//! ```

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
};

use axum::{Router, body::Body, http::HeaderMap, response::Response, routing::post};
use serde_json::json;
use olo_token_pool::{
    authority::Authority,
    gateway::Shared,
    intercept,
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};

/// A Messages stream as Anthropic sends it, saying `text`.
fn claude_stream(text: &str) -> String {
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_pool", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 12, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 6}}),
        json!({"type": "message_stop"}),
    ];
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect()
}

async fn serve(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real Claude Code (CLAUDE_BIN)"]
async fn claude_code_on_its_subscription_through_the_proxy() {
    let Some(bin) = std::env::var_os("CLAUDE_BIN") else { panic!("set CLAUDE_BIN to Claude Code's binary") };
    // SAFETY: set before anything else in this test reads it.
    unsafe { std::env::set_var("OLO_POOL_NO_TUNNEL", "1") };
    // The fake Anthropic: who signed each model request in, and its betas.
    let seen: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let s2 = seen.clone();
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let seen = s2.clone();
                async move {
                    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                    seen.lock().unwrap().push((get("authorization"), get("anthropic-beta")));
                    Response::builder().header("content-type", "text/event-stream").body(Body::from(claude_stream("Hello from the pool."))).unwrap()
                }
            }),
        )
        .fallback(|| async { Response::builder().status(404).body(Body::from("{}")).unwrap() });
    let up = serve(fake).await;

    let mut saved = Saved::default();
    let mut a = Account::new("pooled".into(), "pooled".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("pooled".into(), Credential { access: "pool-account-token".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let dir = tempfile::Builder::new().prefix("olo-pool-claude-").tempdir_in("/var/tmp").unwrap();
    let authority = Arc::new(Authority::ephemeral());
    let ca = dir.path().join("pool-ca.pem");
    std::fs::write(&ca, &authority.cert_pem).unwrap();
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority, 0, Arc::new(move |p| s.serves(p)), intercept::Who::Clients).await.unwrap();

    // Claude Code as the pool leaves it: its own subscription sign-in
    // (a fake one), the proxy and the pool's certificate in its settings.
    let config = dir.path().join("claude");
    let settings = json!({"env": {"HTTPS_PROXY": intercept::url(proxy.port()), "NODE_EXTRA_CA_CERTS": ca.display().to_string(), "NO_PROXY": "localhost,127.0.0.1,::1"}});
    write(&config.join("settings.json"), &settings.to_string());
    let creds = json!({"claudeAiOauth": {"accessToken": "their-own-token", "refreshToken": "their-refresh", "expiresAt": (now() + 3600) * 1000, "scopes": ["user:inference", "user:profile"], "subscriptionType": "max"}});
    write(&config.join(".credentials.json"), &creds.to_string());
    write(&dir.path().join("home/.claude.json"), &json!({"hasCompletedOnboarding": true}).to_string());

    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["-p", "Say hello.", "--output-format", "text"])
        .env_clear()
        .env("HOME", dir.path().join("home"))
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("PATH", "/usr/bin:/bin")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output()).await.expect("Claude Code finished").unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("stdout: {stdout}\nstderr: {stderr}");
    let seen = seen.lock().unwrap().clone();
    println!("model requests: {seen:?}");
    assert!(stdout.contains("Hello from the pool."), "Claude Code didn't answer through the pool");
    // The pool's account signed its model requests in, not Claude Code's
    // own sign-in; and it ran as on a subscription (the OAuth beta its own).
    assert!(!seen.is_empty() && seen.iter().all(|(auth, _)| auth == "Bearer pool-account-token"), "{seen:?}");
    assert!(seen.iter().any(|(_, beta)| beta.contains("oauth-2025-04-20")), "{seen:?}");
}

/// An unsigned JWT with `claims` (Codex reads its sign-in's claims; the
/// fake provider checks nothing).
fn jwt(claims: serde_json::Value) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    format!("{}.{}.sig", B64.encode(br#"{"alg":"none","typ":"JWT"}"#), B64.encode(claims.to_string()))
}

/// A Responses stream as ChatGPT's Codex backend sends it, saying `text`.
fn codex_stream(text: &str) -> String {
    let events = [
        json!({"type": "response.created", "response": {"id": "resp_pool"}}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "role": "assistant", "id": "msg_pool", "content": [{"type": "output_text", "text": text, "annotations": []}]}}),
        json!({"type": "response.completed", "response": {"id": "resp_pool", "usage": {"input_tokens": 10, "input_tokens_details": null, "output_tokens": 5, "output_tokens_details": null, "total_tokens": 15}}}),
    ];
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real Codex (CODEX_BIN)"]
async fn codex_on_its_subscription_through_the_proxy() {
    let Some(bin) = std::env::var_os("CODEX_BIN") else { panic!("set CODEX_BIN to Codex's binary") };
    // SAFETY: set before anything else in this test reads it.
    unsafe { std::env::set_var("OLO_POOL_NO_TUNNEL", "1") };
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
    let s2 = seen.clone();
    let fake = Router::new()
        .route(
            "/backend-api/codex/responses",
            post(move |h: HeaderMap| {
                let seen = s2.clone();
                async move {
                    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                    seen.lock().unwrap().push(("/responses".into(), get("authorization"), get("chatgpt-account-id")));
                    Response::builder().header("content-type", "text/event-stream").body(Body::from(codex_stream("Hello from the pool."))).unwrap()
                }
            }),
        )
        // Which workspace a sign-in is, and where its backend is (Codex
        // asks before a turn), as ChatGPT answers for Codex's own.
        .route(
            "/backend-api/wham/accounts/check",
            axum::routing::get(|| async {
                axum::Json(json!({
                    "accounts": [{"id": "their-account", "plan_type": "pro", "workspace_backend_origin": "https://chatgpt.com", "account_routing_override": "NO_CONSTRAINT"}],
                    "account_ordering": ["their-account"],
                    "default_account_id": "their-account",
                }))
            }),
        )
        .fallback(|uri: axum::http::Uri| async move {
            println!("fake: {uri} (not answered)");
            Response::builder().status(404).body(Body::from("{}")).unwrap()
        });
    let up = serve(fake).await;

    let mut saved = Saved::default();
    let mut a = Account::new("pooled".into(), "pooled".into(), Provider::Codex);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("pooled".into(), Credential { access: "pool-account-token".into(), refresh: None, expires_at: Some(now() + 3600), account_id: Some("pool-account".into()) });
    let shared = Shared::new(saved, None, Upstreams::fake(&format!("http://{up}")), || {});
    let dir = tempfile::Builder::new().prefix("olo-pool-codex-").tempdir_in("/var/tmp").unwrap();
    let authority = Arc::new(Authority::ephemeral());
    // The system's roots and the pool's, as Codex's setting replaces the roots.
    let roots = ["/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/certs/ca-certificates.crt"].iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let bundle = dir.path().join("pool-ca-bundle.pem");
    std::fs::write(&bundle, format!("{roots}\n{}", authority.cert_pem)).unwrap();
    let s = shared.clone();
    let proxy = intercept::serve(shared.clone(), authority, 0, Arc::new(move |p| s.serves(p)), intercept::Who::Clients).await.unwrap();

    // Codex as the pool leaves it: its own ChatGPT sign-in (a fake one), the
    // proxy and the bundle in the `.env` it reads as it starts.
    let home = dir.path().join("codex");
    write(&home.join(".env"), &format!("HTTPS_PROXY={}\nSSL_CERT_FILE={}\nNO_PROXY=localhost,127.0.0.1,::1\n", intercept::url(proxy.port()), bundle.display()));
    let auth = json!({"https://api.openai.com/auth": {"chatgpt_plan_type": "pro", "chatgpt_account_id": "their-account", "chatgpt_user_id": "user-1"}, "email": "someone@example.com", "exp": now() + 3600});
    let tokens = json!({"id_token": jwt(auth.clone()), "access_token": jwt(auth), "refresh_token": "their-refresh", "account_id": "their-account"});
    write(&home.join("auth.json"), &json!({"OPENAI_API_KEY": null, "tokens": tokens, "last_refresh": chrono_now()}).to_string());
    write(&home.join("config.toml"), "model = \"gpt-5-codex\"\n");

    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // Its output goes to files, so a hang still says where it was.
    let (out_path, err_path) = (dir.path().join("stdout"), dir.path().join("stderr"));
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["exec", "--skip-git-repo-check", "Say hello."])
        .current_dir(&work)
        .env_clear()
        .env("HOME", dir.path())
        .env("CODEX_HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("RUST_LOG", std::env::var("CODEX_LOG").unwrap_or_else(|_| "warn".into()))
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(90), child.wait()).await;
    let (stdout, stderr) = (std::fs::read_to_string(&out_path).unwrap_or_default(), std::fs::read_to_string(&err_path).unwrap_or_default());
    let tail = |s: &str| s.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    println!("stdout: {}\nstderr: {}", tail(&stdout), tail(&stderr));
    assert!(done.is_ok(), "Codex didn't finish in 90 s");
    let seen = seen.lock().unwrap().clone();
    println!("model requests: {seen:?}");
    assert!(stdout.contains("Hello from the pool."), "Codex didn't answer through the pool");
    assert!(!seen.is_empty() && seen.iter().all(|(_, auth, acct)| auth == "Bearer pool-account-token" && acct == "pool-account"), "{seen:?}");
    // Its body comes compressed: the model is still seen.
    assert!(shared.pool.lock().unwrap().served.iter().any(|s| s.request.model == "gpt-5-codex"), "the pool didn't see Codex's model");
}

/// Now, as Codex writes its last refresh.
fn chrono_now() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}
