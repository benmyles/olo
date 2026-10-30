//! The gateway: how a request is served from the pool, on loopback. Claude
//! Code and Codex reach it through the proxy (`intercept`), as if talking
//! to their own hosts; the gateway's own HTTP server (`/claude/v1/…`,
//! `/codex/v1/…`, `/pool/status`) takes requests from clients pointed at
//! it by address too. Each request goes to the account the scheduler picks, with
//! that account's sign-in in place of the client's; a refused sign-in is
//! refreshed once, a rejection for quota moves to the next account, both
//! before any of the answer has been passed on. Anything else (a network
//! failure, a server error, a broken stream) is passed back as it is, as
//! the work upstream may have happened: the client's own retries decide.
//! Answers stream through; only their usage counts are kept, never what
//! was asked or answered.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    activity::Activity,
    model::{Account, Credential, Outcome, Provider, Request, Usage, model_family, now, today},
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
}

impl Pool {
    pub fn account(&self, id: &str) -> Option<&Account> {
        self.saved.accounts.iter().find(|a| a.id == id)
    }
    pub fn account_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.saved.accounts.iter_mut().find(|a| a.id == id)
    }
    pub fn busy(&self, id: &str) -> bool {
        self.in_flight.get(id).is_some_and(|&n| n > 0)
    }
}

/// The gateway's state, shared with the app.
pub struct Shared {
    pub pool: Mutex<Pool>,
    store: Mutex<Option<Store>>,
    client: reqwest::Client,
    up: Upstreams,
    /// One refresh at a time per account.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    save: std::sync::mpsc::Sender<()>,
    changed: Box<dyn Fn() + Send + Sync>,
    runtime: Mutex<Option<tokio::runtime::Handle>>,
    /// Make the sealed store when there's first something to keep.
    lazy: bool,
    /// When each account's quota is next read on its own, and how long
    /// the provider last made it wait (a 429 doubles it).
    due: Mutex<HashMap<String, (i64, i64)>>,
    /// When each account's reset credits are next read.
    credits_due: Mutex<HashMap<String, i64>>,
}

/// How often an account's quota is read while nothing asks for it (its
/// answers' headers keep it current meanwhile), and its credits.
const SYNC_EVERY: i64 = 5 * 60;
const CREDITS_EVERY: i64 = 30 * 60;
/// The longest wait after the provider asks the pool to slow down.
const SYNC_BACKOFF_MAX: i64 = 30 * 60;

