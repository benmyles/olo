//! The worker that owns the local store (`vault.rs`): the UI sends jobs
//! and gets replies through the kit proxy, so opening the store (the
//! keyring may ask) and saving it never hold up a frame. Syncing with
//! 1Password runs on threads of its own (`sync.rs`) from a snapshot: a
//! plan comes back here and waits for the person's approval (unless it
//! changes nothing they'd see), then the approved changes are made and
//! recorded. A reconnect starts a new generation; replies from an older
//! one are dropped.

use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc,
    thread,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use chrono::{DateTime, Utc};
use onepassword::{AutofillBehavior, Error, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, ItemOverview, VaultOverview, Website};
use tabula_keychain::{Incoming, Request, Response};
use tabula_kit::Proxy;

use crate::{
    passkey,
    sync::{self, Plan, Report, Row},
    vault::Vault,
};

pub enum Job {
    /// Open the store (made the first time) and list it.
    Open,
    /// Let go of the store's secrets (locking); after a running sync.
    Close,
    Get { vault: String, item: String },
    Create(Box<ItemCreateParams>),
    Put(Box<Item>),
    Archive { vault: String, item: String },
    Delete { vault: String, item: String },
    /// Answer another app (NetPositive): `candidates` are the logins
    /// saved for its page; `vault` is where a new one goes.
    Keychain { incoming: Box<Incoming>, candidates: Vec<(String, String)>, vault: String },
    /// Work out what syncing with these 1Password accounts (id, name;
    /// the first takes items made here) would change, as one review
    /// (unless a sync is running).
    Sync { accounts: Vec<(String, String)> },
    /// Stop syncing with an account: its items go from here (never
    /// anything in 1Password).
    RemoveAccount { account: String },
    /// The plan worked out (sent by its thread).
    Planned(Result<Plan, String>),
    /// Make the changes of the plan the person reviewed.
    Approve { plan: u64 },
    /// The person said no: drop the plan.
    Decline,
    /// A sync finished (sent by its thread).
    SyncDone(Result<Report, String>),
    /// Erase the store here (never anything in 1Password).
    Reset,
}

/// How syncing with 1Password is going.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    Running,
    /// Changes waiting for the person's approval.
    Review { changes: usize, pending: usize },
    /// The person didn't approve the last plan.
    Declined { pending: usize },
    Done { at: DateTime<Utc>, received: usize, sent: usize, problems: Vec<crate::sync::Problem>, pending: usize },
    Failed { message: String, pending: usize },
}

pub enum Reply {
    Connected,
    Failed(String),
    /// The vaults, and which account each synced one is in.
    Vaults { vaults: Vec<VaultOverview>, accounts: HashMap<String, String> },
    Items { vault: String, items: Vec<ItemOverview> },
    /// A refresh finished.
    Loaded,
    Item(Box<Item>),
    /// Created or changed (`created`: it is new).
    Saved { item: Box<Item>, created: bool },
    Archived { item: String },
    Deleted { item: String },
    /// An item has a new id (1Password gave it one).
    Renamed { old: String, new: String },
    Sync(SyncState),
    /// A plan to review (`plan` is its id for [`Job::Approve`]).
    Review { plan: u64, rows: Vec<Row>, problems: Vec<crate::sync::Problem> },
    /// The store was erased.
    Reset,
    /// An operation failed (the store is still good).
    Error(String),
    /// A fetch failed: the item may be gone.
    Missing { item: String, error: String },
}

pub struct FromStore {
    pub generation: u64,
    pub reply: Reply,
}

pub struct Store {
    tx: mpsc::Sender<(u64, Job)>,
    pub generation: u64,
}

/// The 1Password app's socket, or a stand-in's (`onepassword-fake`) for tests.
pub fn socket() -> String {
    std::env::var("TABULA_1PASSWORD_SOCKET").unwrap_or_else(|_| onepassword::desktop::SOCKET.into())
}

