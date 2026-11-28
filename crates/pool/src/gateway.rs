//! The gateway: how a request is served from the pool, on loopback. Claude
//! Code and Codex reach it through the proxy (`intercept`), as if talking
//! to their own hosts; the gateway's own HTTP server (`/claude/v1/…`,
//! `/codex/v1/…`, `/pool/status`) takes requests from clients pointed at
//! it by address too. Each request goes to the account the scheduler picks, with
//! that account's sign-in in place of the client's; a refused sign-in is
//! refreshed once, a rejection for quota moves to the next account, both
//! before any of the answer has been passed on. A request the network
//! failed (it changed under it, a cable pulled or Wi-Fi left, and its
//! connection was cut with the network that went; or there was none, or
//! no connection was made) is tried again on the network there is now,
//! once there's one, before any of the answer has been passed on: what the
//! client would do itself, without its wait. Anything else (a server
//! error, a broken stream) is passed back as it is, as the work upstream
//! may have happened: the client's own retries decide. Requests go out on
//! a client made for the network there is (`olo_net::PerNetwork`), so
//! after a change they use the new one, not idle connections on the old.
//! Answers stream through; only their usage counts are kept, never what
//! was asked or answered.

use olo_sync::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};

mod hosted;
pub use hosted::{Carried, CarriedAccount, ClaimAsk, Claimer, Hosting, Lender};

use crate::{
    activity::Activity,
    model::{Account, ClearedScope, Credential, ObservedLimit, Outcome, PendingRemoteClaim, Provider, RemoteClaim, RemoteClaimRecord, Request, Usage, Window, model_family, now, today},
    schedule,
    store::{Saved, Store},
    upstream::{self, Failure, Upstreams},
};

/// Where the gateway listens unless `OLO_POOL_PORT` says otherwise.
pub const PORT: u16 = 17891;

pub fn port() -> u16 {
    std::env::var("OLO_POOL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT)
}

/// A request served (for the window's activity list): never its content.
#[derive(Debug, Clone, Serialize)]
pub struct Served {
    pub account: String,
    pub account_id: String,
    pub provider: Provider,
    #[serde(flatten)]
    pub request: Request,
}

/// A request going through the pool now.
#[derive(Debug, Clone, Serialize)]
pub struct Open {
    pub id: u64,
    pub account: String,
    pub account_id: String,
    pub provider: Provider,
    pub model: String,
    pub client: String,
    /// When it was sent on (milliseconds since the epoch).
    pub started: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The program a request came from, by its user agent.
fn client_of(headers: &HeaderMap) -> String {
    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
    let first = ua.split(['/', ' ']).next().unwrap_or("");
    match first {
        "claude-cli" | "claude-code" => "Claude Code".into(),
        f if f.starts_with("codex") => "Codex".into(),
        f if f.eq_ignore_ascii_case("antigravity") => "Antigravity".into(),
        "" => "Unknown".into(),
        f => f.chars().take(40).collect(),
    }
}

/// Everything the gateway and the window share.
#[derive(Default)]
pub struct Pool {
    pub saved: Saved,
    /// The account lately serving each provider and model family.
    sticky: HashMap<(Provider, String), String>,
    /// Requests now going through each account.
    pub in_flight: HashMap<String, u32>,
    /// The latest requests, newest last.
    pub served: VecDeque<Served>,
    /// Requests going through now, oldest first.
    pub open: Vec<Open>,
    next_open: u64,
    /// Why the gateway isn't listening (or the accounts can't be
    /// opened), if so.
    pub error: Option<String>,
    /// Each minute's requests by client, over the last day.
    pub activity: Activity,
    /// The saved accounts have been read (until then nothing is saved,
    /// so a keyring that isn't open yet can't cost them).
    pub loaded: bool,
    /// Each account's latest confirmed reset. An observation from an older
    /// revision cannot put the account back at its former limit.
    resets: HashMap<String, u64>,
    /// When each account's latest reset was confirmed here: what the copy
    /// handed over from saw before it is old ([`Late::Seen`]).
    reset_at: HashMap<String, i64>,
    /// Causal scope of the latest local reset completion per account (on a
    /// keeper), so borrowers and keepers order observations and selectively
    /// clear windows without relying on cross-machine wall clocks.
    last_reset: HashMap<String, ClearedScope>,
    /// Last `reset_id` seen by this borrower from `(keeper, account)`.
    keeper_reset_id: HashMap<(String, String), String>,
}

impl Pool {
    pub fn account(&self, id: &str) -> Option<&Account> {
        self.saved.accounts.iter().find(|a| a.id == id)
    }
    pub fn account_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.saved.accounts.iter_mut().find(|a| a.id == id)
    }
    fn reset_revision(&self, id: &str) -> u64 {
        self.resets.get(id).copied().unwrap_or(0)
    }

    fn pick<'a>(&'a self, provider: Provider, family: &str, sticky: Option<&str>, tried: &HashMap<String, u64>, reconsider: &[String]) -> Option<&'a Account> {
        schedule::pick(&self.saved.accounts, provider, family, sticky, now(), &[], |a| {
            self.saved.credentials.contains_key(&a.id) && (reconsider.contains(&a.id) || tried.get(&a.id) != Some(&self.reset_revision(&a.id)))
        })
    }

    /// The account to use when none has allowance left and no reset
    /// could be claimed: one its provider answers from usage credits
    /// (`schedule::pick_paid`), of those not tried for this request.
    fn pick_paid<'a>(&'a self, provider: Provider, family: &str, sticky: Option<&str>, tried: &HashMap<String, u64>) -> Option<&'a Account> {
        schedule::pick_paid(&self.saved.accounts, provider, family, sticky, now(), |a| self.saved.credentials.contains_key(&a.id) && tried.get(&a.id) != Some(&self.reset_revision(&a.id)))
    }

    pub fn busy(&self, id: &str) -> bool {
        self.in_flight.get(id).is_some_and(|&n| n > 0)
    }
}

fn provider_index(p: Provider) -> usize {
    match p {
        Provider::Claude => 0,
        Provider::Codex => 1,
        Provider::Antigravity => 2,
    }
}

/// The gateway's state, shared with the app.
/// What the saver's told (`touch`, `noted`, `flush`).
enum Save {
    /// Something of the accounts changed: written a moment later.
    Now,
    /// A request's bookkeeping (when used, what it used): written with the
    /// next save, or `USAGE_EVERY` after the first such at the latest.
    Later,
    /// Write what's waiting now, then say so (before a handover).
    Flush(std::sync::mpsc::Sender<()>),
}

/// The longest a request's bookkeeping waits to be written (Ben's pool,
/// 2026-11-15: each request asked for two saves, so with agents at work
/// the sealed file was written more than once a second, 38 times in 30 s:
/// disk, sealing, and a wake for everything watching the config folder).
pub const USAGE_EVERY: Duration = Duration::from_secs(30);

pub struct Shared {
    pub pool: Mutex<Pool>,
    store: Mutex<Option<Store>>,
    /// The HTTP client for the network there is now.
    http: olo_net::PerNetwork<reqwest::Client>,
    up: Upstreams,
    /// One refresh at a time per account.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One profile/project discovery at a time per account.
    profile_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Whether this pool reads/deletes per-account capture files on disk
    /// (`false` for in-memory test pools so they never touch `~/.local/state`).
    capture_disk: bool,
    save: std::sync::mpsc::Sender<Save>,
    changed: Box<dyn Fn() + Send + Sync>,
    runtime: Mutex<Option<tokio::runtime::Handle>>,
    /// Make the sealed store when there's first something to keep.
    lazy: bool,
    /// When each account's quota is next read on its own, and how long
    /// the provider last made it wait (a 429 doubles it).
    due: Mutex<HashMap<String, (i64, i64)>>,
    /// When each account's reset credits are next read.
    credits_due: Mutex<HashMap<String, i64>>,
    /// When each account's reset credits were last read (or tried on exhaustion),
    /// to avoid hammering the provider during a burst of 429s.
    credits_fetched: Mutex<HashMap<String, i64>>,
    /// What is under way, for an upgrade to wait for (`drain`).
    pub drain: crate::drain::Drain,
    /// The clients the person turned the pool off for, still pointed at
    /// its proxy (`clients::set`): their requests pass straight through.
    paused: [std::sync::atomic::AtomicBool; 3],
    /// What opens the app's window when asked over the gateway
    /// (`/pool/open`: a launch that found no copy taking launches, while
    /// this one serves).
    opener: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Each account's capture, read once (None: it has none yet), kept as
    /// the pool makes another; and how often each account has left the
    /// pool or signed in again, so a capture made before isn't kept.
    captures: Mutex<HashMap<String, Option<Arc<crate::capture::Capture>>>>,
    captures_gen: Mutex<HashMap<String, u64>>,
    jars: Mutex<HashMap<String, crate::likeness::Jar>>,
    /// The account whose client is being captured now (`capture`), for
    /// the window and agents; whether a capture runs, and whether one more
    /// was asked for meanwhile (1: what's due, 2: every account).
    capturing: Mutex<Option<String>>,
    /// Each account's own HTTP client (`client_for`), and the latest few
    /// sign-ins' own whose requests pass through (`own_client`).
    clients: Mutex<HashMap<String, Arc<olo_net::PerNetwork<reqwest::Client>>>>,
    own_clients: Mutex<std::collections::VecDeque<(String, Arc<olo_net::PerNetwork<reqwest::Client>>)>>,
    /// Each account's own HTTP/1.1 connections to Claude, for requests
    /// passed on exactly as they came (`relay`).
    exact: Arc<crate::relay::Exact>,
    /// Whether a capture runs, and what was asked for meanwhile (0:
    /// nothing, 1: what's due, 2: every account): one lock, so an ask
    /// never falls between a run's last look and its end.
    capture_state: std::sync::Mutex<(bool, u8)>,
    /// This pool is a Rex host's (`hosted`): it serves through its proxy
    /// alone and is given its accounts (claiming resets for the accounts it keeps).
    hosted: std::sync::atomic::AtomicBool,
    /// Who brings a newer sign-in for an account kept elsewhere
    /// (`hosted::Lender`), and when each was last asked and failed (with why).
    lender: Mutex<Option<hosted::Lender>>,
    asked: Mutex<HashMap<String, (std::time::Instant, String)>>,
    /// Who asks an account's keeper to claim a reset credit (`hosted::Claimer`),
    /// negative-probe throttle per `(keeper, provider, family)`, and unreachable
    /// keeper negative cache per `keeper`.
    claimer: Mutex<Option<hosted::Claimer>>,
    remote_claim_tried: Mutex<HashMap<(String, Provider, String), i64>>,
    keeper_unreachable: Mutex<HashMap<String, i64>>,
    /// Monotonic generation per account bumped on capture install and sign-in
    /// replacement/removal so stale async reads cannot overwrite newer state.
    sync_gen: Mutex<HashMap<String, u64>>,
    /// Per-account `(next_issued, last_committed)` read sequence numbers so an
    /// older concurrent quota/credit read cannot overwrite a newer completed one.
    sync_seq: Mutex<HashMap<String, (u64, u64)>>,
    /// Throttle failed `/api/oauth/profile` lookups during background sync.
    profile_tried: Mutex<HashMap<String, i64>>,
    /// When each account's sign-in was last renewed here.
    lent: Mutex<HashMap<String, std::time::Instant>>,
    /// The pools on Rex hosts, as the gateway's `/pool/hosts` shows and
    /// changes them (`hosts::Hosts`, the desktop's), and who is told
    /// when the accounts change.
    hosting: Mutex<Option<Arc<dyn Hosting>>>,
    accounts_changed: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Counts the changes to how the hosts' pools stand: what
    /// `/pool/hosts?after=N` waits on, so that whoever shows them (Rex's
    /// Host window) hears of a change without asking over and over.
    hosts_seen: tokio::sync::watch::Sender<u64>,
    /// Actual TCP port the gateway bound to (useful when launched with port 0).
    bound_port: std::sync::atomic::AtomicU16,
    /// Asynchronous TLS proxy readiness (separate from gateway bind so CA
    /// initialization never blocks the UI looper, while `olo-pool-host` can
    /// await it before configuring clients).
    proxy_ready: (std::sync::Mutex<ProxyReady>, std::sync::Condvar),
}

#[derive(Debug, Clone, Default)]
pub struct ProxyReady {
    pub finished: bool,
    pub proxy_ok: bool,
    pub agy_ok: bool,
    pub error: Option<String>,
    pub agy_error: Option<String>,
}

/// How often an account's quota is read while nothing asks for it (its
/// answers' headers keep it current meanwhile), and its credits.
const SYNC_EVERY: i64 = 5 * 60;
const CREDITS_EVERY: i64 = 30 * 60;
/// The longest wait after the provider asks the pool to slow down.
const SYNC_BACKOFF_MAX: i64 = 30 * 60;

impl Shared {
    /// The TCP port this gateway actually bound to (or 0 if not listening).
    pub fn bound_port(&self) -> u16 {
        self.bound_port.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wait until asynchronous CA + TLS proxy initialization has settled (or
    /// `timeout` elapses), returning which providers' proxy paths are ready.
    pub fn wait_proxy_ready(&self, timeout: Duration) -> ProxyReady {
        let (lock, cv) = &self.proxy_ready;
        let deadline = std::time::Instant::now() + timeout;
        let mut st = lock.lock().unwrap();
        while !st.finished {
            let rem = deadline.saturating_duration_since(std::time::Instant::now());
            if rem.is_zero() {
                break;
            }
            let (next, res) = cv.wait_timeout(st, rem).unwrap();
            st = next;
            if res.timed_out() {
                break;
            }
        }
        st.clone()
    }

    fn set_proxy_ready(&self, ready: ProxyReady) {
        let (lock, cv) = &self.proxy_ready;
        *lock.lock().unwrap() = ready;
        cv.notify_all();
    }

    /// `store`: where changes are saved (None: kept in memory, for tests);
    /// `changed` is called (from any thread) when there is something new
    /// to show.
    pub fn new(saved: Saved, store: Option<Store>, up: Upstreams, changed: impl Fn() + Send + Sync + 'static) -> Arc<Shared> {
        Shared::make(saved, store, up, false, changed)
    }

    /// The pool as the app runs it: its accounts read from the sealed
    /// store on a worker thread (again every 30 s while the keyring
    /// isn't open), the gateway listening on `port`.
    pub fn launch(port: u16, changed: impl Fn() + Send + Sync + 'static) -> Arc<Shared> {
        // (Its meters written for Deskbar's widgets as they change.)
        let (changed, meters) = crate::meters::and_written(changed);
        let shared = Shared::make(Saved::default(), None, Upstreams::from_env(), true, changed);
        crate::meters::start(&shared, meters);
        shared.pool.lock().unwrap().loaded = false;
        // The clients turned off, before the proxy takes a request.
        for p in Provider::ALL {
            shared.set_paused(p, crate::clients::paused(p));
        }
        // The day's activity, kept a minute at a time.
        let weak = Arc::downgrade(&shared);
        let _ = std::thread::Builder::new().name("pool-activity".into()).spawn(move || {
            let loaded = Activity::load();
            match weak.upgrade() {
                Some(s) => {
                    let mut p = s.pool.lock().unwrap();
                    // (Any counted meanwhile go in with it.)
                    let since = std::mem::replace(&mut p.activity, loaded);
                    for m in since.minutes {
                        let at = p.activity.minutes.partition_point(|k| k.minute <= m.minute);
                        p.activity.minutes.insert(at, m);
                    }
                }
                None => return,
            }
            loop {
                std::thread::sleep(Duration::from_secs(60));
                let Some(s) = weak.upgrade() else { break };
                // (Handed over: the new copy keeps the day's activity.)
                if s.drain.retired() {
                    continue;
                }
                let mut activity = {
                    let mut p = s.pool.lock().unwrap();
                    p.activity.prune(now());
                    if !p.activity.dirty {
                        continue;
                    }
                    p.activity.dirty = false;
                    p.activity.clone()
                };
                activity.dirty = true;
                if let Err(e) = activity.save() {
                    tracing::warn!("pool: couldn’t keep its activity: {e}");
                }
            }
        });
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("pool-open".into()).spawn(move || {
            loop {
                if !crate::store::path().exists() {
                    s.pool.lock().unwrap().loaded = true;
                    break;
                }
                match Store::open() {
                    Ok((store, saved)) => {
                        *s.store.lock().unwrap() = Some(store);
                        let mut p = s.pool.lock().unwrap();
                        p.saved = saved;
                        p.loaded = true;
                        if p.error.as_deref().is_some_and(|e| !e.starts_with("Can’t listen")) {
                            p.error = None;
                        }
                        break;
                    }
                    Err(e) => {
                        s.pool.lock().unwrap().error = Some(e);
                        (s.changed)();
                    }
                }
                std::thread::sleep(Duration::from_secs(30));
            }
            (s.changed)();
            s.sync_soon(None);
        });
        if let Err(e) = start(shared.clone(), port) {
            tracing::warn!("pool: {e}");
        }
        shared
    }

    fn make(saved: Saved, store: Option<Store>, up: Upstreams, lazy: bool, changed: impl Fn() + Send + Sync + 'static) -> Arc<Shared> {
        let (tx, rx) = std::sync::mpsc::channel::<Save>();
        let http = olo_net::PerNetwork::new(new_client);
        let capture_disk = lazy || store.is_some();
        let shared = Arc::new(Shared {
            pool: Mutex::new(Pool { saved, loaded: true, ..Default::default() }),
            store: Mutex::new(store),
            http,
            up,
            locks: Mutex::new(HashMap::new()),
            profile_locks: Mutex::new(HashMap::new()),
            capture_disk,
            save: tx,
            changed: Box::new(changed),
            runtime: Mutex::new(None),
            lazy,
            due: Mutex::new(HashMap::new()),
            credits_due: Mutex::new(HashMap::new()),
            credits_fetched: Mutex::new(HashMap::new()),
            drain: Default::default(),
            paused: Default::default(),
            opener: Mutex::new(None),
            captures: Mutex::new(HashMap::new()),
            captures_gen: Mutex::new(HashMap::new()),
            jars: Mutex::new(HashMap::new()),
            capturing: Mutex::new(None),
            clients: Mutex::new(HashMap::new()),
            own_clients: Mutex::new(std::collections::VecDeque::new()),
            exact: Default::default(),
            capture_state: std::sync::Mutex::new((false, 0)),
            hosted: Default::default(),
            lender: Mutex::new(None),
            asked: Mutex::new(HashMap::new()),
            claimer: Mutex::new(None),
            remote_claim_tried: Mutex::new(HashMap::new()),
            keeper_unreachable: Mutex::new(HashMap::new()),
            sync_gen: Mutex::new(HashMap::new()),
            sync_seq: Mutex::new(HashMap::new()),
            profile_tried: Mutex::new(HashMap::new()),
            lent: Mutex::new(HashMap::new()),
            hosting: Mutex::new(None),
            accounts_changed: Mutex::new(None),
            hosts_seen: tokio::sync::watch::channel(0).0,
            bound_port: std::sync::atomic::AtomicU16::new(0),
            proxy_ready: (std::sync::Mutex::new(ProxyReady::default()), std::sync::Condvar::new()),
        });
        // What the copy it took over from finishes after the handover,
        // booked here, where it's saved.
        shared.follow_predecessor();
        // Saving off every async and UI thread, a batch at a time: what
        // changed of the accounts a moment after, a request's bookkeeping
        // with it or `USAGE_EVERY` on at the latest, all of it at a flush.
        let weak = Arc::downgrade(&shared);
        let _ = std::thread::Builder::new().name("pool-save".into()).spawn(move || {
            let mut later: Option<std::time::Instant> = None;
            loop {
                let got = match later {
                    Some(t) => match rx.recv_timeout(t.saturating_duration_since(std::time::Instant::now())) {
                        Ok(m) => Some(m),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    },
                    None => match rx.recv() {
                        Ok(m) => Some(m),
                        Err(_) => break,
                    },
                };
                let mut flushed = Vec::new();
                match got {
                    Some(Save::Later) => {
                        later.get_or_insert_with(|| std::time::Instant::now() + USAGE_EVERY);
                        continue;
                    }
                    Some(Save::Now) => std::thread::sleep(Duration::from_millis(300)),
                    Some(Save::Flush(done)) => flushed.push(done),
                    None => {}
                }
                while let Ok(m) = rx.try_recv() {
                    if let Save::Flush(done) = m {
                        flushed.push(done);
                    }
                }
                later = None;
                let Some(s) = weak.upgrade() else { break };
                save_once(&s);
                drop(s);
                for done in flushed {
                    let _ = done.send(());
                }
            }
        });
        shared
    }

    /// Something of the accounts changed: save it, and show it.
    pub fn touch(&self) {
        let _ = self.save.send(Save::Now);
        (self.changed)();
        if let Some(told) = self.accounts_changed.lock().unwrap().as_ref() {
            told();
        }
    }

    /// A request's bookkeeping changed (when an account was used, what it
    /// used): shown now, saved with the next save or within
    /// `USAGE_EVERY`.
    pub fn noted(&self) {
        let _ = self.save.send(Save::Later);
        (self.changed)();
    }

    /// Write what's waiting now, and return once it's written (a few
    /// seconds at most): before another copy reads the file (a handover).
    pub fn flush(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.save.send(Save::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(3));
        }
    }

    /// Tell the app to look again (nothing to save).
    pub fn nudge(&self) {
        (self.changed)();
    }

    /// The HTTP client requests go out with (the network's there is now).
    /// The accounts whose clients are due a capture: each enabled,
    /// signed-in one whose capture is missing or was made of another
    /// install of its client than the one there now (`all`: every one).
    fn capture_due(&self, all: bool) -> Vec<(String, Provider)> {
        let installed: Vec<(Provider, String)> = Provider::ALL.into_iter().filter_map(|p| crate::capture::installed(p).map(|i| (p, i.stamp))).collect();
        // (The accounts at a glance under the lock; their captures read
        // after, with nothing held.)
        let accounts: Vec<(String, Provider)> = {
            let p = self.pool.lock().unwrap();
            p.saved.accounts.iter().filter(|a| a.enabled && a.signed_out.is_none() && p.saved.credentials.contains_key(&a.id)).map(|a| (a.id.clone(), a.provider)).collect()
        };
        accounts
            .into_iter()
            .filter(|(id, provider)| {
                let Some((_, stamp)) = installed.iter().find(|(p, _)| p == provider) else { return false };
                all || self.capture_of(id).is_none_or(|c| c.installed != *stamp)
            })
            .collect()
    }

    /// What's being captured now ("Claude Code · NAME"), if anything.
    pub fn capturing(&self) -> Option<String> {
        self.capturing.lock().unwrap().clone()
    }

    /// Capture the clients of the accounts that are due one (`all`: every
    /// account's again: the person asked), one account at a time, on a
    /// thread of its own at idle priority (the clients it runs too); one
    /// asked for while one runs is looked at when it ends. Started when a
    /// client changes, when the pool starts, and by hand (Pool ▸ Capture Client
    /// Requests Again, `olo-pool --capture`).
    pub fn capture(self: &Arc<Self>, all: bool) {
        // Only a pool talking to the providers' own hosts: one pointed at
        // made-up ones (a test's) never runs a client, whose reads would go
        // to the real ones with its made-up sign-ins.
        if self.up.origin.is_some() || self.up.claude != Upstreams::default().claude || self.up.chatgpt != Upstreams::default().chatgpt || self.up.agy != Upstreams::default().agy {
            return;
        }
        {
            let mut st = self.capture_state.lock().unwrap();
            if st.0 {
                st.1 = st.1.max(if all { 2 } else { 1 });
                return;
            }
            st.0 = true;
        }
        let s = self.clone();
        let started = std::thread::Builder::new().name("pool-capture".into()).spawn(move || {
            // SAFETY: lowers this thread's own priority; what it starts
            // inherits it.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, 19);
            }
            let mut all = all;
            loop {
                for (id, provider) in s.capture_due(all) {
                    // The sign-in as the pool keeps it, never renewed for a
                    // capture (Codex's review: a renewal is the pool's own
                    // business); one due within a quarter of an hour waits
                    // for the next capture.
                    let made = s.captures_gen.lock().unwrap().get(&id).copied().unwrap_or(0);
                    let (account, cred) = {
                        let p = s.pool.lock().unwrap();
                        let Some(a) = p.saved.accounts.iter().find(|a| a.id == id).cloned() else { continue };
                        let Some(c) = p.saved.credentials.get(&id).cloned() else { continue };
                        (a, c)
                    };
                    if cred.expires_at.is_some_and(|e| e - now() < 900) {
                        tracing::info!("pool: {} for {} not captured now: its sign-in is due for renewal", provider.client(), account.name);
                        continue;
                    }
                    *s.capturing.lock().unwrap() = Some(format!("{} · {}", provider.client(), account.name));
                    (s.changed)();
                    let began = std::time::Instant::now();
                    match crate::capture::capture(&account, &cred, &|_| {}) {
                        Ok(c) => {
                            // (Kept only if the account is still the one
                            // captured: it may have left the pool, or signed
                            // in again, meanwhile.)
                            let now_made = s.captures_gen.lock().unwrap();
                            if now_made.get(&id).copied().unwrap_or(0) != made {
                                tracing::info!("pool: a capture of {} for {} not kept: the account changed meanwhile", provider.client(), account.name);
                                continue;
                            }
                            match crate::capture::save(&c) {
                                Ok(()) => {
                                    tracing::info!("pool: captured {} {} for {} ({} requests, {:.0?})", provider.client(), c.version, account.name, c.requests.len(), began.elapsed());
                                    match provider {
                                        Provider::Claude => crate::upstream::refresh_claude_version(),
                                        Provider::Antigravity => crate::upstream::refresh_agy_version(),
                                        Provider::Codex => {}
                                    }
                                    s.keep_capture(&id, c);
                                    // Re-read quota and reset credits right away with the new capture
                                    // instead of waiting up to CREDITS_EVERY (30 min).
                                    s.credits_due.lock().unwrap().remove(&id);
                                    s.credits_fetched.lock().unwrap().remove(&id);
                                    s.sync_soon(Some(id.clone()));
                                }
                                Err(e) => tracing::warn!("pool: keeping the capture of {} for {}: {e}", provider.client(), account.name),
                            }
                            drop(now_made);
                        }
                        Err(e) => tracing::warn!("pool: capturing {} for {}: {e}", provider.client(), account.name),
                    }
                }
                *s.capturing.lock().unwrap() = None;
                (s.changed)();
                let mut st = s.capture_state.lock().unwrap();
                match std::mem::take(&mut st.1) {
                    0 => {
                        st.0 = false;
                        break;
                    }
                    n => all = n == 2,
                }
            }
        });
        if started.is_err() {
            self.capture_state.lock().unwrap().0 = false;
        }
    }

    /// An account's capture and its own values (`likeness::Like`): what
    /// the pool's own requests for it are made from, when its client was
    /// captured (`capture`).
    pub fn like(&self, id: &str) -> Option<crate::likeness::Like> {
        let (org, email) = {
            let p = self.pool.lock().unwrap();
            let a = p.saved.accounts.iter().find(|a| a.id == id)?;
            (a.org.clone(), a.email.clone())
        };
        let capture = self.capture_of(id)?;
        Some(crate::likeness::Like { capture, org, email, jar: self.jar(id) })
    }

    /// Account `id`'s capture `c`, kept for its requests from now (the
    /// capture's own, once saved; a test's, never saved).
    pub fn keep_capture(&self, id: &str, c: crate::capture::Capture) {
        self.captures.lock().unwrap().insert(id.to_owned(), Some(Arc::new(c)));
        self.bump_sync_gen(id);
    }

    fn sync_gen(&self, id: &str) -> u64 {
        self.sync_gen.lock().unwrap().get(id).copied().unwrap_or(0)
    }

    fn bump_sync_gen(&self, id: &str) {
        *self.sync_gen.lock().unwrap().entry(id.to_owned()).or_default() += 1;
    }

    fn begin_sync_seq(&self, id: &str) -> u64 {
        let mut map = self.sync_seq.lock().unwrap();
        let entry = map.entry(id.to_owned()).or_insert((0, 0));
        entry.0 += 1;
        entry.0
    }

    fn commit_sync_seq(&self, id: &str, seq: u64) -> bool {
        let mut map = self.sync_seq.lock().unwrap();
        let entry = map.entry(id.to_owned()).or_insert((0, 0));
        if seq <= entry.1 {
            return false;
        }
        entry.1 = seq;
        true
    }

    /// Account `id`'s capture: read from its file the first time it's
    /// asked for, kept after (the pool keeps it current as it makes one).
    fn capture_of(&self, id: &str) -> Option<Arc<crate::capture::Capture>> {
        if let Some(c) = self.captures.lock().unwrap().get(id) {
            return c.clone();
        }
        if !self.capture_disk {
            return None;
        }
        let c = crate::capture::load(id).map(Arc::new);
        self.captures.lock().unwrap().entry(id.to_owned()).or_insert(c).clone()
    }

    /// What was kept of account `id`'s client goes (it left the pool, or
    /// signed in again, perhaps as another account): its capture (one
    /// under way isn't kept either), cookies and connections.
    fn forget_kept(&self, id: &str) {
        self.bump_sync_gen(id);
        {
            let mut made = self.captures_gen.lock().unwrap();
            *made.entry(id.to_owned()).or_default() += 1;
            self.captures.lock().unwrap().insert(id.to_owned(), None);
            if self.capture_disk {
                // (One small file.)
                match std::fs::remove_file(crate::capture::path(id)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => tracing::warn!("pool: removing a capture: {e}"),
                }
            }
        }
        self.jars.lock().unwrap().remove(id);
        self.clients.lock().unwrap().remove(id);
        self.exact.forget(id);
    }