impl Shared {
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
        let shared = Shared::make(Saved::default(), None, Upstreams::from_env(), true, changed);
        shared.pool.lock().unwrap().loaded = false;
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
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // Thinking can take a while before the first byte.
            .read_timeout(Duration::from_secs(15 * 60))
            .build()
            .expect("HTTP client");
        let shared = Arc::new(Shared {
            pool: Mutex::new(Pool { saved, loaded: true, ..Default::default() }),
            store: Mutex::new(store),
            client,
            up,
            locks: Mutex::new(HashMap::new()),
            save: tx,
            changed: Box::new(changed),
            runtime: Mutex::new(None),
            lazy,
            due: Mutex::new(HashMap::new()),
            credits_due: Mutex::new(HashMap::new()),
        });
        // Saving off every async and UI thread, a batch at a time.
        let weak = Arc::downgrade(&shared);
        let _ = std::thread::Builder::new().name("pool-save".into()).spawn(move || {
            while rx.recv().is_ok() {
                std::thread::sleep(Duration::from_millis(300));
                while rx.try_recv().is_ok() {}
                let Some(s) = weak.upgrade() else { break };
                let snapshot = {
                    let p = s.pool.lock().unwrap();
                    if !p.loaded {
                        continue;
                    }
                    p.saved.clone()
                };
                let mut store = s.store.lock().unwrap();
                // The first account made: the sealed file (and its key) now.
                if store.is_none() && s.lazy {
                    match Store::open() {
                        Ok((st, _)) => *store = Some(st),
                        Err(e) => {
                            s.pool.lock().unwrap().error = Some(e);
                            (s.changed)();
                            continue;
                        }
                    }
                }
                if let Some(st) = store.as_ref()
                    && let Err(e) = st.save(&snapshot)
                {
                    tracing::warn!("pool: {e}");
                    s.pool.lock().unwrap().error = Some(e);
                    (s.changed)();
                }
            }
        });
        shared
    }

    /// Something changed: save it, and show it.
    pub fn touch(&self) {
        let _ = self.save.send(());
        (self.changed)();
    }

    /// The HTTP client requests go out with.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Where a request to `host` the proxy passes through goes.
    pub fn upstream_origin(&self, host: &str) -> String {
        self.up.origin.clone().unwrap_or_else(|| format!("https://{host}"))
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
        Ok(())
    }

    /// Replace an account's sign-in (signed in again).
    pub fn reauth(self: &Arc<Self>, id: &str, credential: Credential) {
        {
            let mut p = self.pool.lock().unwrap();
            p.saved.credentials.insert(id.to_owned(), credential);
            if let Some(a) = p.account_mut(id) {
                a.signed_out = None;
                a.cooldown_until = None;
            }
        }
        self.touch();
        self.sync_soon(Some(id.to_owned()));
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
        }
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
    /// `force`d (the provider refused it).
    async fn fresh(&self, id: &str, provider: Provider, force: bool, within: i64) -> Result<Credential, Failure> {
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        // Someone else may have refreshed it while we waited.
        let current = self.pool.lock().unwrap().saved.credentials.get(id).cloned().ok_or_else(|| Failure::Refused("No sign-in".into()))?;
        let due = current.expires_at.is_some_and(|e| e - now() < within);
        if !force && !due {
            return Ok(current);
        }
        match upstream::refresh(&self.client, &self.up, provider, &current).await {
            Ok(c) => {
                self.pool.lock().unwrap().saved.credentials.insert(id.to_owned(), c.clone());
                self.touch();
                Ok(c)
            }
            Err(Failure::Refused(why)) => {
                self.signed_out(id, &why);
                Err(Failure::Refused(why))
            }
            // Not refused: the old one may still do.
            Err(e) if !force && current.expires_at.is_none_or(|t| t > now()) => {
                tracing::warn!("pool: {e}");
                Ok(current)
            }
            Err(e) => Err(e),
        }
    }

    fn signed_out(&self, id: &str, why: &str) {
        if let Some(a) = self.pool.lock().unwrap().account_mut(id) {
            a.signed_out = Some(why.to_owned());
        }
        self.touch();
    }

    /// Write the pool now (for what must be kept before a provider is asked).
    async fn save_now(&self) {
        let snapshot = {
            let p = self.pool.lock().unwrap();
            if !p.loaded {
                return;
            }
            p.saved.clone()
        };
        let store = self.store.lock().unwrap().is_some();
        if !store {
            // Not made yet: the saver makes it.
            self.touch();
            return;
        }
        // A small write, on this worker thread, before the provider is asked.
        let result = tokio::task::block_in_place(|| self.store.lock().unwrap().as_ref().map(|s| s.save(&snapshot)));
        if let Some(Err(e)) = result {
            tracing::warn!("pool: {e}");
        }
    }

    /// Spend one of account `id`'s reset credits (the soonest to expire),
    /// then read its quota again.
    pub async fn redeem(&self, id: &str) -> Result<(), String> {
        let lock = self.lock_for(&format!("redeem:{id}"));
        let _held = lock.lock().await;
        let (provider, org, credit, request) = {
            let mut p = self.pool.lock().unwrap();
            let a = p.account_mut(id).ok_or("No such account.")?;
            let credit = a.credit().cloned().ok_or("It has no reset credit to use now.")?;
            // The same redemption again, if one was left unclear.
            let request = match &a.redeeming {
                Some((c, r)) if *c == credit.id => r.clone(),
                _ => crate::store::new_uuid(),
            };
            a.redeeming = Some((credit.id.clone(), request.clone()));
            a.redeem_tried = Some(now());
            (a.provider, a.org.clone(), credit, request)
        };
        self.save_now().await;
        let cred = self.fresh(id, provider, false, 60).await.map_err(|e| e.to_string())?;
        let result = upstream::redeem(&self.client, &self.up, provider, &cred, org.as_deref(), &credit, &request).await;
        {
            let mut p = self.pool.lock().unwrap();
            if let Some(a) = p.account_mut(id) {
                match &result {
                    Ok(()) => {
                        a.redeeming = None;
                        a.cooldown_until = None;
                        // The shared windows start again; the report says by how much.
                        for w in a.windows.iter_mut().filter(|w| w.models.is_empty()) {
                            w.used = 0.0;
                        }
                        if let Some(c) = a.credits.iter_mut().find(|c| c.id == credit.id) {
                            c.remaining = c.remaining.saturating_sub(1);
                        }
                        a.credits.retain(|c| c.remaining > 0);
                    }
                    // Refused outright: nothing was spent.
                    Err(Failure::Refused(_)) => a.redeeming = None,
                    Err(Failure::Other(_)) => {}
                }
            }
        }
        self.touch();
        let result = result.map_err(|e| e.to_string());
        if result.is_ok() {
            self.sync(Some(id)).await;
        }
        result
    }

    /// When no account can serve: redeem a credit for one whose shared
    /// quota (not just a model's meter) ran out, if one has a credit it
    /// may use now. At most once a minute per account.
    async fn redeem_for(&self, provider: Provider, family: &str) -> Option<String> {
        let t = now();
        let id = {
            let p = self.pool.lock().unwrap();
            p.saved
                .accounts
                .iter()
                .filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && a.credit().is_some())
                .filter(|a| a.redeem_tried.is_none_or(|r| t - r >= 60))
                // Its own model meter would still block it after a reset.
                .filter(|a| !a.model_cooldowns.get(family).is_some_and(|&c| c > t))
                .filter(|a| !a.windows.iter().any(|w| !w.models.is_empty() && w.applies_to(family) && w.exhausted(t)))
                .filter(|a| a.cooldown_until.is_some_and(|c| c > t) || a.windows.iter().any(|w| w.models.is_empty() && w.exhausted(t)))
                .min_by_key(|a| a.credit().and_then(|c| c.expires_at).unwrap_or(i64::MAX))
                .map(|a| a.id.clone())
        }?;
        match self.redeem(&id).await {
            Ok(()) => Some(id),
            Err(e) => {
                tracing::warn!("pool: redeeming a reset credit: {e}");
                None
            }
        }
    }

    /// Redeem credits that would otherwise expire unused, each at its best
    /// moment (`Account::spend_before_expiry`). At most once a minute per
    /// account.
    async fn spend_expiring(&self) {
        let t = now();
        let due: Vec<String> = {
            let p = self.pool.lock().unwrap();
            p.saved
                .accounts
                .iter()
                .filter(|a| a.enabled && a.signed_out.is_none() && a.redeem_tried.is_none_or(|r| t - r >= 60))
                .filter(|a| a.spend_before_expiry(t))
                .map(|a| a.id.clone())
                .collect()
        };
        for id in due {
            match self.redeem(&id).await {
                Ok(()) => tracing::info!("pool: redeemed a reset credit before it expired"),
                Err(e) => tracing::warn!("pool: redeeming an expiring reset credit: {e}"),
            }
        }
    }

    /// Refresh sign-ins due within ten minutes and read the quota of
    /// every enabled account due a read (every `SYNC_EVERY`, longer after
    /// the provider asks the pool to slow down), or just `only`'s, now.
    pub async fn sync(&self, only: Option<&str>) {
        let t = now();
        let accounts: Vec<(String, Provider)> = {
            let p = self.pool.lock().unwrap();
            let due = self.due.lock().unwrap();
            p.saved
                .accounts
                .iter()
                .filter(|a| only.is_none_or(|o| o == a.id) && (only.is_some() || a.enabled) && a.signed_out.is_none())
                .filter(|a| only.is_some() || due.get(&a.id).is_none_or(|(at, _)| *at <= t))
                .map(|a| (a.id.clone(), a.provider))
                .collect()
        };
        for (id, provider) in accounts {
            let Ok(cred) = self.fresh(&id, provider, false, 600).await else { continue };
            let report = match upstream::usage(&self.client, &self.up, provider, &cred).await {
                Err(Failure::Refused(_)) => match self.fresh(&id, provider, true, 0).await {
                    Ok(c) => upstream::usage(&self.client, &self.up, provider, &c).await,
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
            let credits = match &report {
                Ok(_) if credits_due => {
                    self.credits_due.lock().unwrap().insert(id.clone(), now() + CREDITS_EVERY);
                    self.fresh(&id, provider, false, 60).await.ok()
                }
                _ => None,
            };
            let credits = match credits {
                Some(c) => upstream::credits(&self.client, &self.up, provider, &c).await.ok(),
                None => None,
            };
            let mut p = self.pool.lock().unwrap();
            let Some(a) = p.account_mut(&id) else { continue };
            if let Some(c) = credits {
                a.credits = c;
            }
            match report {
                Ok((windows, plan)) => {
                    // The report is the truth; a rejection's per-model
                    // window stays until it resets.
                    let t = now();
                    a.windows.retain(|w| w.name.starts_with("model:") && w.resets_at.is_some_and(|r| r > t));
                    a.merge_windows(windows);
                    if plan.is_some() {
                        a.plan = plan;
                    }
                    a.synced = Some(t);
                    if a.cooldown_until.is_some_and(|c| c <= t) {
                        a.cooldown_until = None;
                    }
                    a.model_cooldowns.retain(|_, &mut c| c > t);
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
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("pool-gateway").enable_all().build().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    *shared.runtime.lock().unwrap() = Some(runtime.handle().clone());
    std::thread::Builder::new()
        .name("pool-gateway".into())
        .spawn(move || {
            runtime.block_on(async move {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        let msg = format!("Can’t listen on {addr}: {e}");
                        shared.pool.lock().unwrap().error = Some(msg.clone());
                        shared.touch();
                        let _ = tx.send(Err(msg));
                        return;
                    }
                };
                let _ = tx.send(Ok(()));
                // The clients' proxy, beside it.
                let s = shared.clone();
                let proxy = match crate::authority::Authority::open() {
                    Ok(a) => {
                        let serving: crate::intercept::Serving = Arc::new(move |p| s.serves(p));
                        crate::intercept::serve(shared.clone(), Arc::new(a), crate::intercept::port(), serving, crate::intercept::Who::Clients).await.map(|_| ())
                    }
                    Err(e) => Err(format!("The pool’s certificate: {e}")),
                };
                if let Err(e) = proxy {
                    tracing::warn!("pool: {e}");
                    shared.pool.lock().unwrap().error = Some(e);
                    shared.touch();
                }
                let s = shared.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    loop {
                        s.sync(None).await;
                        s.spend_expiring().await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                });
                let app = router(shared.clone(), port);
                if let Err(e) = axum::serve(listener, app).await {
                    shared.pool.lock().unwrap().error = Some(format!("The gateway stopped: {e}"));
                    shared.touch();
                }
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
        .route("/pool/status", get(status))
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
    axum::Json(status_json(&ctx.shared.snapshot())).into_response()
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
                "reset_credits": a.credits.iter().map(|c| json!({"remaining": c.remaining, "usable": c.usable, "expires_at": c.expires_at})).collect::<Vec<_>>(),
                "blocked_until": schedule::blocked_until(a, "", t),
                "windows": a.windows, "today": a.today.1, "total": a.total,
                "in_flight": p.in_flight.get(&a.id).copied().unwrap_or(0),
                "last_used": a.last_used, "synced": a.synced, "recent": a.recent,
            })
        })
        .collect();
    json!({"accounts": accounts, "error": p.error, "loaded": p.loaded, "served": p.served, "open": p.open, "activity": p.activity.minutes})
}

async fn claude(State(ctx): State<Ctx>, Path(rest): Path<String>, RawQuery(q): RawQuery, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    proxy(ctx, Provider::Claude, rest, q, method, headers, body).await
}

async fn codex(State(ctx): State<Ctx>, Path(rest): Path<String>, RawQuery(q): RawQuery, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    proxy(ctx, Provider::Codex, rest, q, method, headers, body).await
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
            | "cookie"
            | "accept-encoding"
            | "origin"
            // The client's own account's (another, through the proxy).
            | "x-organization-uuid"
            | "anthropic-organization-id"
    ) && !name.starts_with("x-olo-pool-")
}

/// Response headers not passed back.
fn returned(name: &str) -> bool {
    !matches!(name, "content-length" | "connection" | "transfer-encoding" | "keep-alive" | "set-cookie" | "content-encoding")
}

/// The answer when no account can serve, shaped as the client expects.
fn unavailable(provider: Provider, status: StatusCode, message: &str, retry: Option<i64>) -> Response {
    let body = match provider {
        Provider::Claude => {
            let kind = if status == StatusCode::TOO_MANY_REQUESTS { "rate_limit_error" } else { "overloaded_error" };
            json!({"type": "error", "error": {"type": kind, "message": message}})
        }
        Provider::Codex => json!({"error": {"type": "usage_limit_reached", "message": message}}),
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
    serve(ctx.shared, provider, rest, query, method, headers, body).await
}

/// The most a request body is taken to be, decompressed, for a look
/// inside (a model request is far smaller).
const PLAIN_MAX: u64 = 64 << 20;

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

/// Serve a model request from the pool: `rest` is its path after the
/// provider's API base (`messages`, `responses`), from the gateway's own
/// address or the proxy (`intercept`).
pub async fn serve(s: Arc<Shared>, provider: Provider, rest: String, query: Option<String>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    // (The body goes on as it came; Codex compresses it on ChatGPT's
    // backend.)
    let model = serde_json::from_slice::<Value>(&plain(&headers, &body)).ok().and_then(|v| v["model"].as_str().map(str::to_owned)).unwrap_or_default();
    let family = model_family(&model);
    let url = {
        let base = match provider {
            Provider::Claude => format!("{}/v1/{rest}", s.up.claude),
            Provider::Codex => format!("{}/{rest}", s.up.codex),
        };
        match &query {
            Some(q) => format!("{base}?{q}"),
            None => base,
        }
    };
    let client = client_of(&headers);
    let mut tried: Vec<String> = Vec::new();
    loop {
        // Choose, and count it as busy at once.
        let chosen = {
            let mut p = s.pool.lock().unwrap();
            let t = now();
            let creds = &p.saved.credentials;
            let sticky = p.sticky.get(&(provider, family.clone())).cloned();
            let pick = schedule::pick(&p.saved.accounts, provider, &family, sticky.as_deref(), t, &tried, |a| creds.contains_key(&a.id)).map(|a| (a.id.clone(), a.name.clone()));
            match pick {
                Some((id, name)) => {
                    p.sticky.insert((provider, family.clone()), id.clone());
                    *p.in_flight.entry(id.clone()).or_default() += 1;
                    p.next_open += 1;
                    let open = p.next_open;
                    let o = Open { id: open, account: name.clone(), account_id: id.clone(), provider, model: model.clone(), client: client.clone(), started: now_ms() };
                    p.open.push(o);
                    Ok((id, name, open))
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
        let (id, name, open) = match chosen {
            Ok(c) => {
                s.touch();
                c
            }
            Err((any, retry)) => {
                // Every account out: a reset credit keeps things going.
                if any && let Some(id) = s.redeem_for(provider, &family).await {
                    tried.retain(|t| *t != id);
                    continue;
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
        // Records how this try ends, whichever way, when dropped.
        let mut finish = Finish {
            id: id.clone(),
            name,
            open,
            provider,
            model: model.clone(),
            client: client.clone(),
            started: std::time::Instant::now(),
            status: 0,
            outcome: Some(Outcome::Failed),
            ended: false,
            broken: false,
            scan: Scan::new(),
            shared: Some(s.clone()),
        };
        let mut cred = match s.fresh(&id, provider, false, 60).await {
            Ok(c) => c,
            Err(_) => {
                drop(finish);
                tried.push(id.clone());
                continue;
            }
        };
        let mut refreshed = false;
        let response = loop {
            let mut req = upstream::authorize(s.client.request(method.clone(), &url), provider, &cred);
            for (k, v) in headers.iter().filter(|(k, _)| forwarded(k.as_str())) {
                if provider == Provider::Claude && k == "anthropic-beta" {
                    continue;
                }
                req = req.header(k, v);
            }
            if provider == Provider::Claude {
                req = req.header("anthropic-beta", upstream::with_oauth_beta(headers.get("anthropic-beta").and_then(|v| v.to_str().ok())));
            }
            match req.body(body.clone()).send().await {
                Ok(r) if r.status() == StatusCode::UNAUTHORIZED && !refreshed => {
                    refreshed = true;
                    match s.fresh(&id, provider, true, 0).await {
                        Ok(c) => cred = c,
                        Err(_) => break None,
                    }
                }
                Ok(r) => break Some(r),
                Err(e) => {
                    // Not replayed: it may have reached the provider.
                    drop(finish);
                    return unavailable(provider, StatusCode::BAD_GATEWAY, &format!("The pool couldn’t reach {}: {e}", provider.client()), None);
                }
            }
        };
        let Some(r) = response else {
            finish.status = StatusCode::UNAUTHORIZED.as_u16();
            drop(finish);
            tried.push(id.clone());
            continue;
        };
        let status = r.status();
        finish.status = status.as_u16();
        let t = now();
        let (windows, rejection) = upstream::header_windows(r.headers(), provider, &family, status.as_u16(), t);
        {
            let mut p = s.pool.lock().unwrap();
            if let Some(a) = p.account_mut(&id) {
                a.merge_windows(windows);
                if status == StatusCode::UNAUTHORIZED {
                    a.signed_out = Some("Its provider refused the sign-in.".into());
                }
                if let Some(rej) = &rejection {
                    if rej.model_only {
                        a.model_cooldowns.insert(family.clone(), rej.until);
                    } else {
                        a.cooldown_until = Some(rej.until);
                    }
                }
            }
        }
        if status == StatusCode::UNAUTHORIZED || rejection.is_some() {
            // Nothing passed on yet: the next account may serve it.
            if rejection.is_some() {
                finish.outcome = Some(Outcome::RateLimited);
            }
            drop(finish);
            tried.push(id.clone());
            continue;
        }
        // Passed on: how it ends is up to the answer.
        finish.outcome = None;
        return respond(r, finish);
    }
}

/// One try at a request on one account: when dropped (the answer done,
/// the client gone, or the try given up) it frees the account, counts
/// what the answer reported and keeps how it ended.
struct Finish {
    id: String,
    name: String,
    open: u64,
    provider: Provider,
    model: String,
    client: String,
    started: std::time::Instant,
    status: u16,
    /// How it ended, when known before the answer (None: passed on).
    outcome: Option<Outcome>,
    /// The answer reached its end.
    ended: bool,
    /// The answer broke off upstream.
    broken: bool,
    scan: Scan,
    shared: Option<Arc<Shared>>,
}

impl Drop for Finish {
    fn drop(&mut self) {
        let Some(s) = self.shared.take() else { return };
        let passed = self.outcome.is_none();
        let outcome = self.outcome.unwrap_or(match self.status {
            _ if self.broken => Outcome::Failed,
            _ if !self.ended => Outcome::Cancelled,
            429 => Outcome::RateLimited,
            400.. => Outcome::Failed,
            _ => Outcome::Completed,
        });
        let mut usage = self.scan.finish();
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
        {
            let mut p = s.pool.lock().unwrap();
            if let Some(n) = p.in_flight.get_mut(&self.id) {
                *n = n.saturating_sub(1);
            }
            p.open.retain(|o| o.id != self.open);
            if let Some(a) = p.account_mut(&self.id) {
                if passed {
                    a.record(&usage, &today(), t);
                }
                a.finished(request.clone());
            }
            p.activity.record(&request);
            p.served.push_back(Served { account: self.name.clone(), account_id: self.id.clone(), provider: self.provider, request });
            while p.served.len() > 50 {
                p.served.pop_front();
            }
        }
        s.touch();
    }
}

fn respond(r: reqwest::Response, finish: Finish) -> Response {
    let status = r.status();
    let mut out = HeaderMap::new();
    for (k, v) in r.headers().iter().filter(|(k, _)| returned(k.as_str())) {
        out.append(k.clone(), v.clone());
    }
    if let Ok(v) = HeaderValue::from_str(&finish.name) {
        out.insert(HeaderName::from_static("x-olo-pool-account"), v);
    }
    let finish = Arc::new(Mutex::new(finish));
    finish.lock().unwrap().scan.sse = r.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("text/event-stream"));
    let end = finish.clone();
    let stream = r
        .bytes_stream()
        .map(move |chunk| {
            let mut f = finish.lock().unwrap();
            match &chunk {
                Ok(b) => f.scan.feed(b),
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
    resp
}

/// Reads the usage counts out of an answer as it passes, keeping nothing
/// else: server-sent events a line at a time, JSON whole (up to 8 MB).
struct Scan {
    sse: bool,
    pending: Vec<u8>,
    usage: Usage,
}

impl Scan {
    fn new() -> Scan {
        Scan { sse: false, pending: Vec::new(), usage: Usage::default() }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if !self.sse {
            if self.pending.len() + chunk.len() <= 8 << 20 {
                self.pending.extend_from_slice(chunk);
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
        }
    }

    fn line(&mut self, line: &[u8]) {
        let Some(data) = line.strip_prefix(b"data:") else { return };
        // Only lines that can hold usage are parsed.
        if !data.windows(5).any(|w| w == b"usage") {
            return;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(data.trim_ascii()) {
            self.event(&v);
        }
    }

    fn event(&mut self, v: &Value) {
        let u = match v["type"].as_str() {
            Some("message_start") => &v["message"]["usage"],
            Some("message_delta") => &v["usage"],
            Some("response.completed") | Some("response.done") => &v["response"]["usage"],
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
    }

    fn finish(&mut self) -> Usage {
        if !self.sse && !self.pending.is_empty() {
            if let Ok(v) = serde_json::from_slice::<Value>(&self.pending) {
                self.event(&v);
            }
        } else if self.sse && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.line(&rest);
        }
        self.pending.clear();
        self.usage.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn headers_kept_back() {
        for h in ["authorization", "x-api-key", "cookie", "host", "origin", "x-olo-pool-x"] {
            assert!(!forwarded(h), "{h}");
        }
        assert!(forwarded("anthropic-version") && forwarded("x-stainless-lang") && forwarded("session_id"));
        assert!(!returned("set-cookie") && returned("anthropic-ratelimit-unified-5h-utilization"));
    }
}