/// What to tell the person.
pub fn describe(e: &Error) -> String {
    match e {
        Error::DesktopAppUnavailable => "1Password isn’t running, or isn’t letting apps in. Open 1Password, then turn on Settings ▸ Developer ▸ Integrate with other apps.".into(),
        Error::ConnectionDropped => "1Password closed the connection. If it asked whether to allow Passwords, choose Allow, then try again.".into(),
        other => other.to_string(),
    }
}

impl Store {
    pub fn new(proxy: Proxy) -> Store {
        let (tx, rx) = mpsc::channel::<(u64, Job)>();
        let back = tx.clone();
        thread::Builder::new()
            .name("passwords-store".into())
            .spawn(move || worker(rx, back, proxy))
            .expect("thread");
        Store { tx, generation: 0 }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send((self.generation, job));
    }

    /// Drops everything in flight; then opens the store.
    pub fn connect(&mut self) {
        self.generation += 1;
        self.send(Job::Open);
    }
}

fn refresh(v: &Vault, reply: &impl Fn(Reply)) {
    let vaults = v.vaults();
    reply(Reply::Vaults { vaults: vaults.clone(), accounts: v.data.vault_account.clone() });
    for vault in vaults {
        reply(Reply::Items { items: v.items(&vault.id), vault: vault.id });
    }
    reply(Reply::Loaded);
}

