//! Passwords: logins and other secrets kept on this computer, encrypted
//! (`vault.rs`), and synced both ways with a 1Password account if asked
//! (`sync.rs`, through `tabula-onepassword`). Locked with Touch ID.

mod edit;
mod generator;
mod lock;
mod passkey;
mod store;
mod sync;
mod vault;
mod view;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use onepassword::{Item, ItemCategory, ItemFieldType, ItemOverview, ItemState, VaultOverview, totp::Totp};
use serde::{Deserialize, Serialize};
use tabula_keychain::{Incoming, Request, Response};
use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key},
    instance::{self, Launch},
};

use crate::{
    edit::{EditAction, Editor},
    generator::Generator,
    lock::{Lock, LockMsg},
    store::{FromStore, Job, Reply, Store, SyncState},
};

const APP_ID: &str = "tabula.passwords";

/// A 1Password account Passwords syncs with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Linked {
    /// Its UUID (or its name in 1Password's sidebar, if typed).
    pub id: String,
    /// How to name it to the person.
    pub label: String,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The 1Password accounts to sync with; the first takes items made
    /// here. None: no syncing.
    accounts: Vec<Linked>,
    /// Chosen to keep items only on this computer.
    local_only: bool,
    /// From when there was one account (read once into `accounts`).
    #[serde(skip_serializing)]
    account: Option<String>,
    #[serde(skip_serializing)]
    account_label: Option<String>,
    /// Lock after this long without use (0: only when asked).
    lock_minutes: u32,
    /// Take copied secrets off the clipboard after this long.
    clear_seconds: u32,
    generator: generator::Recipe,
    size: Option<(u32, u32)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { accounts: Vec::new(), local_only: false, account: None, account_label: None, lock_minutes: 10, clear_seconds: 90, generator: Default::default(), size: None }
    }
}

fn settings_path() -> std::path::PathBuf {
    tabula_kit::config_dir().join("passwords.json")
}

/// Where the list's items come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    All,
    Vault(String),
    Category(ItemCategory),
    Archive,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    /// Asking which account.
    Setup,
    Connecting,
    Ready,
    Failed(String),
}

enum Alert {
    ConfirmReset,
    /// Stop syncing with `removed`, then sync with `keep`.
    ConfirmRemove { removed: Vec<Linked>, keep: Vec<Linked> },
    ConfirmDelete { vault: String, item: String, title: String },
    DiscardChanges { then: Box<Then> },
    Message(String),
}

/// What to do once changes are discarded.
enum Then {
    Select(Option<String>),
    Source(Source),
    New(ItemCategory),
    Close,
    Lock,
}

/// Saved for an in-place upgrade.
#[derive(Serialize, Deserialize)]
struct Saved {
    source: Source,
    selected: Option<String>,
    search: String,
}

/// The accounts in the 1Password app, for the setup panel.
pub enum Accounts {
    Unknown,
    Finding,
    Found(Vec<onepassword::accounts::Account>),
}

/// What removing accounts does, for the alert.
fn remove_message(removed: &[Linked]) -> String {
    let names = removed.iter().map(|l| format!("“{}”", l.label)).collect::<Vec<_>>().join(" and ");
    let its = if removed.len() == 1 { "Its" } else { "Their" };
    format!("Stop syncing with {names}? {its} items are removed from this computer, along with changes made here and not yet sent. Nothing in 1Password changes.")
}

const RESET: &str = "Erase everything Passwords keeps on this computer and start again? Its items, changes not yet sent to 1Password, and the choice of account all go. Nothing in 1Password is changed. This can’t be undone.";

/// A sync's changes, waiting for the person's approval.
pub struct Review {
    pub plan: u64,
    pub rows: Vec<sync::Row>,
    pub problems: Vec<sync::Problem>,
    /// The Review Sync window, while it's open.
    pub window: Option<WindowId>,
}

/// Ticks for the auto-lock, syncing and one-time-password countdowns.
struct Tick;

/// How often to sync while unlocked, and how soon after a change here.
const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const SYNC_AFTER_CHANGE: Duration = Duration::from_secs(3);

pub struct Passwords {
    settings: Settings,
    store: Store,
    conn: Conn,
    /// The setup's choices: listed accounts ticked, and one named by hand.
    pub chosen: Vec<String>,
    account_field: String,
    accounts: Accounts,
    /// Naming an account by hand rather than choosing a listed one.
    other_account: bool,
    vaults: Vec<VaultOverview>,
    /// Which account each synced vault is in.
    pub vault_accounts: HashMap<String, String>,
    items: Vec<ItemOverview>,
    loading: bool,
    details: HashMap<String, Item>,
    missing: HashMap<String, String>,
    source: Source,
    selected: Option<String>,
    reveal_selection: bool,
    search: String,
    focus_search: bool,
    revealed: HashSet<String>,
    editor: Option<Editor>,
    lock: Lock,
    main: Option<WindowId>,
    generator: Option<(WindowId, Generator)>,
    alerts: HashMap<WindowId, Alert>,
    status: Option<(String, Instant)>,
    last_use: Instant,
    /// Just unlocked: drop the password field's keyboard focus.
    pub unlocked: bool,
    /// Other apps' requests waiting for an unlock or the item list.
    pub waiting: Vec<Incoming>,
    /// Syncing with 1Password: how the last one went.
    pub sync: Option<SyncState>,
    /// When the last sync was asked for, and when the next is due early.
    pub sync_asked: Option<Instant>,
    sync_due: Option<Instant>,
    /// The latest sync's changes, until applied or declined.
    pub review: Option<Review>,
    /// The Sync Problems window and what it lists.
    pub problems: Option<(WindowId, Vec<sync::Problem>)>,
    /// Open the review as soon as the sync asked for has one (the person
    /// asked; automatic syncs only say so in the status line).
    pub show_review: bool,
}

fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).or_else(|_| url::Url::parse(&format!("https://{url}"))).ok()?;
    Some(parsed.host_str()?.trim_start_matches("www.").to_string())
}

impl Passwords {
    /// A vault's name, with its account's when there are several.
    fn vault_title(&self, id: &str) -> String {
        self.vaults.iter().find(|v| v.id == id).map_or(String::new(), |v| self.vault_name(v))
    }

    pub fn vault_name(&self, v: &VaultOverview) -> String {
        match self.vault_accounts.get(&v.id).and_then(|a| self.settings.accounts.iter().find(|l| &l.id == a)) {
            Some(l) if self.settings.accounts.len() > 1 => format!("{} ({})", v.title, l.label),
            _ => v.title.clone(),
        }
    }

    fn overview(&self, id: &str) -> Option<&ItemOverview> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The list for the current source and search, sorted by title.
    pub fn visible(&self) -> Vec<&ItemOverview> {
        let words: Vec<String> = self.search.split_whitespace().map(str::to_lowercase).collect();
        let mut list: Vec<&ItemOverview> = self
            .items
            .iter()
            .filter(|i| match &self.source {
                Source::Archive => i.state == ItemState::Archived,
                _ if i.state == ItemState::Archived => false,
                Source::All => true,
                Source::Vault(v) => &i.vault_id == v,
                Source::Category(c) => i.category == *c,
            })
            .filter(|i| {
                words.iter().all(|w| {
                    i.title.to_lowercase().contains(w)
                        || i.websites.iter().any(|s| s.url.to_lowercase().contains(w))
                        || i.tags.iter().any(|t| t.to_lowercase().contains(w))
                        || self.details.get(&i.id).and_then(|d| d.username()).is_some_and(|u| u.to_lowercase().contains(w))
                })
            })
            .collect();
        list.sort_by_cached_key(|i| (i.title.to_lowercase(), i.id.clone()));
        list
    }

    /// The second line under an item's title.
    pub fn subtitle(&self, item: &ItemOverview) -> String {
        if let Some(h) = item.websites.first().and_then(|w| host(&w.url)) {
            return h;
        }
        item.category.label().to_string()
    }

