//! olo-palette: the command palette. ⌘Space (or a tap of ⌘, if chosen
//! in Preferences ▸ Keyboard) brings it up over everything; typing finds
//! applications, the front app's menu commands, open windows, the
//! desktop's shortcuts (with their keys, so it doubles as a reminder),
//! preference panels, settings, Passwords' items and files, and works out
//! sums. Return does the selected one; Escape, a click outside or ⌘Space
//! again puts it away. On Olo's own kit (olo-kit): what lasts between its
//! showings (the applications, what was chosen, the model, the workers)
//! is a `Hub` on the program's looper; each showing is a window with a
//! `Palette` view of its own, told what's found.
//!
//! Slash commands (Ben, 2026-11-07): `/ws TEXT` searches the web;
//! `/ask QUESTION` asks the AI (Preferences ▸ AI's Ask: `ask.rs`) and
//! shows the answer in the panel, the prose as it comes, then up to four
//! numbered snippets: a number shows that one whole, Return copies it,
//! Escape goes back a step, Space asks a follow-up in the same
//! conversation. `/` alone lists them.
//!
//! It stays running between uses so it opens at once: app_server starts
//! `olo-palette --toggle`, which hands the toggle to the running copy.
//! It shows a window's menu (a right-click on its tab) the same way, from
//! `--window-menu` (window_menu.rs), and the shortcuts sheet (⇧⌘/) from
//! `--shortcuts` (sheet.rs).

mod answer;
mod ask;
mod calc;
mod model;
mod rank;
mod sheet;
mod sources;
mod window_menu;

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use olo_kit::{
    Bounds, Child, ClipboardItem, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, LayerKeyboard, LayerShellLayer, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, Painter, ParentElement as _, Pixels, Point, ScrollWheelEvent, Styled as _, View, WindowHandle, WindowOptions, clipboard, div, icons,
    instance::{self, Launch},
    metrics as m, point, px, size,
    style::{self, c},
    theme, tracker,
    widgets::{KeyHint, TextField, TextFieldEvent, text_field},
};
use olo_protocol::WindowInfo;
use serde::{Deserialize, Serialize};
use sources::{Action, Found, Item, Kind, Workers};

const APP_ID: &str = "olo.palette";
/// The panel's width, and how far down the screen it sits.
const WIDTH: f32 = 600.0;
const TOP: f32 = 0.2;
const FIELD_HEIGHT: f32 = 40.0;
const ROW: f32 = 34.0;
const ICON: f32 = 24.0;
const ROWS: usize = 9;
/// The query's type size.
const QUERY_SIZE: f32 = 20.0;

/// How often and how lately each thing was chosen.
#[derive(Default, Serialize, Deserialize)]
struct Recents {
    chosen: HashMap<String, (u32, u64)>,
}

impl Recents {
    fn path() -> PathBuf {
        olo_xdg::config_dir().join("palette.json")
    }
    fn load() -> Recents {
        let read = |p: PathBuf| olo_kit::store::load::<Recents>(&p);
        // Before the rename (Tabula) it was kept a folder deeper, and named
        // our programs by their old names.
        read(Self::path())
            .or_else(|| {
                let mut old = read(olo_xdg::config_dir().join(olo_xdg::legacy::OLD).join("palette.json"))?;
                old.chosen = old.chosen.into_iter().map(|(k, v)| (k.replace("tabula-", "olo-").replace("tabula.", "olo."), v)).collect();
                Some(old)
            })
            .unwrap_or_default()
    }
    fn save(&self) {
        olo_kit::store::save(Self::path(), self);
    }
    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    fn note(&mut self, key: String) {
        let e = self.chosen.entry(key).or_default();
        e.0 += 1;
        e.1 = Self::now();
        // Keep it small.
        if self.chosen.len() > 300 {
            let oldest = self.chosen.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.chosen.remove(&k);
            }
        }
        self.save();
    }
    /// Boost for something chosen often and lately.
    fn boost(&self, key: &str) -> i32 {
        let Some(&(count, when)) = self.chosen.get(key) else { return 0 };
        let days = Self::now().saturating_sub(when) / 86_400;
        (count.min(20) as i32 * 6)
            + if days < 1 {
                40
            } else if days < 7 {
                20
            } else {
                0
            }
    }
}

/// What lasts between showings, shared by the hub and each window's
/// palette.
#[derive(Clone)]
struct Shared {
    /// Applications, preference panels and settings.
    fixed: Arc<Vec<Item>>,
    recents: Arc<Mutex<Recents>>,
    /// The model ranking what's typed, and what it knows of the choices.
    ranker: Arc<Mutex<model::Ranker>>,
    /// A tune on its way (one at a time).
    tuning: Arc<AtomicBool>,
    workers: Workers,
}

/// Counts the questions to the model, whichever window asked (each
/// one's updates carry its number: a late answer is to no one).
static ASKS: AtomicU64 = AtomicU64::new(0);

/// A palette's window has closed.
struct Closed;

/// On the program's looper: what's shared, and the windows showing.
struct Hub {
    shared: Shared,
    win: Option<WindowHandle<Palette>>,
    /// Asked to show; opens once the window in front is known.
    opening: bool,
    /// A window's menu, while it shows.
    window_menu: Option<WindowHandle<window_menu::Layer>>,
    /// The shortcuts sheet, while it shows.
    sheet: Option<WindowHandle<sheet::Sheet>>,
}

impl Hub {
    fn showing(&self) -> Option<&WindowHandle<Palette>> {
        self.win.as_ref().filter(|w| w.is_open())
    }

    fn hide(&mut self) {
        if let Some(w) = self.win.take() {
            w.close();
        }
    }

    fn toggle(&mut self) {
        if self.showing().is_some() {
            return self.hide();
        }
        if self.opening {
            return;
        }
        self.opening = true;
        // Learn which window is in front before taking the keyboard from it.
        let proxy = olo_kit::proxy();
        let _ = std::thread::Builder::new().name("palette-context".into()).spawn(move || {
            let (front, items) = sources::context();
            proxy.send(Found::Context { front: front.clone(), items });
            if let Some(f) = front {
                proxy.send(Found::Menus(sources::menus(&f)));
            }
        });
    }

