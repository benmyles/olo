//! Other programs' tray icons (StatusNotifierItem, as KDE and Ayatana
//! define it): Slack's, Discord's, Telegram's, nm-applet's. Deskbar is the
//! session's StatusNotifierWatcher, which items register with, and its
//! host: a worker reads each item's picture, title, tooltip and status,
//! and its menu (`com.canonical.dbusmenu`) ahead of time, so the menu
//! opens within a frame, and follows their changes. Each read runs on a
//! thread of its own, so an app that doesn't answer holds up nothing.
//! Deskbar draws the items in its tray; what is done to one (a click,
//! a menu choice) comes back here as a [`Do`].
//!
//! The same worker hears programs' launcher badges
//! (`com.canonical.Unity.LauncherEntry`'s Update, which Chromium, Electron
//! and Telegram send): an unread count, a progress, a wish for attention,
//! kept per desktop entry until the program leaves the bus.

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
};

use olo_ui::gpui::RenderImage;
use zbus::{
    blocking::Connection,
    message::Header,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const LAUNCHER: &str = "com.canonical.Unity.LauncherEntry";

/// A program's launcher badge.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Badge {
    /// The desktop entry it is for (`slack`, from `application://slack.desktop`).
    pub app: String,
    /// Its unread count, if it shows one.
    pub count: Option<i64>,
    /// How far along what it is doing is (0–1), if it shows that.
    pub progress: Option<f64>,
    pub urgent: bool,
    /// Who sent it (dropped when they leave the bus).
    sender: String,
}

/// What Deskbar shows of other programs.
#[derive(Debug, Clone, Default)]
pub struct Tray {
    pub items: Vec<Item>,
    pub badges: Vec<Badge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Nothing to say: not shown.
    Passive,
    Active,
    NeedsAttention,
}

/// A picture an item sent, RGBA with its alpha premultiplied.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// An entry of an item's menu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    /// A checkmark or radio item, and whether it is on.
    pub check: Option<bool>,
    pub icon: Option<String>,
    pub children: Vec<Entry>,
}

/// A tray icon, as last read.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Who it is: its bus name and object path, together.
    pub key: String,
    pub title: String,
    /// The program it belongs to (its executable's name, "slack"), to name
    /// it by when it has no title (Electron's have none).
    pub program: String,
    pub tooltip: String,
    pub status: Status,
    /// A theme icon's name, or the file it was found as in the item's own
    /// theme folder.
    pub icon: String,
    /// Its pictures, every size it sent (used when there's no icon name,
    /// or it can't be found).
    pub pixmaps: Vec<Pixmap>,
    /// Clicking it opens its menu (it has no action of its own).
    pub item_is_menu: bool,
    /// Its menu's entries, if it has one.
    pub menu: Option<Vec<Entry>>,
    /// Changes when its picture does (textures are kept by it).
    pub serial: u64,
}

/// What was done to an item.
#[derive(Debug, Clone, Copy)]
pub enum Do {
    Activate,
    SecondaryActivate,
    /// The right button, for an item with no menu of ours to show.
    ContextMenu,
    /// Scrolled over it by this much (positive: down).
    Scroll(i32),
    /// Its menu is open (it may update it).
    MenuShown,
    /// Its menu's entry was chosen.
    Chosen(i32),
}

enum Event {
    Registered {
        bus: String,
        path: String,
    },
    /// A bus name (an app) went away.
    Gone(String),
    /// An item (or its menu) changed: the signal's sender and path.
    Changed {
        sender: String,
        path: String,
        menu: bool,
    },
    /// An item's properties, read.
    Read {
        key: String,
        owner: String,
        item: Box<Item>,
        menu_path: Option<String>,
    },
    /// An item's menu, read.
    Menu {
        key: String,
        entries: Vec<Entry>,
    },
    Do(String, Do),
    /// A launcher badge's update: who sent it, its app's URI, what changed.
    Badge {
        sender: String,
        app: String,
        props: HashMap<String, OwnedValue>,
    },
}