    /// The HTTP client for requests passed through on sign-in `key`'s own
    /// (`intercept::own_key`): one of its own, the latest eight kept (a
    /// sign-in renewed is another).
    pub fn own_client(&self, key: &str) -> reqwest::Client {
        let mut own = self.own_clients.lock().unwrap();
        if let Some(at) = own.iter().position(|(k, _)| k == key) {
            let kept = own.remove(at).expect("found");
            own.push_back(kept);
        } else {
            own.push_back((key.to_owned(), Arc::new(olo_net::PerNetwork::new(new_client))));
            if own.len() > 8 {
                own.pop_front();
            }
        }
        own.back().expect("just put").1.get()
    }

    /// Account `id`'s cookies (`likeness::Jar`).
    pub fn jar(&self, id: &str) -> crate::likeness::Jar {
        self.jars.lock().unwrap().entry(id.to_owned()).or_default().clone()
    }

    /// The HTTP client for account `id`'s requests: one of its own, so no
    /// connection ever carries two accounts' requests (a client has one
    /// account), for the network there is now.
    pub fn client_for(&self, id: &str) -> reqwest::Client {
        let made = self.clients.lock().unwrap().entry(id.to_owned()).or_insert_with(|| Arc::new(olo_net::PerNetwork::new(new_client))).clone();
        made.get()
    }

    /// The connections requests are passed on exactly on (`relay`).
    pub fn exact(&self) -> &Arc<crate::relay::Exact> {
        &self.exact
    }

    /// What a request for account `id` carries of its own: its
    /// organization, and (Claude) the body with the account's id where the
    /// client named its own: in `metadata.user_id`'s string (a JSON text
    /// holding `account_uuid`), there alone, as the client wrote it; the
    /// rest of the body byte for byte (Codex's review: one replaced in what
    /// a person wrote changed their words).
    fn own(&self, id: &str, provider: Provider, headers: &HeaderMap, body: &Bytes) -> Own {
        let org = self.pool.lock().unwrap().saved.accounts.iter().find(|a| a.id == id).and_then(|a| a.org.clone());
        let mut out = Own { org, body: body.clone() };
        if provider != Provider::Claude || headers.get("content-encoding").is_some_and(|c| c.to_str().map(|c| !c.trim().eq_ignore_ascii_case("identity")).unwrap_or(true)) {
            return out;
        }
        let Some(span) = user_id_span(body) else { return out };
        let Some(ours) = self.like(id).and_then(|l| l.capture.account_uuid.clone()) else { return out };
        if let Some(b) = with_account(body, span, &ours) {
            out.body = b;
        }
        out
    }

    pub fn client(&self) -> reqwest::Client {
        self.http.get()
    }

    /// Where a request to `host` the proxy passes through goes.
    pub fn upstream_origin(&self, host: &str) -> String {
        self.up.origin.clone().unwrap_or_else(|| format!("https://{host}"))
    }

    /// Whether `provider`'s client is turned off, its requests passed
    /// straight through though it is pointed at the proxy.
    pub fn paused(&self, provider: Provider) -> bool {
        self.paused[provider_index(provider)].load(std::sync::atomic::Ordering::SeqCst)
    }

    /// What `/pool/open` does: opens the app's window (and takes its
    /// launches again, if no copy does).
    pub fn on_open(&self, open: impl Fn() + Send + Sync + 'static) {
        *self.opener.lock().unwrap() = Some(Box::new(open));
    }

    pub fn set_paused(&self, provider: Provider, paused: bool) {
        self.paused[provider_index(provider)].store(paused, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether `provider`'s requests are the pool's to serve now (else
    /// passed through on the client's own sign-in): it serves them, and
    /// the client isn't turned off.
    pub fn routes(&self, provider: Provider) -> bool {
        !self.paused(provider) && self.serves(provider)
    }

    /// Whether the pool has more to draw from for `provider`'s `family`
    /// than the account `serving` (none given: than the one it would
    /// choose now): another account with allowance, or a reset credit to
    /// claim (`told::more`). While it has, its clients are told nothing
    /// is used.
    pub fn more_to_draw(&self, provider: Provider, family: &str, serving: Option<&str>) -> bool {
        let p = self.pool.lock().unwrap();
        let chosen = match serving {
            Some(id) => Some(id.to_owned()),
            None => {
                let sticky = p.sticky.get(&(provider, family.to_owned())).cloned();
                p.pick(provider, family, sticky.as_deref(), &HashMap::new(), &[]).map(|a| a.id.clone())
            }
        };
        crate::told::more(&p.saved.accounts, provider, family, chosen.as_deref(), now(), |a| p.saved.credentials.contains_key(&a.id))
    }

    /// Whether the pool serves `provider`'s requests now: it has an
    /// account for it that's on and signed in. (If not, the proxy lets
    /// the client through on its own sign-in.)
    pub fn serves(&self, provider: Provider) -> bool {
        let p = self.pool.lock().unwrap();
        p.loaded && p.saved.accounts.iter().any(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && p.saved.credentials.contains_key(&a.id))
    }

    fn lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(id.to_owned()).or_default().clone()
    }

    fn profile_lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.profile_locks.lock().unwrap().entry(id.to_owned()).or_default().clone()
    }

    pub fn snapshot(&self) -> Pool {
        let p = self.pool.lock().unwrap();
        // (Its activity read apart, with `activity`.)
        Pool {
            saved: p.saved.clone(),
            sticky: HashMap::new(),
            in_flight: p.in_flight.clone(),
            served: p.served.clone(),
            open: p.open.clone(),
            next_open: 0,
            error: p.error.clone(),
            loaded: p.loaded,
            activity: Activity::default(),
            resets: p.resets.clone(),
            reset_at: p.reset_at.clone(),
            last_reset: p.last_reset.clone(),
            keeper_reset_id: p.keeper_reset_id.clone(),
        }
    }

    /// Read the pool's activity.
    pub fn activity<T>(&self, f: impl FnOnce(&Activity) -> T) -> T {
        f(&self.pool.lock().unwrap().activity)
    }

    /// Add an account with its sign-in (and read its quota at once).
    pub fn add(self: &Arc<Self>, account: Account, credential: Credential) -> Result<(), String> {
        let id = account.id.clone();
        {
            let mut p = self.pool.lock().unwrap();
            if !p.loaded {
                return Err(p.error.clone().unwrap_or_else(|| "The pool’s accounts aren’t open yet.".into()));
            }
            p.saved.credentials.insert(id.clone(), credential);
            p.saved.accounts.push(account);
        }
        self.touch();
        self.sync_soon(Some(id));
        // (Its client captured for it, in a moment: what's due.)
        self.capture(false);
        Ok(())
    }

    /// Replace an account's sign-in (signed in again).
    pub fn reauth(self: &Arc<Self>, id: &str, credential: Credential) {
        let prev_keeper = {
            let mut p = self.pool.lock().unwrap();
            p.saved.credentials.insert(id.to_owned(), credential);
            p.account_mut(id).and_then(|a| {
                a.signed_out = None;
                a.cooldown_until = None;
                // Invalidate any stale org UUID so `/api/oauth/profile` heals
                // the new sign-in's real organization.
                a.org = None;
                // (Signed in anew here: this pool's to keep, until it
                // is next carried to the hosts, `hosted`.)
                let prev = a.kept_on.take();
                if let Some(ref host) = prev {
                    a.reauth_pending_host = Some(host.clone());
                }
                prev
            })
        };
        if let Some(host) = prev_keeper
            && let Some(hosting) = self.hosting()
        {
            let _ = hosting.act(&host, "sync");
        }
        self.profile_tried.lock().unwrap().remove(id);
        // (Signed in again, perhaps as another account under the same name:
        // what was captured and kept of the old goes, and it's captured
        // anew.)
        self.forget_kept(id);
        self.touch();
        self.sync_soon(Some(id.to_owned()));
        self.capture(false);
    }

    pub fn set_enabled(&self, id: &str, on: bool) {
        if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
            a.enabled = on;
        }
        self.touch();
    }

