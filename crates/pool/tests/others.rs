//! A program that isn't Claude Code or Codex but has their proxy setting
//! (one they run) is never opened up: its connection to a provider's host
//! is a tunnel, as without the pool. Off the network: tunnels are refused
//! here (`OLO_POOL_NO_TUNNEL`), so a refusal is how a tunnel shows.

use std::sync::Arc;

use olo_token_pool::{
    authority::Authority,
    gateway::Shared,
    intercept,
    model::{Account, Credential, Provider, now},
    store::Saved,
    upstream::Upstreams,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
async fn others_go_straight_through() {
    // SAFETY: set before anything in this test binary reads it.
    unsafe { std::env::set_var("OLO_POOL_NO_TUNNEL", "1") };
    let mut saved = Saved::default();
    let mut a = Account::new("claude-a".into(), "claude-a".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.push(a);
    saved.credentials.insert("claude-a".into(), Credential { access: "pooled".into(), refresh: None, expires_at: Some(now() + 3600), account_id: None });
    let shared = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
    let s = shared.clone();
    let connect = |who| {
        let (shared, s) = (shared.clone(), s.clone());
        async move {
            let proxy = intercept::serve(shared, Arc::new(Authority::ephemeral()), 0, Arc::new(move |p| s.serves(p)), who).await.unwrap();
            let mut c = tokio::net::TcpStream::connect(proxy).await.unwrap();
            c.write_all(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n").await.unwrap();
            let mut b = vec![0u8; 256];
            let n = c.read(&mut b).await.unwrap();
            String::from_utf8_lossy(&b[..n]).into_owned()
        }
    };
    // This test isn't Claude Code: a tunnel (refused here).
    assert!(connect(intercept::Who::Clients).await.starts_with("HTTP/1.1 403"));
    // Taken for one, it is opened.
    assert!(connect(intercept::Who::Anyone).await.starts_with("HTTP/1.1 200"));
}
