//! olo-palette: the command palette. ⌘Space (or a tap of ⌘, if chosen
//! in Preferences ▸ Keyboard) brings it up over everything; typing finds
//! applications, the front app's menu commands, open windows, the
//! desktop's shortcuts (with their keys, so it doubles as a reminder),
//! preference panels, settings, Passwords' items and files, and works out
//! sums. Return does the selected one; Escape, a click outside or ⌘Space
//! again puts it away.
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

use serde::{Deserialize, Serialize};
use sources::{Action, Found, Item, Kind, Workers};
use olo_kit::{
    App, Cx, Keyboard, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Align, Color32, Key, Rect, Sense, pos2, vec2},
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
};
use olo_protocol::WindowInfo;

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
        olo_kit::config_dir().join("palette.json")
    }
    fn load() -> Recents {
        let read = |p: PathBuf| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Recents>(&s).ok());
        // Before the rename (Tabula) it was kept a folder deeper, and named
        // our programs by their old names.
        read(Self::path())
            .or_else(|| {
                let mut old = read(olo_kit::config_dir().join(olo_kit::legacy::OLD).join("palette.json"))?;
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
    win: Option<WindowId>,
    /// Asked to show; opens once the window in front is known.
    opening: bool,
    query: String,
    /// Bumped with each change to the query, so late answers are dropped.
    asked: u64,
    selected: usize,
    /// Scrolled this many rows down.
    top: usize,
    focus_field: bool,
    fixed: Vec<Item>,
    context: Vec<Item>,
    menus: Vec<Item>,
    files: Vec<Item>,
    passwords: Vec<Item>,
    results: Vec<Item>,
    front: Option<WindowInfo>,
    recents: Recents,
    workers: Workers,
    /// A window's menu, while it shows.
    window_menu: Option<window_menu::Shown>,
    /// The shortcuts sheet, while it shows.
    sheet: Option<sheet::Shown>,
}

impl Palette {
    fn toggle(&mut self, cx: &mut Cx) {
        if self.win.is_some() {
            self.hide(cx);
            return;
        }
        if self.opening {
            return;
        }
        self.opening = true;
        // Learn which window is in front before taking the keyboard from it.
        let proxy = cx.proxy();
        let _ = std::thread::Builder::new().name("palette-context".into()).spawn(move || {
            let (front, items) = sources::context();
            proxy.send(Found::Context { front: front.clone(), items });
            if let Some(f) = front {
                proxy.send(Found::Menus(sources::menus(&f)));
            }
        });
    }

    fn show(&mut self, cx: &mut Cx) {
        self.opening = false;
        self.query.clear();
        self.files.clear();
        self.passwords.clear();
        self.selected = 0;
        self.top = 0;
        self.focus_field = true;
        let win = cx.open_window(WindowOptions {
            title: "Command Palette".into(),
            size: (1, 1),
            kind: WindowKind::Layer(LayerOptions {
                layer: Layer::Overlay,
                anchor: (true, true, true, true),
                exclusive_zone: -1,
                margin: (0, 0, 0, 0),
                keyboard: Keyboard::Exclusive,
                namespace: "palette".into(),
                output: None,
            }),
            panel: false,
            ..Default::default()
        });
        self.win = Some(win);
        self.refresh();
    }

    fn hide(&mut self, cx: &mut Cx) {
        if let Some(w) = self.win.take() {
            cx.close_window(w);
        }
        self.menus.clear();
        self.front = None;
    }

    /// The results for the query now.
    fn refresh(&mut self) {
        let words = rank::words(&self.query);
        let mut scored: Vec<(i32, Item)> = Vec::new();
        if words.is_empty() {
            // Nothing typed: what was chosen lately.
            let all = self.fixed.iter().chain(&self.context);
            let mut recent: Vec<(i32, Item)> =
                all.filter_map(|i| i.action.key().map(|k| (self.recents.boost(&k), i.clone()))).filter(|(b, _)| *b > 0).collect();
            recent.sort_by_key(|r| std::cmp::Reverse(r.0));
            scored = recent.into_iter().take(ROWS).collect();
        } else {
            if let Some(v) = calc::eval(&self.query) {
                let shown = calc::show(v);
                let mut item = Item::new(Kind::Calculator, format!("= {shown}"), Action::Copy(shown)).subtitle(format!("{} — Return copies it", self.query.trim())).icon("calculator");
                item.keywords = String::new();
                scored.push((i32::MAX / 2, item));
            }
            let all = self.fixed.iter().chain(&self.context).chain(&self.menus).chain(&self.passwords).chain(&self.files);
            scored.extend(sources::ranked(&words, all, |k| self.recents.boost(k)));
            let q = self.query.trim().to_owned();
            scored.push((i32::MIN, Item::new(Kind::Web, format!("Search the web for “{q}”"), Action::Search(q)).icon("browser").subtitle("Web browser")));
        }
        self.results = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        if !self.results.get(self.selected).is_some_and(|i| i.enabled) {
            self.selected = self.results.iter().position(|i| i.enabled).unwrap_or(0);
        }
    }

    fn query_changed(&mut self) {
        self.asked += 1;
        self.selected = 0;
        self.top = 0;
        let words = rank::words(&self.query);
        let _ = self.workers.files.send((self.asked, words));
        let _ = self.workers.passwords.send((self.asked, self.query.clone()));
        self.refresh();
    }

    fn choose(&mut self, cx: &mut Cx, index: usize) {
        let Some(item) = self.results.get(index).cloned() else { return };
        if !item.enabled {
            return;
        }
        if let Some(k) = item.action.key() {
            self.recents.note(k);
        }
        if let Action::Copy(text) = &item.action {
            cx.copy(text.clone());
        }
        let front = self.front.as_ref().map(|f| f.id);
        self.hide(cx);
        sources::perform(item.action, front);
    }

    /// ⌘↩ or ⌘-click: a file or folder found shown in Tracker rather than
    /// opened; anything else chosen as usual.
    fn reveal(&mut self, cx: &mut Cx, index: usize) {
        let Some(Item { action: Action::Open(path), enabled: true, .. }) = self.results.get(index).cloned() else {
            return self.choose(cx, index);
        };
        if let Some(k) = Action::Open(path.clone()).key() {
            self.recents.note(k);
        }
        self.hide(cx);
        if let Err(e) = olo_kit::tracker::open(&path) {
            tracing::warn!("couldn't show {} in Tracker: {e}", path.display());
        }
    }

    /// The shortcuts sheet, or away with it if it shows.
    fn toggle_sheet(&mut self, cx: &mut Cx) {
        if let Some(old) = self.sheet.take() {
            cx.close_window(old.win);
            return;
        }
        if self.win.is_some() {
            self.hide(cx);
        }
        sheet::learn(cx.proxy());
    }

    /// A right-click on a window's tab: put the palette away and learn
    /// what the menu needs.
    fn ask_window_menu(&mut self, cx: &mut Cx, ask: window_menu::Ask) {
        if self.win.is_some() {
            self.hide(cx);
        }
        if let Some(old) = self.window_menu.take() {
            cx.close_window(old.win);
        }
        window_menu::learn(ask, cx.proxy());
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
}

impl App for Palette {
    fn ui(&mut self, cx: &mut Cx, window: olo_kit::WindowId, ui: &mut egui::Ui) {
        if let Some(shown) = self.sheet.as_mut().filter(|s| s.win == window) {
            if sheet::ui(shown, ui) {
                cx.close_window(window);
                self.sheet = None;
            }
            return;
        }
        if let Some(shown) = self.window_menu.as_mut().filter(|s| s.win == window) {
            if window_menu::ui(shown, ui) {
                cx.close_window(window);
                self.window_menu = None;
            }
            return;
        }
        let p = style::palette(ui.ctx());
        let screen = ui.max_rect();
        let shown = self.results.len().min(ROWS);
        let list_h = if self.query.trim().is_empty() && shown == 0 { 0.0 } else { shown as f32 * ROW + if shown > 0 { m::TIGHT * 2.0 } else { 0.0 } };
        let width = WIDTH.min(screen.width() - 32.0);
        let height = FIELD_HEIGHT + 2.0 * m::SPACING + list_h + if self.results.is_empty() { 0.0 } else { m::SPACING };
        let panel = Rect::from_min_size(pos2(screen.center().x - width / 2.0, screen.top() + (screen.height() * TOP).round()), vec2(width, height));
        let panel = style::snap(ui.ctx(), panel);

        // Outside the panel: a click puts it away.
        let outside = ui.interact(screen, ui.make_persistent_id("outside"), Sense::click());
        if outside.clicked() && !outside.interact_pointer_pos().is_some_and(|pos| panel.contains(pos)) {
            self.hide(cx);
            return;
        }

        let painter = ui.painter().clone();
        // A soft shadow, then the BeOS panel.
        style::soft_shadow(&painter, panel, &p);
        painter.rect_filled(panel, 0.0, c(p.panel_background));
        style::frame_rect(&painter, panel, style::darken(p.panel_background, 1.6));
        style::raised(&painter, panel.shrink(1.0), &p);

        // The query.
        let field = Rect::from_min_size(panel.min + vec2(m::SPACING, m::SPACING), vec2(width - 2.0 * m::SPACING, FIELD_HEIGHT));
        painter.rect_filled(field, 0.0, c(p.document_background));
        style::sunken(&painter, field, &p, true);
        let glass = Rect::from_center_size(pos2(field.left() + 8.0 + 12.0, field.center().y), vec2(24.0, 24.0));
        icons::paint(&painter, glass, "query");
        let line = ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(QUERY_SIZE)));
        // The capitals on the field's middle, as the glass is, on device
        // pixels (`style::text_origin`, from where a row of the type puts
        // them).
        let probe = painter.layout_no_wrap("H".into(), egui::FontId::proportional(QUERY_SIZE), Color32::WHITE);
        let top = style::text_origin(ui.ctx(), &probe, 0.0, field.center().y, QUERY_SIZE).y;
        let text_rect = Rect::from_min_max(pos2(glass.right() + 8.0, top), pos2(field.right() - 8.0, top + line));
        // Arrows and Return belong to the list, not the text.
        let (up, down, enter, reveal, escape, page_up, page_down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                olo_kit::keyboard::consume(i, "alternate"),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.consume_key(egui::Modifiers::NONE, Key::PageUp),
                i.consume_key(egui::Modifiers::NONE, Key::PageDown),
            )
        });
        if escape {
            self.hide(cx);
            return;
        }
        if up {
            self.step(-1);
        }
        if down {
            self.step(1);
        }
        if page_up {
            self.step(-(ROWS as isize));
        }
        if page_down {
            self.step(ROWS as isize);
        }
        if enter {
            let i = self.selected;
            self.choose(cx, i);
            return;
        }
        if reveal {
            let i = self.selected;
            self.reveal(cx, i);
            return;
        }
        let edit_id = ui.make_persistent_id("query");
        let before = self.query.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect));
        let out = egui::TextEdit::singleline(&mut self.query)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .font(egui::FontId::proportional(QUERY_SIZE))
            .text_color(c(p.document_text))
            .hint_text(egui::RichText::new("Search apps, commands, files and more").font(egui::FontId::proportional(QUERY_SIZE)).color(style::disabled_text(&p)))
            .desired_width(text_rect.width())
            .min_size(text_rect.size())
            .show(&mut child);
        if std::mem::take(&mut self.focus_field) || !out.response.has_focus() {
            out.response.request_focus();
        }
        if self.query != before {
            self.query_changed();
        }

        // The results.
        if !self.results.is_empty() {
            let list = Rect::from_min_size(pos2(field.left(), field.bottom() + m::SPACING), vec2(field.width(), list_h));
            if ui.rect_contains_pointer(list) {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.5 {
                    let max_top = self.results.len().saturating_sub(ROWS);
                    let rows = (-scroll / ROW).round() as isize;
                    self.top = (self.top as isize + rows.signum() * rows.abs().max(1)).clamp(0, max_top as isize) as usize;
                }
            }
            let mut chosen = None;
            // ⌘ held: a file's ⌘↩ (or ⌘-click) shows it in Tracker, and
            // its row says so.
            let command = ui.input(|i| i.modifiers.command);
            for (row, index) in (self.top..self.results.len()).take(ROWS).enumerate() {
                let item = &self.results[index];
                let r = Rect::from_min_size(pos2(list.left(), list.top() + m::TIGHT + row as f32 * ROW), vec2(list.width(), ROW));
                let resp = ui.interact(r, ui.make_persistent_id(("row", index)), Sense::click());
                if resp.hovered() && ui.input(|i| i.pointer.delta().length_sq() > 0.0) && item.enabled {
                    self.selected = index;
                }
                if resp.clicked() {
                    chosen = Some(index);
                }
                let selected = index == self.selected;
                let (fg, dim) = if selected {
                    painter.rect_filled(r, 0.0, c(p.menu_selected_background));
                    (c(p.menu_selected_item_text), c(p.menu_selected_item_text).gamma_multiply(0.75))
                } else {
                    (c(p.panel_text), style::disabled_text(&p))
                };
                let fg = if item.enabled { fg } else { dim };
                let icon_r = Rect::from_center_size(pos2(r.left() + 8.0 + ICON / 2.0, r.center().y), vec2(ICON, ICON));
                let drew = item.icon.as_deref().is_some_and(|i| icons::paint_tinted(&painter, icon_r, i, if item.enabled { Color32::WHITE } else { Color32::from_white_alpha(110) }));
                if !drew {
                    icons::paint(&painter, icon_r, "application");
                }
                // Right side: the kind (on the subtitle's line), and keys
                // before it.
                let label = if selected && command && matches!(item.action, Action::Open(_)) { olo_kit::tracker::LABEL } else { item.kind.label() };
                let kind = style::label_fit(&painter, r.right() - 8.0, r.center().y + 1.0, Align::Max, label, style::small_font(), dim, f32::INFINITY);
                let mut right = kind.left() - m::SPACING;
                if let Some(keys) = &item.keys {
                    let w = style::key_combo_width(ui.ctx(), keys);
                    style::key_combo(&painter, pos2(right, r.center().y), keys, fg);
                    right -= w + m::SPACING;
                }
                // Title, and the subtitle after it in grey.
                let x = icon_r.right() + 10.0;
                let avail = (right - x).max(40.0);
                let title = style::label_fit(&painter, x, r.center().y, Align::Min, &item.title, style::plain_font(), fg, avail);
                let sx = title.right() + 8.0;
                if !item.subtitle.is_empty() && right - sx > 40.0 {
                    style::label_fit(&painter, sx, r.center().y + 1.0, Align::Min, &item.subtitle, style::small_font(), dim, right - sx);
                }
            }
            if let Some(i) = chosen {
                if command { self.reveal(cx, i) } else { self.choose(cx, i) }
            }
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) if launch.args.iter().any(|a| a == "--toggle") => return self.toggle(cx),
            Ok(launch) if launch.args.iter().any(|a| a == "--shortcuts") => return self.toggle_sheet(cx),
            Ok(launch) => {
                if let Some(ask) = window_menu::Ask::parse(&launch.args) {
                    self.ask_window_menu(cx, ask);
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<sheet::Learned>() {
            Ok(learned) => {
                if let Some(old) = self.sheet.take() {
                    cx.close_window(old.win);
                }
                self.sheet = Some(sheet::open(cx, *learned));
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<window_menu::Facts>() {
            Ok(facts) => {
                if let Some(old) = self.window_menu.take() {
                    cx.close_window(old.win);
                }
                self.window_menu = Some(window_menu::open(cx, *facts));
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
        }
        if let Some(w) = self.win {
            cx.redraw(w);
        }
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle");
    let shortcuts = args.iter().any(|a| a == "--shortcuts");
    let ask = window_menu::Ask::parse(&args);
    olo_kit::run(APP_ID, move |cx| {
        cx.stay_resident();
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        let _ = std::thread::Builder::new().name("palette-fixed".into()).spawn(move || {
            proxy.send(Found::Fixed(sources::fixed()));
            // Applications installed or removed: listed again at once.
            if let Some(w) = olo_kit::watch::watch(olo_kit::desktop_entry::app_dirs(), move |_| proxy.send(Found::Fixed(sources::fixed()))) {
                std::mem::forget(w);
            }
        });
        let mut palette = Palette {
            win: None,
            opening: false,
            query: String::new(),
            asked: 0,
            selected: 0,
            top: 0,
            focus_field: false,
            fixed: Vec::new(),
            context: Vec::new(),
            menus: Vec::new(),
            files: Vec::new(),
            passwords: Vec::new(),
            results: Vec::new(),
            front: None,
            recents: Recents::load(),
            workers: sources::workers(cx.proxy()),
            window_menu: None,
            sheet: None,
        };
        if let Some(ask) = ask {
            palette.ask_window_menu(cx, ask);
        }
        if toggle {
            palette.toggle(cx);
        }
        if shortcuts {
            palette.toggle_sheet(cx);
        }
        palette
    });
}
