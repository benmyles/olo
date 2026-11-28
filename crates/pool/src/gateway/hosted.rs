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
use crate::model::{Account, Credential, ObservedLimit, Provider, RemoteClaim, now};
use crate::upstream::Failure;

/// Asks an account's keeper (a host, by its id) for its sign-in, good
/// for at least so many seconds more: the app's way to that host.
pub type Lender = Arc<dyn Fn(String, String, i64) -> Pin<Box<dyn Future<Output = Result<Credential, String>> + Send>> + Send + Sync>;

/// What a borrower pool asks an account's keeper to claim (`Claimer`):
/// either a specific account (manual "Use Reset Credit…"), or any account of
/// `provider` kept by that host so a `family` request can proceed (carrying
/// the borrower's exhaustion observations and a stable `op_id` + relative
/// `timeout_secs`).
#[derive(Debug, Clone)]
pub enum ClaimAsk {
    Account { id: String, credit: Option<String>, op_id: String, timeout_secs: Option<u64>, reconcile_only: bool, observed: Vec<ObservedLimit> },
    For { provider: Provider, family: String, op_id: String, timeout_secs: Option<u64>, reconcile_only: bool, observed: Vec<ObservedLimit> },
}

/// Asks an account's keeper (a host, by its id) to claim a reset credit.
/// Returns a distinct [`RemoteClaim`] (`Redeemed` vs `AlreadyReady`) on success.
pub type Claimer = Arc<dyn Fn(String, ClaimAsk) -> Pin<Box<dyn Future<Output = Result<RemoteClaim, super::Claimed>> + Send>> + Send + Sync>;

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
    /// Set `true` by v2+ desktops that delegate claiming to the host keeper
    /// (`Ask::Redeem`); absent (`false`) from legacy v1 desktops that still
    /// claim borrowed accounts directly.
    #[serde(default)]
    pub host_claims: bool,
    /// Authoritative set of providers covered by this carry payload. Omitted
    /// accounts are only deleted on the receiver if their provider is covered
    /// here (legacy v1/v2 desktops without this field only cover Claude+Codex,
    /// so they can never delete kept Antigravity credentials on an upgraded host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_providers: Option<Vec<Provider>>,
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
    a.kept_on.is_none() && renewable && a.redeeming.is_none() && a.kept_for_host(now).is_none_or(|h| h == host) && a.reauth_pending_host.as_deref().is_none_or(|h| h == host)
}

/// Whether a keeper's reply describes a sign-in/auth problem on the keeper
/// itself (as opposed to a transport/SSH reachability error).
fn is_keeper_auth_error(why: &str) -> bool {
    let low = why.to_ascii_lowercase();
    ["no refresh token", "sign in again", "no such account", "no sign-in", "invalid_grant", "signed out", "unauthorized", "refused", "revoked", "token expired"]
        .iter()
        .any(|needle| low.contains(needle))
}

