//! Passwords: logins and other secrets kept on this computer, encrypted
//! (`vault.rs`), and synced both ways with a 1Password account if asked
//! (`sync.rs`, through `olo-onepassword`). Locked with Touch ID.
//!
//! On olo-kit. Passwords runs with no window (started at login as the
//! SSH agent and the keychain, locked) and its window comes and goes, so
//! its state and logic are the app's: one [`Passwords`] behind a lock
//! ([`app`]). The app's looper takes what the proxy brings (the store's
//! replies, the lock's, other programs' requests, settings, what's due)
//! and the windows' views lock it to draw and to act (`view.rs`); what
//! changed is told to the windows ([`Passwords::changed`]). What only a
//! window has (its text fields, lists, scrolls) is its view's, and what
//! the logic wants of those it leaves in [`Ui`] for the view to take.

mod agent;
mod allow;
mod cli;
mod edit;
mod generator;
mod lock;
mod passkey;
mod store;
mod sync;
mod undo;
mod vault;
mod view;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_keychain::{Incoming, Request, Response};
use olo_kit::{
    ClickEvent, ClipboardItem, Cx, Element, FocusHandle, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, ParentElement as _, Proxy, SharedString, Styled as _, View, WindowHandle,
    WindowOptions, div,
    due::Notice,
    instance::{self, Launch},
    metrics as m,
    notify::{Note, Notifier},
    px,
    search::{self, Facet, Needle, Query},
    widgets::{Alert, AlertFrame, Button, RadioButton, alert, button_row, open_app_alert, paragraph},
};
use olo_protocol::PasswordsSecurity;
use onepassword::{Item, ItemCategory, ItemFieldType, ItemOverview, ItemState, VaultOverview, totp::Totp};
use serde::{Deserialize, Serialize};

use crate::{
    edit::Editor,
    generator::{Generator, GeneratorMsg, GeneratorView},
    lock::{Lock, LockMsg},
    store::{FieldText, FromStore, Job, Reply, Searchable, Store, SyncState},
    view::{MainMsg, MainView, ProblemsView, ReviewView},
};

const APP_ID: &str = "olo.passwords";

/// A 1Password account Passwords syncs with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Linked {
    /// Its UUID (or its name in 1Password's sidebar, if typed).
    pub id: String,
    /// How to name it to the person.
    pub label: String,
}

#[derive(Serialize, Deserialize, Default)]
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
    /// From before these moved to Preferences ▸ Security (read once, to
    /// carry them over).
    #[serde(skip_serializing)]
    lock_minutes: Option<u32>,
    #[serde(skip_serializing)]
    clear_seconds: Option<u32>,
    generator: generator::Recipe,
    /// The window's size, kept here before the kit kept windows' sizes
    /// (`keep_size`): its first size, then.
    #[serde(skip_serializing)]
    size: Option<(u32, u32)>,
}

/// Passwords on an agent screen: what's shared with the screen, opened
/// already (the person chose what, in Agent Screens), read only, never
/// locked or synced, and the person's settings left alone.
pub(crate) fn on_screen() -> bool {
    olo_keychain::shared::this_screen().is_some()
}

/// What a screen's Passwords says of a change it can't make.
const SHARED_ONLY: &str = "Shared with this screen: change it in your own Passwords.";

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("passwords.json")
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
    /// Unlocked when the old version handed over, and how long since it
    /// was last used: the new version starts unlocked too, unless the
    /// lock-after time has passed meanwhile (Ben, 2026-11-01: an upgrade
    /// shouldn't lock the keychain).
    #[serde(default)]
    unlocked: bool,
    #[serde(default)]
    idle_secs: u64,
    /// Its window was open (not running for the SSH agent alone).
    #[serde(default = "yes")]
    window: bool,
}

fn yes() -> bool {
    true
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

/// Whether app_server approves requests without asking (Deskbar's key),
/// for `ask_approval`, and what else could allow a session's `op`.
struct Approval {
    about: Option<cli::Session>,
    on: bool,
    ways: allow::Ways,
}

/// A terminal session's Allow alert, open: its window (once it's
/// opened), the touch, finger or look also waited for, and what's said
/// when one of them allows it.
struct Allowing {
    window: Option<WindowHandle<Alert>>,
    _waits: allow::Waits,
    command: String,
    ssh: bool,
}

/// Said in a notification that makes no sound.
fn quietly(n: &Notifier, title: &str, body: String) {
    let mut note = Note::new(title).body(body);
    note.silent = true;
    n.show(None, note);
}

/// Asked of app_server, off the UI thread.
fn approving_without_asking() -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let status = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&olo_protocol::Request::PowerStatus).ok());
    matches!(status, Some(olo_protocol::Reply::Power { status }) if status.auto_approve_until.is_some_and(|t| t > now))
}

/// The question for letting `op` use the passwords, with what else
/// allows it (`ways`: a touch, a finger, a look).
fn allow_message(a: &cli::Asker, ssh: bool, ways: Option<String>) -> String {
    let what = if ssh { "sign in with your SSH keys" } else { "use your passwords" };
    let mut m = format!("Allow “{}” to {what}?", a.command);
    match (&a.parent, &a.cwd) {
        (Some(p), Some(d)) => m.push_str(&format!(" It was run by {p} in {d}.")),
        (Some(p), None) => m.push_str(&format!(" It was run by {p}.")),
        _ => {}
    }
    if let Some(ways) = ways {
        m.push(' ');
        m.push_str(&ways);
    }
    m.push_str(" Other commands from the same terminal can then use them too, until Passwords locks or for 10 minutes after the last.");
    m
}

/// The accounts, as `op account list --format json` prints them: those
/// Passwords syncs with (with what the 1Password app says of them), or
/// this computer's own when it syncs with none.
fn cli_accounts(linked: &[Linked]) -> serde_json::Value {
    if linked.is_empty() {
        return serde_json::json!([{ "url": "local", "email": "", "user_uuid": "", "account_uuid": "local" }]);
    }
    let known = onepassword::accounts::local();
    serde_json::Value::Array(
        linked
            .iter()
            .map(|l| match known.iter().find(|a| a.uuid == l.id || a.name.eq_ignore_ascii_case(&l.id)) {
                Some(a) => serde_json::json!({ "url": a.address, "email": a.email, "user_uuid": "", "account_uuid": a.uuid }),
                None => serde_json::json!({ "url": l.label, "email": "", "user_uuid": "", "account_uuid": l.id }),
            })
            .collect(),
    )
}

const RESET: &str = "Erase everything Passwords keeps on this computer and start again? Its items, changes not yet sent to 1Password, and the choice of account all go. Nothing in 1Password is changed. This can’t be undone.";

/// A sync's changes, waiting for the person's approval.
pub struct Review {
    pub plan: u64,
    pub rows: Vec<sync::Row>,
    pub problems: Vec<sync::Problem>,
    /// The Review Sync window, while it's open.
    pub window: Option<WindowHandle<ReviewView>>,
}

/// The Sync Problems window and what it lists.
pub struct Problems {
    pub window: Option<WindowHandle<ProblemsView>>,
    pub list: Vec<sync::Problem>,
}

/// The screen locked.
struct ScreenLocked;

/// What's shared with this agent screen changed: opened again.
struct SharedChanged;

/// How often to sync while unlocked, and how soon after a change here.
const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const SYNC_AFTER_CHANGE: Duration = Duration::from_secs(3);
/// How long something said in the status line stays.
const STATUS_FOR: Duration = Duration::from_secs(6);

/// What the logic wants of the main window's own things (its text
/// fields, its lists, its scrolls), left here for its views to take as
/// they next show.
#[derive(Default)]
pub struct Ui {
    /// The password field lets go of what was typed.
    pub clear_lock: bool,
    /// The setup's account field is emptied.
    pub clear_account: bool,
    /// The search field shows the app's search (set here, not typed).
    pub push_search: bool,
    /// The search field takes the keyboard (Edit ▸ Find).
    pub focus_search: bool,
    /// The item's pane starts at its top again (another item).
    pub detail_top: bool,
    /// The editor starts at its top (just opened), or shows its end (why
    /// it can't be saved is said there).
    pub editor_top: bool,
    pub editor_bottom: bool,
    /// The editor's fields came or went: counted, and its form is made
    /// again when this moves.
    pub form: u64,
    /// A value put in the form from outside (the generator's Use): the
    /// field, and its text.
    pub form_value: Option<(usize, String)>,
}

pub struct Passwords {
    /// Edit ▸ Undo and Redo (`undo.rs`): items as they were. Secrets: gone
    /// when it locks.
    history: olo_kit::undo::History<undo::Change>,
    pub settings: Settings,
    /// Preferences ▸ Security's choices for Passwords (followed live).
    security: PasswordsSecurity,
    store: Store,
    pub conn: Conn,
    /// The setup's choices: listed accounts ticked, and one named by hand.
    pub chosen: Vec<String>,
    pub accounts: Accounts,
    /// Naming an account by hand rather than choosing a listed one.
    pub other_account: bool,
    pub vaults: Vec<VaultOverview>,
    /// Which account each synced vault is in.
    pub vault_accounts: HashMap<String, String>,
    pub items: Vec<ItemOverview>,
    pub loading: bool,
    pub details: HashMap<String, Item>,
    /// What search and the filters need of every item (store.rs).
    index: HashMap<String, Searchable>,
    /// The filters, made again when the items change.
    facets: Mutex<Option<Arc<Vec<Facet>>>>,
    /// The list worked out last (see `visible`), for which items
    /// (`rev`), source and search, and where in each item the search
    /// found what it did (`found_in`).
    #[allow(clippy::type_complexity)]
    listed: Mutex<Option<(u64, Source, String, Arc<Vec<ItemOverview>>, Arc<Vec<Option<String>>>)>>,
    /// Counts changes to the items (and what's known of them).
    pub rev: u64,
    pub missing: HashMap<String, String>,
    pub source: Source,
    pub selected: Option<String>,
    pub reveal_selection: bool,
    pub search: String,
    revealed: HashSet<String>,
    pub editor: Option<Editor>,
    /// The editor just opened: its title takes the keyboard.
    pub focus_title: bool,
    /// The editor's field to focus when next drawn (one just added).
    pub focus_field: Option<usize>,
    pub lock: Lock,
    /// The main window, while it's open.
    main: Option<WindowHandle<MainView>>,
    generator: Option<WindowHandle<GeneratorView>>,
    /// The terminal sessions being asked about (an alert showing).
    allow_shown: HashMap<cli::Session, Allowing>,
    pub status: Notice,
    last_use: Instant,
    /// Just unlocked: the list takes the keyboard from the password
    /// field.
    pub unlocked: bool,
    /// Other apps' requests waiting for an unlock or the item list.
    pub waiting: Vec<Incoming>,
    /// `op`'s, waiting for the person to allow their sessions.
    cli_waiting: Vec<(cli::Session, Incoming)>,
    /// The terminal sessions allowed to use `op`.
    grants: cli::Grants,
    /// Asking app_server whether it approves without asking, for the
    /// unlock (None) or for a session's `op`.
    asking: HashSet<Option<cli::Session>>,
    notifier: Notifier,
    /// Syncing with 1Password: how the last one went.
    pub sync: Option<SyncState>,
    /// When the last sync was asked for, and when the next is due early.
    pub sync_asked: Option<Instant>,
    sync_due: Option<Instant>,
    /// The latest sync's changes, until applied or declined.
    pub review: Option<Review>,
    /// The Sync Problems window and what it lists.
    pub problems: Option<Problems>,
    /// Open the review as soon as the sync asked for has one (the person
    /// asked; automatic syncs only say so in the status line).
    pub show_review: bool,
    /// What the main window's views are to do with their own things.
    pub ui: Ui,
    /// A secret copied here is on the clipboard until then.
    secret_until: Option<Instant>,
    /// Closed with a secret still on the clipboard: running on without
    /// its windows until that has left (`quit`).
    leaving: bool,
    proxy: Proxy,
    /// On an agent screen: what's shared with it, followed (the person's
    /// Passwords writes it again as they share more or less).
    _shared: Option<olo_kit::watch::Watch>,
}