    pub fn rename(&self, id: &str, name: &str) {
        if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
            a.name = name.to_owned();
        }
        self.touch();
    }

    /// Take an account out of the pool (not while it's serving).
    pub fn remove(&self, id: &str) -> Result<(), String> {
        {
            let mut p = self.pool.lock().unwrap();
            if p.busy(id) {
                return Err("It’s answering a request now; pause it and try again when that’s done.".into());
            }
            p.saved.accounts.retain(|a| a.id != id);
            p.saved.credentials.remove(id);
            p.resets.remove(id);
            p.reset_at.remove(id);
            p.last_reset.remove(id);
            p.keeper_reset_id.retain(|(_, aid), _| aid != id);
        }
        // (What was kept of it goes with it: its capture, its cookies, its
        // connections.)
        self.forget_kept(id);
        self.touch();
        Ok(())
    }

    /// Run `task` on the gateway's runtime (once it is running).
    pub fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(rt) = self.runtime.lock().unwrap().clone() {
            rt.spawn(task);
        }
    }

    /// Read quota (and refresh sign-ins due) for one account or all, soon.
    pub fn sync_soon(self: &Arc<Self>, id: Option<String>) {
        let Some(rt) = self.runtime.lock().unwrap().clone() else { return };
        let s = self.clone();
        rt.spawn(async move { s.sync(id.as_deref()).await });
    }

    /// The sign-in for `id`, refreshed first if it's (nearly) expired or
    /// when the provider refused the credential that is still current.
    async fn fresh(&self, id: &str, provider: Provider, refused: Option<&Credential>, within: i64) -> Result<Credential, Failure> {
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        // Another copy of the pool at work beside this one (an upgrade
        // handing over): renewed under the store's lock, after reading what
        // the other may have renewed already.
        let shared_file = if self.drain.overlapping() { tokio::task::block_in_place(|| self.store_lock_and_adopt(id)).map_err(Failure::Other)? } else { None };
        let renewed_there = shared_file.as_ref().is_some_and(|(_, adopted)| *adopted);
        // Someone else may have refreshed it while we waited.
        let current = self.pool.lock().unwrap().saved.credentials.get(id).cloned().ok_or_else(|| Failure::Refused("No sign-in".into()))?;
        let due = current.expires_at.is_some_and(|e| e - now() < within);
        // Kept elsewhere: never renewed here (`hosted`).
        let kept_on = self.pool.lock().unwrap().account(id).and_then(|a| a.kept_on.clone());
        if let Some(keeper) = kept_on {
            let refused = refused.is_some_and(|rejected| current.access == rejected.access);
            return self.borrowed(id, &keeper, current, due || refused, refused).await;
        }
        // (Refused, but the other copy renewed it since: that one, then.)
        if (renewed_there || refused.is_none_or(|rejected| current.access != rejected.access)) && !due {
            return Ok(current);
        }
        match upstream::refresh(&self.client_for(id), &self.up, provider, &current, self.like(id).as_ref()).await {
            Ok(c) => {
                self.pool.lock().unwrap().saved.credentials.insert(id.to_owned(), c.clone());
                // (When: a borrower asking for a longer-lived one soon
                // after gets this one, `hosted::lend`.)
                self.lent.lock().unwrap().insert(id.to_owned(), std::time::Instant::now());
                if self.drain.retired() {
                    // (Handed over: only this sign-in goes into the file.)
                    let written = tokio::task::block_in_place(|| self.store.lock().unwrap().as_ref().map(|st| st.put_credential(id, &c)));
                    if let Some(Err(e)) = written {
                        tracing::warn!("pool: {e}");
                    }
                } else if shared_file.is_some() {
                    // Written before the store's lock goes, so the other
                    // copy reads it.
                    self.save_now().await;
                } else {
                    self.touch();
                }
                drop(shared_file);
                Ok(c)
            }
            Err(Failure::Refused(why)) => {
                self.signed_out(id, &why);
                Err(Failure::Refused(why))
            }
            // Not refused: the old one may still do.
            Err(e) if refused.is_none() && current.expires_at.is_none_or(|t| t > now()) => {
                tracing::warn!("pool: {e}");
                Ok(current)
            }
            Err(e) => Err(e),
        }
    }

    /// Account `id`'s sign-in, renewed first if it's due within ten minutes
    /// (as a quota read does).
    pub async fn renewed(&self, id: &str, provider: Provider) -> Result<Credential, Failure> {
        self.fresh(id, provider, None, 600).await
    }

    /// The store's lock (`store::lock_at`), waited for with the store itself
    /// let go: its holder takes the store next, so the two are only ever
    /// taken in that order. Blocks.
    fn store_lock(&self) -> Result<Option<std::fs::File>, String> {
        let at = self.store.lock().unwrap().as_ref().map(Store::lock_path).or_else(|| self.lazy.then(|| crate::store::path().with_extension("lock")));
        let Some(at) = at else { return Ok(None) }; // An in-memory fixture.
        if let Some(dir) = at.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::store::lock_at(&at).map(Some).ok_or_else(|| "Couldn’t acquire the shared pool store lock.".to_owned())
    }

    /// The store's lock, and whether the file had a newer sign-in for `id`
    /// than this copy's, taken in (the other copy renewed it). Blocks.
    fn store_lock_and_adopt(&self, id: &str) -> Result<Option<(std::fs::File, bool)>, String> {
        let Some(held) = self.store_lock()? else { return Ok(None) };
        let disk = self.store.lock().unwrap().as_ref().map(Store::read);
        let adopted = match disk {
            Some(Ok(disk)) => {
                let mut p = self.pool.lock().unwrap();
                match (disk.credentials.get(id), p.saved.credentials.get(id)) {
                    (Some(theirs), Some(ours)) if newer(theirs, ours) => {
                        p.saved.credentials.insert(id.to_owned(), theirs.clone());
                        true
                    }
                    _ => false,
                }
            }
            Some(Err(e)) => return Err(e),
            None => false,
        };
        Ok(Some((held, adopted)))
    }

    /// The sign-ins in `disk` newer than `snapshot`'s (another copy renewed
    /// them) taken into it and into this copy's own.
    fn adopt_newer(&self, disk: &Saved, snapshot: &mut Saved) {
        let mut p = self.pool.lock().unwrap();
        for (id, theirs) in &disk.credentials {
            if snapshot.credentials.get(id).is_some_and(|ours| newer(theirs, ours)) {
                snapshot.credentials.insert(id.clone(), theirs.clone());
                p.saved.credentials.insert(id.clone(), theirs.clone());
            }
        }
    }

    fn signed_out(&self, id: &str, why: &str) {
        if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
            a.signed_out = Some(why.to_owned());
        }
        self.touch();
    }

    /// Write the pool now (for what must be kept before a provider is
    /// asked), with the store's lock already held (`fresh`).
    async fn save_now(&self) {
        // A small write, on this worker thread, before the provider is asked.
        if let Err(e) = tokio::task::block_in_place(|| self.write(false)) {
            tracing::warn!("pool: {e}");
        }
    }

    /// Write the pool as it is now: the store first (made now, the first
    /// time there's something to keep), the snapshot after, so that of two
    /// writes the later always has what the earlier had (Codex's review: a
    /// save's older snapshot written after a claim's own left the claim's
    /// intent out of the file). With `file_lock`, while another copy is at
    /// work, under the store's lock, keeping the sign-ins it renewed.
    /// Blocks.
    fn write(&self, file_lock: bool) -> Result<(), String> {
        let held = if file_lock && self.drain.overlapping() { self.store_lock()? } else { None };
        let mut store = self.store.lock().unwrap();
        if self.drain.retired() {
            return Err("This copy of the pool has handed over.".into());
        }
        let mut snapshot = {
            let p = self.pool.lock().unwrap();
            if !p.loaded {
                return Err("The pool’s accounts aren’t open.".into());
            }
            p.saved.clone()
        };
        if held.is_some()
            && let Some(st) = store.as_ref()
        {
            self.adopt_newer(&st.read()?, &mut snapshot);
        }
        if store.is_none() && self.lazy {
            *store = Some(Store::open()?.0);
        }
        match store.as_ref() {
            Some(st) => st.save(&snapshot),
            // (None to keep it in: a test's.)
            None => Ok(()),
        }
    }

    /// Spend one of account `id`'s reset credits (the soonest to expire),
    /// then read its quota again.
    pub async fn redeem(&self, id: &str) -> Result<(), String> {
        self.redeem_credit(id, None, Why::Asked).await.map_err(|e| e.to_string())
    }

    /// The lock a provider's claims are made under, one at a time: one
    /// claimed while a request waited may serve it too (Codex's review: two
    /// requests turned away together each claimed one).
    fn claims_gate(&self, provider: Provider) -> Arc<tokio::sync::Mutex<()>> {
        self.lock_for(&format!("\u{0}claims {}", provider_index(provider)))
    }

    /// Every provider's claims held, once none is being made (waited for
    /// `within` at most; not from the runtime's threads): what a handover
    /// holds while the new copy starts and this one retires, so no claim
    /// is under way across it (Codex's third review: one made here as the
    /// copy started could be made again there, its intent kept but not
    /// how it went). One waiting for the gate meanwhile then finds this
    /// copy retired, and isn't made. None if one is still under way.
    pub fn claims_closed(&self, within: Duration) -> Option<Vec<tokio::sync::OwnedMutexGuard<()>>> {
        let gates: Vec<_> = Provider::ALL.into_iter().map(|p| self.claims_gate(p)).collect();
        if let Some(held) = gates.iter().map(|g| g.clone().try_lock_owned().ok()).collect::<Option<Vec<_>>>() {
            return Some(held);
        }
        // (Waited for on a thread of its own, so the wait has its limit:
        // one that comes later is let go there.)
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("pool-claims".into())
            .spawn(move || {
                let held: Vec<_> = gates.into_iter().map(|g| g.blocking_lock_owned()).collect();
                let _ = tx.send(held);
            })
            .ok()?;
        rx.recv_timeout(within).ok()
    }

    /// Claim the credit `credit` of account `id` (None: the best manually
    /// claimable credit via [`Account::manual_credit`]) for `why`. Only the
    /// pool that keeps `id` (`kept_on.is_none()`) claims directly (unless a
    /// remote keeper is a legacy v1 host without `Ask::Redeem`); a borrower
    /// delegates to the keeper (`Claimer`) under `claims_gate(provider)` with
    /// a durable `op_id` so two pools never double-claim even across lost
    /// replies or handovers.
    pub async fn redeem_credit(&self, id: &str, credit: Option<&str>, why: Why) -> Result<(), Claimed> {
        if self.drain.retired() {
            return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
        }
        let (provider, kept_on, reauth_pending) =
            self.pool.lock().unwrap().account(id).map(|a| (a.provider, a.kept_on.clone(), a.reauth_pending_host.clone())).ok_or_else(|| Claimed::NotNow("No such account.".into()))?;
        let gate = self.claims_gate(provider);
        let _gate = gate.lock().await;
        if self.drain.retired() {
            return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
        }
        // Reconcile any pending remote claim for this provider before spending
        // anything new (prevents interrupted manual retries from double-spending).
        if let Some(reconciled) = self.reconcile_pending_remote_locked(provider, "", &HashMap::new()).await?
            && reconciled == id
            && why == Why::Asked
        {
            return Ok(());
        }
        if let Some(keeper) = kept_on {
            // Expiring claims belong strictly to the keeper; if ownership
            // changed after `spend_expiring` picked `id`, skip rather than
            // converting it into a manual claim on the new keeper.
            if why == Why::Expiring {
                return Err(Claimed::NotNow(format!("This account is now kept on {keeper}.")));
            }
            let claimer = self.claimer().ok_or_else(|| Claimed::NotNow(format!("This account’s reset credits are claimed by {keeper}, which keeps its sign-in.")))?;
            let op_id = crate::store::new_uuid();
            let observed = self.observed_limit_for_account(id, &keeper);
            let t = now();
            // Persist the pending remote op BEFORE dispatching to the keeper so
            // cancellation or crash mid-RPC reconciles the exact op_id on retry.
            self.persist_pending_remote_claim(PendingRemoteClaim {
                provider,
                keeper: keeper.clone(),
                op_id: op_id.clone(),
                account: Some(id.to_owned()),
                credit: credit.map(str::to_owned),
                family: String::new(),
                at: t,
                tried_at: t,
            })
            .await?;
            let ask = ClaimAsk::Account { id: id.to_owned(), credit: credit.map(str::to_owned), op_id: op_id.clone(), timeout_secs: Some(90), reconcile_only: false, observed };
            match claimer(keeper.clone(), ask).await {
                Ok(claim) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    let _ = self.apply_remote_claim(&keeper, claim, "", &HashMap::new());
                    return Ok(());
                }
                Err(Claimed::Unsupported(_)) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    // Legacy v1 host doesn't support Ask::Redeem: fall back to
                    // direct claim using borrowed token until the host upgrades.
                    return self.redeem_locked(id, credit, why, true, None, None, false).await.map(|_| ());
                }
                Err(e @ Claimed::Unclear(_)) => {
                    // Leave PendingRemoteClaim persisted so the next attempt reconciles op_id.
                    return Err(e);
                }
                Err(e) => {
                    // Definitive Refused / NotNow / NoRecord / Unreachable (never delivered):
                    // clear the pre-persisted pending op.
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    return Err(e);
                }
            }
        }
        if let Some(host) = reauth_pending {
            return Err(Claimed::NotNow(format!("Waiting for {host} to accept the updated sign-in before claiming a reset.")));
        }
        self.redeem_locked(id, credit, why, false, None, None, false).await.map(|_| ())
    }

    /// Valid Claude organization UUID check shared by sync and redemption.
    fn valid_claude_org(org: Option<&str>) -> bool {
        org.filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).is_some()
    }

    /// Ensure an Antigravity account's `cloudaicompanionProject` is cached in
    /// `Credential::account_id` (and update `Account::plan` from `loadCodeAssist`).
    /// Coalesces concurrent discovery requests per account under a dedicated
    /// async lock so bursts never stampede `loadCodeAssist`.
    /// `cooldown_secs` throttles retries after failures or empty projects (`0` forces).
    /// Returns `true` ONLY when `loadCodeAssist` returned HTTP 401 Unauthorized
    /// (never on 400/403) so callers on the request path refresh and retry once.
    async fn ensure_agy_project(&self, id: &str, cred: &mut Credential, like: Option<&crate::likeness::Like>, cooldown_secs: i64) -> bool {
        if cred.account_id.as_deref().map(str::trim).is_some_and(|s| !s.is_empty()) {
            return false;
        }
        let plock = self.profile_lock_for(id);
        let _held = plock.lock().await;
        // Re-check cached project after acquiring lock in case a concurrent
        // request already discovered it for this same credential.
        {
            let p = self.pool.lock().unwrap();
            if let Some(saved) = p.saved.credentials.get(id)
                && saved.access == cred.access
                && saved.refresh == cred.refresh
                && let Some(proj) = saved.account_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
            {
                cred.account_id = Some(proj.to_owned());
                return false;
            }
        }
        if cooldown_secs > 0 && self.profile_tried.lock().unwrap().get(id).is_some_and(|&at| now() - at < cooldown_secs) {
            return false;
        }
        let sgen = self.sync_gen(id);
        let profiled_access = cred.access.clone();
        let profiled_refresh = cred.refresh.clone();
        match upstream::agy_profile(&self.client_for(id), &self.up, cred, like).await {
            Ok((project, plan)) => {
                if self.sync_gen(id) != sgen {
                    return false;
                }
                if project.is_none() {
                    self.profile_tried.lock().unwrap().insert(id.to_owned(), now());
                } else {
                    self.profile_tried.lock().unwrap().remove(id);
                }
                let mut touched = false;
                {
                    let mut p = self.pool.lock().unwrap();
                    let still_same_cred = p.saved.credentials.get(id).is_some_and(|c| c.access == profiled_access && c.refresh == profiled_refresh);
                    if still_same_cred {
                        if let Some(proj) = project {
                            cred.account_id = Some(proj.clone());
                            if let Some(c) = p.saved.credentials.get_mut(id) {
                                c.account_id = Some(proj);
                                touched = true;
                            }
                        }
                        if let Some(pl) = plan
                            && let Some(a) = p.account_mut(id)
                            && a.plan.as_deref() != Some(pl.as_str())
                        {
                            a.plan = Some(pl);
                            touched = true;
                        }
                    }
                }
                if touched {
                    self.touch();
                }
                false
            }
            Err(ref e) if e.is_unauthorized() => {
                self.profile_tried.lock().unwrap().insert(id.to_owned(), now());
                true
            }
            Err(_) => {
                self.profile_tried.lock().unwrap().insert(id.to_owned(), now());
                false
            }
        }
    }

    fn is_critical_remote_record(r: &RemoteClaimRecord) -> bool {
        matches!(r.result, Some(RemoteClaim::Redeemed { .. })) || r.error_kind.as_deref() == Some("unclear")
    }

    fn prune_remote_claims(records: &mut Vec<RemoteClaimRecord>, t: i64) {
        let mut critical: Vec<RemoteClaimRecord> = Vec::new();
        let mut ephemeral: Vec<RemoteClaimRecord> = Vec::new();
        for r in records.drain(..) {
            if Self::is_critical_remote_record(&r) {
                if t - r.at <= 7 * 86_400 {
                    critical.push(r);
                }
            } else if t - r.at <= 86_400 {
                ephemeral.push(r);
            }
        }
        if critical.len() > 128 {
            let excess = critical.len() - 128;
            critical.drain(..excess);
        }
        if ephemeral.len() > 32 {
            let excess = ephemeral.len() - 32;
            ephemeral.drain(..excess);
        }
        records.extend(critical);
        records.extend(ephemeral);
    }

    fn record_remote_claim_pending_locked(p: &mut Pool, op_id: &str, account: &str) {
        let t = now();
        p.saved.remote_claims.retain(|r| r.op_id != op_id);
        p.saved.remote_claims.push(RemoteClaimRecord {
            op_id: op_id.to_owned(),
            at: t,
            account: Some(account.to_owned()),
            result: None,
            error_kind: Some("unclear".into()),
            error: Some("Reset claim is still in flight.".into()),
        });
        Self::prune_remote_claims(&mut p.saved.remote_claims, t);
    }

    fn make_remote_record(op_id: &str, account: Option<String>, outcome: Result<&RemoteClaim, &Claimed>, t: i64) -> RemoteClaimRecord {
        match outcome {
            Ok(claim) => RemoteClaimRecord { op_id: op_id.to_owned(), at: t, account: Some(claim.account().to_owned()), result: Some(claim.clone()), error_kind: None, error: None },
            Err(e) => {
                let kind = match e {
                    Claimed::Refused(_) => "refused",
                    Claimed::NotNow(_) | Claimed::Unsupported(_) => "not_now",
                    Claimed::NoRecord(_) => "no_record",
                    Claimed::Unreachable(_) => "unreachable",
                    Claimed::Unclear(_) => "unclear",
                };
                RemoteClaimRecord { op_id: op_id.to_owned(), at: t, account, result: None, error_kind: Some(kind.into()), error: Some(e.to_string()) }
            }
        }
    }

    fn record_remote_claim_outcome(&self, op_id: &str, outcome: Result<&RemoteClaim, &Claimed>) {
        let t = now();
        let mut p = self.pool.lock().unwrap();
        let prev_account = p.saved.remote_claims.iter().find(|r| r.op_id == op_id).and_then(|r| r.account.clone());
        let rec = Self::make_remote_record(op_id, prev_account, outcome, t);
        p.saved.remote_claims.retain(|r| r.op_id != op_id);
        p.saved.remote_claims.push(rec);
        Self::prune_remote_claims(&mut p.saved.remote_claims, t);
    }

    fn resolve_pending_ops_for_account_locked(p: &mut Pool, account: &str, outcome: Result<&RemoteClaim, &Claimed>) -> bool {
        let t = now();
        let mut resolved = false;
        for rec in &mut p.saved.remote_claims {
            if rec.error_kind.as_deref() == Some("unclear") && rec.account.as_deref() == Some(account) {
                *rec = Self::make_remote_record(&rec.op_id, Some(account.to_owned()), outcome, t);
                resolved = true;
            }
        }
        if resolved {
            Self::prune_remote_claims(&mut p.saved.remote_claims, t);
        }
        resolved
    }

    fn lookup_remote_claim_record(&self, op_id: &str) -> Option<RemoteClaimRecord> {
        let p = self.pool.lock().unwrap();
        p.saved.remote_claims.iter().find(|r| r.op_id == op_id).cloned()
    }

    async fn persist_pending_remote_claim(&self, pending: PendingRemoteClaim) -> Result<(), Claimed> {
        let provider = pending.provider;
        let op_id = pending.op_id.clone();
        {
            let mut p = self.pool.lock().unwrap();
            p.saved.pending_remote_claims.retain(|r| r.provider != provider);
            p.saved.pending_remote_claims.push(pending);
        }
        if let Err(e) = self.save_checked().await {
            self.pool.lock().unwrap().saved.pending_remote_claims.retain(|r| !(r.provider == provider && r.op_id == op_id));
            return Err(Claimed::NotNow(e));
        }
        self.touch();
        Ok(())
    }

    async fn clear_pending_remote_claim(&self, provider: Provider, op_id: &str) {
        let removed = {
            let mut p = self.pool.lock().unwrap();
            let before = p.saved.pending_remote_claims.len();
            p.saved.pending_remote_claims.retain(|r| !(r.provider == provider && r.op_id == op_id));
            p.saved.pending_remote_claims.len() != before
        };
        if removed {
            let _ = self.save_checked().await;
            self.touch();
        }
    }

    /// Reconcile any pending unclear remote claim for `provider` before
    /// starting a new claim or falling back to usage credits.
    /// ONLY clears `PendingRemoteClaim` when the keeper returns `Ok`,
    /// `NoRecord`, `Refused`, or `Unsupported` — NEVER on `Unreachable`,
    /// `NotNow` (service down / gateway starting / copy handed over), or `Unclear`.
    /// Caller MUST hold `self.claims_gate(provider)`.
    async fn reconcile_pending_remote_locked(&self, provider: Provider, family: &str, tried: &HashMap<String, u64>) -> Result<Option<String>, Claimed> {
        let pending = {
            let p = self.pool.lock().unwrap();
            p.saved.pending_remote_claims.iter().find(|r| r.provider == provider).cloned()
        };
        let Some(pending) = pending else { return Ok(None) };
        let t = now();
        if t - pending.tried_at < 10 {
            return Err(Claimed::Unclear("A previous remote reset claim still needs to be reconciled.".into()));
        }
        let Some(claimer) = self.claimer() else {
            return Err(Claimed::Unclear(format!("A previous reset claim on {} still needs to be reconciled.", pending.keeper)));
        };
        {
            let mut p = self.pool.lock().unwrap();
            if let Some(r) = p.saved.pending_remote_claims.iter_mut().find(|r| r.provider == provider && r.op_id == pending.op_id) {
                r.tried_at = t;
            }
        }
        let ask = match &pending.account {
            Some(id) => ClaimAsk::Account { id: id.clone(), credit: pending.credit.clone(), op_id: pending.op_id.clone(), timeout_secs: Some(90), reconcile_only: true, observed: Vec::new() },
            None => ClaimAsk::For { provider, family: pending.family.clone(), op_id: pending.op_id.clone(), timeout_secs: Some(90), reconcile_only: true, observed: Vec::new() },
        };
        match claimer(pending.keeper.clone(), ask).await {
            Ok(claim) => {
                self.clear_pending_remote_claim(provider, &pending.op_id).await;
                Ok(self.apply_remote_claim(&pending.keeper, claim, family, tried))
            }
            Err(Claimed::NoRecord(_) | Claimed::Refused(_) | Claimed::Unsupported(_)) => {
                // Keeper explicitly confirmed this op_id did not spend a credit: safe to clear!
                self.clear_pending_remote_claim(provider, &pending.op_id).await;
                Ok(None)
            }
            Err(e) => {
                // Unreachable / NotNow (service down, gateway starting, copy retired) / Unclear:
                // keep PendingRemoteClaim intact so we NEVER spend a second credit!
                Err(Claimed::Unclear(e.to_string()))
            }
        }
    }

    /// Drop synthetic Antigravity fallback/429 windows once authoritative real
    /// buckets arrive, and reconcile quota refusals (`cooldown_until`,
    /// `model_cooldowns`) covered by healthy real buckets while keeping
    /// transient capacity backoffs (`capacity_until`, `capacity_cooldowns`) intact.
    fn reconcile_agy_windows(a: &mut Account, real_windows: &[Window], t: i64) {
        if a.provider != Provider::Antigravity || real_windows.is_empty() {
            return;
        }
        a.windows.retain(|w| !w.name.starts_with("model:") && w.name != "quota" && !w.name.starts_with("models:") && w.name != "available_models");
        if a.cooldown_until.is_some_and(|c| c > t) && real_windows.iter().all(|w| !w.exhausted(t)) {
            a.cooldown_until = None;
        }
        a.model_cooldowns.retain(|fam, &mut c| {
            if c <= t {
                return false;
            }
            let mut matching = real_windows.iter().filter(|w| w.applies_to(fam)).peekable();
            let covered_healthy = matching.peek().is_some() && matching.all(|w| !w.exhausted(t));
            !covered_healthy
        });
    }

    /// Selectively clear ONLY the windows, cooldowns, and paid marks proven by
    /// a keeper reset scope (`cleared_shared`, `cleared_models`, `cleared_windows`),
    /// preserving any omitted exhausted windows (such as `five_hour`) not cleared.
    #[allow(clippy::too_many_arguments)]
    fn apply_cleared_scope(
        a: &mut Account,
        cleared_shared: bool,
        cleared_models: &[String],
        cleared_windows: &[String],
        windows: Vec<Window>,
        credits: Vec<crate::model::ResetCredit>,
        force_credits: bool,
        t: i64,
    ) {
        let clears_window = |w: &Window| {
            cleared_windows.iter().any(|c| c == &w.name)
                || (cleared_shared && cleared_windows.is_empty() && w.is_shared())
                || (w.name.starts_with("model:") && w.models.iter().any(|m| cleared_models.iter().any(|scope| crate::model::family_matches(scope, m))))
        };
        if cleared_shared {
            a.cooldown_until = None;
            if a.paid.as_ref().is_none_or(|paid| cleared_windows.is_empty() || cleared_windows.iter().any(|w| w == &paid.meter)) {
                a.paid = None;
            }
        }
        a.model_cooldowns.retain(|k, _| !cleared_models.iter().any(|scope| crate::model::family_matches(scope, k)));
        a.model_paid.retain(|k, _| !cleared_models.iter().any(|scope| crate::model::family_matches(scope, k)));
        if cleared_shared && a.provider == Provider::Antigravity {
            a.model_cooldowns.retain(|k, _| a.windows.iter().any(|w| !w.is_shared() && !clears_window(w) && w.applies_to(k) && w.exhausted(t)));
            a.model_paid.retain(|k, _| a.windows.iter().any(|w| !w.is_shared() && !clears_window(w) && w.applies_to(k) && w.exhausted(t)));
        }
        for w in &mut a.windows {
            if clears_window(w) {
                w.used = 0.0;
            }
        }
        if !windows.is_empty() {
            a.paid_settled(Some(&windows), t);
            let fresh_ok = |name: &str| windows.iter().any(|w| w.name == name && w.used < 100.0);
            Self::reconcile_agy_windows(a, &windows, t);
            a.windows.retain(|w| !clears_window(w) && w.resets_at.is_some_and(|r| r > t) && (w.name.starts_with("model:") || (w.used >= 100.0 && !fresh_ok(&w.name))));
            a.merge_windows(windows);
        }
        if !credits.is_empty() || force_credits {
            a.credits = credits;
        }
    }

    /// Apply a keeper's distinct [`RemoteClaim`] outcome on a borrower pool.
    /// - Verifies `account` is actually kept by `keeper`.
    /// - For `Redeemed`, bumps revision and clears ONLY the cooldowns, paid
    ///   marks, and windows proven cleared by the reset, updating from the
    ///   keeper's post-reset snapshot.
    /// - For `AlreadyReady`, NEVER bumps revision and NEVER wipes local
    ///   exhaustion evidence or retries an account already tried at its
    ///   current revision, UNLESS the keeper attaches a strictly unseen causal
    ///   `last_reset` (`ClearedScope`), in which case ONLY that reset's proven
    ///   scope is cleared (never unrelated windows or shared cooldowns).
    fn apply_remote_claim(&self, keeper: &str, claim: RemoteClaim, family: &str, tried: &HashMap<String, u64>) -> Option<String> {
        let t = now();
        let mut p = self.pool.lock().unwrap();
        let account_id = claim.account().to_owned();
        if p.account(&account_id).is_none_or(|a| a.kept_on.as_deref() != Some(keeper)) {
            tracing::warn!("pool: ignoring remote claim reply from {keeper} for account {account_id} not kept there");
            return None;
        }
        match claim {
            RemoteClaim::Redeemed { account, credit_id, reset_id, cleared_shared, cleared_models, cleared_windows, windows, credits } => {
                *p.resets.entry(account.clone()).or_default() += 1;
                p.reset_at.insert(account.clone(), now_ms());
                if let Some(rid) = reset_id {
                    p.keeper_reset_id.insert((keeper.to_owned(), account.clone()), rid);
                }
                if let Some(a) = p.account_mut(&account) {
                    Self::apply_cleared_scope(a, cleared_shared, &cleared_models, &cleared_windows, windows, credits, !credit_id.is_empty(), t);
                }
                drop(p);
                self.touch();
                Some(account)
            }
            RemoteClaim::AlreadyReady { account, last_reset, windows, credits } => {
                let unseen_scope =
                    last_reset.as_ref().filter(|s| !s.reset_id.is_empty() && p.keeper_reset_id.get(&(keeper.to_owned(), account.clone())).map(String::as_str) != Some(s.reset_id.as_str())).cloned();
                if let Some(ref scope) = unseen_scope {
                    p.keeper_reset_id.insert((keeper.to_owned(), account.clone()), scope.reset_id.clone());
                    *p.resets.entry(account.clone()).or_default() += 1;
                    p.reset_at.insert(account.clone(), now_ms());
                }
                let rev = p.reset_revision(&account);
                let already_tried = tried.get(&account) == Some(&rev);
                let has_cred = p.saved.credentials.contains_key(&account);
                let ready = if let Some(a) = p.account_mut(&account) {
                    if let Some(scope) = unseen_scope {
                        // Apply ONLY the keeper reset's proven cleared scope so an
                        // unrelated Fable reset NEVER discards an exhausted five_hour
                        // window or shared cooldown.
                        Self::apply_cleared_scope(a, scope.cleared_shared, &scope.cleared_models, &scope.cleared_windows, windows, credits, false, t);
                    } else {
                        if !credits.is_empty() {
                            a.credits = credits;
                        }
                        let local_blocked =
                            a.cooldown_until.is_some_and(|c| c > t) || (!family.is_empty() && a.model_cooldowns.get(family).is_some_and(|&c| c > t)) || a.paid_until(family, t).is_some();
                        if !already_tried && !local_blocked && !windows.is_empty() {
                            a.merge_windows(windows);
                        }
                    }
                    !already_tried && schedule::eligible(a, a.provider, family, t, has_cred)
                } else {
                    false
                };
                drop(p);
                self.touch();
                ready.then_some(account)
            }
        }
    }

    fn build_observed_limit(p: &Pool, a: &Account, keeper: &str, family: &str, t: i64) -> Option<ObservedLimit> {
        let windows: Vec<Window> = a.windows.iter().filter(|w| w.exhausted(t)).cloned().collect();
        let cooldown_until = a.cooldown_until.filter(|&c| c > t);
        let model_cooldown = (!family.is_empty()).then(|| a.model_cooldowns.get(family).copied().filter(|&c| c > t).map(|u| (family.to_owned(), u))).flatten();
        let paid = a
            .paid
            .as_ref()
            .filter(|pf| pf.until > t)
            .map(|pf| (family.to_owned(), false, pf.until, pf.meter.clone()))
            .or_else(|| (!family.is_empty()).then(|| a.model_paid.get(family).filter(|pf| pf.until > t).map(|pf| (family.to_owned(), true, pf.until, pf.meter.clone()))).flatten());
        if windows.is_empty() && cooldown_until.is_none() && model_cooldown.is_none() && paid.is_none() {
            return None;
        }
        let last_reset_id = p.keeper_reset_id.get(&(keeper.to_owned(), a.id.clone())).cloned();
        Some(ObservedLimit { account: a.id.clone(), last_reset_id, windows, cooldown_until, model_cooldown, paid })
    }

    fn observed_limits_for(&self, provider: Provider, keeper: &str, family: &str) -> Vec<ObservedLimit> {
        let t = now();
        let p = self.pool.lock().unwrap();
        p.saved
            .accounts
            .iter()
            .filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.as_deref() == Some(keeper))
            .filter_map(|a| Self::build_observed_limit(&p, a, keeper, family, t))
            .collect()
    }

    fn observed_limit_for_account(&self, id: &str, keeper: &str) -> Vec<ObservedLimit> {
        let t = now();
        let p = self.pool.lock().unwrap();
        p.account(id).and_then(|a| Self::build_observed_limit(&p, a, keeper, "", t)).into_iter().collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn redeem_locked(
        &self,
        id: &str,
        credit: Option<&str>,
        why: Why,
        allow_legacy_borrowed: bool,
        remote_op: Option<&str>,
        deadline: Option<std::time::Instant>,
        reconcile_pending_only: bool,
    ) -> Result<RedeemedInfo, Claimed> {
        if self.drain.retired() {
            return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
        }
        let lock = self.lock_for(&format!("redeem:{id}"));
        let _held = lock.lock().await;
        if self.drain.retired() {
            return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
        }
        enum PendingCheck {
            Proceed(Provider, Option<String>, crate::model::ResetCredit, String, bool),
            AlreadyUsed(RedeemedInfo, bool),
        }
        let check = {
            let mut p = self.pool.lock().unwrap();
            let a = p.account(id).ok_or_else(|| Claimed::NotNow("No such account.".into()))?;
            if !allow_legacy_borrowed && let Some(keeper) = &a.kept_on {
                return Err(Claimed::NotNow(format!("This account’s reset credits are claimed by {keeper}, which keeps its sign-in.")));
            }
            if !allow_legacy_borrowed && let Some(host) = &a.reauth_pending_host {
                return Err(Claimed::NotNow(format!("Waiting for {host} to accept the updated sign-in before claiming a reset.")));
            }
            if reconcile_pending_only && a.redeeming.is_none() {
                return Err(Claimed::NotNow("No pending reset claim remains for this account.".into()));
            }
            if a.redeeming.is_none() && p.saved.accounts.iter().any(|other| other.provider == a.provider && (other.kept_on.is_none() || allow_legacy_borrowed) && other.redeeming.is_some()) {
                return Err(Claimed::Unclear("A previous reset claim still needs to be reconciled.".into()));
            }
            if why != Why::Asked && (!a.enabled || a.signed_out.is_some() || !p.saved.credentials.contains_key(id)) {
                return Err(Claimed::NotNow("This account is unavailable.".into()));
            }
            let a = p.account_mut(id).expect("account checked under this lock");
            let t = now();
            match a.redeeming.clone() {
                Some((cid, request)) => match a.credits.iter().find(|c| c.id == cid).cloned() {
                    Some(c) => PendingCheck::Proceed(a.provider, a.org.clone(), c, request, true),
                    None => {
                        // (Gone from what the provider lists: it was used.)
                        let provider = a.provider;
                        a.redeeming = None;
                        a.redeem_tried = None;
                        let rid = crate::store::new_uuid();
                        let cleared_shared = provider == Provider::Codex;
                        let scope = ClearedScope { reset_id: rid.clone(), cleared_shared, cleared_models: Vec::new(), cleared_windows: Vec::new() };
                        let windows = a.windows.clone();
                        let credits = a.credits.clone();
                        *p.resets.entry(id.to_owned()).or_default() += 1;
                        p.reset_at.insert(id.to_owned(), t);
                        p.last_reset.insert(id.to_owned(), scope);
                        let claim = RemoteClaim::Redeemed {
                            account: id.to_owned(),
                            credit_id: cid.clone(),
                            reset_id: Some(rid.clone()),
                            cleared_shared,
                            cleared_models: Vec::new(),
                            cleared_windows: Vec::new(),
                            windows,
                            credits,
                        };
                        let resolved = Self::resolve_pending_ops_for_account_locked(&mut p, id, Ok(&claim));
                        PendingCheck::AlreadyUsed(
                            RedeemedInfo { account: id.to_owned(), credit_id: cid, reset_id: Some(rid), cleared_shared, cleared_models: Vec::new(), cleared_windows: Vec::new() },
                            resolved,
                        )
                    }
                },
                None => {
                    let chosen = match (credit, &why) {
                        (Some(c), Why::Blocked(family)) => a.claims_for(family, t).into_iter().find(|(x, _)| x.id == c).map(|(x, _)| x.clone()),
                        (Some(c), Why::Expiring) => a.expiring_claim(t).filter(|x| x.id == c).cloned(),
                        (Some(c), Why::Asked) => a.credits.iter().find(|x| x.id == c && a.credit_claimable_now(x, t)).cloned(),
                        (None, _) => a.manual_credit(t).cloned(),
                    };
                    let c = chosen.ok_or_else(|| Claimed::NotNow("No reset credit to use for it now.".into()))?;
                    PendingCheck::Proceed(a.provider, a.org.clone(), c, crate::store::new_uuid(), false)
                }
            }
        };
        let (provider, mut org, credit, request, had_pending) = match check {
            PendingCheck::Proceed(provider, org, credit, request, had_pending) => (provider, org, credit, request, had_pending),
            PendingCheck::AlreadyUsed(info, resolved) => {
                if resolved {
                    let _ = self.save_checked().await;
                }
                self.touch();
                return Ok(info);
            }
        };
        // Renew/verify the sign-in BEFORE persisting `a.redeeming` so a sign-in
        // failure never leaves a stranded pending reset claim on disk.
        let mut cred = match self.fresh(id, provider, None, 60).await {
            Ok(c) => c,
            Err(e) => {
                if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
                    a.redeem_tried = Some(now());
                }
                return Err(Claimed::NotNow(e.to_string()));
            }
        };
        // For Claude, ensure the organization UUID is known BEFORE persisting
        // `a.redeeming` so a missing org never strands a pending claim and
        // blocks other accounts. If missing, try healing it from `/api/oauth/profile`.
        if provider == Provider::Claude {
            if !Self::valid_claude_org(org.as_deref()) {
                let sgen = self.sync_gen(id);
                if let Ok(Some(fetched)) = upstream::profile_org(&self.client_for(id), &self.up, &cred, self.like(id).as_ref()).await
                    && Self::valid_claude_org(Some(&fetched))
                    && self.sync_gen(id) == sgen
                {
                    if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
                        a.org = Some(fetched.clone());
                    }
                    self.touch();
                    org = Some(fetched);
                }
            }
            if !Self::valid_claude_org(org.as_deref()) {
                if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
                    // A Claude claim without a valid org could never have been
                    // sent upstream; drop any stale pending marker too.
                    a.redeeming = None;
                    a.redeem_tried = Some(now());
                }
                self.touch();
                return Err(Claimed::NotNow("This account's Claude organization isn't known: sign in again.".into()));
            }
        } else if provider == Provider::Antigravity {
            self.ensure_agy_project(id, &mut cred, self.like(id).as_ref(), 0).await;
            if cred.account_id.as_deref().map(str::trim).is_none_or(str::is_empty) {
                if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
                    // Never erase an already-persisted pending claim during
                    // reconciliation on a transient project-discovery failure:
                    // keep `a.redeeming` intact so the original requestId is reused.
                    if !had_pending {
                        a.redeeming = None;
                    }
                    a.redeem_tried = Some(now());
                }
                self.touch();
                let msg = "This account's Antigravity project isn't known: sync or sign in again.".to_owned();
                return Err(if had_pending { Claimed::Unclear(msg) } else { Claimed::NotNow(msg) });
            }
        }
        // Re-verify deadline, retirement, and ownership under lock right before
        // persisting intent so a slow profile/token refresh or handover can
        // never dispatch an expired or out-of-custody upstream POST.
        {
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Err(Claimed::NotNow("This reset request expired before sending.".into()));
            }
            let mut p = self.pool.lock().unwrap();
            if self.drain.retired() {
                return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
            }
            let Some(a) = p.account_mut(id) else {
                return Err(Claimed::NotNow("No such account.".into()));
            };
            if !allow_legacy_borrowed && let Some(keeper) = &a.kept_on {
                return Err(Claimed::NotNow(format!("This account’s reset credits are claimed by {keeper}, which keeps its sign-in.")));
            }
            if !allow_legacy_borrowed && let Some(host) = &a.reauth_pending_host {
                return Err(Claimed::NotNow(format!("Waiting for {host} to accept the updated sign-in before claiming a reset.")));
            }
            a.redeeming = Some((credit.id.clone(), request.clone()));
            a.redeem_tried = Some(now());
            if let Some(op) = remote_op {
                Self::record_remote_claim_pending_locked(&mut p, op, id);
            }
        }
        // (Kept atomically — both `a.redeeming` and any `remote_op` — before the provider is asked.)
        if let Err(e) = self.save_checked().await {
            let mut p = self.pool.lock().unwrap();
            if !had_pending {
                if let Some(a) = p.account_mut(id) {
                    a.redeeming = None;
                }
                if let Some(op) = remote_op {
                    p.saved.remote_claims.retain(|r| r.op_id != op);
                }
            }
            return Err(Claimed::NotNow(e));
        }
        let result = upstream::redeem(&self.client_for(id), &self.up, provider, &cred, org.as_deref(), &credit, &request, self.like(id).as_ref()).await;
        let mut info = RedeemedInfo { account: id.to_owned(), credit_id: credit.id.clone(), reset_id: None, cleared_shared: false, cleared_models: Vec::new(), cleared_windows: Vec::new() };
        let mut resolved_pending_ops = false;
        {
            let mut p = self.pool.lock().unwrap();
            let rid = if result.is_ok() {
                let rid = crate::store::new_uuid();
                *p.resets.entry(id.to_owned()).or_default() += 1;
                p.reset_at.insert(id.to_owned(), now_ms());
                info.reset_id = Some(rid.clone());
                Some(rid)
            } else {
                None
            };
            let mut post_claim: Option<Result<RemoteClaim, Claimed>> = None;
            let mut new_scope: Option<ClearedScope> = None;
            if let Some(a) = p.account_mut(id) {
                match &result {
                    Ok(summary) => {
                        a.redeeming = None;
                        // Clear failure throttle on success so a Fable-only
                        // reset never suppresses an Opus weekly reset right after.
                        a.redeem_tried = None;
                        let provider = a.provider;
                        let mut cleared_windows: Vec<String> = a.windows.iter().filter(|w| credit.clears(provider, w)).map(|w| w.name.clone()).collect();
                        // Always include the grant's own `clears` names so a borrower
                        // clears omitted windows (e.g. `five_hour`) even if this
                        // keeper didn't previously store that window entry.
                        for cname in &credit.clears {
                            if !cleared_windows.iter().any(|w| w == cname) {
                                cleared_windows.push(cname.clone());
                            }
                        }
                        let cleared_shared = a.windows.iter().any(|w| w.is_shared() && credit.clears(provider, w))
                            || (provider == Provider::Codex)
                            || credit.clears.is_empty()
                            || credit.clears.iter().any(|c| c == "five_hour" || c == "seven_day")
                            || credit.gives_back_weekly;
                        if cleared_shared {
                            a.cooldown_until = None;
                            // Clear `paid` only when this reset actually covers
                            // the meter that put the account on usage credits.
                            if a.paid.as_ref().is_none_or(|paid| {
                                cleared_windows.iter().any(|w| w == &paid.meter)
                                    || credit.clears.is_empty()
                                    || credit.clears.iter().any(|c| c == &paid.meter)
                                    || (credit.gives_back_weekly && paid.meter == "seven_day")
                            }) {
                                a.paid = None;
                            }
                        }
                        for w in a.windows.iter_mut().filter(|w| credit.clears(provider, w)) {
                            w.used = 0.0;
                        }
                        let mut cleared_models: Vec<String> = a.windows.iter().filter(|w| credit.clears(provider, w)).flat_map(|w| w.scoped_models()).collect();
                        if credit.clears.iter().any(|c| c == "seven_day_overage_included") && !cleared_models.iter().any(|m| m == "fable") {
                            cleared_models.push("fable".to_owned());
                        }
                        a.model_cooldowns.retain(|k, _| !cleared_models.iter().any(|scope| crate::model::family_matches(scope, k)));
                        a.model_paid.retain(|k, _| !cleared_models.iter().any(|scope| crate::model::family_matches(scope, k)));
                        if cleared_shared && provider == Provider::Antigravity {
                            let t = now();
                            a.model_cooldowns.retain(|k, _| a.windows.iter().any(|w| !w.is_shared() && !credit.clears(provider, w) && w.applies_to(k) && w.exhausted(t)));
                            a.model_paid.retain(|k, _| a.windows.iter().any(|w| !w.is_shared() && !credit.clears(provider, w) && w.applies_to(k) && w.exhausted(t)));
                        }
                        if let Some(summary) = summary {
                            let t = now();
                            if !summary.windows.is_empty() {
                                a.paid_settled(Some(&summary.windows), t);
                                Self::reconcile_agy_windows(a, &summary.windows, t);
                                a.merge_windows(summary.windows.clone());
                                a.synced = Some(t);
                            }
                            a.credits = summary.credits.clone();
                        } else {
                            if let Some(c) = a.credits.iter_mut().find(|c| c.id == credit.id) {
                                c.remaining = c.remaining.saturating_sub(1);
                            }
                            a.credits.retain(|c| c.remaining > 0);
                        }
                        info.cleared_shared = cleared_shared;
                        info.cleared_models = cleared_models.clone();
                        info.cleared_windows = cleared_windows.clone();
                        if let Some(ref rid) = rid {
                            new_scope = Some(ClearedScope { reset_id: rid.clone(), cleared_shared, cleared_models: cleared_models.clone(), cleared_windows: cleared_windows.clone() });
                        }
                        post_claim = Some(Ok(RemoteClaim::Redeemed {
                            account: id.to_owned(),
                            credit_id: credit.id.clone(),
                            reset_id: rid.clone(),
                            cleared_shared,
                            cleared_models,
                            cleared_windows,
                            windows: a.windows.clone(),
                            credits: a.credits.clone(),
                        }));
                    }
                    // Refused outright: nothing was spent.
                    Err(Failure::Refused(e)) => {
                        a.redeeming = None;
                        post_claim = Some(Err(Claimed::Refused(e.clone())));
                    }
                    // Never left the pool (local build/validation/connect error): clear if newly set.
                    Err(Failure::NotSent(e)) => {
                        if !had_pending {
                            a.redeeming = None;
                            post_claim = Some(Err(Claimed::NotNow(e.clone())));
                        }
                    }
                    Err(Failure::Other(_)) => {}
                }
            }
            if let Some(scope) = new_scope {
                p.last_reset.insert(id.to_owned(), scope);
            }
            if let Some(ref outcome) = post_claim {
                resolved_pending_ops = Self::resolve_pending_ops_for_account_locked(&mut p, id, outcome.as_ref());
            }
        }
        if resolved_pending_ops {
            let _ = self.save_checked().await;
        }
        self.touch();
        // (What the provider says now, after either: what it reset, or the
        // credit it refused, which may be gone. Skip a redundant follow-up read
        // when the claim response already included a full `quotaSummary`.)
        let need_sync = !matches!(&result, Ok(Some(s)) if !s.windows.is_empty());
        if need_sync {
            self.sync(Some(id)).await;
        }
        match result {
            Ok(_) => Ok(info),
            Err(Failure::Refused(e)) => Err(Claimed::Refused(e)),
            Err(Failure::NotSent(e)) => Err(Claimed::NotNow(e)),
            Err(Failure::Other(e)) => Err(Claimed::Unclear(e)),
        }
    }

    /// Save now and say whether it was kept (none to keep it in, as in
    /// tests: kept enough).
    async fn save_checked(&self) -> Result<(), String> {
        tokio::task::block_in_place(|| self.write(true)).map_err(|e| format!("The pool couldn’t keep the claim before making it: {e}"))
    }

    /// Refresh reset credits (and missing Claude org, or usage for a Codex
    /// cooldown without an exhausted window) for locally-kept enabled accounts
    /// of `provider` not checked in the last 60 s. Stops early as soon as a
    /// claimable candidate for `family` is found.
    async fn refresh_credits_if_stale(&self, provider: Provider, family: &str) {
        let t = now();
        let fetched_snapshot: HashMap<String, i64> = self.credits_fetched.lock().unwrap().clone();
        let stale: Vec<(String, bool, bool)> = {
            let p = self.pool.lock().unwrap();
            p.saved
                .accounts
                .iter()
                .filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.is_none() && p.saved.credentials.contains_key(&a.id))
                .filter(|a| fetched_snapshot.get(&a.id).is_none_or(|&at| t - at >= 60))
                .map(|a| {
                    let needs_org = provider == Provider::Claude && !Self::valid_claude_org(a.org.as_deref());
                    let needs_usage = a.cooldown_until.is_some_and(|c| c > t) && !a.windows.iter().any(|w| w.applies_to(family) && w.exhausted(t));
                    (a.id.clone(), needs_org, needs_usage)
                })
                .collect()
        };
        if stale.is_empty() {
            return;
        }
        let mut changed = false;
        for (id, needs_org, needs_usage) in stale {
            if self.drain.retired() {
                break;
            }
            let revision = self.pool.lock().unwrap().reset_revision(&id);
            let sgen = self.sync_gen(&id);
            let seq = self.begin_sync_seq(&id);
            self.credits_fetched.lock().unwrap().insert(id.clone(), now());
            let Ok(mut cred) = self.fresh(&id, provider, None, 60).await else { continue };
            let like = self.like(&id);
            let (fetched_windows, fetched_org, fetched_credits) = if provider == Provider::Antigravity {
                self.ensure_agy_project(&id, &mut cred, like.as_ref(), 60).await;
                let read = upstream::agy_quota_summary(&self.client_for(&id), &self.up, &cred, like.as_ref()).await;
                let c = match read.credits {
                    Ok(Some(c)) => {
                        self.credits_due.lock().unwrap().insert(id.clone(), now() + CREDITS_EVERY);
                        Some(c)
                    }
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!("pool: reading reset credits for {id}: {e}");
                        None
                    }
                };
                let w = match read.windows {
                    Ok(w) if !w.is_empty() => Some(w),
                    _ => None,
                };
                (w, None, c)
            } else {
                let fetched_windows = if needs_usage { upstream::usage(&self.client_for(&id), &self.up, provider, &cred, like.as_ref()).await.ok().map(|(w, _)| w) } else { None };
                let fetched_org =
                    if needs_org { upstream::profile_org(&self.client_for(&id), &self.up, &cred, like.as_ref()).await.ok().flatten().filter(|o| Self::valid_claude_org(Some(o))) } else { None };
                let fetched_credits = match upstream::credits(&self.client_for(&id), &self.up, provider, &cred, like.as_ref()).await {
                    Ok(c) => {
                        self.credits_due.lock().unwrap().insert(id.clone(), now() + CREDITS_EVERY);
                        Some(c)
                    }
                    Err(e) => {
                        tracing::warn!("pool: reading reset credits for {id}: {e}");
                        None
                    }
                };
                (fetched_windows, fetched_org, fetched_credits)
            };
            if self.sync_gen(&id) != sgen || !self.commit_sync_seq(&id, seq) {
                continue;
            }
            let found_candidate = {
                let mut p = self.pool.lock().unwrap();
                if p.reset_revision(&id) != revision {
                    continue;
                }
                if let Some(a) = p.account_mut(&id) {
                    let t = now();
                    if let Some(w) = fetched_windows {
                        a.paid_settled(Some(&w), t);
                        Self::reconcile_agy_windows(a, &w, t);
                        a.merge_windows(w);
                        changed = true;
                    }
                    if let Some(c) = fetched_credits
                        && a.credits != c
                    {
                        a.credits = c;
                        changed = true;
                    }
                    if let Some(org) = fetched_org {
                        a.org = Some(org);
                        changed = true;
                    }
                    !a.claims_for(family, t).is_empty()
                } else {
                    false
                }
            };
            if found_candidate {
                break;
            }
        }
        if changed {
            self.touch();
        }
    }

    /// Reconcile any locally-kept account with an unclear pending claim
    /// (`a.redeeming.is_some()`). Shared by local-only and mixed/remote paths.
    async fn reconcile_local_pending_locked(&self, provider: Provider, family: &str, tried: &HashMap<String, u64>, not_ready: &[String], kept_only: bool) -> Result<Option<KeptOutcome>, Claimed> {
        let t = now();
        let pending = {
            let p = self.pool.lock().unwrap();
            p.saved.accounts.iter().find(|a| a.provider == provider && a.kept_on.is_none() && a.redeeming.is_some()).map(|a| (a.id.clone(), a.redeem_tried))
        };
        let Some((id, last_tried)) = pending else { return Ok(None) };
        if last_tried.is_some_and(|r| t - r < 10) {
            return Err(Claimed::Unclear("A previous reset claim still needs to be reconciled.".into()));
        }
        match self.redeem_locked(&id, None, Why::Blocked(family.to_owned()), false, None, None, true).await {
            Ok(info) => Ok(Some(KeptOutcome::Redeemed(info))),
            Err(e @ Claimed::Unclear(_)) => {
                tracing::warn!("pool: a reset claim still unclear: {e}");
                Err(e)
            }
            Err(Claimed::NotNow(_) | Claimed::Unsupported(_) | Claimed::NoRecord(_) | Claimed::Unreachable(_)) => Err(Claimed::Unclear("A previous reset claim could not be reconciled yet.".into())),
            Err(Claimed::Refused(_)) => {
                let ready = self.pick_available(provider, family, tried, std::slice::from_ref(&id), not_ready, kept_only);
                Ok(ready.map(KeptOutcome::Ready))
            }
        }
    }

    fn pick_available(&self, provider: Provider, family: &str, tried: &HashMap<String, u64>, reconsider: &[String], not_ready: &[String], kept_only: bool) -> Option<String> {
        let p = self.pool.lock().unwrap();
        if kept_only {
            schedule::pick(&p.saved.accounts, provider, family, None, now(), &[], |a| {
                a.kept_on.is_none() && !not_ready.contains(&a.id) && p.saved.credentials.contains_key(&a.id) && (reconsider.contains(&a.id) || tried.get(&a.id) != Some(&p.reset_revision(&a.id)))
            })
            .map(|a| a.id.clone())
        } else {
            schedule::pick(&p.saved.accounts, provider, family, None, now(), &[], |a| {
                !not_ready.contains(&a.id) && p.saved.credentials.contains_key(&a.id) && (reconsider.contains(&a.id) || tried.get(&a.id) != Some(&p.reset_revision(&a.id)))
            })
            .map(|a| a.id.clone())
        }
    }

    /// Reconcile any pending unclear claim on a locally-kept account of
    /// `provider`, then claim the best locally-kept credit for `family`.
    /// Caller MUST hold `self.claims_gate(provider)`.
    #[allow(clippy::too_many_arguments)]
    async fn redeem_kept_locked(
        &self,
        provider: Provider,
        family: &str,
        tried: &HashMap<String, u64>,
        refused: &mut Vec<String>,
        not_ready: &[String],
        kept_only: bool,
        remote_op: Option<&str>,
        deadline: Option<std::time::Instant>,
    ) -> Result<KeptOutcome, Claimed> {
        if let Some(outcome) = self.reconcile_local_pending_locked(provider, family, tried, not_ready, kept_only).await? {
            return Ok(outcome);
        }
        let collect = |now_s: i64, skip: &[(String, String)]| -> Vec<(String, String, f64, usize, i64)> {
            let p = self.pool.lock().unwrap();
            let mut cands: Vec<_> = p
                .saved
                .accounts
                .iter()
                .filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.is_none() && a.reauth_pending_host.is_none())
                .filter(|a| a.redeem_tried.is_none_or(|r| now_s - r >= 60))
                .flat_map(|a| {
                    a.claims_for(family, now_s)
                        .into_iter()
                        .filter(|(c, _)| !skip.iter().any(|(si, sc)| si == &a.id && sc == &c.id))
                        .map(|(c, worth)| (a.id.clone(), c.id.clone(), worth, c.breadth(), c.expires_at.unwrap_or(i64::MAX)))
                })
                .collect();
            cands.sort_by(|x, y| y.2.total_cmp(&x.2).then(x.3.cmp(&y.3)).then(x.4.cmp(&y.4)));
            cands
        };
        let mut tried_pairs: Vec<(String, String)> = Vec::new();
        let mut refreshed = false;
        loop {
            let t = now();
            let candidates = collect(t, &tried_pairs);
            if candidates.is_empty() {
                if refreshed {
                    break;
                }
                refreshed = true;
                self.refresh_credits_if_stale(provider, family).await;
                if let Some(ready) = self.pick_available(provider, family, tried, refused, not_ready, kept_only) {
                    return Ok(KeptOutcome::Ready(ready));
                }
                continue;
            }
            for (id, credit, _, _, _) in candidates {
                if let Some(ready) = self.pick_available(provider, family, tried, refused, not_ready, kept_only) {
                    return Ok(KeptOutcome::Ready(ready));
                }
                tried_pairs.push((id.clone(), credit.clone()));
                match self.redeem_locked(&id, Some(&credit), Why::Blocked(family.to_owned()), false, remote_op, deadline, false).await {
                    Ok(info) => return Ok(KeptOutcome::Redeemed(info)),
                    Err(e @ Claimed::Refused(_)) => {
                        tracing::info!("pool: a reset credit not used: {e}; the next");
                        refused.push(id);
                    }
                    Err(e @ Claimed::NotNow(_)) => tracing::info!("pool: a reset credit not used: {e}; the next"),
                    Err(e) => {
                        tracing::warn!("pool: redeeming a reset credit: {e}");
                        return Err(e);
                    }
                }
            }
            // If all cached candidates failed definitively, do one bounded
            // stale-credit refresh so newly issued grants on other accounts
            // can still rescue this request.
            if refreshed {
                break;
            }
            refreshed = true;
            self.refresh_credits_if_stale(provider, family).await;
            if let Some(ready) = self.pick_available(provider, family, tried, refused, not_ready, kept_only) {
                return Ok(KeptOutcome::Ready(ready));
            }
        }
        Ok(match self.pick_available(provider, family, tried, refused, not_ready, kept_only) {
            Some(id) => KeptOutcome::Ready(id),
            None => KeptOutcome::None,
        })
    }

    fn remote_claim_snapshot(&self, info: RedeemedInfo) -> RemoteClaim {
        let (windows, credits) = {
            let p = self.pool.lock().unwrap();
            p.account(&info.account).map(|a| (a.windows.clone(), a.credits.clone())).unwrap_or_default()
        };
        RemoteClaim::Redeemed {
            account: info.account,
            credit_id: info.credit_id,
            reset_id: info.reset_id,
            cleared_shared: info.cleared_shared,
            cleared_models: info.cleared_models,
            cleared_windows: info.cleared_windows,
            windows,
            credits,
        }
    }

    fn already_ready_snapshot(&self, account: String) -> RemoteClaim {
        let (last_reset, windows, credits) = {
            let p = self.pool.lock().unwrap();
            let last_reset = p.last_reset.get(&account).cloned();
            let (windows, credits) = p.account(&account).map(|a| (a.windows.clone(), a.credits.clone())).unwrap_or_default();
            (last_reset, windows, credits)
        };
        RemoteClaim::AlreadyReady { account, last_reset, windows, credits }
    }

    fn merge_observed_limits(&self, provider: Provider, observed: &[ObservedLimit], only_account: Option<&str>) -> Vec<String> {
        let t = now();
        let mut observed_ids = Vec::new();
        let mut p = self.pool.lock().unwrap();
        for obs in observed {
            if only_account.is_some_and(|id| id != obs.account) {
                continue;
            }
            if p.last_reset.get(&obs.account).is_some_and(|cur| obs.last_reset_id.as_deref() != Some(cur.reset_id.as_str())) {
                continue;
            }
            let Some(a) = p.account_mut(&obs.account) else { continue };
            if a.kept_on.is_some() || a.provider != provider {
                continue;
            }
            observed_ids.push(obs.account.clone());
            if !obs.windows.is_empty() {
                a.merge_windows(obs.windows.clone());
            }
            if let Some(u) = obs.cooldown_until.filter(|&u| u > t) {
                a.cooldown_until = Some(a.cooldown_until.map_or(u, |cur| cur.max(u)));
            }
            if let Some((ref f, u)) = obs.model_cooldown
                && u > t
            {
                let entry = a.model_cooldowns.entry(f.clone()).or_insert(0);
                *entry = (*entry).max(u);
            }
            if let Some((ref f, model_only, u, ref meter)) = obs.paid
                && u > t
            {
                a.now_paid(f, model_only, u, meter);
            }
        }
        observed_ids
    }

    /// Keeper's handler for a borrower pool's `host::Ask::Redeem`:
    /// - Serialized under this keeper's `claims_gate(provider)`.
    /// - Deduplicated by durable `op_id` (`Saved::remote_claims`) checked BEFORE
    ///   retirement so a borrower retrying after a lost reply, HTTP 500, or
    ///   handover receives the original outcome (or reconciles `a.redeeming`)
    ///   instead of double-spending.
    /// - Stamps relative `timeout_secs` on receipt with the keeper's monotonic
    ///   clock and re-checks before upstream dispatch.
    /// - Merges borrower's `observed` exhaustion limits for BOTH manual
    ///   (`id: Some`) and automatic (`id: None`) asks.
    /// - Returns distinct [`RemoteClaim::Redeemed`] vs [`RemoteClaim::AlreadyReady`],
    ///   and distinct [`Claimed::NoRecord`] ONLY when `reconcile_only` truly has
    ///   no record on an active keeper.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_remote_redeem(
        &self,
        id: Option<String>,
        credit: Option<String>,
        provider: Option<Provider>,
        family: Option<String>,
        op_id: Option<String>,
        timeout_secs: Option<u64>,
        reconcile_only: bool,
        observed: Vec<ObservedLimit>,
    ) -> Result<RemoteClaim, Claimed> {
        let deadline = timeout_secs.map(|s| std::time::Instant::now() + Duration::from_secs(s));
        let provider = match (provider, id.as_deref()) {
            (Some(p), _) => p,
            (None, Some(aid)) => self.pool.lock().unwrap().account(aid).map(|a| a.provider).ok_or_else(|| Claimed::NotNow("No such account.".into()))?,
            (None, None) => return Err(Claimed::NotNow("Neither an account nor a provider was named.".into())),
        };
        let gate = self.claims_gate(provider);
        let _gate = gate.lock().await;
        // 1. Check durable op_id record BEFORE retirement so a retiring copy can
        //    still return a recorded outcome (or report Unclear, never NoRecord).
        if let Some(ref op) = op_id
            && let Some(rec) = self.lookup_remote_claim_record(op)
        {
            if let Some(claim) = rec.result {
                return Ok(claim);
            }
            match rec.error_kind.as_deref() {
                Some("refused") => return Err(Claimed::Refused(rec.error.unwrap_or_else(|| "Previous reset request was refused.".into()))),
                Some("not_now") => return Err(Claimed::NotNow(rec.error.unwrap_or_else(|| "Previous reset request did not succeed.".into()))),
                Some("unclear") => {
                    if self.drain.retired() {
                        return Err(Claimed::Unclear("Previous reset claim is still pending on a retiring copy.".into()));
                    }
                    if let Some(ref aid) = rec.account {
                        let still_pending = self.pool.lock().unwrap().account(aid).is_some_and(|a| a.redeeming.is_some());
                        if still_pending {
                            // Refresh credits first so a crash-before-sync sees a spent grant without re-POSTing.
                            self.sync(Some(aid)).await;
                            match self.redeem_locked(aid, None, Why::Asked, false, Some(op), deadline, true).await {
                                Ok(info) => {
                                    let claim = self.remote_claim_snapshot(info);
                                    self.record_remote_claim_outcome(op, Ok(&claim));
                                    let _ = self.save_checked().await;
                                    self.touch();
                                    return Ok(claim);
                                }
                                Err(e @ (Claimed::Refused(_) | Claimed::NotNow(_))) => {
                                    self.record_remote_claim_outcome(op, Err(&e));
                                    let _ = self.save_checked().await;
                                    self.touch();
                                    return Err(e);
                                }
                                Err(e) => return Err(Claimed::Unclear(e.to_string())),
                            }
                        }
                    }
                    return Err(Claimed::Unclear(rec.error.unwrap_or_else(|| "Previous reset request is still unclear.".into())));
                }
                _ => return Err(Claimed::Unclear(rec.error.unwrap_or_else(|| "Previous reset request is still unclear.".into()))),
            }
        }
        if self.drain.retired() {
            return Err(Claimed::NotNow("This copy of the pool has handed over.".into()));
        }
        if reconcile_only {
            return Err(Claimed::NoRecord("No record of that reset request on the keeper.".into()));
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(Claimed::NotNow("This reset request expired before reaching the keeper.".into()));
        }
        // Receiving an Ask::Redeem proves a v2+ peer delegates claiming here.
        {
            let mut p = self.pool.lock().unwrap();
            p.saved.legacy_desktop_claims = false;
        }
        let outcome: Result<RemoteClaim, Claimed> = if let Some(id) = id {
            // Merge borrower observations for `id` so manual "Use a Reset Credit…"
            // sees limits only the borrower observed (e.g. unlisted `five_hour`).
            let _ = self.merge_observed_limits(provider, &observed, Some(&id));
            self.redeem_locked(&id, credit.as_deref(), Why::Asked, false, op_id.as_deref(), deadline, false).await.map(|info| self.remote_claim_snapshot(info))
        } else {
            let fam = family.unwrap_or_default();
            let t = now();
            let observed_ids = self.merge_observed_limits(provider, &observed, None);
            // Confirm any locally-kept account that still appears eligible
            // unless it was just reset (< 30 s) or freshly synced (< 15 s).
            let to_confirm: Vec<String> = {
                let p = self.pool.lock().unwrap();
                p.saved
                    .accounts
                    .iter()
                    .filter(|a| a.kept_on.is_none() && schedule::eligible(a, provider, &fam, t, p.saved.credentials.contains_key(&a.id)))
                    .filter(|a| p.reset_at.get(&a.id).is_none_or(|&r| t - r >= 30))
                    .filter(|a| a.synced.is_none_or(|s| t - s >= 15))
                    .map(|a| a.id.clone())
                    .collect()
            };
            let mut confirm_failed: Vec<String> = Vec::new();
            for aid in to_confirm {
                let before_synced = self.pool.lock().unwrap().account(&aid).and_then(|a| a.synced);
                self.sync(Some(&aid)).await;
                let after_synced = self.pool.lock().unwrap().account(&aid).and_then(|a| a.synced);
                // If usage read failed (e.g. 429 on /api/oauth/usage) and didn't
                // advance `synced`, don't falsely report a stale account as ready.
                if after_synced == before_synced {
                    confirm_failed.push(aid);
                }
            }
            let mut not_ready = confirm_failed;
            for oid in &observed_ids {
                if !not_ready.contains(oid) {
                    not_ready.push(oid.clone());
                }
            }
            let ready = {
                let p = self.pool.lock().unwrap();
                schedule::pick(&p.saved.accounts, provider, &fam, None, now(), &[], |a| a.kept_on.is_none() && p.saved.credentials.contains_key(&a.id) && !not_ready.contains(&a.id))
                    .map(|a| a.id.clone())
            };
            if let Some(aid) = ready {
                Ok(self.already_ready_snapshot(aid))
            } else {
                let mut refused = Vec::new();
                match self.redeem_kept_locked(provider, &fam, &HashMap::new(), &mut refused, &not_ready, true, op_id.as_deref(), deadline).await {
                    Ok(KeptOutcome::Redeemed(info)) => Ok(self.remote_claim_snapshot(info)),
                    Ok(KeptOutcome::Ready(aid)) => Ok(self.already_ready_snapshot(aid)),
                    Ok(KeptOutcome::None) => Err(Claimed::NotNow("No reset credit to use for it now.".into())),
                    Err(e) => Err(e),
                }
            }
        };
        if let Some(ref op) = op_id {
            match &outcome {
                Ok(claim) => {
                    self.record_remote_claim_outcome(op, Ok(claim));
                    if matches!(claim, RemoteClaim::Redeemed { .. }) {
                        let _ = self.save_checked().await;
                    }
                    self.touch();
                }
                Err(e @ (Claimed::Refused(_) | Claimed::NotNow(_))) => {
                    self.record_remote_claim_outcome(op, Err(e));
                    self.touch();
                }
                Err(Claimed::Unclear(_) | Claimed::Unsupported(_) | Claimed::NoRecord(_) | Claimed::Unreachable(_)) => {}
            }
        }
        outcome
    }

    /// Convenience wrapper for tests calling remote exhaustion directly.
    pub async fn redeem_for_remote(&self, provider: Provider, family: &str) -> Result<RemoteClaim, Claimed> {
        self.handle_remote_redeem(None, None, Some(provider), Some(family.to_owned()), None, None, false, Vec::new()).await
    }

    /// Recheck which account can serve under the provider's claim lock.
    /// Locally-kept accounts (`kept_on.is_none()`) are claimed directly here
    /// (unless this host is paired with a legacy v1 desktop); accounts kept on
    /// another pool are delegated to their keeper (`Claimer`) with durable
    /// `op_id` reconciliation and observed exhaustion limits.
    async fn redeem_for(&self, provider: Provider, family: &str, tried: &HashMap<String, u64>) -> RedeemOutcome {
        let gate = self.claims_gate(provider);
        let _gate = gate.lock().await;
        if self.drain.retired() {
            return RedeemOutcome::None;
        }
        if let Some(id) = self.pick_available(provider, family, tried, &[], &[], false) {
            return RedeemOutcome::Ready(id);
        }
        // Reconcile any pending unclear remote claim before doing anything else.
        match self.reconcile_pending_remote_locked(provider, family, tried).await {
            Ok(Some(id)) => return RedeemOutcome::Ready(id),
            Ok(None) => {
                if let Some(id) = self.pick_available(provider, family, tried, &[], &[], false) {
                    return RedeemOutcome::Ready(id);
                }
            }
            Err(_) => return RedeemOutcome::ResetWaits,
        }
        // On a host paired with a legacy v1 desktop (`legacy_desktop_claims`),
        // let the legacy desktop claim to avoid double-claiming until it upgrades.
        if self.is_hosted() && self.pool.lock().unwrap().saved.legacy_desktop_claims {
            let t = now();
            let waiting = self.pool.lock().unwrap().saved.accounts.iter().any(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.claim_for(family, t).is_some());
            return if waiting { RedeemOutcome::ResetWaits } else { RedeemOutcome::None };
        }
        let mut refused: Vec<String> = Vec::new();
        let claimer = self.claimer();
        let has_remote_accounts = self.pool.lock().unwrap().saved.accounts.iter().any(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.is_some());
        let Some(claimer) = claimer.filter(|_| has_remote_accounts) else {
            let local_res = self.redeem_kept_locked(provider, family, tried, &mut refused, &[], false, None, None).await;
            return match local_res {
                Ok(KeptOutcome::Redeemed(info)) => RedeemOutcome::Ready(info.account),
                Ok(KeptOutcome::Ready(id)) => RedeemOutcome::Ready(id),
                Err(Claimed::Unclear(_)) => RedeemOutcome::ResetWaits,
                Ok(KeptOutcome::None) | Err(_) => {
                    // A headless borrower (no Claimer) with a cached credit
                    // waiting on a remote keeper holds off usage credits.
                    let t = now();
                    let remote_waits = has_remote_accounts
                        && self
                            .pool
                            .lock()
                            .unwrap()
                            .saved
                            .accounts
                            .iter()
                            .any(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.is_some() && a.claim_for(family, t).is_some());
                    if remote_waits { RedeemOutcome::ResetWaits } else { RedeemOutcome::None }
                }
            };
        };
        // Mixed or remote-kept pool (desktop): reconcile local pending first.
        match self.reconcile_local_pending_locked(provider, family, tried, &[], false).await {
            Ok(Some(KeptOutcome::Redeemed(info))) => return RedeemOutcome::Ready(info.account),
            Ok(Some(KeptOutcome::Ready(id))) => return RedeemOutcome::Ready(id),
            Ok(Some(KeptOutcome::None)) | Ok(None) => {}
            Err(_) => return RedeemOutcome::ResetWaits,
        }
        let collect_all = |now_s: i64, skip_local: &[(String, String)]| -> Vec<(String, Option<String>, String, f64, usize, i64)> {
            let p = self.pool.lock().unwrap();
            let mut cands: Vec<_> = p
                .saved
                .accounts
                .iter()
                .filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none())
                .filter(|a| a.kept_on.is_some() || a.reauth_pending_host.is_none())
                .filter(|a| a.redeem_tried.is_none_or(|r| now_s - r >= 60))
                .flat_map(|a| {
                    a.claims_for(family, now_s)
                        .into_iter()
                        .filter(|(c, _)| a.kept_on.is_some() || !skip_local.iter().any(|(si, sc)| si == &a.id && sc == &c.id))
                        .map(|(c, worth)| (a.id.clone(), a.kept_on.clone(), c.id.clone(), worth, c.breadth(), c.expires_at.unwrap_or(i64::MAX)))
                })
                .collect();
            cands.sort_by(|x, y| y.3.total_cmp(&x.3).then(x.4.cmp(&y.4)).then(x.5.cmp(&y.5)));
            cands
        };
        let mut tried_local_pairs: Vec<(String, String)> = Vec::new();
        let mut tried_keepers: Vec<String> = Vec::new();
        let mut legacy_keepers: Vec<String> = Vec::new();
        let mut unreachable_keepers: Vec<String> = Vec::new();
        let mut refreshed_local = false;
        let is_unreachable_cached = |k: &str| -> bool { self.keeper_unreachable.lock().unwrap().get(k).is_some_and(|&at| now() - at < 60) };
        let is_probe_throttled = |k: &str| -> bool { self.remote_claim_tried.lock().unwrap().get(&(k.to_owned(), provider, family.to_owned())).is_some_and(|&at| now() - at < 60) };
        loop {
            let mut candidates = collect_all(now(), &tried_local_pairs);
            if !refreshed_local && !candidates.iter().any(|c| c.1.is_none()) {
                refreshed_local = true;
                self.refresh_credits_if_stale(provider, family).await;
                if let Some(ready) = self.pick_available(provider, family, tried, &refused, &[], false) {
                    return RedeemOutcome::Ready(ready);
                }
                candidates = collect_all(now(), &tried_local_pairs);
            }
            let mut tried_any_local = false;
            for (id, kept_on, credit, _, _, _) in candidates {
                if let Some(ready) = self.pick_available(provider, family, tried, &refused, &[], false) {
                    return RedeemOutcome::Ready(ready);
                }
                match kept_on {
                    None => {
                        tried_any_local = true;
                        tried_local_pairs.push((id.clone(), credit.clone()));
                        match self.redeem_locked(&id, Some(&credit), Why::Blocked(family.to_owned()), false, None, None, false).await {
                            Ok(info) => return RedeemOutcome::Ready(info.account),
                            Err(e @ Claimed::Refused(_)) => {
                                tracing::info!("pool: a reset credit not used: {e}; the next");
                                refused.push(id);
                            }
                            Err(e @ Claimed::NotNow(_)) => tracing::info!("pool: a reset credit not used: {e}; the next"),
                            Err(e) => {
                                tracing::warn!("pool: redeeming a reset credit: {e}");
                                return RedeemOutcome::ResetWaits;
                            }
                        }
                    }
                    Some(keeper) => {
                        if legacy_keepers.contains(&keeper) {
                            match self.redeem_locked(&id, Some(&credit), Why::Blocked(family.to_owned()), true, None, None, false).await {
                                Ok(info) => return RedeemOutcome::Ready(info.account),
                                Err(Claimed::Refused(_)) => refused.push(id),
                                Err(Claimed::NotNow(_) | Claimed::Unsupported(_) | Claimed::NoRecord(_) | Claimed::Unreachable(_)) => {}
                                Err(_) => return RedeemOutcome::ResetWaits,
                            }
                            continue;
                        }
                        if tried_keepers.contains(&keeper) {
                            continue;
                        }
                        if is_unreachable_cached(&keeper) {
                            if !unreachable_keepers.contains(&keeper) {
                                unreachable_keepers.push(keeper.clone());
                            }
                            tried_keepers.push(keeper);
                            continue;
                        }
                        if is_probe_throttled(&keeper) {
                            tried_keepers.push(keeper);
                            continue;
                        }
                        tried_keepers.push(keeper.clone());
                        let op_id = crate::store::new_uuid();
                        let observed = self.observed_limits_for(provider, &keeper, family);
                        let t = now();
                        if self
                            .persist_pending_remote_claim(PendingRemoteClaim {
                                provider,
                                keeper: keeper.clone(),
                                op_id: op_id.clone(),
                                account: None,
                                credit: None,
                                family: family.to_owned(),
                                at: t,
                                tried_at: t,
                            })
                            .await
                            .is_err()
                        {
                            continue;
                        }
                        let ask = ClaimAsk::For { provider, family: family.to_owned(), op_id: op_id.clone(), timeout_secs: Some(90), reconcile_only: false, observed };
                        match claimer(keeper.clone(), ask).await {
                            Ok(claim) => {
                                self.clear_pending_remote_claim(provider, &op_id).await;
                                self.keeper_unreachable.lock().unwrap().remove(&keeper);
                                if let Some(ready) = self.apply_remote_claim(&keeper, claim, family, tried) {
                                    return RedeemOutcome::Ready(ready);
                                }
                                // Keeper said AlreadyReady for an account we already
                                // tried / can't use -> throttle probes for this family.
                                self.remote_claim_tried.lock().unwrap().insert((keeper, provider, family.to_owned()), now());
                            }
                            Err(Claimed::Unsupported(_)) => {
                                self.clear_pending_remote_claim(provider, &op_id).await;
                                legacy_keepers.push(keeper);
                                match self.redeem_locked(&id, Some(&credit), Why::Blocked(family.to_owned()), true, None, None, false).await {
                                    Ok(info) => return RedeemOutcome::Ready(info.account),
                                    Err(Claimed::Refused(_)) => refused.push(id),
                                    Err(Claimed::NotNow(_) | Claimed::Unsupported(_) | Claimed::NoRecord(_) | Claimed::Unreachable(_)) => {}
                                    Err(_) => return RedeemOutcome::ResetWaits,
                                }
                            }
                            Err(Claimed::Unclear(e)) => {
                                tracing::warn!("pool: redeeming a reset credit on {keeper}: {e}");
                                // PendingRemoteClaim is already persisted on disk.
                                return RedeemOutcome::ResetWaits;
                            }
                            Err(Claimed::Unreachable(e)) => {
                                self.clear_pending_remote_claim(provider, &op_id).await;
                                tracing::info!("pool: {keeper} couldn’t be reached for a reset credit: {e}");
                                self.keeper_unreachable.lock().unwrap().insert(keeper.clone(), now());
                                if !unreachable_keepers.contains(&keeper) {
                                    unreachable_keepers.push(keeper);
                                }
                            }
                            Err(e) => {
                                self.clear_pending_remote_claim(provider, &op_id).await;
                                tracing::info!("pool: {keeper} couldn’t claim a reset credit: {e}; the next");
                                self.remote_claim_tried.lock().unwrap().insert((keeper, provider, family.to_owned()), now());
                            }
                        }
                    }
                }
            }
            // If cached local candidates all failed definitively, perform ONE
            // bounded stale-credit refresh so newly issued local grants can still rescue.
            if tried_any_local && !refreshed_local {
                refreshed_local = true;
                self.refresh_credits_if_stale(provider, family).await;
                if let Some(ready) = self.pick_available(provider, family, tried, &refused, &[], false) {
                    return RedeemOutcome::Ready(ready);
                }
                continue;
            }
            break;
        }
        // Even if this borrower's local credit cache was empty/stale, ask any
        // remaining keeper once (throttled per keeper+provider+family to 60 s).
        let keepers: Vec<String> = {
            let p = self.pool.lock().unwrap();
            let mut out = Vec::new();
            for a in p.saved.accounts.iter().filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none()) {
                if let Some(k) = &a.kept_on
                    && !tried_keepers.contains(k)
                    && !out.contains(k)
                {
                    out.push(k.clone());
                }
            }
            out
        };
        for keeper in keepers {
            if let Some(ready) = self.pick_available(provider, family, tried, &refused, &[], false) {
                return RedeemOutcome::Ready(ready);
            }
            if is_unreachable_cached(&keeper) {
                if !unreachable_keepers.contains(&keeper) {
                    unreachable_keepers.push(keeper);
                }
                continue;
            }
            if is_probe_throttled(&keeper) {
                continue;
            }
            let op_id = crate::store::new_uuid();
            let observed = self.observed_limits_for(provider, &keeper, family);
            let t = now();
            if self
                .persist_pending_remote_claim(PendingRemoteClaim { provider, keeper: keeper.clone(), op_id: op_id.clone(), account: None, credit: None, family: family.to_owned(), at: t, tried_at: t })
                .await
                .is_err()
            {
                continue;
            }
            let ask = ClaimAsk::For { provider, family: family.to_owned(), op_id: op_id.clone(), timeout_secs: Some(90), reconcile_only: false, observed };
            match claimer(keeper.clone(), ask).await {
                Ok(claim) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    self.keeper_unreachable.lock().unwrap().remove(&keeper);
                    if let Some(ready) = self.apply_remote_claim(&keeper, claim, family, tried) {
                        return RedeemOutcome::Ready(ready);
                    }
                    self.remote_claim_tried.lock().unwrap().insert((keeper, provider, family.to_owned()), now());
                }
                Err(Claimed::Unsupported(_)) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    // Legacy v1 host with empty cache here: nothing cached to claim.
                    self.remote_claim_tried.lock().unwrap().insert((keeper, provider, family.to_owned()), now());
                }
                Err(Claimed::Unclear(e)) => {
                    tracing::warn!("pool: redeeming a reset credit on {keeper}: {e}");
                    return RedeemOutcome::ResetWaits;
                }
                Err(Claimed::Unreachable(e)) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    tracing::info!("pool: {keeper} couldn’t be reached for a reset credit: {e}");
                    self.keeper_unreachable.lock().unwrap().insert(keeper.clone(), now());
                    if !unreachable_keepers.contains(&keeper) {
                        unreachable_keepers.push(keeper);
                    }
                }
                Err(e) => {
                    self.clear_pending_remote_claim(provider, &op_id).await;
                    tracing::info!("pool: {keeper} had no reset credit to claim: {e}");
                    self.remote_claim_tried.lock().unwrap().insert((keeper, provider, family.to_owned()), now());
                }
            }
        }
        if let Some(ready) = self.pick_available(provider, family, tried, &refused, &[], false) {
            return RedeemOutcome::Ready(ready);
        }
        // Only hold off usage credits (`ResetWaits`) for an unreachable keeper
        // when we actually know a claimable reset credit waits on that keeper.
        let t = now();
        let waiting_on_unreachable = !unreachable_keepers.is_empty()
            && self
                .pool
                .lock()
                .unwrap()
                .saved
                .accounts
                .iter()
                .any(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.kept_on.as_ref().is_some_and(|k| unreachable_keepers.contains(k)) && a.claim_for(family, t).is_some());
        if waiting_on_unreachable { RedeemOutcome::ResetWaits } else { RedeemOutcome::None }
    }

    /// Claim credits that would otherwise expire unused, each at its best
    /// moment (`Account::expiring_claim`). At most once a minute per
    /// account; only for accounts this pool keeps (`kept_on.is_none()`, so
    /// two pools never claim the same reset); none from a copy that has
    /// handed over or a host paired with a legacy v1 desktop.
    async fn spend_expiring(&self) {
        if self.drain.retired() {
            return;
        }
        let t = now();
        let due: Vec<(String, String)> = {
            let p = self.pool.lock().unwrap();
            if self.is_hosted() && p.saved.legacy_desktop_claims {
                return;
            }
            p.saved
                .accounts
                .iter()
                .filter(|a| a.enabled && a.signed_out.is_none() && a.kept_on.is_none() && a.reauth_pending_host.is_none() && a.redeem_tried.is_none_or(|r| t - r >= 60))
                .filter(|a| !p.saved.pending_remote_claims.iter().any(|r| r.provider == a.provider))
                .filter_map(|a| a.expiring_claim(t).map(|c| (a.id.clone(), c.id.clone())))
                .collect()
        };
        for (id, credit) in due {
            match self.redeem_credit(&id, Some(&credit), Why::Expiring).await {
                Ok(()) => tracing::info!("pool: redeemed a reset credit before it expired"),
                Err(e) => tracing::warn!("pool: redeeming an expiring reset credit: {e}"),
            }
        }
    }

    /// Refresh sign-ins due within ten minutes and read the quota of
    /// every enabled account due a read (every `SYNC_EVERY`, longer after
    /// the provider asks the pool to slow down), or just `only`'s, now.
    pub async fn sync(&self, only: Option<&str>) {
        // (Handed over: the new copy reads quotas and renews sign-ins.)
        if self.drain.retired() {
            return;
        }
        let t = now();
        let accounts: Vec<(String, Provider, bool)> = {
            let p = self.pool.lock().unwrap();
            let due = self.due.lock().unwrap();
            p.saved
                .accounts
                .iter()
                .filter(|a| only.is_none_or(|o| o == a.id) && (only.is_some() || a.enabled) && a.signed_out.is_none())
                .filter(|a| only.is_some() || due.get(&a.id).is_none_or(|(at, _)| *at <= t))
                .map(|a| (a.id.clone(), a.provider, a.provider == Provider::Claude && !Self::valid_claude_org(a.org.as_deref())))
                .collect()
        };
        for (id, provider, needs_org) in accounts {
            let revision = self.pool.lock().unwrap().reset_revision(&id);
            let sgen = self.sync_gen(&id);
            let seq = self.begin_sync_seq(&id);
            let Ok(mut cred) = self.fresh(&id, provider, None, 600).await else { continue };
            let like = self.like(&id);
            let (report, fetched_org, credits) = if provider == Provider::Antigravity {
                self.ensure_agy_project(&id, &mut cred, like.as_ref(), if only.is_some() { 0 } else { 3600 }).await;
                let mut read = upstream::agy_quota_summary(&self.client_for(&id), &self.up, &cred, like.as_ref()).await;
                if read.windows.as_ref().is_err_and(Failure::is_unauthorized) && read.credits.as_ref().is_err_and(Failure::is_unauthorized) {
                    read = match self.fresh(&id, provider, Some(&cred), 0).await {
                        Ok(mut c) => {
                            self.ensure_agy_project(&id, &mut c, like.as_ref(), 0).await;
                            upstream::agy_quota_summary(&self.client_for(&id), &self.up, &c, like.as_ref()).await
                        }
                        Err(e) => upstream::AgyQuotaRead { windows: Err(e.clone()), credits: Err(e) },
                    };
                }
                {
                    let mut due = self.due.lock().unwrap();
                    let backoff = due.get(&id).map_or(0, |d| d.1);
                    let rate_limited = read.windows.as_ref().is_err_and(Failure::rate_limited) || read.credits.as_ref().is_err_and(Failure::rate_limited);
                    let next = if rate_limited {
                        let wait = (backoff * 2).clamp(SYNC_EVERY, SYNC_BACKOFF_MAX);
                        (now() + wait, wait)
                    } else {
                        (now() + SYNC_EVERY, 0)
                    };
                    due.insert(id.clone(), next);
                }
                let agy_credits = match read.credits {
                    Ok(Some(c)) => {
                        let ts = now();
                        self.credits_due.lock().unwrap().insert(id.clone(), ts + CREDITS_EVERY);
                        self.credits_fetched.lock().unwrap().insert(id.clone(), ts);
                        Some(c)
                    }
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!("pool: reading reset credits for {id}: {e}");
                        None
                    }
                };
                let report = read.windows.map(|w| (w, None));
                (report, None, agy_credits)
            } else {
                let report = match upstream::usage(&self.client_for(&id), &self.up, provider, &cred, like.as_ref()).await {
                    Err(Failure::Refused(_)) => match self.fresh(&id, provider, Some(&cred), 0).await {
                        Ok(c) => upstream::usage(&self.client_for(&id), &self.up, provider, &c, like.as_ref()).await,
                        Err(e) => Err(e),
                    },
                    r => r,
                };
                // The next read: later, and later again while the provider
                // keeps asking the pool to slow down.
                {
                    let mut due = self.due.lock().unwrap();
                    let backoff = due.get(&id).map_or(0, |d| d.1);
                    let next = match &report {
                        Err(e) if e.rate_limited() => {
                            let wait = (backoff * 2).clamp(SYNC_EVERY, SYNC_BACKOFF_MAX);
                            (now() + wait, wait)
                        }
                        _ => (now() + SYNC_EVERY, 0),
                    };
                    due.insert(id.clone(), next);
                }
                let credits_due = only.is_some() || self.credits_due.lock().unwrap().get(&id).is_none_or(|&at| at <= now());
                let credits_cred = match &report {
                    Ok(_) if credits_due => {
                        self.credits_due.lock().unwrap().insert(id.clone(), now() + CREDITS_EVERY);
                        self.fresh(&id, provider, None, 60).await.ok()
                    }
                    _ => None,
                };
                let profile_allowed = needs_org && self.profile_tried.lock().unwrap().get(&id).is_none_or(|&at| now() - at >= 3600);
                let fetched_org = if profile_allowed && let Some(c) = &credits_cred {
                    match upstream::profile_org(&self.client_for(&id), &self.up, c, like.as_ref()).await {
                        Ok(Some(o)) if Self::valid_claude_org(Some(&o)) => Some(o),
                        _ => {
                            self.profile_tried.lock().unwrap().insert(id.clone(), now());
                            None
                        }
                    }
                } else {
                    None
                };
                let credits = match credits_cred {
                    Some(c) => {
                        self.credits_fetched.lock().unwrap().insert(id.clone(), now());
                        match upstream::credits(&self.client_for(&id), &self.up, provider, &c, like.as_ref()).await {
                            Ok(c) => Some(c),
                            Err(e) => {
                                tracing::warn!("pool: reading reset credits for {id}: {e}");
                                None
                            }
                        }
                    }
                    None => None,
                };
                (report, fetched_org, credits)
            };
            if self.sync_gen(&id) != sgen || !self.commit_sync_seq(&id, seq) {
                continue;
            }
            let mut p = self.pool.lock().unwrap();
            if p.reset_revision(&id) != revision {
                continue;
            }
            let Some(a) = p.account_mut(&id) else { continue };
            if let Some(org) = fetched_org {
                a.org = Some(org);
            }
            if let Some(c) = credits {
                a.credits = c;
            }
            match report {
                Ok((windows, plan)) => {
                    // The report is the truth; preserve per-model windows AND
                    // any unexpired exhausted header window (e.g. `five_hour`)
                    // omitted from a sparse report until it resets or the report
                    // explicitly shows headroom (< 100%).
                    let t = now();
                    a.paid_settled(Some(&windows), t);
                    let fresh_ok = |name: &str| windows.iter().any(|w| w.name == name && w.used < 100.0);
                    Self::reconcile_agy_windows(a, &windows, t);
                    a.windows.retain(|w| w.resets_at.is_some_and(|r| r > t) && (w.name.starts_with("model:") || (w.used >= 100.0 && !fresh_ok(&w.name))));
                    a.merge_windows(windows);
                    if plan.is_some() {
                        a.plan = plan;
                    }
                    a.synced = Some(t);
                    if a.cooldown_until.is_some_and(|c| c <= t) {
                        a.cooldown_until = None;
                    }
                    a.model_cooldowns.retain(|_, &mut c| c > t);
                    if a.capacity_until.is_some_and(|c| c <= t) {
                        a.capacity_until = None;
                    }
                    a.capacity_cooldowns.retain(|_, &mut c| c > t);
                }
                Err(e) => tracing::info!("pool: {}: {e}", a.name),
            }
            drop(p);
            self.touch();
        }
    }
}