/// Deskbar's side: tell the worker what was done to an item.
pub struct Host {
    tx: mpsc::Sender<Event>,
}

impl Host {
    pub fn send(&self, key: &str, what: Do) {
        let _ = self.tx.send(Event::Do(key.to_owned(), what));
    }
}

/// `org.kde.StatusNotifierWatcher`: where items register.
struct Watcher {
    tx: mpsc::Sender<Event>,
    items: Arc<olo_sync::Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// `service` is the item's bus name (its object at
    /// `/StatusNotifierItem`), or its object's path on the caller's
    /// connection (Ayatana's).
    async fn register_status_notifier_item(&self, service: String, #[zbus(header)] header: Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let (bus, path) = if service.starts_with('/') { (sender, service) } else { (service, ITEM_PATH.to_owned()) };
        let key = format!("{bus}{path}");
        let new = {
            let mut items = self.items.lock().unwrap();
            let new = !items.contains(&key);
            if new {
                items.push(key.clone());
            }
            new
        };
        if new {
            let _ = Self::status_notifier_item_registered(&emitter, &key).await;
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
        }
        let _ = self.tx.send(Event::Registered { bus, path });
    }

    async fn register_status_notifier_host(&self, _service: String, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let _ = Self::status_notifier_host_registered(&emitter).await;
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Be the session's watcher and host (on `address`, a test's bus, or the
/// session's), telling `changed` the items each time they change. The bus
/// is reached on the worker, so a slow one holds up nothing.
pub fn start(address: Option<String>, changed: impl Fn(Tray) + Send + 'static) -> Host {
    let (tx, rx) = mpsc::channel();
    let host = Host { tx: tx.clone() };
    let _ = std::thread::Builder::new().name("tray-host".into()).spawn(move || {
        let items = Arc::new(olo_sync::Mutex::new(Vec::new()));
        let builder = match &address {
            Some(a) => zbus::blocking::connection::Builder::address(a.as_str()),
            None => zbus::blocking::connection::Builder::session(),
        };
        let conn = match builder.and_then(|b| b.serve_at(WATCHER_PATH, Watcher { tx: tx.clone(), items: items.clone() })).and_then(|b| b.build()) {
            Ok(c) => c,
            Err(e) => return tracing::warn!("tray: no D-Bus for other programs' tray icons: {e}"),
        };
        let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
        match conn.request_name_with_flags(WATCHER, flags) {
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner) => {}
            // Another desktop's watcher: be one of its hosts.
            _ => {
                tracing::info!("tray: another StatusNotifierWatcher is running; hosting its items");
                follow_other_watcher(&conn, &tx);
            }
        }
        let name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        let _ = conn.request_name(name.as_str());
        listen(&conn, &tx);
        run(conn, rx, tx, items, changed);
    });
    host
}

/// Another program is the watcher: take its items, and the ones that
/// register with it from now on.
fn follow_other_watcher(conn: &Connection, tx: &mpsc::Sender<Event>) {
    let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let _ = conn.call_method(Some(WATCHER), WATCHER_PATH, Some(WATCHER), "RegisterStatusNotifierHost", &(host,));
    let registered: Vec<String> = conn
        .call_method(Some(WATCHER), WATCHER_PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(WATCHER, "RegisteredStatusNotifierItems"))
        .ok()
        .and_then(|m| m.body().deserialize::<OwnedValue>().ok())
        .and_then(|v| <Vec<String>>::try_from(v).ok())
        .unwrap_or_default();
    for key in registered {
        let (bus, path) = split_key(&key);
        let _ = tx.send(Event::Registered { bus, path });
    }
    let Ok(rule) = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(WATCHER).and_then(|r| r.member("StatusNotifierItemRegistered")).map(|r| r.build()) else { return };
    let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(64)) else { return };
    let tx = tx.clone();
    let _ = std::thread::Builder::new().name("tray-watcher".into()).spawn(move || {
        for m in signals.flatten() {
            if let Ok(key) = m.body().deserialize::<String>() {
                let (bus, path) = split_key(&key);
                let _ = tx.send(Event::Registered { bus, path });
            }
        }
    });
}

