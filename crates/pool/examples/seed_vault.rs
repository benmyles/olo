//! For trying the pool by hand (HACKING's "Upgrading without a gap"): a
//! `pool.vault` in `$XDG_CONFIG_HOME/olo` with one made-up Claude account
//! signed in for eight hours, sealed with the key in `OLO_POOL_KEYFILE`.
//! Never a real account: point `OLO_POOL_UPSTREAM` at a fake provider.
//! `--meters`: a Claude and a Codex account besides, with every kind of
//! meter their providers report (a session not started, the week, Fable's
//! and Sonnet's own weeks; a model's limit of its own, code review's), to
//! see the window show them.

use olo_token_pool::{
    model::{Account, Credential, Provider, Window, now},
    store::Store,
};

fn window(name: &str, label: &str, used: f64, resets_in: Option<i64>, models: &[&str]) -> Window {
    Window { name: name.into(), label: label.into(), used, resets_at: resets_in.map(|s| now() + s), models: models.iter().map(|m| m.to_string()).collect() }
}

fn main() {
    assert!(std::env::var_os("OLO_POOL_KEYFILE").is_some(), "OLO_POOL_KEYFILE: the test key's file");
    let (store, mut saved) = Store::open().expect("the pool's file");
    let mut a = Account::new("test-claude".into(), "Test Claude".into(), Provider::Claude);
    a.created = 1;
    let mut accounts = vec![a];
    if std::env::args().any(|a| a == "--meters") {
        let mut claude = Account::new("test-claude-meters".into(), "Claude, every meter".into(), Provider::Claude);
        claude.plan = Some("max".into());
        claude.synced = Some(now());
        claude.windows = vec![
            window("five_hour", "5-hour", 0.0, None, &[]),
            window("seven_day", "Weekly", 26.0, Some(3 * 86400 + 4000), &[]),
            window("seven_day_fable", "Weekly Fable", 61.5, Some(3 * 86400 + 4000), &["fable"]),
            window("seven_day_sonnet", "Weekly Sonnet", 4.0, Some(3 * 86400 + 4000), &["sonnet"]),
        ];
        let mut codex = Account::new("test-codex-meters".into(), "Codex, every meter".into(), Provider::Codex);
        codex.plan = Some("pro".into());
        codex.synced = Some(now());
        codex.windows = vec![
            window("primary_window", "Weekly", 12.0, Some(5 * 86400), &[]),
            window("codex_bengalfox/primary_window", "GPT-5.3-Codex-Spark 5-hour", 84.0, Some(9000), &["gpt-5.3-codex-spark"]),
            window("codex_bengalfox/secondary_window", "GPT-5.3-Codex-Spark Weekly", 5.0, Some(6 * 86400), &["gpt-5.3-codex-spark"]),
            window("code_review/primary_window", "Code review Weekly", 0.0, None, &["code-review"]),
        ];
        accounts.push(claude);
        accounts.push(codex);
    }
    for a in accounts {
        saved.accounts.retain(|x| x.id != a.id);
        saved.credentials.insert(a.id.clone(), Credential { access: "pooled-test".into(), refresh: Some("r1".into()), expires_at: Some(now() + 8 * 3600), account_id: None });
        saved.accounts.push(a);
    }
    store.save(&saved).expect("saving the pool");
    println!("{}", olo_token_pool::store::path().display());
}