    /// The shortcuts sheet, or away with it if it shows.
    fn toggle_sheet(&mut self) {
        // (Unless it put itself away: Escape, a click outside.)
        if let Some(old) = self.sheet.take()
            && old.is_open()
        {
            return old.close();
        }
        self.hide();
        sheet::learn(olo_kit::proxy());
    }

    /// A right-click on a window's tab: put the palette away and learn
    /// what the menu needs.
    fn ask_window_menu(&mut self, ask: window_menu::Ask) {
        self.hide();
        if let Some(old) = self.window_menu.take() {
            old.close();
        }
        window_menu::learn(ask, olo_kit::proxy());
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) if launch.args.iter().any(|a| a == "--toggle") => return self.toggle(),
            Ok(launch) if launch.args.iter().any(|a| a == "--shortcuts") => return self.toggle_sheet(),
            Ok(launch) => {
                if let Some(ask) = window_menu::Ask::parse(&launch.args) {
                    self.ask_window_menu(ask);
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Closed>() {
            Ok(_) => {
                // (Unless another has come up since.)
                if self.showing().is_none() {
                    self.win = None;
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<sheet::Learned>() {
            Ok(learned) => {
                if let Some(old) = self.sheet.take() {
                    old.close();
                }
                self.sheet = Some(sheet::open(*learned));
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<window_menu::Facts>() {
            Ok(facts) => {
                if let Some(old) = self.window_menu.take() {
                    old.close();
                }
                self.window_menu = Some(window_menu::open(*facts));
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<ask::Asked>() {
            Ok(asked) => {
                if let Some(w) = self.showing() {
                    w.messenger().send(Msg::Asked(*asked));
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<model::Retuned>() {
            Ok(r) => {
                self.shared.tuning.store(false, Ordering::Relaxed);
                if let Some(w) = r.0 {
                    self.shared.ranker.lock().unwrap_or_else(|e| e.into_inner()).weights = w;
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(found) = message.downcast::<Found>() else { return };
        match *found {
            Found::Fixed(items) => {
                self.shared.fixed = Arc::new(items);
                if let Some(w) = self.showing() {
                    w.messenger().send(Msg::Fixed(self.shared.fixed.clone()));
                }
            }
            Found::Context { front, items } => {
                if std::mem::take(&mut self.opening) {
                    let shared = self.shared.clone();
                    self.win = Some(olo_kit::open_window(overlay("Command Palette", "palette", None), move |cx| Palette::new(shared, front, items, cx)));
                }
            }
            found => {
                if let Some(w) = self.showing() {
                    w.messenger().send(Msg::Found(found));
                }
            }
        }
    }
}

/// Where the palette's parts are in its window (the whole display).
#[derive(Clone, Copy, Default)]
struct Geo {
    panel: Bounds<Pixels>,
    field: Bounds<Pixels>,
    glass: Bounds<Pixels>,
    list: Bounds<Pixels>,
}

/// A showing of the palette: its window's view.
struct Palette {
    shared: Shared,
    query: Child<TextField>,
    /// The query's text, as last seen.
    query_text: String,
    /// Bumped with each change to the query, so late answers are dropped.
    asked: u64,
    selected: usize,
    /// Scrolled this many rows down.
    top: usize,
    /// ⌘ held: a file's row says Open in Tracker.
    command: bool,
    /// Files and folders shown rather than commands, shortcuts and the
    /// rest (a tap of Ctrl goes from one to the other: Ben, 2026-11-03,
    /// "not show file/folder matches by default, and you have to press
    /// ctrl to have it show them").
    files_mode: bool,
    /// Ctrl went down on its own and nothing else yet: let go, it's a tap.
    ctrl_tap: bool,
    /// Ctrl has been seen up since the palette opened: until then a Ctrl
    /// held is the shortcut that opened it (⌘ Space, ⌘ reaching apps as
    /// Ctrl, its press before the window had the keyboard), and letting
    /// it go is no tap (it opened on files: Ben, 2026-11-04).
    ctrl_free: bool,
    fixed: Arc<Vec<Item>>,
    context: Vec<Item>,
    menus: Vec<Item>,
    files: Vec<Item>,
    passwords: Vec<Item>,
    /// Go to the path in the clipboard, if it holds one that's there
    /// (asked for once, when the window has the keyboard).
    copied: Option<Item>,
    read_copied: bool,
    /// Go to the path typed, for the query numbered so.
    typed: Option<(u64, Item)>,
    results: Vec<Item>,
    /// Each result's features, as the model ranked it (None: not its,
    /// a sum, a path typed, a web search): what a choice is learnt from.
    features: Vec<Option<[f32; model::F]>>,
    front: Option<WindowInfo>,
    recents: Arc<Mutex<Recents>>,
    ranker: Arc<Mutex<model::Ranker>>,
    /// An answer showing (/ask).
    answer: Option<answer::Answering>,
    /// Where its parts were last laid out (the results are drawn there,
    /// not where layout rounds their box to: at 1.5× that's a pixel off).
    geo: Geo,
}

/// What a palette is told.
enum Msg {
    Fixed(Arc<Vec<Item>>),
    /// The front app's menus, files or passwords, found.
    Found(Found),
    Asked(ask::Asked),
    /// The path in the clipboard, if it's there.
    Copied(Item),
    /// The path typed for query number this, if it's there.
    Typed(u64, Option<Item>),
}

/// The slash commands: what's typed, what it does, its icon.
const SLASH: [(&str, &str, &str); 2] = [("/ws", "Search the web", "browser"), ("/ask", "Ask AI", "ai")];

/// The results for a slash command typed, if it's one: `/` or the start
/// of one lists them (the palette's commands, not a path: nothing after
/// a second `/`); `/ws TEXT` searches the web; `/ask QUESTION` asks.
fn slash(query: &str) -> Option<Vec<Item>> {
    let q = query.trim_start();
    if !q.starts_with('/') {
        return None;
    }
    let (word, rest) = q.split_once(char::is_whitespace).map_or((q, ""), |(w, r)| (w, r.trim()));
    let command = SLASH.iter().find(|(c, ..)| *c == word);
    match command {
        Some(("/ws", ..)) if !rest.is_empty() => Some(vec![Item::new(Kind::Web, format!("Search the web for “{rest}”"), Action::Search(rest.to_owned())).icon("browser").subtitle("Web browser")]),
        Some(("/ask", ..)) if !rest.is_empty() => {
            Some(vec![Item::new(Kind::Command, format!("Ask: “{rest}”"), Action::Ask(rest.to_owned())).icon("ai").subtitle("Return asks the model in Preferences ▸ AI")])
        }
        Some((c, what, icon)) => Some(vec![Item::new(Kind::Command, format!("{c} …"), Action::Fill(format!("{c} "))).icon(*icon).subtitle(format!("{what}: type what after it"))]),
        None if rest.is_empty() && !word[1..].contains('/') => {
            let found: Vec<Item> = SLASH
                .iter()
                .filter(|(c, ..)| c.starts_with(word))
                .map(|(c, what, icon)| Item::new(Kind::Command, c.to_string(), Action::Fill(format!("{c} "))).icon(*icon).subtitle(what.to_string()))
                .collect();
            (!found.is_empty()).then_some(found)
        }
        None => None,
    }
}

/// The overlay over a whole output that the palette, the sheet and a
/// window's menu sit on: the keyboard is theirs.
fn overlay(title: &str, namespace: &str, output: Option<String>) -> WindowOptions {
    let mut options = WindowOptions::new(title.to_owned(), 1.0, 1.0)
        .layer(namespace, LayerShellLayer::Overlay, (true, true, true, true))
        .layer_options(|l| {
            l.exclusive_zone = -1;
            l.keyboard = LayerKeyboard::Exclusive;
            l.output = output;
        })
        .no_panel();
    options.fixed = true;
    options
}

/// `v` on the device pixel grid at `scale` (`style::snap_coord`, where
/// there's no painter: laying out).
fn snap_at(scale: f32, v: Pixels) -> Pixels {
    px((f32::from(v) * scale).round() / scale)
}

/// `b`'s edges on the device pixel grid (`style::snap`).
pub(crate) fn snapped(scale: f32, b: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::from_corners(point(snap_at(scale, b.left()), snap_at(scale, b.top())), point(snap_at(scale, b.right()), snap_at(scale, b.bottom())))
}

/// A box centred on `center`, on whole device pixels (`style::snap_centred`).
fn snapped_centred(scale: f32, center: Point<Pixels>, w: f32, h: f32) -> Bounds<Pixels> {
    let (dw, dh) = ((w * scale).round(), (h * scale).round());
    let min = point(px(((f32::from(center.x) * scale - dw / 2.0).round()) / scale), px(((f32::from(center.y) * scale - dh / 2.0).round()) / scale));
    Bounds { origin: min, size: size(px(dw / scale), px(dh / scale)) }
}

impl Palette {
    fn new(shared: Shared, front: Option<WindowInfo>, context: Vec<Item>, cx: &mut Cx<Self>) -> Palette {
        let query = text_field(cx, |f| f.name("Search").font_size(QUERY_SIZE).hint(hint(false)));
        query.focus(cx);
        cx.subscribe(&query, |p: &mut Palette, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => {
                // (An answer's Return copies: the field has the key
                // before any listener does.)
                match p.answer.as_ref().map(|a| a.following) {
                    Some(true) => return p.follow_up(cx),
                    Some(false) => {
                        p.copy_answer(true, cx);
                        return;
                    }
                    None => {}
                }
                let i = p.selected;
                p.choose(i, cx);
            }
            _ => p.query_changed(cx),
        });
        let mut p = Palette {
            fixed: shared.fixed.clone(),
            recents: shared.recents.clone(),
            ranker: shared.ranker.clone(),
            shared,
            query,
            query_text: String::new(),
            asked: 0,
            selected: 0,
            top: 0,
            command: false,
            files_mode: false,
            ctrl_tap: false,
            ctrl_free: false,
            context,
            menus: Vec::new(),
            files: Vec::new(),
            passwords: Vec::new(),
            copied: None,
            read_copied: false,
            typed: None,
            results: Vec::new(),
            features: Vec::new(),
            front,
            answer: None,
            geo: Geo::default(),
        };
        p.refresh();
        p
    }

    /// Offer to go to the path in the clipboard, if it holds one that's
    /// there: read and looked for off the window's looper (a program slow
    /// to give what it copied, a slow disk, never hold the palette up).
    fn read_copied(&mut self, cx: &mut Cx<Self>) {
        if std::mem::replace(&mut self.read_copied, true) {
            return;
        }
        let to = cx.messenger();
        clipboard::paste_as(sources::TEXT.iter().map(|t| t.to_string()).collect(), move |read| {
            let Some((_, bytes)) = read else { return };
            let _ = std::thread::Builder::new().name("palette-copied".into()).spawn(move || {
                if let Some(found) = std::str::from_utf8(&bytes).ok().and_then(|text| sources::go_to(text, true)) {
                    to.send(Msg::Copied(found));
                }
            });
        });
    }

    fn hide(&mut self, cx: &mut Cx<Self>) {
        self.stop_asking();
        cx.close_window();
    }

    /// The results for the query now.
    fn refresh(&mut self) {
        let query = self.query_text.clone();
        // A slash command: its own results alone.
        if !self.files_mode
            && let Some(items) = slash(&query)
        {
            self.features = vec![None; items.len()];
            self.results = items;
            self.selected = 0;
            return;
        }
        let words = rank::words(&query);
        let mut scored: Vec<(i32, Item)> = Vec::new();
        let mut features: Vec<Option<[f32; model::F]>> = Vec::new();
        if words.is_empty() {
            // Nothing typed: what was chosen lately.
            let all = self.fixed.iter().chain(&self.context);
            let recents = self.recents.lock().unwrap_or_else(|e| e.into_inner());
            let mut recent: Vec<(i32, Item)> = all.filter_map(|i| i.action.key().map(|k| (recents.boost(&k), i.clone()))).filter(|(b, _)| *b > 0).collect();
            drop(recents);
            recent.sort_by_key(|r| std::cmp::Reverse(r.0));
            // The path in the clipboard first: likeliest wanted.
            scored = self.copied.iter().map(|i| (0, i.clone())).chain(recent).take(ROWS).collect();
        } else {
            if let Some(v) = calc::eval(&query) {
                let shown = calc::show(v);
                let mut item = Item::new(Kind::Calculator, format!("= {shown}"), Action::Copy(shown)).subtitle(format!("{} — Return copies it", query.trim())).icon("calculator");
                item.keywords = String::new();
                scored.push((i32::MAX / 2, item));
            }
            // A path typed: go there, first.
            if let Some((n, item)) = &self.typed
                && *n == self.asked
            {
                scored.push((i32::MAX / 2 - 1, item.clone()));
            }
            // Files and folders, or (at first) everything else, no file
            // among them (Ben: "i only want to see commands unless i press
            // ctrl then only files"); the path in the clipboard either way.
            // Ranked by the model (`model`): the matches' features, scored
            // in one pass.
            features.resize(scored.len(), None);
            let typed = query.trim().to_lowercase();
            let front_app = self.front.as_ref().map(|f| f.app_id.clone()).unwrap_or_default();
            let now = model::Now::new(Recents::now(), &typed, &front_app);
            let open: std::collections::HashSet<&str> = self.context.iter().chain(self.fixed.iter()).filter(|i| i.kind == Kind::Window).map(|i| i.subtitle.as_str()).collect();
            let ranker = self.ranker.lock().unwrap_or_else(|e| e.into_inner());
            let ranker = &*ranker;
            let mut rows: Vec<[f32; model::F]> = Vec::new();
            let by_model = |found: &[(&Item, i32)]| -> Vec<i32> {
                rows = found
                    .iter()
                    .map(|(item, s)| ranker.features(item, *s, words.len(), item.kind == Kind::Window || (item.kind == Kind::Application && open.contains(item.title.as_str())), &now))
                    .collect();
                ranker.scores(&rows).iter().map(|v| (v * 100.0).round() as i32).collect()
            };
            let found = if self.files_mode {
                sources::ranked_with(&words, self.files.iter().chain(&self.copied), by_model)
            } else {
                let all = self.fixed.iter().chain(&self.context).chain(&self.menus).chain(&self.passwords).chain(&self.copied);
                sources::ranked_with(&words, all, by_model)
            };
            for (s, item, n) in found {
                scored.push((s, item));
                features.push(rows.get(n).copied());
            }
            if !self.files_mode {
                let q = query.trim().to_owned();
                scored.push((i32::MIN, Item::new(Kind::Web, format!("Search the web for “{q}”"), Action::Search(q)).icon("browser").subtitle("Web browser")));
                features.push(None);
            }
        }
        features.resize(scored.len(), None);
        self.features = features;
        self.results = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        if !self.results.get(self.selected).is_some_and(|i| i.enabled) {
            self.selected = self.results.iter().position(|i| i.enabled).unwrap_or(0);
        }
    }

    fn query_changed(&mut self, cx: &mut Cx<Self>) {
        let text = self.query.read(cx).text().to_owned();
        if text == self.query_text {
            return;
        }
        self.query_text = text;
        self.asked += 1;
        self.selected = 0;
        self.top = 0;
        // Files are searched only while they're what's shown (a tap of
        // Ctrl); commands are found here, as typed.
        if self.files_mode {
            let _ = self.shared.workers.files.send((self.asked, rank::words(&self.query_text)));
        }
        let _ = self.shared.workers.passwords.send((self.asked, self.query_text.clone()));
        // A path written whole: looked for on a worker, offered if it's
        // there.
        if olo_kit::files::written_path(&self.query_text).is_some() {
            let (text, asked) = (self.query_text.clone(), self.asked);
            cx.work(move || sources::go_to(&text, false), move |item| Msg::Typed(asked, item));
        }
        self.refresh();
        cx.invalidate();
    }

    /// Files and folders, or back to the rest (a tap of Ctrl).
    fn switch_files(&mut self, cx: &mut Cx<Self>) {
        self.files_mode = !self.files_mode;
        let files = self.files_mode;
        // To files: what's typed, looked for now (not while commands were
        // shown); their answer keeps to this query's number.
        if files {
            self.files.clear();
            let _ = self.shared.workers.files.send((self.asked, rank::words(&self.query_text)));
        }
        self.query.update(cx, |f, cx| f.set_hint(hint(files), cx));
        self.selected = 0;
        self.top = 0;
        self.refresh();
        cx.invalidate();
    }

    /// The modifiers changed: ⌘ held (Open in Tracker), and a tap of
    /// Ctrl on its own (not ⌘, which comes as the right Ctrl with a
    /// command key) switches to files and back.
    fn modifiers(&mut self, e: &ModifiersChangedEvent, cx: &mut Cx<Self>) {
        let held = e.modifiers.platform || e.modifiers.control;
        if self.command != held {
            self.command = held;
            cx.invalidate();
        }
        let m = e.modifiers;
        if !m.control {
            self.ctrl_free = true;
        }
        // (Still the opening shortcut's Ctrl: no tap of it.)
        if !self.ctrl_free {
            self.ctrl_tap = false;
            return;
        }
        let ctrl = m.control && !(olo_kit_base::keys::command_key() && olo_kit::right_control_held());
        let alone = ctrl && !m.shift && !m.alt && !m.platform;
        if alone && !self.ctrl_tap {
            self.ctrl_tap = true;
        } else if !m.control && self.ctrl_tap {
            self.ctrl_tap = false;
            self.switch_files(cx);
        } else if !alone {
            self.ctrl_tap = false;
        }
    }

    fn note(&self, key: String) {
        self.recents.lock().unwrap_or_else(|e| e.into_inner()).note(key);
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<Self>) {
        let Some(item) = self.results.get(index).cloned() else { return };
        if !item.enabled {
            return;
        }
        if let Some(k) = item.action.key() {
            self.note(k);
        }
        match &item.action {
            // (The palette's own: the query filled, or asked here.)
            Action::Fill(text) => {
                let text = text.clone();
                self.query.update(cx, |f, cx| f.set_text(text, cx));
                self.query_changed(cx);
                return;
            }
            Action::Ask(question) => {
                let question = question.clone();
                return self.start_ask(question, Vec::new(), cx);
            }
            _ => {}
        }
        self.learn(index);
        if let Action::Copy(text) = &item.action {
            clipboard::write(ClipboardItem::new_string(text.clone()));
        }
        let front = self.front.as_ref().map(|f| f.id);
        self.hide(cx);
        sources::perform(item.action, front);
    }

    /// The model learns that result `index` was chosen: the choice noted,
    /// with what was shown, and the model tuned again on a worker (a few
    /// milliseconds; one at a time).
    fn learn(&mut self, index: usize) {
        let Some(item) = self.results.get(index) else { return };
        let front_app = self.front.as_ref().map(|f| f.app_id.clone()).unwrap_or_default();
        let Some(key) = model::key_of(item, &front_app) else { return };
        let typed = self.query_text.trim().to_lowercase();
        let choice = model::Choice { at: Recents::now(), typed, key, kind: item.kind.label().to_owned(), app: front_app };
        // The model's results, and where among them the one chosen is.
        let rows: Vec<[f32; model::F]> = self.features.iter().flatten().copied().collect();
        let chosen = self.features[..index.min(self.features.len())].iter().flatten().count();
        let chosen = if self.features.get(index).is_some_and(Option::is_some) { chosen } else { usize::MAX };
        let start = {
            let mut ranker = self.ranker.lock().unwrap_or_else(|e| e.into_inner());
            ranker.chose(choice, &rows, chosen);
            ranker.weights.clone()
        };
        if self.shared.tuning.swap(true, Ordering::Relaxed) {
            return;
        }
        let proxy = olo_kit::proxy();
        std::thread::Builder::new()
            .name("palette tune".into())
            .spawn(move || {
                let tuned = model::tune(&start);
                if let Some(t) = &tuned {
                    let r = &t.report;
                    tracing::info!(target: "olo::palette", examples = r.examples, before = r.before.mrr, after = r.after.mrr, baseline = r.baseline.mrr, kept = r.kept, micros = r.micros, "the palette's model tuned");
                }
                proxy.send(model::Retuned(tuned.filter(|t| t.report.kept).map(|t| t.weights)));
            })
            .ok();
    }

    /// ⌘↩ or ⌘-click: a file or folder found shown in Tracker rather than
    /// opened; anything else chosen as usual.
    fn reveal(&mut self, index: usize, cx: &mut Cx<Self>) {
        let Some(Item { action: Action::Open(path), enabled: true, .. }) = self.results.get(index).cloned() else {
            return self.choose(index, cx);
        };
        if let Some(k) = Action::Open(path.clone()).key() {
            self.note(k);
        }
        self.hide(cx);
        if let Err(e) = tracker::open(&path) {
            tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
        }
    }

    fn step(&mut self, by: isize) {
        let n = self.results.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + by).rem_euclid(n as isize);
            if self.results[i as usize].enabled {
                break;
            }
        }
        self.selected = i as usize;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + ROWS {
            self.top = self.selected + 1 - ROWS;
        }
    }

    /// Arrows, Page Up and Down, Escape and ⌘↩ belong to the list, not
    /// the text (Return comes as the field's).
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
        let k = &e.keystroke;
        // (Ctrl with a key isn't a tap.)
        self.ctrl_tap = false;
        if self.answer.is_some() {
            return self.answer_key(e, cx);
        }
        if olo_kit_base::keys::shortcut("alternate").is_some_and(|s| olo_kit::keys::pressed(&s, k)) {
            let i = self.selected;
            self.reveal(i, cx);
            return true;
        }
        if k.modifiers.modified() {
            return false;
        }
        match k.key.as_str() {
            "escape" => self.hide(cx),
            "up" => self.step(-1),
            "down" => self.step(1),
            "pageup" => self.step(-(ROWS as isize)),
            "pagedown" => self.step(ROWS as isize),
            _ => return false,
        }
        cx.invalidate();
        true
    }

    /// Ask `question` (after `history`): the answer shows in the panel as
    /// it comes, the question in the field as asked.
    fn start_ask(&mut self, question: String, history: Vec<olo_ai::Turn>, cx: &mut Cx<Self>) {
        self.stop_asking();
        let n = ASKS.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        ask::ask(n, question.clone(), history.clone(), cancel.clone(), olo_kit::proxy());
        self.answer = Some(answer::Answering::new(n, question.clone(), history, cancel));
        self.show_question(cx);
        cx.invalidate();
    }

    /// The question asked, in the field (where typing goes only for a
    /// follow-up).
    fn show_question(&mut self, cx: &mut Cx<Self>) {
        let Some(a) = &self.answer else { return };
        let text = format!("/ask {}", a.question);
        let shown = text.clone();
        self.query.update(cx, |f, cx| {
            f.set_text(shown, cx);
            f.set_hint(hint(false), cx);
        });
        self.query_text = text;
    }

    /// The answer let go of (its reply too, if it's still coming).
    fn stop_asking(&mut self) {
        if let Some(a) = self.answer.take() {
            a.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The answer as it comes.
    fn asked(&mut self, asked: ask::Asked, cx: &mut Cx<Self>) {
        let Some(a) = self.answer.as_mut() else { return };
        match asked {
            ask::Asked::Model(n, model) if n == a.n => a.model = model,
            ask::Asked::SoFar(n, prose) if n == a.n && a.json.is_none() => a.prose = prose,
            ask::Asked::Done(n, result) if n == a.n => match result {
                Ok((prose, snippets, json)) => {
                    a.prose = prose;
                    a.snippets = snippets;
                    a.json = Some(json);
                }
                Err(e) => a.error = Some(e),
            },
            _ => return,
        }
        cx.invalidate();
    }

    /// Return (or ⌘C) on an answer: the snippet shown copied, else the
    /// only one, else the answer's words; then with Return the palette
    /// goes (Ben: "if i press enter to copy a snippet it should close the
    /// command pallette too"), with ⌘C it stays and says so in the foot.
    fn copy_answer(&mut self, close: bool, cx: &mut Cx<Self>) -> bool {
        let Some(a) = self.answer.as_mut() else { return false };
        let Some((what, text)) = a.to_copy() else { return false };
        clipboard::write(ClipboardItem::new_string(text));
        if close {
            self.hide(cx);
        } else {
            a.copied = Some(what);
            cx.invalidate();
        }
        true
    }

    /// A follow-up typed (Return): asked in the same conversation.
    fn follow_up(&mut self, cx: &mut Cx<Self>) {
        let text = self.query.read(cx).text().trim().to_owned();
        let Some(a) = &self.answer else { return };
        if text.is_empty() {
            return;
        }
        let mut history = a.history.clone();
        if let Some(json) = &a.json {
            history.push(olo_ai::Turn { question: a.question.clone(), answer: json.clone() });
        }
        self.start_ask(text, history, cx);
    }

    /// The keys while an answer shows (Ben: "if i press the number then it
    /// shows that one expanded, and if i hit enter it copies that code to
    /// the clipboard, if i hit escape it goes back one … and escape again
    /// goes back so i can type something else … if i hit space let me ask
    /// a followup question"): a number shows that snippet whole, Return
    /// copies it (with none shown: the only one, else the answer), Escape
    /// goes back a step (to the answer, then to the palette: stopping it
    /// if it's still coming), Space asks a follow-up, the arrows scroll.
    fn answer_key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
        let k = &e.keystroke;
        let Some(a) = self.answer.as_mut() else { return false };
        if a.following {
            // The field's, but Escape goes back to the answer.
            if k.key == "escape" && !k.modifiers.modified() {
                a.following = false;
                self.show_question(cx);
                cx.invalidate();
                return true;
            }
            return false;
        }
        // (⌘C copies as Return does; other keys with modifiers are no
        // one's here.)
        let copy = k.key == "enter" || (k.key == "c" && (k.modifiers.platform || k.modifiers.control));
        if k.modifiers.modified() && !copy {
            return true;
        }
        let line = answer::mono_line();
        match k.key.as_str() {
            "escape" => {
                if a.open.take().is_some() {
                    a.scroll = 0.0;
                    a.copied = None;
                } else {
                    self.stop_asking();
                    self.query.update(cx, |f, cx| f.set_text("", cx));
                    self.query_text.clear();
                    self.selected = 0;
                    self.top = 0;
                    self.refresh();
                }
            }
            "space" if a.done() => {
                a.following = true;
                a.copied = None;
                self.query.update(cx, |f, cx| {
                    f.set_text("", cx);
                    f.set_hint("Ask a follow-up, then Return", cx);
                });
                self.query_text.clear();
            }
            _ if copy => return self.copy_answer(false, cx) || true,
            "up" => a.scroll = (a.scroll - 3.0 * line).max(0.0),
            "down" => a.scroll += 3.0 * line,
            "pageup" => a.scroll = (a.scroll - 240.0).max(0.0),
            "pagedown" => a.scroll += 240.0,
            n if n.len() == 1 && n.as_bytes()[0].is_ascii_digit() => {
                let i = (n.as_bytes()[0] - b'0') as usize;
                if i >= 1 && i <= a.snippets.len() {
                    // (Its number again: back, as Escape; Ben: "pressing that
                    // number again should also unzoom it".)
                    a.open = if a.open == Some(i - 1) { None } else { Some(i - 1) };
                    a.scroll = 0.0;
                    a.copied = None;
                }
            }
            _ => {}
        }
        cx.invalidate();
        true
    }
}

impl Palette {
    /// The panel, the query's well and the glass.
    fn paint_backdrop(&self, g: &mut Painter) {
        let (Geo { panel, field, glass, .. }, p) = (self.geo, theme::palette());
        // A soft shadow, then the BeOS panel.
        style::soft_shadow(g, panel, &p);
        style::fill_rect(g, panel, c(p.panel_background));
        style::frame_rect(g, panel, style::tint(p.panel_background, 1.6));
        style::raised(g, style::inset(g, panel, 1.0), &p);
        // The query's well, and the glass.
        style::fill_rect(g, field, c(p.document_background));
        style::sunken(g, field, &p, true);
        icons::paint(g, glass, "query");
    }

    /// The row at `y` of the results drawn at `list`.
    fn row_at(list: Bounds<Pixels>, y: Pixels) -> Option<usize> {
        let k = (f32::from(y - list.top()) - m::TIGHT) / ROW;
        (k >= 0.0).then_some(k as usize)
    }

    fn paint_results(&self, g: &mut Painter, list: Bounds<Pixels>) {
        let p = theme::palette();
        for (n, (index, item)) in (self.top..self.results.len()).take(ROWS).map(|i| (i, &self.results[i])).enumerate() {
            let r = Bounds { origin: point(list.left(), list.top() + px(m::TIGHT + n as f32 * ROW)), size: size(list.size.width, px(ROW)) };
            // (The row the pointer comes over is chosen: only rows'
            // edges matter to it.)
            g.note_pointer_edge(r);
            let is_selected = index == self.selected;
            let (fg, dim) = if is_selected {
                style::fill_rect(g, r, c(p.menu_selected_background));
                (c(p.menu_selected_item_text), c(p.menu_selected_item_text).opacity(0.75))
            } else {
                (c(p.panel_text), style::disabled_text(&p))
            };
            let fg = if item.enabled { fg } else { dim };
            // One middle on the device grid for the icon and the
            // words (a row can start between pixels at 1.25×).
            let mid = style::snap_coord(g, r.center().y);
            let icon_r = style::snap_centred(g, point(r.left() + px(8.0 + ICON / 2.0), mid), ICON, ICON);
            let drew = item.icon.as_deref().is_some_and(|i| if item.enabled { icons::paint(g, icon_r, i) } else { icons::paint_faded(g, icon_r, i, 110.0 / 255.0) });
            if !drew {
                icons::paint(g, icon_r, "application");
            }
            // The smaller words (subtitle, kind) stand on the
            // title's baseline.
            let detail_y = mid + px((style::PLAIN_SIZE - style::DETAIL_SIZE) * style::CAP_HEIGHT / 2.0);
            // Right side: the kind (on the subtitle's line), and keys
            // before it.
            let label = if is_selected && self.command && matches!(item.action, Action::Open(_)) { tracker::LABEL } else { item.kind.label() };
            let small = style::ui_font(FontWeight::NORMAL);
            let kind = style::label(g, r.right() - px(8.0), detail_y, style::Align::Right, label, style::DETAIL_SIZE, small.clone(), dim);
            let mut right = kind.left() - px(m::SPACING);
            if let Some(keys) = &item.keys {
                let w = style::key_combo_width(keys);
                style::key_combo(g, point(right, mid), keys, fg);
                right -= px(w + m::SPACING);
            }
            // Title, and the subtitle after it in grey.
            let x = icon_r.right() + px(10.0);
            let avail = (right - x).max(px(40.0));
            let title = style::label_fit(g, x, mid, style::Align::Left, item.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), fg, avail);
            let sx = title.right() + px(8.0);
            if !item.subtitle.is_empty() && right - sx > px(40.0) {
                style::label_fit(g, sx, detail_y, style::Align::Left, item.subtitle.clone(), style::DETAIL_SIZE, small, dim, right - sx);
            }
            // For agents: the title, and what else the row says.
            let said: Vec<&str> = [item.subtitle.as_str(), item.keys.as_deref().unwrap_or(""), label].into_iter().filter(|s| !s.is_empty()).collect();
            olo_kit::agent::record(
                g,
                olo_kit::agent::Node {
                    role: "Row",
                    name: Some(item.title.clone().into()),
                    value: Some(said.join(" · ").into()),
                    bounds: r,
                    enabled: item.enabled,
                    clickable: item.enabled,
                    selected: Some(is_selected),
                    ..Default::default()
                },
            );
        }
    }

    /// The result at `at` in the list, if one that can be chosen is.
    fn result_at(&self, at: Point<Pixels>, cx: &Cx<Self>) -> Option<usize> {
        let _ = cx;
        let n = Self::row_at(self.geo.list, at.y)?;
        let index = self.top + n;
        (n < ROWS && self.results.get(index).is_some_and(|i| i.enabled)).then_some(index)
    }
}

impl View for Palette {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Fixed(items) => self.fixed = items,
            Msg::Found(Found::Menus(items)) => self.menus = items,
            Msg::Found(Found::Files(n, items)) if n == self.asked => self.files = items,
            Msg::Found(Found::Passwords(n, items)) if n == self.asked => self.passwords = items,
            Msg::Found(_) => return,
            Msg::Asked(asked) => return self.asked(asked, cx),
            Msg::Copied(item) => self.copied = Some(item),
            Msg::Typed(n, item) => {
                let Some(item) = item.filter(|_| n == self.asked) else { return };
                self.typed = Some((n, item));
            }
        }
        let keep = self.results.get(self.selected).map(|i| i.action.clone());
        self.refresh();
        // Keep what was selected selected as more results come.
        if let Some(a) = keep
            && let Some(i) = self.results.iter().position(|r| r.action == a)
        {
            self.selected = i;
        }
        cx.invalidate();
    }

    /// (The clipboard's offer comes to the client with the keyboard, so
    /// it's read once the window has that.)
    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        if active && self.copied.is_none() {
            self.read_copied(cx);
        }
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.stop_asking();
        olo_kit::proxy().send(Closed);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = cx.viewport_size();
        let scale = cx.scale_factor();
        let shown = self.results.len().min(ROWS);
        let width = WIDTH.min(f32::from(screen.width) - 32.0);
        // An answer (/ask): as tall as it is, up to most of the screen
        // under the panel's top, its keys along the foot.
        let laid = self.answer.as_ref().map(|a| answer::lay_out(a, width - 2.0 * m::SPACING, (f32::from(screen.height) * (0.92 - TOP) - FIELD_HEIGHT - 4.0 * m::SPACING - answer::FOOT).max(120.0)));
        if let (Some(a), Some(l)) = (self.answer.as_mut(), &laid) {
            a.scroll = l.scroll;
        }
        let (list_h, height) = match &laid {
            Some(l) => (l.area, FIELD_HEIGHT + 3.0 * m::SPACING + l.area + m::SPACING + answer::FOOT),
            None => {
                let list_h = if self.query_text.trim().is_empty() && shown == 0 { 0.0 } else { shown as f32 * ROW + if shown > 0 { m::TIGHT * 2.0 } else { 0.0 } };
                (list_h, FIELD_HEIGHT + 2.0 * m::SPACING + list_h + if self.results.is_empty() { 0.0 } else { m::SPACING })
            }
        };
        let panel = snapped(scale, Bounds { origin: point((screen.width - px(width)) / 2.0, (screen.height * TOP).round()), size: size(px(width), px(height)) });
        let field = Bounds { origin: point(panel.left() + px(m::SPACING), panel.top() + px(m::SPACING)), size: size(px(width - 2.0 * m::SPACING), px(FIELD_HEIGHT)) };
        let glass = snapped_centred(scale, point(field.left() + px(8.0 + 12.0), field.center().y), 24.0, 24.0);
        let list = Bounds { origin: point(field.left(), field.bottom() + px(m::SPACING)), size: size(field.size.width, px(list_h)) };
        self.geo = Geo { panel, field, glass, list };

        // The panel, and the clicks around it (outside: put away).
        let backdrop = cx.canvas(|p: &Palette, g, _| p.paint_backdrop(g)).id("palette-backdrop").absolute().left(px(0.0)).top(px(0.0)).size_full().on_any_mouse_down(cx.listener(
            |p: &mut Self, e: &MouseDownEvent, cx| {
                if !p.geo.panel.contains(&e.position) {
                    p.hide(cx);
                }
            },
        ));

        // The query, its capitals on the field's middle as the glass is.
        // (Where the results' titles start: the glass is as wide as their
        // icons. On the device grid: from between two pixels, at 1.25×,
        // the field's edge cut the first letter's side.)
        let text_left = snap_at(scale, glass.right() + px(10.0));
        let line = px((QUERY_SIZE * style::LINE_HEIGHT).ceil());
        // After it, what a tap of Ctrl shows instead, there as it's typed.
        let other = if self.files_mode { "Commands" } else { "Files" };
        // (A little taller than its line, about the same middle: the field
        // sets its text by the capitals' middle, so the line's box rises
        // past the top of a field just its height, and its clip cut the
        // letters' tops at 1.25×.)
        let room = line + px(2.0 * 2.0);
        let query = div()
            .absolute()
            .left(text_left)
            .top(snap_at(scale, field.center().y - room / 2.0))
            .w(field.right() - px(m::TEXT_INSET + m::TIGHT) - text_left)
            .h(room)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(div().flex_1().h(room).child(&self.query))
            .child(KeyHint::new("Ctrl", other));

        let mut root = div().size_full().relative().child(backdrop);
        match (&self.answer, &laid) {
            (Some(a), Some(l)) => {
                let foot = Bounds { origin: point(field.left(), list.bottom() + px(m::SPACING)), size: size(field.size.width, px(answer::FOOT)) };
                root = root.child(answer::view(a, l, list, cx)).child(answer::foot(a, foot));
            }
            // The results (none while an answer shows): the pointer moving
            // over a row selects it; a click chooses (with ⌘, shows a file
            // in Tracker); the wheel scrolls.
            _ if list_h > 0.0 => {
                let results = cx
                    .canvas(|p: &Palette, g, _| p.paint_results(g, p.geo.list))
                    .id("palette-results")
                    .absolute()
                    .left(list.left())
                    .top(list.top())
                    .w(list.size.width)
                    .h(list.size.height)
                    .moves_matter_at_edges()
                    .on_mouse_move(cx.listener(|p: &mut Self, e: &MouseMoveEvent, cx| {
                        if let Some(index) = p.result_at(e.position, cx).filter(|i| *i != p.selected) {
                            p.selected = index;
                            cx.invalidate();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|p: &mut Self, e: &MouseDownEvent, cx| {
                            let Some(n) = Self::row_at(p.geo.list, e.position.y).filter(|n| p.top + *n < p.results.len() && *n < ROWS) else { return };
                            let index = p.top + n;
                            if e.modifiers.platform || e.modifiers.control {
                                p.reveal(index, cx)
                            } else {
                                p.choose(index, cx)
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|p: &mut Self, e: &ScrollWheelEvent, cx| {
                        let dy = f32::from(e.delta.pixel_delta(px(ROW)).y);
                        if dy.abs() < 0.5 {
                            return;
                        }
                        let max_top = p.results.len().saturating_sub(ROWS);
                        let rows = (-dy / ROW).round() as isize;
                        p.top = (p.top as isize + rows.signum() * rows.abs().max(1)).clamp(0, max_top as isize) as usize;
                        cx.invalidate();
                    }));
                root = root.child(results);
            }
            _ => {}
        }
        root.child(query)
            .capture_key_down(cx.listener(|p: &mut Self, e: &KeyDownEvent, cx| {
                if p.key(e, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_modifiers_changed(cx.listener(|p: &mut Self, e: &ModifiersChangedEvent, cx| p.modifiers(e, cx)))
            .into_element()
    }
}

/// The query's placeholder: what it finds (the key hint after it says
/// what a tap of Ctrl finds instead).
fn hint(files: bool) -> &'static str {
    if files { "Search files and folders" } else { "Search apps, commands and shortcuts" }
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // What the ranking model knows, for a terminal or an agent.
    if args.first().is_some_and(|a| a == "--model") {
        println!("{}", serde_json::to_string_pretty(&model::Ranker::load().describe()).unwrap_or_default());
        return;
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle");
    let shortcuts = args.iter().any(|a| a == "--shortcuts");
    let ask = window_menu::Ask::parse(&args);
    olo_kit::run(APP_ID, move |app| {
        olo_kit::stay_resident();
        instance::listen(APP_ID, olo_kit::proxy());
        let proxy = olo_kit::proxy();
        let _ = std::thread::Builder::new().name("palette-fixed".into()).spawn(move || {
            proxy.send(Found::Fixed(sources::fixed()));
            // Applications installed or removed: listed again at once.
            if let Some(w) = olo_kit::watch::watch(olo_xdg::desktop_entry::app_dirs(), move |_| proxy.send(Found::Fixed(sources::fixed()))) {
                std::mem::forget(w);
            }
        });
        let shared = Shared {
            fixed: Arc::default(),
            recents: Arc::new(Mutex::new(Recents::load())),
            ranker: Arc::new(Mutex::new(model::Ranker::load())),
            tuning: Arc::default(),
            workers: sources::workers(olo_kit::proxy()),
        };
        let hub = Rc::new(RefCell::new(Hub { shared, win: None, opening: false, window_menu: None, sheet: None }));
        let messages = hub.clone();
        olo_kit::message_handler(app, move |message, _| messages.borrow_mut().message(message));
        // Nothing to keep but that this is a new version of a running
        // copy: it re-executes with the arguments it first started with
        // (`--toggle`, from the first ⌘Space), which must not open it
        // again by themselves.
        olo_kit::on_upgrade(|_| Some("{}".into()));
        if olo_kit::restored_state().is_none() {
            let mut hub = hub.borrow_mut();
            if let Some(ask) = ask {
                hub.ask_window_menu(ask);
            }
            if toggle {
                hub.toggle();
            }
            if shortcuts {
                hub.toggle_sheet();
            }
        }
    });
}
