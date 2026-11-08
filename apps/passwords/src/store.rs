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
use olo_keychain::{Incoming, Request, Response};
use olo_ui::Proxy;
use onepassword::{AutofillBehavior, Error, Item, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, ItemOverview, VaultOverview, Website};

use crate::{
    passkey,
    sync::{self, Plan, Report, Row},
    vault::Vault,
};

/// What a change is for, so its reply says (Edit ▸ Undo): done, undone or
/// redone, its step's name, and for an edit the item as it was.
#[derive(Clone, Debug)]
pub struct Tag {
    pub purpose: olo_ui::undo::Purpose,
    pub name: String,
    pub before: Option<Box<Item>>,
}

pub enum Job {
    /// Open the store (made the first time) and list it.
    Open,
    /// Let go of the store's secrets (locking); after a running sync.
    Close,
    Get {
        vault: String,
        item: String,
    },
    Create(Box<ItemCreateParams>, Option<Tag>),
    Put(Box<Item>, Option<Tag>),
    Archive {
        vault: String,
        item: String,
    },
    Delete {
        vault: String,
        item: String,
        tag: Option<Tag>,
    },
    /// Answer another app (Chrome's extension): `candidates` are the logins
    /// saved for its page (for a passkey to sign with: the one chosen);
    /// `vault` is where a new one goes.
    Keychain {
        incoming: Box<Incoming>,
        candidates: Vec<(String, String)>,
        vault: String,
    },
    /// Answer `op` (an allowed session's question).
    Cli {
        incoming: Box<Incoming>,
    },
    /// Work out what syncing with these 1Password accounts (id, name;
    /// the first takes items made here) would change, as one review
    /// (unless a sync is running).
    Sync {
        accounts: Vec<(String, String)>,
    },
    /// Stop syncing with an account: its items go from here (never
    /// anything in 1Password).
    RemoveAccount {
        account: String,
    },
    /// The plan worked out (sent by its thread).
    Planned(Result<Plan, String>),
    /// Make the changes of the plan the person reviewed.
    Approve {
        plan: u64,
    },
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
    Review {
        changes: usize,
        pending: usize,
    },
    /// The person didn't approve the last plan.
    Declined {
        pending: usize,
    },
    Done {
        at: DateTime<Utc>,
        received: usize,
        sent: usize,
        problems: Vec<crate::sync::Problem>,
        pending: usize,
    },
    Failed {
        message: String,
        pending: usize,
    },
}

pub enum Reply {
    Connected,
    Failed(String),
    /// The vaults, and which account each synced one is in.
    Vaults {
        vaults: Vec<VaultOverview>,
        accounts: HashMap<String, String>,
    },
    Items {
        vault: String,
        items: Vec<ItemOverview>,
    },
    /// A refresh finished.
    Loaded,
    /// What searching and filtering need of every item (no secrets).
    Index(HashMap<String, Searchable>),
    Item(Box<Item>),
    /// Created or changed (`created`: it is new), and what for.
    Saved {
        item: Box<Item>,
        created: bool,
        tag: Option<Tag>,
    },
    Archived {
        item: String,
    },
    /// Deleted: as it was, and what for.
    Deleted {
        item: String,
        was: Option<Box<Item>>,
        tag: Option<Tag>,
    },
    /// An item has a new id (1Password gave it one).
    Renamed {
        old: String,
        new: String,
    },
    Sync(SyncState),
    /// A plan to review (`plan` is its id for [`Job::Approve`]).
    Review {
        plan: u64,
        rows: Vec<Row>,
        problems: Vec<crate::sync::Problem>,
    },
    /// The store was erased.
    Reset,
    /// An operation failed (the store is still good).
    Error(String),
    /// A fetch failed: the item may be gone.
    Missing {
        item: String,
        error: String,
    },
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
    std::env::var("OLO_1PASSWORD_SOCKET").unwrap_or_else(|_| onepassword::desktop::SOCKET.into())
}

/// What to tell the person.
pub fn describe(e: &Error) -> String {
    match e {
        // (Running, its socket isn't there: the setting is off, as on a
        // new computer.)
        Error::DesktopAppUnavailable if onepassword::desktop::app_running() => {
            "1Password is open but isn’t letting other apps in. In 1Password, turn on Settings ▸ Developer ▸ Integrate with other apps, then try again.".into()
        }
        Error::DesktopAppUnavailable => "1Password isn’t running. Open it (and turn on Settings ▸ Developer ▸ Integrate with other apps), then try again.".into(),
        Error::ConnectionDropped => "1Password closed the connection. If it asked whether to allow Passwords, choose Allow, then try again.".into(),
        other => other.to_string(),
    }
}