fn worker(rx: mpsc::Receiver<(u64, Job)>, back: mpsc::Sender<(u64, Job)>, proxy: Proxy) {
    let mut vault: Option<Vault> = None;
    let mut generation = 0;
    let mut syncing = false;
    let mut close_after_sync = false;
    // A plan waiting for approval, and the last plan's id.
    let mut waiting: Option<Plan> = None;
    let mut plans = 0u64;
    let mut queue: VecDeque<(u64, Job)> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());
        let Some((gen_of_job, job)) = queue.pop_front() else { continue };
        // A finished sync always lands: what it sent must be recorded.
        if gen_of_job < generation && !matches!(job, Job::SyncDone(_) | Job::Planned(_)) {
            continue;
        }
        // Only the latest selection's item is worth fetching.
        if matches!(job, Job::Get { .. }) && queue.iter().any(|(_, j)| matches!(j, Job::Get { .. })) {
            continue;
        }
        let reply = |reply: Reply| proxy.send(FromStore { generation: gen_of_job, reply });
        match job {
            Job::Open => {
                generation = generation.max(gen_of_job);
                close_after_sync = false;
                if vault.is_none() {
                    match Vault::open() {
                        Ok(v) => vault = Some(v),
                        Err(e) => {
                            reply(Reply::Failed(e.to_string()));
                            continue;
                        }
                    }
                }
                reply(Reply::Connected);
                refresh(vault.as_ref().expect("open"), &reply);
                continue;
            }
            Job::Close => {
                waiting = None;
                if syncing {
                    close_after_sync = true;
                } else {
                    vault = None;
                }
                continue;
            }
            Job::Reset => {
                if syncing {
                    reply(Reply::Error("Wait for the sync to finish, then reset.".into()));
                    continue;
                }
                waiting = None;
                // Even a store that can't be opened (damaged, its key lost)
                // can be erased.
                let done = match vault.as_mut() {
                    Some(v) => v.reset(),
                    None => match std::fs::remove_file(crate::vault::path()) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::Core(e.to_string())),
                        _ => Ok(()),
                    },
                };
                match done {
                    Ok(()) => reply(Reply::Reset),
                    Err(e) => reply(Reply::Error(format!("Couldn’t reset: {}", describe(&e)))),
                }
                continue;
            }
            Job::Planned(result) => {
                syncing = false;
                let fresh = gen_of_job >= generation;
                match vault.as_mut() {
                    Some(v) if fresh => match result {
                        Ok(plan) if plan.visible() => {
                            plans += 1;
                            let mut plan = plan;
                            plan.id = plans;
                            reply(Reply::Review { plan: plan.id, rows: plan.rows.clone(), problems: plan.problems.clone() });
                            reply(Reply::Sync(SyncState::Review { changes: plan.rows.len(), pending: v.pending() }));
                            waiting = Some(plan);
                        }
                        // Nothing the person would see: record what was
                        // looked at (and send any unchanged-looking edits).
                        Ok(plan) => {
                            syncing = true;
                            apply_plan(plan, gen_of_job, &back);
                        }
                        Err(message) => reply(Reply::Sync(SyncState::Failed { message, pending: v.pending() })),
                    },
                    _ => {}
                }
                if close_after_sync && !syncing {
                    close_after_sync = false;
                    vault = None;
                }
                continue;
            }
            Job::SyncDone(result) => {
                syncing = false;
                let Some(v) = vault.as_mut() else { continue };
                match result {
                    Ok(report) => {
                        let problems = report.problems;
                        match v.apply(report.changes) {
                            Ok(renamed) => {
                                for (old, new) in renamed {
                                    reply(Reply::Renamed { old, new });
                                }
                                refresh(v, &reply);
                                reply(Reply::Sync(SyncState::Done {
                                    at: Utc::now(),
                                    received: report.received,
                                    sent: report.sent,
                                    problems,
                                    pending: v.pending(),
                                }));
                            }
                            Err(e) => reply(Reply::Sync(SyncState::Failed { message: e.to_string(), pending: v.pending() })),
                        }
                    }
                    Err(message) => reply(Reply::Sync(SyncState::Failed { message, pending: v.pending() })),
                }
                if close_after_sync {
                    close_after_sync = false;
                    vault = None;
                }
                continue;
            }
            _ => {}
        }
        let Some(v) = vault.as_mut() else { continue };
        match job {
            Job::Open | Job::Close | Job::SyncDone(_) | Job::Planned(_) | Job::Reset => unreachable!(),
            Job::Sync { accounts } => {
                if syncing {
                    continue;
                }
                syncing = true;
                waiting = None;
                reply(Reply::Sync(SyncState::Running));
                let several = accounts.len() > 1;
                let snaps: Vec<_> = accounts
                    .iter()
                    .enumerate()
                    .map(|(i, (account, label))| {
                        let mut snap = v.snapshot(account, i == 0);
                        if several {
                            snap.label = label.clone();
                        }
                        snap
                    })
                    .collect();
                let back = back.clone();
                thread::Builder::new()
                    .name("onepassword-plan".into())
                    .spawn(move || {
                        let _ = back.send((gen_of_job, Job::Planned(sync::plan(&snaps, &socket()))));
                    })
                    .expect("thread");
            }
            Job::RemoveAccount { account } => {
                // A sync in flight would bring it back.
                if syncing {
                    reply(Reply::Error("Wait for the sync to finish, then remove the account.".into()));
                    continue;
                }
                if waiting.take().is_some() {
                    reply(Reply::Sync(SyncState::Declined { pending: v.pending() }));
                }
                match v.remove_account(&account) {
                    Ok(()) => refresh(v, &reply),
                    Err(e) => reply(Reply::Error(format!("Couldn’t remove the account: {}", describe(&e)))),
                }
            }
            Job::Approve { plan } => {
                // Only the plan that was shown.
                if syncing || waiting.as_ref().is_none_or(|p| p.id != plan) {
                    continue;
                }
                let Some(plan) = waiting.take() else { continue };
                syncing = true;
                reply(Reply::Sync(SyncState::Running));
                apply_plan(plan, gen_of_job, &back);
            }
            Job::Decline => {
                if waiting.take().is_some() {
                    reply(Reply::Sync(SyncState::Declined { pending: v.pending() }));
                }
            }

            Job::Get { vault, item } => match v.get(&vault, &item) {
                Ok(i) => reply(Reply::Item(Box::new(i))),
                Err(e) => reply(Reply::Missing { item, error: describe(&e) }),
            },
            Job::Create(params) => match v.create(&params) {
                Ok(i) => reply(Reply::Saved { item: Box::new(i), created: true }),
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", params.title, describe(&e)))),
            },
            Job::Put(item) => match v.put(&item) {
                Ok(i) => {
                    if i.id != item.id {
                        reply(Reply::Renamed { old: item.id.clone(), new: i.id.clone() });
                    }
                    reply(Reply::Saved { item: Box::new(i), created: false })
                }
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", item.title, describe(&e)))),
            },
            Job::Archive { vault, item } => match v.archive(&vault, &item) {
                Ok(()) => reply(Reply::Archived { item }),
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Delete { vault, item } => match v.delete(&vault, &item) {
                Ok(()) => reply(Reply::Deleted { item }),
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Keychain { incoming, candidates, vault } => {
                let response = keychain(v, &incoming.request, &candidates, &vault, &reply);
                incoming.reply(response);
            }
        }
    }
}

