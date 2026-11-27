//! One pool's accounts on two machines (`gateway::hosted`): the
//! desktop's pool and a Rex host's, against a provider that gives a new
//! refresh token at every renewal and refuses one used twice. Each
//! sign-in is renewed by one pool, whichever asks; the host goes on
//! with the desktop gone; the desktop waits, without signing the
//! account out, while the host can't be reached; and what is given
//! back is whole. Never a real account or service.

use olo_sync::Mutex;
use std::sync::Arc;

use axum::{Router, body::Body, response::Response, routing::post};
use olo_token_pool::{
    gateway::{Lender, Shared},
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use serde_json::json;

/// A provider's renewals: each refresh token good once, each access
/// token an hour.
async fn provider(renewals: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let used: Arc<Mutex<std::collections::HashSet<String>>> = Default::default();
    let app = Router::new().route(
        "/oauth",
        post(move |body: axum::Json<serde_json::Value>| {
            let (renewals, used) = (renewals.clone(), used.clone());
            async move {
                let token = body["refresh_token"].as_str().unwrap_or("").to_owned();
                renewals.lock().unwrap().push(token.clone());
                if !used.lock().unwrap().insert(token.clone()) {
                    return Response::builder().status(400).body(Body::from(r#"{"error":"invalid_grant"}"#)).unwrap();
                }
                Response::builder().body(Body::from(json!({"access_token": format!("fresh-{token}"), "refresh_token": format!("{token}+"), "expires_in": 3600}).to_string())).unwrap()
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    at
}

fn sign_in(of: &Shared, id: &str) -> (String, Option<String>, Option<String>) {
    let p = of.pool.lock().unwrap();
    let c = &p.saved.credentials[id];
    (c.access.clone(), c.refresh.clone(), p.account(id).and_then(|a| a.kept_on.clone()))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_is_kept_by_the_host_and_borrowed_by_the_desktop() {
    let renewals: Arc<Mutex<Vec<String>>> = Default::default();
    let up = Upstreams::fake(&format!("http://{}", provider(renewals.clone()).await));
    let mut saved = Saved::default();
    for (id, refresh) in [("claude-a", "ra"), ("claude-b", "rb")] {
        let mut a = Account::new(id.into(), id.into(), Provider::Claude);
        a.created = 1;
        saved.accounts.push(a);
        // (About to run out: the first thing asked renews it.)
        saved.credentials.insert(id.into(), Credential { access: format!("old-{id}"), refresh: Some(refresh.into()), expires_at: Some(now() + 30), account_id: None });
    }
    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();

    // Carried: the host keeps both from now, and the desktop holds no
    // refresh token once the host has said so.
    let carried = desktop.carry_to("build", "this computer");
    assert!(carried.accounts.iter().all(|a| a.kept_on.as_deref() == Some("build") && a.credential.as_ref().is_some_and(|c| c.refresh.is_some())));
    assert_eq!(sign_in(&desktop, "claude-a").2.as_deref(), Some("build"), "marked before it is sent");
    let status = host.carried(carried).unwrap();
    assert_eq!(status["sign_ins"].as_array().unwrap().iter().filter(|s| s["kept"] == true).count(), 2);
    assert!(desktop.handed_over("build", &status));
    assert_eq!(sign_in(&desktop, "claude-a"), ("old-claude-a".into(), None, Some("build".into())));
    assert_eq!(sign_in(&host, "claude-a"), ("old-claude-a".into(), Some("ra".into()), None));

    // The desktop needs a newer one: it asks the host, which renews.
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let lender: Lender = {
        let (host, reached) = (host.clone(), reached.clone());
        Arc::new(move |keeper, id, at_least| {
            let (host, reached) = (host.clone(), reached.clone());
            Box::pin(async move {
                assert_eq!(keeper, "build");
                if !reached.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("no route to the host".to_owned());
                }
                host.lend(&id, at_least).await
            })
        })
    };
    desktop.set_lender(Some(lender));
    let c = desktop.renewed("claude-a", Provider::Claude).await.expect("borrowed from the host");
    assert_eq!((c.access.as_str(), c.refresh.as_deref()), ("fresh-ra", None));
    assert_eq!(sign_in(&host, "claude-a"), ("fresh-ra".into(), Some("ra+".into()), None), "the host has the rotated token");
    assert_eq!(renewals.lock().unwrap().as_slice(), ["ra"]);
    // Asked again, both have what they have: nothing is renewed.
    desktop.renewed("claude-a", Provider::Claude).await.unwrap();
    host.renewed("claude-a", Provider::Claude).await.unwrap();
    assert_eq!(renewals.lock().unwrap().as_slice(), ["ra"]);

    // The host renews by itself, the desktop gone.
    let c = host.renewed("claude-b", Provider::Claude).await.expect("the host renews its own");
    assert_eq!(c.access, "fresh-rb");
    // And the desktop, the host out of reach, with a sign-in that has
    // run out: it waits, and the account isn't signed out.
    reached.store(false, std::sync::atomic::Ordering::Relaxed);
    desktop.pool.lock().unwrap().saved.credentials.get_mut("claude-b").unwrap().expires_at = Some(now() - 5);
    let waits = desktop.renewed("claude-b", Provider::Claude).await.expect_err("nothing to use");
    assert!(waits.to_string().contains("kept on build"), "{waits}");
    assert_eq!(desktop.pool.lock().unwrap().account("claude-b").unwrap().signed_out, None);
    assert_eq!(renewals.lock().unwrap().as_slice(), ["ra", "rb"], "the desktop spent nothing");
    // One that still lasts is used as it is meanwhile.
    assert_eq!(desktop.renewed("claude-a", Provider::Claude).await.unwrap().access, "fresh-ra");

    // Given back whole (asked twice: the same), the desktop keeps it
    // again and renews it itself; the host is told and lets its
    // refresh token go.
    let back = host.give_back("claude-a", "this computer").await.unwrap();
    assert_eq!(host.give_back("claude-a", "this computer").await.unwrap().refresh, back.refresh);
    assert_eq!(back.refresh.as_deref(), Some("ra+"));
    desktop.took_back("claude-a", back);
    assert_eq!(sign_in(&desktop, "claude-a"), ("fresh-ra".into(), Some("ra+".into()), None));
    assert_eq!(sign_in(&host, "claude-a").2.as_deref(), Some("this computer"), "the host renews it no more");
    desktop.pool.lock().unwrap().saved.credentials.get_mut("claude-a").unwrap().expires_at = Some(now() + 30);
    assert_eq!(desktop.renewed("claude-a", Provider::Claude).await.unwrap().access, "fresh-ra+");
    // (Kept here now, it isn't handed over again by a carrying that
    // says so: the person took it back. Here it is carried as lent.)
    let mut carried = desktop.carry_to("elsewhere", "this computer");
    carried.you = "build".into();
    let status = host.carried(carried).unwrap();
    let a = status["sign_ins"].as_array().unwrap().iter().find(|s| s["id"] == "claude-a").unwrap().clone();
    assert_eq!((a["kept"].as_bool(), sign_in(&host, "claude-a").1), (Some(false), None), "its refresh token is gone from the host");
    assert_eq!(sign_in(&host, "claude-a").0, "fresh-ra+", "and it has the newer access token, lent");

    // No refresh token was ever spent twice.
    let spent = renewals.lock().unwrap().clone();
    assert_eq!(spent.len(), spent.iter().collect::<std::collections::HashSet<_>>().len(), "{spent:?}");
}

/// What is carried is the host's accounts: one no longer carried goes,
/// a new one comes, a pause and a name follow, and the host's own
/// reading of an account (its use) stays.
#[tokio::test(flavor = "multi_thread")]
async fn what_is_carried_is_what_the_host_has() {
    let up = Upstreams::fake("http://127.0.0.1:9");
    let mut saved = Saved::default();
    for id in ["one", "two"] {
        saved.accounts.push(Account::new(id.into(), id.into(), Provider::Codex));
        saved.credentials.insert(id.into(), Credential { access: id.into(), refresh: Some(format!("r-{id}")), expires_at: Some(now() + 86_400), account_id: Some("acct".into()) });
    }
    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    let status = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    desktop.handed_over("build", &status);
    assert_eq!(status["accounts"].as_array().unwrap().len(), 2);
    host.pool.lock().unwrap().account_mut("one").unwrap().total.requests = 7;

    desktop.remove("two").unwrap();
    desktop.rename("one", "the first");
    desktop.set_enabled("one", false);
    desktop.add(Account::new("three".into(), "three".into(), Provider::Claude), Credential { access: "three".into(), refresh: Some("r-three".into()), expires_at: None, account_id: None }).unwrap();
    let status = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    let names: Vec<(&str, bool)> = status["accounts"].as_array().unwrap().iter().map(|a| (a["name"].as_str().unwrap(), a["enabled"].as_bool().unwrap())).collect();
    assert_eq!(names, [("the first", false), ("three", true)]);
    let p = host.pool.lock().unwrap();
    assert_eq!(p.account("one").unwrap().total.requests, 7, "its own count of what it served");
    // (Carried again without a refresh token, once handed over: the
    // host's own, which it keeps, stays.)
    assert_eq!(p.saved.credentials["one"].refresh.as_deref(), Some("r-one"));
    assert!(!p.saved.credentials.contains_key("two"));
    assert_eq!(p.saved.credentials["three"].refresh.as_deref(), Some("r-three"));
}

/// Promoting a host from borrower (already holding lent `{ access: X, refresh: None }`)
/// to keeper (receiving `{ access: X, refresh: Some(RT) }` with the exact same access token)
/// MUST accept the refresh token, clear `signed_out`, and report `kept: true`; replaying an
/// older carry after rotation MUST NOT overwrite the keeper's rotated refresh token.
#[tokio::test(flavor = "multi_thread")]
async fn promoting_borrower_to_keeper_with_same_access_token_keeps_refresh_and_resists_replay() {
    let up = Upstreams::fake("http://127.0.0.1:9");
    let mut saved = Saved::default();
    let mut a = Account::new("claude-1".into(), "Work".into(), Provider::Claude);
    a.created = 1;
    a.kept_on = Some("old-host".into());
    saved.accounts.push(a);
    let exp = now() + 3600;
    saved.credentials.insert("claude-1".into(), Credential { access: "access-X".into(), refresh: None, expires_at: Some(exp), account_id: None });

    let desktop = Shared::new(saved, None, up.clone(), || {});
    let ovh = Shared::new(Saved::default(), None, up, || {});
    ovh.set_hosted();

    // Step 1: OVH starts as a borrower while old-host is keeper -> receives lent access-X, refresh: None.
    let status = ovh.carried(desktop.carry_to("ovh", "this computer")).unwrap();
    assert_eq!(status["sign_ins"][0]["kept"], false);
    assert_eq!(sign_in(&ovh, "claude-1"), ("access-X".into(), None, Some("old-host".into())));

    // Simulate OVH previously failing renewal and marking signed_out.
    ovh.pool.lock().unwrap().account_mut("claude-1").unwrap().signed_out = Some("No refresh token: sign in again.".into());

    // Step 2: Account moves from old-host -> desktop -> OVH with the SAME access-X + refresh RT!
    desktop.took_back_for("claude-1", Credential { access: "access-X".into(), refresh: Some("RT-1".into()), expires_at: Some(exp), account_id: None }, Some("ovh"));
    let promote = desktop.carry_to("ovh", "this computer");
    assert_eq!(desktop.unaccepted_handovers("ovh"), vec!["claude-1".to_owned()]);
    let status = ovh.carried(promote.clone()).unwrap();
    assert_eq!(status["sign_ins"][0]["kept"], true, "OVH must accept refresh token even when access matches");
    assert_eq!(sign_in(&ovh, "claude-1"), ("access-X".into(), Some("RT-1".into()), None));
    assert_eq!(ovh.pool.lock().unwrap().account("claude-1").unwrap().signed_out, None, "signed_out cleared on handover");

    // Desktop accepts handover and drops its local refresh token.
    assert!(desktop.handed_over("ovh", &status));
    assert!(desktop.unaccepted_handovers("ovh").is_empty());
    assert_eq!(sign_in(&desktop, "claude-1"), ("access-X".into(), None, Some("ovh".into())));

    // Step 3: OVH rotates its refresh token to RT-2 / access-Y. Replaying the older carry must NOT clobber it!
    ovh.pool.lock().unwrap().saved.credentials.insert("claude-1".into(), Credential { access: "access-Y".into(), refresh: Some("RT-2".into()), expires_at: Some(exp + 7200), account_id: None });
    let _ = ovh.carried(promote).unwrap();
    assert_eq!(sign_in(&ovh, "claude-1"), ("access-Y".into(), Some("RT-2".into()), None), "replay must not overwrite rotated keeper token");
}

/// When the keeper explicitly rejects lending with an auth error ("No refresh token: sign in again."):
/// 1) `borrowed()` surfaces that auth error instead of claiming the host is unreachable;
/// 2) if the desktop still holds the refresh token (unfinished handover), it triggers a host re-carry;
/// 3) HTTP `serve()` returns 502 with the sign-in error instead of 429 "out of quota";
/// 4) `borrow_ahead()` stops immediately once `drain.retire()` is called.
#[tokio::test(flavor = "multi_thread")]
async fn keeper_auth_error_surfaces_as_502_triggers_recarry_and_retired_skips_borrow_ahead() {
    use olo_token_pool::gateway::Hosting;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct SpyHosting(Arc<Mutex<Vec<(String, String)>>>);
    impl Hosting for SpyHosting {
        fn list(&self) -> serde_json::Value {
            json!({"hosts": []})
        }
        fn act(self: Arc<Self>, id: &str, what: &str) -> Result<(), String> {
            self.0.lock().unwrap().push((id.to_owned(), what.to_owned()));
            Ok(())
        }
    }

    let up = Upstreams::fake("http://127.0.0.1:9");

    // Case A: Desktop still holds refresh token (handover incomplete) -> triggers sync re-carry!
    let mut saved = Saved::default();
    let mut a = Account::new("c1".into(), "Work".into(), Provider::Claude);
    a.kept_on = Some("ovh".into());
    saved.accounts.push(a);
    saved.credentials.insert("c1".into(), Credential { access: "expired".into(), refresh: Some("RT-kept-local".into()), expires_at: Some(now() - 10), account_id: None });
    let desktop = Shared::new(saved, None, up.clone(), || {});
    let actions: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    desktop.set_hosting(Some(Arc::new(SpyHosting(actions.clone()))));
    let lend_calls = Arc::new(AtomicUsize::new(0));
    let lc = lend_calls.clone();
    desktop.set_lender(Some(Arc::new(move |_, _, _| {
        lc.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err("No refresh token: sign in again.".to_owned()) })
    })));
    let err = desktop.renewed("c1", Provider::Claude).await.unwrap_err().to_string();
    assert!(err.contains("waiting for sync"), "{err}");
    assert_eq!(actions.lock().unwrap().as_slice(), &[("ovh".to_owned(), "sync".to_owned())]);
    assert_eq!(lend_calls.load(Ordering::SeqCst), 0, "should trigger re-carry instead of looping Ask::Lend");

    // Case B: Retired draining copy never runs borrow_ahead.
    desktop.drain.retire();
    desktop.borrow_ahead().await;
    assert_eq!(lend_calls.load(Ordering::SeqCst), 0);

    // Case C: Normal borrower whose keeper explicitly returns "No refresh token: sign in again." ->
    // HTTP gateway returns 502 with the actual keeper auth error, NOT 429 "out of quota".
    let mut saved2 = Saved::default();
    let mut a2 = Account::new("c2".into(), "Work".into(), Provider::Claude);
    a2.kept_on = Some("ovh".into());
    saved2.accounts.push(a2);
    saved2.credentials.insert("c2".into(), Credential { access: "expired".into(), refresh: None, expires_at: Some(now() - 10), account_id: None });
    let gw = Shared::new(saved2, None, up, || {});
    gw.set_lender(Some(Arc::new(|_, _, _| Box::pin(async { Err("No refresh token: sign in again.".to_owned()) }))));
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    olo_token_pool::gateway::start(gw.clone(), port).unwrap();
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/claude/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-sonnet-4-6","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::BAD_GATEWAY, "must return 502, not 429 out-of-quota: {body}");
    assert!(body.contains("No refresh token: sign in again"), "must surface keeper auth error: {body}");
    assert!(!body.contains("out of quota"), "must never falsely claim out of quota: {body}");
}

/// Keeper-owned reset claiming across a desktop borrower and a Rex host keeper:
/// 1) Stale keeper cache: when the borrower exhausts all accounts while the
///    keeper still thinks they have headroom (even synced 5 s ago), delegating
///    `ClaimAsk::For` makes the keeper confirm usage with the provider and
///    claim a reset once.
/// 2) Concurrent exhaustion on host and desktop: both hit 429 at once; the
///    keeper's `claims_gate` serializes them so exactly ONE reset credit is
///    claimed across both machines, and both requests succeed (200).
/// 3) Manual `desktop.redeem(id)` delegates `ClaimAsk::Account` to the keeper.
#[tokio::test(flavor = "multi_thread")]
async fn keeper_claims_resets_for_both_host_and_desktop_without_double_claiming() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, ClaimAsk, Claimer},
        model::{ResetCredit, Window},
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

    let exhausted: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(["a".into(), "b".into()].into_iter().collect()));
    let claims: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let delay_ms = Arc::new(AtomicU64::new(0));
    let (e1, e2, e3) = (exhausted.clone(), exhausted.clone(), exhausted.clone());
    let (c1, c2) = (claims.clone(), claims.clone());
    let d1 = delay_ms.clone();

    let tok = |h: &HeaderMap| h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_owned();
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let exhausted = e1.clone();
                async move {
                    let who = tok(&h);
                    if exhausted.lock().unwrap().contains(&who) {
                        Response::builder()
                            .status(429)
                            .header("anthropic-ratelimit-unified-status", "rejected")
                            .header("anthropic-ratelimit-unified-representative-claim", "seven_day")
                            .header("anthropic-ratelimit-unified-reset", (now() + 3 * 86400).to_string())
                            .body(Body::from("{}"))
                            .unwrap()
                    } else {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":3,"output_tokens":4}}"#)).unwrap()
                    }
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| {
                let (exhausted, claims) = (e2.clone(), c1.clone());
                async move {
                    let who = tok(&h);
                    let out = exhausted.lock().unwrap().contains(&who);
                    let used_ids: Vec<String> = claims.lock().unwrap().iter().filter(|(o, _)| o == &format!("org-{who}")).map(|(_, g)| g.clone()).collect();
                    let all_grants: Vec<&str> = if who == "a" { vec!["ga1", "ga2"] } else { vec!["gb1", "gb2"] };
                    let grants: Vec<serde_json::Value> = all_grants
                        .into_iter()
                        .filter(|g| !used_ids.iter().any(|u| u == g))
                        .map(|g| json!({"id": g, "resets_left": 1, "usable_now": true, "use_requires_limit": false, "clears": ["seven_day"], "ends_at": now() + 86400}))
                        .collect();
                    axum::Json(json!({
                        "seven_day": {"utilization": if out { 100.0 } else { 5.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": grants},
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>, body: axum::body::Bytes| {
                let (exhausted, claims, delay_ms) = (e3.clone(), c2.clone(), d1.clone());
                async move {
                    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let grant = v["grant_id"].as_str().unwrap().to_owned();
                    let ms = delay_ms.load(SeqCst);
                    if ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    }
                    claims.lock().unwrap().push((org.clone(), grant));
                    exhausted.lock().unwrap().remove(org.trim_start_matches("org-"));
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });
    let up = Upstreams::fake(&format!("http://{up_addr}"));

    let mut saved = Saved::default();
    for (n, (id, g)) in [("a", "ga1"), ("b", "gb1")].into_iter().enumerate() {
        let mut a = Account::new(id.into(), id.into(), Provider::Claude);
        a.created = n as i64 + 1;
        a.org = Some(format!("org-{id}"));
        a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        a.credits = vec![ResetCredit {
            id: g.into(),
            expires_at: Some(now() + 86400 + n as i64),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];
        saved.accounts.push(a);
        saved.credentials.insert(id.into(), Credential { access: id.into(), refresh: Some(format!("r-{id}")), expires_at: Some(now() + 3600), account_id: None });
    }
    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();

    // Carry accounts to host so host is keeper (`kept_on: None`) and desktop is borrower (`kept_on: Some("build")`).
    let status = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &status));
    // Give host stale 50% windows synced 5 seconds ago to prove `redeem_for_remote` confirms against the provider!
    {
        let mut p = host.pool.lock().unwrap();
        for (id, g) in [("a", "ga1"), ("b", "gb1")] {
            let a = p.account_mut(id).unwrap();
            a.org = Some(format!("org-{id}"));
            a.synced = Some(now() - 5);
            a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 50.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
            a.credits = vec![ResetCredit {
                id: g.into(),
                expires_at: Some(now() + 86400),
                remaining: 1,
                usable: true,
                program: Some("cedar_ember".into()),
                clears: vec!["seven_day".into()],
                ..Default::default()
            }];
        }
    }
    let claimer: Claimer = {
        let host = host.clone();
        Arc::new(move |keeper, ask| {
            let host = host.clone();
            Box::pin(async move {
                assert_eq!(keeper, "build");
                match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                }
            })
        })
    };
    desktop.set_claimer(Some(claimer));

    let ask_on = |pool: Arc<Shared>| async move {
        let mut h = HeaderMap::new();
        h.insert("content-type", "application/json".parse().unwrap());
        let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
        let r = gateway::serve(pool, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
        let who = r.headers().get("x-olo-pool-account").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
        (r.status().as_u16(), who)
    };

    // 1. Desktop hits exhaustion while host has stale 50% cache -> host merges observed/confirms 100%, claims 1 reset, desktop returns 200!
    assert_eq!(ask_on(desktop.clone()).await, (200, "a".to_owned()));
    assert_eq!(claims.lock().unwrap().len(), 1, "exactly one reset claimed for stale keeper cache");

    // 2. Concurrent exhaustion on BOTH host and desktop -> serialized under keeper's claims_gate, still only ONE reset!
    // Desktop even hits a live 429 on `a` while host is mid-claim on `a` and still succeeds via AlreadyReady { reset_at }.
    exhausted.lock().unwrap().insert("a".into());
    exhausted.lock().unwrap().insert("b".into());
    {
        let mut hp = host.pool.lock().unwrap();
        for id in ["a", "b"] {
            hp.account_mut(id).unwrap().windows[0].used = 100.0;
        }
    }
    {
        let mut dp = desktop.pool.lock().unwrap();
        // Leave `a` looking 50% on desktop so desktop hits a live 429 while host is mid-claim!
        dp.account_mut("a").unwrap().windows[0].used = 50.0;
        dp.account_mut("b").unwrap().windows[0].used = 100.0;
    }
    host.sync(Some("a")).await;
    host.sync(Some("b")).await;
    delay_ms.store(200, SeqCst);
    let (on_host, on_desktop) = tokio::join!(ask_on(host.clone()), async {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        ask_on(desktop.clone()).await
    });
    assert_eq!(on_host.0, 200, "host request succeeded");
    assert_eq!(on_desktop.0, 200, "desktop request succeeded after concurrent keeper reset");
    assert_eq!(on_host.1, on_desktop.1, "both served by the single reset account");
    let snap = claims.lock().unwrap().clone();
    assert_eq!(snap.len(), 2, "concurrent host + desktop exhaustion must claim only 1 additional reset, not 2: {snap:?}");

    // 3. Manual redeem from desktop delegates `ClaimAsk::Account` to keeper.
    delay_ms.store(0, SeqCst);
    let other = if on_host.1 == "a" { "b" } else { "a" };
    desktop.redeem(other).await.expect("manual desktop redeem delegates to keeper");
    assert_eq!(claims.lock().unwrap().len(), 3);
}

/// Regression for Fable C1 / Astra C2:
/// When a borrower gets 429 on `five_hour` while `/api/oauth/usage` omits
/// `five_hour` (or returns 429 itself), the borrower carries `ObservedLimit`
/// and keeper returns distinct `RemoteClaim::{Redeemed, AlreadyReady}`:
/// (A) with a reset credit, keeper redeems once and borrower returns 200;
/// (B) without a reset credit (and usage returning 429), borrower terminates
/// promptly with 429 in bounded calls — NEVER an infinite loop.
#[tokio::test(flavor = "multi_thread")]
async fn borrower_five_hour_429_unlisted_in_usage_does_not_loop() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, ClaimAsk, Claimer},
        model::{ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    // Case A: 5-hour 429 omitted from /api/oauth/usage, grant clears five_hour -> claims once & succeeds.
    let reset = Arc::new(AtomicBool::new(false));
    let msg_hits = Arc::new(AtomicUsize::new(0));
    let usage_hits = Arc::new(AtomicUsize::new(0));
    let claims = Arc::new(AtomicUsize::new(0));
    let (r1, r2, r3, m1, u1, c1) = (reset.clone(), reset.clone(), reset.clone(), msg_hits.clone(), usage_hits.clone(), claims.clone());
    let fake_a = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let (reset, m) = (r1.clone(), m1.clone());
                async move {
                    m.fetch_add(1, SeqCst);
                    if reset.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
            "/api/oauth/usage",
            get(move || {
                let (reset, u) = (r2.clone(), u1.clone());
                async move {
                    u.fetch_add(1, SeqCst);
                    // Omit `five_hour` completely (exact Anthropic Max plan shape)!
                    let grants = if reset.load(SeqCst) {
                        json!([])
                    } else {
                        json!([{"id": "g-5h", "resets_left": 1, "usable_now": false, "use_requires_limit": true, "unavailable_reason": "needs_limit", "clears": ["five_hour"], "ends_at": now() + 86400}])
                    };
                    axum::Json(json!({
                        "seven_day": {"utilization": 20.0, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": grants}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let (reset, c) = (r3.clone(), c1.clone());
                async move {
                    c.fetch_add(1, SeqCst);
                    reset.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_a = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake_a).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 20.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });
    let desktop = Shared::new(saved, None, up_a.clone(), || {});
    let host = Shared::new(Saved::default(), None, up_a, || {});
    host.set_hosted();
    let status = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &status));
    let claimer: Claimer = {
        let host = host.clone();
        Arc::new(move |_, ask| {
            let host = host.clone();
            Box::pin(async move {
                match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                }
            })
        })
    };
    desktop.set_claimer(Some(claimer));

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(desktop.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r.status(), 200);
    assert_eq!(claims.load(SeqCst), 1);
    assert!(msg_hits.load(SeqCst) <= 2, "expected <= 2 /v1/messages calls, got {}", msg_hits.load(SeqCst));
    assert!(usage_hits.load(SeqCst) <= 4, "expected <= 4 /api/oauth/usage calls, got {}", usage_hits.load(SeqCst));

    // Case B: No credits left AND /api/oauth/usage returns 429 -> must terminate promptly with 429!
    let msg_b = Arc::new(AtomicUsize::new(0));
    let usage_b = Arc::new(AtomicUsize::new(0));
    let (mb, ub) = (msg_b.clone(), usage_b.clone());
    let fake_b = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let mb = mb.clone();
                async move {
                    mb.fetch_add(1, SeqCst);
                    Response::builder()
                        .status(429)
                        .header("anthropic-ratelimit-unified-status", "rejected")
                        .header("anthropic-ratelimit-unified-representative-claim", "five_hour")
                        .header("anthropic-ratelimit-unified-reset", (now() + 3600).to_string())
                        .body(Body::from("{}"))
                        .unwrap()
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move || {
                let ub = ub.clone();
                async move {
                    ub.fetch_add(1, SeqCst);
                    Response::builder().status(429).body(Body::from("{}")).unwrap()
                }
            }),
        );
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_b = Upstreams::fake(&format!("http://{}", l2.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l2, fake_b).await.unwrap() });
    let mut saved_b = Saved::default();
    let mut ab = Account::new("ab".into(), "AB".into(), Provider::Claude);
    ab.created = 1;
    ab.org = Some("org-ab".into());
    ab.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 10.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    ab.credits = vec![ResetCredit { id: "stale".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, clears: vec!["seven_day".into()], ..Default::default() }];
    saved_b.accounts.push(ab);
    saved_b.credentials.insert("ab".into(), Credential { access: "ab".into(), refresh: Some("rab".into()), expires_at: Some(now() + 3600), account_id: None });
    let desk_b = Shared::new(saved_b, None, up_b.clone(), || {});
    let host_b = Shared::new(Saved::default(), None, up_b, || {});
    host_b.set_hosted();
    let st_b = host_b.carried(desk_b.carry_to("build", "this computer")).unwrap();
    assert!(desk_b.handed_over("build", &st_b));
    let claimer_b: Claimer = {
        let host_b = host_b.clone();
        Arc::new(move |_, ask| {
            let host_b = host_b.clone();
            Box::pin(async move {
                match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host_b.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host_b.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                }
            })
        })
    };
    desk_b.set_claimer(Some(claimer_b));
    let r2 = gateway::serve(desk_b, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r2.status(), 429);
    assert_eq!(msg_b.load(SeqCst), 1, "must never loop /v1/messages when no reset clears five_hour");
    assert!(usage_b.load(SeqCst) <= 3, "must never hammer /api/oauth/usage: got {}", usage_b.load(SeqCst));
}

/// Real Unix-socket `host::serve_at` wire test for Astra C1 / Fable I1:
/// 1) Keeper redeems 1 reset over Unix socket, then simulated dropped SSH reply
///    returns `Unclear` to borrower -> borrower persists `PendingRemoteClaim`,
///    holds off spending a second reset or paid fallback, and reconciles the
///    exact `op_id` on the next attempt -> total upstream resets == 1!
/// 2) An `op_id` lost before reaching keeper returns `NotNow` on
///    `reconcile_only: true` -> clears pending -> fresh claim proceeds!
#[tokio::test(flavor = "multi_thread")]
async fn lost_remote_claim_reply_reconciles_via_op_id_over_unix_socket() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, ClaimAsk, Claimed, Claimer},
        host::{self, Ask},
        model::{RemoteClaim, ResetCredit, Window},
    };
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    let redeemed = Arc::new(AtomicBool::new(false));
    let post_count = Arc::new(AtomicUsize::new(0));
    let (r1, r2, r3, pc) = (redeemed.clone(), redeemed.clone(), redeemed.clone(), post_count.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let r = r1.clone();
                async move {
                    if r.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
            get(move || {
                let r = r2.clone();
                async move {
                    let ok = r.load(SeqCst);
                    axum::Json(json!({
                        "seven_day": {"utilization": if ok { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": if ok { json!([]) } else { json!([{"id": "g1", "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400}]) }}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let (r, pc) = (r3.clone(), pc.clone());
                async move {
                    pc.fetch_add(1, SeqCst);
                    r.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits =
        vec![ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() }];
    // Also mark `a` as paid to verify pending remote claim holds off usage credits!
    a.paid = Some(olo_token_pool::model::PaidFor { until: now() + 3 * 86400, meter: "seven_day".into() });
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();
    host.set_runtime(tokio::runtime::Handle::current());
    let status = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &status));
    {
        let mut hp = host.pool.lock().unwrap();
        let ha = hp.account_mut("a").unwrap();
        ha.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        ha.credits = vec![ResetCredit {
            id: "g1".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];
    }

    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-test-{}", olo_token_pool::store::new_uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("pool-host.sock");
    host::serve_at(host.clone(), sock.clone()).unwrap();

    let drop_first_reply = Arc::new(AtomicBool::new(true));
    let drop_flag = drop_first_reply.clone();
    let sock_for_claimer = sock.clone();
    let claimer: Claimer = Arc::new(move |_, ask| {
        let (sock, drop_flag) = (sock_for_claimer.clone(), drop_flag.clone());
        Box::pin(async move {
            let wire_ask = match ask {
                ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                    Ask::Redeem { id: Some(id), credit, provider: None, family: None, op_id: Some(op_id), timeout_secs, reconcile_only, observed }
                }
                ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                    Ask::Redeem { id: None, credit: None, provider: Some(provider), family: Some(family), op_id: Some(op_id), timeout_secs, reconcile_only, observed }
                }
            };
            let v: serde_json::Value = tokio::task::spawn_blocking(move || {
                let mut stream = UnixStream::connect(&sock).unwrap();
                writeln!(stream, "{}", serde_json::to_string(&wire_ask).unwrap()).unwrap();
                stream.flush().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                serde_json::from_str(&line).unwrap()
            })
            .await
            .unwrap();
            // Simulate SSH connection dropping right after the keeper completed and replied!
            if drop_flag.swap(false, SeqCst) {
                return Err(Claimed::Unclear("simulated SSH drop after keeper spent credit".into()));
            }
            if let Some(err) = v["error"].as_str() {
                return Err(match v["kind"].as_str() {
                    Some("refused") => Claimed::Refused(err.to_owned()),
                    Some("not_now") => Claimed::NotNow(err.to_owned()),
                    Some("no_record") => Claimed::NoRecord(err.to_owned()),
                    Some("unreachable") => Claimed::Unreachable(err.to_owned()),
                    _ => Claimed::Unclear(err.to_owned()),
                });
            }
            serde_json::from_value::<RemoteClaim>(v).map_err(|e| Claimed::Unclear(e.to_string()))
        })
    });
    desktop.set_claimer(Some(claimer));

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());

    // Attempt 1: keeper spends 1 credit, reply is lost -> borrower persists PendingRemoteClaim & returns 429 ("reset credit waits") instead of using paid fallback!
    let r1 = gateway::serve(desktop.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r1.status(), 429, "unclear remote claim must hold off usage credits");
    assert_eq!(post_count.load(SeqCst), 1, "keeper spent 1 reset credit");
    assert_eq!(desktop.pool.lock().unwrap().saved.pending_remote_claims.len(), 1);

    // Advance pending reconcile throttle so Attempt 2 reconciles the exact op_id with reconcile_only: true.
    desktop.pool.lock().unwrap().saved.pending_remote_claims[0].tried_at = now() - 20;
    let r2 = gateway::serve(desktop.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r2.status(), 200, "reconciling op_id must recover the original Redeemed result and succeed");
    assert_eq!(post_count.load(SeqCst), 1, "reconciling op_id must NEVER spend a second reset credit");
    assert!(desktop.pool.lock().unwrap().saved.pending_remote_claims.is_empty());

    // Part 2: Unknown op_id probed with reconcile_only: true over the wire returns definitive no_record (never spends!).
    let probe = Ask::Redeem {
        id: None,
        credit: None,
        provider: Some(Provider::Claude),
        family: Some("opus".into()),
        op_id: Some("never-reached-keeper".into()),
        timeout_secs: None,
        reconcile_only: true,
        observed: Vec::new(),
    };
    let sock_probe = sock.clone();
    let v: serde_json::Value = tokio::task::spawn_blocking(move || {
        let mut stream = UnixStream::connect(&sock_probe).unwrap();
        writeln!(stream, "{}", serde_json::to_string(&probe).unwrap()).unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(v["kind"].as_str(), Some("no_record"));
    assert_eq!(post_count.load(SeqCst), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression for Fable I2: a slow `Ask::Redeem` on the host control socket
/// must run on a worker thread and never block concurrent `Ask::Status`.
#[tokio::test(flavor = "multi_thread")]
async fn slow_redeem_does_not_block_status_on_host_socket() {
    use olo_token_pool::{
        host::{self, Ask},
        model::{ResetCredit, Window},
    };
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let fake = Router::new()
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                axum::Json(json!({"result": "reset"}))
            }),
        )
        .route(
            "/api/oauth/usage",
            axum::routing::get(|| async { axum::Json(json!({"seven_day": {"utilization": 0.0, "resets_at": now() + 3 * 86400}, "cedar_ember": {"eligible": true, "grants": []}})) }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits =
        vec![ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let host = Shared::new(saved, None, up, || {});
    host.set_hosted();
    host.set_runtime(tokio::runtime::Handle::current());

    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-test-{}", olo_token_pool::store::new_uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("pool-host.sock");
    host::serve_at(host, sock.clone()).unwrap();

    let sock_slow = sock.clone();
    let redeem_task = tokio::task::spawn_blocking(move || {
        let mut stream = UnixStream::connect(&sock_slow).unwrap();
        let ask = Ask::Redeem {
            id: Some("a".into()),
            credit: Some("g1".into()),
            provider: None,
            family: None,
            op_id: Some("slow-1".into()),
            timeout_secs: None,
            reconcile_only: false,
            observed: Vec::new(),
        };
        writeln!(stream, "{}", serde_json::to_string(&ask).unwrap()).unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        line
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let sock_status = sock.clone();
    let status_ms = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut stream = UnixStream::connect(&sock_status).unwrap();
        writeln!(stream, "{}", serde_json::to_string(&Ask::Status).unwrap()).unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["version"], host::VERSION);
        started.elapsed()
    })
    .await
    .unwrap();
    assert!(status_ms < Duration::from_millis(250), "Ask::Status must not wait behind 500 ms Ask::Redeem; took {status_ms:?}");
    let redeem_line = redeem_task.await.unwrap();
    assert!(redeem_line.contains("\"redeemed\":true"), "{redeem_line}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression for Fable I3 / Astra I5:
/// - When keeper definitively answers `NotNow`, a borrower with a stale cached
///   credit MUST fall back to paid usage credits (no false "reset credit waits").
/// - When keeper is unreachable (`isn’t running here`), the borrower MUST hold
///   off usage credits and return 429 ("reset credit waits").
#[tokio::test(flavor = "multi_thread")]
async fn definitive_not_now_allows_paid_fallback_while_unreachable_holds_off() {
    use axum::http::HeaderMap;
    use olo_token_pool::{
        gateway::{self, Claimed, Claimer},
        model::{PaidFor, ResetCredit, Window},
    };

    let fake = Router::new().route(
        "/v1/messages",
        post(|| async {
            Response::builder()
                .header("content-type", "application/json")
                .header("anthropic-ratelimit-unified-overage-status", "allowed")
                .body(Body::from(r#"{"usage":{"input_tokens":2,"output_tokens":3}}"#))
                .unwrap()
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let make_desktop = |claimer: Claimer| {
        let mut saved = Saved::default();
        let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
        a.created = 1;
        a.kept_on = Some("build".into());
        a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        a.paid = Some(PaidFor { until: now() + 3 * 86400, meter: "seven_day".into() });
        a.credits = vec![ResetCredit { id: "stale".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, clears: vec!["seven_day".into()], ..Default::default() }];
        saved.accounts.push(a);
        saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
        let s = Shared::new(saved, None, up.clone(), || {});
        s.set_claimer(Some(claimer));
        s
    };

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());

    // 1. Definitive NotNow from keeper -> paid fallback proceeds (200 OK).
    let d_not_now = make_desktop(Arc::new(|_, _| Box::pin(async { Err(Claimed::NotNow("No reset credit to use for it now.".into())) })));
    let r1 = gateway::serve(d_not_now, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r1.status(), 200, "definitive NotNow must allow paid usage credit fallback");

    // 2. Unreachable keeper -> holds off usage credits (429 "reset credit waits").
    let d_unreach = make_desktop(Arc::new(|_, _| Box::pin(async { Err(Claimed::Unreachable("The Token Pool isn’t running here.".into())) })));
    let r2 = gateway::serve(d_unreach, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r2.status(), 429, "unreachable keeper must hold off usage credits");
}

/// Regression for Fable I4 / Astra C6 (mixed-version rolling upgrades):
/// - v2 desktop receiving `Claimed::Unsupported` from a legacy v1 host falls
///   back to direct claiming so resets still work before the host upgrades.
/// - v2 host carried by a legacy v1 desktop (`host_claims: false`) sets
///   `legacy_desktop_claims = true` (`redeems: false`) and defers claiming
///   until a v2 desktop carries `host_claims: true` (`redeems: true`).
#[tokio::test(flavor = "multi_thread")]
async fn mixed_version_rolling_upgrade_desktop_and_host_compat() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, Claimed},
        model::{ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

    let reset = Arc::new(AtomicBool::new(false));
    let (r1, r2, r3) = (reset.clone(), reset.clone(), reset.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let r = r1.clone();
                async move {
                    if r.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
            get(move || {
                let r = r2.clone();
                async move {
                    axum::Json(json!({
                        "seven_day": {"utilization": if r.load(SeqCst) { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": []}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let r = r3.clone();
                async move {
                    r.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    // Part A: v2 desktop + legacy v1 host -> Claimed::Unsupported triggers direct claim fallback.
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.kept_on = Some("v1-host".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits =
        vec![ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let desktop = Shared::new(saved, None, up.clone(), || {});
    desktop.set_claimer(Some(Arc::new(|_, _| Box::pin(async { Err(Claimed::Unsupported("v1 host".into())) }))));

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(desktop, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r.status(), 200, "v2 desktop must fall back to direct claim against legacy v1 host");

    // Part B: v2 host carried by legacy v1 desktop (`host_claims: false`) defers claiming!
    reset.store(false, SeqCst);
    let mut d2_saved = Saved::default();
    let mut a2 = Account::new("a".into(), "A".into(), Provider::Claude);
    a2.created = 1;
    a2.org = Some("org-a".into());
    d2_saved.accounts.push(a2);
    d2_saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });
    let d2 = Shared::new(d2_saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();
    let mut legacy_carry = d2.carry_to("build", "this computer");
    legacy_carry.host_claims = false;
    let st1 = host.carried(legacy_carry).unwrap();
    assert_eq!(st1["redeems"], false, "v2 host carried by v1 desktop reports redeems: false");
    {
        let mut hp = host.pool.lock().unwrap();
        let ha = hp.account_mut("a").unwrap();
        ha.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        ha.credits = vec![ResetCredit {
            id: "g1".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];
    }
    let r_deferred = gateway::serve(host.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r_deferred.status(), 429, "v2 host paired with v1 desktop must defer autonomous claiming");
    assert!(!reset.load(SeqCst), "v2 host must not spend while paired with v1 desktop");

    // Once v2 desktop carries `host_claims: true`, host claims autonomously!
    let v2_carry = d2.carry_to("build", "this computer");
    assert!(v2_carry.host_claims);
    let st2 = host.carried(v2_carry).unwrap();
    assert_eq!(st2["redeems"], true);
    let r_active = gateway::serve(host, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r_active.status(), 200, "v2 host resumes autonomous claiming after v2 carry");
    assert!(reset.load(SeqCst));
}

/// Regression for Astra C3 (partial remote reset scope):
/// Claiming a Fable-only credit via remote delegation clears ONLY Fable
/// cooldown/window and leaves an independently exhausted `seven_day` intact.
#[tokio::test(flavor = "multi_thread")]
async fn fable_only_remote_reset_preserves_exhausted_shared_windows() {
    use axum::routing::get;
    use olo_token_pool::{
        gateway::{ClaimAsk, Claimer},
        model::{ResetCredit, Window},
    };

    let fake = Router::new().route("/api/organizations/{org}/reset_rate_limits", post(|| async { axum::Json(json!({"result": "reset"})) })).route(
        "/api/oauth/usage",
        get(|| async {
            axum::Json(json!({
                "seven_day": {"utilization": 100.0, "resets_at": now() + 3 * 86400},
                "seven_day_overage_included": {"utilization": 0.0, "resets_at": now() + 3 * 86400},
                "cedar_ember": {"eligible": true, "grants": []}
            }))
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![
        Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] },
        Window { name: "seven_day_overage_included".into(), label: "Weekly · Fable".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec!["fable".into()] },
    ];
    a.cooldown_until = Some(now() + 3600);
    a.model_cooldowns.insert("fable".into(), now() + 3600);
    a.credits = vec![ResetCredit {
        id: "g-fable".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: true,
        program: Some("cedar_ember".into()),
        clears: vec!["seven_day_overage_included".into()],
        ..Default::default()
    }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();
    let st = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &st));
    {
        let mut hp = host.pool.lock().unwrap();
        let ha = hp.account_mut("a").unwrap();
        ha.windows = desktop.pool.lock().unwrap().account("a").unwrap().windows.clone();
        ha.credits = desktop.pool.lock().unwrap().account("a").unwrap().credits.clone();
    }
    let claimer: Claimer = {
        let host = host.clone();
        Arc::new(move |_, ask| {
            let host = host.clone();
            Box::pin(async move {
                match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                }
            })
        })
    };
    desktop.set_claimer(Some(claimer));

    desktop.redeem_credit("a", Some("g-fable"), olo_token_pool::gateway::Why::Asked).await.unwrap();
    let snap = desktop.snapshot();
    let da = snap.account("a").unwrap();
    assert!(!da.model_cooldowns.contains_key("fable"), "fable cooldown must clear");
    assert!(da.cooldown_until.is_some(), "shared cooldown must stay intact after fable-only reset");
    assert_eq!(da.windows.iter().find(|w| w.name == "seven_day").unwrap().used, 100.0, "shared seven_day must stay exhausted");
    assert_eq!(da.windows.iter().find(|w| w.name == "seven_day_overage_included").unwrap().used, 0.0, "fable window must reset");
}

/// Regressions for Astra C4, Fable I5, and Astra I3:
/// - `give_back` refuses while `a.redeeming.is_some()`;
/// - `carry_to` skips handing over an account with `a.redeeming.is_some()`;
/// - `reauth` clears stale `a.org`;
/// - keeper with locally-kept exhausted `a` and borrowed-ready `b` never
///   returns borrowed `b` to a remote caller in `handle_remote_redeem`.
#[tokio::test(flavor = "multi_thread")]
async fn ownership_transfers_block_pending_redeeming_and_keeper_never_returns_borrowed() {
    use olo_token_pool::model::Window;

    let up = Upstreams::fake("http://127.0.0.1:9");
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-old".into());
    a.redeeming = Some(("g1".into(), "req-1".into()));
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let pool = Shared::new(saved, None, up.clone(), || {});
    // 1. give_back blocked while redeeming is pending.
    assert!(pool.give_back("a", "other").await.is_err(), "give_back must reject pending redeeming");
    // 2. carry_to does not hand over an account with pending redeeming.
    let carried = pool.carry_to("build", "this computer");
    assert_eq!(carried.accounts[0].kept_on, None, "carry_to must not hand over an account mid-claim");
    assert_eq!(carried.accounts[0].credential.as_ref().and_then(|c| c.refresh.as_deref()), None);
    // 3. reauth clears stale org.
    pool.reauth("a", Credential { access: "new".into(), refresh: Some("r-new".into()), expires_at: Some(now() + 3600), account_id: None });
    assert_eq!(pool.snapshot().account("a").unwrap().org, None, "reauth must invalidate stale org");

    // 4. Keeper with locally-kept exhausted `a` and borrowed-ready `b` never returns borrowed `b`.
    let mut ksaved = Saved::default();
    let mut ka = Account::new("a".into(), "A".into(), Provider::Claude);
    ka.created = 1;
    ka.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    let mut kb = Account::new("b".into(), "B".into(), Provider::Claude);
    kb.created = 2;
    kb.kept_on = Some("other-host".into());
    kb.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 0.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    ksaved.accounts.extend([ka, kb]);
    ksaved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });
    ksaved.credentials.insert("b".into(), Credential { access: "b".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let keeper = Shared::new(ksaved, None, up, || {});
    keeper.set_hosted();
    let res = keeper.handle_remote_redeem(None, None, Some(Provider::Claude), Some("opus".into()), Some("op-1".into()), None, false, Vec::new()).await;
    assert!(matches!(res, Err(olo_token_pool::gateway::Claimed::NotNow(_))), "keeper must never return borrowed account b, got {res:?}");
}

/// Regression for Fable R2 B1 / Astra R2 #3a:
/// `Ask::Stop` must flush pending vault writes BEFORE retiring `drain`, so
/// rotated refresh tokens and queued bookkeeping are never lost on stop.
#[tokio::test(flavor = "multi_thread")]
async fn ask_stop_flushes_pending_writes_before_retiring() {
    use olo_token_pool::{
        host::{self, Ask},
        store::Store,
    };

    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pool-stop-test-{}", olo_token_pool::store::new_uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    let vault = dir.join("pool.vault");
    let key = [7u8; 32];
    let store = Store::with_key(vault.clone(), key);
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "old".into(), refresh: Some("rotated-1".into()), expires_at: Some(now() + 3600), account_id: None });
    store.save(&saved).unwrap();

    let shared = Shared::new(saved, Some(store), Upstreams::fake("http://127.0.0.1:9"), || {});
    shared.set_hosted();
    {
        let mut p = shared.pool.lock().unwrap();
        p.saved.credentials.get_mut("a").unwrap().refresh = Some("rotated-2".into());
        p.account_mut("a").unwrap().total.requests = 42;
    }
    shared.noted();
    let said = host::answer(&shared, Ask::Stop);
    assert_eq!(said["stopping"], true);
    assert!(shared.drain.retired(), "stop retires drain after flushing");
    let reopened = Store::with_key(vault, key).read().unwrap();
    assert_eq!(reopened.credentials["a"].refresh.as_deref(), Some("rotated-2"), "rotated refresh token must survive Ask::Stop");
    assert_eq!(reopened.accounts[0].total.requests, 42, "queued bookkeeping must survive Ask::Stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression for Astra R2 #1:
/// Cancelling a borrower mid-RPC after the keeper spent the first grant must
/// leave `PendingRemoteClaim` persisted on disk so retrying reconciles the
/// exact `op_id` instead of spending a second grant!
#[tokio::test(flavor = "multi_thread")]
async fn interrupted_borrower_manual_redeem_reconciles_without_double_spend() {
    use axum::routing::get;
    use olo_token_pool::{
        gateway::{ClaimAsk, Claimer},
        model::{ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use tokio::sync::Notify;

    let spends = Arc::new(AtomicUsize::new(0));
    let (sp1, sp2) = (spends.clone(), spends.clone());
    let fake = Router::new()
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let sp = sp1.clone();
                async move {
                    sp.fetch_add(1, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move || {
                let sp = sp2.clone();
                async move {
                    let left = 2usize.saturating_sub(sp.load(SeqCst));
                    let grants: Vec<_> = (0..left).map(|i| json!({"id": format!("g{}", i + 1), "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400})).collect();
                    axum::Json(json!({"seven_day": {"utilization": 0.0, "resets_at": now() + 3 * 86400}, "cedar_ember": {"eligible": true, "grants": grants}}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits = vec![
        ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() },
        ResetCredit { id: "g2".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() },
    ];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();
    let st = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &st));
    {
        let mut hp = host.pool.lock().unwrap();
        let ha = hp.account_mut("a").unwrap();
        ha.windows = desktop.pool.lock().unwrap().account("a").unwrap().windows.clone();
        ha.credits = desktop.pool.lock().unwrap().account("a").unwrap().credits.clone();
    }

    let keeper_done = Arc::new(Notify::new());
    let hang_first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let claimer: Claimer = {
        let (host, kd, hf) = (host.clone(), keeper_done.clone(), hang_first.clone());
        Arc::new(move |_, ask| {
            let (host, kd, hf) = (host.clone(), kd.clone(), hf.clone());
            Box::pin(async move {
                let res = match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                };
                if hf.swap(false, SeqCst) {
                    kd.notify_one();
                    // Hang until cancelled so borrower drops mid-RPC!
                    std::future::pending::<()>().await;
                }
                res
            })
        })
    };
    desktop.set_claimer(Some(claimer));

    let d1 = desktop.clone();
    let task = tokio::spawn(async move { d1.redeem("a").await });
    keeper_done.notified().await;
    task.abort();
    let _ = task.await;
    assert_eq!(spends.load(SeqCst), 1, "first grant spent on keeper");
    assert_eq!(desktop.pool.lock().unwrap().saved.pending_remote_claims.len(), 1, "pending op persisted BEFORE dispatch survives cancellation");

    // Retrying manual redeem reconciles the persisted op_id and MUST NOT spend g2!
    desktop.pool.lock().unwrap().saved.pending_remote_claims[0].tried_at = now() - 20;
    desktop.redeem("a").await.expect("retry reconciles interrupted manual claim");
    assert_eq!(spends.load(SeqCst), 1, "interrupted manual retry must NEVER spend a second grant");
    assert!(desktop.pool.lock().unwrap().saved.pending_remote_claims.is_empty());
}

/// Regression for Astra R2 #2:
/// 1) Provider applies reset and returns HTTP 500 -> keeper keeps `a.redeeming`
///    AND `remote_claims[op_id] = unclear`, so `reconcile_only` recovers `Redeemed`
///    (NEVER `NoRecord`) without spending a second grant.
/// 2) 40 subsequent `NotNow` asks do NOT evict a confirmed `Redeemed` record.
/// 3) A slow `/api/oauth/profile` exceeding `timeout_secs` aborts before POST.
#[tokio::test(flavor = "multi_thread")]
async fn keeper_http_500_and_not_now_flood_preserve_op_id_and_deadline_checked_before_post() {
    use axum::{http::StatusCode, response::IntoResponse, routing::get};
    use olo_token_pool::{
        gateway::Claimed,
        model::{RemoteClaim, ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    let applied = Arc::new(AtomicBool::new(false));
    let posts = Arc::new(AtomicUsize::new(0));
    let (a1, a2, p1) = (applied.clone(), applied.clone(), posts.clone());
    let fake = Router::new()
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let (a, p) = (a1.clone(), p1.clone());
                async move {
                    p.fetch_add(1, SeqCst);
                    a.store(true, SeqCst);
                    // Apply reset, then return HTTP 500!
                    (StatusCode::INTERNAL_SERVER_ERROR, "upstream 500").into_response()
                }
            }),
        )
        .route(
            "/api/oauth/usage",
            get(move || {
                let a = a2.clone();
                async move {
                    let ok = a.load(SeqCst);
                    axum::Json(json!({
                        "seven_day": {"utilization": if ok { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": if ok { json!([]) } else { json!([{"id": "g1", "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400}]) }}
                    }))
                }
            }),
        )
        .route(
            "/api/oauth/profile",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                axum::Json(json!({"organization": {"uuid": "org-slow"}}))
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits =
        vec![ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let host = Shared::new(saved, None, up, || {});
    host.set_hosted();

    // 1. Provider applies reset and returns 500 -> keeper returns Unclear, retains op-500.
    let err = host.handle_remote_redeem(Some("a".into()), Some("g1".into()), None, None, Some("op-500".into()), Some(30), false, Vec::new()).await.unwrap_err();
    assert!(matches!(err, Claimed::Unclear(_)), "expected Unclear, got {err:?}");
    assert_eq!(posts.load(SeqCst), 1);

    // Reconciling op-500 recovers Redeemed (NOT NoRecord) and sends zero extra POSTs!
    let rec = host
        .handle_remote_redeem(Some("a".into()), Some("g1".into()), None, None, Some("op-500".into()), Some(30), true, Vec::new())
        .await
        .expect("reconcile_only must recover spent credit after HTTP 500");
    assert!(matches!(rec, RemoteClaim::Redeemed { ref account, .. } if account == "a"));
    assert_eq!(posts.load(SeqCst), 1);

    // 2. Flood 40 NotNow asks -> op-500's confirmed Redeemed record MUST NOT be evicted!
    for i in 0..40 {
        let _ = host.handle_remote_redeem(Some("a".into()), None, None, None, Some(format!("noop-{i}")), Some(30), false, Vec::new()).await;
    }
    let replay = host
        .handle_remote_redeem(Some("a".into()), Some("g1".into()), None, None, Some("op-500".into()), Some(30), true, Vec::new())
        .await
        .expect("confirmed Redeemed record must survive 40 NotNow probes");
    assert!(matches!(replay, RemoteClaim::Redeemed { ref account, .. } if account == "a"));

    // 3. Slow profile lookup exceeding timeout_secs (1 s) MUST abort before POST!
    {
        let mut p = host.pool.lock().unwrap();
        let a = p.account_mut("a").unwrap();
        a.org = None;
        a.credits = vec![ResetCredit {
            id: "g-late".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];
    }
    let expired = host.handle_remote_redeem(Some("a".into()), Some("g-late".into()), None, None, Some("op-expired".into()), Some(1), false, Vec::new()).await.unwrap_err();
    assert!(matches!(expired, Claimed::NotNow(_)), "expired ask must fail NotNow, got {expired:?}");
    assert_eq!(posts.load(SeqCst), 1, "expired ask must NEVER send upstream POST after slow profile read");
}

/// Regression for Fable R2 B2(a) and B2(b):
/// (a) When a remote keeper is unreachable (SSH refused / not running), the
///     desktop clears the unsent op, caches unreachable, and STILL claims its
///     own locally-kept account immediately (never stuck on ResetWaits!).
/// (b) When a pending remote claim IS unclear (lost reply), a reconcile hitting
///     `Unreachable` or `NotNow` (service stopped / retired) MUST preserve the
///     pending claim and NOT spend a second reset on the desktop's own account!
#[tokio::test(flavor = "multi_thread")]
async fn unreachable_keeper_allows_local_claim_and_not_running_keeps_pending_claim() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, Claimed},
        model::{PendingRemoteClaim, ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    let local_reset = Arc::new(AtomicBool::new(false));
    let local_posts = Arc::new(AtomicUsize::new(0));
    let (lr1, lr2, lr3, lp) = (local_reset.clone(), local_reset.clone(), local_reset.clone(), local_posts.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let lr = lr1.clone();
                async move {
                    let who = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ");
                    if who == "l" && lr.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
            get(move || {
                let lr = lr2.clone();
                async move {
                    axum::Json(json!({
                        "seven_day": {"utilization": if lr.load(SeqCst) { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": []}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let (lr, lp) = (lr3.clone(), lp.clone());
                async move {
                    lp.fetch_add(1, SeqCst);
                    lr.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let make_mixed = || {
        let mut saved = Saved::default();
        let mut k = Account::new("k".into(), "KeptRemote".into(), Provider::Claude);
        k.created = 1;
        k.org = Some("org-k".into());
        k.kept_on = Some("down-host".into());
        k.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        k.credits = vec![ResetCredit {
            id: "gk".into(),
            expires_at: Some(now() + 86400),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];

        let mut loc = Account::new("l".into(), "Local".into(), Provider::Claude);
        loc.created = 2;
        loc.org = Some("org-l".into());
        loc.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        loc.credits = vec![ResetCredit {
            id: "gl".into(),
            expires_at: Some(now() + 90000),
            remaining: 1,
            usable: true,
            program: Some("cedar_ember".into()),
            clears: vec!["seven_day".into()],
            ..Default::default()
        }];
        saved.accounts.extend([k, loc]);
        saved.credentials.insert("k".into(), Credential { access: "k".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
        saved.credentials.insert("l".into(), Credential { access: "l".into(), refresh: Some("rl".into()), expires_at: Some(now() + 3600), account_id: None });
        saved
    };

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());

    // Part (a): SSH connection refused (`Claimed::Unreachable`) on `down-host` -> desktop claims its own `l` immediately!
    let desk_a = Shared::new(make_mixed(), None, up.clone(), || {});
    desk_a.set_claimer(Some(Arc::new(|_, _| Box::pin(async { Err(Claimed::Unreachable("connection refused".into())) }))));
    let r_a = gateway::serve(desk_a.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h.clone(), axum::http::Extensions::new(), body.clone()).await;
    assert_eq!(r_a.status(), 200, "unreachable keeper must not block desktop's own account l");
    assert_eq!(local_posts.load(SeqCst), 1);
    assert!(desk_a.pool.lock().unwrap().saved.pending_remote_claims.is_empty(), "pre-delivery Unreachable must not leave pending claim");

    // Part (b): Pending unclear claim on `down-host`, then reconcile gets Unreachable ("isn't running here") -> MUST keep pending claim & NOT spend `l`!
    local_reset.store(false, SeqCst);
    let mut saved_b = make_mixed();
    saved_b.pending_remote_claims.push(PendingRemoteClaim {
        provider: Provider::Claude,
        keeper: "down-host".into(),
        op_id: "op-lost".into(),
        account: None,
        credit: None,
        family: "opus".into(),
        at: now() - 30,
        tried_at: now() - 30,
    });
    let desk_b = Shared::new(saved_b, None, up, || {});
    desk_b.set_claimer(Some(Arc::new(|_, _| Box::pin(async { Err(Claimed::Unreachable("The Token Pool isn’t running here.".into())) }))));
    let r_b = gateway::serve(desk_b.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r_b.status(), 429, "stopped keeper during reconcile must keep waiting, never double-spend local");
    assert_eq!(local_posts.load(SeqCst), 1, "local account l must NOT be spent while op-lost is unreconciled");
    assert_eq!(desk_b.pool.lock().unwrap().saved.pending_remote_claims.len(), 1, "pending claim must survive service-stopped reconcile");
}

/// Regression for Fable R2 B3:
/// Manual "Use a Reset Credit…" from the desktop for a host-kept account at a
/// `five_hour` limit only the desktop saw:
/// 1) With keeper's grant cached as `needs_limit`: desktop sends `observed` on
///    `ClaimAsk::Account`, so keeper claims it!
/// 2) After claim, BOTH desktop and keeper clear `five_hour` (`cleared_windows`
///    includes `credit.clears`), so the next request succeeds immediately (200).
#[tokio::test(flavor = "multi_thread")]
async fn manual_desktop_redeem_at_desktop_observed_five_hour_limit() {
    use axum::{http::HeaderMap, routing::get};
    use olo_token_pool::{
        gateway::{self, ClaimAsk, Claimer},
        model::{ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

    let reset = Arc::new(AtomicBool::new(false));
    let posts = Arc::new(AtomicUsize::new(0));
    let (r1, r2, p1) = (reset.clone(), reset.clone(), posts.clone());
    let fake = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let r = r1.clone();
                async move {
                    if r.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
            "/api/oauth/usage",
            get(move || {
                // Omit five_hour completely (Anthropic Max shape).
                async move {
                    axum::Json(json!({
                        "seven_day": {"utilization": 20.0, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": []}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move || {
                let (r, p) = (r2.clone(), p1.clone());
                async move {
                    p.fetch_add(1, SeqCst);
                    r.store(true, SeqCst);
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let grant = ResetCredit {
        id: "g-5h".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: false,
        needs_limit: true,
        unavailable_reason: Some("needs_limit".into()),
        program: Some("cedar_ember".into()),
        clears: vec!["five_hour".into()],
        ..Default::default()
    };
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    // Desktop saw 5-hour exhaustion; host only has seven_day: 20%!
    a.windows = vec![
        Window { name: "five_hour".into(), label: "5-hour".into(), used: 100.0, resets_at: Some(now() + 3600), models: vec![] },
        Window { name: "seven_day".into(), label: "Weekly".into(), used: 20.0, resets_at: Some(now() + 3 * 86400), models: vec![] },
    ];
    a.credits = vec![grant.clone()];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, up.clone(), || {});
    let host = Shared::new(Saved::default(), None, up, || {});
    host.set_hosted();
    let st = host.carried(desktop.carry_to("build", "this computer")).unwrap();
    assert!(desktop.handed_over("build", &st));
    {
        let mut hp = host.pool.lock().unwrap();
        let ha = hp.account_mut("a").unwrap();
        ha.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 20.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
        ha.credits = vec![grant];
    }
    let claimer: Claimer = {
        let host = host.clone();
        Arc::new(move |_, ask| {
            let host = host.clone();
            Box::pin(async move {
                match ask {
                    ClaimAsk::Account { id, credit, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(Some(id), credit, None, None, Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                    ClaimAsk::For { provider, family, op_id, timeout_secs, reconcile_only, observed } => {
                        host.handle_remote_redeem(None, None, Some(provider), Some(family), Some(op_id), timeout_secs, reconcile_only, observed).await
                    }
                }
            })
        })
    };
    desktop.set_claimer(Some(claimer));

    // Manual redeem from desktop succeeds even though keeper hadn't seen five_hour!
    desktop.redeem("a").await.expect("manual desktop redeem must carry observed five_hour to keeper");
    assert_eq!(posts.load(SeqCst), 1);
    // Desktop must have cleared five_hour so next request goes straight through (200 OK)!
    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(desktop, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(r.status(), 200, "after manual reset, desktop must clear five_hour and serve 200");
}

/// Regression for Astra R2 #4:
/// 1) Deserializing a legacy v1 vault JSON (no `legacy_desktop_claims` field)
///    into a v2 host defaults `legacy_desktop_claims = true` so the v2 host
///    does NOT claim autonomously before a v2 desktop handshake.
/// 2) Reauthenticating a host-kept account persists `reauth_pending_host` and
///    blocks BOTH automatic and manual local claiming until handover settles.
#[tokio::test(flavor = "multi_thread")]
async fn v1_vault_loaded_into_v2_host_defers_claiming_and_reauth_blocks_manual_redeem() {
    use olo_token_pool::model::{ResetCredit, Window};

    // 1. Legacy v1 vault JSON without `legacy_desktop_claims`:
    let v1_json = json!({
        "accounts": [{
            "id": "a", "name": "A", "provider": "claude", "created": 1, "enabled": true,
            "email": null, "plan": null, "last_used": null, "synced": null,
            "org": "org-a",
            "windows": [{"name": "seven_day", "label": "Weekly", "used": 100.0, "resets_at": now() + 3 * 86400}],
            "credits": [{"id": "g1", "remaining": 1, "usable": true, "clears": ["seven_day"], "expires_at": now() + 86400}]
        }],
        "credentials": {"a": {"access": "a", "refresh": "ra", "expires_at": now() + 3600, "account_id": null}}
    });
    let loaded: Saved = serde_json::from_value(v1_json).unwrap();
    assert!(loaded.legacy_desktop_claims, "v1 vault without legacy_desktop_claims must default to true");

    // 2. Reauth on a host-kept account blocks manual redeem until handed_over settles it!
    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.org = Some("org-a".into());
    a.kept_on = Some("build".into());
    a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    a.credits = vec![ResetCredit { id: "g1".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, clears: vec!["seven_day".into()], ..Default::default() }];
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "old".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
    desktop.reauth("a", Credential { access: "new".into(), refresh: Some("r-new".into()), expires_at: Some(now() + 3600), account_id: None });
    assert_eq!(desktop.snapshot().account("a").unwrap().reauth_pending_host.as_deref(), Some("build"));
    let err = desktop.redeem("a").await.expect_err("manual redeem must be blocked while reauth handover is pending");
    assert!(err.contains("accept the updated sign-in"), "{err}");
    // Another host must NOT receive this account while reauth_pending_host == "build".
    let wrong_host = desktop.carry_to("other-host", "this computer");
    assert_eq!(wrong_host.accounts[0].kept_on.as_deref(), Some("build"));
    assert!(wrong_host.accounts[0].credential.as_ref().unwrap().refresh.is_none());
}

/// Regression for Astra R2 #5:
/// When the keeper returns `AlreadyReady` carrying a previously unseen Fable-only
/// `ClearedScope` and a weekly-only snapshot, the borrower MUST preserve its
/// exhausted `five_hour` meter and shared cooldown (never clearing them and
/// routing a request into paid usage!).
#[tokio::test(flavor = "multi_thread")]
async fn unrelated_fable_already_ready_preserves_borrower_five_hour_and_cooldown() {
    use axum::http::HeaderMap;
    use olo_token_pool::{
        gateway,
        model::{ClearedScope, PaidFor, RemoteClaim, Window},
    };
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    let msg_hits = Arc::new(AtomicUsize::new(0));
    let mh = msg_hits.clone();
    let fake = Router::new().route(
        "/v1/messages",
        post(move || {
            let mh = mh.clone();
            async move {
                mh.fetch_add(1, SeqCst);
                Response::builder()
                    .header("content-type", "application/json")
                    .header("anthropic-ratelimit-unified-overage-status", "allowed")
                    .body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#))
                    .unwrap()
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = Upstreams::fake(&format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

    let mut saved = Saved::default();
    let mut a = Account::new("a".into(), "A".into(), Provider::Claude);
    a.created = 1;
    a.kept_on = Some("build".into());
    a.windows = vec![
        Window { name: "five_hour".into(), label: "5-hour".into(), used: 100.0, resets_at: Some(now() + 3600), models: vec![] },
        Window { name: "seven_day".into(), label: "Weekly".into(), used: 20.0, resets_at: Some(now() + 3 * 86400), models: vec![] },
    ];
    a.cooldown_until = Some(now() + 3600);
    a.paid = Some(PaidFor { until: now() + 3600, meter: "five_hour".into() });
    saved.accounts.push(a);
    saved.credentials.insert("a".into(), Credential { access: "a".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });

    let desktop = Shared::new(saved, None, up, || {});
    desktop.set_claimer(Some(Arc::new(move |_, _| {
        Box::pin(async move {
            Ok(RemoteClaim::AlreadyReady {
                account: "a".into(),
                last_reset: Some(ClearedScope {
                    reset_id: "fable-reset-1".into(),
                    cleared_shared: false,
                    cleared_models: vec!["fable".into()],
                    cleared_windows: vec!["seven_day_overage_included".into()],
                }),
                windows: vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 20.0, resets_at: Some(now() + 3 * 86400), models: vec![] }],
                credits: vec![],
            })
        })
    })));

    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let r = gateway::serve(desktop.clone(), Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    // Account has a live cooldown so neither normal nor paid usage may fire!
    assert_eq!(r.status(), 429, "unrelated Fable AlreadyReady must NOT clear five_hour or shared cooldown");
    assert_eq!(msg_hits.load(SeqCst), 0, "must never send a billed request after unrelated Fable reset");
    let after = desktop.snapshot();
    let da = after.account("a").unwrap();
    assert!(da.cooldown_until.is_some(), "shared cooldown must stay intact");
    assert_eq!(da.windows.iter().find(|w| w.name == "five_hour").unwrap().used, 100.0, "omitted five_hour must stay exhausted");
}

/// Regression for Astra R2 #6:
/// 1) Keeper where account A's usage confirmation returns 429 and exhausted B
///    has a usable grant MUST skip A in `redeem_kept_locked` and claim B!
/// 2) Mixed pool where local A's cached grant is refused, local B has a fresh
///    uncached grant, and remote keeper has none MUST refresh local credits and
///    claim B!
#[tokio::test(flavor = "multi_thread")]
async fn confirm_failed_skips_ready_and_mixed_pool_refreshes_after_local_refusal() {
    use axum::{http::HeaderMap, response::IntoResponse, routing::get};
    use olo_token_pool::{
        gateway::{self, Claimed},
        model::{RemoteClaim, ResetCredit, Window},
    };
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

    // Part 1: Keeper confirm_failed on A + usable grant on B -> claims B!
    let b_claimed = Arc::new(AtomicBool::new(false));
    let bc = b_claimed.clone();
    let fake1 = Router::new()
        .route(
            "/api/oauth/usage",
            get(move |h: HeaderMap| async move {
                let who = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ");
                if who == "a" {
                    Response::builder().status(429).body(Body::from("{}")).unwrap()
                } else {
                    axum::Json(json!({"seven_day": {"utilization": 0.0, "resets_at": now() + 3 * 86400}, "cedar_ember": {"eligible": true, "grants": []}})).into_response()
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>| {
                let bc = bc.clone();
                async move {
                    if org == "org-b" {
                        bc.store(true, SeqCst);
                    }
                    axum::Json(json!({"result": "reset"}))
                }
            }),
        );
    let l1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up1 = Upstreams::fake(&format!("http://{}", l1.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l1, fake1).await.unwrap() });

    let mut ks = Saved::default();
    let mut ka = Account::new("a".into(), "A".into(), Provider::Claude);
    ka.created = 1;
    ka.org = Some("org-a".into());
    // Stale 50% cache on A that fails confirmation (429 on /api/oauth/usage).
    ka.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 50.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    let mut kb = Account::new("b".into(), "B".into(), Provider::Claude);
    kb.created = 2;
    kb.org = Some("org-b".into());
    kb.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    kb.credits =
        vec![ResetCredit { id: "gb".into(), expires_at: Some(now() + 86400), remaining: 1, usable: true, program: Some("cedar_ember".into()), clears: vec!["seven_day".into()], ..Default::default() }];
    ks.accounts.extend([ka, kb]);
    ks.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });
    ks.credentials.insert("b".into(), Credential { access: "b".into(), refresh: Some("rb".into()), expires_at: Some(now() + 3600), account_id: None });
    let keeper = Shared::new(ks, None, up1, || {});
    keeper.set_hosted();
    let res = keeper.handle_remote_redeem(None, None, Some(Provider::Claude), Some("opus".into()), Some("op-c1".into()), Some(30), false, Vec::new()).await.unwrap();
    assert!(matches!(res, RemoteClaim::Redeemed { ref account, .. } if account == "b"), "must skip confirm_failed A and claim B, got {res:?}");
    assert!(b_claimed.load(SeqCst));

    // Part 2: Mixed pool where local A's cached grant is refused, local B has an uncached grant, remote keeper has none -> refreshes & claims B!
    let lb_reset = Arc::new(AtomicBool::new(false));
    let (lr1, lr2, lr3) = (lb_reset.clone(), lb_reset.clone(), lb_reset.clone());
    let fake2 = Router::new()
        .route(
            "/v1/messages",
            post(move |h: HeaderMap| {
                let lr = lr1.clone();
                async move {
                    let who = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ");
                    if who == "b" && lr.load(SeqCst) {
                        Response::builder().header("content-type", "application/json").body(Body::from(r#"{"usage":{"input_tokens":1,"output_tokens":1}}"#)).unwrap()
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
                let lr = lr2.clone();
                async move {
                    let who = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ");
                    let grants =
                        if who == "b" && !lr.load(SeqCst) { json!([{"id": "gb-new", "resets_left": 1, "usable_now": true, "clears": ["seven_day"], "ends_at": now() + 86400}]) } else { json!([]) };
                    axum::Json(json!({
                        "seven_day": {"utilization": if who == "b" && lr.load(SeqCst) { 0.0 } else { 100.0 }, "resets_at": now() + 3 * 86400},
                        "cedar_ember": {"eligible": true, "grants": grants}
                    }))
                }
            }),
        )
        .route(
            "/api/organizations/{org}/reset_rate_limits",
            post(move |axum::extract::Path(org): axum::extract::Path<String>| {
                let lr = lr3.clone();
                async move {
                    if org == "org-a" {
                        (axum::http::StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"message": "refused"}}))).into_response()
                    } else {
                        lr.store(true, SeqCst);
                        axum::Json(json!({"result": "reset"})).into_response()
                    }
                }
            }),
        );
    let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up2 = Upstreams::fake(&format!("http://{}", l2.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(l2, fake2).await.unwrap() });

    let mut ms = Saved::default();
    let mut ma = Account::new("a".into(), "A".into(), Provider::Claude);
    ma.created = 1;
    ma.org = Some("org-a".into());
    ma.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    ma.credits = vec![ResetCredit {
        id: "ga-stale".into(),
        expires_at: Some(now() + 86400),
        remaining: 1,
        usable: true,
        program: Some("cedar_ember".into()),
        clears: vec!["seven_day".into()],
        ..Default::default()
    }];
    let mut mb = Account::new("b".into(), "B".into(), Provider::Claude);
    mb.created = 2;
    mb.org = Some("org-b".into());
    mb.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    let mut mr = Account::new("r".into(), "R".into(), Provider::Claude);
    mr.created = 3;
    mr.kept_on = Some("build".into());
    mr.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(now() + 3 * 86400), models: vec![] }];
    ms.accounts.extend([ma, mb, mr]);
    ms.credentials.insert("a".into(), Credential { access: "a".into(), refresh: Some("ra".into()), expires_at: Some(now() + 3600), account_id: None });
    ms.credentials.insert("b".into(), Credential { access: "b".into(), refresh: Some("rb".into()), expires_at: Some(now() + 3600), account_id: None });
    ms.credentials.insert("r".into(), Credential { access: "r".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });

    let mixed = Shared::new(ms, None, up2, || {});
    mixed.set_claimer(Some(Arc::new(|_, _| Box::pin(async { Err(Claimed::NotNow("none".into())) }))));
    let mut h = HeaderMap::new();
    h.insert("content-type", "application/json".parse().unwrap());
    let body = axum::body::Bytes::from(json!({"model": "claude-opus-5-5", "messages": []}).to_string());
    let resp = gateway::serve(mixed, Provider::Claude, "messages".into(), None, axum::http::Method::POST, h, axum::http::Extensions::new(), body).await;
    assert_eq!(resp.status(), 200, "mixed pool must refresh stale local credits after A's refusal and claim B");
    assert!(lb_reset.load(SeqCst));
}