impl Store {
    pub fn new(proxy: Proxy) -> Store {
        let (tx, rx) = mpsc::channel::<(u64, Job)>();
        let back = tx.clone();
        thread::Builder::new().name("passwords-store".into()).spawn(move || worker(rx, back, proxy)).expect("thread");
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
    reply(Reply::Index(index(v)));
    reply(Reply::Loaded);
}

/// What the list searches in an item besides its overview, and what its
/// filters ask of it. Never a secret: concealed fields, card numbers,
/// one-time codes and keys stay out, and of a password only whether it
/// is weak or used by another item too.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Searchable {
    pub username: String,
    /// Its sections' titles, its other fields' labels and values (a
    /// secret's label only) and its notes.
    pub text: String,
    /// Those fields one by one, to say where a search found something.
    pub fields: Vec<FieldText>,
    pub otp: bool,
    pub password: bool,
    pub files: bool,
    /// Its password is another item's too.
    pub reused: bool,
    /// Its password is short (under 12) or of one kind of character.
    pub weak: bool,
    /// Its passkey's public parts.
    pub passkey: Option<PasskeyInfo>,
}

/// One of an item's fields as search sees it: its section's title (none
/// for the built-in ones and untitled sections), its label, and its
/// value unless that's secret.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldText {
    pub id: String,
    pub section: String,
    pub label: String,
    pub value: String,
}

/// What may be said of a passkey while Passwords is unlocked (its key
/// stays in the store).
#[derive(Clone, Debug, PartialEq)]
pub struct PasskeyInfo {
    pub rp_id: String,
    pub user_name: String,
    pub credential_id: String,
}

/// Whether `pw` is weak: short, or only one kind of character.
fn weak(pw: &str) -> bool {
    let kinds = [pw.chars().any(|c| c.is_lowercase()), pw.chars().any(|c| c.is_uppercase()), pw.chars().any(|c| c.is_ascii_digit()), pw.chars().any(|c| !c.is_alphanumeric())];
    pw.chars().count() < 12 || kinds.iter().filter(|&&k| k).count() < 2
}