/// Make an approved plan's changes on a thread of its own.
fn apply_plan(plan: Plan, generation: u64, back: &mpsc::Sender<(u64, Job)>) {
    let back = back.clone();
    thread::Builder::new()
        .name("onepassword-sync".into())
        .spawn(move || {
            let _ = back.send((generation, Job::SyncDone(sync::apply(plan, &socket()))));
        })
        .expect("thread");
}

fn error(e: &Error) -> Response {
    Response::Error { message: describe(e) }
}

/// The username and password of a login, by the built-in fields or else
/// the first text and concealed ones (not a passkey's key).
fn login_fields(item: &Item) -> (Option<&ItemField>, Option<&ItemField>) {
    let user = item.field("username").or_else(|| {
        item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Text | ItemFieldType::Email) && f.section_id.is_none())
    });
    let password = item.field("password").or_else(|| {
        item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed && f.section_id.as_deref() != Some(passkey::SECTION))
    });
    (user, password)
}

fn keychain(c: &mut Vault, request: &Request, candidates: &[(String, String)], vault: &str, reply: &impl Fn(Reply)) -> Response {
    match request {
        Request::Fill { url, vault, id } => {
            let item = match c.get(vault, id) {
                Ok(i) => i,
                Err(e) => return error(&e),
            };
            // Only into a page the login is for, whatever the caller says.
            if !item.websites.iter().any(|w| fits(w, url)) {
                return Response::Error { message: format!("“{}” isn’t saved for this site.", item.title) };
            }
            let (user, password) = login_fields(&item);
            let otp = item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp).and_then(|f| crate::otp_code(&f.value));
            Response::Credentials {
                username: user.map(|f| f.value.clone()).unwrap_or_default(),
                password: password.map(|f| f.value.clone()).unwrap_or_default(),
                otp,
            }
        }
        Request::Check { username, password, .. } => {
            let mut changed = None;
            for (vault, id) in candidates {
                let Ok(item) = c.get(vault, id) else { continue };
                let (user, saved) = login_fields(&item);
                if !user.is_some_and(|u| u.value.eq_ignore_ascii_case(username)) {
                    continue;
                }
                if saved.is_some_and(|p| &p.value == password) {
                    return Response::Known;
                }
                changed.get_or_insert(Response::Changed { vault: vault.clone(), id: id.clone(), title: item.title.clone() });
            }
            changed.unwrap_or(Response::New)
        }
        Request::Save { url, title, username, password, replace } => {
            if let Some((vault, id)) = replace {
                let mut item = match c.get(vault, id) {
                    Ok(i) => i,
                    Err(e) => return error(&e),
                };
                let field = match login_fields(&item).1.map(|f| f.id.clone()) {
                    Some(fid) => item.field_mut(&fid),
                    None => {
                        item.fields.push(ItemField::new("password", "password", ItemFieldType::Concealed, ""));
                        item.fields.last_mut()
                    }
                };
                if let Some(f) = field {
                    f.value = password.clone();
                }
                return match c.put(&item) {
                    Ok(saved) => {
                        let title = saved.title.clone();
                        reply(Reply::Saved { item: Box::new(saved), created: false });
                        Response::Saved { title, updated: true }
                    }
                    Err(e) => error(&e),
                };
            }
            let mut params = ItemCreateParams::new(ItemCategory::Login, vault, title.clone());
            params.fields = vec![
                ItemField::new("username", "username", ItemFieldType::Text, username),
                ItemField::new("password", "password", ItemFieldType::Concealed, password),
            ];
            let site = url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| url.clone());
            params.websites = vec![Website {
                url: site,
                label: "website".into(),
                autofill_behavior: AutofillBehavior::AnywhereOnWebsite,
            }];
            match c.create(&params) {
                Ok(item) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created: true });
                    Response::Saved { title, updated: false }
                }
                Err(e) => error(&e),
            }
        }
        Request::Logins { url } => {
            let mut logins: Vec<tabula_keychain::Login> = candidates
                .iter()
                .filter_map(|(vault, id)| {
                    c.get(vault, id).map_err(|e| tracing::warn!("login {id} for a page: couldn't read it: {e}")).ok()
                })
                .map(|item| tabula_keychain::Login {
                    vault: item.vault_id.clone(),
                    id: item.id.clone(),
                    username: login_fields(&item).0.map(|f| f.value.clone()).unwrap_or_default(),
                    title: item.title.clone(),
                    otp: item.fields.iter().any(|f| f.field_type == ItemFieldType::Totp && !f.value.is_empty()),
                })
                .collect();
            // Nearest first (this host, then its parent domains, then the
            // site's other hosts), then by title.
            let near: std::collections::HashMap<String, u32> = candidates
                .iter()
                .filter_map(|(vault, id)| c.get(vault, id).ok())
                .map(|item| {
                    let best = item.websites.iter().filter(|w| fits(w, url)).filter_map(|w| tabula_keychain::closeness(&w.url, url)).min();
                    (item.id.clone(), best.unwrap_or(u32::MAX))
                })
                .collect();
            logins.sort_by_cached_key(|l| (near.get(&l.id).copied().unwrap_or(u32::MAX), l.title.to_lowercase(), l.username.to_lowercase()));
            Response::Logins { logins }
        }
        Request::Passkeys { url, rp_id, allow } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t use passkeys for {rp_id}.") };
            }
            let mut passkeys: Vec<tabula_keychain::Passkey> = candidates
                .iter()
                .filter_map(|(vault, id)| c.get(vault, id).ok())
                .filter_map(|item| {
                    let s = passkey::stored(&item)?;
                    let fits = s.rp_id.eq_ignore_ascii_case(rp_id) && (allow.is_empty() || allow.contains(&s.credential_id));
                    fits.then(|| tabula_keychain::Passkey {
                        vault: item.vault_id.clone(),
                        id: item.id.clone(),
                        title: item.title.clone(),
                        user_name: s.user_name.clone(),
                        credential_id: s.credential_id.clone(),
                    })
                })
                .collect();
            passkeys.sort_by_key(|p| (p.user_name.to_lowercase(), p.title.to_lowercase()));
            Response::Passkeys { passkeys }
        }
        Request::PasskeyCreate { url, rp_id, rp_name, user_id, user_name, exclude, .. } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t make passkeys for {rp_id}.") };
            }
            let items: Vec<Item> = candidates.iter().filter_map(|(vault, id)| c.get(vault, id).ok()).collect();
            let theirs = |i: &&Item| passkey::stored(i).is_some_and(|s| s.rp_id.eq_ignore_ascii_case(rp_id));
            if items.iter().filter(theirs).any(|i| passkey::stored(i).is_some_and(|s| exclude.contains(&s.credential_id))) {
                return Response::PasskeyExists;
            }
            // The same account's passkey is replaced; else it joins the
            // account's login, or gets a login of its own.
            let same_user = items.iter().filter(theirs).find(|i| passkey::stored(i).is_some_and(|s| &s.user_handle == user_id));
            let login = items.iter().find(|i| {
                passkey::stored(i).is_none()
                    && login_fields(i).0.is_some_and(|u| !u.value.is_empty() && u.value.eq_ignore_ascii_case(user_name))
            });
            let new = passkey::New::generate();
            let saved = match same_user.or(login) {
                Some(item) => {
                    let mut item = item.clone();
                    new.store(&mut item, rp_id, user_id, user_name);
                    c.put(&item).map(|i| (i, false))
                }
                None => {
                    let title = if rp_name.trim().is_empty() { rp_id.clone() } else { rp_name.trim().to_string() };
                    let mut params = ItemCreateParams::new(ItemCategory::Login, vault, title);
                    params.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, user_name)];
                    params.fields.extend(new.fields(rp_id, user_id, user_name));
                    params.sections = vec![passkey::section()];
                    let site = url::Url::parse(url)
                        .ok()
                        .filter(|u| u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(rp_id)))
                        .map(|u| u.origin().ascii_serialization())
                        .unwrap_or_else(|| format!("https://{rp_id}"));
                    params.websites =
                        vec![Website { url: site, label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
                    c.create(&params).map(|i| (i, true))
                }
            };
            match saved {
                Ok((item, created)) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created });
                    let auth_data = new.auth_data(rp_id);
                    Response::PasskeyCreated {
                        title,
                        credential_id: B64.encode(&new.credential_id),
                        attestation_object: B64.encode(passkey::attestation_object(&auth_data)),
                        authenticator_data: B64.encode(&auth_data),
                        public_key: B64.encode(new.public_key_der()),
                    }
                }
                Err(e) => error(&e),
            }
        }
        Request::PasskeySign { url, rp_id, vault, id, client_data_hash } => {
            if !tabula_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t use passkeys for {rp_id}.") };
            }
            let Ok(hash) = B64.decode(client_data_hash) else {
                return Response::Error { message: "The page’s request was malformed.".into() };
            };
            let item = match c.get(vault, id) {
                Ok(i) => i,
                Err(e) => return error(&e),
            };
            let Some(stored) = passkey::stored(&item).filter(|s| s.rp_id.eq_ignore_ascii_case(rp_id)) else {
                return Response::Error { message: format!("“{}” has no passkey for {rp_id}.", item.title) };
            };
            let Some(key) = stored.signing_key() else {
                return Response::Error { message: format!("The passkey in “{}” is damaged.", item.title) };
            };
            let auth_data = passkey::auth_data(rp_id, 0);
            Response::Assertion {
                credential_id: stored.credential_id.clone(),
                signature: B64.encode(passkey::sign(&key, &auth_data, &hash)),
                authenticator_data: B64.encode(&auth_data),
                user_handle: stored.user_handle.clone(),
            }
        }
        // Answered by the app itself.
        Request::Unlock => Response::Unlocked,
        // Answered on the UI thread.
        Request::Search { .. } | Request::Show { .. } => Response::Error { message: "unexpected".into() },
    }
}

