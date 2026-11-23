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
