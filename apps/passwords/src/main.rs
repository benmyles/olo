//! Passwords: logins and other secrets kept on this computer, encrypted
//! (`vault.rs`), and synced both ways with a 1Password account if asked
//! (`sync.rs`, through `olo-onepassword`). Locked with Touch ID.
//!
//! On olo-ui: the app's state is a [`Passwords`] entity, which is the
//! main window's view too; its panes are cached parts of it
//! (`olo_ui::part`), so a one-time code counting down redraws the item
//! pane alone. The generator, Review Sync and Sync Problems windows and
//! the alerts are views of their own that read it.

mod cli;
mod edit;
mod generator;
mod lock;
mod passkey;
mod store;
mod sync;
mod vault;
mod view;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_keychain::{Incoming, Request, Response};
use olo_protocol::PasswordsSecurity;
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, WindowOptions,
    gpui::{
        AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, MouseDownEvent, OutgoingDrag, ParentElement as _, Render, ScrollHandle,
        SharedString, StyleRefinement, Styled as _, Subscription, Task, Window, WindowHandle, div, point, px,
    },
    instance::{self, Launch},
    menu, metrics as m,
    notify::{Note, Notifier},
    part::{self, Part},
    search::{self, Facet, Needle, Query, SearchEvent, SearchField, search_field},
    widgets::{AlertFrame, Button, ListModel, RadioButton, TextField, TextFieldEvent, alert, button_row, open_alert, paragraph, text_field},
};
use onepassword::{Item, ItemCategory, ItemFieldType, ItemOverview, ItemState, VaultOverview, totp::Totp};
use serde::{Deserialize, Serialize};

use crate::{
    edit::{EditForm, Editor},
    generator::{Generator, GeneratorView},
    lock::{Lock, LockMsg},
    store::{FromStore, Job, Reply, Searchable, Store, SyncState},
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
    size: Option<(u32, u32)>,
}

fn settings_path() -> std::path::PathBuf {
    olo_ui::config_dir().join("passwords.json")
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
/// for `ask_approval`.
struct Approval {
    about: Option<cli::Session>,
    on: bool,
}

/// Asked of app_server, off the UI thread.
fn approving_without_asking() -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let status = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&olo_protocol::Request::PowerStatus).ok());
    matches!(status, Some(olo_protocol::Reply::Power { status }) if status.auto_approve_until.is_some_and(|t| t > now))
}

/// The question for letting `op` use the passwords.
fn allow_message(a: &cli::Asker) -> String {
    let mut m = format!("Allow “{}” to use your passwords?", a.command);
    match (&a.parent, &a.cwd) {
        (Some(p), Some(d)) => m.push_str(&format!(" It was run by {p} in {d}.")),
        (Some(p), None) => m.push_str(&format!(" It was run by {p}.")),
        _ => {}
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
    pub window: Option<WindowHandle<Frame>>,
}

/// The Sync Problems window and what it lists.
pub struct Problems {
    pub window: Option<WindowHandle<Frame>>,
    pub list: Vec<sync::Problem>,
}

/// The screen locked.
struct ScreenLocked;

/// How often to sync while unlocked, and how soon after a change here.
const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const SYNC_AFTER_CHANGE: Duration = Duration::from_secs(3);
/// How long something said in the status line stays.
const STATUS_FOR: Duration = Duration::from_secs(6);

/// The main window's cached parts.
pub struct Parts {
    menus: Entity<Part<Passwords>>,
    pub sources: Entity<Part<Passwords>>,
    pub items: Entity<Part<Passwords>>,
    pub detail: Entity<Part<Passwords>>,
}

pub struct Passwords {
    settings: Settings,
    /// Preferences ▸ Security's choices for Passwords (followed live).
    security: PasswordsSecurity,
    store: Store,
    conn: Conn,
    /// The setup's choices: listed accounts ticked, and one named by hand.
    pub chosen: Vec<String>,
    pub account_field: Entity<TextField>,
    accounts: Accounts,
    /// Naming an account by hand rather than choosing a listed one.
    pub other_account: bool,
    vaults: Vec<VaultOverview>,
    /// Which account each synced vault is in.
    pub vault_accounts: HashMap<String, String>,
    items: Vec<ItemOverview>,
    loading: bool,
    details: HashMap<String, Item>,
    /// What search and the filters need of every item (store.rs).
    index: HashMap<String, Searchable>,
    /// The filters, made again when the items change.
    facets: RefCell<Option<Rc<Vec<Facet>>>>,
    /// The list as last worked out: for which items (`rev`), source and
    /// search.
    listed: RefCell<Option<(u64, Source, String, Rc<Vec<ItemOverview>>)>>,
    /// Counts changes to the items (and what's known of them).
    rev: std::cell::Cell<u64>,
    missing: HashMap<String, String>,
    source: Source,
    selected: Option<String>,
    reveal_selection: bool,
    search: String,
    pub search_field: Entity<SearchField>,
    revealed: HashSet<String>,
    pub editor: Option<Editor>,
    /// The editor's fields (made again when its fields come or go).
    pub form: Option<EditForm>,
    /// The editor just opened: its title takes the keyboard.
    pub focus_title: bool,
    lock: Lock,
    pub lock_field: Entity<TextField>,
    main: Option<WindowHandle<Frame>>,
    parts: Option<Parts>,
    generator: Option<(WindowHandle<Frame>, Entity<GeneratorView>)>,
    /// The terminal sessions being asked about (an alert showing).
    allow_shown: HashSet<cli::Session>,
    status: Option<(String, Instant)>,
    last_use: Instant,
    /// Just unlocked: the password field gives up the keyboard.
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
    pub items_list: Entity<ListModel>,
    /// The field under the pointer (its Copy and Show showing).
    pub hover_field: Option<String>,
    /// The field pressed, and where (a click copies it, a drag takes its
    /// value out).
    pub field_press: Option<(String, olo_ui::gpui::Point<olo_ui::gpui::Pixels>)>,
    pub detail_scroll: ScrollHandle,
    pub editor_scroll: ScrollHandle,
    pub sources_scroll: ScrollHandle,
    /// The next time something is due (an auto-lock, a sync, the status
    /// line clearing), and the timer for it.
    wake: Option<(Instant, Task<()>)>,
    /// One-time codes counting down in the item pane: its tick.
    countdown: Option<Task<()>>,
    proxy: Proxy,
    _subs: Vec<Subscription>,
}

/// `s` as a filter value: lower case, words joined by `-`
/// ("secure-note").
fn slug(s: &str) -> String {
    s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join("-")
}

fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).or_else(|_| url::Url::parse(&format!("https://{url}"))).ok()?;
    Some(parsed.host_str()?.trim_start_matches("www.").to_string())
}

impl Passwords {
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
    fn invalidate(&self) {
        self.rev.set(self.rev.get() + 1);
        *self.facets.borrow_mut() = None;
    }