/// A watcher's item key ("bus/path", or a bus name alone) as its bus name
/// and path.
fn split_key(key: &str) -> (String, String) {
    match key.find('/') {
        Some(i) => (key[..i].to_owned(), key[i..].to_owned()),
        None => (key.to_owned(), ITEM_PATH.to_owned()),
    }
}

/// Follow items' and menus' change signals, and apps leaving the bus.
fn listen(conn: &Connection, tx: &mpsc::Sender<Event>) {
    for (interface, menu) in [(ITEM, false), (MENU, true)] {
        let Ok(rule) = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(interface).map(|r| r.build()) else { continue };
        let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(256)) else { continue };
        let tx = tx.clone();
        let _ = std::thread::Builder::new().name("tray-signals".into()).spawn(move || {
            for m in signals.flatten() {
                let h = m.header();
                if let (Some(sender), Some(path)) = (h.sender(), h.path()) {
                    let _ = tx.send(Event::Changed { sender: sender.to_string(), path: path.to_string(), menu });
                }
            }
        });
    }
    if let Ok(rule) = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(LAUNCHER).and_then(|r| r.member("Update")).map(|r| r.build())
        && let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(256))
    {
        let tx = tx.clone();
        let _ = std::thread::Builder::new().name("tray-badges".into()).spawn(move || {
            for m in signals.flatten() {
                let sender = m.header().sender().map(|s| s.to_string()).unwrap_or_default();
                if let Ok((app, props)) = m.body().deserialize::<(String, HashMap<String, OwnedValue>)>() {
                    let _ = tx.send(Event::Badge { sender, app, props });
                }
            }
        });
    }
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return };
    let Ok(changes) = dbus.receive_name_owner_changed() else { return };
    let tx = tx.clone();
    let _ = std::thread::Builder::new().name("tray-names".into()).spawn(move || {
        for change in changes {
            if let Ok(args) = change.args()
                && args.new_owner().is_none()
            {
                let _ = tx.send(Event::Gone(args.name().to_string()));
            }
        }
    });
}

/// What the worker knows of an item.
struct Known {
    item: Item,
    bus: String,
    path: String,
    /// The bus name its signals come from (`bus` may be a well-known one).
    owner: String,
    menu_path: Option<String>,
}