fn keeper_failure(keeper: &str, why: &str) -> String {
    if is_keeper_auth_error(why) {
        format!("Its sign-in kept on {keeper} could not be renewed ({}).", why.trim_end_matches('.'))
    } else {
        format!("Its sign-in is kept on {keeper}, which can’t be reached now.")
    }
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

    /// The way to ask an account's keeper to claim a reset credit.
    pub fn set_claimer(&self, claimer: Option<Claimer>) {
        *self.claimer.lock().unwrap() = claimer;
    }

    pub(super) fn claimer(&self) -> Option<Claimer> {
        self.claimer.lock().unwrap().clone()
    }

    /// The sign-in of an account kept on `keeper`, for `fresh`: the one
    /// there is while it does; a newer one from the keeper when it is
    /// `due` (or was refused). Transport errors leave the account waiting
    /// (not signed out); explicit keeper auth errors are surfaced as such,
    /// and if this desktop still holds the refresh token (an unfinished
    /// handover), a re-carry to `keeper` is triggered immediately.
    pub(super) async fn borrowed(&self, id: &str, keeper: &str, current: Credential, due: bool, refused: bool) -> Result<Credential, Failure> {
        if !due {
            return Ok(current);
        }
        let lasts = current.expires_at.is_none_or(|t| t > now());
        let default_waits = || format!("Its sign-in is kept on {keeper}, which can’t be reached now.");
        let lender = self.lender.lock().unwrap().clone();
        let recent = self.asked.lock().unwrap().get(id).and_then(|(at, why)| (at.elapsed() < ASKED_AGAIN_AFTER).then(|| why.clone()));
        if lender.is_none() || recent.is_some() {
            return if lasts && !refused { Ok(current) } else { Err(Failure::Other(recent.unwrap_or_else(default_waits))) };
        }
        let lender = lender.unwrap();
        // If this desktop still holds the refresh token for an account marked
        // `kept_on == keeper`, handover never finished — trigger a re-carry so
        // the keeper gets the refresh token instead of looping Ask::Lend!
        if current.refresh.is_some() {
            if let Some(hosting) = self.hosting() {
                let _ = hosting.act(keeper, "sync");
            }
            let msg = format!("Its sign-in kept on {keeper} is waiting for sync to finish handing over credentials.");
            self.asked.lock().unwrap().insert(id.to_owned(), (std::time::Instant::now(), msg.clone()));
            return if lasts && !refused { Ok(current) } else { Err(Failure::Other(msg)) };
        }
        let lent = lender(keeper.to_owned(), id.to_owned(), BORROW_AHEAD + 600).await;
        // What the account holds now, the loan asked for: signed in here
        // meanwhile (its own sign-in, kept here now, refresh token and
        // all), that stays and is used, never the keeper's late loan
        // without one, nor the old token (Astra's review, 2026-11-28);
        // carried elsewhere, or removed: nothing written, no wait kept
        // for this keeper.
        let since = {
            let p = self.pool.lock().unwrap();
            super::since(&p, id, &current, Some(keeper))
        };
        match (lent, since) {
            (_, super::Since::Newer(newer)) => Ok(newer),
            (Ok(mut newer), held) => {
                // (Only what is used is kept here: never its refresh token.)
                newer.refresh = None;
                if newer.account_id.is_none() {
                    newer.account_id = current.account_id.clone();
                }
                self.asked.lock().unwrap().remove(id);
                let changed = newer.access != current.access || newer.account_id != current.account_id;
                match held {
                    super::Since::Held if changed => {
                        let newer = {
                            let mut p = self.pool.lock().unwrap();
                            // (Checked again as it's written: the lock was
                            // let go.)
                            match super::since(&p, id, &current, Some(keeper)) {
                                super::Since::Held => {}
                                super::Since::Newer(newer) => return Ok(newer),
                                super::Since::Moved => return Ok(newer),
                            }
                            let newer = super::with_found(&p, id, newer);
                            p.saved.credentials.insert(id.to_owned(), newer.clone());
                            newer
                        };
                        self.touch();
                        Ok(newer)
                    }
                    // (Carried as it was: the keeper's loan, for this
                    // request, kept by whoever keeps it now.)
                    super::Since::Moved if changed && self.pool.lock().unwrap().account(id).is_some() => Ok(newer),
                    super::Since::Held if lasts && !refused => Ok(current),
                    super::Since::Moved if self.pool.lock().unwrap().account(id).is_none() => Err(Failure::Refused("No sign-in".into())),
                    _ => Err(Failure::Other(default_waits())),
                }
            }
            (Err(why), held) => {
                tracing::info!("pool: asking {keeper} for a sign-in: {why}");
                let msg = keeper_failure(keeper, &why);
                if !matches!(held, super::Since::Held) {
                    return Err(Failure::Other(msg));
                }
                self.asked.lock().unwrap().insert(id.to_owned(), (std::time::Instant::now(), msg.clone()));
                if lasts && !refused { Ok(current) } else { Err(Failure::Other(msg)) }
            }
        }
    }

    /// A newer sign-in asked of the keeper for every account kept
    /// elsewhere whose own runs out within [`BORROW_AHEAD`]: with the
    /// quotas' reading, so a request never waits for it. Stopped once this
    /// copy has handed over (`drain.retired`).
    pub async fn borrow_ahead(&self) {
        if self.drain.retired() {
            return;
        }
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
            if self.drain.retired() {
                break;
            }
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
        let provider = self.pool.lock().unwrap().account(id).map(|a| a.provider);
        let gate = provider.map(|p| self.claims_gate(p));
        let _gate_held = match &gate {
            Some(g) => Some(g.lock().await),
            None => None,
        };
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        let c = {
            let mut p = self.pool.lock().unwrap();
            if p.account(id).is_some_and(|a| a.redeeming.is_some()) {
                return Err("A reset credit claim for this account is still pending; reconcile it before moving its sign-in.".into());
            }
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
        let mut removed = Vec::new();
        {
            let mut p = self.pool.lock().unwrap();
            if !p.loaded {
                return Err(p.error.clone().unwrap_or_else(|| "The pool’s accounts aren’t open yet.".into()));
            }
            // If carried by a legacy v1 desktop (`host_claims == false`), let it
            // keep claiming so we never double-claim; once a v2 desktop carries
            // (`host_claims == true`), this host claims autonomously.
            p.saved.legacy_desktop_claims = !carried.host_claims;
            for c in &carried.accounts {
                if p.account(&c.id).is_none() {
                    p.saved.accounts.push(Account::new(c.id.clone(), c.name.clone(), c.provider));
                }
                let kept_elsewhere = match &c.kept_on {
                    Some(host) if *host == carried.you => None,
                    Some(host) => Some(host.clone()),
                    None => Some(carried.from.clone()),
                };
                let prev_email = p.account(&c.id).and_then(|a| a.email.clone());
                let same_identity = match (&prev_email, &c.email) {
                    (Some(old), Some(new)) => old.eq_ignore_ascii_case(new),
                    _ => true,
                };
                let ours = p.saved.credentials.get(&c.id).cloned();
                let take = match (&c.credential, &ours) {
                    // A refresh token carried: this pool keeps it from now (a
                    // handover, or signed in anew on the desktop).
                    // Always accept when we have no credential or only a lent
                    // access token (`refresh: None`), even if `o.access == theirs.access`
                    // (promoting a borrower host to keeper!). When we already
                    // hold a refresh token as keeper, only replace on a genuinely
                    // newer re-sign-in so a replay never overwrites a rotated token.
                    (Some(theirs), ours) if theirs.refresh.is_some() && kept_elsewhere.is_none() => match ours {
                        None => Some(theirs.clone()),
                        Some(o) if o.refresh.is_none() => {
                            let mut next = theirs.clone();
                            if same_identity && next.account_id.is_none() {
                                next.account_id = o.account_id.clone();
                            }
                            if same_identity && super::newer(o, theirs) {
                                next.access = o.access.clone();
                                next.expires_at = o.expires_at;
                            }
                            Some(next)
                        }
                        Some(o) => {
                            (!same_identity || (o.refresh != theirs.refresh && o.access != theirs.access && theirs.expires_at.unwrap_or(i64::MAX) >= o.expires_at.unwrap_or(0))).then(|| theirs.clone())
                        }
                    },
                    // Lent: taken where this pool isn't the keeper, and it is newer.
                    (Some(theirs), ours) if kept_elsewhere.is_some() => {
                        let mut lent = theirs.clone();
                        lent.refresh = None;
                        if let Some(o) = ours
                            && same_identity
                            && lent.account_id.is_none()
                        {
                            lent.account_id = o.account_id.clone();
                        }
                        ours.as_ref().is_none_or(|o| !same_identity || (o.access != lent.access && lent.expires_at.unwrap_or(0) >= o.expires_at.unwrap_or(0))).then_some(lent)
                    }
                    _ => None,
                };
                if let Some(credential) = take {
                    p.saved.credentials.insert(c.id.clone(), credential);
                    renewed.push(c.id.clone());
                } else if let Some(mine) = p.saved.credentials.get_mut(&c.id) {
                    if kept_elsewhere.is_some() {
                        // (Kept elsewhere now: the refresh token given back goes.)
                        mine.refresh = None;
                    }
                    if !same_identity {
                        mine.account_id = None;
                    }
                }
                let a = p.account_mut(&c.id).expect("there or just added");
                (a.name, a.created, a.enabled, a.separate) = (c.name.clone(), c.created, c.enabled, c.separate);
                let is_renewed = renewed.contains(&c.id);
                a.email = c.email.clone().or(a.email.take());
                a.plan = c.plan.clone().or(a.plan.take());
                // On a replacement sign-in without an explicit org, drop any
                // stale org UUID so `/api/oauth/profile` heals the real one.
                a.org = if is_renewed && c.org.is_none() { None } else { c.org.clone().or(a.org.take()) };
                a.kept_on = kept_elsewhere;
                if a.kept_on.is_some() {
                    a.redeeming = None;
                }
                if is_renewed {
                    a.signed_out = None;
                    a.cooldown_until = None;
                }
            }
            // Only remove omitted accounts whose provider is authoritatively
            // covered by this carry payload (legacy desktops without
            // `covered_providers` only cover Claude + Codex, never Antigravity).
            let covered: &[Provider] = match carried.covered_providers.as_deref() {
                Some(list) => list,
                None => &[Provider::Claude, Provider::Codex],
            };
            let gone: Vec<String> = p.saved.accounts.iter().filter(|a| covered.contains(&a.provider) && !carried.accounts.iter().any(|c| c.id == a.id)).map(|a| a.id.clone()).collect();
            for id in gone {
                if p.busy(&id) || p.account(&id).is_some_and(|a| a.redeeming.is_some()) {
                    // (Answering or claiming now: paused, and gone the next time.)
                    if let Some(a) = p.account_mut(&id) {
                        a.enabled = false;
                    }
                } else {
                    p.saved.accounts.retain(|a| a.id != id);
                    p.saved.credentials.remove(&id);
                    p.resets.remove(&id);
                    p.reset_at.remove(&id);
                    p.last_reset.remove(&id);
                    p.keeper_reset_id.retain(|(_, aid), _| aid != &id);
                    removed.push(id);
                }
            }
        }
        for id in renewed.iter().chain(removed.iter()) {
            self.forget_kept(id);
        }
        self.touch();
        self.flush();
        self.sync_soon(None);
        self.capture(false);
        Ok(self.hosted_status())
    }

    /// As [`Self::carry_to_supported`], holding every provider's claim gate across
    /// the ownership transition so no in-flight local claim can race a handover.
    pub async fn carry_to_checked(&self, host: &str, from: &str) -> Carried {
        self.carry_to_checked_supported(host, from, &Provider::ALL).await
    }

    /// As [`Self::carry_to_checked`], restricting ownership handover and carried
    /// accounts to providers in `supported` (for rolling upgrades against older
    /// hosts that don't yet know `Provider::Antigravity`).
    pub async fn carry_to_checked_supported(&self, host: &str, from: &str, supported: &[Provider]) -> Carried {
        let gates: Vec<_> = Provider::ALL.into_iter().map(|p| self.claims_gate(p)).collect();
        let mut held = Vec::with_capacity(gates.len());
        for g in &gates {
            held.push(g.lock().await);
        }
        let carried = self.carry_to_supported(host, from, supported);
        drop(held);
        carried
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
        self.carry_to_supported(host, from, &Provider::ALL)
    }

    pub fn carry_to_supported(&self, host: &str, from: &str, supported: &[Provider]) -> Carried {
        let handed: Vec<String> = {
            let mut p = self.pool.lock().unwrap();
            // (Never one kept here for another host: a moving account's,
            // for the host it moves to; never a provider the host doesn't know.)
            let now = crate::model::now();
            let handed: Vec<String> = p
                .saved
                .accounts
                .iter()
                .filter(|a| supported.contains(&a.provider))
                .filter(|a| handed_to(a, p.saved.credentials.get(&a.id).is_some_and(|c| c.refresh.is_some()), host, now))
                .map(|a| a.id.clone())
                .collect();
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
        self.would_carry_supported(host, from, supported)
    }

    /// What [`Self::carry_to`] would carry to `host`, nothing marked: to
    /// know whether the host has what is here.
    pub fn would_carry(&self, host: &str, from: &str) -> Carried {
        self.would_carry_supported(host, from, &Provider::ALL)
    }

    pub fn would_carry_supported(&self, host: &str, from: &str, supported: &[Provider]) -> Carried {
        let p = self.pool.lock().unwrap();
        let accounts = p
            .saved
            .accounts
            .iter()
            .filter(|a| supported.contains(&a.provider))
            .map(|a| {
                // (One this pool still keeps would be handed to the host,
                // unless it is kept here for another.)
                let kept_for = a.kept_for_host(crate::model::now()).map(str::to_owned).or_else(|| a.reauth_pending_host.clone());
                let kept_on = a.kept_on.clone().or_else(|| match kept_for {
                    Some(other) if other != host => Some(other),
                    _ => (a.redeeming.is_none() && p.saved.credentials.get(&a.id).is_some_and(|c| c.refresh.is_some())).then(|| host.to_owned()),
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
        Carried { from: from.to_owned(), you: host.to_owned(), accounts, host_claims: true, covered_providers: Some(supported.to_vec()) }
    }

    /// `host` answered a carrying with how it stands: each account it
    /// says it keeps is its to renew, and its refresh token goes from
    /// here. Whether anything went.
    pub fn handed_over(&self, host: &str, status: &Value) -> bool {
        let kept: Vec<&str> = status["sign_ins"].as_array().map(|a| a.iter().filter(|s| s["kept"].as_bool() == Some(true)).filter_map(|s| s["id"].as_str()).collect()).unwrap_or_default();
        let mut went = false;
        let mut cleared_pending = false;
        {
            let mut p = self.pool.lock().unwrap();
            let ours: Vec<String> = p.saved.accounts.iter().filter(|a| a.kept_on.as_deref() == Some(host) && kept.contains(&a.id.as_str())).map(|a| a.id.clone()).collect();
            for id in ours {
                if let Some(a) = p.account_mut(&id)
                    && a.reauth_pending_host.as_deref() == Some(host)
                {
                    a.reauth_pending_host = None;
                    cleared_pending = true;
                }
                if let Some(c) = p.saved.credentials.get_mut(&id)
                    && c.refresh.take().is_some()
                {
                    went = true;
                }
            }
        }
        if went || cleared_pending {
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
                a.reauth_pending_host = None;
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
                if a.reauth_pending_host.as_deref().is_some_and(|h| !has_pool(h)) {
                    a.reauth_pending_host = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.touch();
            self.flush();
        }
    }

    /// Accounts marked `kept_on == Some(host)` that still hold a local
    /// refresh token here (handover not yet accepted by `host`).
    pub fn unaccepted_handovers(&self, host: &str) -> Vec<String> {
        let p = self.pool.lock().unwrap();
        p.saved.accounts.iter().filter(|a| a.kept_on.as_deref() == Some(host)).filter(|a| p.saved.credentials.get(&a.id).is_some_and(|c| c.refresh.is_some())).map(|a| a.id.clone()).collect()
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

    pub fn has_runtime(&self) -> bool {
        self.runtime.lock().unwrap().is_some()
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
        status["providers"] = json!(Provider::ALL);
        status["redeems"] = json!(!p.saved.legacy_desktop_claims);
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
