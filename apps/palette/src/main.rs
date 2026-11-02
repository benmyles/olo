//! olo-palette: the command palette. ⌘Space (or a tap of ⌘, if chosen
//! in Preferences ▸ Keyboard) brings it up over everything; typing finds
//! applications, the front app's menu commands, open windows, the
//! desktop's shortcuts (with their keys, so it doubles as a reminder),
//! preference panels, settings, Passwords' items and files, and works out
//! sums. Return does the selected one; Escape, a click outside or ⌘Space
//! again puts it away. On the GPUI kit (olo-ui).
//!
//! It stays running between uses so it opens at once: app_server starts
//! `olo-palette --toggle`, which hands the toggle to the running copy.
//! It shows a window's menu (a right-click on its tab) the same way, from
//! `--window-menu` (window_menu.rs), and the shortcuts sheet (⇧⌘/) from
//! `--shortcuts` (sheet.rs).

mod calc;
mod rank;
mod sheet;
mod sources;
mod window_menu;

use std::{collections::HashMap, path::PathBuf};

use olo_protocol::WindowInfo;
use olo_ui::{
    Frame, WindowOptions,
    gpui::{
        App, AppContext as _, Bounds, ClipboardItem, Context, Entity, Focusable as _, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent, LayerKeyboard, LayerShellLayer, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollWheelEvent, Styled as _, Subscription, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    theme, tracker,
    widgets::{TextField, TextFieldEvent, text_field},
};
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
        let read = |p: PathBuf| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Recents>(&s).ok());
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
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(path, s);
        }
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
        (count.min(20) as i32 * 6) + if days < 1 { 40 } else if days < 7 { 20 } else { 0 }
    }
}

struct Palette {
    win: Option<WindowHandle<Frame>>,
    /// Asked to show; opens once the window in front is known.
    opening: bool,
    query: Entity<TextField>,
    /// The query's text, as last seen.
    query_text: String,
    /// Bumped with each change to the query, so late answers are dropped.
    asked: u64,
    selected: usize,
    /// Scrolled this many rows down.
    top: usize,
    /// ⌘ held: a file's row says Open in Tracker.
    command: bool,
    fixed: Vec<Item>,
    context: Vec<Item>,
    menus: Vec<Item>,
    files: Vec<Item>,
    passwords: Vec<Item>,
    /// Go to the path in the clipboard, if it holds one that's there.
    copied: Option<Item>,
    /// Go to the path typed, for the query numbered so.
    typed: Option<(u64, Item)>,
    /// Following the window's activation: the clipboard is read once it
    /// has the keyboard.
    activation: Option<Subscription>,
    results: Vec<Item>,
    front: Option<WindowInfo>,
    recents: Recents,
    workers: Workers,
    /// A window's menu, while it shows.
    window_menu: Option<WindowHandle<Frame>>,
    /// The shortcuts sheet, while it shows.
    sheet: Option<WindowHandle<Frame>>,
}