fn run(conn: Connection, rx: mpsc::Receiver<Event>, tx: mpsc::Sender<Event>, registered: Arc<olo_sync::Mutex<Vec<String>>>, changed: impl Fn(Tray)) {
    let mut known: Vec<Known> = Vec::new();
    let mut badges: Vec<Badge> = Vec::new();
    let mut serial = 0u64;
    let publish = |known: &[Known], badges: &[Badge]| changed(Tray { items: known.iter().map(|k| k.item.clone()).collect(), badges: badges.to_vec() });
    for event in rx {
        match event {
            Event::Registered { bus, path } => read(&conn, &tx, bus, path),
            Event::Read { key, owner, mut item, menu_path } => {
                serial += 1;
                item.serial = serial;
                match known.iter_mut().find(|k| k.item.key == key) {
                    Some(k) => {
                        // Keep the menu read until the new one comes.
                        if k.menu_path == menu_path {
                            item.menu = k.item.menu.take();
                        }
                        if k.item.icon == item.icon && k.item.pixmaps == item.pixmaps {
                            item.serial = k.item.serial;
                        }
                        k.item = *item;
                        k.owner = owner;
                        k.menu_path = menu_path.clone();
                    }
                    None => {
                        let (bus, path) = split_key(&key);
                        known.push(Known { item: *item, bus, path, owner, menu_path: menu_path.clone() });
                    }
                }
                if let Some(menu) = menu_path
                    && let Some(k) = known.iter().find(|k| k.item.key == key)
                {
                    read_menu(&conn, &tx, key, k.bus.clone(), menu);
                }
                publish(&known, &badges);
            }
            Event::Menu { key, entries } => {
                if let Some(k) = known.iter_mut().find(|k| k.item.key == key) {
                    k.item.menu = Some(entries);
                    publish(&known, &badges);
                }
            }
            Event::Badge { sender, app, props } => {
                let app = app.trim_start_matches("application://").trim_end_matches(".desktop").to_owned();
                let i = match badges.iter().position(|b| b.app == app) {
                    Some(i) => i,
                    None => {
                        badges.push(Badge { app, ..Default::default() });
                        badges.len() - 1
                    }
                };
                update_badge(&mut badges[i], sender, &props);
                badges.retain(|b| b.count.is_some() || b.progress.is_some() || b.urgent);
                publish(&known, &badges);
            }
            Event::Changed { sender, path, menu } => {
                for k in known.iter().filter(|k| k.owner == sender || k.bus == sender) {
                    if menu && k.menu_path.as_deref() == Some(path.as_str()) {
                        read_menu(&conn, &tx, k.item.key.clone(), k.bus.clone(), path.clone());
                    } else if !menu && k.path == path {
                        read(&conn, &tx, k.bus.clone(), k.path.clone());
                    }
                }
            }
            Event::Gone(name) => {
                let before = known.len();
                let gone: Vec<String> = known.iter().filter(|k| k.bus == name || k.owner == name).map(|k| k.item.key.clone()).collect();
                known.retain(|k| k.bus != name && k.owner != name);
                registered.lock().unwrap().retain(|r| !gone.contains(r) && !r.starts_with(&format!("{name}/")) && *r != name);
                let badges_before = badges.len();
                badges.retain(|b| b.sender != name);
                if known.len() != before {
                    unregistered(&conn, &gone);
                }
                if known.len() != before || badges.len() != badges_before {
                    publish(&known, &badges);
                }
            }
            Event::Do(key, what) => {
                if let Some(k) = known.iter().find(|k| k.item.key == key) {
                    act(&conn, k, what);
                }
            }
        }
    }
}

/// Apply a launcher Update's properties to `b`: each only changes what it
/// names (`count` with `count-visible`, `progress` with
/// `progress-visible`, `urgent`).
fn update_badge(b: &mut Badge, sender: String, props: &HashMap<String, OwnedValue>) {
    b.sender = sender;
    let get = |key: &str| props.get(key).and_then(|v| v.try_clone().ok());
    let visible = |key: &str| get(key).and_then(|v| bool::try_from(v).ok());
    let count = get("count").and_then(|v| i64::try_from(v.try_clone().ok()?).ok().or_else(|| i32::try_from(v).ok().map(i64::from)));
    let progress = get("progress").and_then(|v| f64::try_from(v).ok());
    // A count or progress kept from before stays unless it is hidden.
    let shown_count = visible("count-visible").unwrap_or(b.count.is_some() || count.is_some());
    b.count = if shown_count { count.or(b.count).filter(|n| *n > 0) } else { None };
    let shown_progress = visible("progress-visible").unwrap_or(b.progress.is_some() || progress.is_some());
    b.progress = if shown_progress { progress.or(b.progress).map(|p| p.clamp(0.0, 1.0)) } else { None };
    if let Some(u) = visible("urgent") {
        b.urgent = u;
    }
}

/// Tell the bus `keys` are gone (the watcher's signal and property).
fn unregistered(conn: &Connection, keys: &[String]) {
    let Ok(watcher) = conn.object_server().interface::<_, Watcher>(WATCHER_PATH) else { return };
    let emitter = watcher.signal_emitter().clone();
    for key in keys {
        let _ = zbus::block_on(Watcher::status_notifier_item_unregistered(&emitter, key));
    }
    let _ = zbus::block_on(watcher.get().registered_status_notifier_items_changed(&emitter));
}