/// Whether a login's website lets it fill the page at `url`.
pub fn fits(w: &Website, url: &str) -> bool {
    match w.autofill_behavior {
        AutofillBehavior::Never => false,
        AutofillBehavior::ExactDomain => {
            let parts = |u: &str| url::Url::parse(u).ok().map(|u| (u.host_str().map(str::to_ascii_lowercase), u.port_or_known_default()));
            tabula_keychain::matches(&w.url, url) && parts(&w.url).is_some() && parts(&w.url) == parts(url)
        }
        _ => tabula_keychain::matches(&w.url, url),
    }
}

#[cfg(test)]
mod tests {
    use onepassword::{AutofillBehavior, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, Website};
    use tabula_keychain::{Request, Response};

    use crate::vault::Vault;

    /// A page's logins come nearest first: its own host, its parent
    /// domains, then the site's other hosts.
    #[test]
    fn logins_nearest_first() {
        let dir = std::env::temp_dir().join(format!("tabula-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap();
        let vault = v.data.vaults[0].id.clone();
        let mut ids = Vec::new();
        for (title, site) in [("A mail", "https://mail.google.com"), ("B root", "https://google.com"), ("C accounts", "https://accounts.google.com")] {
            let mut p = ItemCreateParams::new(ItemCategory::Login, vault.clone(), title);
            p.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, "me")];
            p.websites = vec![Website { url: site.into(), label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
            ids.push((vault.clone(), v.create(&p).unwrap().id));
        }
        let request = Request::Logins { url: "https://accounts.google.com/signin".into() };
        let response = super::keychain(&mut v, &request, &ids, &vault, &|_| {});
        let Response::Logins { logins } = &response else { panic!("not logins") };
        let titles: Vec<&str> = logins.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["C accounts", "B root", "A mail"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