/// Run the gateway on its own threads; returns once it is listening (or
/// couldn't), the error kept in the pool too.
pub fn start(shared: Arc<Shared>, port: u16) -> Result<(), String> {
    start_in(shared, port, crate::intercept::port(), olo_xdg::config_dir())
}

/// Run the gateway and clients' proxy using an explicit proxy port and config
/// directory (used by tests to isolate CAs/ports completely under `/var/tmp`).
pub fn start_in(shared: Arc<Shared>, port: u16, proxy_port: u16, config_dir: std::path::PathBuf) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("pool-gateway").enable_all().build().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    *shared.runtime.lock().unwrap() = Some(runtime.handle().clone());
    std::thread::Builder::new()
        .name("pool-gateway".into())
        .spawn(move || {
            runtime.block_on(async move {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                // (The one the version before handed over, if it did.)
                let listener = match crate::drain::listen(port).and_then(tokio::net::TcpListener::from_std) {
                    Ok(l) => {
                        if let Ok(local) = l.local_addr() {
                            shared.bound_port.store(local.port(), std::sync::atomic::Ordering::Relaxed);
                        }
                        shared.drain.keep(port, &l);
                        l
                    }
                    Err(e) => {
                        let msg = format!("Can’t listen on {addr}: {e}");
                        shared.pool.lock().unwrap().error = Some(msg.clone());
                        shared.touch();
                        shared.set_proxy_ready(ProxyReady { finished: true, proxy_ok: false, agy_ok: false, error: Some(msg.clone()), agy_error: None });
                        let _ = tx.send(Err(msg));
                        return;
                    }
                };
                // Report gateway listening immediately so the UI looper never
                // blocks on CA keygen/fsync; initialize the CA and clients'
                // proxy asynchronously right after on this runtime.
                let _ = tx.send(Ok(()));
                let s_proxy = shared.clone();
                let cfg_dir = config_dir.clone();
                tokio::spawn(async move {
                    let opened_ca = tokio::task::spawn_blocking(move || crate::authority::Authority::open_in(&cfg_dir)).await;
                    let opened_ca = match opened_ca {
                        Ok(res) => res,
                        Err(e) => Err(format!("CA task failed: {e}")),
                    };
                    match opened_ca {
                        Ok(a) => {
                            let agy_ok = a.has_agy();
                            let agy_error = a.agy_error.clone();
                            let s = s_proxy.clone();
                            let serving: crate::intercept::Serving = Arc::new(move |p| s.routes(p));
                            match crate::intercept::serve(s_proxy.clone(), Arc::new(a), proxy_port, serving, crate::intercept::Who::Clients).await {
                                Ok(_) => {
                                    s_proxy.set_proxy_ready(ProxyReady { finished: true, proxy_ok: true, agy_ok, error: None, agy_error });
                                    s_proxy.touch();
                                }
                                Err(e) => {
                                    tracing::warn!("pool: {e}");
                                    s_proxy.pool.lock().unwrap().error = Some(e.clone());
                                    s_proxy.set_proxy_ready(ProxyReady { finished: true, proxy_ok: false, agy_ok: false, error: Some(e), agy_error });
                                    s_proxy.touch();
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("The pool’s certificate: {e}");
                            tracing::warn!("pool: {msg}");
                            s_proxy.pool.lock().unwrap().error = Some(msg.clone());
                            s_proxy.set_proxy_ready(ProxyReady { finished: true, proxy_ok: false, agy_ok: false, error: Some(msg), agy_error: None });
                            s_proxy.touch();
                        }
                    }
                });
                let s = shared.clone();
                tokio::spawn(async move {
                    let mut stop = s.drain.watch();
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                        _ = stop.wait_for(|_| s.drain.retired()) => return,
                    }
                    loop {
                        if s.drain.retired() {
                            break;
                        }
                        s.sync(None).await;
                        if s.drain.retired() {
                            break;
                        }
                        s.borrow_ahead().await;
                        if s.drain.retired() {
                            break;
                        }
                        s.spend_expiring().await;
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                            _ = stop.wait_for(|_| s.drain.retired()) => break,
                        }
                    }
                });
                let mut listener = Some(listener);
                loop {
                    let Some(l) = listener.take().or_else(|| shared.drain.listener(port).and_then(|l| tokio::net::TcpListener::from_std(l).ok())) else { break };
                    // An upgrade waiting: no more connections (they wait
                    // for the new version), the open ones finishing.
                    let mut stop = shared.drain.watch();
                    let stopped = async move {
                        let _ = stop.wait_for(|s| *s).await;
                    };
                    if let Err(e) = axum::serve(l, router(shared.clone(), port)).with_graceful_shutdown(stopped).await {
                        shared.pool.lock().unwrap().error = Some(format!("The gateway stopped: {e}"));
                        shared.touch();
                        break;
                    }
                    // Stopped for an upgrade: the proxy's requests run on
                    // (on this runtime) until the new version takes over,
                    // or, if it never does, connections are taken again.
                    let mut stop = shared.drain.watch();
                    let _ = stop.wait_for(|s| !*s).await;
                }
                std::future::pending::<()>().await;
            })
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[derive(Clone)]
struct Ctx {
    shared: Arc<Shared>,
    port: u16,
}

pub fn router(shared: Arc<Shared>, port: u16) -> Router {
    Router::new()
        .route("/claude/v1/{*rest}", any(claude))
        .route("/codex/v1/{*rest}", any(codex))
        .route("/agy/{*rest}", any(agy))
        .route("/pool/status", get(status))
        .route("/pool/open", post(open))
        .route("/pool/capture", post(capture_now))
        .route("/pool/hosts", get(hosts))
        .route("/pool/hosts/{id}/{what}", post(host_act))
        .with_state(Ctx { shared, port })
}

/// Only clients on this computer, and never a web page: a browser sends
/// an Origin, and a DNS-rebinding page a Host that isn't ours.
fn local_only(headers: &HeaderMap, port: u16) -> Result<(), Box<Response>> {
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("");
    let ok_host = [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")].iter().any(|h| h == host);
    if !ok_host || headers.contains_key("origin") {
        return Err(Box::new((StatusCode::FORBIDDEN, "Only programs on this computer may use the pool.").into_response()));
    }
    Ok(())
}

async fn status(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    let mut v = status_json(&ctx.shared.snapshot());
    v["drain"] = ctx.shared.drain.status();
    v["capturing"] = json!(ctx.shared.capturing());
    v["pid"] = json!(std::process::id());
    axum::Json(v).into_response()
}

/// Open the app's window: Token Pool launched while the copy serving
/// takes no launches (it is starting, having just taken over, or another
/// took its launch socket), so the launch asks the gateway instead of
/// starting a copy beside it with no gateway of its own.
async fn open(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    match ctx.shared.opener.lock().unwrap().as_ref() {
        Some(open) => {
            open();
            StatusCode::NO_CONTENT.into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Capture every account's client again (`olo-pool --capture`).
async fn capture_now(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    ctx.shared.capture(true);
    StatusCode::ACCEPTED.into_response()
}

/// The pool for agents and the command line: no sign-ins.
pub fn status_json(p: &Pool) -> Value {
    let t = now();
    let accounts: Vec<Value> = p
        .saved
        .accounts
        .iter()
        .map(|a| {
            json!({
                "id": a.id, "name": a.name, "provider": a.provider, "enabled": a.enabled,
                "email": a.email, "plan": a.plan, "signed_out": a.signed_out,
                "reset_credits": a.credits.iter().map(|c| json!({"remaining": c.remaining, "usable": c.usable, "needs_limit": c.needs_limit, "unavailable_reason": c.unavailable_reason, "expires_at": c.expires_at})).collect::<Vec<_>>(),
                "blocked_until": schedule::blocked_until(a, "", t),
                // (Its allowance out, its provider answering from usage
                // credits until then: the last the pool turns to.)
                "paid_until": a.paid_until("", t),
                "model_paid": a.model_paid.iter().filter(|(_, p)| p.until > t).map(|(family, p)| (family.clone(), p.until)).collect::<std::collections::BTreeMap<_, _>>(),
                "windows": a.windows, "today": a.today.1, "total": a.total,
                "in_flight": p.in_flight.get(&a.id).copied().unwrap_or(0),
                "last_used": a.last_used, "synced": a.synced, "recent": a.recent,
            })
        })
        .collect();
    json!({"accounts": accounts, "error": p.error, "loaded": p.loaded, "served": p.served, "open": p.open, "activity": p.activity.minutes})
}

/// The pools on Rex hosts, for Rex's windows and agents: how each
/// stands (`hosts::Stands`), and `stamp`, which counts their changes.
/// `?after=N`: answered when the stamp is no longer N (a pool installed
/// from the Token Pool's own window, a sync that ended), or after five
/// minutes as things stand: one request held open for whoever shows
/// them, instead of asking again and again. (Rex's Host window asked
/// as it opened and never again: a pool installed from the Token Pool's
/// window stayed "Not installed" there. Ben, 2026-11-23: "even after i
/// enabeld it the rex settings still say not installed".)
async fn hosts(State(ctx): State<Ctx>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    let after = query.as_deref().and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("after="))).and_then(|n| n.parse::<u64>().ok());
    let mut seen = ctx.shared.hosts_seen();
    if after == Some(*seen.borrow_and_update()) {
        let _ = tokio::time::timeout(Duration::from_secs(300), seen.changed()).await;
    }
    let stamp = *seen.borrow();
    let mut list = match ctx.shared.hosting() {
        Some(hosting) => hosting.list(),
        None => json!({"hosts": []}),
    };
    list["stamp"] = json!(stamp);
    axum::Json(list).into_response()
}

/// One thing done to a host's pool: `install`, `remove`, `sync`,
/// `look`. Begun, and answered at once: how it goes shows in
/// `/pool/hosts`.
async fn host_act(State(ctx): State<Ctx>, Path((id, what)): Path<(String, String)>, headers: HeaderMap) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    match ctx.shared.hosting().map(|hosting| hosting.act(&id, &what)) {
        Some(Ok(())) => StatusCode::ACCEPTED.into_response(),
        Some(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A host's pool serves its clients through its proxy alone, which
/// knows the program that asks (`peer`: the account's own Claude Code
/// or Codex). By address anybody on the machine could spend the
/// accounts: a host has other people on it, a desktop its one person.
fn not_by_address() -> Response {
    (StatusCode::NOT_FOUND, "This Token Pool serves Claude Code, Codex and Antigravity through its proxy only.").into_response()
}

async fn claude(State(ctx): State<Ctx>, Path(rest): Path<String>, RawQuery(q): RawQuery, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    if ctx.shared.is_hosted() {
        return not_by_address();
    }
    proxy(ctx, Provider::Claude, rest, q, method, headers, body).await
}

async fn codex(State(ctx): State<Ctx>, Path(rest): Path<String>, RawQuery(q): RawQuery, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    if ctx.shared.is_hosted() {
        return not_by_address();
    }
    proxy(ctx, Provider::Codex, rest, q, method, headers, body).await
}

async fn agy(State(ctx): State<Ctx>, Path(rest): Path<String>, RawQuery(q): RawQuery, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    if ctx.shared.is_hosted() {
        return not_by_address();
    }
    // Enforce the same 4-operation allowlist as the transparent proxy so local
    // callers can never use `/agy` to submit auxiliary operations like
    // `claimBankedQuotaReset` outside the keeper gate.
    if !crate::intercept::is_agy_generation_path(&rest) {
        return (StatusCode::NOT_FOUND, "This operation isn’t served by the Token Pool gateway.").into_response();
    }
    if method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "Antigravity model generation requires POST.").into_response();
    }
    proxy(ctx, Provider::Antigravity, rest, q, method, headers, body).await
}

/// An HTTP client as the pool's are: a connection's own time to come,
/// and long enough for thinking before the first byte (a connection on a
/// network that went is cut at once).
/// Answers pass as they came, never decoded on the way (the pool reads a
/// decoded copy itself), and nothing asked for that the client didn't.
fn new_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(15 * 60))
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .expect("HTTP client")
}

/// Why a reset credit is claimed: a request of a model family no account
/// could serve, one about to expire, or the person asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    Blocked(String),
    Expiring,
    Asked,
}

/// A claim that didn't reset: refused (nothing spent), not made now (not
/// needed, not allowed, not kept first), no record (keeper explicitly confirmed
/// an `op_id` never ran), unreachable (pre-delivery transport/service failure),
/// or unclear (it may have gone: the same claim is reconciled first).
#[derive(Debug, Clone, PartialEq)]
pub enum Claimed {
    Refused(String),
    NotNow(String),
    /// Keeper explicitly confirmed a `reconcile_only` `op_id` has no record
    /// (did not spend a credit); safe for the borrower to clear pending state.
    NoRecord(String),
    /// Failed before delivery to the keeper (SSH refused/unreachable or service
    /// not running); safe to clear an unsent pre-persisted op on initial send,
    /// NEVER safe to clear an already-unclear pending op on reconcile.
    Unreachable(String),
    Unclear(String),
    /// The remote keeper is an older host (`version < 2`) that does not
    /// implement `Ask::Redeem`; the borrower may fall back to direct claiming.
    Unsupported(String),
}

impl std::fmt::Display for Claimed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Claimed::Refused(e) | Claimed::NotNow(e) | Claimed::NoRecord(e) | Claimed::Unreachable(e) | Claimed::Unclear(e) | Claimed::Unsupported(e) => f.write_str(e),
        }
    }
}