/// Do `what` to item `k`, on a thread of its own (the app may be slow to
/// answer).
fn act(conn: &Connection, k: &Known, what: Do) {
    let (conn, bus, path, menu) = (conn.clone(), k.bus.clone(), k.path.clone(), k.menu_path.clone());
    let _ = std::thread::Builder::new().name("tray-act".into()).spawn(move || {
        let r = match what {
            Do::Activate => conn.call_method(Some(bus.as_str()), path.as_str(), Some(ITEM), "Activate", &(0i32, 0i32)).map(drop),
            Do::SecondaryActivate => conn.call_method(Some(bus.as_str()), path.as_str(), Some(ITEM), "SecondaryActivate", &(0i32, 0i32)).map(drop),
            Do::ContextMenu => conn.call_method(Some(bus.as_str()), path.as_str(), Some(ITEM), "ContextMenu", &(0i32, 0i32)).map(drop),
            Do::Scroll(delta) => conn.call_method(Some(bus.as_str()), path.as_str(), Some(ITEM), "Scroll", &(delta, "vertical")).map(drop),
            Do::MenuShown => match &menu {
                Some(m) => conn.call_method(Some(bus.as_str()), m.as_str(), Some(MENU), "AboutToShow", &(0i32,)).map(drop),
                None => Ok(()),
            },
            Do::Chosen(id) => match &menu {
                Some(m) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
                    conn.call_method(Some(bus.as_str()), m.as_str(), Some(MENU), "Event", &(id, "clicked", Value::from(0i32), now)).map(drop)
                }
                None => Ok(()),
            },
        };
        if let Err(e) = r {
            tracing::debug!("tray: {bus}{path}: {what:?}: {e}");
        }
    });
}

fn string(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    props.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

/// Pictures as an item sends them (ARGB32 in network byte order), as
/// premultiplied RGBA.
pub fn pixmaps(raw: Vec<(i32, i32, Vec<u8>)>) -> Vec<Pixmap> {
    raw.into_iter()
        .filter(|(w, h, data)| *w > 0 && *h > 0 && data.len() >= (*w as usize) * (*h as usize) * 4)
        .map(|(w, h, data)| {
            let mut rgba = Vec::with_capacity(data.len());
            for px in data.chunks_exact(4).take((w * h) as usize) {
                let (a, r, g, b) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
                // (Sent straight; premultiplied for drawing and scaling.)
                rgba.extend_from_slice(&[(r * a / 255) as u8, (g * a / 255) as u8, (b * a / 255) as u8, a as u8]);
            }
            Pixmap { width: w as u32, height: h as u32, rgba: Arc::new(rgba) }
        })
        .collect()
}

fn pixmaps_of(props: &HashMap<String, OwnedValue>, key: &str) -> Vec<Pixmap> {
    pixmaps(props.get(key).and_then(|v| <Vec<(i32, i32, Vec<u8>)>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default())
}

/// An icon name in the item's own theme folder, as a file (largest first).
fn in_theme_path(dir: &str, name: &str) -> Option<String> {
    if dir.is_empty() || name.is_empty() || name.starts_with('/') {
        return None;
    }
    let mut found: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for ext in ["svg", "png"] {
        let flat = std::path::Path::new(dir).join(format!("{name}.{ext}"));
        if flat.is_file() {
            found.push((if ext == "svg" { 1024 } else { 0 }, flat));
        }
    }
    for theme in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for size in std::fs::read_dir(theme.path()).into_iter().flatten().flatten() {
            let px = size.file_name().to_string_lossy().split('x').next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(if size.file_name() == "scalable" { 1024 } else { 0 });
            for kind in std::fs::read_dir(size.path()).into_iter().flatten().flatten() {
                for ext in ["svg", "png"] {
                    let f = kind.path().join(format!("{name}.{ext}"));
                    if f.is_file() {
                        found.push((px, f));
                    }
                }
            }
        }
    }
    found.into_iter().max_by_key(|(px, _)| *px).map(|(_, f)| f.display().to_string())
}

/// What an item's properties say.
fn item_of(key: String, props: &HashMap<String, OwnedValue>) -> (Item, Option<String>) {
    let status = match string(props, "Status").as_str() {
        "Passive" => Status::Passive,
        "NeedsAttention" => Status::NeedsAttention,
        _ => Status::Active,
    };
    let attention = status == Status::NeedsAttention;
    let (mut name, mut pix) = (string(props, "IconName"), pixmaps_of(props, "IconPixmap"));
    if attention {
        let (an, ap) = (string(props, "AttentionIconName"), pixmaps_of(props, "AttentionIconPixmap"));
        if !an.is_empty() || !ap.is_empty() {
            (name, pix) = (an, ap);
        }
    }
    let theme_path = string(props, "IconThemePath");
    let icon = in_theme_path(&theme_path, &name).unwrap_or(name);
    let tooltip = props
        .get("ToolTip")
        .and_then(|v| <(String, Vec<(i32, i32, Vec<u8>)>, String, String)>::try_from(v.try_clone().ok()?).ok())
        .map(|(_, _, title, body)| {
            let body = strip_markup(&body);
            [title, body].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" — ")
        })
        .unwrap_or_default();
    let title = string(props, "Title");
    let menu = props.get("Menu").and_then(|v| zbus::zvariant::OwnedObjectPath::try_from(v.try_clone().ok()?).ok()).map(|p| p.to_string()).filter(|p| p != "/");
    let item_is_menu = props.get("ItemIsMenu").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    (Item { key, title, program: String::new(), tooltip, status, icon, pixmaps: pix, item_is_menu, menu: None, serial: 0 }, menu)
}