/// The app: its state and what it does, the one of it, behind a lock
/// (the app's looper and each window's take it in turn, briefly; nothing
/// that waits is done under it).
static APP: OnceLock<Mutex<Passwords>> = OnceLock::new();

pub fn app() -> MutexGuard<'static, Passwords> {
    APP.get().expect("made as it starts").lock().unwrap_or_else(|e| e.into_inner())
}

/// Something is due to be looked at on the app's looper (a timer set for
/// what's next due): asked once until it has been.
static SCHEDULING: AtomicBool = AtomicBool::new(false);

/// What the app's looper is told by the app itself.
enum Tell {
    /// Look at what's next due, and set a timer for it.
    Schedule,
    /// A timer came due.
    Tick,
}

/// `s` as a filter value: lower case, words joined by `-`
/// ("secure-note").
fn slug(s: &str) -> String {
    s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join("-")
}

/// Whether `n` names `name` (a section's title, a field's label): as
/// typed, or as its filter value (`credit-card`).
pub fn names(n: &Needle, name: &str) -> bool {
    !name.is_empty() && (n.starts(&slug(name)) || n.in_text(name))
}

fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).or_else(|_| url::Url::parse(&format!("https://{url}"))).ok()?;
    Some(parsed.host_str()?.trim_start_matches("www.").to_string())
}

impl Passwords {
    /// Something the windows show changed: they are told (each says what
    /// it shows again), and what's next due is looked at on the app's
    /// looper.
    pub fn changed(&self) {
        if let Some(w) = &self.main {
            w.messenger().send(MainMsg::Refresh);
        }
        if let Some(w) = self.review.as_ref().and_then(|r| r.window.as_ref()) {
            w.messenger().send(());
        }
        if let Some(w) = self.problems.as_ref().and_then(|p| p.window.as_ref()) {
            w.messenger().send(());
        }
        if !SCHEDULING.swap(true, Ordering::AcqRel) {
            self.proxy.send(Tell::Schedule);
        }
    }

    /// The person did something in the main window: the auto-lock is put
    /// off.
    pub fn used(&mut self) {
        self.last_use = Instant::now();
    }

    /// A vault's name, with its account's when there are several.
    pub fn vault_title(&self, id: &str) -> String {
        self.vaults.iter().find(|v| v.id == id).map_or(String::new(), |v| self.vault_name(v))
    }

    pub fn vault_name(&self, v: &VaultOverview) -> String {
        match self.vault_accounts.get(&v.id).and_then(|a| self.settings.accounts.iter().find(|l| &l.id == a)) {
            Some(l) if self.settings.accounts.len() > 1 => format!("{} ({})", v.title, l.label),
            _ => v.title.clone(),
        }
    }

    pub fn overview(&self, id: &str) -> Option<&ItemOverview> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The items (or what's known of them) changed: the list and the
    /// filters are worked out again when next wanted.
    fn invalidate(&mut self) {
        self.rev += 1;
        *self.facets.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The list for the current source and search: best matches first
    /// while searching, else by title. (Worked out once for each.)
    pub fn visible(&self) -> Arc<Vec<ItemOverview>> {
        self.listing().0
    }

    /// For each item `visible` lists, where the search found what it
    /// looks for when that's inside it (`found_in`).
    pub fn found(&self) -> Arc<Vec<Option<String>>> {
        self.listing().1
    }

    fn listing(&self) -> (Arc<Vec<ItemOverview>>, Arc<Vec<Option<String>>>) {
        if let Some((rev, source, search, list, found)) = &*self.listed.lock().unwrap_or_else(|e| e.into_inner())
            && *rev == self.rev
            && *source == self.source
            && *search == self.search
        {
            return (list.clone(), found.clone());
        }
        let facets = self.facets_rc();
        let q = Query::parse(&self.search, &search::keys(&facets));
        let mut list: Vec<(u32, &ItemOverview)> = self
            .items
            .iter()
            .filter(|i| match &self.source {
                Source::Archive => i.state == ItemState::Archived,
                _ if i.state == ItemState::Archived => false,
                Source::All => true,
                Source::Vault(v) => &i.vault_id == v,
                Source::Category(c) => i.category == *c,
            })
            .filter(|i| q.matches(|key, n| self.has(i, key, n)))
            .map(|i| {
                let s = self.index.get(&i.id);
                let user = self.username_of(i);
                let hosts: Vec<String> = i.websites.iter().filter_map(|w| host(&w.url)).collect();
                let mut fields: Vec<(&str, u32)> = vec![(i.title.as_str(), 4), (user, 3)];
                fields.extend(hosts.iter().map(|h| (h.as_str(), 3)));
                fields.extend(i.tags.iter().map(|t| (t.as_str(), 2)));
                if let Some(s) = s {
                    fields.push((s.text.as_str(), 1));
                }
                (q.score(&fields), i)
            })
            .collect();
        list.sort_by_cached_key(|(score, i)| (std::cmp::Reverse(*score), i.title.to_lowercase(), i.id.clone()));
        let found: Arc<Vec<Option<String>>> = Arc::new(list.iter().map(|(_, i)| self.found_in(i, &q)).collect());
        let list: Arc<Vec<ItemOverview>> = Arc::new(list.into_iter().map(|(_, i)| i.clone()).collect());
        *self.listed.lock().unwrap_or_else(|e| e.into_inner()) = Some((self.rev, self.source.clone(), self.search.clone(), list.clone(), found.clone()));
        (list, found)
    }

    /// Where in item `i` query `q` found what it looks for, when that's
    /// in its sections and fields rather than its title, username,
    /// websites or tags: the field with most of it, as "Section › label:
    /// value" (a secret's without its value).
    fn found_in(&self, i: &ItemOverview, q: &Query) -> Option<String> {
        let s = self.index.get(&i.id)?;
        let user = self.username_of(i);
        let outside = |n: &Needle| n.in_text(&i.title) || n.in_text(user) || i.websites.iter().any(|w| n.in_text(&w.url)) || i.tags.iter().any(|t| n.in_text(t));
        let words: Vec<&Needle> = q.words().filter(|n| !outside(n)).collect();
        let (sections, labels) = (q.values("section"), q.values("field"));
        if words.is_empty() && sections.is_empty() && labels.is_empty() {
            return None;
        }
        let asked = |f: &FieldText| (sections.is_empty() || sections.iter().any(|n| names(n, &f.section))) && (labels.is_empty() || labels.iter().any(|n| names(n, &f.label)));
        let has = |f: &FieldText| words.iter().filter(|n| n.in_text(&f.section) || n.in_text(&f.label) || n.in_text(&f.value)).count();
        // The first of those with most of the words.
        let (n, f) = s.fields.iter().filter(|f| asked(f)).map(|f| (has(f), f)).fold(None, |best: Option<(usize, &FieldText)>, (n, f)| match best {
            Some((m, _)) if m >= n => best,
            _ => Some((n, f)),
        })?;
        if n == 0 && !words.is_empty() {
            return None;
        }
        let mut out = if f.section.is_empty() { f.label.clone() } else { format!("{} › {}", f.section, f.label) };
        // Its line that has a word (a long value's), else its first.
        let line = f.value.lines().find(|l| words.iter().any(|n| n.in_text(l))).or_else(|| f.value.lines().next()).unwrap_or_default().trim();
        if !line.is_empty() {
            out = if out.is_empty() { line.to_owned() } else { format!("{out}: {line}") };
        }
        (!out.is_empty()).then_some(out)
    }

    /// An item's username: from the index, else its details.
    pub fn username_of<'a>(&'a self, i: &ItemOverview) -> &'a str {
        match self.index.get(&i.id).map(|s| s.username.as_str()).filter(|u| !u.is_empty()) {
            Some(u) => u,
            None => self.details.get(&i.id).and_then(|d| d.username()).unwrap_or_default(),
        }
    }

    /// Whether item `i` has `n` under filter `key` (none: anywhere).
    fn has(&self, i: &ItemOverview, key: Option<&str>, n: &Needle) -> bool {
        let s = self.index.get(&i.id);
        let sites = || i.websites.iter().any(|w| n.in_text(&w.url));
        match key {
            None => n.in_text(&i.title) || n.in_text(self.username_of(i)) || sites() || i.tags.iter().any(|t| n.in_text(t)) || n.in_text(i.category.label()) || s.is_some_and(|s| n.in_text(&s.text)),
            // (A value still being typed matches by its start.)
            Some("type") => n.starts(&slug(i.category.label())) || n.in_text(i.category.label()),
            Some("vault") => self.vaults.iter().find(|v| v.id == i.vault_id).is_some_and(|v| n.starts(&slug(&v.title)) || n.in_text(&v.title)),
            Some("tag") => i.tags.iter().any(|t| n.starts(t)),
            Some("site") => sites(),
            Some("user") => n.in_text(self.username_of(i)),
            Some("section") => s.is_some_and(|s| s.fields.iter().any(|f| names(n, &f.section))),
            Some("field") => s.is_some_and(|s| s.fields.iter().any(|f| names(n, &f.label))),
            Some("has") => {
                let Some(s) = s else { return false };
                let flags =
                    [("otp", s.otp), ("passkey", s.passkey.is_some()), ("password", s.password), ("website", !i.websites.is_empty()), ("files", s.files), ("reused", s.reused), ("weak", s.weak)];
                flags.iter().any(|(name, on)| *on && n.starts(name))
            }
            Some(_) => false,
        }
    }

    /// The list's filters: typed (`type:`, `vault:`, `tag:`, `site:`,
    /// `user:`, `has:`) and, for tags and what items have, in the bar;
    /// with how many items have each value.
    pub fn facets_rc(&self) -> Arc<Vec<Facet>> {
        if let Some(f) = &*self.facets.lock().unwrap_or_else(|e| e.into_inner()) {
            return f.clone();
        }
        let active: Vec<&ItemOverview> = self.items.iter().filter(|i| i.state == ItemState::Active).collect();
        let mut types: Vec<(ItemCategory, usize)> = Vec::new();
        let mut tags: std::collections::BTreeMap<String, usize> = Default::default();
        for i in &active {
            match types.iter_mut().find(|(c, _)| *c == i.category) {
                Some((_, n)) => *n += 1,
                None => types.push((i.category, 1)),
            }
            for t in &i.tags {
                *tags.entry(t.clone()).or_default() += 1;
            }
        }
        types.sort_by_key(|(c, _)| c.label());
        let mut ty = Facet::new("type", "Type").multiple().typed_only();
        for (c, n) in types {
            ty = ty.value(slug(c.label()), c.label(), Some(n));
        }
        let mut vault = Facet::new("vault", "Vault").multiple().typed_only();
        for v in &self.vaults {
            let n = active.iter().filter(|i| i.vault_id == v.id).count();
            vault = vault.value(slug(&v.title), self.vault_name(v), Some(n));
        }
        let mut tag = Facet::new("tag", "Tag").multiple();
        for (t, n) in tags {
            tag = tag.value(t.clone(), t, Some(n));
        }
        let count = |f: &dyn Fn(&Searchable) -> bool| Some(active.iter().filter(|i| self.index.get(&i.id).is_some_and(f)).count());
        let has = Facet::new("has", "Has")
            .multiple()
            .value("otp", "One-Time Code", count(&|s| s.otp))
            .value("passkey", "Passkey", count(&|s| s.passkey.is_some()))
            .value("password", "Password", count(&|s| s.password))
            .value("website", "Website", Some(active.iter().filter(|i| !i.websites.is_empty()).count()))
            .value("files", "Attachments", count(&|s| s.files))
            .value("reused", "Reused Password", count(&|s| s.reused))
            .value("weak", "Weak Password", count(&|s| s.weak));
        // Sections and fields by their names, with how many items have each.
        let (mut sections, mut labels): (std::collections::BTreeMap<String, (String, usize)>, std::collections::BTreeMap<String, (String, usize)>) = Default::default();
        for s in active.iter().filter_map(|i| self.index.get(&i.id)) {
            let mut seen: std::collections::HashSet<(bool, String)> = Default::default();
            for f in &s.fields {
                for (field, name, map) in [(false, &f.section, &mut sections), (true, &f.label, &mut labels)] {
                    let key = slug(name);
                    if !key.is_empty() && seen.insert((field, key.clone())) {
                        map.entry(key).or_insert_with(|| (name.clone(), 0)).1 += 1;
                    }
                }
            }
        }
        let mut section = Facet::new("section", "Section").multiple().typed_only();
        for (key, (name, n)) in sections {
            section = section.value(key, name, Some(n));
        }
        let mut field = Facet::new("field", "Field").multiple().typed_only();
        for (key, (name, n)) in labels {
            field = field.value(key, name, Some(n));
        }
        let facets = Arc::new(vec![tag, has, ty, vault, Facet::new("site", "Website").typed_only(), Facet::new("user", "Username").typed_only(), section, field]);
        *self.facets.lock().unwrap_or_else(|e| e.into_inner()) = Some(facets.clone());
        facets
    }

