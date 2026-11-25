//! One pool's accounts on several machines: the desktop's pool, and a
//! pool of its own on each Rex host that has one (Ben, 2026-11-23: "it
//! shouldn't proxy through the token pool running on the user's desktop
//! because then the coding agents couldn't continue when the desktop is
//! off. what it should do is install a token pool service … that
//! automatically gets the same config/accounts as the desktop token
//! pool … the remote service itself should not require the desktop
//! token pool available, and the remote service should send requests
//! for those remote agents directly to claude/openai etc").
//!
//! **One pool renews each sign-in.** A provider's refresh token is
//! spent once: renewed by two pools, the second is refused, and a
//! provider may end the whole sign-in for it. So each account has one
//! keeper, the pool that holds its refresh token and renews it; every
//! other pool that has the account holds only its access token, which
//! all may use at once, and takes a newer one from the keeper as it
//! comes due. Since a host's pool must go on with the desktop off, an
//! account carried to a host is kept there: the desktop hands over the
//! refresh token ([`Shared::carried`]), marks the account `kept_on`
//! that host, and from then on borrows ([`Shared::borrowed`], through
//! a [`Lender`] the app gives it, which asks the host:
//! [`Shared::lend`]). Taken back ([`Shared::give_back`]) when the
//! host's pool is removed. Where the keeper can't be reached and the
//! access token has run out, the account waits, said so, and isn't
//! signed out: it is whole again when the keeper is next reached.
//!
//! What is carried never lowers what a pool has: an account's use, its
//! meters and its cooldowns are each pool's own reading of the
//! provider.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shared;
use crate::model::{Account, Credential, Provider, now};
use crate::upstream::Failure;

/// Asks an account's keeper (a host, by its id) for its sign-in, good
/// for at least so many seconds more: the app's way to that host.
pub type Lender = Arc<dyn Fn(String, String, i64) -> Pin<Box<dyn Future<Output = Result<Credential, String>> + Send>> + Send + Sync>;

/// The pools on Rex hosts, as the gateway shows and changes them (the
/// desktop's `hosts::Hosts`).
pub trait Hosting: Send + Sync {
    fn list(&self) -> Value;
    fn act(self: Arc<Self>, id: &str, what: &str) -> Result<(), String>;
}

/// How long before a borrowed sign-in runs out a newer one is asked
/// for (as the accounts' quotas are read, every few minutes: so a
/// keeper out of reach for a while costs nothing).
pub const BORROW_AHEAD: i64 = 2 * 3600;
/// A keeper asked for a longer-lived sign-in renews one no sooner
/// than this after its last renewal, whoever asked for that (one whose
/// sign-ins last less than [`BORROW_AHEAD`] would else be renewed at
/// every asking, and one just renewed by the keeper for itself again
/// for its first borrower: seen in the first run over SSH).
const LEND_RENEWS_EVERY: Duration = Duration::from_secs(30 * 60);
/// A keeper that couldn't be reached isn't asked again for so long (a
/// request waits for none of this: it has its answer at once).
const ASKED_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// What the desktop's pool carries to a host's: every account, as it
/// is to be there.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Carried {
    /// Who carries (the desktop pool's name for itself, for people).
    pub from: String,
    /// The host's id as the carrier knows it: what `kept_on` says when
    /// the host itself is the keeper.
    pub you: String,
    pub accounts: Vec<CarriedAccount>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CarriedAccount {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub created: i64,
    pub enabled: bool,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub separate: bool,
    /// Its keeper: `you` (this host keeps it), another host's id, or
    /// none (the carrier does).
    #[serde(default)]
    pub kept_on: Option<String>,
    /// Its sign-in as the carrier has it: with a refresh token only
    /// when the host is to keep it and the carrier still holds one (a
    /// handover, or a sign-in made anew on the desktop).
    #[serde(default)]
    pub credential: Option<Credential>,
}

impl std::fmt::Debug for CarriedAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarriedAccount").field("id", &self.id).field("name", &self.name).field("kept_on", &self.kept_on).finish_non_exhaustive()
    }
}

/// Whether account `a` (with a refresh token here: `renewable`) is handed
/// to `host` to keep: it is kept here, and not kept here for another host
/// (a moving account's, for the host it moves to).
fn handed_to(a: &crate::model::Account, renewable: bool, host: &str, now: i64) -> bool {
    a.kept_on.is_none() && renewable && a.kept_for_host(now).is_none_or(|h| h == host)
}