/// Text with its markup (`<b>`, `<br/>`, `&amp;`) taken out.
fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").trim().to_owned()
}

/// Read an item's properties, on a thread of its own.
fn read(conn: &Connection, tx: &mpsc::Sender<Event>, bus: String, path: String) {
    let (conn, tx) = (conn.clone(), tx.clone());
    let _ = std::thread::Builder::new().name("tray-read".into()).spawn(move || {
        let props =
            conn.call_method(Some(bus.as_str()), path.as_str(), Some("org.freedesktop.DBus.Properties"), "GetAll", &(ITEM,)).and_then(|m| m.body().deserialize::<HashMap<String, OwnedValue>>());
        let props = match props {
            Ok(p) => p,
            Err(e) => return tracing::debug!("tray: can't read {bus}{path}: {e}"),
        };
        let owner = if bus.starts_with(':') {
            bus.clone()
        } else {
            conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetNameOwner", &(bus.as_str(),))
                .and_then(|m| m.body().deserialize::<String>())
                .unwrap_or_else(|_| bus.clone())
        };
        let (mut item, menu_path) = item_of(format!("{bus}{path}"), &props);
        item.program = conn
            .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetConnectionUnixProcessID", &(owner.as_str(),))
            .and_then(|m| m.body().deserialize::<u32>())
            .ok()
            .and_then(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).ok())
            .and_then(|exe| Some(exe.file_name()?.to_string_lossy().into_owned()))
            .unwrap_or_default();
        // Neither a title nor a program to name it by: its id, at least.
        if item.title.is_empty() && item.program.is_empty() {
            item.title = string(&props, "Id");
        }
        let _ = tx.send(Event::Read { key: item.key.clone(), owner, item: Box::new(item), menu_path });
    });
}

/// Read an item's menu, on a thread of its own.
fn read_menu(conn: &Connection, tx: &mpsc::Sender<Event>, key: String, bus: String, menu: String) {
    let (conn, tx) = (conn.clone(), tx.clone());
    let _ = std::thread::Builder::new().name("tray-menu".into()).spawn(move || {
        type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
        let layout = conn.call_method(Some(bus.as_str()), menu.as_str(), Some(MENU), "GetLayout", &(0i32, -1i32, Vec::<String>::new())).and_then(|m| m.body().deserialize::<(u32, Node)>());
        match layout {
            Ok((_, (_, _, children))) => {
                let entries = children.iter().filter_map(|c| entry(c)).collect();
                let _ = tx.send(Event::Menu { key, entries });
            }
            Err(e) => tracing::debug!("tray: can't read {key}'s menu: {e}"),
        }
    });
}