    fn select(&mut self, cx: &mut Cx, id: Option<String>) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) && self.selected != id {
            self.ask_discard(cx, Then::Select(id));
            return;
        }
        self.editor = None;
        self.revealed.clear();
        self.selected = id;
        self.fetch_selected();
    }

    fn fetch_selected(&mut self) {
        if let Some(id) = &self.selected
            && let Some(o) = self.overview(id)
            && !self.details.contains_key(id)
        {
            self.store.send(Job::Get { vault: o.vault_id.clone(), item: id.clone() });
        }
    }

    fn set_source(&mut self, cx: &mut Cx, source: Source) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(cx, Then::Source(source));
            return;
        }
        self.editor = None;
        self.source = source;
        // Keep the selection if it's still listed.
        let keep = self.selected.as_ref().is_some_and(|s| self.visible().iter().any(|i| &i.id == s));
        if !keep {
            let first = self.visible().first().map(|i| i.id.clone());
            self.selected = first;
            self.revealed.clear();
            self.fetch_selected();
        }
    }

    /// Whether the person has chosen how to keep their items.
    pub fn set_up(&self) -> bool {
        !self.settings.accounts.is_empty() || self.settings.local_only
    }

    /// Open the store (the setup comes first if it hasn't been done).
    pub fn connect(&mut self, cx: &mut Cx) {
        if !self.set_up() {
            self.conn = Conn::Setup;
            return;
        }
        self.conn = Conn::Connecting;
        self.loading = true;
        self.store.connect();
        self.redraw(cx);
    }

    /// Sync with 1Password now, if there's an account and the store is open.
    pub fn sync_now(&mut self) {
        if self.settings.accounts.is_empty() {
            return;
        }
        let accounts = self.settings.accounts.iter().map(|l| (l.id.clone(), l.label.clone())).collect();
        if self.lock.locked || self.conn != Conn::Ready || self.sync == Some(SyncState::Running) {
            return;
        }
        self.sync_asked = Some(Instant::now());
        self.sync_due = None;
        self.store.send(Job::Sync { accounts });
    }

    /// Sync now because the person asked: its changes open for review.
    pub fn sync_and_review(&mut self, cx: &mut Cx) {
        if self.review.as_ref().is_some_and(|r| r.window.is_some()) {
            return self.open_review(cx);
        }
        self.show_review = true;
        self.sync_now();
    }

    /// Show the waiting changes.
    pub fn open_review(&mut self, cx: &mut Cx) {
        let Some(review) = &mut self.review else { return };
        match review.window {
            Some(w) => cx.activate(w),
            None => review.window = Some(cx.open_window(WindowOptions::new("Review Sync", 560, 440).min_size(420, 300))),
        }
    }

    /// The review is over (applied, declined, or overtaken).
    fn end_review(&mut self, cx: &mut Cx) {
        if let Some(w) = self.review.take().and_then(|r| r.window) {
            cx.close_window(w);
        }
    }

    /// Ask, then erase everything kept here (nothing in 1Password).
    fn confirm_reset(&mut self, cx: &mut Cx) {
        let w = cx.open_window(WindowOptions::alert("Reset Passwords", RESET));
        self.alerts.insert(w, Alert::ConfirmReset);
    }

    /// The store was erased: back to the start.
    fn after_reset(&mut self, cx: &mut Cx) {
        self.end_review(cx);
        self.editor = None;
        self.vaults.clear();
        self.items.clear();
        self.details.clear();
        self.missing.clear();
        self.revealed.clear();
        self.selected = None;
        self.source = Source::All;
        self.settings.accounts.clear();
        self.settings.local_only = false;
        self.save_settings();
        self.sync = None;
        self.sync_due = None;
        self.sync_asked = None;
        self.account_field.clear();
        self.chosen.clear();
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
        self.flash("Passwords was reset");
    }

    /// Keep items only here from now on (no syncing).
    pub fn keep_local(&mut self, cx: &mut Cx) {
        self.settings.accounts.clear();
        self.settings.local_only = true;
        self.sync = None;
        self.save_settings();
        self.connect(cx);
    }

    /// Back to choosing the accounts (listing the app's afresh).
    pub fn change_account(&mut self) {
        self.chosen = self.settings.accounts.iter().map(|l| l.id.clone()).collect();
        self.account_field.clear();
        self.other_account = false;
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
    }

    /// The setup's choice: sync with `chosen` (the accounts kept keep
    /// their order, so the first stays first). Dropping any asks first.
    pub fn choose_accounts(&mut self, cx: &mut Cx, chosen: Vec<Linked>) {
        let mut keep: Vec<Linked> = self.settings.accounts.iter().filter(|l| chosen.iter().any(|c| c.id == l.id)).cloned().collect();
        keep.extend(chosen.into_iter().filter(|c| !self.settings.accounts.iter().any(|l| l.id == c.id)));
        let removed: Vec<Linked> = self.settings.accounts.iter().filter(|l| !keep.iter().any(|k| k.id == l.id)).cloned().collect();
        if removed.is_empty() {
            return self.link_accounts(cx, Vec::new(), keep);
        }
        let message = remove_message(&removed);
        let mut options = WindowOptions::alert("Remove Account", &message);
        if let Some(main) = self.main {
            options = options.parent(main);
        }
        let w = cx.open_window(options);
        self.alerts.insert(w, Alert::ConfirmRemove { removed, keep });
    }

    fn link_accounts(&mut self, cx: &mut Cx, removed: Vec<Linked>, keep: Vec<Linked>) {
        for l in &removed {
            self.store.send(Job::RemoveAccount { account: l.id.clone() });
        }
        let first = self.settings.accounts.is_empty() && !self.settings.local_only;
        self.settings.accounts = keep;
        self.settings.local_only = self.settings.accounts.is_empty();
        self.save_settings();
        self.sync = None;
        // Sync at once, and its changes open for review.
        self.show_review = true;
        self.sync_asked = None;
        if first || self.vaults.is_empty() {
            self.connect(cx);
        } else {
            self.conn = Conn::Ready;
            self.sync_now();
            self.redraw(cx);
        }
    }

    /// Read the 1Password app's accounts off the UI thread.
    pub fn find_accounts(&mut self, cx: &mut Cx) {
        self.accounts = Accounts::Finding;
        let proxy = cx.proxy();
        std::thread::spawn(move || proxy.send(Accounts::Found(onepassword::accounts::local())));
    }

    /// The accounts as the person knows them: "Family", "Family and
    /// Work", "Family, Work and Club".
    pub fn account_label(&self) -> String {
        let names: Vec<&str> = self.settings.accounts.iter().map(|l| l.label.as_str()).collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => one.to_string(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(w) = self.main {
            cx.redraw(w);
        }
    }

    fn flash(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn save_settings(&self) {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.settings) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    /// Another app asks (NetPositive, for a page's logins or passkeys).
    fn keychain(&mut self, cx: &mut Cx, incoming: Incoming) {
        if !self.set_up() {
            return incoming.reply(Response::NotSetUp);
        }
        let waits = !matches!(incoming.request, Request::Logins { .. } | Request::Check { .. } | Request::Search { .. });
        if self.lock.locked {
            if !waits {
                return incoming.reply(Response::Locked);
            }
            // Come forward to be unlocked; the answer follows.
            self.open_main(cx);
            self.waiting.push(incoming);
            return;
        }
        if let Conn::Failed(message) = &self.conn {
            return incoming.reply(Response::Error { message: message.clone() });
        }
        if self.loading || self.conn == Conn::Connecting {
            self.waiting.push(incoming);
            return;
        }
        // The items the request may concern, by their websites.
        let fits: Box<dyn Fn(&onepassword::Website) -> bool> = match &incoming.request {
            Request::Unlock => return incoming.reply(Response::Unlocked),
            Request::Search { query } => {
                let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
                let mut items: Vec<tabula_keychain::Found> = self
                    .items
                    .iter()
                    .filter(|i| i.state == ItemState::Active)
                    .filter(|i| {
                        !words.is_empty()
                            && words.iter().all(|w| {
                                i.title.to_lowercase().contains(w)
                                    || i.websites.iter().any(|s| s.url.to_lowercase().contains(w))
                                    || i.tags.iter().any(|t| t.to_lowercase().contains(w))
                            })
                    })
                    .map(|i| tabula_keychain::Found {
                        vault: i.vault_id.clone(),
                        id: i.id.clone(),
                        title: i.title.clone(),
                        subtitle: self.subtitle(i),
                    })
                    .collect();
                items.sort_by_cached_key(|f| (f.title.to_lowercase(), f.id.clone()));
                items.truncate(50);
                return incoming.reply(Response::Items { items });
            }
            Request::Show { id, .. } => {
                let id = id.clone();
                if self.overview(&id).is_some() {
                    self.set_source(cx, Source::All);
                    self.search.clear();
                    self.select(cx, Some(id));
                }
                self.open_main(cx);
                self.redraw(cx);
                return incoming.reply(Response::Shown);
            }
            Request::Logins { url } | Request::Fill { url, .. } | Request::Check { url, .. } | Request::Save { url, .. } => {
                let url = url.clone();
                Box::new(move |w| store::fits(w, &url))
            }
            Request::Passkeys { rp_id, .. } | Request::PasskeyCreate { rp_id, .. } | Request::PasskeySign { rp_id, .. } => {
                let rp_id = rp_id.clone();
                Box::new(move |w| passkey::near(&w.url, &rp_id))
            }
        };
        let candidates: Vec<(String, String)> = self
            .items
            .iter()
            .filter(|i| i.state == ItemState::Active && i.websites.iter().any(&fits))
            .map(|i| (i.vault_id.clone(), i.id.clone()))
            .collect();
        let vault = self
            .vaults
            .iter()
            .find(|v| matches!(v.title.as_str(), "Personal" | "Private" | "Employee"))
            .or(self.vaults.first())
            .map(|v| v.id.clone());
        let Some(vault) = vault else {
            return incoming.reply(Response::Error { message: "There’s no vault to save in.".into() });
        };
        self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates, vault });
    }

    /// Answer what waited for an unlock or for the items.
    fn serve_waiting(&mut self, cx: &mut Cx) {
        for incoming in std::mem::take(&mut self.waiting) {
            self.keychain(cx, incoming);
        }
    }

    fn do_lock(&mut self, cx: &mut Cx) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(cx, Then::Lock);
            return;
        }
        self.editor = None;
        // Secrets go; overviews stay (hidden) so unlocking is instant.
        self.details.clear();
        self.revealed.clear();
        if let Some((w, _)) = self.generator.take() {
            cx.close_window(w);
        }
        // A waiting plan holds secrets too; it is worked out again later.
        self.end_review(cx);
        // The store lets go of its secrets (after a running sync).
        self.store.send(Job::Close);
        self.lock.lock();
        self.redraw(cx);
    }

    fn copy_secret(&mut self, cx: &mut Cx, what: &str, text: String) {
        let seconds = self.settings.clear_seconds;
        cx.copy_secret(text, Duration::from_secs(seconds as u64));
        self.flash(format!("Copied {what}. It leaves the clipboard in {seconds} seconds."));
        self.redraw(cx);
    }

    fn copy_field(&mut self, cx: &mut Cx, item: &str, field: &str) {
        let Some(f) = self.details.get(item).and_then(|i| i.field(field)).cloned() else { return };
        let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
        match f.field_type {
            ItemFieldType::Totp => {
                if let Some(code) = otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone())) {
                    self.copy_secret(cx, "the one-time password", code);
                }
            }
            t if edit::concealed(t) || t == ItemFieldType::CreditCardNumber => {
                self.copy_secret(cx, &format!("the {label}"), f.value.clone())
            }
            _ => {
                cx.copy(f.value.clone());
                self.flash(format!("Copied the {label}."));
            }
        }
    }

    /// The selected item's username, password or one-time code.
    fn copy_selected(&mut self, cx: &mut Cx, kind: &str) {
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)).cloned() else { return };
        let field = match kind {
            "username" => item.fields.iter().find(|f| f.id == "username").or_else(|| {
                item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Email) || f.title.eq_ignore_ascii_case("username"))
            }),
            "password" => item.fields.iter().find(|f| f.id == "password").or_else(|| {
                item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed)
            }),
            _ => item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp),
        };
        match field {
            Some(f) => self.copy_field(cx, &item.id, &f.id.clone()),
            None => self.flash(format!("“{}” has no {}.", item.title, if kind == "otp" { "one-time password" } else { kind })),
        }
        self.redraw(cx);
    }

    fn open_website(&mut self, cx: &mut Cx) {
        let Some(url) = self.selected.as_ref().and_then(|s| self.overview(s)).and_then(|o| o.websites.first()).map(|w| w.url.clone())
        else { return };
        open_url(&url);
        self.flash(format!("Opening {}", host(&url).unwrap_or(url)));
        self.redraw(cx);
    }

    fn new_item(&mut self, cx: &mut Cx, category: ItemCategory) {
        if self.lock.locked || self.conn != Conn::Ready || self.vaults.is_empty() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(cx, Then::New(category));
            return;
        }
        // The vault being looked at, else the first (Personal).
        let vault = match &self.source {
            Source::Vault(v) => self.vaults.iter().position(|x| &x.id == v).unwrap_or(0),
            _ => 0,
        };
        self.editor = Some(Editor::new_item(category, vault, None));
        self.redraw(cx);
    }

    fn edit_selected(&mut self, cx: &mut Cx) {
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)) else { return };
        if self.overview(&item.id).is_some_and(|o| o.state == ItemState::Archived) {
            return;
        }
        let vault = self.vaults.iter().position(|v| v.id == item.vault_id).unwrap_or(0);
        self.editor = Some(Editor::edit(item, vault));
        self.redraw(cx);
    }

    fn save_editor(&mut self, cx: &mut Cx) {
        let Some(editor) = &self.editor else { return };
        if editor.base.is_some() {
            if !editor.changed() {
                self.editor = None;
            } else if let Some(item) = editor.to_item() {
                self.store.send(Job::Put(Box::new(item)));
                self.flash("Saving…");
            }
        } else if let Some(params) = editor.to_params(&self.vaults) {
            self.store.send(Job::Create(Box::new(params)));
            self.flash("Saving…");
        }
        self.redraw(cx);
    }

    fn ask_discard(&mut self, cx: &mut Cx, then: Then) {
        let msg = "Discard the changes to this item?";
        let mut options = WindowOptions::alert("Passwords", msg);
        if let Some(main) = self.main {
            options = options.parent(main);
        }
        let w = cx.open_window(options);
        self.alerts.insert(w, Alert::DiscardChanges { then: Box::new(then) });
    }

    fn show_error(&mut self, cx: &mut Cx, message: String) {
        let mut options = WindowOptions::alert("Passwords", &message);
        if let Some(main) = self.main {
            options = options.parent(main);
        }
        let w = cx.open_window(options);
        self.alerts.insert(w, Alert::Message(message));
    }

    /// The whole of a sync problem, from the status line.
    pub fn show_problem(&mut self, cx: &mut Cx, message: String) {
        self.show_error(cx, message);
    }

    /// What a sync couldn't do, item by item, grouped by vault.
    pub fn show_problems(&mut self, cx: &mut Cx, list: Vec<sync::Problem>) {
        let list = sync::grouped(list);
        match &mut self.problems {
            Some((w, shown)) => {
                *shown = list;
                cx.activate(*w);
            }
            None => {
                let mut options = WindowOptions::new("Sync Problems", 520, 400).min_size(400, 260);
                if let Some(main) = self.main {
                    options = options.parent(main);
                }
                self.problems = Some((cx.open_window(options), list));
            }
        }
    }

    fn confirm_delete(&mut self, cx: &mut Cx) {
        let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)).cloned() else { return };
        let message = format!("Delete “{}” for good? This can’t be undone.", o.title);
        let mut options = WindowOptions::alert("Delete Item", &message);
        if let Some(main) = self.main {
            options = options.parent(main);
        }
        let w = cx.open_window(options);
        self.alerts.insert(w, Alert::ConfirmDelete { vault: o.vault_id, item: o.id, title: o.title });
    }

    fn after_discard(&mut self, cx: &mut Cx, then: Then) {
        self.editor = None;
        match then {
            Then::Select(id) => self.select(cx, id),
            Then::Source(s) => self.set_source(cx, s),
            Then::New(c) => self.new_item(cx, c),
            Then::Close => {
                if let Some(w) = self.main.take() {
                    cx.close_window(w);
                }
                cx.quit();
            }
            Then::Lock => self.do_lock(cx),
        }
    }

    fn open_generator(&mut self, cx: &mut Cx, target: Option<(String, String)>) {
        if let Some((w, g)) = &mut self.generator {
            g.target = target;
            cx.activate(*w);
            return;
        }
        let w = cx.open_window(WindowOptions::new("Password Generator", 380, 330).min_size(340, 300));
        self.generator = Some((w, Generator::new(self.settings.generator.clone(), target)));
    }

    fn open_main(&mut self, cx: &mut Cx) {
        // (Gone if it was closed while we hold the clipboard.)
        if let Some(w) = self.main.filter(|w| cx.is_open(*w)) {
            cx.activate(w);
            return;
        }
        self.generator = None;
        let (w, h) = self.settings.size.unwrap_or((920, 580));
        self.main = Some(cx.open_window(WindowOptions::new("Passwords", w, h).min_size(680, 420)));
    }

    fn apply_reply(&mut self, cx: &mut Cx, reply: Reply) {
        match reply {
            Reply::Connected => {
                self.conn = Conn::Ready;
                if self.sync_asked.is_none_or(|t| t.elapsed() > SYNC_EVERY) {
                    self.sync_due = Some(Instant::now());
                }
            }
            Reply::Renamed { old, new } => {
                if self.selected.as_deref() == Some(old.as_str()) {
                    self.selected = Some(new.clone());
                }
                if let Some(mut d) = self.details.remove(&old) {
                    d.id = new.clone();
                    self.details.insert(new.clone(), d);
                }
                for i in self.items.iter_mut().filter(|i| i.id == old) {
                    i.id = new.clone();
                }
                if let Some(base) = self.editor.as_mut().and_then(|e| e.base.as_mut()).filter(|b| b.id == old) {
                    base.id = new;
                }
            }
            Reply::Sync(state) => {
                if let SyncState::Done { problems, .. } = &state {
                    for e in problems.iter().filter(|e| !e.note) {
                        tracing::warn!("sync: {}: {}: {}", e.vault, e.item.as_deref().unwrap_or("(vault)"), e.reason);
                    }
                }
                if !matches!(state, SyncState::Review { .. }) {
                    self.end_review(cx);
                }
                if !matches!(state, SyncState::Running | SyncState::Review { .. }) {
                    self.show_review = false;
                }
                self.sync = Some(state);
            }
            Reply::Review { plan, rows, problems } => {
                let window = self.review.take().and_then(|r| r.window);
                self.review = Some(Review { plan, rows, problems, window });
                if std::mem::take(&mut self.show_review) || window.is_some() {
                    self.open_review(cx);
                }
            }
            Reply::Reset => self.after_reset(cx),
            Reply::Failed(message) => {
                self.conn = Conn::Failed(message);
                self.loading = false;
                self.serve_waiting(cx);
            }
            Reply::Vaults { vaults, accounts } => {
                let ids: HashSet<&str> = vaults.iter().map(|v| v.id.as_str()).collect();
                self.items.retain(|i| ids.contains(i.vault_id.as_str()));
                if let Source::Vault(v) = &self.source
                    && !ids.contains(v.as_str())
                {
                    self.source = Source::All;
                }
                self.vaults = vaults;
                self.vault_accounts = accounts;
            }
            Reply::Items { vault, items } => {
                self.items.retain(|i| i.vault_id != vault);
                // Drop cached secrets for items that changed elsewhere.
                for item in &items {
                    if self.details.get(&item.id).is_some_and(|d| d.updated_at != item.updated_at) {
                        self.details.remove(&item.id);
                    }
                }
                self.items.extend(items);
                self.fetch_selected();
            }
            Reply::Loaded => {
                self.loading = false;
                // Start on the top row once every vault is in.
                if self.selected.is_none() {
                    self.selected = self.visible().first().map(|i| i.id.clone());
                    self.reveal_selection = true;
                    self.fetch_selected();
                }
                self.serve_waiting(cx);
            }
            Reply::Item(item) => {
                self.missing.remove(&item.id);
                self.details.insert(item.id.clone(), *item);
            }
            Reply::Missing { item, error } => {
                self.missing.insert(item, error);
            }
            Reply::Saved { item, created } => {
                let overview = ItemOverview {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    category: item.category,
                    vault_id: item.vault_id.clone(),
                    websites: item.websites.clone(),
                    tags: item.tags.clone(),
                    created_at: item.created_at,
                    updated_at: item.updated_at,
                    state: ItemState::Active,
                };
                self.items.retain(|i| i.id != item.id);
                self.items.push(overview);
                let title = item.title.clone();
                self.selected = Some(item.id.clone());
                self.reveal_selection = true;
                self.details.insert(item.id.clone(), *item);
                if self.editor.as_ref().is_some_and(|e| e.item_id().is_none() == created) {
                    self.editor = None;
                }
                if created && !self.visible().iter().any(|i| Some(&i.id) == self.selected.as_ref()) {
                    self.source = Source::All;
                    self.search.clear();
                }
                self.flash(format!("Saved “{title}”."));
                self.changed_here();
            }
            Reply::Archived { item } => {
                let row = self.visible().iter().position(|i| i.id == item);
                if let Some(i) = self.items.iter_mut().find(|i| i.id == item) {
                    i.state = ItemState::Archived;
                    let title = i.title.clone();
                    self.flash(format!("Moved “{title}” to the Archive."));
                }
                self.after_removal(&item, row);
                self.changed_here();
            }
            Reply::Deleted { item } => {
                let row = self.visible().iter().position(|i| i.id == item);
                if let Some(i) = self.items.iter().find(|i| i.id == item) {
                    self.flash(format!("Deleted “{}”.", i.title));
                }
                self.items.retain(|i| i.id != item);
                self.details.remove(&item);
                self.after_removal(&item, row);
                self.changed_here();
            }
            Reply::Error(message) => {
                if self.status.as_ref().is_some_and(|(s, _)| s == "Saving…") {
                    self.status = None;
                }
                self.show_error(cx, message);
            }
        }
        self.redraw(cx);
    }

    /// Select the next item after `gone` left the list.
    /// Once `gone` (listed at `row`) leaves the list, select what took its place.
    fn after_removal(&mut self, gone: &str, row: Option<usize>) {
        if self.selected.as_deref() != Some(gone) || self.visible().iter().any(|i| i.id == gone) {
            return;
        }
        self.editor = None;
        let visible = self.visible();
        let next = row.unwrap_or(0).min(visible.len().saturating_sub(1));
        self.selected = visible.get(next).map(|i| i.id.clone());
        self.reveal_selection = true;
        self.fetch_selected();
    }

    /// Something changed here: send it to 1Password soon.
    fn changed_here(&mut self) {
        if !self.settings.accounts.is_empty() {
            self.sync_due = Some(Instant::now() + SYNC_AFTER_CHANGE);
        }
    }

    fn tick(&mut self, cx: &mut Cx) {
        let due = self.sync_due.is_some_and(|t| Instant::now() >= t) || self.sync_asked.is_some_and(|t| t.elapsed() >= SYNC_EVERY);
        // Not while the person is looking at what the last one found.
        if due && self.review.as_ref().is_none_or(|r| r.window.is_none()) {
            self.sync_now();
        }
        let idle = self.last_use.elapsed();
        if !self.lock.locked
            && self.settings.lock_minutes > 0
            && idle >= Duration::from_secs(self.settings.lock_minutes as u64 * 60)
            && self.editor.as_ref().is_none_or(|e| !e.changed())
        {
            self.do_lock(cx);
        }
        if self.status.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(6)) {
            self.status = None;
            self.redraw(cx);
        }
    }
}

