//! For trying the pool by hand (HACKING's "Upgrading without a gap"): a
//! `pool.vault` in `$XDG_CONFIG_HOME/olo` with one made-up Claude account
//! signed in for eight hours, sealed with the key in `OLO_POOL_KEYFILE`.
//! Never a real account: point `OLO_POOL_UPSTREAM` at a fake provider.

use olo_token_pool::{
    model::{Account, Credential, Provider, now},
    store::Store,
};

fn main() {
    assert!(std::env::var_os("OLO_POOL_KEYFILE").is_some(), "OLO_POOL_KEYFILE: the test key's file");
    let (store, mut saved) = Store::open().expect("the pool's file");
    let mut a = Account::new("test-claude".into(), "Test Claude".into(), Provider::Claude);
    a.created = 1;
    saved.accounts.retain(|x| x.id != a.id);
    saved.accounts.push(a);
    saved.credentials.insert("test-claude".into(), Credential { access: "pooled-test".into(), refresh: Some("r1".into()), expires_at: Some(now() + 8 * 3600), account_id: None });
    store.save(&saved).expect("saving the pool");
    println!("{}", olo_token_pool::store::path().display());
}