    /// The second line under an item's title: its username, else its
    /// site, else its kind.
    pub fn subtitle(&self, item: &ItemOverview) -> String {
        let user = self.username_of(item);
        if !user.is_empty() {
            return user.to_owned();
        }
        if let Some(h) = item.websites.first().and_then(|w| host(&w.url)) {
            return h;
        }
        item.category.label().to_string()
    }

    pub fn select(&mut self, id: Option<String>) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) && self.selected != id {
            self.ask_discard(Then::Select(id));
            return;
        }
        if self.selected != id {
            self.close_editor();
            self.revealed.clear();
            self.selected = id;
            self.ui.detail_top = true;
            self.fetch_selected();
        }
        self.changed();
    }

    fn fetch_selected(&mut self) {
        if let Some(id) = &self.selected
            && let Some(o) = self.overview(id)
            && !self.details.contains_key(id)
        {
            self.store.send(Job::Get { vault: o.vault_id.clone(), item: id.clone() });
        }
    }

    pub fn set_source(&mut self, source: Source) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Source(source));
            return;
        }
        self.close_editor();
        self.source = source;
        self.keep_selection_listed();
        self.changed();
    }

    /// The search (or a filter) changed; `from_bar`: not typed, so the
    /// field is told.
    pub fn set_search(&mut self, text: String, from_bar: bool) {
        if from_bar {
            self.ui.push_search = true;
        }
        if text == self.search {
            return;
        }
        self.search = text;
        self.keep_selection_listed();
        self.changed();
    }

    /// Keep the selection if it's still listed, else the first.
    fn keep_selection_listed(&mut self) {
        let keep = self.selected.as_ref().is_some_and(|s| self.visible().iter().any(|i| &i.id == s));
        if !keep {
            let first = self.visible().first().map(|i| i.id.clone());
            self.selected = first;
            self.revealed.clear();
            self.reveal_selection = true;
            self.fetch_selected();
        }
    }

    /// Whether the person has chosen how to keep their items.
    pub fn set_up(&self) -> bool {
        !self.settings.accounts.is_empty() || self.settings.local_only
    }

    /// Open the store (the setup comes first if it hasn't been done).
    pub fn connect(&mut self) {
        if !self.set_up() {
            self.conn = Conn::Setup;
            self.changed();
            return;
        }
        self.conn = Conn::Connecting;
        self.loading = true;
        self.store.connect();
        self.changed();
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
    pub fn sync_and_review(&mut self) {
        if self.review.as_ref().is_some_and(|r| r.window.is_some()) {
            return self.open_review();
        }
        self.show_review = true;
        self.sync_now();
        self.changed();
    }

    /// Show the waiting changes.
    pub fn open_review(&mut self) {
        let Some(review) = &mut self.review else { return };
        if let Some(w) = review.window.as_ref().filter(|w| w.is_open()) {
            olo_kit::raise(Some(w));
            return;
        }
        let options = WindowOptions::new("Review Sync", 560.0, 440.0).min_size(420.0, 300.0).floating();
        review.window = Some(olo_kit::open_window(options, ReviewView::new));
    }

    /// The Review Sync window was closed from its tab: as Cancel.
    pub fn review_closed(&mut self) {
        if let Some(r) = &mut self.review
            && r.window.take().is_some()
        {
            self.store.send(Job::Decline);
            self.review = None;
            self.changed();
        }
    }

    /// The review is over (applied, declined, or overtaken).
    pub fn end_review(&mut self) {
        if let Some(w) = self.review.take().and_then(|r| r.window) {
            w.messenger().send(());
            w.close();
        }
        self.changed();
    }

    /// Ask, then erase everything kept here (nothing in 1Password).
    fn confirm_reset(&mut self) {
        self.alert("Reset Passwords", "warning", RESET.to_owned(), &["Cancel", "Erase and Start Again"], |choice| {
            if choice == Some(1) {
                app().store.send(Job::Reset);
            }
        });
    }

    /// Opens an alert (the app's: over whichever of its windows has the
    /// keyboard). Its answer comes on the alert's looper, with the app
    /// not held.
    fn alert(&self, title: &'static str, icon: &'static str, message: String, buttons: &'static [&'static str], on_choose: impl Fn(Option<usize>) + Send + Sync + 'static) {
        let _ = open_app_alert(title, icon, &message, buttons, on_choose);
    }

    /// The store was erased: back to the start.
    fn after_reset(&mut self) {
        self.end_review();
        self.close_editor();
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
        self.ui.clear_account = true;
        self.chosen.clear();
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
        self.invalidate();
        self.flash("Passwords was reset");
    }

    /// Keep items only here from now on (no syncing).
    pub fn keep_local(&mut self) {
        self.settings.accounts.clear();
        self.settings.local_only = true;
        self.sync = None;
        self.save_settings();
        self.connect();
    }

    /// Back to choosing the accounts (listing the app's afresh).
    pub fn change_account(&mut self) {
        self.chosen = self.settings.accounts.iter().map(|l| l.id.clone()).collect();
        self.ui.clear_account = true;
        self.other_account = false;
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
        self.changed();
    }

    /// The setup's Sync: the accounts ticked and any named by hand.
    pub fn sync_chosen(&mut self, typed: &str) {
        let listed = view::listed_accounts(self);
        let mut chosen: Vec<Linked> = listed.iter().filter(|a| self.chosen.iter().any(|c| view::same(a, c))).map(|a| Linked { id: view::linked_id(self, a), label: a.name.clone() }).collect();
        let typed = typed.trim().to_string();
        if (listed.is_empty() || self.other_account) && !typed.is_empty() && !chosen.iter().any(|c| c.id == typed) {
            chosen.push(Linked { id: typed.clone(), label: typed });
        }
        if chosen.is_empty() && !self.set_up() {
            return;
        }
        self.choose_accounts(chosen);
    }

    /// The setup's choice: sync with `chosen` (the accounts kept keep
    /// their order, so the first stays first). Dropping any asks first.
    pub fn choose_accounts(&mut self, chosen: Vec<Linked>) {
        let mut keep: Vec<Linked> = self.settings.accounts.iter().filter(|l| chosen.iter().any(|c| c.id == l.id)).cloned().collect();
        keep.extend(chosen.into_iter().filter(|c| !self.settings.accounts.iter().any(|l| l.id == c.id)));
        let removed: Vec<Linked> = self.settings.accounts.iter().filter(|l| !keep.iter().any(|k| k.id == l.id)).cloned().collect();
        if removed.is_empty() {
            return self.link_accounts(Vec::new(), keep);
        }
        let message = remove_message(&removed);
        let what = Mutex::new(Some((removed, keep)));
        self.alert("Remove Account", "warning", message, &["Cancel", "Remove"], move |choice| {
            if choice == Some(1)
                && let Some((removed, keep)) = what.lock().unwrap_or_else(|e| e.into_inner()).take()
            {
                app().link_accounts(removed, keep);
            }
        });
    }

    fn link_accounts(&mut self, removed: Vec<Linked>, keep: Vec<Linked>) {
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
            self.connect();
        } else {
            self.conn = Conn::Ready;
            self.sync_now();
            self.changed();
        }
    }

    /// Read the 1Password app's accounts off the UI thread.
    pub fn find_accounts(&mut self) {
        self.accounts = Accounts::Finding;
        let proxy = self.proxy.clone();
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

    /// Says `text` in the status line for a few seconds.
    fn flash(&mut self, text: impl Into<String>) {
        self.status.say(text.into(), STATUS_FOR);
    }

    fn save_settings(&self) {
        // (A screen's Passwords keeps nothing of its own: these are the
        // person's.)
        if on_screen() {
            return;
        }
        olo_kit::store::save(settings_path(), &self.settings);
    }

    /// Another app asks (Chrome's extension for a page's logins, the
    /// palette, op).
    fn keychain(&mut self, incoming: Incoming) {
        // What's shared with agent screens is the person's to choose, from
        // their own display: never an agent's on a screen (which could
        // share itself more), nor a screen's Passwords' to say.
        if matches!(incoming.request, Request::Shares | Request::SetShares { .. }) && (olo_keychain::shared::this_screen().is_some() || olo_keychain::shared::on_a_screen(incoming.pid)) {
            let message = "What’s shared with agent screens is chosen in Agent Screens on your own display.".into();
            return incoming.reply(Response::Error { message });
        }
        if let Request::Cli { query } = &incoming.request {
            if !self.security.op_cli {
                let message = "Olo’s op is turned off (Preferences ▸ Security).".into();
                return incoming.reply(Response::Error { message });
            }
            // Which accounts there are needs no permission (or unlocking).
            if matches!(query, olo_keychain::CliQuery::Accounts) {
                let linked = self.settings.accounts.clone();
                std::thread::spawn(move || incoming.reply(Response::Cli { json: cli_accounts(&linked) }));
                return;
            }
        }
        if matches!(incoming.request, Request::SshKeys | Request::SshSign { .. }) && !self.security.ssh_agent {
            let message = "Passwords isn’t the SSH agent (Preferences ▸ Security).".into();
            return incoming.reply(Response::Error { message });
        }
        if !self.set_up() {
            return incoming.reply(Response::NotSetUp);
        }
        // Locked, a page's passkeys are none if the index (kept while
        // locked) has none it may use: Chrome uses its own without asking
        // for an unlock. Else locked (it asks to unlock for one).
        if self.lock.locked
            && let Request::Passkeys { url, rp_id, allow } = &incoming.request
            && !self.index.is_empty()
            && (!olo_keychain::rp_id_fits(rp_id, url) || self.passkeys_for(rp_id, allow).is_empty())
        {
            return incoming.reply(Response::Passkeys { passkeys: Vec::new() });
        }
        let waits = !matches!(incoming.request, Request::Logins { .. } | Request::Cards { .. } | Request::Check { .. } | Request::Search { .. } | Request::Passkeys { .. });
        if self.lock.locked {
            if !waits {
                return incoming.reply(Response::Locked);
            }
            // Unlocked for it while approving without asking (Deskbar's
            // key); else come forward to be unlocked. The answer follows.
            self.waiting.push(incoming);
            self.ask_approval(None);
            self.changed();
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
            // An Olo program's own secrets: the items with that title.
            Request::KeepSecret { title, .. } | Request::Secrets { title } => {
                let title = title.clone();
                let candidates = self.items.iter().filter(|i| i.state == ItemState::Active && i.title == title).map(|i| (i.vault_id.clone(), i.id.clone())).collect();
                let Some(vault) = self.new_item_vault() else {
                    // Nowhere to keep one: none is kept (asked for one), or
                    // it can't be (asked to keep one).
                    if matches!(incoming.request, Request::Secrets { .. }) {
                        return incoming.reply(Response::Secrets { values: Vec::new() });
                    }
                    return incoming.reply(Response::Error { message: "There’s no vault to keep it in.".into() });
                };
                return self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates, vault });
            }
            Request::Shares | Request::SetShares { .. } | Request::SshKeys => {
                return self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates: Vec::new(), vault: String::new() });
            }
            // A signature with an SSH key: once the asking program's
            // terminal session is allowed, as `op` is (one grant for both).
            Request::SshSign { .. } => {
                let Some(session) = cli::session_of(incoming.pid) else {
                    return incoming.reply(Response::Error { message: "the program went away".into() });
                };
                if !on_screen() && !self.grants.check(session) {
                    self.cli_waiting.push((session, incoming));
                    self.ask_approval(Some(session));
                    return;
                }
                return self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates: Vec::new(), vault: String::new() });
            }
            Request::Cli { query } => {
                let Some(session) = cli::session_of(incoming.pid) else {
                    return incoming.reply(Response::Error { message: "op went away".into() });
                };
                // (On a screen: what it was given is the agents' there.)
                if !on_screen() && !self.grants.check(session) {
                    // Ask once per session, however many are waiting.
                    // Allowed while approving without asking; else asked.
                    self.cli_waiting.push((session, incoming));
                    self.ask_approval(Some(session));
                    return;
                }
                if matches!(query, olo_keychain::CliQuery::Signin) {
                    return incoming.reply(Response::Cli { json: serde_json::json!({}) });
                }
                self.store.send(Job::Cli { incoming: Box::new(incoming) });
                return;
            }
            Request::Search { query } => {
                let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
                let mut items: Vec<olo_keychain::Found> = self
                    .items
                    .iter()
                    .filter(|i| i.state == ItemState::Active)
                    .filter(|i| {
                        !words.is_empty()
                            && words
                                .iter()
                                .all(|w| i.title.to_lowercase().contains(w) || i.websites.iter().any(|s| s.url.to_lowercase().contains(w)) || i.tags.iter().any(|t| t.to_lowercase().contains(w)))
                    })
                    .map(|i| olo_keychain::Found { vault: i.vault_id.clone(), id: i.id.clone(), title: i.title.clone(), subtitle: self.subtitle(i) })
                    .collect();
                items.sort_by_cached_key(|f| (f.title.to_lowercase(), f.id.clone()));
                items.truncate(50);
                return incoming.reply(Response::Items { items });
            }
            Request::Show { id, .. } => {
                let id = id.clone();
                if self.overview(&id).is_some() {
                    self.set_source(Source::All);
                    self.set_search(String::new(), true);
                    self.select(Some(id));
                    self.reveal_selection = true;
                }
                self.open_main();
                self.changed();
                return incoming.reply(Response::Shown);
            }
            // Credit cards: every one for the menu (not kept for sites);
            // only a card, and the one chosen, to fill.
            Request::Cards { .. } | Request::FillCard { .. } => {
                let chosen = match &incoming.request {
                    Request::FillCard { vault, id, .. } => Some((vault.clone(), id.clone())),
                    _ => None,
                };
                let candidates = self
                    .items
                    .iter()
                    .filter(|i| i.state == ItemState::Active && i.category == ItemCategory::CreditCard)
                    .map(|i| (i.vault_id.clone(), i.id.clone()))
                    .filter(|c| chosen.as_ref().is_none_or(|w| w == c))
                    .collect();
                return self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates, vault: String::new() });
            }
            Request::Logins { url } | Request::Fill { url, .. } | Request::Check { url, .. } | Request::Save { url, .. } => {
                let url = url.clone();
                Box::new(move |w| store::fits(w, &url))
            }
            Request::Passkeys { url, rp_id, allow } => {
                let passkeys = if olo_keychain::rp_id_fits(rp_id, url) { self.passkeys_for(rp_id, allow) } else { Vec::new() };
                return incoming.reply(Response::Passkeys { passkeys });
            }
            Request::PasskeySign { url, rp_id, allow, chosen, .. } => {
                if !olo_keychain::rp_id_fits(rp_id, url) {
                    let message = format!("This page can’t use passkeys for {rp_id}.");
                    return incoming.reply(Response::Error { message });
                }
                let found = self.passkeys_for(rp_id, allow);
                if found.is_empty() {
                    return incoming.reply(Response::NoPasskey);
                }
                // Picked in Chrome's menu already: signed with at once.
                if let Some(c) = chosen.clone().filter(|(v, id)| found.iter().any(|p| &p.vault == v && &p.id == id)) {
                    return self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates: vec![c], vault: String::new() });
                }
                let rp = rp_id.clone();
                let label = |p: &olo_keychain::Passkey| if p.user_name.is_empty() { p.title.clone() } else { p.user_name.clone() };
                let message = match found.as_slice() {
                    [one] => format!("Sign in to {rp} as “{}” with your passkey?", label(one)),
                    _ => format!("Sign in to {rp} with which passkey?"),
                };
                let choices = found.iter().map(|p| ((p.vault.clone(), p.id.clone()), label(p))).collect();
                return self.ask_passkey(incoming, message, choices, Vec::new());
            }
            Request::PasskeyCreate { url, rp_id, user_name, exclude, .. } => {
                if !olo_keychain::rp_id_fits(rp_id, url) {
                    let message = format!("This page can’t make passkeys for {rp_id}.");
                    return incoming.reply(Response::Error { message });
                }
                if !exclude.is_empty() && !self.passkeys_for(rp_id, exclude).is_empty() {
                    return incoming.reply(Response::PasskeyExists);
                }
                // The logins it may join: the site's, and any with a
                // passkey for it.
                let near: Vec<&ItemOverview> = self
                    .items
                    .iter()
                    .filter(|i| i.state == ItemState::Active)
                    .filter(|i| i.websites.iter().any(|w| passkey::near(&w.url, rp_id)) || self.index.get(&i.id).and_then(|s| s.passkey.as_ref()).is_some_and(|p| p.rp_id.eq_ignore_ascii_case(rp_id)))
                    .collect();
                let joins = near.iter().find(|i| self.username_of(i).eq_ignore_ascii_case(user_name) && !user_name.is_empty());
                let who = if user_name.is_empty() { String::new() } else { format!(" for “{user_name}”") };
                let message = match joins {
                    Some(i) => format!("Save a passkey{who} on {rp_id} in Passwords? It goes with your login “{}”.", i.title),
                    None => format!("Save a passkey{who} on {rp_id} in Passwords?"),
                };
                let candidates = near.iter().map(|i| (i.vault_id.clone(), i.id.clone())).collect();
                return self.ask_passkey(incoming, message, Vec::new(), candidates);
            }
        };
        let candidates: Vec<(String, String)> = self.items.iter().filter(|i| i.state == ItemState::Active && i.websites.iter().any(&fits)).map(|i| (i.vault_id.clone(), i.id.clone())).collect();
        let Some(vault) = self.new_item_vault() else {
            return incoming.reply(Response::Error { message: "There’s no vault to save in.".into() });
        };
        self.store.send(Job::Keychain { incoming: Box::new(incoming), candidates, vault });
    }

    /// Where a login made for another app goes.
    fn new_item_vault(&self) -> Option<String> {
        self.vaults.iter().find(|v| matches!(v.title.as_str(), "Personal" | "Private" | "Employee")).or(self.vaults.first()).map(|v| v.id.clone())
    }

    /// The passkeys kept for `rp_id` (those in `allow`, if any), by user.
    fn passkeys_for(&self, rp_id: &str, allow: &[String]) -> Vec<olo_keychain::Passkey> {
        let mut found: Vec<olo_keychain::Passkey> = self
            .items
            .iter()
            .filter(|i| i.state == ItemState::Active)
            .filter_map(|i| {
                let p = self.index.get(&i.id)?.passkey.as_ref()?;
                let fits = p.rp_id.eq_ignore_ascii_case(rp_id) && (allow.is_empty() || allow.contains(&p.credential_id));
                fits.then(|| olo_keychain::Passkey { vault: i.vault_id.clone(), id: i.id.clone(), title: i.title.clone(), user_name: p.user_name.clone(), credential_id: p.credential_id.clone() })
            })
            .collect();
        found.sort_by_cached_key(|p| (p.user_name.to_lowercase(), p.title.to_lowercase()));
        found
    }

    /// Ask the person about a page's passkey, in an alert in front: to
    /// sign in with one of `choices`, or (with none) to save a new one.
    fn ask_passkey(&mut self, incoming: Incoming, message: String, choices: Vec<((String, String), String)>, candidates: Vec<(String, String)>) {
        PasskeyAsk::open(incoming, message, choices, candidates);
    }

    /// The person answered a passkey alert with button `choice` (None:
    /// closed, or the page gave up).
    fn answer_passkey(&mut self, incoming: Box<Incoming>, choices: Vec<((String, String), String)>, chosen: usize, candidates: Vec<(String, String)>, choice: Option<usize>) {
        match choice {
            Some(0) => incoming.reply(Response::OtherDevice),
            Some(2) => {
                let candidates = if choices.is_empty() { candidates } else { choices.into_iter().nth(chosen).map(|(c, _)| c).into_iter().collect() };
                let vault = self.new_item_vault().unwrap_or_default();
                self.store.send(Job::Keychain { incoming, candidates, vault });
            }
            _ => incoming.reply(Response::Declined),
        }
    }

    /// A touch, a finger or a look allowed a terminal session: its alert
    /// goes, and a quiet notification says so.
    fn allowed(&mut self, allow::Allowed { session, how }: allow::Allowed) {
        let Some(asked) = self.allow_shown.remove(&session) else { return };
        tracing::info!(command = asked.command, how, "op allowed");
        let what = if asked.ssh { "sign in with your SSH keys" } else { "use your passwords" };
        quietly(&self.notifier, &format!("Allowed with {how}"), format!("“{}” can {what}.", asked.command));
        if let Some(w) = asked.window {
            w.close();
        }
        self.answer_cli(session, true);
    }

    /// The person allowed (or denied) a terminal session's `op`.
    fn answer_cli(&mut self, session: cli::Session, allow: bool) {
        self.allow_shown.remove(&session);
        if allow {
            self.grants.allow(session);
        }
        let (theirs, rest) = std::mem::take(&mut self.cli_waiting).into_iter().partition::<Vec<_>, _>(|(s, _)| *s == session);
        self.cli_waiting = rest;
        for (_, incoming) in theirs {
            if allow {
                self.keychain(incoming);
            } else {
                incoming.reply(Response::Error { message: "Passwords: not allowed".into() });
            }
        }
    }

    /// Ask app_server (on a thread) whether it approves requests without
    /// asking now, for the unlock (None) or `op` in `session`; `approval`
    /// goes on from the answer.
    fn ask_approval(&mut self, about: Option<cli::Session>) {
        let shown = about.is_some_and(|s| self.allow_shown.contains_key(&s));
        if shown || !self.asking.insert(about) {
            return;
        }
        let (proxy, user) = (self.proxy.clone(), self.lock.user().to_owned());
        std::thread::spawn(move || {
            let on = approving_without_asking();
            let ways = if about.is_some() && !on { allow::Ways::find(&user) } else { allow::Ways::default() };
            proxy.send(Approval { about, on, ways })
        });
    }

    /// Unlocked or allowed without asking, said in a quiet notification;
    /// else asked as usual.
    fn approval(&mut self, Approval { about, on, ways }: Approval) {
        self.asking.remove(&about);
        match about {
            None if !self.lock.locked => self.serve_waiting(),
            None if self.waiting.is_empty() => {}
            None if on => {
                let mut from: Vec<&str> = self.waiting.iter().map(|i| i.from.as_str()).collect();
                from.dedup();
                let from = from.join(", ");
                tracing::info!(%from, "unlocked without asking");
                quietly(&self.notifier, "Passwords unlocked without asking", format!("For {from}."));
                if self.lock.approved() {
                    self.now_unlocked();
                }
            }
            None => self.open_main(),
            Some(session) => {
                let Some(pid) = self.cli_waiting.iter().find(|(s, _)| *s == session).map(|(_, i)| i.pid) else { return };
                let ssh = self.cli_waiting.iter().any(|(s, i)| *s == session && matches!(i.request, Request::SshSign { .. }));
                let asker = cli::asker(pid);
                if on {
                    tracing::info!(command = asker.command, "op allowed without asking");
                    quietly(&self.notifier, "op allowed without asking", asker.command.clone());
                    self.answer_cli(session, true);
                } else {
                    // Allowed by its button, or by what unlocks Passwords.
                    let message = allow_message(&asker, ssh, ways.sentence());
                    let waits = allow::Waits::start(ways, self.lock.user(), session, &self.proxy);
                    let window = open_app_alert("Passwords", "warning", &message, &["Deny", "Allow"], move |choice| app().answer_cli(session, choice == Some(1)));
                    self.allow_shown.insert(session, Allowing { window: Some(window), _waits: waits, command: asker.command, ssh });
                }
            }
        }
    }

    /// Just unlocked (Touch ID, the password, or approved): the store
    /// opens and what waited is answered.
    fn now_unlocked(&mut self) {
        self.last_use = Instant::now();
        self.unlocked = true;
        self.ui.clear_lock = true;
        // The store opens (again) now its secrets are wanted.
        self.connect();
        if self.selected.is_none() {
            self.selected = self.visible().first().map(|i| i.id.clone());
        }
        self.fetch_selected();
        self.serve_waiting();
    }

    /// Answer what waited for an unlock or for the items.
    fn serve_waiting(&mut self) {
        for incoming in std::mem::take(&mut self.waiting) {
            self.keychain(incoming);
        }
    }

    /// The password typed in the lock panel, checked.
    pub fn submit_password(&mut self) {
        self.lock.submit();
        // (The lock took the password; the field lets go of it too.)
        self.ui.clear_lock = true;
        self.changed();
    }

    fn do_lock(&mut self) {
        if on_screen() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Lock);
            return;
        }
        self.close_editor();
        // Secrets go; overviews stay (hidden) so unlocking is instant.
        self.details.clear();
        self.revealed.clear();
        self.invalidate();
        if let Some(w) = self.generator.take() {
            w.messenger().send(GeneratorMsg::Close);
        }
        // A waiting plan holds secrets too; it is worked out again later.
        self.end_review();
        // The store lets go of its secrets (after a running sync), and
        // `op` must be allowed again; Undo's steps hold them too.
        self.store.send(Job::Close);
        self.history.clear();
        self.grants.clear();
        self.lock.lock();
        self.ui.clear_lock = true;
        self.changed();
    }

    /// A secret on the clipboard, taken off again after the time chosen
    /// in Preferences ▸ Security.
    pub fn copy_secret(&mut self, what: &str, text: &str) {
        let seconds = self.security.clear_seconds;
        olo_kit::copy::secret(text, Duration::from_secs(seconds as u64));
        self.secret_until = Some(Instant::now() + Duration::from_secs(seconds as u64));
        self.flash(format!("Copied {what}. It leaves the clipboard in {seconds} seconds."));
        self.changed();
    }

    /// What dragging a field out takes (onto another app's field): its
    /// value (a one-time password's current code) and its name, which
    /// the drag's picture shows, never the secret. `field` is `url:…`
    /// for a website.
    pub fn dragged_field(&self, item: &str, field: &str) -> Option<(String, String)> {
        match field.strip_prefix("url:") {
            Some(url) => Some((url.to_owned(), "website".to_owned())),
            None => {
                let f = self.details.get(item).and_then(|i| i.field(field))?;
                let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
                let value = match f.field_type {
                    ItemFieldType::Totp => otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone()))?,
                    _ => f.value.clone(),
                };
                Some((value, label))
            }
        }
    }

    pub fn copy_field(&mut self, item: &str, field: &str) {
        if let Some(url) = field.strip_prefix("url:") {
            olo_kit::clipboard::write(ClipboardItem::new_string(url.to_owned()));
            self.flash("Copied the website.");
            self.changed();
            return;
        }
        let Some(f) = self.details.get(item).and_then(|i| i.field(field)).cloned() else { return };
        let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
        match f.field_type {
            ItemFieldType::Totp => {
                if let Some(code) = otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone())) {
                    self.copy_secret("the one-time password", &code);
                }
            }
            t if edit::concealed(t) || t == ItemFieldType::CreditCardNumber => self.copy_secret(&format!("the {label}"), &f.value),
            _ => {
                olo_kit::clipboard::write(ClipboardItem::new_string(f.value.clone()));
                self.flash(format!("Copied the {label}."));
                self.changed();
            }
        }
    }

    /// A secret field shown, or hidden again.
    pub fn toggle_revealed(&mut self, field: String) {
        if !self.revealed.remove(&field) {
            self.revealed.insert(field);
        }
        self.changed();
    }

    pub fn revealed(&self, field: &str) -> bool {
        self.revealed.contains(field)
    }

    /// The selected item's username, password or one-time code.
    /// An `op://` reference to field `field` of `item` (its main one when
    /// None: `cli::main_field`), for the `op` command (Ben, 2026-11-05:
    /// "right click and get a reference that will work in the op cli
    /// shim"): copied as plain text, no secret in it.
    pub fn copy_reference(&mut self, item: &str, field: Option<&str>) {
        let Some(i) = self.details.get(item).cloned() else { return };
        let vault = self.vaults.iter().find(|v| v.id == i.vault_id).map_or_else(|| i.vault_id.clone(), |v| v.title.clone());
        let found = match field {
            Some("notesPlain") => Some((cli::notes_reference(&vault, &i), "notes".to_owned())),
            Some(f) => i.field(f).map(|f| (cli::reference(&vault, &i, f), if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() })),
            None => cli::main_field(&i).map(|f| (cli::reference(&vault, &i, f), if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() })),
        };
        match found {
            Some((reference, what)) => {
                olo_kit::clipboard::write(ClipboardItem::new_string(reference));
                self.flash(format!("Copied the reference to “{}”’s {what}.", i.title));
            }
            None => self.flash(format!("“{}” has nothing to refer to.", i.title)),
        }
        self.changed();
    }

    fn copy_selected(&mut self, kind: &str) {
        if kind == "reference" {
            if let Some(id) = self.selected.clone() {
                self.copy_reference(&id, None);
            }
            return;
        }
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)).cloned() else { return };
        let field = match kind {
            "username" => {
                item.fields.iter().find(|f| f.id == "username").or_else(|| item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Email) || f.title.eq_ignore_ascii_case("username")))
            }
            "password" => item.fields.iter().find(|f| f.id == "password").or_else(|| item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed)),
            _ => item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp),
        };
        match field {
            Some(f) => self.copy_field(&item.id, &f.id.clone()),
            None => self.flash(format!("“{}” has no {}.", item.title, if kind == "otp" { "one-time password" } else { kind })),
        }
        self.changed();
    }

    pub fn open_website(&mut self) {
        let Some(url) = self.selected.as_ref().and_then(|s| self.overview(s)).and_then(|o| o.websites.first()).map(|w| w.url.clone()) else { return };
        open_url(&url);
        self.flash(format!("Opening {}", host(&url).unwrap_or(url)));
        self.changed();
    }

    /// The editor closed (saved, cancelled or overtaken).
    fn close_editor(&mut self) {
        self.editor = None;
        self.ui.form += 1;
    }

    /// The editor opened on `editor`: its fields are made (by the item
    /// pane, as it next shows), the title given the keyboard.
    fn open_editor(&mut self, editor: Editor) {
        // A new item's title takes the keyboard (an item edited keeps it
        // where it was).
        self.focus_title = editor.base.is_none();
        self.editor = Some(editor);
        self.ui.form += 1;
        self.ui.editor_top = true;
        self.changed();
    }

    /// The editor's fields came or went: its form made again.
    fn rebuild_form(&mut self) {
        self.ui.form += 1;
        self.changed();
    }

    fn new_item(&mut self, category: ItemCategory) {
        if on_screen() {
            return self.flash(SHARED_ONLY);
        }
        if self.lock.locked || self.conn != Conn::Ready || self.vaults.is_empty() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::New(category));
            return;
        }
        // The vault being looked at, else the first (Personal).
        let vault = match &self.source {
            Source::Vault(v) => self.vaults.iter().position(|x| &x.id == v).unwrap_or(0),
            _ => 0,
        };
        self.open_editor(Editor::new_item(category, vault, None));
    }

    pub fn edit_selected(&mut self) {
        if on_screen() {
            return self.flash(SHARED_ONLY);
        }
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)) else { return };
        if self.overview(&item.id).is_some_and(|o| o.state == ItemState::Archived) {
            return;
        }
        let vault = self.vaults.iter().position(|v| v.id == item.vault_id).unwrap_or(0);
        let editor = Editor::edit(item, vault);
        self.open_editor(editor);
    }

    pub fn save_editor(&mut self) {
        let Some(editor) = &mut self.editor else { return };
        if editor.base.is_some() && !editor.changed() {
            self.close_editor();
            self.changed();
            return;
        }
        // What 1Password wouldn't take is said here, not found at sync,
        // under the form (scrolled to: it's below the fold on a long item).
        if !editor.ready() {
            self.ui.editor_bottom = true;
            self.changed();
            return;
        }
        let tag = |name: String, before: Option<Box<onepassword::Item>>| Some(store::Tag { purpose: olo_kit::undo::Purpose::Do, name, before });
        if let Some(base) = editor.base.clone() {
            if let Some(item) = editor.to_item() {
                let name = format!("Changes to “{}”", base.title);
                self.store.send(Job::Put(Box::new(item), tag(name, Some(Box::new(base)))));
                self.flash("Saving…");
            }
        } else if let Some(params) = editor.to_params(&self.vaults) {
            let name = format!("New “{}”", params.title);
            self.store.send(Job::Create(Box::new(params), tag(name, None)));
            self.flash("Saving…");
        }
        self.changed();
    }

    pub fn cancel_editor(&mut self) {
        self.close_editor();
        self.changed();
    }

    /// The generator, opened for an editor's field (its New).
    pub fn generate_for(&mut self, field: String) {
        let item = self.editor.as_ref().and_then(|e| e.item_id()).unwrap_or("").to_string();
        self.open_generator(Some((item, field)));
    }

    pub fn toggle_edit_reveal(&mut self, i: usize) {
        let Some(f) = self.editor.as_mut().and_then(|e| e.fields.get_mut(i)) else { return };
        f.reveal = !f.reveal;
        self.changed();
    }

    pub fn remove_edit_field(&mut self, i: usize) {
        if let Some(e) = self.editor.as_mut()
            && i < e.fields.len()
        {
            e.fields.remove(i);
            self.rebuild_form();
        }
    }

    /// A field of `kind` added to `section` (a titled one; `None`: the
    /// first group), and focused: its label if it has none yet, else its
    /// value.
    pub fn add_edit_field(&mut self, kind: ItemFieldType, section: Option<&str>) {
        if let Some(e) = self.editor.as_mut() {
            let at = e.add_field(kind, section);
            self.focus_field = Some(at);
            self.rebuild_form();
        }
    }

    pub fn remove_edit_website(&mut self, i: usize) {
        if let Some(e) = self.editor.as_mut()
            && i < e.websites.len()
        {
            e.websites.remove(i);
            self.rebuild_form();
        }
    }

    pub fn add_edit_website(&mut self) {
        if let Some(e) = self.editor.as_mut() {
            e.websites.push((String::new(), "website".into(), onepassword::AutofillBehavior::AnywhereOnWebsite));
            self.rebuild_form();
        }
    }

    /// The generator's Use: its password into the field it was opened
    /// for (`target`: the item and the field).
    pub fn use_generated(&mut self, password: String, target: Option<(String, String)>) {
        let Some((item, field)) = target else { return };
        let Some(editor) = &mut self.editor else { return };
        if editor.item_id().unwrap_or("") != item {
            return;
        }
        let Some(i) = editor.fields.iter().position(|f| f.id == field) else { return };
        editor.fields[i].value = password.clone();
        editor.fields[i].reveal = true;
        self.ui.form_value = Some((i, password));
        self.changed();
    }

    /// The generator's window went.
    pub fn generator_closed(&mut self) {
        self.generator = None;
        self.save_settings();
    }

    fn ask_discard(&mut self, then: Then) {
        let then = Mutex::new(Some(then));
        self.alert("Passwords", "warning", "Discard the changes to this item?".into(), &["Keep Editing", "Discard"], move |choice| {
            if choice == Some(1)
                && let Some(then) = then.lock().unwrap_or_else(|e| e.into_inner()).take()
            {
                app().after_discard(then);
            }
        });
    }

    fn show_error(&mut self, message: String) {
        self.alert("Passwords", "stop", message, &["OK"], |_| {});
    }

    /// The whole of a sync problem, from the status line.
    pub fn show_problem(&mut self, message: String) {
        self.show_error(message);
    }

    /// What a sync couldn't do, item by item, grouped by vault.
    pub fn show_problems(&mut self, list: Vec<sync::Problem>) {
        let list = sync::grouped(list);
        let open = self.problems.as_ref().and_then(|p| p.window.as_ref()).filter(|w| w.is_open()).cloned();
        match open {
            Some(w) => {
                self.problems = Some(Problems { window: Some(w.clone()), list });
                olo_kit::raise(Some(&w));
            }
            None => {
                let window = olo_kit::open_window(WindowOptions::new("Sync Problems", 520.0, 400.0).min_size(400.0, 260.0).dialog().child(), ProblemsView::new);
                self.problems = Some(Problems { window: Some(window), list });
            }
        }
        self.changed();
    }

    /// The Sync Problems window went (closed from its tab, or by us).
    pub fn problems_gone(&mut self) {
        self.problems = None;
    }

    /// The Sync Problems window closed from its buttons.
    pub fn close_problems(&mut self) {
        if let Some(w) = self.problems.take().and_then(|p| p.window) {
            w.close();
        }
    }

    /// The review's Apply or Cancel.
    pub fn answer_review(&mut self, apply: bool) {
        let Some(plan) = self.review.as_ref().map(|r| r.plan) else { return };
        self.store.send(if apply { Job::Approve { plan } } else { Job::Decline });
        self.end_review();
    }

    /// Edit ▸ Undo (or Redo): the step's inverse as a save, a creation or
    /// a deletion, its reply recording what it did for the other way.
    fn undo_redo(&mut self, redo: bool) {
        let step = if redo { self.history.take_redo() } else { self.history.take_undo() };
        let Some(step) = step else { return };
        let purpose = if redo { olo_kit::undo::Purpose::Redo } else { olo_kit::undo::Purpose::Undo };
        let name = step.name.clone();
        self.store.send(step.change.inverse(|before| Some(store::Tag { purpose, name, before })));
        self.flash(if redo { "Redoing…" } else { "Undoing…" });
        self.changed();
    }

    fn confirm_delete(&mut self) {
        if on_screen() {
            return self.flash(SHARED_ONLY);
        }
        let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)).cloned() else { return };
        // (Undone, it's made again: not with files attached.)
        let back = self.details.get(&o.id).is_none_or(undo::can_come_back);
        let message = if back {
            format!("Delete “{}”? Edit ▸ Undo makes it again until Passwords locks.", o.title)
        } else {
            format!("Delete “{}” for good? Its attached files can’t come back.", o.title)
        };
        let tag = back.then(|| store::Tag { purpose: olo_kit::undo::Purpose::Do, name: format!("Delete “{}”", o.title), before: None });
        self.alert("Delete Item", "warning", message, &["Cancel", "Delete"], move |choice| {
            if choice == Some(1) {
                app().store.send(Job::Delete { vault: o.vault_id.clone(), item: o.id.clone(), tag: tag.clone() });
            }
        });
    }

    fn after_discard(&mut self, then: Then) {
        self.close_editor();
        match then {
            Then::Select(id) => self.select(id),
            Then::Source(s) => self.set_source(s),
            Then::New(c) => self.new_item(c),
            Then::Close => self.quit(),
            Then::Lock => self.do_lock(),
        }
        self.changed();
    }

    /// Passwords ends (its main window closed, or Quit): the generator's
    /// recipe kept. With a secret it copied still on the clipboard it
    /// runs on without its windows until that has left (so it can still
    /// be pasted), and ends then; opened meanwhile, it carries on.
    fn quit(&mut self) {
        self.save_settings();
        if self.secret_until.is_some_and(|t| t > Instant::now()) {
            // (Its windows go; nothing else of it does.)
            olo_kit::stay_resident();
            self.leaving = true;
            if let Some(w) = self.main.take() {
                w.close();
            }
            if let Some(w) = self.generator.take() {
                w.messenger().send(GeneratorMsg::Close);
            }
            if let Some(w) = self.problems.take().and_then(|p| p.window) {
                w.close();
            }
            if let Some(w) = self.review.as_mut().and_then(|r| r.window.take()) {
                w.close();
            }
            self.changed();
            return;
        }
        self.main = None;
        olo_kit::quit();
    }

    pub fn open_generator(&mut self, target: Option<(String, String)>) {
        if let Some(w) = self.generator.as_ref().filter(|w| w.is_open()) {
            w.messenger().send(GeneratorMsg::Target(target));
            return;
        }
        let generator = Generator::new(self.settings.generator.clone(), target);
        // (A panel beside the app's window: it floats, never a tab.)
        self.generator = Some(olo_kit::open_window(WindowOptions::new("Password Generator", 380.0, 330.0).min_size(340.0, 300.0).dialog(), move |cx| GeneratorView::new(generator, cx)));
    }

    fn open_main(&mut self) {
        self.leaving = false;
        // (Gone if it was closed while we hold the clipboard.)
        if let Some(w) = self.main.as_ref().filter(|w| w.is_open()) {
            olo_kit::raise(Some(w));
            return;
        }
        let (w, h) = self.settings.size.unwrap_or((920, 580));
        self.main = Some(olo_kit::open_window(WindowOptions::new("Passwords", w as f32, h as f32).min_size(680.0, 420.0).keep_size("main"), MainView::new));
    }

    /// The main window's close: asks about an edit first; else Passwords
    /// ends (and so do its other windows).
    pub fn close_requested(&mut self) -> bool {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Close);
            return false;
        }
        self.quit();
        true
    }

    fn apply_reply(&mut self, reply: Reply) {
        // (The list and the filters' values and counts follow the items.
        // Not an item's own details coming in, for one the index has:
        // what the list and the filters show of it they have from
        // there, and each item the arrows pass would have had the whole
        // list and every filter worked out again.)
        let listed = !matches!(&reply, Reply::Item(item) if self.index.contains_key(&item.id));
        if listed {
            self.invalidate();
        }
        match reply {
            Reply::Index(index) => self.index = index,
            Reply::Connected => {
                self.conn = Conn::Ready;
                if self.security.auto_sync && self.sync_asked.is_none_or(|t| t.elapsed() > SYNC_EVERY) {
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
                    self.end_review();
                }
                if !matches!(state, SyncState::Running | SyncState::Review { .. }) {
                    self.show_review = false;
                }
                self.sync = Some(state);
            }
            Reply::Review { plan, rows, problems } => {
                let window = self.review.take().and_then(|r| r.window);
                let open = window.is_some();
                self.review = Some(Review { plan, rows, problems, window });
                if std::mem::take(&mut self.show_review) || open {
                    self.open_review();
                }
            }
            Reply::Reset => self.after_reset(),
            Reply::Failed(message) => {
                self.conn = Conn::Failed(message);
                self.loading = false;
                self.serve_waiting();
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
                self.invalidate();
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
                self.serve_waiting();
            }
            Reply::Item(item) => {
                self.missing.remove(&item.id);
                self.details.insert(item.id.clone(), *item);
            }
            Reply::Missing { item, error } => {
                self.missing.insert(item, error);
            }
            Reply::Saved { item, created, tag } => {
                if let Some(tag) = &tag
                    && let Some(change) = undo::Change::of_saved(&item, created, tag)
                {
                    self.history.record(tag.purpose, tag.name.clone(), change);
                }
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
                self.invalidate();
                let title = item.title.clone();
                self.selected = Some(item.id.clone());
                self.reveal_selection = true;
                self.details.insert(item.id.clone(), *item);
                if self.editor.as_ref().is_some_and(|e| e.item_id().is_none() == created) {
                    self.close_editor();
                }
                if created && !self.visible().iter().any(|i| Some(&i.id) == self.selected.as_ref()) {
                    self.source = Source::All;
                    self.set_search(String::new(), true);
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
                self.invalidate();
                self.after_removal(&item, row);
                self.changed_here();
            }
            Reply::Deleted { item, was, tag } => {
                if let (Some(tag), Some(was)) = (tag, was)
                    && undo::can_come_back(&was)
                {
                    self.history.record(tag.purpose, tag.name, undo::Change::Deleted(was));
                }
                let row = self.visible().iter().position(|i| i.id == item);
                if let Some(i) = self.items.iter().find(|i| i.id == item) {
                    self.flash(format!("Deleted “{}”.", i.title));
                }
                self.items.retain(|i| i.id != item);
                self.details.remove(&item);
                self.invalidate();
                self.after_removal(&item, row);
                self.changed_here();
            }
            Reply::Error(message) => {
                if self.status.says("Saving…") {
                    self.status.clear();
                }
                self.show_error(message);
            }
        }
        if listed {
            self.invalidate();
        }
        self.changed();
    }

    /// Once `gone` (listed at `row`) leaves the list, select what took
    /// its place.
    fn after_removal(&mut self, gone: &str, row: Option<usize>) {
        if self.selected.as_deref() != Some(gone) || self.visible().iter().any(|i| i.id == gone) {
            return;
        }
        self.close_editor();
        let visible = self.visible();
        let next = row.unwrap_or(0).min(visible.len().saturating_sub(1));
        self.selected = visible.get(next).map(|i| i.id.clone());
        self.reveal_selection = true;
        self.fetch_selected();
    }

    /// Something changed here: send it to 1Password soon.
    fn changed_here(&mut self) {
        if !self.settings.accounts.is_empty() && self.security.auto_sync {
            self.sync_due = Some(Instant::now() + SYNC_AFTER_CHANGE);
        }
    }

    /// When something is next due: a sync, the auto-lock, the status
    /// line clearing.
    fn next_due(&self) -> Option<Instant> {
        let mut due: Vec<Instant> = Vec::new();
        // (Only what the tick would do something about: a sync not while
        // one runs or its review is open, the lock not over an edit.
        // Each is looked at again when those change.)
        let reviewing = self.review.as_ref().is_some_and(|r| r.window.is_some());
        if self.security.auto_sync && !self.lock.locked && self.conn == Conn::Ready && !self.settings.accounts.is_empty() && self.sync != Some(SyncState::Running) && !reviewing {
            due.extend(self.sync_due);
            due.extend(self.sync_asked.map(|t| t + SYNC_EVERY));
        }
        if !self.lock.locked && self.security.lock_minutes > 0 && !on_screen() && self.editor.as_ref().is_none_or(|e| !e.changed()) {
            due.push(self.last_use + Duration::from_secs(self.security.lock_minutes as u64 * 60));
        }
        due.extend(self.status.until());
        if self.leaving {
            due.extend(self.secret_until);
        }
        due.into_iter().min()
    }

    /// What's next due has come (the app's looper's timer).
    fn tick(&mut self) {
        let now = Instant::now();
        // Closed with a secret copied: that has left the clipboard now.
        if self.leaving && self.secret_until.is_none_or(|t| now >= t) {
            self.main = None;
            return olo_kit::quit();
        }
        // (Syncing only when asked: File ▸ Sync Now.)
        let due = self.security.auto_sync && (self.sync_due.is_some_and(|t| now >= t) || self.sync_asked.is_some_and(|t| now >= t + SYNC_EVERY));
        // Not while the person is looking at what the last one found.
        if due && self.review.as_ref().is_none_or(|r| r.window.is_none()) {
            self.sync_now();
        }
        let idle = now.saturating_duration_since(self.last_use);
        if !self.lock.locked && self.security.lock_minutes > 0 && idle >= Duration::from_secs(self.security.lock_minutes as u64 * 60) && self.editor.as_ref().is_none_or(|e| !e.changed()) {
            self.do_lock();
        }
        self.changed();
    }

    pub fn menus(&self) -> Vec<Menu> {
        let ready = self.conn == Conn::Ready && !self.lock.locked;
        let editing = self.editor.is_some();
        let selected = self.selected.as_ref().and_then(|s| self.items.iter().find(|i| &i.id == s));
        let has = ready && selected.is_some() && !editing;
        let archived = selected.is_some_and(|s| s.state == ItemState::Archived);
        let details = self.selected.as_ref().and_then(|s| self.details.get(s));
        let has_otp = details.is_some_and(|d| d.fields.iter().any(|f| f.field_type == ItemFieldType::Totp));
        let has_site = selected.is_some_and(|s| !s.websites.is_empty());

        let shared = on_screen();
        let file = Menu::new("File")
            .submenu(category_menu(Menu::new("New Item"), "new:"))
            .enabled(ready && !shared)
            .separator()
            .item("Password Generator", "generator")
            .key("g")
            .separator()
            .item("Lock", "lock")
            .key("l")
            .enabled(!self.lock.locked && !shared)
            .item("Sync Now", "sync")
            .enabled(ready && !self.settings.accounts.is_empty() && self.sync != Some(SyncState::Running) && !shared)
            .item("1Password Accounts…", "account")
            .enabled(!self.lock.locked && !shared)
            .item("Reset Passwords…", "reset")
            .enabled(!self.lock.locked && self.set_up() && !shared)
            .separator()
            .item("Close", "close")
            .key("w")
            .item("Quit", "quit")
            .key("q");
        let edit = self
            .history
            .edit_items(Menu::new("Edit"))
            .separator()
            .item("Copy Username", "copy:username")
            .key("c")
            .enabled(has && details.is_some())
            .item("Copy Password", "copy:password")
            .shift_key("c")
            .enabled(has && details.is_some())
            .item("Copy One-Time Password", "copy:otp")
            .shift_key("o")
            .enabled(has && has_otp)
            .item("Copy Reference", "copy:reference")
            .enabled(has && details.is_some())
            .separator()
            .item("Find", "find")
            .key("f")
            .enabled(ready);
        let item = Menu::new("Item")
            .item("Edit", "edit")
            .key("e")
            .enabled(has && details.is_some() && !archived && !shared)
            .item("Open Website", "open")
            .key("o")
            .enabled(has && has_site)
            .separator()
            .item("Move to Archive", "archive")
            .enabled(has && !archived && !shared)
            .item("Delete…", "delete")
            .enabled(has && !shared);
        let mut view = Menu::new("View").item("All Items", "source:all").key("0").checked(self.source == Source::All);
        for (i, v) in self.vaults.iter().enumerate().take(9) {
            // (By place, as app_keys has them: ⌘1 the first vault.)
            view = view.item(v.title.clone(), format!("vault:{i}")).key(&(i + 1).to_string()).checked(self.source == Source::Vault(v.id.clone()));
        }
        view = view.separator().item("Archive", "source:archive").checked(self.source == Source::Archive);
        let mut lock_after = Menu::new("Lock After");
        for (minutes, label) in [(1, "1 Minute"), (5, "5 Minutes"), (10, "10 Minutes"), (30, "30 Minutes"), (60, "1 Hour"), (0, "Never")] {
            lock_after = lock_after.item(label, format!("lockafter:{minutes}")).checked(self.security.lock_minutes == minutes);
        }
        if !shared {
            view = view.separator().submenu(lock_after);
        }
        vec![file, edit, item, view]
    }

    /// The item menu, for a right-click on `id` in the list.
    pub fn item_menu(&self, id: &str) -> Option<Menu> {
        let o = self.overview(id)?;
        let archived = o.state == ItemState::Archived;
        // Right-clicking selected it, so its details are on the way;
        // until they arrive the copies stay enabled.
        let details = self.details.get(id);
        let has = |t: ItemFieldType| details.is_none_or(|d| d.fields.iter().any(|f| f.field_type == t && !f.value.is_empty()));
        Some(
            Menu::new("Item")
                .item("Copy Username", "copy:username")
                .enabled(details.is_none_or(|d| d.username().is_some()))
                .item("Copy Password", "copy:password")
                .enabled(has(ItemFieldType::Concealed))
                .item("Copy One-Time Password", "copy:otp")
                .enabled(has(ItemFieldType::Totp))
                .item("Copy Reference", "copy:reference")
                .separator()
                .item("Open Website", "open")
                .enabled(!o.websites.is_empty())
                .item("Edit", "edit")
                .enabled(!archived)
                .separator()
                .item("Move to Archive", "archive")
                .enabled(!archived)
                .item("Delete…", "delete"),
        )
    }

    pub fn command(&mut self, command: &str) {
        self.last_use = Instant::now();
        match command {
            "quit" | "close" => {
                if self.editor.as_ref().is_some_and(|e| e.changed()) {
                    self.ask_discard(Then::Close);
                } else {
                    self.quit();
                }
            }
            "sync" | "account" | "reset" | "lock" if on_screen() => {}
            "lock" => self.do_lock(),
            "account" => self.change_account(),
            "sync" => self.sync_and_review(),
            "reset" => self.confirm_reset(),
            "generator" => self.open_generator(None),
            "find" => self.ui.focus_search = true,
            "edit" => self.edit_selected(),
            "open" => self.open_website(),
            "archive" if on_screen() => self.flash(SHARED_ONLY),
            "archive" => {
                if let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)) {
                    self.store.send(Job::Archive { vault: o.vault_id.clone(), item: o.id.clone() });
                }
            }
            "delete" => self.confirm_delete(),
            "undo" | "redo" if on_screen() => self.flash(SHARED_ONLY),
            "undo" | "redo" => self.undo_redo(command == "redo"),
            c if c.starts_with("copy:") => self.copy_selected(&c[5..]),
            c if c.starts_with("new:") => {
                if let Some(cat) = ItemCategory::ALL.iter().find(|x| x.as_str() == &c[4..]) {
                    self.new_item(*cat);
                }
            }
            "source:all" => self.set_source(Source::All),
            "source:archive" => self.set_source(Source::Archive),
            c if c.starts_with("source:vault:") => self.set_source(Source::Vault(c[13..].to_string())),
            c if c.starts_with("vault:") => {
                if let Some(v) = c[6..].parse::<usize>().ok().and_then(|i| self.vaults.get(i)) {
                    let id = v.id.clone();
                    self.set_source(Source::Vault(id));
                }
            }
            c if c.starts_with("lockafter:") => {
                // The same choice as Preferences ▸ Security's.
                let minutes = c[10..].parse().unwrap_or(10);
                self.security.lock_minutes = minutes;
                olo_kit::settings::update(move |s| s.security.passwords.lock_minutes = minutes);
            }
            _ => {}
        }
        self.changed();
    }

    fn message(&mut self, msg: Box<dyn std::any::Any + Send>) {
        let msg = match msg.downcast::<FromStore>() {
            Ok(m) => {
                if m.generation == self.store.generation {
                    self.apply_reply(m.reply);
                }
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<LockMsg>() {
            Ok(m) => {
                if self.lock.message(*m) {
                    self.now_unlocked();
                }
                self.changed();
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
                self.changed();
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<PasswordsSecurity>() {
            Ok(security) => {
                if !security.auto_sync {
                    self.sync_due = None;
                }
                self.security = *security;
                self.changed();
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<SharedChanged>() {
            Ok(_) => {
                self.store.send(Job::Close);
                self.history.clear();
                self.connect();
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<ScreenLocked>() {
            Ok(_) => {
                // Not with changes still unsaved: they'd be lost.
                if self.security.lock_with_screen && !self.lock.locked && self.editor.as_ref().is_none_or(|e| !e.changed()) {
                    self.do_lock();
                }
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<Incoming>() {
            Ok(incoming) => return self.keychain(*incoming),
            Err(m) => m,
        };
        let msg = match msg.downcast::<allow::Allowed>() {
            Ok(a) => return self.allowed(*a),
            Err(m) => m,
        };
        let msg = match msg.downcast::<Approval>() {
            Ok(a) => {
                self.approval(*a);
                self.changed();
                return;
            }
            Err(m) => m,
        };
        // Opened (Deskbar, a link), its window shows; olo-session's
        // `--agent` (or a program's `--keychain`) for a Passwords already
        // running asks nothing to show.
        if let Ok(launch) = msg.downcast::<Launch>()
            && !in_background(&launch.args)
        {
            self.open_main();
        }
    }

    /// Up or Down in the search field: the list's selection moves.
    pub fn step_selection(&mut self, up: bool) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let row = visible.iter().position(|i| Some(&i.id) == self.selected.as_ref());
        let next = match (up, row) {
            (true, Some(r)) => r.saturating_sub(1),
            (false, Some(r)) => (r + 1).min(visible.len() - 1),
            (_, None) => 0,
        };
        if Some(next) != row {
            let id = visible[next].id.clone();
            self.select(Some(id));
            self.reveal_selection = true;
        }
    }
}

/// The question of a page's passkey: sign in with one (which, if more
/// than one), save a new one, or use another device; its answer goes to
/// the page. The page giving up (gone, or its time out) closes it.
struct PasskeyAsk {
    incoming: Option<Box<Incoming>>,
    message: SharedString,
    choices: Vec<((String, String), String)>,
    chosen: usize,
    candidates: Vec<(String, String)>,
    focus: FocusHandle,
}

impl PasskeyAsk {
    fn open(incoming: Incoming, message: String, choices: Vec<((String, String), String)>, candidates: Vec<(String, String)>) {
        let buttons = ["Use Another Device…", "Cancel", if choices.is_empty() { "Save Passkey" } else { "Sign In" }];
        let width = alert::alert_width(&buttons);
        let radios = if choices.is_empty() { 0.0 } else { m::SPACING + choices.len() as f32 * (m::CONTROL_HEIGHT + m::TIGHT) };
        let height = alert::alert_height(&message, width) + radios;
        olo_kit::open_window(WindowOptions::alert("Passwords", width, height), move |cx| {
            cx.activate_window();
            let focus = FocusHandle::new();
            focus.focus(cx);
            // The page may give up (gone, or its time out): looked for
            // twice a second while asking, and the question goes with it.
            cx.every(Duration::from_millis(500), || ());
            PasskeyAsk { incoming: Some(Box::new(incoming)), message: message.into(), choices, chosen: 0, candidates, focus }
        });
    }

    /// Answered with button `choice` (None: closed, or the page gave
    /// up: declined).
    fn answer(&mut self, choice: Option<usize>) {
        let Some(incoming) = self.incoming.take() else { return };
        let (choices, candidates, chosen) = (std::mem::take(&mut self.choices), std::mem::take(&mut self.candidates), self.chosen);
        app().answer_passkey(incoming, choices, chosen, candidates, choice);
    }
}

impl View for PasskeyAsk {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        if self.incoming.as_ref().is_some_and(|i| i.gone()) {
            self.answer(None);
            cx.close_window();
        }
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.answer(None);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut col = AlertFrame::new("key").child(paragraph(self.message.clone()));
        if !self.choices.is_empty() {
            let mut radios = div().mt(px(m::SPACING)).flex().flex_col().gap(px(m::TIGHT));
            for (i, (_, label)) in self.choices.iter().enumerate() {
                radios = radios.child(RadioButton::new(label.clone(), i == self.chosen).id(format!("passkey-{i}")).on_select(cx.listener(move |p: &mut PasskeyAsk, _: &(), cx| {
                    p.chosen = i;
                    cx.invalidate();
                })));
            }
            col = col.child(radios);
        }
        let done = if self.choices.is_empty() { "Save Passkey" } else { "Sign In" };
        let mut row = button_row();
        for (i, label) in ["Use Another Device…", "Cancel", done].into_iter().enumerate() {
            row = row.child(Button::new(label).default(i == 2).on_click(cx.listener(move |p: &mut PasskeyAsk, _: &ClickEvent, cx| {
                p.answer(Some(i));
                cx.close_window();
            })));
        }
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|p: &mut PasskeyAsk, e: &KeyDownEvent, cx| {
                let choice = match e.keystroke.key.as_str() {
                    "escape" => 1,
                    "enter" => 2,
                    _ => return,
                };
                cx.stop_propagation();
                p.answer(Some(choice));
                cx.close_window();
            }))
            .child(col.child(div().flex_1()).child(row))
            .into_any_element()
    }
}

/// The code a TOTP field's value gives now.
pub fn otp_code(value: &str) -> Option<String> {
    Totp::parse(value).ok().map(|t| t.now().0)
}

pub fn open_url(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(url)) {
            tracing::warn!("open: {e}");
        }
    });
}

pub fn category_menu(menu: Menu, prefix: &str) -> Menu {
    let mut menu = menu;
    for (i, c) in view::NEW_CATEGORIES.iter().enumerate() {
        if i == 3 {
            menu = menu.separator();
        }
        menu = menu.item(c.label(), format!("{prefix}{}", c.as_str()));
        if *c == ItemCategory::Login && prefix == "new:" {
            menu = menu.key("n");
        }
    }
    menu
}

/// How olo-session starts Passwords at login: the SSH agent (and the
/// keychain), no window until it's opened.
const AGENT_FLAG: &str = "--agent";
/// How a program that keeps a key here starts Passwords when it isn't
/// running (Coder, reading its model service's key): the keychain, no
/// window until it's opened, and it stays for the keychain.
const KEYCHAIN_FLAG: &str = "--keychain";

/// Started with no window: for the SSH agent or the keychain.
fn in_background(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some(AGENT_FLAG | KEYCHAIN_FLAG))
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let agent_only = args.first().map(String::as_str) == Some(AGENT_FLAG);
    let keychain_only = args.first().map(String::as_str) == Some(KEYCHAIN_FLAG);
    olo_kit::run(APP_ID, move |cx| {
        let proxy = olo_kit::proxy();
        instance::listen(APP_ID, proxy.clone());
        // Logins for Chrome's pages, the palette's search and op.
        let keychain = proxy.clone();
        match olo_keychain::serve(move |incoming| keychain.send(incoming)) {
            // Started for it: it stays for it, with no window.
            Ok(()) if keychain_only => olo_kit::stay_resident(),
            Ok(()) => {}
            Err(e) => tracing::warn!("keychain: {e}"),
        }
        let mut settings: Settings = std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // From when there was one account.
        if let Some(id) = settings.account.take()
            && settings.accounts.is_empty()
        {
            let label = settings.account_label.take().unwrap_or_else(|| id.clone());
            settings.accounts.push(Linked { id, label });
        }
        // Preferences ▸ Security, carrying over what was chosen here
        // before it moved there.
        let mut security = olo_kit::settings::current().map(|s| s.security.passwords).unwrap_or_default();
        // The SSH agent (on by default): its socket served while Passwords
        // runs, and Passwords kept running without a window for it.
        if agent_only && !security.ssh_agent {
            std::process::exit(olo_xdg::SERVICE_NOT_WANTED);
        }
        if security.ssh_agent {
            let to = proxy.clone();
            match agent::serve(&olo_xdg::ssh_agent_socket(), move |incoming| to.send(incoming)) {
                Ok(()) => olo_kit::stay_resident(),
                Err(e) => tracing::warn!("ssh agent: {e}"),
            }
        }
        let (old_lock, old_clear) = (settings.lock_minutes.take(), settings.clear_seconds.take());
        if (old_lock.is_some() || old_clear.is_some()) && security == PasswordsSecurity::default() {
            security.lock_minutes = old_lock.unwrap_or(security.lock_minutes);
            security.clear_seconds = old_clear.unwrap_or(security.clear_seconds);
            let carried = security.clone();
            olo_kit::settings::update(move |s| s.security.passwords = carried);
        }
        {
            let proxy_lock = proxy.clone();
            let proxy = proxy.clone();
            olo_kit::settings::watch(move |s| proxy.send(s.security.passwords));
            let was = AtomicBool::new(false);
            olo_kit::settings::watch_power(move |status| {
                // Only as it locks, not every time something else changes.
                if status.locked && !was.swap(true, Ordering::Relaxed) {
                    proxy_lock.send(ScreenLocked);
                } else if !status.locked {
                    was.store(false, Ordering::Relaxed);
                }
            });
        }
        let saved: Option<Saved> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
        // On an agent screen: what's shared with it, no account of its own.
        if on_screen() {
            settings.accounts.clear();
            settings.local_only = true;
        }
        // Locked from the start once there are items to protect (never
        // what's shared with a screen: the person chose to give it).
        let locked = !on_screen() && (!settings.accounts.is_empty() || settings.local_only);
        let notifier = Notifier::new(proxy.clone(), "Passwords", "olo-passwords");
        let mut app = Passwords {
            history: Default::default(),
            chosen: settings.accounts.iter().map(|l| l.id.clone()).collect(),
            accounts: Accounts::Unknown,
            other_account: false,
            store: Store::new(proxy.clone()),
            lock: Lock::new(proxy.clone(), locked),
            conn: if locked { Conn::Connecting } else { Conn::Setup },
            settings,
            security,
            vaults: Vec::new(),
            vault_accounts: HashMap::new(),
            items: Vec::new(),
            loading: false,
            details: HashMap::new(),
            index: HashMap::new(),
            facets: Default::default(),
            listed: Default::default(),
            rev: 0,
            missing: HashMap::new(),
            source: Source::All,
            selected: None,
            reveal_selection: true,
            search: String::new(),
            revealed: HashSet::new(),
            editor: None,
            focus_title: false,
            focus_field: None,
            main: None,
            generator: None,
            allow_shown: HashMap::new(),
            status: Notice::default(),
            last_use: Instant::now(),
            unlocked: false,
            waiting: Vec::new(),
            cli_waiting: Vec::new(),
            grants: cli::Grants::default(),
            asking: HashSet::new(),
            notifier,
            sync: None,
            sync_asked: None,
            sync_due: None,
            review: None,
            problems: None,
            show_review: false,
            ui: Ui::default(),
            secret_until: None,
            leaving: false,
            proxy: proxy.clone(),
            _shared: olo_keychain::shared::this_screen().and_then(|screen| {
                let proxy = proxy.clone();
                let copies = [olo_keychain::shared::copy_path(Some(&screen)), olo_keychain::shared::copy_path(None)];
                olo_kit::watch::watch(copies, move |_| proxy.send(SharedChanged))
            }),
        };
        let mut unlocked_before = false;
        // Started for the SSH agent or the keychain, no window: unless
        // it's coming back from an upgrade with its window open.
        let show = saved.as_ref().map_or(!(agent_only || keychain_only), |s| s.window);
        if let Some(saved) = saved {
            app.source = saved.source;
            app.selected = saved.selected;
            app.search = saved.search;
            // Still within its lock-after time: as unlocked as it was.
            let within = app.security.lock_minutes == 0 || saved.idle_secs < app.security.lock_minutes as u64 * 60;
            if saved.unlocked && within && app.lock.approved() {
                app.last_use = Instant::now() - Duration::from_secs(saved.idle_secs);
                unlocked_before = true;
                tracing::info!("unlocked as before the upgrade");
            }
        }
        if APP.set(Mutex::new(app)).is_err() {
            return;
        }
        {
            let mut app = crate::app();
            if show {
                app.open_main();
            }
            if unlocked_before {
                app.now_unlocked();
            }
            // Locked, the store opens at the unlock.
            if !locked {
                app.connect();
            }
            app.changed();
        }
        // What the proxy brings is the app's, on its looper; and its one
        // timer, for what's next due (none while nothing is: idle costs
        // nothing).
        let mut timer: Option<olo_kit::app_kit::TimerId> = None;
        let mut ticked: Option<(Instant, Instant)> = None;
        olo_kit::message_handler(cx, move |message, cx| {
            let message = match message.downcast::<Tell>() {
                Ok(tell) => {
                    match *tell {
                        Tell::Tick => {
                            timer = None;
                            crate::app().tick();
                        }
                        Tell::Schedule => {
                            SCHEDULING.store(false, Ordering::Release);
                            if let Some(t) = timer.take() {
                                cx.looper().cancel(t);
                            }
                            let due = crate::app().next_due();
                            if let Some(at) = due {
                                let now = Instant::now();
                                let mut delay = at.saturating_duration_since(now);
                                // (Due already, and just looked at: the
                                // tick left it, so not round and round.)
                                if delay.is_zero() && ticked.is_some_and(|(for_, when)| for_ == at && now.saturating_duration_since(when) < Duration::from_secs(1)) {
                                    delay = Duration::from_secs(1);
                                }
                                if delay.is_zero() {
                                    ticked = Some((at, now));
                                }
                                let proxy = olo_kit::proxy();
                                timer = Some(cx.looper().after(delay, move |_| proxy.send(Tell::Tick)));
                            }
                        }
                    }
                    return;
                }
                Err(m) => m,
            };
            crate::app().message(message);
        });
        olo_kit::on_upgrade(|_| {
            let a = crate::app();
            a.save_settings();
            let window = a.main.as_ref().is_some_and(|w| w.is_open());
            serde_json::to_string(&Saved {
                source: a.source.clone(),
                selected: a.selected.clone(),
                search: a.search.clone(),
                unlocked: !a.lock.locked,
                idle_secs: a.last_use.elapsed().as_secs(),
                window,
            })
            .ok()
        });
    });
}
