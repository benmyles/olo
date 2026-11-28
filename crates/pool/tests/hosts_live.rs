//! The desktop pool's side of a host's pool (`hosts`), the whole way
//! over SSH to a real SSH server: the host's program put there and
//! started, the accounts carried, a sign-in borrowed, everything taken
//! back and removed. Against the rig's far host
//! (`tools/dev/rex-slow/farhost.py`, whose pool listens on ports of the
//! rig's own and reaches no provider but the test's), never a real
//! account or service.
//!
//! `OLO_REX_TEST_HOST`: the host (`ssh://user@127.0.0.1:PORT`);
//! `OLO_POOL_HOST_BIN`: the host's program, built for it;
//! `OLO_POOL_TEST_UPSTREAM_PORT`: where the test's provider listens
//! (the far host's pool is told the same: `FAR_POOL_UPSTREAM`).
//! Without it (a host that can't reach this computer's loopback: a
//! container with systemd, for the service's own path), the sign-in is
//! one that lasts, and nothing is renewed; `OLO_POOL_TEST_PAUSE`: a
//! file waited for before the pool is removed, for a look at the host
//! with the pool on it.

use std::{sync::Arc, time::Duration};

use axum::{Router, body::Body, response::Response, routing::post};
use olo_token_pool::{
    gateway::Shared,
    hosts::{Build, Hosts},
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs OLO_REX_TEST_HOST and OLO_POOL_HOST_BIN: the rig's far host"]
async fn a_pool_is_put_on_a_host_kept_in_step_and_taken_away() {
    let (Ok(host), Ok(bin)) = (std::env::var("OLO_REX_TEST_HOST"), std::env::var("OLO_POOL_HOST_BIN")) else { return };
    assert!(
        std::env::var_os("XDG_CONFIG_HOME").is_some_and(|d| std::path::Path::new(&d).starts_with("/var/tmp")),
        "hosts_live must run with XDG_CONFIG_HOME under /var/tmp so it never touches real ~/.config/olo"
    );
    let provider: Option<u16> = std::env::var("OLO_POOL_TEST_UPSTREAM_PORT").ok().and_then(|p| p.parse().ok());
    let port = provider.unwrap_or(9);
    // A provider: each refresh token good once.
    let renewals: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let seen = renewals.clone();
    let app = Router::new().route(
        "/oauth",
        post(move |body: axum::Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                let token = body["refresh_token"].as_str().unwrap_or("").to_owned();
                let again = seen.lock().unwrap().contains(&token);
                seen.lock().unwrap().push(token.clone());
                if again {
                    return Response::builder().status(400).body(Body::from(r#"{"error":"invalid_grant"}"#)).unwrap();
                }
                Response::builder().body(Body::from(json!({"access_token": format!("fresh-{token}"), "refresh_token": format!("{token}+"), "expires_in": 3600}).to_string())).unwrap()
            }
        }),
    );
    if provider.is_some() {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    }
    // (About to run out where there is a provider to renew it.)
    let lasts = if provider.is_some() { 30 } else { 86_400 };

    let mut saved = Saved::default();
    let mut a = Account::new("made-up".into(), "made-up".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("made-up".into(), Credential { access: "old".into(), refresh: Some("r1".into()), expires_at: Some(now() + lasts), account_id: None });
    let desktop = Shared::new(saved, None, Upstreams::fake(&format!("http://127.0.0.1:{port}")), || {});
    desktop.set_runtime(tokio::runtime::Handle::current());
    let bytes: &'static [u8] = Box::leak(std::fs::read(&bin).expect("the host's program").into_boxed_slice());
    let hosts = Hosts::start(desktop.clone(), vec![Build { machines: &["x86_64", "aarch64"], bytes }], "test1", || {});
    hosts.set_known(vec![("far".into(), "far".into(), host.clone())]);
    let stands = |hosts: &Arc<Hosts>| hosts.list().into_iter().next().unwrap();
    let settled = |hosts: &Arc<Hosts>, what: &'static str| {
        let hosts = hosts.clone();
        async move {
            let began = std::time::Instant::now();
            loop {
                let s = hosts.list().into_iter().next().unwrap();
                if s.doing.is_none() {
                    println!("the pool: {what} in {:.1} s{}", began.elapsed().as_secs_f32(), s.error.as_ref().map(|e| format!(" (said: {e})")).unwrap_or_default());
                    return s;
                }
                assert!(began.elapsed() < Duration::from_secs(60), "{what} never ended");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    assert!(!stands(&hosts).wanted);

    // Installed: the program there, its service running, the account
    // carried and kept there.
    hosts.act("far", "install").unwrap();
    let s = settled(&hosts, "installed").await;
    assert!(s.error.is_none() && s.wanted && s.in_step, "{s:?}");
    let status = s.status.expect("how it stands");
    assert_eq!((status["hosted"].as_bool(), status["accounts"][0]["name"].as_str(), status["sign_ins"][0]["kept"].as_bool()), (Some(true), Some("made-up"), Some(true)));
    let ours = |of: &Shared| {
        let p = of.pool.lock().unwrap();
        (p.saved.credentials["made-up"].access.clone(), p.saved.credentials["made-up"].refresh.clone(), p.account("made-up").unwrap().kept_on.clone())
    };
    assert_eq!(ours(&desktop), ("old".into(), None, Some("far".into())), "handed over");

    // A newer sign-in, borrowed over SSH: the host renews, once.
    if provider.is_some() {
        let c = desktop.renewed("made-up", Provider::Claude).await.expect("borrowed");
        assert_eq!((c.access.as_str(), c.refresh.as_deref()), ("fresh-r1", None));
        assert_eq!(renewals.lock().unwrap().as_slice(), ["r1"]);
    }

    // Changed here, carried there by itself.
    desktop.rename("made-up", "renamed");
    let began = std::time::Instant::now();
    loop {
        let s = stands(&hosts);
        if s.doing.is_none() && s.status.as_ref().is_some_and(|st| st["accounts"][0]["name"] == "renamed") {
            println!("the pool: a change here was there {:.1} s later", began.elapsed().as_secs_f32());
            assert!(s.in_step);
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(30), "the change never got there: {s:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Installed again (an upgrade of the program): the service starts
    // anew and still has its account, whose sign-in it kept.
    hosts.act("far", "install").unwrap();
    let s = settled(&hosts, "installed again").await;
    assert!(s.error.is_none() && s.in_step && s.status.as_ref().is_some_and(|st| st["sign_ins"][0]["kept"] == true), "{s:?}");

    // Olo upgraded: its pool starts carrying another program, and puts
    // it on the host by itself, nobody asking; the service starts anew
    // with it and still has its account and the sign-in it kept.
    let there = |what: &'static str| {
        let host = host.clone();
        async move {
            let target = olo_rex_client::SshTarget::parse(&host).unwrap();
            olo_rex_client::SshSessionPool::default().exec(&target, None, what, &mut |_| {}).await.expect("asked of the host").output.trim().to_owned()
        }
    };
    assert_eq!(there("readlink \"$HOME\"/.local/share/olo/pool/olo-pool-host").await, "olo-pool-host-test1");
    drop(hosts);
    let hosts = Hosts::start(desktop.clone(), vec![Build { machines: &["x86_64", "aarch64"], bytes }], "test2", || {});
    hosts.set_known(vec![("far".into(), "far".into(), host.clone())]);
    let began = std::time::Instant::now();
    let mut said = false;
    loop {
        let s = stands(&hosts);
        said |= s.doing.as_deref() == Some("Upgrading");
        if said && s.doing.is_none() {
            println!("the pool: upgraded by itself {:.1} s after the pool started", began.elapsed().as_secs_f32());
            assert!(s.error.is_none() && s.wanted && s.in_step && s.status.as_ref().is_some_and(|st| st["sign_ins"][0]["kept"] == true), "{s:?}");
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(60), "never upgraded: {s:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(there("cd \"$HOME\"/.local/share/olo/pool && readlink olo-pool-host && ls | grep -c '^olo-pool-host-'").await, "olo-pool-host-test2\n1", "the new one in its place, the old one gone");
    // (And not put again where it is: a pool started once more does nothing to it.)
    let before = there("stat -c '%Y %i' \"$HOME\"/.local/share/olo/pool/olo-pool-host-test2; find \"${XDG_STATE_HOME:-$HOME/.local/state}\" -name pool-host.sock -exec stat -c '%Z %i' {} +").await;
    // (The program's file, and the socket its service made as it started.)
    assert_eq!(before.lines().count(), 2, "{before}");
    drop(hosts);
    // (As a pool put there before Olo kept which program it put: it
    // looks, finds its own in its place and answering, and leaves it.)
    let kept = olo_xdg::config_dir().join("pool-hosts.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&kept).unwrap()).unwrap();
    assert_eq!(v["far"]["build"], "test2");
    v["far"].as_object_mut().unwrap().remove("build");
    std::fs::write(&kept, v.to_string()).unwrap();
    let hosts = Hosts::start(desktop.clone(), vec![Build { machines: &["x86_64", "aarch64"], bytes }], "test2", || {});
    hosts.set_known(vec![("far".into(), "far".into(), host.clone())]);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let s = settled(&hosts, "started again with the same program").await;
    assert!(s.error.is_none() && s.in_step, "{s:?}");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&kept).unwrap()).unwrap();
    assert_eq!(v["far"]["build"], "test2", "looked at, and kept");
    assert_eq!(
        there("stat -c '%Y %i' \"$HOME\"/.local/share/olo/pool/olo-pool-host-test2; find \"${XDG_STATE_HOME:-$HOME/.local/state}\" -name pool-host.sock -exec stat -c '%Z %i' {} +").await,
        before,
        "nothing touched"
    );

    if let Ok(pause) = std::env::var("OLO_POOL_TEST_PAUSE") {
        println!("the pool: there, waiting for {pause}");
        let began = std::time::Instant::now();
        while !std::path::Path::new(&pause).exists() && began.elapsed() < Duration::from_secs(60) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Removed: the sign-in comes back whole first, and nothing of the
    // pool is left there.
    hosts.act("far", "remove").unwrap();
    let s = settled(&hosts, "removed").await;
    assert!(s.error.is_none() && !s.wanted, "{s:?}");
    if provider.is_none() {
        assert_eq!(ours(&desktop), ("old".into(), Some("r1".into()), None), "taken back whole");
        return;
    }
    assert_eq!(ours(&desktop), ("fresh-r1".into(), Some("r1+".into()), None), "taken back whole");
    desktop.pool.lock().unwrap().saved.credentials.get_mut("made-up").unwrap().expires_at = Some(now() + 30);
    assert_eq!(desktop.renewed("made-up", Provider::Claude).await.unwrap().access, "fresh-r1+", "and renewed here again");
    assert_eq!(renewals.lock().unwrap().as_slice(), ["r1", "r1+"], "no refresh token spent twice");
}

/// A host this computer has never met (its key in no known_hosts): the
/// pool, with nobody to ask, takes no key on trust, and says what to do.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs OLO_REX_TEST_HOST (the rig's far host) and a home that has never met it"]
async fn a_host_never_met_is_said_plainly() {
    let Ok(host) = std::env::var("OLO_REX_TEST_HOST") else { return };
    assert!(
        std::env::var_os("XDG_CONFIG_HOME").is_some_and(|d| std::path::Path::new(&d).starts_with("/var/tmp")),
        "hosts_live must run with XDG_CONFIG_HOME under /var/tmp so it never touches real ~/.config/olo"
    );
    let desktop = Shared::new(Saved::default(), None, Upstreams::fake("http://127.0.0.1:9"), || {});
    desktop.set_runtime(tokio::runtime::Handle::current());
    let hosts = Hosts::start(desktop.clone(), vec![Build { machines: &["x86_64", "aarch64"], bytes: b"never sent" }], "test1", || {});
    hosts.set_known(vec![("far".into(), "far".into(), host)]);
    hosts.act("far", "install").unwrap();
    let began = std::time::Instant::now();
    let s = loop {
        let s = hosts.list().into_iter().next().unwrap();
        if s.doing.is_none() {
            break s;
        }
        assert!(began.elapsed() < Duration::from_secs(30), "never ended: {s:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    println!("the pool: a host never met said {:?}", s.error);
    assert_eq!(s.error.as_deref(), Some("This computer hasn’t met that host yet. Connect to it in Rex first."));
    assert!(!s.wanted);
}
