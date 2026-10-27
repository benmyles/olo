//! Passwords: the 1Password items of the account in the 1Password app,
//! natively, through `tabula-onepassword`. Locked with Touch ID.

mod edit;
mod generator;
mod lock;
mod passkey;
mod store;
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
    store::{FromStore, Job, Reply, Store},
};

const APP_ID: &str = "tabula.passwords";

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The account's UUID (or its name in 1Password's sidebar).
    account: Option<String>,
    /// How to name it to the user.
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
        Settings { account: None, account_label: None, lock_minutes: 10, clear_seconds: 90, generator: Default::default(), size: None }
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

/// Ticks for the auto-lock and one-time-password countdowns.
struct Tick;

pub struct Passwords {
    settings: Settings,
    store: Store,
    conn: Conn,
    account_field: String,
    accounts: Accounts,
    /// Naming an account by hand rather than choosing a listed one.
    other_account: bool,
    vaults: Vec<VaultOverview>,
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
}

fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).or_else(|_| url::Url::parse(&format!("https://{url}"))).ok()?;
    Some(parsed.host_str()?.trim_start_matches("www.").to_string())
}

impl Passwords {
    fn vault_title(&self, id: &str) -> &str {
        self.vaults.iter().find(|v| v.id == id).map_or("", |v| v.title.as_str())
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

    fn connect(&mut self, cx: &mut Cx) {
        let Some(account) = self.settings.account.clone() else {
            self.conn = Conn::Setup;
            return;
        };
        self.conn = Conn::Connecting;
        self.loading = true;
        self.store.connect(&account);
        self.redraw(cx);
    }

    /// Back to choosing the account (listing the app's afresh).
    pub fn change_account(&mut self) {
        self.account_field = self.settings.account.clone().unwrap_or_default();
        self.accounts = Accounts::Unknown;
        self.conn = Conn::Setup;
    }

    /// Read the 1Password app's accounts off the UI thread.
    pub fn find_accounts(&mut self, cx: &mut Cx) {
        self.accounts = Accounts::Finding;
        let proxy = cx.proxy();
        std::thread::spawn(move || proxy.send(Accounts::Found(onepassword::accounts::local())));
    }

    /// The account as the user knows it.
    pub fn account_label(&self) -> String {
        self.settings.account_label.clone().or_else(|| self.settings.account.clone()).unwrap_or_default()
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
        if self.settings.account.is_none() {
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
            }
            Reply::Failed(message) => {
                self.conn = Conn::Failed(message);
                self.loading = false;
                self.serve_waiting(cx);
            }
            Reply::Vaults(vaults) => {
                let ids: HashSet<&str> = vaults.iter().map(|v| v.id.as_str()).collect();
                self.items.retain(|i| ids.contains(i.vault_id.as_str()));
                self.vaults = vaults;
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
            }
            Reply::Archived { item } => {
                let row = self.visible().iter().position(|i| i.id == item);
                if let Some(i) = self.items.iter_mut().find(|i| i.id == item) {
                    i.state = ItemState::Archived;
                    let title = i.title.clone();
                    self.flash(format!("Moved “{title}” to the Archive."));
                }
                self.after_removal(&item, row);
            }
            Reply::Deleted { item } => {
                let row = self.visible().iter().position(|i| i.id == item);
                if let Some(i) = self.items.iter().find(|i| i.id == item) {
                    self.flash(format!("Deleted “{}”.", i.title));
                }
                self.items.retain(|i| i.id != item);
                self.details.remove(&item);
                self.after_removal(&item, row);
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

    fn tick(&mut self, cx: &mut Cx) {
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
        if let Some(alert) = self.alerts.get(&window) {
            let (icon, message, buttons): (&str, String, &[&str]) = match alert {
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
            .item("Change Account…", "account", None)
            .enabled(!self.lock.locked)
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
                    // Choose the one in use, else the first; by hand if none fits.
                    let field = self.account_field.trim();
                    if let Some(a) = list.iter().find(|a| a.uuid == field || a.name == field) {
                        self.account_field = a.uuid.clone();
                        self.other_account = false;
                    } else if field.is_empty() {
                        self.account_field = list.first().map(|a| a.uuid.clone()).unwrap_or_default();
                        self.other_account = list.is_empty();
                    } else {
                        self.other_account = true;
                    }
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
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
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
        // Locked from the start once there's an account to protect.
        let locked = settings.account.is_some();
        let mut app = Passwords {
            account_field: settings.account.clone().unwrap_or_default(),
            accounts: Accounts::Unknown,
            other_account: false,
            store: Store::new(proxy.clone()),
            lock: Lock::new(proxy.clone(), locked),
            settings,
            conn: Conn::Setup,
            vaults: Vec::new(),
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
        };
        if let Some(saved) = saved {
            app.source = saved.source;
            app.selected = saved.selected;
            app.search = saved.search;
        }
        app.open_main(cx);
        app.connect(cx);
        app
    });
}