/// The code a TOTP field's value gives now.
pub fn otp_code(value: &str) -> Option<String> {
    Totp::parse(value).ok().map(|t| t.now().0)
}

pub fn open_url(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        if let Err(e) = tabula_xdg::open::open(&tabula_xdg::open::Target::Url(url)) {
            tracing::warn!("open: {e}");
        }
    });
}

fn category_menu(menu: Menu, prefix: &str) -> Menu {
    let mut menu = menu;
    for (i, c) in view::NEW_CATEGORIES.iter().enumerate() {
        if i == 3 {
            menu = menu.separator();
        }
        let key = if *c == ItemCategory::Login && prefix == "new:" { Some(Key::N) } else { None };
        menu = menu.item(c.label(), format!("{prefix}{}", c.as_str()), key);
    }
    menu
}

impl App for Passwords {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        if ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::PointerButton { .. } | egui::Event::Text(_)))
        }) {
            self.last_use = Instant::now();
        }
        if Some(window) == self.main {
            let focused = ui.input(|i| i.focused);
            self.lock.focus(focused);
            view::main(self, cx, ui);
            return;
        }
        if let Some((w, generator)) = &mut self.generator
            && *w == window
        {
            let action = tabula_kit::widgets::window_content(ui, |ui| generator.ui(ui));
            self.settings.generator = generator.recipe.clone();
            match action {
                Some(generator::Action::Copy(pw)) => self.copy_secret(cx, "the password", pw),
                Some(generator::Action::Use(pw)) => {
                    if let Some((item, field)) = generator.target.take()
                        && let Some(editor) = &mut self.editor
                        && editor.item_id().unwrap_or("") == item
                        && let Some(f) = editor.fields.iter_mut().find(|f| f.id == field)
                    {
                        f.value = pw;
                        f.reveal = true;
                    }
                    cx.close_window(window);
                    self.generator = None;
                    self.save_settings();
                    self.redraw(cx);
                }
                Some(generator::Action::Close) => {
                    cx.close_window(window);
                    self.generator = None;
                    self.save_settings();
                }
                None => {}
            }
            return;
        }
        if let Some(review) = &self.review
            && review.window == Some(window)
        {
            let plan = review.plan;
            match view::review(self, ui) {
                Some(view::ReviewAction::Apply) => {
                    self.store.send(Job::Approve { plan });
                    self.end_review(cx);
                }
                Some(view::ReviewAction::Cancel) => {
                    self.store.send(Job::Decline);
                    self.end_review(cx);
                }
                Some(view::ReviewAction::Problems) => {
                    let list = self.review.as_ref().map(|r| r.problems.clone()).unwrap_or_default();
                    self.show_problems(cx, list);
                }
                None => {}
            }
            return;
        }
        if self.problems.as_ref().is_some_and(|(w, _)| *w == window) {
            match view::problems(self, ui) {
                Some(view::ProblemsAction::Close) => {
                    self.problems = None;
                    cx.close_window(window);
                }
                Some(view::ProblemsAction::SyncAgain) => {
                    self.problems = None;
                    cx.close_window(window);
                    self.sync_and_review(cx);
                }
                Some(view::ProblemsAction::Copy(text)) => cx.copy(text),
                None => {}
            }
            return;
        }
        if let Some(alert) = self.alerts.get(&window) {
            let (icon, message, buttons): (&str, String, &[&str]) = match alert {
                Alert::ConfirmReset => ("warning", RESET.into(), &["Cancel", "Erase and Start Again"]),
                Alert::ConfirmRemove { removed, .. } => ("warning", remove_message(removed), &["Cancel", "Remove"]),
                Alert::ConfirmDelete { title, .. } => {
                    ("warning", format!("Delete “{title}” for good? This can’t be undone."), &["Cancel", "Delete"])
                }
                Alert::DiscardChanges { .. } => ("warning", "Discard the changes to this item?".into(), &["Keep Editing", "Discard"]),
                Alert::Message(m) => ("stop", m.clone(), &["OK"]),
            };
            if let Some(choice) = tabula_kit::widgets::alert(ui, icon, &message, buttons) {
                cx.close_window(window);
                match self.alerts.remove(&window) {
                    Some(Alert::ConfirmDelete { vault, item, .. }) if choice == 1 => {
                        self.store.send(Job::Delete { vault, item });
                    }
                    Some(Alert::DiscardChanges { then }) if choice == 1 => self.after_discard(cx, *then),
                    Some(Alert::ConfirmReset) if choice == 1 => self.store.send(Job::Reset),
                    Some(Alert::ConfirmRemove { removed, keep }) if choice == 1 => self.link_accounts(cx, removed, keep),
                    _ => {}
                }
                self.redraw(cx);
            }
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if Some(window) != self.main {
            return None;
        }
        let ready = self.conn == Conn::Ready && !self.lock.locked;
        let editing = self.editor.is_some();
        let selected = self.selected.as_ref().and_then(|s| self.items.iter().find(|i| &i.id == s));
        let has = ready && selected.is_some() && !editing;
        let archived = selected.is_some_and(|s| s.state == ItemState::Archived);
        let details = self.selected.as_ref().and_then(|s| self.details.get(s));
        let has_otp = details.is_some_and(|d| d.fields.iter().any(|f| f.field_type == ItemFieldType::Totp));
        let has_site = selected.is_some_and(|s| !s.websites.is_empty());

        let file = Menu::new("File")
            .submenu(category_menu(Menu::new("New Item"), "new:"))
            .enabled(ready)
            .separator()
            .item("Password Generator", "generator", Some(Key::G))
            .separator()
            .item("Lock", "lock", Some(Key::L))
            .enabled(!self.lock.locked)
            .item("Sync Now", "sync", None)
            .enabled(ready && !self.settings.accounts.is_empty() && self.sync != Some(SyncState::Running))
            .item("1Password Accounts…", "account", None)
            .enabled(!self.lock.locked)
            .item("Reset Passwords…", "reset", None)
            .enabled(!self.lock.locked && self.set_up())
            .separator()
            .item("Close", "close", Some(Key::W))
            .item("Quit", "quit", Some(Key::Q));
        let edit = Menu::new("Edit")
            .item("Copy Username", "copy:username", Some(Key::C))
            .enabled(has && details.is_some())
            .item_shift("Copy Password", "copy:password", Key::C)
            .enabled(has && details.is_some())
            .item_shift("Copy One-Time Password", "copy:otp", Key::O)
            .enabled(has && has_otp)
            .separator()
            .item("Find", "find", Some(Key::F))
            .enabled(ready);
        let item = Menu::new("Item")
            .item("Edit", "edit", Some(Key::E))
            .enabled(has && details.is_some() && !archived)
            .item("Open Website", "open", Some(Key::O))
            .enabled(has && has_site)
            .separator()
            .item("Move to Archive", "archive", None)
            .enabled(has && !archived)
            .item("Delete…", "delete", None)
            .enabled(has);
        let mut view = Menu::new("View").item("All Items", "source:all", Some(Key::Num0)).checked(self.source == Source::All);
        for (i, v) in self.vaults.iter().enumerate().take(9) {
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9][i];
            view = view.item(&v.title, format!("source:vault:{}", v.id), Some(key)).checked(self.source == Source::Vault(v.id.clone()));
        }
        view = view.separator().item("Archive", "source:archive", None).checked(self.source == Source::Archive);
        let mut lock_after = Menu::new("Lock After");
        for (minutes, label) in [(1, "1 Minute"), (5, "5 Minutes"), (10, "10 Minutes"), (30, "30 Minutes"), (60, "1 Hour"), (0, "Never")] {
            lock_after = lock_after.item(label, format!("lockafter:{minutes}"), None).checked(self.settings.lock_minutes == minutes);
        }
        view = view.separator().submenu(lock_after);
        Some(vec![file, edit, item, view])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        self.last_use = Instant::now();
        match command {
            "quit" | "close" => {
                if self.editor.as_ref().is_some_and(|e| e.changed()) {
                    self.ask_discard(cx, Then::Close);
                } else {
                    self.main = None;
                    cx.quit();
                }
            }
            "lock" => self.do_lock(cx),
            "account" => self.change_account(),
            "sync" => self.sync_and_review(cx),
            "reset" => self.confirm_reset(cx),
            "generator" => self.open_generator(cx, None),
            "find" => self.focus_search = true,
            "edit" => self.edit_selected(cx),
            "open" => self.open_website(cx),
            "archive" => {
                if let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)) {
                    self.store.send(Job::Archive { vault: o.vault_id.clone(), item: o.id.clone() });
                }
            }
            "delete" => self.confirm_delete(cx),
            c if c.starts_with("copy:") => self.copy_selected(cx, &c[5..]),
            c if c.starts_with("new:") => {
                if let Some(cat) = ItemCategory::ALL.iter().find(|x| x.as_str() == &c[4..]) {
                    self.new_item(cx, *cat);
                }
            }
            "source:all" => self.set_source(cx, Source::All),
            "source:archive" => self.set_source(cx, Source::Archive),
            c if c.starts_with("source:vault:") => self.set_source(cx, Source::Vault(c[13..].to_string())),
            c if c.starts_with("lockafter:") => {
                self.settings.lock_minutes = c[10..].parse().unwrap_or(10);
                self.save_settings();
            }
            _ => {}
        }
        self.redraw(cx);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if Some(window) == self.main {
            if let Some((w, h)) = cx.window_size(window) {
                self.settings.size = Some((w, h));
                self.save_settings();
            }
            if self.editor.as_ref().is_some_and(|e| e.changed()) {
                self.ask_discard(cx, Then::Close);
                return;
            }
            self.main = None;
            cx.quit();
            return;
        }
        if self.generator.as_ref().is_some_and(|(w, _)| *w == window) {
            self.generator = None;
            self.save_settings();
        }
        if self.problems.as_ref().is_some_and(|(w, _)| *w == window) {
            self.problems = None;
        }
        if self.review.as_ref().is_some_and(|r| r.window == Some(window)) {
            self.store.send(Job::Decline);
            self.end_review(cx);
            return;
        }
        self.alerts.remove(&window);
        cx.close_window(window);
    }

    fn message(&mut self, cx: &mut Cx, msg: Box<dyn std::any::Any + Send>) {
        let msg = match msg.downcast::<FromStore>() {
            Ok(m) => {
                if m.generation == self.store.generation {
                    self.apply_reply(cx, m.reply);
                }
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<LockMsg>() {
            Ok(m) => {
                if self.lock.message(*m) {
                    self.last_use = Instant::now();
                    self.unlocked = true;
                    // The store opens (again) now its secrets are wanted.
                    self.connect(cx);
                    if self.selected.is_none() {
                        self.selected = self.visible().first().map(|i| i.id.clone());
                    }
                    self.fetch_selected();
                    self.serve_waiting(cx);
                }
                self.redraw(cx);
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<Accounts>() {
            Ok(found) => {
                if let Accounts::Found(list) = &*found {
                    // Those in use; with none yet, the first listed.
                    if self.chosen.is_empty()
                        && let Some(a) = list.first()
                    {
                        self.chosen.push(a.uuid.clone());
                    }
                    self.other_account = list.is_empty();
                }
                self.accounts = *found;
                self.redraw(cx);
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<Tick>() {
            Ok(_) => return self.tick(cx),
            Err(m) => m,
        };
        let msg = match msg.downcast::<Incoming>() {
            Ok(incoming) => return self.keychain(cx, *incoming),
            Err(m) => m,
        };
        if msg.downcast::<Launch>().is_ok() {
            self.open_main(cx);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        serde_json::to_string(&Saved { source: self.source.clone(), selected: self.selected.clone(), search: self.search.clone() }).ok()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        // Logins for NetPositive's pages.
        let keychain = cx.proxy();
        if let Err(e) = tabula_keychain::serve(move |incoming| keychain.send(incoming)) {
            tracing::warn!("keychain: {e}");
        }
        let mut settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // From when there was one account.
        if let Some(id) = settings.account.take()
            && settings.accounts.is_empty()
        {
            let label = settings.account_label.take().unwrap_or_else(|| id.clone());
            settings.accounts.push(Linked { id, label });
        }
        let proxy = cx.proxy();
        {
            let proxy = proxy.clone();
            std::thread::Builder::new()
                .name("tick".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(5));
                    proxy.send(Tick);
                })
                .expect("thread");
        }
        let saved: Option<Saved> = cx.restored_state().and_then(|s| serde_json::from_str(&s).ok());
        // Locked from the start once there are items to protect.
        let locked = !settings.accounts.is_empty() || settings.local_only;
        let mut app = Passwords {
            chosen: settings.accounts.iter().map(|l| l.id.clone()).collect(),
            account_field: String::new(),
            accounts: Accounts::Unknown,
            other_account: false,
            store: Store::new(proxy.clone()),
            lock: Lock::new(proxy.clone(), locked),
            conn: if locked { Conn::Connecting } else { Conn::Setup },
            settings,
            vaults: Vec::new(),
            vault_accounts: HashMap::new(),
            items: Vec::new(),
            loading: false,
            details: HashMap::new(),
            missing: HashMap::new(),
            source: Source::All,
            selected: None,
            reveal_selection: true,
            search: String::new(),
            focus_search: false,
            revealed: HashSet::new(),
            editor: None,
            main: None,
            generator: None,
            alerts: HashMap::new(),
            status: None,
            last_use: Instant::now(),
            unlocked: false,
            waiting: Vec::new(),
            sync: None,
            sync_asked: None,
            sync_due: None,
            review: None,
            problems: None,
            show_review: false,
        };
        if let Some(saved) = saved {
            app.source = saved.source;
            app.selected = saved.selected;
            app.search = saved.search;
        }
        app.open_main(cx);
        // Locked, the store opens at the unlock.
        if !locked {
            app.connect(cx);
        }
        app
    });
}