/// Outcome of attempting to free an account via reset credits on exhaustion.
enum RedeemOutcome {
    Ready(String),
    ResetWaits,
    None,
}

/// Outcome of a local kept-account reset claim pass.
enum KeptOutcome {
    Redeemed(RedeemedInfo),
    Ready(String),
    None,
}

struct RedeemedInfo {
    account: String,
    credit_id: String,
    reset_id: Option<String>,
    cleared_shared: bool,
    cleared_models: Vec<String>,
    cleared_windows: Vec<String>,
}

/// What a request for one account carries of its own (`Shared::own`).
struct Own {
    org: Option<String>,
    body: Bytes,
}

/// Request headers not passed on: the client's own credentials (the
/// account's go instead), and hop-by-hop ones.
fn forwarded(name: &str) -> bool {
    !matches!(
        name,
        "host"
            | "authorization"
            | "x-api-key"
            | "chatgpt-account-id"
            | "content-length"
            | "connection"
            | "transfer-encoding"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
            // (The account's own: `Shared::jar`.)
            | "cookie"
    ) && !name.starts_with("x-olo-pool-")
}

/// Response headers not passed back: the connection's own, and the
/// cookies the host set for the account (kept in its jar, never the
/// client's: they'd go back with its own sign-in).
fn returned(name: &str) -> bool {
    !matches!(name, "connection" | "transfer-encoding" | "keep-alive" | "set-cookie")
}

/// The host a provider's requests go to, whose cookies an account keeps.
/// A URL's path, as a cookie's is matched against: from its host on, to
/// its query.
fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    path.split(['?', '#']).next().unwrap_or("/")
}

fn host_of(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "api.anthropic.com",
        Provider::Codex => "chatgpt.com",
        Provider::Antigravity => "daily-cloudcode-pa.googleapis.com",
    }
}

/// `base`'s authority (`https://api.anthropic.com` → `api.anthropic.com`),
/// a request to it names in its `Host`.
fn authority_of(base: &str) -> &str {
    let rest = base.split_once("://").map_or(base, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

/// The answer when no account can serve, shaped as the client expects.
fn unavailable(provider: Provider, status: StatusCode, message: &str, retry: Option<i64>) -> Response {
    let body = match provider {
        Provider::Claude => {
            let kind = if status == StatusCode::TOO_MANY_REQUESTS { "rate_limit_error" } else { "overloaded_error" };
            json!({"type": "error", "error": {"type": kind, "message": message}})
        }
        Provider::Codex => {
            let kind = if status == StatusCode::TOO_MANY_REQUESTS { "usage_limit_reached" } else { "server_error" };
            json!({"error": {"type": kind, "message": message}})
        }
        Provider::Antigravity => {
            let gstatus = if status == StatusCode::TOO_MANY_REQUESTS { "RESOURCE_EXHAUSTED" } else { "UNAVAILABLE" };
            json!({"error": {"code": status.as_u16(), "message": message, "status": gstatus}})
        }
    };
    let mut r = (status, axum::Json(body)).into_response();
    if let Some(at) = retry
        && let Ok(v) = HeaderValue::from_str(&(at - now()).max(1).to_string())
    {
        r.headers_mut().insert("retry-after", v);
    }
    r
}

async fn proxy(ctx: Ctx, provider: Provider, rest: String, query: Option<String>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(r) = local_only(&headers, ctx.port) {
        return *r;
    }
    // Under way until its answer has gone (an upgrade waits for it), as a
    // request through the clients' proxy is (`intercept`).
    let underway = crate::drain::Underway::new(&ctx.shared);
    let (parts, body) = serve(ctx.shared, provider, rest, query, method, headers, Default::default(), body).await.into_parts();
    Response::from_parts(parts, axum::body::Body::new(crate::drain::Counted { inner: body, underway }))
}

/// The most a request body is taken to be, decompressed, for a look
/// inside (a model request is far smaller).
const PLAIN_MAX: u64 = 64 << 20;

/// Whether a request refused with `status` goes again with its body
/// decoded: Codex's, compressed, refused with a bare 400. Since
/// 2026-11-01 ChatGPT's backend answers some zstd-compressed requests
/// `{"detail":"Bad Request"}` (most often when the JSON is just past a
/// multiple of 128 KiB, zstd's block size), every time for the same
/// body, and the same JSON sent plain is served. A conversation whose
/// history sits just past such a size failed on every turn, whatever was
/// said (Ben's Codex, 2026-11-09: 395,917 and 396,878 bytes, past
/// 393,216). Once, so a request that's really wrong costs one more try.
fn resend_plain(provider: Provider, status: StatusCode, headers: &HeaderMap) -> bool {
    provider == Provider::Codex
        && status == StatusCode::BAD_REQUEST
        && headers.get("content-encoding").and_then(|v| v.to_str().ok()).is_some_and(|c| !matches!(c.trim().to_ascii_lowercase().as_str(), "" | "identity"))
}

/// A request body as sent, decompressed if its `content-encoding` says it
/// is (zstd, gzip, deflate); empty if it can't be.
fn plain<'a>(headers: &HeaderMap, body: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
    use std::io::Read;
    let coding = headers.get("content-encoding").and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_ascii_lowercase();
    let reader: Box<dyn Read + 'a> = match coding.as_str() {
        "" | "identity" => return std::borrow::Cow::Borrowed(body),
        "zstd" => match zstd::stream::read::Decoder::new(body) {
            Ok(d) => Box::new(d),
            Err(_) => return std::borrow::Cow::Owned(Vec::new()),
        },
        "gzip" | "x-gzip" => Box::new(flate2::read::GzDecoder::new(body)),
        "deflate" => Box::new(flate2::read::ZlibDecoder::new(body)),
        _ => return std::borrow::Cow::Owned(Vec::new()),
    };
    let mut out = Vec::new();
    match reader.take(PLAIN_MAX).read_to_end(&mut out) {
        Ok(_) => std::borrow::Cow::Owned(out),
        Err(_) => std::borrow::Cow::Owned(Vec::new()),
    }
}