    /// The list for the current source and search: best matches first
    /// while searching, else by title. (Worked out once for each.)
    pub fn visible(&self) -> Rc<Vec<ItemOverview>> {
        if let Some((rev, source, search, list)) = &*self.listed.borrow()
            && *rev == self.rev.get()
            && *source == self.source
            && *search == self.search
        {
            return list.clone();
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
        let list: Rc<Vec<ItemOverview>> = Rc::new(list.into_iter().map(|(_, i)| i.clone()).collect());
        *self.listed.borrow_mut() = Some((self.rev.get(), self.source.clone(), self.search.clone(), list.clone()));
        list
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
    pub fn facets_rc(&self) -> Rc<Vec<Facet>> {
        if let Some(f) = &*self.facets.borrow() {
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
        let facets = Rc::new(vec![tag, has, ty, vault, Facet::new("site", "Website").typed_only(), Facet::new("user", "Username").typed_only()]);
        *self.facets.borrow_mut() = Some(facets.clone());
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

    pub fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) && self.selected != id {
            self.ask_discard(Then::Select(id), cx);
            return;
        }
        if self.selected != id {
            self.close_editor();
            self.revealed.clear();
            self.selected = id;
            self.detail_scroll.set_offset(point(px(0.0), px(0.0)));
            self.fetch_selected();
        }
        cx.notify();
    }

    fn fetch_selected(&mut self) {
        if let Some(id) = &self.selected
            && let Some(o) = self.overview(id)
            && !self.details.contains_key(id)
        {
            self.store.send(Job::Get { vault: o.vault_id.clone(), item: id.clone() });
        }
    }

    pub fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Source(source), cx);
            return;
        }
        self.close_editor();
        self.source = source;
        self.keep_selection_listed();
        cx.notify();
    }

    /// The search (or a filter) changed; `from_bar`: not typed, so the
    /// field is told.
    pub fn set_search(&mut self, text: String, from_bar: bool, cx: &mut Context<Self>) {
        if from_bar {
            self.search_field.update(cx, |f, cx| f.set_text(text.clone(), cx));
        }
        if text == self.search {
            return;
        }
        self.search = text;
        self.keep_selection_listed();
        cx.notify();
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
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if !self.set_up() {
            self.conn = Conn::Setup;
            cx.notify();
            return;
        }
        self.conn = Conn::Connecting;
        self.loading = true;
        self.store.connect();
        cx.notify();
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
    pub fn sync_and_review(&mut self, cx: &mut Context<Self>) {
        if self.review.as_ref().is_some_and(|r| r.window.is_some()) {
            return self.open_review(cx);
        }
        self.show_review = true;
        self.sync_now();
        cx.notify();
    }

    /// Show the waiting changes.
    pub fn open_review(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.review else { return };
        if let Some(w) = review.window
            && w.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return;
        }
        let me = cx.entity();
        cx.defer(move |cx| {
            let view = me.clone();
            let handle = olo_ui::open_window(cx, WindowOptions::new("Review Sync", 560.0, 440.0).min_size(420.0, 300.0).floating(), move |window, cx| {
                let closing = view.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    // Closed from its tab: as Cancel.
                    closing.update(cx, |a, cx| {
                        if let Some(r) = &mut a.review {
                            r.window = None;
                        }
                        a.store.send(Job::Decline);
                        a.end_review(cx);
                    });
                    true
                });
                cx.new(|cx| view::ReviewView::new(view.clone(), cx))
            });
            me.update(cx, |a, _| {
                if let Some(r) = &mut a.review {
                    r.window = handle;
                }
            });
        });
    }

    /// The review is over (applied, declined, or overtaken).
    pub fn end_review(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.review.take().and_then(|r| r.window) {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    /// Ask, then erase everything kept here (nothing in 1Password).
    fn confirm_reset(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity();
        self.alert("Reset Passwords", "warning", RESET.to_owned(), &["Cancel", "Erase and Start Again"], cx, move |choice, cx| {
            if choice == Some(1) {
                me.update(cx, |a, _| a.store.send(Job::Reset));
            }
        });
    }

    /// Opens an alert (once this update is over).
    fn alert(&self, title: &'static str, icon: &'static str, message: String, buttons: &'static [&'static str], cx: &mut Context<Self>, on_choose: impl Fn(Option<usize>, &mut App) + 'static) {
        cx.defer(move |cx| {
            open_alert(cx, title, icon, &message, buttons, on_choose);
        });
    }

    /// The store was erased: back to the start.
    fn after_reset(&mut self, cx: &mut Context<Self>) {
        self.end_review(cx);
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
        self.account_field.update(cx, |f, cx| f.set_text("", cx));
        self.chosen.clear();
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
        self.invalidate();
        self.flash("Passwords was reset");
    }

    /// Keep items only here from now on (no syncing).
    pub fn keep_local(&mut self, cx: &mut Context<Self>) {
        self.settings.accounts.clear();
        self.settings.local_only = true;
        self.sync = None;
        self.save_settings();
        self.connect(cx);
    }

    /// Back to choosing the accounts (listing the app's afresh).
    pub fn change_account(&mut self, cx: &mut Context<Self>) {
        self.chosen = self.settings.accounts.iter().map(|l| l.id.clone()).collect();
        self.account_field.update(cx, |f, cx| f.set_text("", cx));
        self.other_account = false;
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
        cx.notify();
    }

    /// The setup's Sync: the accounts ticked and any named by hand.
    pub fn sync_chosen(&mut self, cx: &mut Context<Self>) {
        let listed = view::listed_accounts(self);
        let mut chosen: Vec<Linked> = listed.iter().filter(|a| self.chosen.iter().any(|c| view::same(a, c))).map(|a| Linked { id: view::linked_id(self, a), label: a.name.clone() }).collect();
        let typed = self.account_field.read(cx).text().trim().to_string();
        if (listed.is_empty() || self.other_account) && !typed.is_empty() && !chosen.iter().any(|c| c.id == typed) {
            chosen.push(Linked { id: typed.clone(), label: typed });
        }
        if chosen.is_empty() && !self.set_up() {
            return;
        }
        self.choose_accounts(chosen, cx);
    }

    /// The setup's choice: sync with `chosen` (the accounts kept keep
    /// their order, so the first stays first). Dropping any asks first.
    pub fn choose_accounts(&mut self, chosen: Vec<Linked>, cx: &mut Context<Self>) {
        let mut keep: Vec<Linked> = self.settings.accounts.iter().filter(|l| chosen.iter().any(|c| c.id == l.id)).cloned().collect();
        keep.extend(chosen.into_iter().filter(|c| !self.settings.accounts.iter().any(|l| l.id == c.id)));
        let removed: Vec<Linked> = self.settings.accounts.iter().filter(|l| !keep.iter().any(|k| k.id == l.id)).cloned().collect();
        if removed.is_empty() {
            return self.link_accounts(Vec::new(), keep, cx);
        }
        let me = cx.entity();
        let message = remove_message(&removed);
        let what = RefCell::new(Some((removed, keep)));
        self.alert("Remove Account", "warning", message, &["Cancel", "Remove"], cx, move |choice, cx| {
            if choice == Some(1)
                && let Some((removed, keep)) = what.borrow_mut().take()
            {
                me.update(cx, |a, cx| a.link_accounts(removed, keep, cx));
            }
        });
    }

    fn link_accounts(&mut self, removed: Vec<Linked>, keep: Vec<Linked>, cx: &mut Context<Self>) {
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
            cx.notify();
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
        self.status = Some((text.into(), Instant::now()));
    }

    fn save_settings(&self) {
        olo_ui::store::save(settings_path(), &self.settings);
    }

    /// Another app asks (Chrome's extension for a page's logins, the
    /// palette, op).
    fn keychain(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
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
        let waits = !matches!(incoming.request, Request::Logins { .. } | Request::Check { .. } | Request::Search { .. } | Request::Passkeys { .. });
        if self.lock.locked {
            if !waits {
                return incoming.reply(Response::Locked);
            }
            // Unlocked for it while approving without asking (Deskbar's
            // key); else come forward to be unlocked. The answer follows.
            self.waiting.push(incoming);
            self.ask_approval(None);
            cx.notify();
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
            Request::Cli { query } => {
                let Some(session) = cli::session_of(incoming.pid) else {
                    return incoming.reply(Response::Error { message: "op went away".into() });
                };
                if !self.grants.check(session) {
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
                    self.set_source(Source::All, cx);
                    self.set_search(String::new(), true, cx);
                    self.select(Some(id), cx);
                    self.reveal_selection = true;
                }
                self.open_main(cx);
                cx.notify();
                return incoming.reply(Response::Shown);
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
                return self.ask_passkey(incoming, message, choices, Vec::new(), cx);
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
                return self.ask_passkey(incoming, message, Vec::new(), candidates, cx);
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
    fn ask_passkey(&mut self, incoming: Incoming, message: String, choices: Vec<((String, String), String)>, candidates: Vec<(String, String)>, cx: &mut Context<Self>) {
        let me = cx.entity();
        cx.defer(move |cx| PasskeyAsk::open(me, incoming, message, choices, candidates, cx));
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

    /// The person allowed (or denied) a terminal session's `op`.
    fn answer_cli(&mut self, session: cli::Session, allow: bool, cx: &mut Context<Self>) {
        self.allow_shown.remove(&session);
        if allow {
            self.grants.allow(session);
        }
        let (theirs, rest) = std::mem::take(&mut self.cli_waiting).into_iter().partition::<Vec<_>, _>(|(s, _)| *s == session);
        self.cli_waiting = rest;
        for (_, incoming) in theirs {
            if allow {
                self.keychain(incoming, cx);
            } else {
                incoming.reply(Response::Error { message: "Passwords: not allowed".into() });
            }
        }
    }

    /// Ask app_server (on a thread) whether it approves requests without
    /// asking now, for the unlock (None) or `op` in `session`; `approval`
    /// goes on from the answer.
    fn ask_approval(&mut self, about: Option<cli::Session>) {
        let shown = about.is_some_and(|s| self.allow_shown.contains(&s));
        if shown || !self.asking.insert(about) {
            return;
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Approval { about, on: approving_without_asking() }));
    }

    /// Unlocked or allowed without asking, said in a quiet notification;
    /// else asked as usual.
    fn approval(&mut self, Approval { about, on }: Approval, cx: &mut Context<Self>) {
        self.asking.remove(&about);
        let quietly = |n: &Notifier, title: &str, body: String| {
            let mut note = Note::new(title).body(body);
            note.silent = true;
            n.show(None, note);
        };
        match about {
            None if !self.lock.locked => self.serve_waiting(cx),
            None if self.waiting.is_empty() => {}
            None if on => {
                let mut from: Vec<&str> = self.waiting.iter().map(|i| i.from.as_str()).collect();
                from.dedup();
                let from = from.join(", ");
                tracing::info!(%from, "unlocked without asking");
                quietly(&self.notifier, "Passwords unlocked without asking", format!("For {from}."));
                if self.lock.approved() {
                    self.now_unlocked(cx);
                }
            }
            None => self.open_main(cx),
            Some(session) => {
                let Some(pid) = self.cli_waiting.iter().find(|(s, _)| *s == session).map(|(_, i)| i.pid) else { return };
                let asker = cli::asker(pid);
                if on {
                    tracing::info!(command = asker.command, "op allowed without asking");
                    quietly(&self.notifier, "op allowed without asking", asker.command.clone());
                    self.answer_cli(session, true, cx);
                } else {
                    self.allow_shown.insert(session);
                    let me = cx.entity();
                    self.alert("Passwords", "warning", allow_message(&asker), &["Deny", "Allow"], cx, move |choice, cx| {
                        me.update(cx, |a, cx| a.answer_cli(session, choice == Some(1), cx));
                    });
                }
            }
        }
    }

    /// Just unlocked (Touch ID, the password, or approved): the store
    /// opens and what waited is answered.
    fn now_unlocked(&mut self, cx: &mut Context<Self>) {
        self.last_use = Instant::now();
        self.unlocked = true;
        self.lock_field.update(cx, |f, cx| f.clear_secret(cx));
        // The store opens (again) now its secrets are wanted.
        self.connect(cx);
        if self.selected.is_none() {
            self.selected = self.visible().first().map(|i| i.id.clone());
        }
        self.fetch_selected();
        self.serve_waiting(cx);
    }

    /// Answer what waited for an unlock or for the items.
    fn serve_waiting(&mut self, cx: &mut Context<Self>) {
        for incoming in std::mem::take(&mut self.waiting) {
            self.keychain(incoming, cx);
        }
    }

    /// The password typed in the lock panel, checked.
    pub fn submit_password(&mut self, cx: &mut Context<Self>) {
        self.lock.submit();
        // (The lock took the password; the field lets go of it too.)
        self.lock_field.update(cx, |f, cx| f.clear_secret(cx));
        cx.notify();
    }

    fn do_lock(&mut self, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Lock, cx);
            return;
        }
        self.close_editor();
        // Secrets go; overviews stay (hidden) so unlocking is instant.
        self.details.clear();
        self.revealed.clear();
        self.invalidate();
        if let Some((w, _)) = self.generator.take() {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            });
        }
        // A waiting plan holds secrets too; it is worked out again later.
        self.end_review(cx);
        // The store lets go of its secrets (after a running sync), and
        // `op` must be allowed again.
        self.store.send(Job::Close);
        self.grants.clear();
        self.lock.lock();
        self.lock_field.update(cx, |f, cx| f.clear_secret(cx));
        cx.notify();
    }

    /// A secret on the clipboard, taken off again after the time chosen
    /// in Preferences ▸ Security.
    pub fn copy_secret(&mut self, what: &str, text: &str, cx: &mut Context<Self>) {
        let seconds = self.security.clear_seconds;
        olo_ui::copy::secret(cx, text, Duration::from_secs(seconds as u64));
        self.flash(format!("Copied {what}. It leaves the clipboard in {seconds} seconds."));
        cx.notify();
    }

    /// Drag a field's value out, onto another app's field: the value
    /// (a one-time password's current code), drawn as the field's name,
    /// never the secret. `field` is `url:…` for a website.
    pub fn drag_field(&mut self, item: &str, field: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (value, label) = match field.strip_prefix("url:") {
            Some(url) => (url.to_owned(), "website".to_owned()),
            None => {
                let Some(f) = self.details.get(item).and_then(|i| i.field(field)).cloned() else { return };
                let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
                let value = match f.field_type {
                    ItemFieldType::Totp => match otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone())) {
                        Some(code) => code,
                        None => return,
                    },
                    _ => f.value.clone(),
                };
                (value, label)
            }
        };
        let icon = olo_ui::dnd::picture(window, cx, "passwords", 32.0, Some(&label), point(px(16.0), px(16.0)));
        let bytes: Arc<[u8]> = Arc::from(value.into_bytes());
        let drag = OutgoingDrag {
            types: vec![("text/plain;charset=utf-8".into(), bytes.clone()), ("text/plain".into(), bytes.clone()), ("UTF8_STRING".into(), bytes)],
            copy: true,
            move_: false,
            ask: false,
            icon,
        };
        olo_ui::dnd::start(window, cx, drag, |_, _| {});
    }

    pub fn copy_field(&mut self, item: &str, field: &str, cx: &mut Context<Self>) {
        if let Some(url) = field.strip_prefix("url:") {
            cx.write_to_clipboard(olo_ui::gpui::ClipboardItem::new_string(url.to_owned()));
            self.flash("Copied the website.");
            cx.notify();
            return;
        }
        let Some(f) = self.details.get(item).and_then(|i| i.field(field)).cloned() else { return };
        let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
        match f.field_type {
            ItemFieldType::Totp => {
                if let Some(code) = otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone())) {
                    self.copy_secret("the one-time password", &code, cx);
                }
            }
            t if edit::concealed(t) || t == ItemFieldType::CreditCardNumber => self.copy_secret(&format!("the {label}"), &f.value, cx),
            _ => {
                cx.write_to_clipboard(olo_ui::gpui::ClipboardItem::new_string(f.value.clone()));
                self.flash(format!("Copied the {label}."));
                cx.notify();
            }
        }
    }

    /// A secret field shown, or hidden again.
    pub fn toggle_revealed(&mut self, field: String, cx: &mut Context<Self>) {
        if !self.revealed.remove(&field) {
            self.revealed.insert(field);
        }
        cx.notify();
    }

    pub fn revealed(&self, field: &str) -> bool {
        self.revealed.contains(field)
    }

    /// The selected item's username, password or one-time code.
    fn copy_selected(&mut self, kind: &str, cx: &mut Context<Self>) {
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)).cloned() else { return };
        let field = match kind {
            "username" => {
                item.fields.iter().find(|f| f.id == "username").or_else(|| item.fields.iter().find(|f| matches!(f.field_type, ItemFieldType::Email) || f.title.eq_ignore_ascii_case("username")))
            }
            "password" => item.fields.iter().find(|f| f.id == "password").or_else(|| item.fields.iter().find(|f| f.field_type == ItemFieldType::Concealed)),
            _ => item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp),
        };
        match field {
            Some(f) => self.copy_field(&item.id, &f.id.clone(), cx),
            None => self.flash(format!("“{}” has no {}.", item.title, if kind == "otp" { "one-time password" } else { kind })),
        }
        cx.notify();
    }

    pub fn open_website(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.selected.as_ref().and_then(|s| self.overview(s)).and_then(|o| o.websites.first()).map(|w| w.url.clone()) else { return };
        open_url(&url);
        self.flash(format!("Opening {}", host(&url).unwrap_or(url)));
        cx.notify();
    }

    /// The editor closed (saved, cancelled or overtaken).
    fn close_editor(&mut self) {
        self.editor = None;
        self.form = None;
    }

    /// The editor opened on `editor`: its fields made, the title given
    /// the keyboard.
    fn open_editor(&mut self, editor: Editor, cx: &mut Context<Self>) {
        self.form = Some(EditForm::new(&editor, cx));
        // A new item's title takes the keyboard (an item edited keeps it
        // where it was).
        self.focus_title = editor.base.is_none();
        self.editor = Some(editor);
        self.hover_field = None;
        self.editor_scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// The editor's fields came or went: its form made again.
    fn rebuild_form(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = &self.editor {
            self.form = Some(EditForm::new(e, cx));
        }
        cx.notify();
    }

    fn new_item(&mut self, category: ItemCategory, cx: &mut Context<Self>) {
        if self.lock.locked || self.conn != Conn::Ready || self.vaults.is_empty() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::New(category), cx);
            return;
        }
        // The vault being looked at, else the first (Personal).
        let vault = match &self.source {
            Source::Vault(v) => self.vaults.iter().position(|x| &x.id == v).unwrap_or(0),
            _ => 0,
        };
        self.open_editor(Editor::new_item(category, vault, None), cx);
    }

    pub fn edit_selected(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected.as_ref().and_then(|s| self.details.get(s)) else { return };
        if self.overview(&item.id).is_some_and(|o| o.state == ItemState::Archived) {
            return;
        }
        let vault = self.vaults.iter().position(|v| v.id == item.vault_id).unwrap_or(0);
        let editor = Editor::edit(item, vault);
        self.open_editor(editor, cx);
    }

    pub fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else { return };
        if editor.base.is_some() && !editor.changed() {
            self.close_editor();
            cx.notify();
            return;
        }
        // What 1Password wouldn't take is said here, not found at sync,
        // under the form (scrolled to: it's below the fold on a long item).
        if !editor.ready() {
            self.editor_scroll.scroll_to_bottom();
            cx.notify();
            return;
        }
        if editor.base.is_some() {
            if let Some(item) = editor.to_item() {
                self.store.send(Job::Put(Box::new(item)));
                self.flash("Saving…");
            }
        } else if let Some(params) = editor.to_params(&self.vaults) {
            self.store.send(Job::Create(Box::new(params)));
            self.flash("Saving…");
        }
        cx.notify();
    }

    pub fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.close_editor();
        cx.notify();
    }

    /// The generator, opened for an editor's field (its New).
    pub fn generate_for(&mut self, field: String, cx: &mut Context<Self>) {
        let item = self.editor.as_ref().and_then(|e| e.item_id()).unwrap_or("").to_string();
        self.open_generator(Some((item, field)), cx);
    }

    pub fn toggle_edit_reveal(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(f) = self.editor.as_mut().and_then(|e| e.fields.get_mut(i)) else { return };
        f.reveal = !f.reveal;
        let on = f.reveal;
        if let Some(form) = &self.form {
            form.reveal(i, on, cx);
        }
        cx.notify();
    }

    pub fn remove_edit_field(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.editor.as_mut()
            && i < e.fields.len()
        {
            e.fields.remove(i);
            self.rebuild_form(cx);
        }
    }

    pub fn add_edit_field(&mut self, kind: ItemFieldType, cx: &mut Context<Self>) {
        if let Some(e) = self.editor.as_mut() {
            e.add_field(kind);
            self.rebuild_form(cx);
        }
    }

    pub fn remove_edit_website(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.editor.as_mut()
            && i < e.websites.len()
        {
            e.websites.remove(i);
            self.rebuild_form(cx);
        }
    }

    pub fn add_edit_website(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = self.editor.as_mut() {
            e.websites.push((String::new(), "website".into(), onepassword::AutofillBehavior::AnywhereOnWebsite));
            self.rebuild_form(cx);
        }
    }

    /// The generator's Use: its password into the field it was opened for.
    pub fn use_generated(&mut self, password: String, cx: &mut Context<Self>) {
        let Some(target) = self.generator.as_ref().and_then(|(_, g)| g.read(cx).generator.target.clone()) else { return };
        let (item, field) = target;
        let Some(editor) = &mut self.editor else { return };
        if editor.item_id().unwrap_or("") != item {
            return;
        }
        let Some(i) = editor.fields.iter().position(|f| f.id == field) else { return };
        editor.fields[i].value = password.clone();
        editor.fields[i].reveal = true;
        if let Some(form) = &self.form {
            form.set_value(i, &password, true, cx);
        }
        cx.notify();
    }

    /// The generator's window went.
    pub fn generator_closed(&mut self) {
        self.generator = None;
        self.save_settings();
    }

    fn ask_discard(&mut self, then: Then, cx: &mut Context<Self>) {
        let me = cx.entity();
        let then = RefCell::new(Some(then));
        self.alert("Passwords", "warning", "Discard the changes to this item?".into(), &["Keep Editing", "Discard"], cx, move |choice, cx| {
            if choice == Some(1)
                && let Some(then) = then.borrow_mut().take()
            {
                me.update(cx, |a, cx| a.after_discard(then, cx));
            }
        });
    }

    fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.alert("Passwords", "stop", message, &["OK"], cx, |_, _| {});
    }

    /// The whole of a sync problem, from the status line.
    pub fn show_problem(&mut self, message: String, cx: &mut Context<Self>) {
        self.show_error(message, cx);
    }

    /// What a sync couldn't do, item by item, grouped by vault.
    pub fn show_problems(&mut self, list: Vec<sync::Problem>, cx: &mut Context<Self>) {
        let list = sync::grouped(list);
        if let Some(p) = &mut self.problems {
            p.list = list;
            if let Some(w) = p.window
                && w.update(cx, |_, window, _| window.activate_window()).is_ok()
            {
                cx.notify();
                return;
            }
        } else {
            self.problems = Some(Problems { window: None, list });
        }
        let me = cx.entity();
        cx.defer(move |cx| {
            let view = me.clone();
            let handle = olo_ui::open_window(cx, WindowOptions::new("Sync Problems", 520.0, 400.0).min_size(400.0, 260.0).dialog().child(), move |window, cx| {
                let closing = view.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    closing.update(cx, |a, _| a.problems = None);
                    true
                });
                cx.new(|cx| view::ProblemsView::new(view.clone(), cx))
            });
            me.update(cx, |a, _| {
                if let Some(p) = &mut a.problems {
                    p.window = handle;
                }
            });
        });
    }

    /// The Sync Problems window closed from its buttons.
    pub fn close_problems(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.problems.take().and_then(|p| p.window) {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    /// The review's Apply or Cancel.
    pub fn answer_review(&mut self, apply: bool, cx: &mut Context<Self>) {
        let Some(plan) = self.review.as_ref().map(|r| r.plan) else { return };
        self.store.send(if apply { Job::Approve { plan } } else { Job::Decline });
        self.end_review(cx);
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)).cloned() else { return };
        let message = format!("Delete “{}” for good? This can’t be undone.", o.title);
        let me = cx.entity();
        self.alert("Delete Item", "warning", message, &["Cancel", "Delete"], cx, move |choice, cx| {
            if choice == Some(1) {
                me.update(cx, |a, _| a.store.send(Job::Delete { vault: o.vault_id.clone(), item: o.id.clone() }));
            }
        });
    }

    fn after_discard(&mut self, then: Then, cx: &mut Context<Self>) {
        self.close_editor();
        match then {
            Then::Select(id) => self.select(id, cx),
            Then::Source(s) => self.set_source(s, cx),
            Then::New(c) => self.new_item(c, cx),
            Then::Close => self.quit(cx),
            Then::Lock => self.do_lock(cx),
        }
        cx.notify();
    }

    /// Passwords ends (its main window closed, or Quit): the generator's
    /// recipe and the window's size kept.
    fn quit(&mut self, cx: &mut Context<Self>) {
        self.save_settings();
        self.main = None;
        cx.defer(|cx| cx.quit());
    }

    pub fn open_generator(&mut self, target: Option<(String, String)>, cx: &mut Context<Self>) {
        if let Some((w, g)) = &self.generator {
            g.update(cx, |g, cx| {
                g.generator.target = target;
                cx.notify();
            });
            let _ = w.update(cx, |_, window, _| window.activate_window());
            return;
        }
        let me = cx.entity();
        let generator = Generator::new(self.settings.generator.clone(), target);
        // (A panel beside the app's window: it floats, never a tab.)
        cx.defer(move |cx| {
            let view = cx.new(|cx| GeneratorView::new(generator, me.clone(), cx));
            let shown = view.clone();
            let closing = me.clone();
            let handle = olo_ui::open_window(cx, WindowOptions::new("Password Generator", 380.0, 330.0).min_size(340.0, 300.0).dialog(), move |window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    closing.update(cx, |a, _| a.generator_closed());
                    true
                });
                shown.read(cx).focus(window);
                shown
            });
            me.update(cx, |a, _| a.generator = handle.map(|h| (h, view)));
        });
    }

    fn open_main(&mut self, cx: &mut Context<Self>) {
        // (Gone if it was closed while we hold the clipboard.)
        if let Some(w) = self.main
            && w.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return;
        }
        self.main = None;
        let me = cx.entity();
        let (w, h) = self.settings.size.unwrap_or((920, 580));
        cx.defer(move |cx| {
            if me.read(cx).parts.is_none() {
                let parts = Parts::new(&me, cx);
                me.update(cx, |a, _| a.parts = Some(parts));
            }
            let view = me.clone();
            let handle = olo_ui::open_window(cx, WindowOptions::new("Passwords", w as f32, h as f32).min_size(680.0, 420.0), move |window, cx| {
                let closing = view.clone();
                window.on_window_should_close(cx, move |window, cx| closing.update(cx, |a, cx| a.close_requested(window, cx)));
                view.update(cx, |a, cx| {
                    let sub = cx.observe_window_activation(window, |a, window, cx| {
                        a.lock.focus(window.is_window_active());
                        cx.notify();
                    });
                    a._subs.push(sub);
                    a.lock.focus(window.is_window_active());
                });
                view
            });
            me.update(cx, |a, _| a.main = handle);
        });
    }

    /// The main window's close: asks about an edit first; else Passwords
    /// ends (and so do its other windows).
    fn close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let size = window.bounds().size;
        self.settings.size = Some((f32::from(size.width).round() as u32, f32::from(size.height).round() as u32));
        if self.editor.as_ref().is_some_and(|e| e.changed()) {
            self.ask_discard(Then::Close, cx);
            return false;
        }
        self.quit(cx);
        true
    }

    fn apply_reply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        // (The list and the filters' values and counts follow the items.)
        self.invalidate();
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
                    self.set_search(String::new(), true, cx);
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
            Reply::Deleted { item } => {
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
                if self.status.as_ref().is_some_and(|(s, _)| s == "Saving…") {
                    self.status = None;
                }
                self.show_error(message, cx);
            }
        }
        self.invalidate();
        let facets = self.facets_rc();
        self.search_field.update(cx, |f, cx| f.set_facets(facets, cx));
        cx.notify();
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
        if self.security.auto_sync && !self.lock.locked && self.conn == Conn::Ready && !self.settings.accounts.is_empty() {
            due.extend(self.sync_due);
            due.extend(self.sync_asked.map(|t| t + SYNC_EVERY));
        }
        if !self.lock.locked && self.security.lock_minutes > 0 {
            due.push(self.last_use + Duration::from_secs(self.security.lock_minutes as u64 * 60));
        }
        due.extend(self.status.as_ref().map(|(_, at)| *at + STATUS_FOR));
        due.into_iter().min()
    }

    /// A timer for the next thing due (none while nothing is: idle costs
    /// nothing).
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let due = self.next_due();
        if self.wake.as_ref().map(|(at, _)| *at) == due {
            return;
        }
        self.wake = due.map(|at| {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
                let _ = this.update(cx, |a, cx| {
                    a.wake = None;
                    a.tick(cx);
                });
            });
            (at, task)
        });
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        // (Syncing only when asked: File ▸ Sync Now.)
        let due = self.security.auto_sync && (self.sync_due.is_some_and(|t| now >= t) || self.sync_asked.is_some_and(|t| now >= t + SYNC_EVERY));
        // Not while the person is looking at what the last one found.
        if due && self.review.as_ref().is_none_or(|r| r.window.is_none()) {
            self.sync_now();
        }
        let idle = now.saturating_duration_since(self.last_use);
        if !self.lock.locked && self.security.lock_minutes > 0 && idle >= Duration::from_secs(self.security.lock_minutes as u64 * 60) && self.editor.as_ref().is_none_or(|e| !e.changed()) {
            self.do_lock(cx);
        }
        if self.status.as_ref().is_some_and(|(_, at)| now >= *at + STATUS_FOR) {
            self.status = None;
        }
        cx.notify();
    }

    /// One-time codes count down in the item pane, which alone is drawn
    /// again each second while one shows.
    fn follow_countdown(&mut self, cx: &mut Context<Self>) {
        let showing = self.main.is_some()
            && !self.lock.locked
            && self.editor.is_none()
            && self.selected.as_ref().and_then(|s| self.details.get(s)).is_some_and(|d| d.fields.iter().any(|f| f.field_type == ItemFieldType::Totp));
        match (showing, &self.countdown, &self.parts) {
            (true, None, Some(parts)) => {
                let detail = parts.detail.downgrade();
                self.countdown = Some(cx.spawn(async move |_, cx| {
                    loop {
                        // On the second, as the codes' periods turn.
                        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                        let to_next = Duration::from_millis(1000 - (since.as_millis() % 1000) as u64);
                        cx.background_executor().timer(to_next).await;
                        if detail.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }));
            }
            (false, Some(_), _) => self.countdown = None,
            _ => {}
        }
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

        let file = Menu::new("File")
            .submenu(category_menu(Menu::new("New Item"), "new:"))
            .enabled(ready)
            .separator()
            .item("Password Generator", "generator")
            .key("g")
            .separator()
            .item("Lock", "lock")
            .key("l")
            .enabled(!self.lock.locked)
            .item("Sync Now", "sync")
            .enabled(ready && !self.settings.accounts.is_empty() && self.sync != Some(SyncState::Running))
            .item("1Password Accounts…", "account")
            .enabled(!self.lock.locked)
            .item("Reset Passwords…", "reset")
            .enabled(!self.lock.locked && self.set_up())
            .separator()
            .item("Close", "close")
            .key("w")
            .item("Quit", "quit")
            .key("q");
        let edit = Menu::new("Edit")
            .item("Copy Username", "copy:username")
            .key("c")
            .enabled(has && details.is_some())
            .item("Copy Password", "copy:password")
            .shift_key("c")
            .enabled(has && details.is_some())
            .item("Copy One-Time Password", "copy:otp")
            .shift_key("o")
            .enabled(has && has_otp)
            .separator()
            .item("Find", "find")
            .key("f")
            .enabled(ready);
        let item = Menu::new("Item")
            .item("Edit", "edit")
            .key("e")
            .enabled(has && details.is_some() && !archived)
            .item("Open Website", "open")
            .key("o")
            .enabled(has && has_site)
            .separator()
            .item("Move to Archive", "archive")
            .enabled(has && !archived)
            .item("Delete…", "delete")
            .enabled(has);
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
        view = view.separator().submenu(lock_after);
        vec![file, edit, item, view]
    }

    fn menu_bar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity();
        MenuBar::new(self.menus(), move |cmd, window, cx| me.update(cx, |a, cx| a.command(cmd, window, cx))).into_any_element()
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

    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.last_use = Instant::now();
        match command {
            "quit" | "close" => {
                if self.editor.as_ref().is_some_and(|e| e.changed()) {
                    self.ask_discard(Then::Close, cx);
                } else {
                    let size = window.bounds().size;
                    if Some(window.window_handle()) == self.main.map(|m| m.into()) {
                        self.settings.size = Some((f32::from(size.width).round() as u32, f32::from(size.height).round() as u32));
                    }
                    self.quit(cx);
                }
            }
            "lock" => self.do_lock(cx),
            "account" => self.change_account(cx),
            "sync" => self.sync_and_review(cx),
            "reset" => self.confirm_reset(cx),
            "generator" => self.open_generator(None, cx),
            "find" => self.search_field.clone().update(cx, |f, cx| f.focus(window, cx)),
            "edit" => self.edit_selected(cx),
            "open" => self.open_website(cx),
            "archive" => {
                if let Some(o) = self.selected.as_ref().and_then(|s| self.overview(s)) {
                    self.store.send(Job::Archive { vault: o.vault_id.clone(), item: o.id.clone() });
                }
            }
            "delete" => self.confirm_delete(cx),
            c if c.starts_with("copy:") => self.copy_selected(&c[5..], cx),
            c if c.starts_with("new:") => {
                if let Some(cat) = ItemCategory::ALL.iter().find(|x| x.as_str() == &c[4..]) {
                    self.new_item(*cat, cx);
                }
            }
            "source:all" => self.set_source(Source::All, cx),
            "source:archive" => self.set_source(Source::Archive, cx),
            c if c.starts_with("source:vault:") => self.set_source(Source::Vault(c[13..].to_string()), cx),
            c if c.starts_with("vault:") => {
                if let Some(v) = c[6..].parse::<usize>().ok().and_then(|i| self.vaults.get(i)) {
                    let id = v.id.clone();
                    self.set_source(Source::Vault(id), cx);
                }
            }
            c if c.starts_with("lockafter:") => {
                // The same choice as Preferences ▸ Security's.
                let minutes = c[10..].parse().unwrap_or(10);
                self.security.lock_minutes = minutes;
                olo_ui::settings::update(move |s| s.security.passwords.lock_minutes = minutes);
            }
            _ => {}
        }
        cx.notify();
    }

    fn message(&mut self, msg: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let msg = match msg.downcast::<FromStore>() {
            Ok(m) => {
                if m.generation == self.store.generation {
                    self.apply_reply(m.reply, cx);
                }
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<LockMsg>() {
            Ok(m) => {
                if self.lock.message(*m) {
                    self.now_unlocked(cx);
                }
                cx.notify();
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
                cx.notify();
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
                cx.notify();
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<ScreenLocked>() {
            Ok(_) => {
                // Not with changes still unsaved: they'd be lost.
                if self.security.lock_with_screen && !self.lock.locked && self.editor.as_ref().is_none_or(|e| !e.changed()) {
                    self.do_lock(cx);
                }
                return;
            }
            Err(m) => m,
        };
        let msg = match msg.downcast::<Incoming>() {
            Ok(incoming) => return self.keychain(*incoming, cx),
            Err(m) => m,
        };
        let msg = match msg.downcast::<Approval>() {
            Ok(a) => {
                self.approval(*a, cx);
                cx.notify();
                return;
            }
            Err(m) => m,
        };
        if msg.downcast::<Launch>().is_ok() {
            self.open_main(cx);
        }
    }

    /// Up and Down in the search field move the list's selection.
    fn arrows(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let up = match e.keystroke.key.as_str() {
            "up" => true,
            "down" => false,
            _ => return,
        };
        let field = self.search_field.read(cx);
        if e.keystroke.modifiers.modified() || self.editor.is_some() || !field.is_focused(window, cx) || field.suggesting(window, cx) {
            return;
        }
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
            self.select(Some(id), cx);
            self.reveal_selection = true;
        }
        cx.stop_propagation();
    }
}

impl Parts {
    fn new(app: &Entity<Passwords>, cx: &mut App) -> Parts {
        let list = app.read(cx).items_list.clone();
        Parts {
            menus: cx.new(|cx| Part::new(app, Passwords::menu_bar, cx)),
            sources: cx.new(|cx| Part::new(app, view::sources, cx)),
            items: cx.new(|cx| Part::new(app, view::items, cx).observe(&list, cx)),
            detail: cx.new(|cx| Part::new(app, view::detail, cx)),
        }
    }
}

impl Render for Passwords {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(parts) = &self.parts else { return div().into_any_element() };
        let mut bar = StyleRefinement::default();
        bar.size.height = Some(px(menu::BAR_HEIGHT).into());
        let menus = part::cached(&parts.menus, bar);
        let body = view::main(self, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            // Anything the person does puts off the auto-lock.
            .capture_any_mouse_down(cx.listener(|a, _: &MouseDownEvent, _, _| a.last_use = Instant::now()))
            .capture_key_down(cx.listener(|a, e: &KeyDownEvent, window, cx| {
                a.last_use = Instant::now();
                a.arrows(e, window, cx);
            }))
            .child(menus)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// The question of a page's passkey: sign in with one (which, if more
/// than one), save a new one, or use another device; its answer goes to
/// the page. The page giving up (gone, or its time out) closes it.
struct PasskeyAsk {
    app: Entity<Passwords>,
    incoming: Option<Box<Incoming>>,
    message: SharedString,
    choices: Vec<((String, String), String)>,
    chosen: usize,
    candidates: Vec<(String, String)>,
    focus: FocusHandle,
    _watch: Task<()>,
}

impl PasskeyAsk {
    fn open(app: Entity<Passwords>, incoming: Incoming, message: String, choices: Vec<((String, String), String)>, candidates: Vec<(String, String)>, cx: &mut App) {
        let buttons = ["Use Another Device…", "Cancel", if choices.is_empty() { "Save Passkey" } else { "Sign In" }];
        let width = alert::alert_width(cx, &buttons);
        let radios = if choices.is_empty() { 0.0 } else { m::SPACING + choices.len() as f32 * (m::CONTROL_HEIGHT + m::TIGHT) };
        let height = alert::alert_height(cx, &message, width) + radios;
        let view = cx.new(|cx| PasskeyAsk { app, incoming: Some(Box::new(incoming)), message: message.into(), choices, chosen: 0, candidates, focus: cx.focus_handle(), _watch: Task::ready(()) });
        let shown = view.clone();
        let handle = olo_ui::open_window(cx, WindowOptions::alert("Passwords", width, height), move |window, cx| {
            window.activate_window();
            let closing = shown.clone();
            window.on_window_should_close(cx, move |_, cx| {
                closing.update(cx, |p, cx| p.answer(None, cx));
                true
            });
            window.focus(&shown.read(cx).focus);
            shown
        });
        let Some(handle) = handle else {
            return view.update(cx, |p, cx| p.answer(None, cx));
        };
        // The page may give up (gone, or its time out): looked for twice
        // a second while asking, and the question goes with it.
        view.update(cx, |p, cx| {
            p._watch = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    match this.update(cx, |p, cx| {
                        let gone = p.incoming.as_ref().is_some_and(|i| i.gone());
                        if gone {
                            p.answer(None, cx);
                        }
                        gone || p.incoming.is_none()
                    }) {
                        Ok(false) => {}
                        Ok(true) => {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                            return;
                        }
                        Err(_) => return,
                    }
                }
            });
        });
    }

    /// Answered with button `choice` (None: closed, or the page gave
    /// up: declined).
    fn answer(&mut self, choice: Option<usize>, cx: &mut Context<Self>) {
        let Some(incoming) = self.incoming.take() else { return };
        let (choices, candidates, chosen) = (std::mem::take(&mut self.choices), std::mem::take(&mut self.candidates), self.chosen);
        self.app.update(cx, |a, _| a.answer_passkey(incoming, choices, chosen, candidates, choice));
    }
}

impl Render for PasskeyAsk {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let mut col = AlertFrame::new("key").child(paragraph(self.message.clone()));
        if !self.choices.is_empty() {
            let mut radios = div().mt(px(m::SPACING)).flex().flex_col().gap(px(m::TIGHT));
            for (i, (_, label)) in self.choices.iter().enumerate() {
                let to = me.clone();
                radios = radios.child(RadioButton::new(label.clone(), i == self.chosen).id(format!("passkey-{i}")).on_select(move |_, cx| {
                    to.update(cx, |p, cx| {
                        p.chosen = i;
                        cx.notify();
                    })
                }));
            }
            col = col.child(radios);
        }
        let done = if self.choices.is_empty() { "Save Passkey" } else { "Sign In" };
        let mut row = button_row();
        for (i, label) in ["Use Another Device…", "Cancel", done].into_iter().enumerate() {
            let to = me.clone();
            row = row.child(Button::new(label).default(i == 2).on_click(move |window, cx| {
                to.update(cx, |p, cx| p.answer(Some(i), cx));
                window.remove_window();
            }));
        }
        let keys = me.clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                let choice = match e.keystroke.key.as_str() {
                    "escape" => 1,
                    "enter" => 2,
                    _ => return,
                };
                cx.stop_propagation();
                keys.update(cx, |p, cx| p.answer(Some(choice), cx));
                window.remove_window();
            })
            .child(col.child(div().flex_1()).child(row))
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

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, |cx| {
        let proxy = olo_ui::proxy(cx);
        instance::listen(APP_ID, proxy.clone());
        // Logins for Chrome's pages, the palette's search and op.
        let keychain = proxy.clone();
        if let Err(e) = olo_keychain::serve(move |incoming| keychain.send(incoming)) {
            tracing::warn!("keychain: {e}");
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
        let mut security = olo_ui::settings::current().map(|s| s.security.passwords).unwrap_or_default();
        let (old_lock, old_clear) = (settings.lock_minutes.take(), settings.clear_seconds.take());
        if (old_lock.is_some() || old_clear.is_some()) && security == PasswordsSecurity::default() {
            security.lock_minutes = old_lock.unwrap_or(security.lock_minutes);
            security.clear_seconds = old_clear.unwrap_or(security.clear_seconds);
            let carried = security.clone();
            olo_ui::settings::update(move |s| s.security.passwords = carried);
        }
        {
            let proxy_lock = proxy.clone();
            let proxy = proxy.clone();
            olo_ui::settings::watch(move |s| proxy.send(s.security.passwords));
            let was = std::sync::atomic::AtomicBool::new(false);
            olo_ui::settings::watch_power(move |status| {
                // Only as it locks, not every time something else changes.
                if status.locked && !was.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    proxy_lock.send(ScreenLocked);
                } else if !status.locked {
                    was.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
        let saved: Option<Saved> = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok());
        // Locked from the start once there are items to protect.
        let locked = !settings.accounts.is_empty() || settings.local_only;
        let account_field = text_field(cx, |f| f.name("Account").hint("as in 1Password's sidebar, or its UUID"));
        let lock_field = text_field(cx, |f| f.name("Password").password(true));
        let search = search_field(cx, "Search");
        let items_list = ListModel::new(cx, false);
        let notifier = Notifier::new(proxy.clone(), "Passwords", "olo-passwords");
        let app = cx.new(|cx| {
            let mut subs = Vec::new();
            subs.push(cx.subscribe(&lock_field, |a: &mut Passwords, f, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Changed => {
                    a.lock.password = f.read(cx).text().to_owned();
                    cx.notify();
                }
                TextFieldEvent::Entered => a.submit_password(cx),
                TextFieldEvent::Typed(_) => {}
            }));
            subs.push(cx.subscribe(&account_field, |a: &mut Passwords, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => a.sync_chosen(cx),
                _ => cx.notify(),
            }));
            subs.push(cx.subscribe(&search, |a: &mut Passwords, f, e: &SearchEvent, cx| {
                if let SearchEvent::Changed = e {
                    let text = f.read(cx).text(cx).to_owned();
                    a.set_search(text, false, cx);
                }
            }));
            // Whatever changed, the next thing due is looked at again.
            subs.push(cx.observe_self(|a, cx| {
                a.schedule(cx);
                a.follow_countdown(cx);
            }));
            let mut app = Passwords {
                chosen: settings.accounts.iter().map(|l| l.id.clone()).collect(),
                account_field: account_field.clone(),
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
                rev: Default::default(),
                missing: HashMap::new(),
                source: Source::All,
                selected: None,
                reveal_selection: true,
                search: String::new(),
                search_field: search.clone(),
                revealed: HashSet::new(),
                editor: None,
                form: None,
                focus_title: false,
                lock_field: lock_field.clone(),
                main: None,
                parts: None,
                generator: None,
                allow_shown: HashSet::new(),
                status: None,
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
                items_list,
                hover_field: None,
                field_press: None,
                detail_scroll: ScrollHandle::new(),
                editor_scroll: ScrollHandle::new(),
                sources_scroll: ScrollHandle::new(),
                wake: None,
                countdown: None,
                proxy: proxy.clone(),
                _subs: subs,
            };
            let mut unlocked_before = false;
            if let Some(saved) = saved {
                app.source = saved.source;
                app.selected = saved.selected;
                app.search = saved.search.clone();
                search.update(cx, |f, cx| f.set_text(saved.search, cx));
                // Still within its lock-after time: as unlocked as it was.
                let within = app.security.lock_minutes == 0 || saved.idle_secs < app.security.lock_minutes as u64 * 60;
                if saved.unlocked && within && app.lock.approved() {
                    app.last_use = Instant::now() - Duration::from_secs(saved.idle_secs);
                    unlocked_before = true;
                    tracing::info!("unlocked as before the upgrade");
                }
            }
            app.open_main(cx);
            if unlocked_before {
                app.now_unlocked(cx);
            }
            // Locked, the store opens at the unlock.
            if !locked {
                app.connect(cx);
            }
            app
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |a, cx| a.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            let a = saving.read(cx);
            a.save_settings();
            serde_json::to_string(&Saved { source: a.source.clone(), selected: a.selected.clone(), search: a.search.clone(), unlocked: !a.lock.locked, idle_secs: a.last_use.elapsed().as_secs() }).ok()
        });
    });
}