/// Closes `w`.
fn close(w: WindowHandle<Frame>, cx: &mut App) {
    cx.close_window(w);
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

impl Palette {
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.win.is_some() {
            return self.hide(cx);
        }
        if self.opening {
            return;
        }
        self.opening = true;
        // Learn which window is in front before taking the keyboard from it.
        let proxy = olo_ui::proxy(cx);
        let _ = std::thread::Builder::new().name("palette-context".into()).spawn(move || {
            let (front, items) = sources::context();
            proxy.send(Found::Context { front: front.clone(), items });
            if let Some(f) = front {
                proxy.send(Found::Menus(sources::menus(&f)));
            }
        });
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        self.opening = false;
        // (Setting the field's text sends no change.)
        self.query.update(cx, |f, cx| f.set_text("", cx));
        self.query_text.clear();
        self.asked += 1;
        self.files.clear();
        self.passwords.clear();
        self.copied = None;
        self.typed = None;
        self.selected = 0;
        self.top = 0;
        self.command = false;
        self.refresh();
        let me = cx.entity();
        let query = self.query.clone();
        cx.defer(move |cx| {
            let view = me.clone();
            let w = olo_ui::open_window(cx, overlay("Command Palette", "palette", None), move |window, cx| {
                window.focus(&query.focus_handle(cx));
                // (The clipboard's offer comes to the client with the
                // keyboard, so it's read once the window has that.)
                view.update(cx, |p, cx| {
                    p.activation = Some(cx.observe_window_activation(window, |p, window, cx| {
                        if window.is_window_active() && p.copied.is_none() {
                            p.read_copied(cx);
                        }
                    }));
                });
                view
            });
            me.update(cx, |p, _| p.win = w);
        });
    }

    /// Offer to go to the path in the clipboard, if it holds one that's
    /// there: read and looked for off the UI thread (a program slow to
    /// give what it copied, a slow disk, never hold the palette up).
    fn read_copied(&mut self, cx: &mut Context<Self>) {
        // (A turn of the event loop first: the keyboard's arrival and the
        // clipboard's offer come together, the offer after.)
        cx.spawn(async move |this, cx| {
            let Ok(read) = this.update(cx, |_, cx| cx.paste_as(&sources::TEXT)) else { return };
            let Some((_, bytes)) = read.await else { return };
            let found = cx.background_spawn(async move { sources::go_to(std::str::from_utf8(&bytes).ok()?, true) }).await;
            let _ = this.update(cx, |p, cx| {
                if found.is_some() && p.win.is_some() {
                    p.copied = found;
                    p.refresh();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.win.take() {
            cx.defer(move |cx| close(w, cx));
        }
        self.menus.clear();
        self.front = None;
        cx.notify();
    }

    /// The results for the query now.
    fn refresh(&mut self) {
        let query = self.query_text.clone();
        let words = rank::words(&query);
        let mut scored: Vec<(i32, Item)> = Vec::new();
        if words.is_empty() {
            // Nothing typed: what was chosen lately.
            let all = self.fixed.iter().chain(&self.context);
            let mut recent: Vec<(i32, Item)> = all.filter_map(|i| i.action.key().map(|k| (self.recents.boost(&k), i.clone()))).filter(|(b, _)| *b > 0).collect();
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
            let all = self.fixed.iter().chain(&self.context).chain(&self.menus).chain(&self.passwords).chain(&self.files).chain(&self.copied);
            scored.extend(sources::ranked(&words, all, |k| self.recents.boost(k)));
            let q = query.trim().to_owned();
            scored.push((i32::MIN, Item::new(Kind::Web, format!("Search the web for “{q}”"), Action::Search(q)).icon("browser").subtitle("Web browser")));
        }
        self.results = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        if !self.results.get(self.selected).is_some_and(|i| i.enabled) {
            self.selected = self.results.iter().position(|i| i.enabled).unwrap_or(0);
        }
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).text().to_owned();
        if text == self.query_text {
            return;
        }
        self.query_text = text;
        self.asked += 1;
        self.selected = 0;
        self.top = 0;
        let words = rank::words(&self.query_text);
        let _ = self.workers.files.send((self.asked, words));
        let _ = self.workers.passwords.send((self.asked, self.query_text.clone()));
        // A path written whole: looked for off the UI thread, offered if
        // it's there.
        if olo_ui::files::written_path(&self.query_text).is_some() {
            let (text, asked) = (self.query_text.clone(), self.asked);
            cx.spawn(async move |this, cx| {
                let Some(item) = cx.background_spawn(async move { sources::go_to(&text, false) }).await else { return };
                let _ = this.update(cx, |p, cx| {
                    if p.asked == asked {
                        p.typed = Some((asked, item));
                        p.refresh();
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        self.refresh();
        cx.notify();
    }

    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.results.get(index).cloned() else { return };
        if !item.enabled {
            return;
        }
        if let Some(k) = item.action.key() {
            self.recents.note(k);
        }
        if let Action::Copy(text) = &item.action {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        let front = self.front.as_ref().map(|f| f.id);
        self.hide(cx);
        sources::perform(item.action, front);
    }

    /// ⌘↩ or ⌘-click: a file or folder found shown in Tracker rather than
    /// opened; anything else chosen as usual.
    fn reveal(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(Item { action: Action::Open(path), enabled: true, .. }) = self.results.get(index).cloned() else {
            return self.choose(index, cx);
        };
        if let Some(k) = Action::Open(path.clone()).key() {
            self.recents.note(k);
        }
        self.hide(cx);
        if let Err(e) = tracker::open(&path) {
            tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
        }
    }

    /// The shortcuts sheet, or away with it if it shows.
    fn toggle_sheet(&mut self, cx: &mut Context<Self>) {
        // (Unless it put itself away: Escape, a click outside.)
        if let Some(old) = self.sheet.take()
            && cx.windows().iter().any(|w| w.window_id() == old.window_id())
        {
            cx.defer(move |cx| close(old, cx));
            return;
        }
        if self.win.is_some() {
            self.hide(cx);
        }
        sheet::learn(olo_ui::proxy(cx));
    }

    /// A right-click on a window's tab: put the palette away and learn
    /// what the menu needs.
    fn ask_window_menu(&mut self, ask: window_menu::Ask, cx: &mut Context<Self>) {
        if self.win.is_some() {
            self.hide(cx);
        }
        if let Some(old) = self.window_menu.take() {
            cx.defer(move |cx| close(old, cx));
        }
        window_menu::learn(ask, olo_ui::proxy(cx));
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
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        if olo_kit_base::keys::shortcut("alternate").is_some_and(|s| olo_ui::keys::pressed(&s, k)) {
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
        cx.notify();
        true
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) if launch.args.iter().any(|a| a == "--toggle") => return self.toggle(cx),
            Ok(launch) if launch.args.iter().any(|a| a == "--shortcuts") => return self.toggle_sheet(cx),
            Ok(launch) => {
                if let Some(ask) = window_menu::Ask::parse(&launch.args) {
                    self.ask_window_menu(ask, cx);
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<sheet::Learned>() {
            Ok(learned) => {
                if let Some(old) = self.sheet.take() {
                    cx.defer(move |cx| close(old, cx));
                }
                let me = cx.entity();
                cx.defer(move |cx| {
                    let w = sheet::open(*learned, cx);
                    me.update(cx, |p, _| p.sheet = w);
                });
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<window_menu::Facts>() {
            Ok(facts) => {
                if let Some(old) = self.window_menu.take() {
                    cx.defer(move |cx| close(old, cx));
                }
                let me = cx.entity();
                cx.defer(move |cx| {
                    let w = window_menu::open(*facts, cx);
                    me.update(cx, |p, _| p.window_menu = w);
                });
                return;
            }
            Err(m) => m,
        };
        let Ok(found) = message.downcast::<Found>() else { return };
        match *found {
            Found::Fixed(items) => self.fixed = items,
            Found::Context { front, items } => {
                self.front = front;
                self.context = items;
                if self.opening {
                    self.show(cx);
                }
            }
            Found::Menus(items) => self.menus = items,
            Found::Files(n, items) if n == self.asked => self.files = items,
            Found::Passwords(n, items) if n == self.asked => self.passwords = items,
            Found::Files(..) | Found::Passwords(..) => return,
        }
        if self.win.is_some() {
            let keep = self.results.get(self.selected).map(|i| i.action.clone());
            self.refresh();
            // Keep what was selected selected as more results come.
            if let Some(a) = keep
                && let Some(i) = self.results.iter().position(|r| r.action == a)
            {
                self.selected = i;
            }
            cx.notify();
        }
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let screen = window.viewport_size();
        let shown = self.results.len().min(ROWS);
        let list_h = if self.query_text.trim().is_empty() && shown == 0 { 0.0 } else { shown as f32 * ROW + if shown > 0 { m::TIGHT * 2.0 } else { 0.0 } };
        let width = WIDTH.min(f32::from(screen.width) - 32.0);
        let height = FIELD_HEIGHT + 2.0 * m::SPACING + list_h + if self.results.is_empty() { 0.0 } else { m::SPACING };
        let panel = style::snap(window, Bounds { origin: point((screen.width - px(width)) / 2.0, (screen.height * TOP).round()), size: size(px(width), px(height)) });
        let field = Bounds { origin: point(panel.left() + px(m::SPACING), panel.top() + px(m::SPACING)), size: size(px(width - 2.0 * m::SPACING), px(FIELD_HEIGHT)) };
        let glass = style::snap_centred(window, point(field.left() + px(8.0 + 12.0), field.center().y), 24.0, 24.0);
        let list = Bounds { origin: point(field.left(), field.bottom() + px(m::SPACING)), size: size(field.size.width, px(list_h)) };

        // The panel, and the clicks around it (outside: put away).
        let backdrop_me = me.clone();
        let backdrop = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                // A soft shadow, then the BeOS panel.
                style::soft_shadow(window, panel, &p);
                style::fill_rect(window, panel, c(p.panel_background));
                style::frame_rect(window, panel, style::tint(p.panel_background, 1.6));
                style::raised(window, style::inset(window, panel, 1.0), &p);
                // The query's well, and the glass.
                style::fill_rect(window, field, c(p.document_background));
                style::sunken(window, field, &p, true);
                icons::paint(window, glass, "query");
                let m2 = backdrop_me.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && hitbox.is_hovered(window) && !panel.contains(&e.position) {
                        m2.update(cx, |p, cx| p.hide(cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        // The query, its capitals on the field's middle as the glass is.
        // (Where the results' titles start: the glass is as wide as their
        // icons.)
        let text_left = glass.right() + px(10.0);
        let line = px((QUERY_SIZE * style::LINE_HEIGHT).ceil());
        let query = div()
            .absolute()
            .left(text_left)
            .top(field.center().y - line / 2.0)
            .w(field.right() - px(8.0) - text_left)
            .h(line)
            .child(self.query.clone());

        // The results.
        let rows: Vec<(usize, Item)> = (self.top..self.results.len()).take(ROWS).map(|i| (i, self.results[i].clone())).collect();
        let (selected, command) = (self.selected, self.command);
        let rows_me = me.clone();
        let total = self.results.len();
        let results = canvas(
            move |_, window, _| window.insert_hitbox(list, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                let row_at = move |y: Pixels| {
                    let k = (f32::from(y - list.top()) - m::TIGHT) / ROW;
                    (k >= 0.0).then_some(k as usize)
                };
                for (n, (index, item)) in rows.iter().enumerate() {
                    let r = Bounds { origin: point(list.left(), list.top() + px(m::TIGHT + n as f32 * ROW)), size: size(list.size.width, px(ROW)) };
                    let is_selected = *index == selected;
                    let (fg, dim) = if is_selected {
                        style::fill_rect(window, r, c(p.menu_selected_background));
                        (c(p.menu_selected_item_text), c(p.menu_selected_item_text).opacity(0.75))
                    } else {
                        (c(p.panel_text), style::disabled_text(&p))
                    };
                    let fg = if item.enabled { fg } else { dim };
                    // One middle on the device grid for the icon and the
                    // words (a row can start between pixels at 1.25×).
                    let mid = style::snap_coord(window, r.center().y);
                    let icon_r = style::snap_centred(window, point(r.left() + px(8.0 + ICON / 2.0), mid), ICON, ICON);
                    let drew = item.icon.as_deref().is_some_and(|i| if item.enabled { icons::paint(window, icon_r, i) } else { icons::paint_faded(window, icon_r, i, 110.0 / 255.0) });
                    if !drew {
                        icons::paint(window, icon_r, "application");
                    }
                    // The smaller words (subtitle, kind) stand on the
                    // title's baseline.
                    let detail_y = mid + px((style::PLAIN_SIZE - style::DETAIL_SIZE) * style::CAP_HEIGHT / 2.0);
                    // Right side: the kind (on the subtitle's line), and keys
                    // before it.
                    let label = if is_selected && command && matches!(item.action, Action::Open(_)) { tracker::LABEL } else { item.kind.label() };
                    let small = style::ui_font(FontWeight::NORMAL);
                    let kind = style::label(window, cx, r.right() - px(8.0), detail_y, style::Align::Right, label, style::DETAIL_SIZE, small.clone(), dim);
                    let mut right = kind.left() - px(m::SPACING);
                    if let Some(keys) = &item.keys {
                        let w = style::key_combo_width(window, keys);
                        style::key_combo(window, cx, point(right, mid), keys, fg);
                        right -= px(w + m::SPACING);
                    }
                    // Title, and the subtitle after it in grey.
                    let x = icon_r.right() + px(10.0);
                    let avail = (right - x).max(px(40.0));
                    let title = style::label_fit(window, cx, x, mid, style::Align::Left, item.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), fg, avail);
                    let sx = title.right() + px(8.0);
                    if !item.subtitle.is_empty() && right - sx > px(40.0) {
                        style::label_fit(window, cx, sx, detail_y, style::Align::Left, item.subtitle.clone(), style::DETAIL_SIZE, small, dim, right - sx);
                    }
                    // For agents: the title, and what else the row says.
                    let said: Vec<&str> = [item.subtitle.as_str(), item.keys.as_deref().unwrap_or(""), label].into_iter().filter(|s| !s.is_empty()).collect();
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Row", name: Some(item.title.clone().into()), value: Some(said.join(" · ").into()), bounds: r, enabled: item.enabled, clickable: item.enabled, selected: Some(is_selected), ..Default::default() });
                }
                // The pointer moving over a row selects it; a click
                // chooses (with ⌘, shows a file in Tracker); the wheel
                // scrolls.
                let first = rows.first().map_or(0, |r| r.0);
                let enabled: Vec<bool> = rows.iter().map(|r| r.1.enabled).collect();
                let (m1, hb) = (rows_me.clone(), hitbox.clone());
                let en = enabled.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase.bubble()
                        && hb.is_hovered(window)
                        && let Some(n) = row_at(e.position.y).filter(|n| en.get(*n).copied().unwrap_or(false))
                        && first + n != selected
                    {
                        m1.update(cx, |p, cx| {
                            p.selected = first + n;
                            cx.notify();
                        });
                    }
                });
                let (m2, hb) = (rows_me.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && e.button == MouseButton::Left
                        && hb.is_hovered(window)
                        && let Some(n) = row_at(e.position.y).filter(|n| *n < enabled.len())
                    {
                        let cmd = e.modifiers.platform || e.modifiers.control;
                        m2.update(cx, |p, cx| if cmd { p.reveal(first + n, cx) } else { p.choose(first + n, cx) });
                    }
                });
                let (m3, hb) = (rows_me.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if !phase.bubble() || !hb.is_hovered(window) {
                        return;
                    }
                    let dy = f32::from(e.delta.pixel_delta(px(ROW)).y);
                    if dy.abs() < 0.5 {
                        return;
                    }
                    m3.update(cx, |p, cx| {
                        let max_top = total.saturating_sub(ROWS);
                        let rows = (-dy / ROW).round() as isize;
                        p.top = (p.top as isize + rows.signum() * rows.abs().max(1)).clamp(0, max_top as isize) as usize;
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .inset_0();

        let keys = me.clone();
        let mods = me.clone();
        div()
            .size_full()
            .relative()
            .child(backdrop)
            .child(results)
            .child(query)
            .capture_key_down(move |e: &KeyDownEvent, _, cx| {
                if keys.update(cx, |p, cx| p.key(e, cx)) {
                    cx.stop_propagation();
                }
            })
            .on_modifiers_changed(move |e: &ModifiersChangedEvent, _, cx| {
                let held = e.modifiers.platform || e.modifiers.control;
                mods.update(cx, |p, cx| {
                    if p.command != held {
                        p.command = held;
                        cx.notify();
                    }
                });
            })
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle");
    let shortcuts = args.iter().any(|a| a == "--shortcuts");
    let ask = window_menu::Ask::parse(&args);
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let proxy = olo_ui::proxy(cx);
        let _ = std::thread::Builder::new().name("palette-fixed".into()).spawn(move || {
            proxy.send(Found::Fixed(sources::fixed()));
            // Applications installed or removed: listed again at once.
            if let Some(w) = olo_ui::watch::watch(olo_xdg::desktop_entry::app_dirs(), move |_| proxy.send(Found::Fixed(sources::fixed()))) {
                std::mem::forget(w);
            }
        });
        let query = text_field(cx, |f| f.name("Search").font_size(QUERY_SIZE).hint("Search apps, commands, files and more"));
        let workers = sources::workers(olo_ui::proxy(cx));
        let palette = cx.new(|cx| {
            cx.subscribe(&query, |p: &mut Palette, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => {
                    let i = p.selected;
                    p.choose(i, cx);
                }
                _ => p.query_changed(cx),
            })
            .detach();
            Palette {
                win: None,
                opening: false,
                query,
                query_text: String::new(),
                asked: 0,
                selected: 0,
                top: 0,
                command: false,
                fixed: Vec::new(),
                context: Vec::new(),
                menus: Vec::new(),
                files: Vec::new(),
                passwords: Vec::new(),
                copied: None,
                typed: None,
                activation: None,
                results: Vec::new(),
                front: None,
                recents: Recents::load(),
                workers,
                window_menu: None,
                sheet: None,
            }
        });
        let messages = palette.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |p, cx| p.message(message, cx)));
        // Nothing to keep but that this is a new version of a running
        // copy: it re-executes with the arguments it first started with
        // (`--toggle`, from the first ⌘Space), which must not open it
        // again by themselves.
        olo_ui::on_upgrade(cx, |_| Some("{}".into()));
        if olo_ui::restored_state().is_none() {
            palette.update(cx, |p, cx| {
                if let Some(ask) = ask {
                    p.ask_window_menu(ask, cx);
                }
                if toggle {
                    p.toggle(cx);
                }
                if shortcuts {
                    p.toggle_sheet(cx);
                }
            });
        }
    });
}