impl Shared {
    /// This pool is a Rex host's from now on.
    pub fn set_hosted(&self) {
        self.hosted.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_hosted(&self) -> bool {
        self.hosted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Where this pool's own work runs, for one that doesn't serve (a
    /// test's: the gateway sets its own as it starts).
    pub fn set_runtime(&self, runtime: tokio::runtime::Handle) {
        *self.runtime.lock().unwrap() = Some(runtime);
    }

    pub fn set_hosting(&self, hosting: Option<Arc<dyn Hosting>>) {
        *self.hosting.lock().unwrap() = hosting;
    }

    pub fn hosting(&self) -> Option<Arc<dyn Hosting>> {
        self.hosting.lock().unwrap().clone()
    }

    /// How the hosts' pools stand has changed (`hosts::Hosts` says so
    /// at each change): whoever waits on `/pool/hosts?after=` is
    /// answered.
    pub fn hosts_changed(&self) {
        self.hosts_seen.send_modify(|stamp| *stamp += 1);
    }

    pub fn hosts_seen(&self) -> tokio::sync::watch::Receiver<u64> {
        self.hosts_seen.subscribe()
    }

    /// `told` is called (from any thread) whenever the accounts or
    /// their sign-ins change here.
    pub fn on_accounts(&self, told: impl Fn() + Send + Sync + 'static) {
        *self.accounts_changed.lock().unwrap() = Some(Box::new(told));
    }

    /// The way to the keepers of the accounts kept elsewhere.
    pub fn set_lender(&self, lender: Option<Lender>) {
        *self.lender.lock().unwrap() = lender;
    }

    /// The sign-in of an account kept on `keeper`, for `fresh`: the one
    /// there is while it does; a newer one from the keeper when it is
    /// `due` (or was refused). The keeper out of reach: the one there
    /// is, as long as it lasts, and then the account waits (an error
    /// that isn't a refusal: it is not signed out).
    pub(super) async fn borrowed(&self, id: &str, keeper: &str, current: Credential, due: bool, refused: bool) -> Result<Credential, Failure> {
        if !due {
            return Ok(current);
        }
        let lasts = current.expires_at.is_none_or(|t| t > now());
        let waits = || Failure::Other(format!("Its sign-in is kept on {keeper}, which can’t be reached now."));
        let lender = self.lender.lock().unwrap().clone();
        let lately = self.asked.lock().unwrap().get(id).is_some_and(|at| at.elapsed() < ASKED_AGAIN_AFTER);
        let (Some(lender), false) = (lender, lately) else {
            return if lasts && !refused { Ok(current) } else { Err(waits()) };
        };
        match lender(keeper.to_owned(), id.to_owned(), BORROW_AHEAD + 600).await {
            Ok(mut newer) => {
                // (Only what is used is kept here: never its refresh token.)
                newer.refresh = None;
                self.asked.lock().unwrap().remove(id);
                if newer.access != current.access {
                    self.pool.lock().unwrap().saved.credentials.insert(id.to_owned(), newer.clone());
                    self.touch();
                    return Ok(newer);
                }
                if lasts && !refused { Ok(current) } else { Err(waits()) }
            }
            Err(why) => {
                tracing::info!("pool: asking {keeper} for a sign-in: {why}");
                self.asked.lock().unwrap().insert(id.to_owned(), std::time::Instant::now());
                if lasts && !refused { Ok(current) } else { Err(waits()) }
            }
        }
    }

    /// A newer sign-in asked of the keeper for every account kept
    /// elsewhere whose own runs out within [`BORROW_AHEAD`]: with the
    /// quotas' reading, so a request never waits for it.
    pub async fn borrow_ahead(&self) {
        let due: Vec<(String, Provider)> = {
            let p = self.pool.lock().unwrap();
            let t = now();
            p.saved
                .accounts
                .iter()
                .filter(|a| a.kept_on.is_some() && a.enabled)
                .filter(|a| p.saved.credentials.get(&a.id).is_some_and(|c| c.expires_at.is_some_and(|e| e - t < BORROW_AHEAD)))
                .map(|a| (a.id.clone(), a.provider))
                .collect()
        };
        for (id, provider) in due {
            let _ = self.fresh(&id, provider, None, BORROW_AHEAD).await;
        }
    }

    /// The keeper's side: account `id`'s sign-in for a borrower, good
    /// for `at_least` seconds more where that can be had (renewed first
    /// if it runs out sooner, but not twice in half an hour), without
    /// its refresh token.
    pub async fn lend(&self, id: &str, at_least: i64) -> Result<Credential, String> {
        let (provider, kept_elsewhere) = {
            let p = self.pool.lock().unwrap();
            let a = p.account(id).ok_or("no such account")?;
            (a.provider, a.kept_on.is_some())
        };
        let lately = self.lent.lock().unwrap().get(id).is_some_and(|at| at.elapsed() < LEND_RENEWS_EVERY);
        let within = if kept_elsewhere || lately { 60 } else { at_least };
        let mut c = self.fresh(id, provider, None, within).await.map_err(|e| e.to_string())?;
        c.refresh = None;
        Ok(c)
    }

    /// The keeper gives an account's sign-in back, whole, to `to`, and
    /// renews it no more: asked again, it gives the same (an answer
    /// lost on the way loses nothing). Its refresh token goes from here
    /// when what is next carried says the account is kept elsewhere.
    pub async fn give_back(&self, id: &str, to: &str) -> Result<Credential, String> {
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        let c = {
            let mut p = self.pool.lock().unwrap();
            let c = p.saved.credentials.get(id).cloned().ok_or("no sign-in for it here")?;
            if c.refresh.is_none() {
                return Err("its sign-in isn’t kept here".into());
            }
            if let Some(a) = p.account_mut(id) {
                a.kept_on = Some(to.to_owned());
            }
            c
        };
        self.touch();
        self.flush();
        Ok(c)
    }

    /// What the desktop's pool carried, taken in: its accounts are this
    /// pool's accounts (new ones added, ones no longer carried removed
    /// unless they are answering now, names and pauses followed), and
    /// each sign-in as [`CarriedAccount::credential`] says. Answers how
    /// the pool stands after ([`Shared::hosted_status`]).
    pub fn carried(self: &Arc<Self>, carried: Carried) -> Result<Value, String> {
        let mut renewed = Vec::new();
        {
            let mut p = self.pool.lock().unwrap();
            if !p.loaded {
                return Err(p.error.clone().unwrap_or_else(|| "The pool’s accounts aren’t open yet.".into()));
            }
            for c in &carried.accounts {
                if p.account(&c.id).is_none() {
                    p.saved.accounts.push(Account::new(c.id.clone(), c.name.clone(), c.provider));
                }
                let kept_elsewhere = match &c.kept_on {
                    Some(host) if *host == carried.you => None,
                    Some(host) => Some(host.clone()),
                    None => Some(carried.from.clone()),
                };
                let ours = p.saved.credentials.get(&c.id).cloned();
                let take = match (&c.credential, &ours) {
                    // A refresh token carried: this pool keeps it from now (a
                    // handover, or signed in anew on the desktop), unless it
                    // is the very one it has already.
                    (Some(theirs), ours) if theirs.refresh.is_some() && kept_elsewhere.is_none() => {
                        ours.as_ref().is_none_or(|o| o.refresh != theirs.refresh && o.access != theirs.access).then(|| theirs.clone())
                    }
                    // Lent: taken where this pool isn't the keeper, and it is newer.
                    (Some(theirs), ours) if kept_elsewhere.is_some() => {
                        let mut lent = theirs.clone();
                        lent.refresh = None;
                        ours.as_ref().is_none_or(|o| o.access != lent.access && lent.expires_at.unwrap_or(0) >= o.expires_at.unwrap_or(0)).then_some(lent)
                    }
                    _ => None,
                };
                if let Some(credential) = take {
                    p.saved.credentials.insert(c.id.clone(), credential);
                    renewed.push(c.id.clone());
                } else if kept_elsewhere.is_some()
                    && let Some(mine) = p.saved.credentials.get_mut(&c.id)
                {
                    // (Kept elsewhere now: the refresh token given back goes.)
                    mine.refresh = None;
                }
                let a = p.account_mut(&c.id).expect("there or just added");
                (a.name, a.created, a.enabled, a.separate) = (c.name.clone(), c.created, c.enabled, c.separate);
                (a.email, a.plan, a.org) = (c.email.clone().or(a.email.take()), c.plan.clone().or(a.plan.take()), c.org.clone().or(a.org.take()));
                a.kept_on = kept_elsewhere;
                if renewed.contains(&c.id) {
                    a.signed_out = None;
                    a.cooldown_until = None;
                }
            }
            let gone: Vec<String> = p.saved.accounts.iter().filter(|a| !carried.accounts.iter().any(|c| c.id == a.id)).map(|a| a.id.clone()).collect();
            for id in gone {
                if p.busy(&id) {
                    // (Answering now: paused, and gone the next time.)
                    if let Some(a) = p.account_mut(&id) {
                        a.enabled = false;
                    }
                } else {
                    p.saved.accounts.retain(|a| a.id != id);
                    p.saved.credentials.remove(&id);
                }
            }
        }
        for id in &renewed {
            self.forget_kept(id);
        }
        self.touch();
        self.flush();
        self.sync_soon(None);
        Ok(self.hosted_status())
    }

    /// The desktop's side. What is carried to `host` now: every
    /// account, and for each its sign-in as that host is to have it.
    /// An account this pool keeps is handed to the host with this:
    /// marked `kept_on` it first, and saved, so that from here on this
    /// pool never renews it, whatever becomes of the message (a token
    /// both might spend is the one thing that must not be); its refresh
    /// token goes with every carrying until the host says it keeps it
    /// ([`Self::handed_over`]). One kept on another host is carried as
    /// lent, with the access token there is.
    pub fn carry_to(&self, host: &str, from: &str) -> Carried {
        let handed: Vec<String> = {
            let mut p = self.pool.lock().unwrap();
            // (Never one kept here for another host: a moving account's,
            // for the host it moves to.)
            let now = crate::model::now();
            let handed: Vec<String> = p.saved.accounts.iter().filter(|a| handed_to(a, p.saved.credentials.get(&a.id).is_some_and(|c| c.refresh.is_some()), host, now)).map(|a| a.id.clone()).collect();
            for id in &handed {
                if let Some(a) = p.account_mut(id) {
                    a.kept_on = Some(host.to_owned());
                    a.kept_for = None;
                }
            }
            handed
        };
        if !handed.is_empty() {
            self.touch();
            self.flush();
        }
        self.would_carry(host, from)
    }

    /// What [`Self::carry_to`] would carry to `host`, nothing marked: to
    /// know whether the host has what is here.
    pub fn would_carry(&self, host: &str, from: &str) -> Carried {
        let p = self.pool.lock().unwrap();
        let accounts = p
            .saved
            .accounts
            .iter()
            .map(|a| {
                // (One this pool still keeps would be handed to the host,
                // unless it is kept here for another.)
                let kept_for = a.kept_for_host(crate::model::now()).map(str::to_owned);
                let kept_on = a.kept_on.clone().or_else(|| match kept_for {
                    Some(other) if other != host => Some(other),
                    _ => p.saved.credentials.get(&a.id).is_some_and(|c| c.refresh.is_some()).then(|| host.to_owned()),
                });
                let credential = p.saved.credentials.get(&a.id).cloned().map(|mut c| {
                    if kept_on.as_deref() != Some(host) {
                        c.refresh = None;
                    }
                    c
                });
                CarriedAccount {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    provider: a.provider,
                    created: a.created,
                    enabled: a.enabled,
                    email: a.email.clone(),
                    plan: a.plan.clone(),
                    org: a.org.clone(),
                    separate: a.separate,
                    kept_on,
                    credential,
                }
            })
            .collect();
        Carried { from: from.to_owned(), you: host.to_owned(), accounts }
    }

    /// `host` answered a carrying with how it stands: each account it
    /// says it keeps is its to renew, and its refresh token goes from
    /// here. Whether anything went.
    pub fn handed_over(&self, host: &str, status: &Value) -> bool {
        let kept: Vec<&str> = status["sign_ins"].as_array().map(|a| a.iter().filter(|s| s["kept"].as_bool() == Some(true)).filter_map(|s| s["id"].as_str()).collect()).unwrap_or_default();
        let mut went = false;
        {
            let mut p = self.pool.lock().unwrap();
            let ours: Vec<String> = p.saved.accounts.iter().filter(|a| a.kept_on.as_deref() == Some(host) && kept.contains(&a.id.as_str())).map(|a| a.id.clone()).collect();
            for id in ours {
                if let Some(c) = p.saved.credentials.get_mut(&id)
                    && c.refresh.take().is_some()
                {
                    went = true;
                }
            }
        }
        if went {
            self.touch();
            self.flush();
        }
        went
    }

    /// An account's sign-in came back whole from the host that kept it
    /// ([`Self::give_back`] there): this pool keeps it again.
    pub fn took_back(&self, id: &str, credential: Credential) {
        self.took_back_for(id, credential, None);
    }

    /// As [`Self::took_back`], kept here for host `for_host` alone (an
    /// account moving to it, Rex's move: the next host carried to is never
    /// another's).
    pub fn took_back_for(&self, id: &str, credential: Credential, for_host: Option<&str>) {
        {
            let mut p = self.pool.lock().unwrap();
            p.saved.credentials.insert(id.to_owned(), credential);
            if let Some(a) = p.account_mut(id) {
                a.kept_on = None;
                a.kept_for = for_host.map(|h| (h.to_owned(), crate::model::now()));
            }
        }
        self.touch();
        self.flush();
    }

    /// What is kept here for a host that no longer has a pool let go of:
    /// any host may have it again (Go Back, a move given up).
    pub fn release_kept_for(&self, has_pool: &dyn Fn(&str) -> bool) {
        let mut changed = false;
        {
            let mut p = self.pool.lock().unwrap();
            for a in p.saved.accounts.iter_mut() {
                if a.kept_for.as_ref().is_some_and(|(h, _)| !has_pool(h)) {
                    a.kept_for = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.touch();
            self.flush();
        }
    }

    /// The accounts kept on `host`.
    pub fn kept_on(&self, host: &str) -> Vec<String> {
        self.pool.lock().unwrap().saved.accounts.iter().filter(|a| a.kept_on.as_deref() == Some(host)).map(|a| a.id.clone()).collect()
    }

    /// `work` done on the gateway's own threads and waited for, from a
    /// thread that isn't one of them (the host's control socket's).
    /// None before the gateway runs.
    pub fn wait<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> Option<T> {
        let runtime = self.runtime.lock().unwrap().clone()?;
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let _ = tx.send(work.await);
        });
        rx.recv_timeout(Duration::from_secs(90)).ok()
    }

    /// How this pool stands, for the pool that carries its accounts:
    /// the status agents read (no sign-ins), and for each account
    /// whether its sign-in is kept here and until when it lasts.
    pub fn hosted_status(&self) -> Value {
        let p = self.pool.lock().unwrap();
        let mut status = super::status_json(&p);
        let kept: Vec<Value> = p
            .saved
            .accounts
            .iter()
            .map(|a| {
                let c = p.saved.credentials.get(&a.id);
                json!({"id": a.id, "kept": c.is_some_and(|c| c.refresh.is_some()) && a.kept_on.is_none(), "kept_on": a.kept_on, "expires_at": c.and_then(|c| c.expires_at)})
            })
            .collect();
        status["sign_ins"] = Value::Array(kept);
        status["hosted"] = json!(self.is_hosted());
        status["version"] = json!(crate::host::VERSION);
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A moving account's sign-in, taken back from its old host, is handed
    /// to the host it moves to and to no other; a day later, or not kept
    /// for anyone, to whichever is carried to.
    #[test]
    fn a_moving_accounts_sign_in_goes_only_where_it_moves() {
        let mut a = crate::model::Account::new("a1".into(), "Work".into(), crate::model::Provider::Claude);
        let now = 1_000_000;
        a.kept_for = Some(("dest".into(), now - 60));
        assert!(handed_to(&a, true, "dest", now));
        assert!(!handed_to(&a, true, "third", now), "another host never gets it meanwhile");
        assert!(handed_to(&a, true, "third", now + 24 * 3600), "a day on, anyone's");
        a.kept_for = None;
        assert!(handed_to(&a, true, "third", now));
        assert!(!handed_to(&a, false, "dest", now), "nothing to keep without its refresh token");
        a.kept_on = Some("dest".into());
        assert!(!handed_to(&a, true, "dest", now), "kept on a host already");
    }
}