/// A menu entry from its layout node (`(ia{sv}av)`, in a variant); None
/// for one that isn't shown.
fn entry(v: &Value) -> Option<Entry> {
    let v = match v {
        Value::Value(inner) => inner.as_ref(),
        v => v,
    };
    let Value::Structure(s) = v else { return None };
    let fields = s.fields();
    let Some(Value::I32(id)) = fields.first() else { return None };
    // (Nested in the layout, each value is still in its variant.)
    let unwrap = |mut v: &Value| -> Option<OwnedValue> {
        while let Value::Value(inner) = v {
            v = inner;
        }
        OwnedValue::try_from(v.try_clone().ok()?).ok()
    };
    let props: HashMap<String, OwnedValue> = match fields.get(1) {
        Some(Value::Dict(d)) => d.iter().filter_map(|(k, v)| Some((String::try_from(k.try_clone().ok()?).ok()?, unwrap(v)?))).collect(),
        _ => HashMap::new(),
    };
    let flag = |key: &str, default: bool| props.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(default);
    if !flag("visible", true) {
        return None;
    }
    let children = match fields.get(2) {
        Some(Value::Array(a)) => a.iter().filter_map(entry).collect(),
        _ => Vec::new(),
    };
    let check = match string(&props, "toggle-type").as_str() {
        "checkmark" | "radio" => Some(props.get("toggle-state").and_then(|v| i32::try_from(v).ok()).unwrap_or(0) == 1),
        _ => None,
    };
    Some(Entry {
        id: *id,
        label: mnemonic_free(&string(&props, "label")),
        enabled: flag("enabled", true),
        separator: string(&props, "type") == "separator",
        check,
        icon: Some(string(&props, "icon-name")).filter(|s| !s.is_empty()),
        children,
    })
}