enum ProjectRewrite {
    /// Body already carried `project`.
    AlreadyCorrect,
    /// Body rewritten to `project`; second field is true if compression was stripped.
    Rewritten(Bytes, bool),
    /// Request body could not be decoded or parsed as a JSON object.
    InvalidBody,
}

/// Decode and parse an Antigravity generation JSON request body once before
/// account selection so invalid bodies fail immediately with 400 (spending zero
/// resets) and valid bodies are not decoded repeatedly across retries.
fn parse_agy_request_body(headers: &HeaderMap, body: &Bytes) -> Option<(serde_json::Map<String, Value>, bool)> {
    let decoded_body = plain(headers, body);
    let Value::Object(obj) = serde_json::from_slice::<Value>(&decoded_body).ok()? else {
        return None;
    };
    let was_compressed = headers.get("content-encoding").and_then(|v| v.to_str().ok()).is_some_and(|c| !matches!(c.trim().to_ascii_lowercase().as_str(), "" | "identity"));
    Some((obj, was_compressed))
}

/// Rewrite top-level `"project"` in a pre-validated Antigravity generation JSON
/// request object to the chosen account's `cloudaicompanionProject`.
fn rewrite_agy_project_parsed(obj: &serde_json::Map<String, Value>, was_compressed: bool, project: &str) -> ProjectRewrite {
    if obj.get("project").and_then(Value::as_str) == Some(project) {
        return ProjectRewrite::AlreadyCorrect;
    }
    let mut updated = obj.clone();
    updated.insert("project".to_owned(), Value::String(project.to_owned()));
    let Ok(bytes) = serde_json::to_vec(&Value::Object(updated)) else {
        return ProjectRewrite::InvalidBody;
    };
    ProjectRewrite::Rewritten(Bytes::from(bytes), was_compressed)
}

/// Choose the authoritative matching window on `a` to mark exhausted on an
/// Antigravity 429 `QUOTA_EXHAUSTED` or `consumedCredits` response:
/// - Never falls back to a shared window when `model_only` is set (so a
///   model-scoped 429 never poisons an unrelated shared bucket).
/// - When multiple matching windows exist, only picks one when `until`
///   uniquely matches a single candidate's `resets_at` within 15 minutes
///   (never guessing the shortest bucket to authorize a blind auto-claim).
///
/// Returns `(chosen_window_index, scoped_only, had_multiple_candidates)`.
fn pick_narrowest_agy_window(a: &Account, family: &str, model_only: bool, until: Option<i64>, _t: i64) -> (Option<usize>, bool, bool) {
    let matching: Vec<usize> = a.windows.iter().enumerate().filter(|(_, w)| w.applies_to(family)).map(|(i, _)| i).collect();
    if matching.is_empty() {
        return (None, model_only && !family.is_empty(), false);
    }
    let scoped: Vec<usize> = matching.iter().copied().filter(|&i| !a.windows[i].is_shared()).collect();
    // When the 429 explicitly named a model (`model_only`) and no scoped bucket
    // exists for it yet, record a model refusal rather than poisoning a healthy
    // shared bucket.
    if model_only && scoped.is_empty() {
        return (None, !family.is_empty(), false);
    }
    let shared_near = matching.iter().copied().any(|i| a.windows[i].is_shared() && a.windows[i].used >= 95.0);
    // Prefer scoped buckets when `model_only` is set OR when scoped buckets
    // match and no shared bucket is already near exhaustion.
    let candidates = if !scoped.is_empty() && (model_only || !shared_near) { scoped } else { matching };
    let scoped_only = !family.is_empty() && candidates.iter().all(|&i| !a.windows[i].is_shared());
    if candidates.len() == 1 {
        return (Some(candidates[0]), scoped_only, false);
    }
    // Multiple matching buckets (e.g. 5-hour + weekly): only match when `until`
    // uniquely identifies a single bucket's reset deadline within 15 minutes;
    // otherwise return `None` so callers record a refusal and trigger sync
    // instead of guessing which bucket to auto-claim.
    let best = until.and_then(|u| {
        let mut close: Vec<usize> = candidates.iter().copied().filter(|&i| a.windows[i].resets_at.is_some_and(|r| (r - u).abs() <= 900)).collect();
        (close.len() == 1).then(|| close.remove(0))
    });
    (best, scoped_only || (best.is_none() && !family.is_empty()), true)
}

/// Inspect an Antigravity HTTP 429 body (up to 64 KiB wire AND 64 KiB decoded)
/// for quota/capacity exhaustion; if it is NOT an account/model rejection (or
/// exceeds the limit), return a complete pass-through [`Response`] preserving
/// every byte.
async fn inspect_agy_429(mut r: crate::relay::Answer, mut finish: Finish, family: &str, t: i64) -> Result<upstream::AgyRejection, Box<Response>> {
    const MAX: usize = 64 << 10;
    let status = r.status();
    let headers = r.headers().clone();
    let ext = r.take_extensions();
    let mut stream = r.into_stream();
    let mut prefix = bytes::BytesMut::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut overflow: Option<Option<Result<Bytes, std::io::Error>>> = None;
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(None) => break,
            Ok(Some(Ok(piece))) if piece.len() <= MAX - prefix.len() => {
                prefix.extend_from_slice(&piece);
                continue;
            }
            Ok(item) => {
                overflow = Some(item);
                break;
            }
            Err(_) => {
                overflow = Some(None);
                break;
            }
        }
    }
    if overflow.is_none() {
        let coding = headers.get("content-encoding").and_then(|v| v.to_str().ok()).unwrap_or("");
        let plain_bytes = decoded_bounded(coding, &prefix, MAX);
        if let Some(inspect_slice) = plain_bytes.as_deref()
            && let Some(rej) = upstream::agy_rejection(&headers, inspect_slice, family, t)
        {
            finish.outcome = Some(Outcome::RateLimited);
            drop(finish);
            return Ok(rej);
        }
    }
    // Not an account/model rejection (or too big/slow): pass through intact.
    let mut out = HeaderMap::new();
    let transfer_coded = headers.contains_key("transfer-encoding");
    for (k, v) in headers.iter().filter(|(k, _)| returned(k.as_str()) && !(transfer_coded && k.as_str() == "content-length")) {
        out.append(k.clone(), v.clone());
    }
    if let Ok(v) = HeaderValue::from_str(&finish.name) {
        out.insert(HeaderName::from_static("x-olo-pool-account"), v);
    }
    finish.outcome = None;
    let body = if let Some(tail) = overflow {
        let finish = Arc::new(Mutex::new(finish));
        let end = finish.clone();
        let combined = futures_util::stream::iter(std::iter::once(Ok(prefix.freeze())).chain(tail)).chain(stream).map(move |chunk| {
            if chunk.is_err() {
                finish.lock().unwrap().broken = true;
            }
            chunk
        });
        let combined = combined.chain(futures_util::stream::poll_fn(move |_| {
            end.lock().unwrap().ended = true;
            std::task::Poll::Ready(None)
        }));
        Body::from_stream(combined)
    } else {
        finish.ended = true;
        drop(finish);
        Body::from(prefix.freeze())
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    *resp.headers_mut() = out;
    *resp.extensions_mut() = ext;
    Err(Box::new(resp))
}

/// Serve a model request from the pool: `rest` is its path after the
/// provider's API base (`messages`, `responses`), from the gateway's own
/// address or the proxy (`intercept`).
#[allow(clippy::too_many_arguments)]
pub async fn serve(s: Arc<Shared>, provider: Provider, rest: String, query: Option<String>, method: Method, headers: HeaderMap, ext: axum::http::Extensions, body: Bytes) -> Response {
    serve_host(s, provider, None, rest, query, method, headers, ext, body).await
}

/// As [`serve`], preserving the intercepted `target_host` (e.g. distinguishing
/// `cloudcode-pa.googleapis.com` from `daily-cloudcode-pa.googleapis.com`).
#[allow(clippy::too_many_arguments)]
pub async fn serve_host(
    s: Arc<Shared>,
    provider: Provider,
    target_host: Option<String>,
    rest: String,
    query: Option<String>,
    method: Method,
    headers: HeaderMap,
    ext: axum::http::Extensions,
    body: Bytes,
) -> Response {
    // Validate & parse Antigravity generation JSON bodies ONCE before account
    // selection / automatic reset claims so malformed bodies return 400 with
    // zero resets spent, and reuse the parsed object for project substitution.
    let (model, agy_parsed) = if provider == Provider::Antigravity {
        let Some((obj, was_compressed)) = parse_agy_request_body(&headers, &body) else {
            return unavailable(provider, StatusCode::BAD_REQUEST, "The Antigravity request body could not be decoded for project routing.", None);
        };
        let model = obj.get("model").and_then(Value::as_str).map(str::to_owned).unwrap_or_default();
        (model, Some((obj, was_compressed)))
    } else {
        let model = serde_json::from_slice::<Value>(&plain(&headers, &body)).ok().and_then(|v| v["model"].as_str().map(str::to_owned)).unwrap_or_default();
        (model, None)
    };
    let family = model_family(&model);
    let cookie_host = target_host.as_deref().unwrap_or(host_of(provider));
    let url = {
        let base = match provider {
            Provider::Claude => format!("{}/v1/{rest}", s.up.claude),
            Provider::Codex => format!("{}/{rest}", s.up.codex),
            Provider::Antigravity => format!("{}/{}", s.up.agy_for_host(target_host.as_deref()), rest.trim_start_matches('/')),
        };
        match &query {
            Some(q) => format!("{base}?{q}"),
            None => base,
        }
    };
    let client = client_of(&headers);
    // The client's connection (the pool's upstream follow it), and whether
    // the request came through the proxy (to the provider's own host, its
    // `Host` the host's) or to the gateway's own address.
    let conn = ext.get::<crate::relay::ClientConn>().copied().unwrap_or_default();
    let proxied = ext.get::<crate::relay::ClientOffer>().is_some();
    let mut tried: HashMap<String, u64> = HashMap::new();
    // Tries the network failed (not counted against an account).
    let mut network_tries = 0;
    let mut refresh_error: Option<String> = None;
    // No account has allowance left and no reset could be claimed: the
    // request goes to one its provider answers from usage credits, the
    // last the pool turns to (never before another account or a reset).
    let mut on_credits = false;
    let mut reset_retries = 0u8;
    loop {
        // Choose, and count it as busy at once.
        let chosen = {
            let mut p = s.pool.lock().unwrap();
            let t = now();
            let sticky = p.sticky.get(&(provider, family.clone())).cloned();
            let pick =
                if on_credits { p.pick_paid(provider, &family, sticky.as_deref(), &tried) } else { p.pick(provider, &family, sticky.as_deref(), &tried, &[]) }.map(|a| (a.id.clone(), a.name.clone()));
            match pick {
                Some((id, name)) => {
                    p.sticky.insert((provider, family.clone()), id.clone());
                    *p.in_flight.entry(id.clone()).or_default() += 1;
                    p.next_open += 1;
                    let open = p.next_open;
                    let o = Open { id: open, account: name.clone(), account_id: id.clone(), provider, model: model.clone(), client: client.clone(), started: now_ms() };
                    p.open.push(o);
                    let revision = p.reset_revision(&id);
                    Ok((id, name, open, revision))
                }
                None => {
                    if !p.loaded {
                        let msg = format!("The pool’s accounts aren’t open yet: {}", p.error.clone().unwrap_or_default());
                        return unavailable(provider, StatusCode::SERVICE_UNAVAILABLE, &msg, None);
                    }
                    let any = p.saved.accounts.iter().any(|a| a.provider == provider && a.enabled && a.signed_out.is_none());
                    let retry = schedule::next_available(&p.saved.accounts, provider, &family, t);
                    Err((any, retry))
                }
            }
        };
        let (id, name, open, mut revision) = match chosen {
            Ok(c) => {
                // (Its use, bookkeeping: saved with the next, or soon.)
                s.noted();
                c
            }
            Err((any, retry)) => {
                // Every account out: a reset credit keeps things going if an
                // account with a working sign-in can redeem one (bounded so a
                // misbehaving upstream or stale peer can never loop).
                let mut reset_waits = false;
                if any && !on_credits && reset_retries < 4 {
                    match s.redeem_for(provider, &family, &tried).await {
                        RedeemOutcome::Ready(id) => {
                            reset_retries += 1;
                            tried.remove(&id);
                            continue;
                        }
                        RedeemOutcome::ResetWaits => reset_waits = true,
                        RedeemOutcome::None => {}
                    }
                }
                // Nothing with allowance, no reset to claim: an account
                // its provider answers from usage credits, if there is
                // one (the next turn of the loop finds it, or none).
                // Usage credits are held back only when a reset waits on an
                // unreachable/unclear keeper or an unresolved pending claim.
                if any && !on_credits && !reset_waits {
                    let waiting = s.pool.lock().unwrap().pick_paid(provider, &family, None, &tried).map(|a| a.name.clone());
                    if let Some(name) = waiting {
                        tracing::info!("pool: no {} account has allowance left for {family} and no reset could be claimed: {name} answers from its usage credits", provider.client());
                        on_credits = true;
                        continue;
                    }
                }
                // If unblocked accounts failed sign-in renewal (e.g., keeper
                // couldn't lend/renew), surface the actual sign-in error as 502
                // instead of claiming every account is out of quota (429).
                if let Some(err) = refresh_error {
                    return unavailable(provider, StatusCode::BAD_GATEWAY, &err, None);
                }
                return if any {
                    let msg = if reset_waits {
                        format!("Every {} account in the pool is out of quota for now. A reset credit waits on the pool that keeps its sign-in.", provider.client())
                    } else {
                        format!("Every {} account in the pool is out of quota for now.", provider.client())
                    };
                    unavailable(provider, StatusCode::TOO_MANY_REQUESTS, &msg, retry)
                } else {
                    let msg = format!("There are no {} accounts in the pool to use (open Token Pool to add one).", provider.client());
                    unavailable(provider, StatusCode::SERVICE_UNAVAILABLE, &msg, None)
                };
            }
        };
        // Records how this try ends, whichever way, when dropped.
        let mut finish = Finish {
            id: id.clone(),
            name,
            open,
            provider,
            revision,
            dispatched_at: now_ms(),
            model: model.clone(),
            client: client.clone(),
            started: std::time::Instant::now(),
            status: 0,
            outcome: Some(Outcome::Failed),
            ended: false,
            length: None,
            passed: 0,
            broken: false,
            scan: Scan::new(),
            decode: Decode::Plain,
            shared: Some(s.clone()),
        };
        let seen = olo_net::generation();
        let mut cred = match s.fresh(&id, provider, None, 60).await {
            Ok(c) => c,
            // (The network, not the account: the same one again, once
            // there's a network.)
            Err(Failure::Other(e)) if network_tries < NETWORK_TRIES && network_failed(seen) => {
                network_tries += 1;
                drop(finish);
                tracing::info!("pool: the network failed renewing a sign-in ({e}): again");
                network_back().await;
                continue;
            }
            Err(e) => {
                if let Failure::Other(why) = e {
                    refresh_error.get_or_insert(why);
                }
                drop(finish);
                tried.insert(id.clone(), revision);
                continue;
            }
        };
        // For Antigravity, discover `cloudaicompanionProject` if not yet cached
        // (with bounded 60 s failure cooldown on request paths so concurrent
        // turns don't stampede `loadCodeAssist`). Fail closed and try the next
        // account if this account's project cannot be established.
        let mut own = s.own(&id, provider, &headers, &body);
        let mut agy_stripped_encoding = false;
        let mut refreshed = false;
        if provider == Provider::Antigravity {
            let unauth = s.ensure_agy_project(&id, &mut cred, s.like(&id).as_ref(), 60).await;
            if unauth {
                refreshed = true;
                if let Ok(mut c) = s.fresh(&id, provider, Some(&cred), 0).await {
                    let _ = s.ensure_agy_project(&id, &mut c, s.like(&id).as_ref(), 0).await;
                    cred = c;
                }
            }
            let Some(proj) = cred.account_id.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned) else {
                refresh_error.get_or_insert_with(|| "This Antigravity account's project could not be discovered.".into());
                drop(finish);
                tried.insert(id.clone(), revision);
                continue;
            };
            let (ref agy_obj, agy_was_compressed) = *agy_parsed.as_ref().expect("validated before loop");
            match rewrite_agy_project_parsed(agy_obj, agy_was_compressed, &proj) {
                ProjectRewrite::AlreadyCorrect => {}
                ProjectRewrite::Rewritten(rewritten, stripped) => {
                    own.body = rewritten;
                    agy_stripped_encoding = stripped;
                }
                ProjectRewrite::InvalidBody => {
                    drop(finish);
                    return unavailable(provider, StatusCode::BAD_REQUEST, "The Antigravity request body could not be decoded for project routing.", None);
                }
            }
        }
        // The body as decoded, once sent so (`resend_plain`).
        let mut sent_plain: Option<Bytes> = None;
        let response = loop {
            let seen = olo_net::generation();
            revision = s.pool.lock().unwrap().reset_revision(&id);
            finish.revision = revision;
            finish.dispatched_at = now_ms();
            // The client's request as it came, in its order, the account's
            // own in place of the client's: its sign-in where the client's
            // was, its ChatGPT account and Claude organization, the OAuth
            // beta in the client's list (Claude Code signed in to it sends
            // them so). Claude's goes exactly so, each name in its case, on
            // a connection of the account's own (`relay`).
            let response_jar = s.jar(&id);
            let sent = if provider == Provider::Claude {
                let target = match &query {
                    Some(q) => format!("/v1/{rest}?{q}"),
                    None => format!("/v1/{rest}"),
                };
                let mut b = hyper::Request::builder().method(method.clone()).uri(target);
                let mut signed = false;
                let cookie = crate::likeness::cookie(&response_jar, host_of(provider), path_of(&url));
                for (k, v) in headers.iter() {
                    match k.as_str() {
                        "authorization" | "x-api-key" if !signed => {
                            signed = true;
                            b = b.header("authorization", format!("Bearer {}", cred.access));
                        }
                        "authorization" | "x-api-key" => {}
                        "x-organization-uuid" | "anthropic-organization-id" => {
                            if let Some(o) = &own.org {
                                b = b.header(k, o.as_str());
                            }
                        }
                        "anthropic-beta" => b = b.header(k, upstream::with_oauth_beta(v.to_str().ok())),
                        // (To the gateway's own address: the host's.)
                        "host" if !proxied => b = b.header(k, authority_of(&s.up.claude)),
                        // (The body's, which may have the account's id.)
                        "content-length" => b = b.header(k, own.body.len()),
                        // (The account's cookies, where the client's were.)
                        "cookie" => {
                            if let Some(c) = &cookie {
                                b = b.header(k, c.as_str());
                            }
                        }
                        // (The connection's own.)
                        "proxy-connection" | "proxy-authorization" | "te" | "trailer" | "upgrade" => {}
                        name if name.starts_with("x-olo-pool-") => {}
                        _ => b = b.header(k, v),
                    }
                }
                if !signed {
                    b = b.header("authorization", format!("Bearer {}", cred.access));
                }
                if !headers.contains_key("anthropic-beta") {
                    b = b.header("anthropic-beta", upstream::with_oauth_beta(None));
                }
                if !headers.contains_key("cookie")
                    && let Some(c) = &cookie
                {
                    b = b.header("cookie", c.as_str());
                }
                match b.body(http_body_util::Full::new(own.body.clone())) {
                    Ok(mut req) => {
                        *req.extensions_mut() = ext.clone();
                        // (The client's own offer; a request from the
                        // gateway's own address, Claude Code's SDK's.)
                        let alpn: Vec<&str> = match ext.get::<crate::relay::ClientOffer>() {
                            Some(o) => o.0.iter().map(String::as_str).collect(),
                            None => vec!["http/1.1"],
                        };
                        s.exact.send(&id, conn, &s.up.claude, &alpn, req).await.map_err(Sent::from)
                    }
                    Err(e) => Err(Sent::Other(e.to_string())),
                }
            } else {
                let mut req = s.client_for(&id).request(method.clone(), &url);
                let mut signed = false;
                for (k, v) in headers.iter() {
                    match k.as_str() {
                        "authorization" | "x-api-key" if !signed => {
                            signed = true;
                            req = req.header("authorization", format!("Bearer {}", cred.access));
                            // (Codex names its account right after: where it
                            // names none, the account's goes there.)
                            if provider == Provider::Codex
                                && !headers.contains_key("chatgpt-account-id")
                                && let Some(account) = &cred.account_id
                            {
                                req = req.header("chatgpt-account-id", account.as_str());
                            }
                        }
                        "authorization" | "x-api-key" => {}
                        "chatgpt-account-id" => {
                            if provider == Provider::Codex
                                && let Some(account) = &cred.account_id
                            {
                                req = req.header(k, account.as_str());
                            }
                        }
                        "content-encoding" if sent_plain.is_some() || agy_stripped_encoding => {}
                        name if forwarded(name) => req = req.header(k, v),
                        _ => {}
                    }
                }
                if !signed {
                    req = upstream::authorize(req, provider, &cred);
                }
                // (The account's cookies, as Codex's store adds its own.)
                if let Some(c) = crate::likeness::cookie(&response_jar, cookie_host, path_of(&url)) {
                    req = req.header("cookie", c);
                }
                req.body(sent_plain.clone().unwrap_or_else(|| own.body.clone())).send().await.map(crate::relay::Answer::Reqwest).map_err(Sent::from)
            };
            if let Ok(r) = &sent {
                crate::likeness::keep(&response_jar, cookie_host, path_of(&url), r.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()));
            }
            match sent {
                Ok(r) if sent_plain.is_none() && resend_plain(provider, r.status(), &headers) => {
                    let plain = plain(&headers, &own.body);
                    if plain.is_empty() {
                        break Some(r);
                    }
                    tracing::info!("pool: ChatGPT refused a compressed Codex request ({} bytes decoded); sending it again uncompressed", plain.len());
                    sent_plain = Some(Bytes::from(plain.into_owned()));
                }
                Ok(r) if r.status() == StatusCode::UNAUTHORIZED && !refreshed => {
                    refreshed = true;
                    match s.fresh(&id, provider, Some(&cred), 0).await {
                        Ok(mut c) => {
                            if provider == Provider::Antigravity {
                                if c.account_id.as_deref().map(str::trim).is_none_or(str::is_empty) {
                                    s.ensure_agy_project(&id, &mut c, s.like(&id).as_ref(), 0).await;
                                }
                                let Some(proj) = c.account_id.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned) else {
                                    break None;
                                };
                                let (ref agy_obj, agy_was_compressed) = *agy_parsed.as_ref().expect("validated before loop");
                                match rewrite_agy_project_parsed(agy_obj, agy_was_compressed, &proj) {
                                    ProjectRewrite::AlreadyCorrect => {
                                        own.body = body.clone();
                                        agy_stripped_encoding = false;
                                    }
                                    ProjectRewrite::Rewritten(rewritten, stripped) => {
                                        own.body = rewritten;
                                        agy_stripped_encoding = stripped;
                                    }
                                    ProjectRewrite::InvalidBody => break None,
                                }
                            }
                            cred = c;
                        }
                        Err(e) => {
                            if let Failure::Other(why) = e {
                                refresh_error.get_or_insert(why);
                            }
                            break None;
                        }
                    }
                }
                Ok(r) => break Some(r),
                // Never reached the host: again, once there's a network.
                Err(e) if network_tries < NETWORK_TRIES && e.is_connect() => {
                    network_tries += 1;
                    tracing::info!("pool: a request to {} couldn’t go ({e}): again", provider.client());
                    if network_failed(seen) {
                        s.exact.clear();
                    }
                    network_back().await;
                }
                // It may have reached the host (sent, then its connection
                // cut, the network changing under it): never sent twice by
                // the pool, which the host would see as no client sends it
                // (Codex's review reproduced the double). Answered once
                // there's a network again, so the client's own retry
                // (Claude Code's SDK, Codex's) asks again on it, as a
                // client whose own connection dropped does.
                Err(e) => {
                    if network_failed(seen) {
                        s.exact.clear();
                        network_back().await;
                    }
                    drop(finish);
                    return unavailable(provider, StatusCode::BAD_GATEWAY, &format!("The pool couldn’t reach {}: {e}", provider.client()), None);
                }
            }
        };
        let Some(r) = response else {
            finish.status = StatusCode::UNAUTHORIZED.as_u16();
            drop(finish);
            tried.insert(id.clone(), revision);
            continue;
        };
        let status = r.status();
        finish.status = status.as_u16();
        let t = now();
        // Antigravity carries 429 `RESOURCE_EXHAUSTED` quota/capacity details
        // in the JSON response body rather than HTTP headers.
        if provider == Provider::Antigravity && status == StatusCode::TOO_MANY_REQUESTS {
            let dispatched_at = finish.dispatched_at;
            match inspect_agy_429(r, finish, &family, t).await {
                Ok(rej) => {
                    let target_family = if rej.model_family.is_empty() { family.clone() } else { rej.model_family.clone() };
                    let mut late_windows = Vec::new();
                    let mut need_summary_sync = false;
                    let mut late_model_only = rej.model_only && !target_family.is_empty();
                    {
                        let mut p = s.pool.lock().unwrap();
                        if p.reset_revision(&id) == revision
                            && let Some(a) = p.account_mut(&id)
                        {
                            if rej.capacity_only {
                                // Transient capacity / rate-limit pressure: store in separate
                                // capacity fields so quota resets never discard active capacity
                                // backoff, and never mark quota windows 100% spent.
                                if rej.model_only && !target_family.is_empty() {
                                    a.capacity_cooldowns.insert(target_family.clone(), rej.until);
                                } else {
                                    a.capacity_until = Some(rej.until);
                                }
                            } else {
                                let (chosen_idx, scoped_only, multi) = pick_narrowest_agy_window(a, &target_family, rej.model_only, Some(rej.until), t);
                                need_summary_sync = multi || chosen_idx.is_none();
                                late_model_only = (scoped_only || rej.model_only) && !target_family.is_empty();
                                if let Some(idx) = chosen_idx {
                                    let w = &mut a.windows[idx];
                                    w.used = w.used.max(100.0);
                                    if w.resets_at.is_none() || w.resets_at.is_some_and(|r| (r - rej.until).abs() <= 900) {
                                        w.resets_at = Some(rej.until);
                                    }
                                    late_windows.push(w.clone());
                                } else {
                                    let synth = Window {
                                        name: if late_model_only { format!("model:{target_family}") } else { "quota".to_owned() },
                                        label: if late_model_only { target_family.clone() } else { "Quota".to_owned() },
                                        used: 100.0,
                                        resets_at: Some(rej.until),
                                        models: if late_model_only { vec![target_family.clone()] } else { Vec::new() },
                                    };
                                    late_windows.push(synth.clone());
                                    a.merge_windows(vec![synth]);
                                }
                                if late_model_only {
                                    a.model_cooldowns.insert(target_family.clone(), rej.until);
                                } else {
                                    a.cooldown_until = Some(rej.until);
                                }
                            }
                        }
                    }
                    if need_summary_sync {
                        s.sync_soon(Some(id.clone()));
                    }
                    if s.drain.retired() && !rej.capacity_only {
                        let (cooldown, model_cooldown) = if late_model_only { (None, Some((target_family, rej.until))) } else { (Some(rej.until), None) };
                        s.tell_late(&Late::Seen { id: id.clone(), at: dispatched_at, windows: late_windows, cooldown, model_cooldown, paid: None });
                    }
                    s.noted();
                    tried.insert(id.clone(), revision);
                    continue;
                }
                Err(pass_through) => return *pass_through,
            }
        }
        let (mut windows, rejection) = upstream::header_windows(r.headers(), provider, &family, status.as_u16(), t);
        // An answer given although the account's allowance is out: paid
        // for from its usage credits. It goes back (it is answered, and
        // paid for), and the account is left for another from the next
        // request on, or for a reset that clears its meter.
        let paid = upstream::paid_by_credits(r.headers(), provider, &family, status.as_u16(), t, &mut windows);
        // (Handed over: what it says told the copy at work too.)
        if s.drain.retired() && (!windows.is_empty() || rejection.is_some()) {
            let (cooldown, model_cooldown) = match &rejection {
                Some(rej) if rej.model_only => (None, Some((family.clone(), rej.until))),
                Some(rej) => (Some(rej.until), None),
                None => (None, None),
            };
            let paid = paid.as_ref().map(|p| (family.clone(), p.model_only, p.until, p.meter.clone()));
            s.tell_late(&Late::Seen { id: id.clone(), at: finish.dispatched_at, windows: windows.clone(), cooldown, model_cooldown, paid });
        }
        {
            let mut p = s.pool.lock().unwrap();
            let current = p.reset_revision(&id) == revision;
            if let Some(a) = p.account_mut(&id) {
                if current {
                    a.merge_windows(windows);
                }
                if status == StatusCode::UNAUTHORIZED {
                    a.signed_out = Some("Its provider refused the sign-in.".into());
                }
                if current && let Some(rej) = &rejection {
                    if rej.model_only {
                        a.model_cooldowns.insert(family.clone(), rej.until);
                    } else {
                        a.cooldown_until = Some(rej.until);
                    }
                }
                if current && let Some(paid) = &paid {
                    // (Said once as it begins, not at each answer of an
                    // account the pool went to knowing so.)
                    if a.paid_until(&family, t).is_none() {
                        tracing::info!("pool: {} answered a {family} request from its usage credits (its {} is used up): left for another account or a reset from here on", a.name, paid.meter);
                    }
                    a.now_paid(&family, paid.model_only, paid.until, &paid.meter);
                }
                // (An answer within its allowance, by Claude's own word:
                // what said it was answered from usage credits no longer
                // holds. A reset made elsewhere, seen at its first use.)
                let within = r.headers().get("anthropic-ratelimit-unified-status").and_then(|v| v.to_str().ok()).is_some_and(|s| matches!(s.trim(), "allowed" | "allowed_warning"));
                if current && paid.is_none() && status.is_success() && within {
                    a.paid = None;
                    a.model_paid.remove(&family);
                }
            }
        }
        if status == StatusCode::UNAUTHORIZED || rejection.is_some() {
            // Nothing passed on yet: the next account may serve it.
            if rejection.is_some() {
                finish.outcome = Some(Outcome::RateLimited);
            }
            drop(finish);
            tried.insert(id.clone(), revision);
            continue;
        }
        // Passed on: how it ends is up to the answer.
        finish.outcome = None;
        return respond(r, finish, &method);
    }
}