/// Every item's [`Searchable`].
fn index(v: &Vault) -> HashMap<String, Searchable> {
    let active = || v.data.entries.iter().filter(|e| e.state == onepassword::ItemState::Active);
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for e in active() {
        if let Some(pw) = e.item.password() {
            *uses.entry(pw).or_default() += 1;
        }
    }
    let secret = |t: ItemFieldType| matches!(t, ItemFieldType::Concealed | ItemFieldType::Totp | ItemFieldType::CreditCardNumber | ItemFieldType::SshKey);
    v.data
        .entries
        .iter()
        .map(|e| {
            let item = &e.item;
            let mut text = String::new();
            for section in item.sections.iter().filter(|s| !s.title.is_empty() && item.fields.iter().any(|f| f.section_id.as_ref() == Some(&s.id))) {
                text.push_str(&section.title);
                text.push('\n');
            }
            let fields: Vec<FieldText> = item
                .fields
                .iter()
                .filter(|f| f.id != "username")
                .map(|f| FieldText {
                    id: f.id.clone(),
                    section: f.section_id.as_ref().and_then(|id| item.sections.iter().find(|s| &s.id == id)).map(|s| s.title.clone()).unwrap_or_default(),
                    label: f.title.clone(),
                    value: if secret(f.field_type) { String::new() } else { f.value.clone() },
                })
                .collect();
            for f in &fields {
                text.push_str(&f.label);
                text.push('\n');
                text.push_str(&f.value);
                text.push('\n');
            }
            text.push_str(&item.notes);
            let pw = item.password();
            let s = Searchable {
                username: item.username().unwrap_or_default().to_owned(),
                text,
                fields,
                otp: item.fields.iter().any(|f| f.field_type == ItemFieldType::Totp && !f.value.is_empty()),
                password: pw.is_some(),
                files: !item.files.is_empty() || item.document.is_some(),
                reused: pw.is_some_and(|p| uses.get(p).copied().unwrap_or(0) > 1),
                weak: pw.is_some_and(weak),
                passkey: passkey::stored(item).map(|p| PasskeyInfo { rp_id: p.rp_id.clone(), user_name: p.user_name.clone(), credential_id: p.credential_id.clone() }),
            };
            (item.id.clone(), s)
        })
        .collect()
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
    // What's shared with agent screens, written again after anything that
    // may have changed it (an unchanged copy isn't).
    let mut written: HashMap<std::path::PathBuf, u64> = HashMap::new();
    let mut copies_due = false;
    loop {
        if std::mem::take(&mut copies_due)
            && let Some(v) = vault.as_ref().filter(|v| !v.is_shared())
        {
            v.write_copies(&mut written);
        }
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
        copies_due = !matches!(job, Job::Get { .. } | Job::Close | Job::Cli { .. } | Job::Sync { .. } | Job::Decline);
        // What's shared with an agent screen is read only.
        if vault.as_ref().is_some_and(Vault::is_shared)
            && matches!(job, Job::Create(..) | Job::Put(..) | Job::Archive { .. } | Job::Delete { .. } | Job::Sync { .. } | Job::RemoveAccount { .. } | Job::Approve { .. } | Job::Reset)
        {
            proxy.send(FromStore { generation: gen_of_job, reply: Reply::Error("Shared with this screen: change it in your own Passwords.".into()) });
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
                                reply(Reply::Sync(SyncState::Done { at: Utc::now(), received: report.received, sent: report.sent, problems, pending: v.pending() }));
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
            Job::Create(params, tag) => match v.create(&params) {
                Ok(i) => {
                    reply(Reply::Saved { item: Box::new(i), created: true, tag });
                    reply(Reply::Index(index(v)));
                }
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", params.title, describe(&e)))),
            },
            Job::Put(item, tag) => match v.put(&item) {
                Ok(i) => {
                    if i.id != item.id {
                        reply(Reply::Renamed { old: item.id.clone(), new: i.id.clone() });
                    }
                    reply(Reply::Saved { item: Box::new(i), created: false, tag });
                    reply(Reply::Index(index(v)));
                }
                Err(e) => reply(Reply::Error(format!("Couldn’t save “{}”: {}", item.title, describe(&e)))),
            },
            Job::Archive { vault, item } => match v.archive(&vault, &item) {
                Ok(()) => {
                    reply(Reply::Archived { item });
                    reply(Reply::Index(index(v)));
                }
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Delete { vault, item, tag } => match v.delete(&vault, &item) {
                Ok(was) => {
                    reply(Reply::Deleted { item, was: Some(Box::new(was)), tag });
                    reply(Reply::Index(index(v)));
                }
                Err(e) => reply(Reply::Error(describe(&e))),
            },
            Job::Keychain { incoming, candidates, vault } => {
                let response = keychain(v, &incoming.request, &candidates, &vault, &reply);
                // A login or passkey saved: searched and offered from now.
                if matches!(response, Response::Saved { .. } | Response::PasskeyCreated { .. }) {
                    reply(Reply::Index(index(v)));
                }
                incoming.reply(response);
            }
            Job::Cli { incoming } => {
                let response = match &incoming.request {
                    Request::Cli { query } => crate::cli::answer(&v.data, query),
                    _ => Response::Error { message: "unexpected".into() },
                };
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

/// A credit card's fields, as a card form takes them.
#[derive(Default)]
struct CardFields {
    /// Digits only.
    number: String,
    cardholder: String,
    brand: String,
    /// "08" and "2029".
    month: String,
    year: String,
    code: String,
}

/// A card's fields: 1Password's built-in ones by their ids (`ccnum`,
/// `cardholder`, `type`, `expiry`, `cvv`), else by their types and labels
/// (an older card's, or one made by hand).
fn card_fields(item: &Item) -> CardFields {
    let by = |id: &str, kind: Option<ItemFieldType>, words: &[&str]| {
        item.field(id)
            .or_else(|| kind.and_then(|k| item.fields.iter().find(|f| f.field_type == k)))
            .or_else(|| item.fields.iter().find(|f| words.iter().any(|w| f.title.to_lowercase().contains(w))))
            .map(|f| f.value.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let number: String = by("ccnum", Some(ItemFieldType::CreditCardNumber), &["card number"]).unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    let (month, year) = by("expiry", Some(ItemFieldType::MonthYear), &["expir", "valid thru", "valid until"])
        .and_then(|v| onepassword::fields::month_year(&v))
        .and_then(|my| my.split_once('/').map(|(m, y)| (m.to_owned(), y.to_owned())))
        .unwrap_or_default();
    CardFields {
        number,
        cardholder: by("cardholder", None, &["cardholder", "name on card", "card holder"]).unwrap_or_default(),
        brand: by("type", Some(ItemFieldType::CreditCardType), &["brand", "card type"]).unwrap_or_default(),
        month,
        year,
        code: by("cvv", None, &["verification", "cvv", "cvc", "security code"]).unwrap_or_default(),
    }
}

/// The username and password of a login, by the built-in fields or else
/// the first text and concealed ones (not a passkey's key, which
/// 1Password keeps in a section of that name).
fn login_fields(item: &Item) -> (Option<&ItemField>, Option<&ItemField>) {
    let user = item.field("username").or_else(|| item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Text | ItemFieldType::Email) && f.section_id.is_none()));
    let password = item.field("password").or_else(|| item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed && f.section_id.as_deref() != Some(passkey::SECTION)));
    (user, password)
}

fn keychain(c: &mut Vault, request: &Request, candidates: &[(String, String)], vault: &str, reply: &impl Fn(Reply)) -> Response {
    let b64 = base64::engine::general_purpose::STANDARD;
    match request {
        // The SSH agent's (`agent`): the keys never leave this thread.
        Request::SshKeys => Response::SshKeys { keys: crate::agent::keys(&c.data) },
        Request::SshSign { key, data, flags } => {
            let (Ok(key), Ok(data)) = (b64.decode(key), b64.decode(data)) else {
                return Response::Error { message: "not base64".into() };
            };
            match crate::agent::sign(&c.data, &key, &data, *flags) {
                Ok((signature, title)) => {
                    tracing::info!(key = title, "signed for the SSH agent");
                    Response::SshSignature { signature: b64.encode(signature) }
                }
                Err(message) => Response::Error { message },
            }
        }
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
            Response::Credentials { username: user.map(|f| f.value.clone()).unwrap_or_default(), password: password.map(|f| f.value.clone()).unwrap_or_default(), otp }
        }
        Request::Cards { .. } => {
            let mut cards: Vec<olo_keychain::Card> = candidates
                .iter()
                .filter_map(|(vault, id)| c.get(vault, id).ok())
                .map(|item| {
                    let f = card_fields(&item);
                    olo_keychain::Card {
                        vault: item.vault_id.clone(),
                        id: item.id.clone(),
                        title: item.title.clone(),
                        cardholder: f.cardholder,
                        brand: f.brand,
                        last4: f.number[f.number.len().saturating_sub(4)..].to_owned(),
                        expiry: if f.month.is_empty() { String::new() } else { format!("{}/{}", f.month, f.year) },
                    }
                })
                .collect();
            cards.sort_by_cached_key(|c| (c.title.to_lowercase(), c.id.clone()));
            Response::Cards { cards }
        }
        Request::FillCard { vault, id, .. } => {
            // (Only a card: the candidates are the one chosen, if it is.)
            if !candidates.iter().any(|(v, i)| v == vault && i == id) {
                return Response::Error { message: "That isn’t a credit card in Passwords.".into() };
            }
            let item = match c.get(vault, id) {
                Ok(i) => i,
                Err(e) => return error(&e),
            };
            let f = card_fields(&item);
            if f.number.is_empty() {
                return Response::Error { message: format!("“{}” has no card number.", item.title) };
            }
            Response::CardDetails { number: f.number, cardholder: f.cardholder, expiry_month: f.month, expiry_year: f.year, code: f.code, brand: f.brand }
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
                        reply(Reply::Saved { item: Box::new(saved), created: false, tag: None });
                        Response::Saved { title, updated: true }
                    }
                    Err(e) => error(&e),
                };
            }
            let mut params = ItemCreateParams::new(ItemCategory::Login, vault, title.clone());
            params.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, username), ItemField::new("password", "password", ItemFieldType::Concealed, password)];
            let site = url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| url.clone());
            params.websites = vec![Website { url: site, label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
            match c.create(&params) {
                Ok(item) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created: true, tag: None });
                    Response::Saved { title, updated: false }
                }
                Err(e) => error(&e),
            }
        }
        Request::Logins { url } => {
            let mut logins: Vec<olo_keychain::Login> = candidates
                .iter()
                .filter_map(|(vault, id)| c.get(vault, id).map_err(|e| tracing::warn!("login {id} for a page: couldn't read it: {e}")).ok())
                .map(|item| olo_keychain::Login {
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
                    let best = item.websites.iter().filter(|w| fits(w, url)).filter_map(|w| olo_keychain::closeness(&w.url, url)).min();
                    (item.id.clone(), best.unwrap_or(u32::MAX))
                })
                .collect();
            logins.sort_by_cached_key(|l| (near.get(&l.id).copied().unwrap_or(u32::MAX), l.title.to_lowercase(), l.username.to_lowercase()));
            Response::Logins { logins }
        }
        Request::PasskeyCreate { url, rp_id, rp_name, user_id, user_name, exclude, .. } => {
            if !olo_keychain::rp_id_fits(rp_id, url) {
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
            let login = items.iter().find(|i| passkey::stored(i).is_none() && login_fields(i).0.is_some_and(|u| !u.value.is_empty() && u.value.eq_ignore_ascii_case(user_name)));
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
                    params.websites = vec![Website { url: site, label: "website".into(), autofill_behavior: AutofillBehavior::AnywhereOnWebsite }];
                    c.create(&params).map(|i| (i, true))
                }
            };
            match saved {
                Ok((item, created)) => {
                    let title = item.title.clone();
                    reply(Reply::Saved { item: Box::new(item), created, tag: None });
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
        Request::PasskeySign { url, rp_id, client_data_hash, .. } => {
            if !olo_keychain::rp_id_fits(rp_id, url) {
                return Response::Error { message: format!("This page can’t use passkeys for {rp_id}.") };
            }
            let Some((vault, id)) = candidates.first() else { return Response::NoPasskey };
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
        // What's shared with agent screens (the app checked who asks).
        Request::Shares => shares(c),
        Request::SetShares { everywhere, screen } => {
            let shares = &mut c.data.shares;
            if let Some(ids) = everywhere {
                shares.everywhere = dedup(ids);
            }
            if let Some((name, s)) = screen {
                let s = olo_keychain::ScreenShares { add: dedup(&s.add), remove: dedup(&s.remove) };
                if s.is_empty() {
                    shares.screens.remove(name);
                } else {
                    shares.screens.insert(name.clone(), s);
                }
            }
            match c.save() {
                Ok(()) => Response::Shared,
                Err(e) => error(&e),
            }
        }
        // Answered by the app itself.
        Request::Unlock => Response::Unlocked,
        // Answered on the UI thread.
        Request::Search { .. } | Request::Show { .. } | Request::Cli { .. } | Request::Passkeys { .. } => Response::Error { message: "unexpected".into() },
    }
}

/// Every item that could be shared with agent screens (no secrets), and
/// what's shared.
fn shares(c: &Vault) -> Response {
    let vault_title = |id: &str| c.data.vaults.iter().find(|v| v.id == id).map(|v| v.title.clone()).unwrap_or_default();
    let mut items: Vec<olo_keychain::Shareable> = c
        .data
        .entries
        .iter()
        .filter(|e| e.state == onepassword::ItemState::Active)
        .map(|e| {
            let i = &e.item;
            let subtitle = i.username().filter(|u| !u.is_empty()).map(str::to_owned).or_else(|| i.websites.first().and_then(|w| olo_keychain::host(&w.url))).unwrap_or_default();
            olo_keychain::Shareable { vault: i.vault_id.clone(), vault_title: vault_title(&i.vault_id), id: i.id.clone(), title: i.title.clone(), subtitle, kind: i.category.label().to_string() }
        })
        .collect();
    items.sort_by_cached_key(|i| (i.title.to_lowercase(), i.id.clone()));
    Response::Shares { items, everywhere: c.data.shares.everywhere.clone(), screens: c.data.shares.screens.clone() }
}

/// `ids` once each, in their order.
fn dedup(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().filter(|id| seen.insert(id.as_str())).cloned().collect()
}

/// Whether a login's website lets it fill the page at `url`.
pub fn fits(w: &Website, url: &str) -> bool {
    match w.autofill_behavior {
        AutofillBehavior::Never => false,
        AutofillBehavior::ExactDomain => {
            let parts = |u: &str| url::Url::parse(u).ok().map(|u| (u.host_str().map(str::to_ascii_lowercase), u.port_or_known_default()));
            olo_keychain::matches(&w.url, url) && parts(&w.url).is_some() && parts(&w.url) == parts(url)
        }
        _ => olo_keychain::matches(&w.url, url),
    }
}

#[cfg(test)]
mod tests {
    use olo_keychain::{Request, Response};
    use onepassword::{AutofillBehavior, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, Website};

    use crate::vault::Vault;

    /// A page's logins come nearest first: its own host, its parent
    /// domains, then the site's other hosts.
    #[test]
    fn logins_nearest_first() {
        let dir = std::env::temp_dir().join(format!("olo-store-test-{}", std::process::id()));
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

    /// A page's card menu gets each card's title, name, brand, last four
    /// digits and expiry, never its number or code; filling gets those,
    /// and only for a card.
    #[test]
    fn cards_for_a_card_form() {
        let dir = std::env::temp_dir().join(format!("olo-store-cards-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap();
        let vault = v.data.vaults[0].id.clone();
        let mut p = ItemCreateParams::new(ItemCategory::CreditCard, vault.clone(), "Travel Visa");
        p.fields = vec![
            ItemField::new("cardholder", "cardholder name", ItemFieldType::Text, "Jane Doe"),
            ItemField::new("type", "type", ItemFieldType::CreditCardType, "Visa"),
            ItemField::new("ccnum", "number", ItemFieldType::CreditCardNumber, "4111 1111 1111 1234"),
            ItemField::new("cvv", "verification number", ItemFieldType::Concealed, "321"),
            ItemField::new("expiry", "expiry date", ItemFieldType::MonthYear, "08/2029"),
        ];
        let card = v.create(&p).unwrap().id;
        let mut l = ItemCreateParams::new(ItemCategory::Login, vault.clone(), "Mail");
        l.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, "me")];
        let login = v.create(&l).unwrap().id;
        let cards = vec![(vault.clone(), card.clone())];
        let r = super::keychain(&mut v, &Request::Cards { url: "https://shop.example/pay".into() }, &cards, &vault, &|_| {});
        let Response::Cards { cards: listed } = &r else { panic!("not cards") };
        assert_eq!(listed.len(), 1);
        let c = &listed[0];
        assert_eq!((c.title.as_str(), c.cardholder.as_str(), c.brand.as_str(), c.last4.as_str(), c.expiry.as_str()), ("Travel Visa", "Jane Doe", "Visa", "1234", "08/2029"));
        assert!(!serde_json::to_string(&r).unwrap().contains("4111"), "never the number");
        let mut fill = |id: &str, candidates: &[(String, String)]| {
            super::keychain(&mut v, &Request::FillCard { url: "https://shop.example/pay".into(), vault: vault.clone(), id: id.into() }, candidates, &vault, &|_| {})
        };
        match &fill(&card, &cards) {
            Response::CardDetails { number, cardholder, expiry_month, expiry_year, code, brand } => {
                assert_eq!(
                    (number.as_str(), cardholder.as_str(), expiry_month.as_str(), expiry_year.as_str(), code.as_str(), brand.as_str()),
                    ("4111111111111234", "Jane Doe", "08", "2029", "321", "Visa")
                );
            }
            other => panic!("{other:?}"),
        }
        // A login isn't a card, whatever the extension asks.
        assert!(matches!(fill(&login, &cards), Response::Error { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Search sees each field with its section's title, a secret's
    /// label but never its value, and sections by their titles.
    #[test]
    fn search_sees_sections_and_fields() {
        let dir = std::env::temp_dir().join(format!("olo-store-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap();
        let mut p = ItemCreateParams::new(ItemCategory::Server, v.data.vaults[0].id.clone(), "Box");
        p.sections = vec![onepassword::ItemSection { id: "admin".into(), title: "Admin Console".into() }, onepassword::ItemSection { id: "empty".into(), title: "Unused".into() }];
        p.fields = vec![
            ItemField::new("username", "username", ItemFieldType::Text, "deploy"),
            ItemField::new("url", "admin console URL", ItemFieldType::Text, "console.example.net").in_section("admin"),
            ItemField::new("pw", "console password", ItemFieldType::Concealed, "Quarry-Fable-88").in_section("admin"),
        ];
        let id = v.create(&p).unwrap().id;
        let index = super::index(&v);
        let s = &index[&id];
        let fields: Vec<(&str, &str, &str)> = s.fields.iter().map(|f| (f.section.as_str(), f.label.as_str(), f.value.as_str())).collect();
        assert_eq!(fields, [("Admin Console", "admin console URL", "console.example.net"), ("Admin Console", "console password", "")]);
        assert!(s.text.contains("Admin Console") && s.text.contains("console password"));
        assert!(!s.text.contains("Quarry") && !s.text.contains("Unused"), "{}", s.text);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