/// A label without its access-key marks (`_Quit`, `Save __As` keeps one).
fn mnemonic_free(label: &str) -> String {
    let mut out = String::new();
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The item's picture for `px` device pixels square: the smallest size it
/// sent that is as big (else its biggest), scaled to fit exactly, aspect
/// kept, so it is drawn a texel to a device pixel; as GPUI wants it
/// (BGRA, straight alpha). None if it sent none.
pub fn picture(item: &Item, px: u32) -> Option<std::sync::Arc<RenderImage>> {
    let best = item.pixmaps.iter().filter(|p| p.width >= px && p.height >= px).min_by_key(|p| p.width * p.height).or_else(|| item.pixmaps.iter().max_by_key(|p| p.width * p.height))?;
    let k = (px as f32 / best.width as f32).min(px as f32 / best.height as f32);
    let (w, h) = (((best.width as f32 * k).round() as u32).max(1), ((best.height as f32 * k).round() as u32).max(1));
    let mut rgba = if (w, h) == (best.width, best.height) {
        best.rgba.as_ref().clone()
    } else {
        let src = image::RgbaImage::from_raw(best.width, best.height, best.rgba.as_ref().clone())?;
        // (Premultiplied, so edges don't fringe as it shrinks.)
        image::imageops::resize(&src, w, h, image::imageops::FilterType::CatmullRom).into_raw()
    };
    // Straight alpha, blue first.
    for p in rgba.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(w, h, rgba)?;
    Some(std::sync::Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// An item's menu as an Olo menu: each entry's command is `sni:ID`.
pub fn menu(title: &str, entries: &[Entry]) -> olo_ui::Menu {
    let mut m = olo_ui::Menu::new(title);
    for e in entries {
        if e.separator {
            // Not first, last or twice in a row.
            if !m.items.is_empty() && !matches!(m.items.last(), Some(olo_ui::MenuItem::Separator)) {
                m = m.separator();
            }
        } else if !e.children.is_empty() {
            m = m.submenu(menu(&e.label, &e.children));
        } else {
            m = m.item(&e.label, format!("sni:{}", e.id)).enabled(e.enabled).checked(e.check == Some(true));
            if let Some(icon) = &e.icon {
                m = m.icon(icon);
            }
        }
    }
    while matches!(m.items.last(), Some(olo_ui::MenuItem::Separator)) {
        m.items.pop();
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_and_labels() {
        // One opaque red pixel and one half-transparent white one, ARGB.
        let p = pixmaps(vec![(2, 1, vec![255, 255, 0, 0, 128, 255, 255, 255]), (0, 0, vec![])]);
        assert_eq!(p.len(), 1);
        assert_eq!(*p[0].rgba, vec![255, 0, 0, 255, 128, 128, 128, 128]);
        assert_eq!(mnemonic_free("_Quit"), "Quit");
        assert_eq!(mnemonic_free("Save __As"), "Save _As");
        assert_eq!(strip_markup("<b>3 unread</b> &amp; more"), "3 unread & more");
        assert_eq!(split_key(":1.45/org/ayatana/NotificationItem/x"), (":1.45".into(), "/org/ayatana/NotificationItem/x".into()));
        assert_eq!(split_key("org.kde.StatusNotifierItem-12-1"), ("org.kde.StatusNotifierItem-12-1".into(), ITEM_PATH.into()));
    }

    #[test]
    fn badges_follow_updates() {
        let props = |pairs: &[(&str, Value<'static>)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), OwnedValue::try_from(v.clone()).unwrap())).collect::<HashMap<_, _>>();
        let mut b = Badge { app: "slack".into(), ..Default::default() };
        update_badge(&mut b, ":1.5".into(), &props(&[("count", Value::from(3i64)), ("count-visible", Value::from(true))]));
        assert_eq!(b.count, Some(3));
        // A progress alone keeps the count.
        update_badge(&mut b, ":1.5".into(), &props(&[("progress", Value::from(0.4f64)), ("progress-visible", Value::from(true))]));
        assert_eq!((b.count, b.progress), (Some(3), Some(0.4)));
        update_badge(&mut b, ":1.5".into(), &props(&[("count-visible", Value::from(false)), ("urgent", Value::from(true))]));
        assert_eq!((b.count, b.urgent), (None, true));
        // Zero unread is no count.
        update_badge(&mut b, ":1.5".into(), &props(&[("count", Value::from(0i64)), ("count-visible", Value::from(true))]));
        assert_eq!(b.count, None);
    }

    #[test]
    fn layout_nodes() {
        // As GetLayout sends a child: (ia{sv}av) in a variant, each
        // property in its own.
        let props: HashMap<String, Value> = HashMap::from([
            ("label".to_owned(), Value::from("_Quit")),
            ("toggle-type".to_owned(), Value::from("checkmark")),
            ("toggle-state".to_owned(), Value::from(1i32)),
            ("enabled".to_owned(), Value::from(false)),
        ]);
        let node = Value::Value(Box::new(Value::from((7i32, props, Vec::<Value>::new()))));
        let e = entry(&node).unwrap();
        assert_eq!((e.id, e.label.as_str(), e.enabled, e.check), (7, "Quit", false, Some(true)));
        let hidden: HashMap<String, Value> = HashMap::from([("visible".to_owned(), Value::from(false))]);
        assert!(entry(&Value::from((8i32, hidden, Vec::<Value>::new()))).is_none());
    }

    #[test]
    fn menus_become_olo_menus() {
        let e = |id, label: &str| Entry { id, label: label.into(), enabled: true, ..Default::default() };
        let sep = Entry { separator: true, ..Default::default() };
        let entries = vec![
            sep.clone(),
            e(1, "Show Slack"),
            sep.clone(),
            sep.clone(),
            Entry { check: Some(true), ..e(2, "Launch at login") },
            Entry { children: vec![e(4, "Away")], ..e(3, "Status") },
            sep.clone(),
        ];
        let m = menu("Slack", &entries);
        assert_eq!(m.items.len(), 4);
        assert!(matches!(&m.items[0], olo_ui::MenuItem::Action(a) if a.command == "sni:1"));
        assert!(matches!(&m.items[1], olo_ui::MenuItem::Separator));
        assert!(matches!(&m.items[2], olo_ui::MenuItem::Action(a) if a.checked));
        assert!(matches!(&m.items[3], olo_ui::MenuItem::Submenu(s) if s.title == "Status"));
    }
}