/// A Codex socket: its Responses over a WebSocket, as Codex 0.170 asks
/// for them first (Ben, 2026-11-19: "all requests from the pool need to be
/// exactly the same as if they came from claude code / codex"; answered
/// 426, Codex spoke HTTP for the rest of its session, which no Codex left
/// to itself does). The handshake goes on as Codex sent it, the account's
/// own in place of Codex's (its sign-in, its ChatGPT account, its
/// cookies), on a connection of its own offering what Codex offered; one
/// refused there (a 429, a sign-in refused) gives way to the next account
/// before Codex hears a thing. Switched, the socket is passed on both ways
/// as it came, the server's side read as it passes (`socket::Reader`):
/// each turn counted from its `response.created` to its end, with its
/// usage; an account that runs out inside the socket (an `error` event
/// with status 429, which Codex takes as final) is marked so, and the
/// socket closed instead of that being said: Codex takes it as a stream
/// that broke and asks again, which the pool gives another account.
pub async fn serve_socket(s: Arc<Shared>, path: String, query: Option<String>, headers: HeaderMap, ext: axum::http::Extensions, on: hyper::upgrade::OnUpgrade) -> Response {
    let provider = Provider::Codex;
    let family = headers.get("x-codex-routing-hint").and_then(|h| h.to_str().ok()).and_then(|hint| hint.split(';').find_map(|p| p.trim().strip_prefix("model="))).map(model_family).unwrap_or_default();
    let client = client_of(&headers);
    let target = match &query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    let base = s.upstream_origin(host_of(provider));
    let offer: Vec<String> = ext.get::<crate::relay::ClientOffer>().map(|o| o.0.clone()).unwrap_or_default();
    let offer: Vec<&str> = offer.iter().map(String::as_str).collect();
    let mut tried: HashMap<String, u64> = HashMap::new();
    let mut network_tries = 0;
    let mut reset_retries = 0u8;
    let mut refresh_error: Option<String> = None;
    loop {
        let chosen = {
            let mut p = s.pool.lock().unwrap();
            let sticky = p.sticky.get(&(provider, family.clone())).cloned();
            match p.pick(provider, &family, sticky.as_deref(), &tried, &[]).map(|a| (a.id.clone(), a.name.clone())) {
                Some((id, name)) => {
                    p.sticky.insert((provider, family.clone()), id.clone());
                    let revision = p.reset_revision(&id);
                    Ok((id, name, revision))
                }
                None => {
                    if !p.loaded {
                        let msg = format!("The pool’s accounts aren’t open yet: {}", p.error.clone().unwrap_or_default());
                        return unavailable(provider, StatusCode::SERVICE_UNAVAILABLE, &msg, None);
                    }
                    let any = p.saved.accounts.iter().any(|a| a.provider == provider && a.enabled && a.signed_out.is_none());
                    Err((any, schedule::next_available(&p.saved.accounts, provider, &family, now())))
                }
            }
        };
        let (id, name, mut revision) = match chosen {
            Ok(c) => c,
            Err((any, retry)) => {
                if any
                    && reset_retries < 4
                    && let RedeemOutcome::Ready(id) = s.redeem_for(provider, &family, &tried).await
                {
                    reset_retries += 1;
                    tried.remove(&id);
                    continue;
                }
                if let Some(err) = refresh_error {
                    return unavailable(provider, StatusCode::BAD_GATEWAY, &err, None);
                }
                return if any {
                    let msg = format!("Every {} account in the pool is out of quota for now.", provider.client());
                    unavailable(provider, StatusCode::TOO_MANY_REQUESTS, &msg, retry)
                } else {
                    let msg = format!("There are no {} accounts in the pool to use (open Token Pool to add one).", provider.client());
                    unavailable(provider, StatusCode::SERVICE_UNAVAILABLE, &msg, None)
                };
            }
        };
        let seen = olo_net::generation();
        let mut cred = match s.fresh(&id, provider, None, 60).await {
            Ok(c) => c,
            Err(Failure::Other(_)) if network_tries < NETWORK_TRIES && network_failed(seen) => {
                network_tries += 1;
                network_back().await;
                continue;
            }
            Err(e) => {
                if let Failure::Other(why) = e {
                    refresh_error.get_or_insert(why);
                }
                tried.insert(id, revision);
                continue;
            }
        };
        let jar = s.jar(&id);
        let mut refreshed = false;
        let answer = loop {
            revision = s.pool.lock().unwrap().reset_revision(&id);
            let req = match socket_request(&headers, &target, &cred, &jar, &ext) {
                Ok(r) => r,
                Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
            };
            match crate::relay::upgrade(&base, &offer, req).await {
                Ok(r) => {
                    crate::likeness::keep(&jar, host_of(provider), target.split('?').next().unwrap_or(&target), r.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()));
                    if r.status() == StatusCode::UNAUTHORIZED && !refreshed {
                        refreshed = true;
                        match s.fresh(&id, provider, Some(&cred), 0).await {
                            Ok(c) => {
                                cred = c;
                                continue;
                            }
                            Err(e) => {
                                if let Failure::Other(why) = e {
                                    refresh_error.get_or_insert(why);
                                }
                                break None;
                            }
                        }
                    }
                    break Some(r);
                }
                // (Never reached the host: again, once there's a network.)
                Err(e @ crate::relay::SendError::Connect(_)) if network_tries < NETWORK_TRIES => {
                    network_tries += 1;
                    tracing::info!("pool: a Codex socket couldn’t open ({e}): again");
                    network_back().await;
                }
                Err(e) => return unavailable(provider, StatusCode::BAD_GATEWAY, &format!("The pool couldn’t reach {}: {e}", provider.client()), None),
            }
        };
        let Some(mut r) = answer else {
            tried.insert(id, revision);
            continue;
        };
        let status = r.status();
        let (windows, rejection) = upstream::header_windows(r.headers(), provider, &family, status.as_u16(), now());
        if status == StatusCode::TOO_MANY_REQUESTS {
            let reset = match handshake_quota(passed_back(r)).await {
                Ok(reset) => reset,
                Err(response) => return response,
            };
            {
                let mut p = s.pool.lock().unwrap();
                if p.reset_revision(&id) == revision
                    && let Some(a) = p.account_mut(&id)
                {
                    a.merge_windows(windows);
                    let until = reset.or_else(|| rejection.map(|r| r.until));
                    a.cooldown_until = a.cooldown_until.max(until);
                }
            }
            s.noted();
            tried.insert(id, revision);
            continue;
        }
        {
            let mut p = s.pool.lock().unwrap();
            let current = p.reset_revision(&id) == revision;
            let same_signin = p.saved.credentials.get(&id).is_some_and(|current| current.access == cred.access);
            if status == StatusCode::UNAUTHORIZED && !same_signin {
                continue;
            }
            if let Some(a) = p.account_mut(&id) {
                if current && same_signin {
                    a.merge_windows(windows);
                }
                if status == StatusCode::UNAUTHORIZED && same_signin {
                    a.signed_out = Some("Its provider refused the sign-in.".into());
                }
            }
        }
        if status == StatusCode::UNAUTHORIZED {
            s.noted();
            tried.insert(id, revision);
            continue;
        }
        // Not switched: its answer, as it came.
        if status != StatusCode::SWITCHING_PROTOCOLS {
            return passed_back(r);
        }
        let extensions = r.headers().get("sec-websocket-extensions").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let host = hyper::upgrade::on(&mut r);
        let mut out = Response::new(Body::empty());
        *out.status_mut() = status;
        for (k, v) in r.headers().iter().filter(|(k, _)| k.as_str() != "set-cookie") {
            out.headers_mut().append(k.clone(), v.clone());
        }
        // (Each name in its case, the status line's reason.)
        *out.extensions_mut() = std::mem::take(r.extensions_mut());
        let switching = crate::drain::Underway::new(&s);
        tokio::spawn(relay_socket(s.clone(), Turns { id, name, client, revision, jar, family }, on, host, extensions, switching));
        return out;
    }
}

/// Codex's handshake as it sent it, in its order, the account's own in
/// place of Codex's: its sign-in where Codex's was, its ChatGPT account
/// (after its sign-in where Codex named none), its cookies where Codex's
/// were (or at the end).
fn socket_request(headers: &HeaderMap, target: &str, cred: &Credential, jar: &crate::likeness::Jar, ext: &axum::http::Extensions) -> Result<hyper::Request<http_body_util::Full<Bytes>>, String> {
    let cookie = crate::likeness::cookie(jar, host_of(Provider::Codex), target.split('?').next().unwrap_or(target));
    let mut b = hyper::Request::builder().method(Method::GET).uri(target);
    let mut signed = false;
    for (k, v) in headers.iter() {
        match k.as_str() {
            "authorization" | "x-api-key" if !signed => {
                signed = true;
                b = b.header("authorization", format!("Bearer {}", cred.access));
                if !headers.contains_key("chatgpt-account-id")
                    && let Some(account) = &cred.account_id
                {
                    b = b.header("chatgpt-account-id", account.as_str());
                }
            }
            "authorization" | "x-api-key" => {}
            "chatgpt-account-id" => {
                if let Some(account) = &cred.account_id {
                    b = b.header(k, account.as_str());
                }
            }
            "cookie" => {
                if let Some(c) = &cookie {
                    b = b.header(k, c.as_str());
                }
            }
            "proxy-connection" | "proxy-authorization" => {}
            name if name.starts_with("x-olo-pool-") => {}
            _ => b = b.header(k, v),
        }
    }
    if !signed {
        b = b.header("authorization", format!("Bearer {}", cred.access));
        if !headers.contains_key("chatgpt-account-id")
            && let Some(account) = &cred.account_id
        {
            b = b.header("chatgpt-account-id", account.as_str());
        }
    }
    if !headers.contains_key("cookie")
        && let Some(c) = &cookie
    {
        b = b.header("cookie", c.as_str());
    }
    let mut req = b.body(http_body_util::Full::new(Bytes::new())).map_err(|e| e.to_string())?;
    *req.extensions_mut() = ext.clone();
    Ok(req)
}

/// An answer that didn't switch protocols, passed back as it came (each
/// name in its case, its reason, its length unless chunked), but for the
/// cookies set for the account.
fn passed_back(mut r: hyper::Response<hyper::body::Incoming>) -> Response {
    let ext = std::mem::take(r.extensions_mut());
    let (status, headers) = (r.status(), r.headers().clone());
    let chunked = headers.contains_key("transfer-encoding");
    let mut out = Response::new(Body::from_stream(crate::relay::Answer::Exact(r, None).into_stream()));
    *out.status_mut() = status;
    *out.extensions_mut() = ext;
    for (k, v) in headers.iter().filter(|(k, _)| returned(k.as_str()) && !(chunked && k.as_str() == "content-length")) {
        out.headers_mut().append(k.clone(), v.clone());
    }
    out
}

/// Inspect a small, plain JSON refusal. When it is a different error or
/// inspection runs out of time or space, preserve its complete body.
#[allow(clippy::result_large_err)]
async fn handshake_quota(response: Response) -> Result<Option<i64>, Response> {
    const MAX: usize = 64 << 10;
    let headers = response.headers();
    if headers.contains_key("content-encoding") || headers.get("x-codex-active-limit").is_some_and(|h| h != "codex") || headers.contains_key("x-codex-rate-limit-reached-type") {
        return Err(response);
    }
    let (parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    let mut prefix = bytes::BytesMut::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let tail = match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(None) => break,
            Ok(Some(Ok(piece))) if piece.len() <= MAX - prefix.len() => {
                prefix.extend_from_slice(&piece);
                continue;
            }
            Ok(item) => item,
            Err(_) => None,
        };
        let body = futures_util::stream::iter(std::iter::once(Ok(prefix.freeze())).chain(tail)).chain(stream);
        return Err(Response::from_parts(parts, Body::from_stream(body)));
    }
    if let Ok(v) = serde_json::from_slice::<Value>(&prefix)
        && v["error"]["type"] == "usage_limit_reached"
    {
        return Ok(v["error"]["resets_at"].as_i64().filter(|&reset| reset > now()));
    }
    Err(Response::from_parts(parts, Body::from(prefix.freeze())))
}

/// Whose a socket's turns are counted as: the account (and the reset it
/// was chosen at), the client.
struct Turns {
    id: String,
    name: String,
    client: String,
    revision: u64,
    jar: crate::likeness::Jar,
    family: String,
}

/// What a message the server sent on a socket says, to the pool.
enum Said {
    /// A turn began (its model).
    Began(String),
    /// A turn ended: its status (200, or the error's), the event (its usage).
    Ended(u16, Value),
    /// The account ran out (an `error` event with status 429).
    Out(Value),
    /// The account's meters (`codex.rate_limits`, as ChatGPT says them on
    /// a socket where it would say them in an answer's headers).
    Limits(Value),
    /// Nothing the pool counts.
    Other,
}

/// What `m` (a text message from the server) says (read only where it can
/// say something: a turn's delta is passed over unparsed).
fn said(m: &str) -> Said {
    let kinds = ["response.created", "response.completed", "response.done", "response.failed", "response.incomplete", "\"error\"", "codex.rate_limits"];
    if !kinds.iter().any(|k| m.contains(k)) {
        return Said::Other;
    }
    let Ok(v) = serde_json::from_str::<Value>(m) else { return Said::Other };
    match v["type"].as_str() {
        Some("response.created") => Said::Began(v["response"]["model"].as_str().unwrap_or_default().to_owned()),
        Some("response.completed" | "response.done" | "response.incomplete") => Said::Ended(200, v),
        Some("response.failed") => Said::Ended(500, v),
        Some("error") => {
            let status = v["status"].as_u64().or_else(|| v["status_code"].as_u64()).and_then(|n| u16::try_from(n).ok()).unwrap_or(500);
            let scoped = [&v["headers"], &v["error"]["headers"]].iter().any(|h| {
                h.as_object()
                    .into_iter()
                    .flatten()
                    .any(|(k, v)| k.eq_ignore_ascii_case("x-codex-rate-limit-reached-type") || k.eq_ignore_ascii_case("x-codex-active-limit") && v.as_str() != Some("codex"))
            });
            if status == 429
                && !scoped
                && v["error"]["type"] == "usage_limit_reached"
                && !matches!(v["error"]["code"].as_str(), Some("websocket_connection_limit_reached" | "previous_response_not_found"))
            {
                Said::Out(v)
            } else {
                Said::Ended(status, v)
            }
        }
        Some("codex.rate_limits") => Said::Limits(v),
        _ => Said::Other,
    }
}

/// Pass a switched socket on both ways, as it came, counting its turns
/// (`Said`), until either side closes it, or the account runs out inside
/// it: then marked, and the socket closed with that unsaid.
async fn relay_socket(s: Arc<Shared>, turns: Turns, on: hyper::upgrade::OnUpgrade, host: hyper::upgrade::OnUpgrade, extensions: Option<String>, switching: crate::drain::Underway) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn write<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) -> bool {
        matches!(tokio::time::timeout(Duration::from_secs(15 * 60), writer.write_all(bytes)).await, Ok(Ok(())))
    }
    let (theirs, ours) = match tokio::join!(on, host) {
        (Ok(a), Ok(b)) => (hyper_util::rt::TokioIo::new(a), hyper_util::rt::TokioIo::new(b)),
        _ => return,
    };
    let drain = &s.drain;
    let _tunnel = drain.tunnel();
    drop(switching);
    let (mut from_client, mut to_client) = tokio::io::split(theirs);
    let (mut from_host, mut to_host) = tokio::io::split(ours);
    // A clean TCP FIN permits a final reply in the other direction. A
    // reset, a failed write, or a deliberate cutoff ends both directions.
    // Codex serializes response streams on a socket. Keep a drain request
    // from the first data frame, including the wait for response.created.
    let active = Mutex::new(None);
    let allow = || {
        let mut active = active.lock().unwrap();
        if active.is_some() {
            return true; // The current turn can still be interrupted.
        }
        let guard = crate::drain::Underway::new(&s);
        let current = {
            let p = s.pool.lock().unwrap();
            p.reset_revision(&turns.id) == turns.revision && p.account(&turns.id).is_some_and(|a| schedule::eligible(a, Provider::Codex, &turns.family, now(), true))
        };
        if !current || drain.draining() || s.paused(Provider::Codex) || !s.jars.lock().unwrap().get(&turns.id).is_some_and(|jar| Arc::ptr_eq(jar, &turns.jar)) {
            return false;
        }
        *active = Some(guard);
        true
    };
    enum End {
        Stop,
        Close,
        Fin,
    }
    let up = async {
        let mut frames = crate::socket::ClientFrames::default();
        let mut buf = vec![0u8; 16 << 10];
        loop {
            match from_client.read(&mut buf).await {
                Ok(0) => return if matches!(tokio::time::timeout(Duration::from_secs(5), to_host.shutdown()).await, Ok(Ok(()))) { End::Fin } else { End::Stop },
                Err(_) => return End::Stop,
                Ok(n) => {
                    let (pass, close) = frames.before_data(&buf[..n], allow);
                    if !write(&mut to_host, &buf[..pass]).await {
                        return End::Stop;
                    }
                    if pass != 0 {
                        drain.tunnel_crossed();
                    }
                    if close {
                        return End::Close;
                    }
                    if pass < n {
                        return End::Stop;
                    }
                }
            }
        }
    };
    let down = async {
        let mut reader = crate::socket::Reader::new(extensions.as_deref());
        let mut turn: Option<Finish> = None;
        let mut exposed_output = false;
        let mut buf = vec![0u8; 16 << 10];
        let clean_eof = loop {
            let n = match from_host.read(&mut buf).await {
                Ok(0) => break End::Fin,
                Err(_) => break End::Stop,
                Ok(n) => n,
            };
            drain.tunnel_crossed();
            let mut finishing = Vec::new();
            let (bytes, stop) = reader.read_while(&buf[..n], |m| {
                match said(m) {
                    Said::Began(model) => {
                        active.lock().unwrap().get_or_insert_with(|| crate::drain::Underway::new(&s));
                        turn = Some(s.begin_turn(&turns, &model));
                    }
                    Said::Ended(status, v) => {
                        finishing.extend(active.lock().unwrap().take());
                        exposed_output = false;
                        if let Some(mut f) = turn.take() {
                            f.status = status;
                            f.scan.event(&v);
                            f.ended = true;
                            if v["type"] == "response.incomplete" {
                                f.outcome = Some(if v["response"]["incomplete_details"]["reason"] == "interrupted" { Outcome::Cancelled } else { Outcome::Failed });
                            }
                        }
                    }
                    Said::Out(v) => {
                        s.socket_out(&turns, &v);
                        finishing.extend(active.lock().unwrap().take());
                        if let Some(mut f) = turn.take() {
                            f.status = 429;
                            f.ended = exposed_output;
                            f.outcome = Some(Outcome::RateLimited);
                        }
                        // Once a result or tool item has gone to the client,
                        // preserve the actual terminal error as well.
                        return exposed_output;
                    }
                    Said::Limits(v) => s.socket_limits(&turns, &v),
                    Said::Other => exposed_output = true,
                }
                true
            });
            if reader.blind() {
                finishing.extend(active.lock().unwrap().take());
                if let Some(mut f) = turn.take() {
                    f.broken = true;
                }
            }
            if !write(&mut to_client, &bytes).await || stop {
                break End::Stop;
            }
            if reader.closed {
                break End::Close;
            }
        };
        if let Some(mut f) = turn.take() {
            f.broken = true;
        }
        if !write(&mut to_client, &reader.finish()).await {
            return End::Stop;
        }
        if !matches!(clean_eof, End::Close) {
            let _ = tokio::time::timeout(Duration::from_secs(5), to_client.shutdown()).await;
        }
        clean_eof
    };
    tokio::pin!(up, down);
    let (half_closed, other) = tokio::select! {
        half_closed = &mut up => (half_closed, futures_util::future::Either::Left(down)),
        half_closed = &mut down => (half_closed, futures_util::future::Either::Right(up)),
    };
    let grace = match half_closed {
        End::Stop => return,
        End::Close => Duration::from_secs(5),
        End::Fin => Duration::from_secs(15 * 60),
    };
    let _ = tokio::time::timeout(grace, other).await;
}

impl Shared {
    /// A turn begins on a socket for `turns`' account: open, and counted
    /// as busy, until it ends (its `Finish` dropped).
    fn begin_turn(self: &Arc<Self>, turns: &Turns, model: &str) -> Finish {
        let open = {
            let mut p = self.pool.lock().unwrap();
            *p.in_flight.entry(turns.id.clone()).or_default() += 1;
            p.next_open += 1;
            let open = p.next_open;
            p.open.push(Open {
                id: open,
                account: turns.name.clone(),
                account_id: turns.id.clone(),
                provider: Provider::Codex,
                model: model.to_owned(),
                client: turns.client.clone(),
                started: now_ms(),
            });
            open
        };
        self.noted();
        let mut scan = Scan::new();
        scan.sse = true;
        Finish {
            id: turns.id.clone(),
            name: turns.name.clone(),
            open,
            provider: Provider::Codex,
            revision: turns.revision,
            model: model.to_owned(),
            client: turns.client.clone(),
            started: std::time::Instant::now(),
            dispatched_at: now_ms(),
            status: 0,
            outcome: None,
            ended: false,
            length: None,
            passed: 0,
            broken: false,
            scan,
            decode: Decode::Plain,
            shared: Some(self.clone()),
        }
    }

    /// `turns`' account's meters as a socket says them (`event`, a
    /// `codex.rate_limits`), read as the same meters in an answer's
    /// headers are, unless it's been reset since it was chosen.
    fn socket_limits(&self, turns: &Turns, event: &Value) {
        let mut headers = HeaderMap::new();
        for kind in ["primary", "secondary"] {
            let w = &event["rate_limits"][kind];
            for (field, name) in [("used_percent", "used-percent"), ("window_minutes", "window-minutes"), ("reset_at", "reset-at"), ("reset_after_seconds", "reset-after-seconds")] {
                if let Some(n) = w[field].as_f64()
                    && let (Ok(k), Ok(v)) = (HeaderName::from_bytes(format!("x-codex-{kind}-{name}").as_bytes()), HeaderValue::from_str(&n.to_string()))
                {
                    headers.insert(k, v);
                }
            }
        }
        let (mut windows, _) = upstream::header_windows(&headers, Provider::Codex, "", 200, now());
        if windows.is_empty() {
            return;
        }
        {
            let mut p = self.pool.lock().unwrap();
            if p.reset_revision(&turns.id) != turns.revision {
                return;
            }
            if let Some(a) = p.account_mut(&turns.id) {
                let limit = event["metered_limit_name"].as_str().or_else(|| event["limit_name"].as_str());
                if let Some(limit) = limit.filter(|name| !name.eq_ignore_ascii_case("codex")) {
                    let scope = limit.to_ascii_lowercase().replace('-', "_");
                    windows.retain_mut(|w| {
                        let name = format!("{scope}/{}", w.name);
                        let Some(known) = a.windows.iter().find(|known| known.name == name) else { return false };
                        w.name = name;
                        w.models.clone_from(&known.models);
                        w.label.clone_from(&known.label);
                        true
                    });
                }
                if self.drain.retired() {
                    self.tell_late(&Late::Seen { id: turns.id.clone(), at: now_ms(), windows: windows.clone(), cooldown: None, model_cooldown: None, paid: None });
                }
                a.merge_windows(windows);
            }
        }
        self.noted();
    }

    /// `turns`' account ran out inside a socket (`event`, the 429 Codex
    /// would have been told): its meters from the headers the event
    /// carries, and a wait until its limit resets (as the event says, else
    /// a minute), unless it's been reset since it was chosen.
    fn socket_out(&self, turns: &Turns, event: &Value) {
        let mut headers = HeaderMap::new();
        for named in [&event["headers"], &event["error"]["headers"]] {
            for (k, v) in named.as_object().into_iter().flatten() {
                let v = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(&v)) {
                    headers.append(k, v);
                }
            }
        }
        let t = now();
        let (windows, rejection) = upstream::header_windows(&headers, Provider::Codex, "", 429, t);
        // (When its limit resets, as the event says; else as its headers
        // say, else a minute.)
        let until = event["error"]["resets_at"].as_i64().filter(|&r| r > t).or_else(|| rejection.map(|r| r.until)).unwrap_or(t + 60);
        {
            let mut p = self.pool.lock().unwrap();
            if p.reset_revision(&turns.id) != turns.revision {
                return;
            }
            if self.drain.retired() {
                self.tell_late(&Late::Seen { id: turns.id.clone(), at: now_ms(), windows: windows.clone(), cooldown: Some(until), model_cooldown: None, paid: None });
            }
            if let Some(a) = p.account_mut(&turns.id) {
                a.merge_windows(windows);
                a.cooldown_until = a.cooldown_until.max(Some(until));
            }
        }
        self.noted();
    }
}

/// What a copy of the pool that handed over (retired) tells the copy now
/// at work of what it saw after: a turn it finished, what an answer or a
/// socket said of an account. The copy at work books it, and saves it: the
/// retired one saves nothing (Codex's fourth review: a turn that finished
/// there was counted in its memory only, a refusal it learned too).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "late", rename_all = "snake_case")]
pub(crate) enum Late {
    Turn {
        id: String,
        name: String,
        provider: Provider,
        usage: Usage,
        passed: bool,
        request: Request,
    },
    /// Its meters, and a refusal's cooldown (the account's, or a model
    /// family's), as of `at`.
    Seen {
        id: String,
        at: i64,
        windows: Vec<crate::model::Window>,
        cooldown: Option<i64>,
        model_cooldown: Option<(String, i64)>,
        /// Answered from usage credits (`upstream::Paid`): the request's
        /// family, whether its own meter alone, until when, the meter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paid: Option<(String, bool, i64, String)>,
    },
}

impl Shared {
    /// Retired: tell the copy at work `late` (`Drain::tell`); else nothing.
    fn tell_late(&self, late: &Late) {
        if !self.drain.retired() {
            return;
        }
        match serde_json::to_string(late) {
            Ok(line) if self.drain.tell(&line) => {}
            Ok(_) => tracing::info!("pool: what finished here after the handover couldn't be told the copy at work"),
            Err(e) => tracing::warn!("pool: {e}"),
        }
    }

    /// Book what the copy handed over from tells (`Late`), as this copy
    /// books its own; what it saw before a reset confirmed here is old.
    pub(crate) fn book_late(&self, late: Late) {
        {
            let mut p = self.pool.lock().unwrap();
            match late {
                Late::Turn { id, name, provider, usage, passed, request } => {
                    if let Some(a) = p.account_mut(&id) {
                        if passed {
                            a.record(&usage, &today(), request.at);
                        }
                        a.finished(request.clone());
                    }
                    p.activity.record(&request);
                    p.served.push_back(Served { account: name, account_id: id, provider, request });
                    while p.served.len() > 50 {
                        p.served.pop_front();
                    }
                }
                Late::Seen { id, at, windows, cooldown, model_cooldown, paid } => {
                    let to_ms = |v: i64| if v > 0 && v < 10_000_000_000 { v.saturating_mul(1000) } else { v };
                    if p.reset_at.get(&id).is_some_and(|&r| to_ms(r) >= to_ms(at)) {
                        return;
                    }
                    if let Some(a) = p.account_mut(&id) {
                        a.merge_windows(windows);
                        a.cooldown_until = a.cooldown_until.max(cooldown);
                        if let Some((family, until)) = model_cooldown {
                            let kept = a.model_cooldowns.entry(family).or_insert(until);
                            *kept = (*kept).max(until);
                        }
                        if let Some((family, model_only, until, meter)) = paid {
                            a.now_paid(&family, model_only, until, &meter);
                        }
                    }
                }
            }
        }
        self.noted();
    }

    /// Read what the copy this one took over from tells (`Drain::tell`),
    /// on a thread of its own, until it has finished: booked as it comes.
    pub fn follow_predecessor(self: &Arc<Self>) {
        let Some(pipe) = self.drain.told_by_predecessor() else { return };
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("pool-late".into()).spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(std::fs::File::from(pipe)).lines() {
                let (Ok(line), Some(s)) = (line, weak.upgrade()) else { break };
                match serde_json::from_str::<Late>(&line) {
                    Ok(late) => s.book_late(late),
                    Err(e) => tracing::warn!("pool: what the copy handed over from told: {e}"),
                }
            }
        });
    }
}

/// Where `metadata.user_id`'s string is in a JSON request body (its
/// quotes with it), if it has one.
fn user_id_span(body: &[u8]) -> Option<std::ops::Range<usize>> {
    use serde_json::value::RawValue;
    #[derive(serde::Deserialize)]
    struct Top<'a> {
        #[serde(borrow)]
        metadata: Option<&'a RawValue>,
    }
    #[derive(serde::Deserialize)]
    struct Meta<'a> {
        #[serde(borrow)]
        user_id: Option<&'a RawValue>,
    }
    let top: Top = serde_json::from_slice(body).ok()?;
    let meta: Meta = serde_json::from_str(top.metadata?.get()).ok()?;
    let raw = meta.user_id?.get();
    if !raw.starts_with('"') {
        return None;
    }
    // (Both read in place: the string's own bytes in the body.)
    let start = (raw.as_ptr() as usize).checked_sub(body.as_ptr() as usize)?;
    (start + raw.len() <= body.len()).then(|| start..start + raw.len())
}

/// Map a byte boundary in a decoded JSON string back to its original
/// spelling. `text` is the validated decoding of `raw`, including any
/// escaped BMP characters or surrogate pairs.
fn spelling_offset(raw: &[u8], text: &str, offset: usize) -> Option<usize> {
    let mut at = 1;
    for (decoded, c) in text.char_indices() {
        if decoded == offset {
            return Some(at);
        }
        at += match raw.get(at)? {
            b'\\' if raw.get(at + 1) == Some(&b'u') => 6 * c.len_utf16(),
            b'\\' => 2,
            _ => c.len_utf8(),
        };
    }
    (offset == text.len()).then_some(at)
}

/// Replace the actual inner JSON field, preserving the original spelling
/// of everything around it at both levels of JSON encoding.
fn with_account(body: &[u8], span: std::ops::Range<usize>, ours: &str) -> Option<Bytes> {
    #[derive(serde::Deserialize)]
    struct UserId<'a> {
        #[serde(borrow)]
        account_uuid: &'a serde_json::value::RawValue,
    }
    let raw = &body[span.clone()];
    let text: String = serde_json::from_slice(raw).ok()?;
    let id = serde_json::from_str::<UserId<'_>>(&text).ok()?.account_uuid.get();
    let theirs: String = serde_json::from_str(id).ok()?;
    if theirs.is_empty() || theirs == ours {
        return None;
    }
    let start = (id.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
    let from = span.start + spelling_offset(raw, &text, start + 1)?;
    let to = span.start + spelling_offset(raw, &text, start + id.len() - 1)?;
    let inner = serde_json::to_string(ours).ok()?;
    let outer = serde_json::to_vec(&inner[1..inner.len() - 1]).ok()?;
    let mut result = Vec::with_capacity(body.len() - (to - from) + outer.len() - 2);
    result.extend_from_slice(&body[..from]);
    result.extend_from_slice(&outer[1..outer.len() - 1]);
    result.extend_from_slice(&body[to..]);
    Some(Bytes::from(result))
}

/// Why a try failed: before it reached the host (`is_connect`), or
/// perhaps after.
#[derive(Debug)]
enum Sent {
    Connect(String),
    Other(String),
}

impl Sent {
    fn is_connect(&self) -> bool {
        matches!(self, Sent::Connect(_))
    }
}

impl std::fmt::Display for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Sent::Connect(e) | Sent::Other(e) => f.write_str(e),
        }
    }
}

impl From<reqwest::Error> for Sent {
    fn from(e: reqwest::Error) -> Sent {
        if e.is_connect() { Sent::Connect(e.to_string()) } else { Sent::Other(e.to_string()) }
    }
}

impl From<crate::relay::SendError> for Sent {
    fn from(e: crate::relay::SendError) -> Sent {
        match e {
            crate::relay::SendError::Connect(m) => Sent::Connect(m),
            crate::relay::SendError::Other(m) => Sent::Other(m),
        }
    }
}

/// Tries a request has that the network fails (its changes, none there).
const NETWORK_TRIES: u32 = 4;
/// How long a request waits for a network to come back.
const NETWORK_PATIENCE: Duration = Duration::from_secs(60);

/// Whether a failure since `seen` was the network's: it changed (a
/// connection on the old one cut), or there's none.
fn network_failed(seen: u64) -> bool {
    olo_net::generation() != seen || !olo_net::online()
}

/// Once there's a network (at once if there is; else waiting for one, up
/// to [`NETWORK_PATIENCE`], off the runtime's threads).
async fn network_back() -> bool {
    if olo_net::online() {
        return true;
    }
    tokio::task::spawn_blocking(|| olo_net::wait_online(Some(std::time::Instant::now() + NETWORK_PATIENCE), &|| false)).await.unwrap_or(false)
}

/// One try at a request on one account: when dropped (the answer done,
/// the client gone, or the try given up) it frees the account, counts
/// what the answer reported and keeps how it ended.
struct Finish {
    id: String,
    name: String,
    open: u64,
    provider: Provider,
    revision: u64,
    /// Wall-clock timestamp when this request was dispatched (used to order
    /// late pre-handover observations against resets and paid observations).
    dispatched_at: i64,
    model: String,
    client: String,
    started: std::time::Instant,
    status: u16,
    /// How it ended, when known before the answer (None: passed on).
    outcome: Option<Outcome>,
    /// The answer reached its end; how long it said it is, and how much of
    /// it has passed (a server passing on an answer of a known length reads
    /// no further than that: its end is its last byte).
    ended: bool,
    length: Option<u64>,
    passed: u64,
    /// The answer broke off upstream.
    broken: bool,
    scan: Scan,
    decode: Decode,
    shared: Option<Arc<Shared>>,
}

impl Drop for Finish {
    fn drop(&mut self) {
        let Some(s) = self.shared.take() else { return };
        let passed = self.ended || self.outcome.is_none();
        let outcome = self.outcome.unwrap_or(match self.status {
            _ if self.broken => Outcome::Failed,
            _ if !self.ended => Outcome::Cancelled,
            429 => Outcome::RateLimited,
            400.. => Outcome::Failed,
            _ => Outcome::Completed,
        });
        let decode = std::mem::replace(&mut self.decode, Decode::Plain);
        let (mut usage, decoded_ok) = if matches!(decode, Decode::Plain) {
            (self.scan.finish(), !self.scan.truncated && !self.scan.malformed)
        } else if self.ended
            && !self.broken
            && let Some(tail) = decode.finish()
        {
            self.scan.feed(&tail);
            (self.scan.finish(), !self.scan.truncated && !self.scan.malformed)
        } else {
            (Usage::default(), false)
        };
        let agy_consumed_credits = self.provider == Provider::Antigravity && passed && (200..300).contains(&self.status) && self.scan.consumed_credits;
        let agy_clean_ok = self.provider == Provider::Antigravity
            && passed
            && self.ended
            && !self.broken
            && decoded_ok
            && self.scan.recognized_response
            && (200..300).contains(&self.status)
            && !self.scan.consumed_credits;
        usage.requests = 1;
        let t = now();
        let request = Request {
            at: t,
            model: self.model.clone(),
            client: self.client.clone(),
            outcome,
            status: self.status,
            ms: self.started.elapsed().as_millis() as u64,
            usage: if passed { usage.clone() } else { Usage::default() },
        };
        #[allow(clippy::type_complexity)]
        let mut late_paid: Option<(Vec<Window>, (String, bool, i64, String))> = None;
        let mut need_agy_sync = false;
        {
            let mut p = s.pool.lock().unwrap();
            if let Some(n) = p.in_flight.get_mut(&self.id) {
                *n = n.saturating_sub(1);
            }
            p.open.retain(|o| o.id != self.open);
            let current = p.reset_revision(&self.id) == self.revision;
            if let Some(a) = p.account_mut(&self.id) {
                if passed {
                    a.record(&usage, &today(), t);
                }
                if current && agy_consumed_credits {
                    let family = model_family(&self.model);
                    let (chosen_idx, scoped_only, multi) = pick_narrowest_agy_window(a, &family, false, None, t);
                    need_agy_sync = multi || chosen_idx.is_none();
                    let effective_scoped = scoped_only || (chosen_idx.is_none() && !family.is_empty());
                    let mut marked = Vec::new();
                    let (meter, until) = if let Some(idx) = chosen_idx {
                        let w = &mut a.windows[idx];
                        let until = w.resets_at.filter(|&r| r > t).unwrap_or(t + upstream::PAID_UNKNOWN);
                        if w.used < 100.0 {
                            w.used = 100.0;
                            if w.resets_at.is_none() {
                                w.resets_at = Some(until);
                            }
                            marked.push(w.clone());
                        }
                        (w.name.clone(), until)
                    } else {
                        // Before first sync (or when multiple buckets match
                        // ambiguously), record a model-scoped synthetic window
                        // and trigger sync instead of marking the whole account paid.
                        let until = t + upstream::PAID_UNKNOWN;
                        let synth_name = if !family.is_empty() { format!("model:{family}") } else { "quota".to_owned() };
                        let synth = Window {
                            name: synth_name.clone(),
                            label: if !family.is_empty() { family.clone() } else { "Quota".to_owned() },
                            used: 100.0,
                            resets_at: Some(until),
                            models: if !family.is_empty() { vec![family.clone()] } else { Vec::new() },
                        };
                        marked.push(synth.clone());
                        a.merge_windows(vec![synth]);
                        (synth_name, until)
                    };
                    if a.paid_until(&family, t).is_none() {
                        tracing::info!("pool: {} answered a {family} request using Antigravity AI credits ({meter}): left for another account or a banked reset from here on", a.name);
                    }
                    a.now_paid(&family, effective_scoped, until, &meter);
                    late_paid = Some((marked, (family, effective_scoped, until, meter)));
                } else if current && agy_clean_ok {
                    let family = model_family(&self.model);
                    let dispatched = self.dispatched_at;
                    if let Some(paid) = &a.paid
                        && dispatched > paid.at_ms()
                        && (paid.meter == "quota" || a.windows.iter().any(|w| w.name == paid.meter && w.applies_to(&family)))
                    {
                        a.paid = None;
                    }
                    if a.model_paid.get(&family).is_some_and(|pf| dispatched > pf.at_ms()) {
                        a.model_paid.remove(&family);
                    }
                }
                a.finished(request.clone());
            }
            p.activity.record(&request);
            p.served.push_back(Served { account: self.name.clone(), account_id: self.id.clone(), provider: self.provider, request: request.clone() });
            while p.served.len() > 50 {
                p.served.pop_front();
            }
        }
        if need_agy_sync && !s.drain.retired() {
            s.sync_soon(Some(self.id.clone()));
        }
        // (Handed over: booked where it's saved.)
        if s.drain.retired() {
            if let Some((windows, paid)) = late_paid {
                s.tell_late(&Late::Seen { id: self.id.clone(), at: self.dispatched_at, windows, cooldown: None, model_cooldown: None, paid: Some(paid) });
            }
            s.tell_late(&Late::Turn { id: self.id.clone(), name: self.name.clone(), provider: self.provider, usage, passed, request });
        }
        // (What it used, bookkeeping: saved with the next, or soon.)
        s.noted();
    }
}

fn respond(mut r: crate::relay::Answer, finish: Finish, method: &Method) -> Response {
    let status = r.status();
    // (Each name in its case, the status line's reason: hyper's notes.)
    let ext = r.take_extensions();
    let mut out = HeaderMap::new();
    let transfer_coded = r.headers().contains_key("transfer-encoding");
    for (k, v) in r.headers().iter().filter(|(k, _)| returned(k.as_str()) && !(transfer_coded && k.as_str() == "content-length")) {
        out.append(k.clone(), v.clone());
    }
    if let Ok(v) = HeaderValue::from_str(&finish.name) {
        out.insert(HeaderName::from_static("x-olo-pool-account"), v);
    }
    // What the client is told of its usage: nothing used, for as long as
    // the pool has more to draw from than the account that answered
    // (`told`). The pool's own reading of this answer is done: `serve`.
    if finish.shared.as_ref().is_some_and(|s| s.more_to_draw(finish.provider, &model_family(&finish.model), Some(&finish.id))) {
        crate::told::headers(finish.provider, &mut out);
    }
    let finish = Arc::new(Mutex::new(finish));
    {
        let mut f = finish.lock().unwrap();
        f.scan.sse = r.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("text/event-stream"));
        f.length = out.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
        f.ended = f.length == Some(0) || method == Method::HEAD || matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED);
    }
    // (Passed on as it came, compressed as the client asked; the pool reads
    // a decoded copy as it goes by.)
    finish.lock().unwrap().decode = Decode::of(r.headers().get("content-encoding").and_then(|v| v.to_str().ok()).unwrap_or(""));
    let end = finish.clone();
    let stream = r
        .into_stream()
        .map(move |chunk| {
            let mut f = finish.lock().unwrap();
            match &chunk {
                Ok(b) => {
                    let Finish { scan, decode, .. } = &mut *f;
                    scan.feed(&decode.push(b));
                    f.passed += b.len() as u64;
                    if f.length.is_some_and(|n| f.passed >= n) {
                        f.ended = true;
                    }
                }
                Err(_) => f.broken = true,
            }
            chunk
        })
        // Reached only when the answer has all been read.
        .chain(futures_util::stream::poll_fn(move |_| {
            end.lock().unwrap().ended = true;
            std::task::Poll::Ready(None)
        }));
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    *resp.headers_mut() = out;
    *resp.extensions_mut() = ext;
    resp
}

/// An answer's body decoded as it streams by, for the pool's own reading
/// (its `content-encoding`: what the client asked for; one the pool can't
/// read is passed on all the same, its usage unread).
enum Decode {
    Plain,
    Gzip(flate2::write::GzDecoder<Vec<u8>>),
    Deflate(flate2::Decompress, bool),
    Br(Box<brotli_decompressor::DecompressorWriter<Vec<u8>>>),
    Zstd(Box<zstd::stream::write::Decoder<'static, Vec<u8>>>),
    Unread,
}

/// A whole body as it reads once its `content-encoding` is undone (none:
/// as it is), capping decoded output at `max_bytes` so a small compressed
/// payload cannot allocate unbounded memory during error inspection.
pub(crate) fn decoded_bounded(coding: &str, body: &[u8], max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    let reader: Box<dyn std::io::Read + '_> = match coding.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => {
            return (body.len() <= max_bytes).then(|| body.to_vec());
        }
        "gzip" | "x-gzip" => Box::new(flate2::read::GzDecoder::new(body)),
        "deflate" => Box::new(flate2::read::ZlibDecoder::new(body)),
        "br" => Box::new(brotli_decompressor::Decompressor::new(body, 4096)),
        "zstd" => Box::new(zstd::stream::read::Decoder::new(body).ok()?),
        _ => return None,
    };
    reader.take((max_bytes + 1) as u64).read_to_end(&mut out).ok()?;
    (out.len() <= max_bytes).then_some(out)
}

impl Decode {
    fn of(coding: &str) -> Decode {
        match coding.trim().to_ascii_lowercase().as_str() {
            "" | "identity" => Decode::Plain,
            "gzip" | "x-gzip" => Decode::Gzip(flate2::write::GzDecoder::new(Vec::new())),
            "deflate" => Decode::Deflate(flate2::Decompress::new(true), false),
            "br" => Decode::Br(Box::new(brotli_decompressor::DecompressorWriter::new(Vec::new(), 4096))),
            "zstd" => zstd::stream::write::Decoder::new(Vec::new()).map(|d| Decode::Zstd(Box::new(d))).unwrap_or(Decode::Unread),
            _ => Decode::Unread,
        }
    }

    fn finish(self) -> Option<Vec<u8>> {
        match self {
            Decode::Plain | Decode::Deflate(_, true) => Some(Vec::new()),
            Decode::Unread | Decode::Deflate(_, false) => None,
            Decode::Gzip(d) => d.finish().ok(),
            Decode::Br(d) => d.into_inner().ok(),
            Decode::Zstd(d) => d.finish().ok(),
        }
    }

    /// What `chunk` decodes to, as far as it goes so far (nothing more,
    /// once it doesn't decode: that answer's usage goes unread).
    fn push<'a>(&mut self, chunk: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        use std::io::Write;
        fn out<W: Write>(w: &mut W, chunk: &[u8], inner: impl FnOnce(&mut W) -> &mut Vec<u8>) -> Option<Vec<u8>> {
            w.write_all(chunk).ok()?;
            w.flush().ok()?;
            Some(std::mem::take(inner(w)))
        }
        let decoded = match self {
            Decode::Plain => return std::borrow::Cow::Borrowed(chunk),
            Decode::Unread => return std::borrow::Cow::Borrowed(&[]),
            Decode::Gzip(d) => out(d, chunk, |d| d.get_mut()),
            Decode::Deflate(d, ended) => inflate(d, ended, chunk),
            Decode::Br(d) => out(d.as_mut(), chunk, |d| d.get_mut()),
            Decode::Zstd(d) => out(d.as_mut(), chunk, |d| d.get_mut()),
        };
        match decoded {
            Some(d) => std::borrow::Cow::Owned(d),
            None => {
                *self = Decode::Unread;
                std::borrow::Cow::Borrowed(&[])
            }
        }
    }
}

/// A zlib writer's flush accepts an incomplete input. Remember whether the
/// inflater actually reached the end, including the checksum.
fn inflate(d: &mut flate2::Decompress, ended: &mut bool, mut input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if *ended {
        return input.is_empty().then_some(out);
    }
    let mut buf = [0; 8192];
    loop {
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let status = d.decompress(input, &mut buf, flate2::FlushDecompress::None).ok()?;
        let consumed = (d.total_in() - before_in) as usize;
        let produced = (d.total_out() - before_out) as usize;
        input = &input[consumed..];
        out.extend_from_slice(&buf[..produced]);
        if status == flate2::Status::StreamEnd {
            *ended = true;
            return input.is_empty().then_some(out);
        }
        if input.is_empty() && produced < buf.len() {
            return Some(out);
        }
        if consumed == 0 && produced == 0 {
            return None;
        }
    }
}

/// Reads the usage counts out of an answer as it passes, keeping nothing
/// else: server-sent events a line at a time, JSON whole (up to 8 MB).
struct Scan {
    sse: bool,
    pending: Vec<u8>,
    usage: Usage,
    /// Antigravity `GenerateContentResponse.consumedCredits` was non-empty
    /// (paid AI credits spent after subscription allowance ran out).
    consumed_credits: bool,
    /// True only when at least one recognized generation JSON object/event
    /// parsed cleanly (so empty/malformed 200 bodies never clear paid marks).
    recognized_response: bool,
    /// True if a relevant SSE `data:` JSON line or non-SSE body failed to parse.
    malformed: bool,
    /// True if the body exceeded scan limits so billing fields may have been missed.
    truncated: bool,
}

impl Scan {
    fn new() -> Scan {
        Scan { sse: false, pending: Vec::new(), usage: Usage::default(), consumed_credits: false, recognized_response: false, malformed: false, truncated: false }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if !self.sse {
            if self.pending.len() + chunk.len() <= 8 << 20 {
                self.pending.extend_from_slice(chunk);
            } else {
                self.truncated = true;
            }
            return;
        }
        self.pending.extend_from_slice(chunk);
        while let Some(i) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=i).collect();
            self.line(&line);
        }
        // A runaway line (a huge delta) needn't be kept: usage lines are short.
        if self.pending.len() > 1 << 20 {
            self.pending.clear();
            self.truncated = true;
        }
    }

    fn is_recognized_object(v: &Value) -> bool {
        let Some(obj) = v.as_object() else { return false };
        obj.contains_key("candidates")
            || obj.contains_key("response")
            || obj.contains_key("usageMetadata")
            || obj.contains_key("usage")
            || obj.contains_key("consumedCredits")
            || obj.contains_key("id")
            || obj.contains_key("content")
            || obj.contains_key("type")
    }

    fn line(&mut self, line: &[u8]) {
        let Some(data) = line.strip_prefix(b"data:") else { return };
        let trimmed = data.trim_ascii();
        if trimmed.is_empty() || trimmed == b"[DONE]" {
            return;
        }
        let has_billing = trimmed.windows(5).any(|w| w == b"usage") || trimmed.windows(15).any(|w| w == b"consumedCredits");
        let check_shape = !self.recognized_response && (trimmed.windows(10).any(|w| w == b"candidates") || trimmed.windows(8).any(|w| w == b"response"));
        if !has_billing && !check_shape {
            return;
        }
        match serde_json::from_slice::<Value>(trimmed) {
            Ok(v) => {
                if Self::is_recognized_object(&v) {
                    self.recognized_response = true;
                }
                self.event(&v);
            }
            Err(_) => {
                self.malformed = true;
            }
        }
    }

    fn event(&mut self, v: &Value) {
        let u = match v["type"].as_str() {
            Some("message_start") => &v["message"]["usage"],
            Some("message_delta") => &v["usage"],
            Some("response.completed" | "response.done" | "response.incomplete" | "response.failed") => &v["response"]["usage"],
            _ => &v["usage"],
        };
        let n = |k: &str| u[k].as_u64();
        let input = [n("input_tokens"), n("cache_creation_input_tokens"), n("cache_read_input_tokens")].into_iter().flatten().sum::<u64>();
        if input > 0 {
            self.usage.input_tokens = self.usage.input_tokens.max(input);
        }
        if let Some(out) = n("output_tokens") {
            self.usage.output_tokens = self.usage.output_tokens.max(out);
        }
        // Antigravity (`GenerateContentResponse`): `response.usageMetadata` or top-level `usageMetadata`.
        let um = if v["response"]["usageMetadata"].is_object() { &v["response"]["usageMetadata"] } else { &v["usageMetadata"] };
        if um.is_object() {
            if let Some(prompt) = um["promptTokenCount"].as_u64() {
                self.usage.input_tokens = self.usage.input_tokens.max(prompt);
            }
            let cand = um["candidatesTokenCount"].as_u64().unwrap_or(0);
            let thoughts = um["thoughtsTokenCount"].as_u64().unwrap_or(0);
            let out = cand + thoughts;
            if out > 0 {
                self.usage.output_tokens = self.usage.output_tokens.max(out);
            }
        }
        if v["consumedCredits"].as_array().is_some_and(|a| !a.is_empty()) || v["response"]["consumedCredits"].as_array().is_some_and(|a| !a.is_empty()) {
            self.consumed_credits = true;
        }
    }

    fn finish(&mut self) -> Usage {
        if !self.sse && !self.pending.is_empty() {
            match serde_json::from_slice::<Value>(&self.pending) {
                Ok(v) => {
                    if Self::is_recognized_object(&v) {
                        self.recognized_response = true;
                    }
                    self.event(&v);
                }
                Err(_) => {
                    self.malformed = true;
                }
            }
        } else if self.sse && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.line(&rest);
        }
        self.pending.clear();
        self.usage.clone()
    }
}

/// Whether sign-in `theirs` is a renewal of `ours`: a different one that
/// lasts longer (a renewal's always does).
fn newer(theirs: &Credential, ours: &Credential) -> bool {
    theirs.access != ours.access && theirs.expires_at.unwrap_or(0) > ours.expires_at.unwrap_or(0)
}

/// One save of the pool as it is now (the saver's).
fn save_once(s: &Shared) {
    // Handed over: the new copy owns the file (a sign-in renewed
    // here is written in on its own: `fresh`).
    if s.drain.retired() {
        return;
    }
    match s.write(true) {
        Ok(()) => {}
        // (Not open yet: nothing to save.)
        Err(_) if s.drain.retired() || !s.pool.lock().unwrap().loaded => {}
        Err(e) => {
            tracing::warn!("pool: {e}");
            s.pool.lock().unwrap().error = Some(e);
            (s.changed)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the copy handed over from tells is booked as this copy's own,
    /// but what it saw before a reset confirmed here is old: it doesn't
    /// put the account back at its former limit.
    #[test]
    fn a_late_observation_from_before_a_reset_is_old() {
        let mut saved = Saved::default();
        saved.accounts.push(Account::new("a".into(), "a".into(), Provider::Codex));
        let s = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
        let window = |used: f64| crate::model::Window { name: "primary_window".into(), label: String::new(), used, resets_at: Some(now() + 3600), models: vec![] };
        let seen = |at: i64, used: f64| Late::Seen { id: "a".into(), at, windows: vec![window(used)], cooldown: Some(now() + 600), model_cooldown: None, paid: None };
        let base = now_ms();
        s.pool.lock().unwrap().reset_at.insert("a".into(), base - 10_000);
        s.book_late(seen(base - 60_000, 100.0));
        // Same millisecond as the reset is also rejected (`r >= at`).
        s.book_late(seen(base - 10_000, 100.0));
        let p = s.snapshot();
        let a = p.account("a").unwrap();
        assert!(a.windows.is_empty() && a.cooldown_until.is_none(), "an old observation taken: {:?}", a.windows);
        s.book_late(seen(base, 12.0));
        let p = s.snapshot();
        let a = p.account("a").unwrap();
        assert_eq!(a.windows.iter().map(|w| w.used).collect::<Vec<_>>(), [12.0]);
        assert!(a.cooldown_until.is_some());
        // A turn: its usage counted, its request among those served.
        let request =
            Request { at: now(), model: "gpt-5".into(), client: "codex".into(), outcome: Outcome::Completed, status: 200, ms: 5, usage: Usage { requests: 1, input_tokens: 11, output_tokens: 4 } };
        s.book_late(Late::Turn { id: "a".into(), name: "a".into(), provider: Provider::Codex, usage: request.usage.clone(), passed: true, request });
        let p = s.snapshot();
        assert_eq!((p.account("a").unwrap().total.input_tokens, p.served.len()), (11, 1));
    }

    #[test]
    fn scans_usage_only() {
        let mut s = Scan::new();
        s.sse = true;
        let events = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":90,\"output_tokens\":1}}}\n\n\
            data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"secret words\"}}\n\n\
            data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\n\n";
        // Fed in awkward pieces.
        for piece in events.as_bytes().chunks(7) {
            s.feed(piece);
        }
        assert_eq!(s.finish(), Usage { requests: 0, input_tokens: 100, output_tokens: 42 });
        let mut s = Scan::new();
        s.feed(br#"{"id":"x","usage":{"input_tokens":5,"output_tokens":6}}"#);
        assert_eq!(s.finish(), Usage { requests: 0, input_tokens: 5, output_tokens: 6 });
        let mut s = Scan::new();
        s.sse = true;
        s.feed(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":101,\"output_tokens\":13}}}\n\ndata: [DONE]\n\n");
        assert_eq!(s.finish(), Usage { requests: 0, input_tokens: 101, output_tokens: 13 });
    }

    /// An answer compressed as the client asked is read as it streams by,
    /// whichever encoding, however it's cut into chunks.
    #[test]
    fn compressed_answers_read_as_they_pass() {
        use std::io::Write;
        let text = b"event: message_delta\ndata: {\"usage\":{\"output_tokens\":42}}\n\n".repeat(20);
        let gz = {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&text).unwrap();
            e.finish().unwrap()
        };
        let zs = zstd::encode_all(&text[..], 3).unwrap();
        let br = {
            let mut out = Vec::new();
            let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            w.write_all(&text).unwrap();
            drop(w);
            out
        };
        for (coding, bytes) in [("gzip", gz), ("zstd", zs), ("br", br), ("identity", text.clone())] {
            let mut d = Decode::of(coding);
            let mut got = Vec::new();
            for chunk in bytes.chunks(7) {
                got.extend_from_slice(&d.push(chunk));
            }
            assert_eq!(got, text, "{coding}");
        }
    }

    /// The client's sign-in is put in its place (the account's), never
    /// passed on as it came, nor what belongs to its connection; the rest
    /// of what it sends goes on as it is (its cookies and origin too, as
    /// Claude Code and Codex send them), and what the host answers comes
    /// back so (its cookies too).
    #[test]
    fn headers_kept_back() {
        // (The client's cookies: the account's go instead.)
        for h in ["authorization", "x-api-key", "host", "connection", "content-length", "x-olo-pool-x", "cookie"] {
            assert!(!forwarded(h), "{h}");
        }
        for h in ["anthropic-version", "x-stainless-lang", "session_id", "origin", "x-organization-uuid", "accept-encoding"] {
            assert!(forwarded(h), "{h}");
        }
        // (Cookies set for the account stay with it.)
        assert!(!returned("set-cookie") && !returned("transfer-encoding"));
        assert!(returned("anthropic-ratelimit-unified-5h-utilization") && returned("content-encoding") && returned("content-length"));
    }

    /// The account's id goes into `metadata.user_id` alone: the same id in
    /// what a person wrote stays as they wrote it (Codex's review), and one
    /// written escaped is still found; the rest of the body byte for byte.
    #[test]
    fn the_account_id_changes_in_its_place_alone() {
        let (theirs, ours) = ("11111111-2222-3333-4444-555555555555", "99999999-8888-7777-6666-000000000000");
        let user_id = format!(r#"{{\"device_id\":\"d\",\"account_uuid\":\"{theirs}\",\"session_id\":\"s\"}}"#);
        let body = format!(r#"{{"model":"claude-opus-5-5","messages":[{{"role":"user","content":"my id is {theirs}"}}],"metadata":{{"user_id":"{user_id}"}}}}"#);
        let span = user_id_span(body.as_bytes()).expect("found");
        let made = with_account(body.as_bytes(), span, ours).expect("changed");
        assert_eq!(String::from_utf8(made.to_vec()).unwrap(), body.replacen(&format!(r#"account_uuid\":\"{theirs}"#), &format!(r#"account_uuid\":\"{ours}"#), 1));
        assert!(String::from_utf8_lossy(&made).contains(&format!("my id is {theirs}")));
        // Escaped where it's named (its first character), and only there.
        let escaped = body.replacen(&format!(r#"account_uuid\":\"{theirs}"#), &format!(r#"account_uuid\":\"\\u0031{}"#, &theirs[1..]), 1);
        let span = user_id_span(escaped.as_bytes()).expect("found");
        let made = with_account(escaped.as_bytes(), span.clone(), ours).expect("changed");
        let v: Value = serde_json::from_slice(&made).unwrap();
        let named: Value = serde_json::from_str(v["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(named["account_uuid"], ours);
        assert_eq!(v["messages"][0]["content"], format!("my id is {theirs}"));
        assert_eq!(&made[..span.start], &escaped.as_bytes()[..span.start]);
        // Nothing to change: its own already, or none named.
        let span = user_id_span(body.as_bytes()).unwrap();
        assert!(with_account(body.as_bytes(), span, theirs).is_none());
        assert!(user_id_span(br#"{"model":"m","metadata":{"user_id":7}}"#).is_none());
        assert!(user_id_span(br#"{"model":"m"}"#).is_none());
    }
}
