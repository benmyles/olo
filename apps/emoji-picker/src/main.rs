//! olo-emoji-picker: emoji, put into the field that had the keyboard.
//! ⌃⌘Space (Windows' keys: ⊞.) brings it up over everything: what was
//! picked lately, then every emoji by group, a tab for each; typing finds
//! them by name, CLDR's keywords and shortcodes as you type, letters left
//! out forgiven. ⌘↩ asks a model (Preferences ▸ AI ▸ Emoji) for five
//! that say what was typed ("pat on the back for going above and
//! beyond"), shown first. Return, or a click, puts the one selected into
//! the window that had the keyboard (app_server's `InsertText`: a text
//! input or keys, any app's) and puts the picker away; Escape, a click
//! outside or the shortcut again puts it away. Skin tones from the
//! hands bottom right, kept. On the GPUI kit (olo-ui).
//!
//! It stays running between uses so it opens at once: app_server starts
//! `olo-emoji-picker --toggle`, which hands the toggle to the running
//! copy.

mod ai;
mod data;

use std::{ops::Range, time::Instant};

use data::{Emoji, GROUPS, Recents, Tone};
use olo_protocol::{Client, Reply, Request};
use olo_ui::{
    Frame, WindowOptions,
    gpui::{
        AppContext as _, Bounds, Context, Entity, Focusable as _, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollWheelEvent, SharedString, Styled as _, Task, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, Align, c},
    theme,
    widgets::{TextField, TextFieldEvent, text_field},
};

const APP_ID: &str = "olo.emoji-picker";
/// Emoji a row, each cell this square, the emoji this big in it.
const COLS: usize = 10;
const CELL: f32 = 40.0;
const EMOJI: f32 = 24.0;
/// A group's heading in the grid.
const HEADER: f32 = 24.0;
/// Rows of cells the grid shows (fewer on a short screen).
const ROWS: f32 = 6.0;
const FIELD_HEIGHT: f32 = 32.0;
const QUERY_SIZE: f32 = 16.0;
/// The tabs: one for what was picked lately, and one a group, each a
/// cell wide.
const TAB: f32 = 30.0;
/// The line at the bottom: the selected emoji, what it is, the tones.
const STATUS: f32 = 44.0;
const BIG: f32 = 30.0;
const TONE: f32 = 18.0;
/// How far down the screen it sits.
const TOP: f32 = 0.2;
/// How many lately picked it shows (two rows).
const RECENT: usize = 2 * COLS;
/// How long its emoji stay drawn after it's put away.
const FORGET: std::time::Duration = std::time::Duration::from_secs(600);

/// One emoji in the grid.
#[derive(Clone)]
struct Cell {
    text: SharedString,
    /// Its emoji in the list (for its name), if Unicode knows it.
    emoji: Option<usize>,
    /// The model's reason, for a suggestion.
    why: Option<SharedString>,
}

/// A line of the grid: a heading, or a row of cells.
#[derive(Clone)]
enum Line {
    Header(SharedString),
    Cells(Range<usize>),
    /// Something to say where cells would be (nothing found, loading).
    Note(SharedString),
}

impl Line {
    fn height(&self) -> f32 {
        match self {
            Line::Cells(_) => CELL,
            Line::Header(_) | Line::Note(_) => HEADER,
        }
    }
}

/// What the workers send back.
enum Found {
    Emoji(Vec<Emoji>),
    Model(u64, Result<String, String>),
    Answer(u64, String, Result<Vec<ai::Suggestion>, String>),
}

/// The model being asked.
struct Asking {
    what: String,
    model: Option<String>,
    since: Instant,
    _tick: Task<()>,
}

struct Picker {
    win: Option<WindowHandle<Frame>>,
    query: Entity<TextField>,
    query_text: String,
    all: Vec<Emoji>,
    loaded: bool,
    recents: Recents,
    tone: Tone,
    lines: Vec<Line>,
    cells: Vec<Cell>,
    /// Where each tab's heading is: (tab's emoji, its name, line).
    tabs: Vec<(&'static str, &'static str, usize)>,
    selected: usize,
    /// How far down the grid is scrolled, in logical pixels.
    scroll: f32,
    /// Bumped with each question to the model, so late answers are dropped.
    asked: u64,
    asking: Option<Asking>,
    /// The model's five, for the description asked.
    suggested: Option<(String, Vec<ai::Suggestion>)>,
    /// What went wrong asking.
    trouble: Option<String>,
    /// Rows of cells showing (fewer than `ROWS` on a short screen).
    rows: f32,
    /// The scales and tones its emoji were drawn ahead for (`warm`).
    warmed: Vec<(u32, Tone)>,
    /// Bumped each time it's put away; the kept emoji let go `FORGET`
    /// after, unless it was shown again.
    shown: u64,
    forget: Option<Task<()>>,
}

/// The overlay over the output the picker sits on: the keyboard is its.
fn overlay() -> WindowOptions {
    let mut options = WindowOptions::new("Emoji".to_owned(), 1.0, 1.0)
        .layer("emoji-picker", LayerShellLayer::Overlay, (true, true, true, true))
        .layer_options(|l| {
            l.exclusive_zone = -1;
            l.keyboard = LayerKeyboard::Exclusive;
        })
        .no_panel();
    options.fixed = true;
    options
}

/// The keys that ask the model: ⌘↩ as the profile writes it.
fn ask_keys() -> String {
    olo_kit_base::keys::Shortcut::command(28).label()
}

impl Picker {
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.win.is_some() { self.hide(cx) } else { self.show(cx) }
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| {
            f.set_text("", cx);
            f.set_hint(format!("Search, or describe it and press {}", ask_keys()), cx);
        });
        self.query_text.clear();
        self.suggested = None;
        self.trouble = None;
        self.rebuild();
        self.select(self.first_cell());
        self.scroll = 0.0;
        let me = cx.entity();
        let query = self.query.clone();
        cx.defer(move |cx| {
            let view = me.clone();
            let w = olo_ui::open_window(cx, overlay(), move |window, cx| {
                window.focus(&query.focus_handle(cx));
                view.update(cx, |p, _| p.warm(window));
                view
            });
            me.update(cx, |p, _| p.win = w);
        });
    }

    /// Every emoji drawn ahead, off the UI thread, the likeliest first (what
    /// was picked lately, then the groups in order): kept for the windows
    /// after, so a search shows them without drawing them in its frame.
    /// Once a scale and tone; let go ten minutes after the picker is put
    /// away (`hide`).
    fn warm(&mut self, window: &mut Window) {
        let key = (window.scale_factor().to_bits(), self.tone);
        if !self.loaded || self.warmed.contains(&key) {
            return;
        }
        // Another scale's may be let go to make room for these (GPUI
        // keeps 32 MB): drawn ahead again if it comes back (those still
        // kept are skipped).
        self.warmed.retain(|k| k.0 == key.0);
        self.warmed.push(key);
        let tone = self.tone;
        let recent = self.recents.most(&self.all, RECENT);
        let texts: Vec<SharedString> = recent.iter().copied().chain((0..self.all.len()).filter(|i| !recent.contains(i))).map(|i| SharedString::from(self.all[i].glyph(tone))).collect();
        let font = style::ui_font(FontWeight::NORMAL);
        window.warm_text(&font, px(EMOJI), texts);
        // The tabs, the hands and the one shown big.
        window.warm_text(&font, px(16.0), GROUPS.iter().map(|g| SharedString::from(g.1)).chain(["🕘".into()]).collect::<Vec<_>>());
        window.warm_text(&font, px(TONE - 4.0), Tone::ALL.iter().filter_map(|t| emojis::get("✋").and_then(|h| t.0.map_or(Some(h), |s| h.with_skin_tone(s)))).map(|h| SharedString::from(h.as_str())).collect::<Vec<_>>());
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.win.take() {
            cx.defer(move |cx| cx.close_window(w));
        }
        // Its emoji kept a while, for the next time; then let go, so a
        // picker left alone takes little memory.
        self.shown += 1;
        let shown = self.shown;
        self.forget = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FORGET).await;
            let _ = this.update(cx, |p, cx| {
                if p.shown == shown && p.win.is_none() {
                    cx.text_system().forget_kept_glyphs();
                    p.warmed.clear();
                }
            });
        }));
        // A question still out is answered to no one.
        self.asked += 1;
        self.asking = None;
        cx.notify();
    }

    /// The grid's lines and cells for the query, the suggestions and
    /// what was picked lately.
    fn rebuild(&mut self) {
        let mut lines = Vec::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut tabs = Vec::new();
        let tone = self.tone;
        let all = &self.all;
        let row = |lines: &mut Vec<Line>, cells: &mut Vec<Cell>, found: Vec<Cell>| {
            for chunk in found.chunks(COLS) {
                let start = cells.len();
                cells.extend(chunk.iter().cloned());
                lines.push(Line::Cells(start..cells.len()));
            }
        };
        let cell = |i: usize| Cell { text: all[i].glyph(tone).into(), emoji: Some(i), why: None };
        if let Some((what, found)) = &self.suggested {
            lines.push(Line::Header(format!("Suggested for “{what}”").into()));
            let found = found
                .iter()
                .map(|s| {
                    // The model's own tone, else the one chosen.
                    let i = all.iter().position(|e| e.e.as_str() == s.emoji || e.e.skin_tones().is_some_and(|mut t| t.any(|t| t.as_str() == s.emoji)));
                    let text = match i {
                        Some(i) if all[i].e.as_str() == s.emoji => all[i].glyph(tone).into(),
                        _ => s.emoji.clone().into(),
                    };
                    Cell { text, emoji: i, why: Some(s.why.clone().into()) }
                })
                .collect();
            row(&mut lines, &mut cells, found);
        }
        if !self.loaded {
            lines.push(Line::Note("Loading…".into()));
        } else if self.query_text.trim().is_empty() {
            let recent = self.recents.most(all, RECENT);
            if !recent.is_empty() {
                tabs.push(("🕘", "Frequently Used", lines.len()));
                lines.push(Line::Header("Frequently Used".into()));
                row(&mut lines, &mut cells, recent.into_iter().map(cell).collect());
            }
            for (g, (name, icon)) in GROUPS.iter().enumerate() {
                tabs.push((*icon, *name, lines.len()));
                lines.push(Line::Header((*name).into()));
                row(&mut lines, &mut cells, (0..all.len()).filter(|&i| all[i].group == g).map(cell).collect());
            }
        } else {
            // The tabs stay, to go back to browsing (`go_to` clears the
            // search, then finds where each group's heading is).
            if !self.recents.most(all, 1).is_empty() {
                tabs.push(("🕘", "Frequently Used", usize::MAX));
            }
            tabs.extend(GROUPS.iter().map(|(name, icon)| (*icon, *name, usize::MAX)));
            let found = data::search(all, &self.query_text, &self.recents);
            let answered = self.suggested.as_ref().is_some_and(|(what, _)| what.trim() == self.query_text.trim());
            if found.is_empty() && !answered {
                lines.push(Line::Note(format!("No emoji match. {} asks AI for some.", ask_keys()).into()));
            } else {
                row(&mut lines, &mut cells, found.into_iter().map(cell).collect());
            }
        }
        self.lines = lines;
        self.cells = cells;
        self.tabs = tabs;
        self.selected = self.selected.min(self.cells.len().saturating_sub(1));
    }

    /// The first emoji of the grid proper (a suggestion first, if any).
    fn first_cell(&self) -> usize {
        0
    }

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.cells.len().saturating_sub(1));
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).text().to_owned();
        if text == self.query_text {
            return;
        }
        self.query_text = text;
        self.rebuild();
        // The best match first: after the suggestions, if they show.
        let skip = self.suggested.as_ref().map_or(0, |(_, s)| s.len());
        self.select(if self.query_text.trim().is_empty() { 0 } else { skip });
        self.scroll = 0.0;
        cx.notify();
    }

    /// Where line `l` starts, from the grid's top.
    fn line_top(&self, l: usize) -> f32 {
        self.lines[..l].iter().map(Line::height).sum()
    }

    fn content_height(&self) -> f32 {
        self.lines.iter().map(Line::height).sum()
    }

    fn view_height(&self) -> f32 {
        self.rows * CELL
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, (self.content_height() - self.view_height()).max(0.0));
    }

    /// The line holding cell `i`, and its column.
    fn place(&self, i: usize) -> Option<(usize, usize)> {
        self.lines.iter().enumerate().find_map(|(l, line)| match line {
            Line::Cells(r) if r.contains(&i) => Some((l, i - r.start)),
            _ => None,
        })
    }

    /// Scroll so the selected cell shows (with its heading, going up to a
    /// group's first row).
    fn reveal(&mut self) {
        let Some((l, _)) = self.place(self.selected) else { return };
        let mut top = self.line_top(l);
        if l > 0 && matches!(self.lines[l - 1], Line::Header(_)) {
            top -= HEADER;
        }
        let bottom = self.line_top(l) + CELL;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + self.view_height() {
            self.scroll = bottom - self.view_height();
        }
        self.clamp_scroll();
    }

    /// The arrows: across a row and on to the next; up and down a column.
    fn step(&mut self, dx: isize, dy: isize) {
        if self.cells.is_empty() {
            return;
        }
        if dy == 0 {
            self.select((self.selected as isize + dx).clamp(0, self.cells.len() as isize - 1) as usize);
        } else if let Some((l, col)) = self.place(self.selected) {
            let rows: Vec<Range<usize>> = self.lines.iter().filter_map(|line| if let Line::Cells(r) = line { Some(r.clone()) } else { None }).collect();
            let here = self.lines[..l].iter().filter(|line| matches!(line, Line::Cells(_))).count() as isize;
            let to = (here + dy).clamp(0, rows.len() as isize - 1) as usize;
            let r = &rows[to];
            self.select((r.start + col).min(r.end - 1));
        }
        self.reveal();
    }

    /// A tab: the grid at its group (the search cleared).
    fn go_to(&mut self, tab: usize, cx: &mut Context<Self>) {
        if !self.query_text.is_empty() {
            self.query.update(cx, |f, cx| f.set_text("", cx));
            self.query_text.clear();
            self.rebuild();
        }
        let Some(&(_, _, line)) = self.tabs.get(tab).filter(|t| t.2 != usize::MAX) else { return };
        self.scroll = self.line_top(line);
        self.clamp_scroll();
        if let Some(Line::Cells(r)) = self.lines.get(line + 1) {
            self.selected = r.start;
        }
        cx.notify();
    }

    /// The tab whose group shows at the grid's top.
    fn current_tab(&self) -> Option<usize> {
        let top = self.scroll + 1.0;
        self.tabs.iter().rposition(|&(_, _, line)| line != usize::MAX && self.line_top(line) <= top)
    }

    fn set_tone(&mut self, tone: Tone, cx: &mut Context<Self>) {
        self.tone = tone;
        self.recents.set_tone(tone);
        self.rebuild();
        if let Some(w) = self.win {
            let me = cx.entity();
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, cx| me.update(cx, |p, _| p.warm(window)));
            });
        }
        cx.notify();
    }

    /// Put cell `i` into the field that had the keyboard.
    fn choose(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(cell) = self.cells.get(i).cloned() else { return };
        if let Some(e) = cell.emoji {
            self.recents.note(self.all[e].e.as_str());
        }
        self.hide(cx);
        let text = cell.text.to_string();
        // app_server puts it in once the keyboard is back with the window
        // that had it (our panel gone).
        let _ = std::thread::Builder::new().name("emoji-insert".into()).spawn(move || {
            let answer = Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&Request::InsertText { text }).map_err(|e| e.to_string()));
            match answer {
                Ok(Reply::Ok) => {}
                Ok(Reply::Error { message }) => tracing::warn!("the emoji didn't go in: {message}"),
                Ok(other) => tracing::warn!("the emoji didn't go in: {other:?}"),
                Err(e) => tracing::warn!("the emoji didn't go in: {e}"),
            }
        });
    }

    /// ⌘↩: five emoji for what was typed, from the model.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let what = self.query_text.trim().to_owned();
        if what.is_empty() {
            return;
        }
        self.asked += 1;
        self.trouble = None;
        let n = self.asked;
        let proxy = olo_ui::proxy(cx);
        let q = what.clone();
        let _ = std::thread::Builder::new().name("emoji-ai".into()).spawn(move || {
            let model = ai::model();
            let ok = model.is_ok();
            proxy.send(Found::Model(n, model));
            if ok {
                proxy.send(Found::Answer(n, q.clone(), ai::suggest(&q)));
            }
        });
        // The seconds waited, while it's asked (and only then).
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let Ok(going) = this.update(cx, |p, cx| {
                    cx.notify();
                    p.asking.is_some()
                }) else { return };
                if !going {
                    return;
                }
            }
        });
        self.asking = Some(Asking { what, model: None, since: Instant::now(), _tick: tick });
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        let command = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "enter" if command && !k.modifiers.shift && !k.modifiers.alt => self.ask(cx),
            "enter" if !k.modifiers.modified() => {
                let i = self.selected;
                self.choose(i, cx);
            }
            "escape" if !k.modifiers.modified() => self.hide(cx),
            "left" if !k.modifiers.modified() => self.step(-1, 0),
            "right" if !k.modifiers.modified() => self.step(1, 0),
            "up" if !k.modifiers.modified() => self.step(0, -1),
            "down" if !k.modifiers.modified() => self.step(0, 1),
            "pageup" if !k.modifiers.modified() => self.step(0, -(self.rows as isize)),
            "pagedown" if !k.modifiers.modified() => self.step(0, self.rows as isize),
            "tab" if !command && !k.modifiers.alt => {
                // The next group (or the one before, with Shift).
                let n = self.tabs.len();
                if n == 0 {
                    return true;
                }
                let at = self.current_tab().unwrap_or(0);
                let to = if k.modifiers.shift { (at + n - 1) % n } else { (at + 1) % n };
                self.go_to(to, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => {
                if launch.args.iter().any(|a| a == "--toggle") {
                    self.toggle(cx);
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(found) = message.downcast::<Found>() else { return };
        match *found {
            Found::Emoji(all) => {
                self.all = all;
                self.loaded = true;
                self.rebuild();
                if let Some(w) = self.win {
                    let me = cx.entity();
                    cx.defer(move |cx| {
                        let _ = w.update(cx, |_, window, cx| me.update(cx, |p, _| p.warm(window)));
                    });
                }
            }
            Found::Model(n, model) if n == self.asked => match model {
                Ok(m) => {
                    if let Some(a) = &mut self.asking {
                        a.model = Some(m);
                    }
                }
                Err(e) => {
                    self.asking = None;
                    self.trouble = Some(e);
                }
            },
            Found::Answer(n, what, answer) if n == self.asked => {
                self.asking = None;
                match answer {
                    Ok(found) if found.is_empty() => self.trouble = Some("No emoji came back. Try saying it another way.".into()),
                    Ok(found) => {
                        self.suggested = Some((what, found));
                        self.rebuild();
                        self.selected = 0;
                        self.scroll = 0.0;
                    }
                    Err(e) => self.trouble = Some(e),
                }
            }
            Found::Model(..) | Found::Answer(..) => return,
        }
        cx.notify();
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let screen = window.viewport_size();
        let grid_w = COLS as f32 * CELL;
        let width = grid_w + 2.0 * m::SPACING;
        // As many rows as fit, up to six; a fifth of the way down, or
        // higher if it must.
        let fixed = m::SPACING + FIELD_HEIGHT + m::TIGHT + TAB + m::TIGHT + m::TIGHT + STATUS + m::TIGHT;
        let screen_h = f32::from(screen.height);
        self.rows = ((screen_h - 2.0 * m::SPACING - fixed) / CELL).floor().clamp(2.0, ROWS);
        let grid_h = self.view_height();
        let height = fixed + grid_h;
        let top = (screen_h * TOP).round().min(screen_h - height - m::SPACING).max(0.0);
        let panel = style::snap(window, Bounds { origin: point((screen.width - px(width)) / 2.0, px(top)), size: size(px(width), px(height)) });
        let left = panel.left() + px(m::SPACING);
        let field = Bounds { origin: point(left, panel.top() + px(m::SPACING)), size: size(px(grid_w), px(FIELD_HEIGHT)) };
        let tabs_r = Bounds { origin: point(left, field.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(TAB)) };
        let grid = Bounds { origin: point(left, tabs_r.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(grid_h)) };
        let status = Bounds { origin: point(left, grid.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(STATUS)) };
        let glass = style::snap_centred(window, point(field.left() + px(6.0 + 8.0), field.center().y), 16.0, 16.0);

        // The panel, its parts' wells, and the clicks around it (outside:
        // put away).
        let backdrop_me = me.clone();
        let backdrop = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                style::soft_shadow(window, panel, &p);
                style::fill_rect(window, panel, c(p.panel_background));
                style::frame_rect(window, panel, style::tint(p.panel_background, 1.6));
                style::raised(window, style::inset(window, panel, 1.0), &p);
                style::fill_rect(window, field, c(p.document_background));
                style::sunken(window, field, &p, true);
                icons::paint(window, glass, "query");
                style::fill_rect(window, grid, c(p.document_background));
                style::sunken(window, grid, &p, false);
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

        let text_left = glass.right() + px(8.0);
        let line = px((QUERY_SIZE * style::LINE_HEIGHT).ceil());
        let query = div().absolute().left(text_left).top(field.center().y - line / 2.0).w(field.right() - px(6.0) - text_left).h(line).child(self.query.clone());

        // The tabs.
        let tabs: Vec<(&'static str, &'static str)> = self.tabs.iter().map(|&(i, n, _)| (i, n)).collect();
        let current = self.current_tab();
        let tabs_me = me.clone();
        let tab_strip = canvas(
            move |_, window, _| window.insert_hitbox(tabs_r, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                let font = style::ui_font(FontWeight::NORMAL);
                let at = |n: usize| Bounds { origin: point(tabs_r.left() + px(n as f32 * CELL), tabs_r.top()), size: size(px(CELL), px(TAB)) };
                for (n, (icon, name)) in tabs.iter().enumerate() {
                    let r = style::snap(window, at(n));
                    let on = current == Some(n);
                    if on {
                        style::fill_rect(window, r, c(p.list_selected_background));
                    }
                    let mid = style::snap_coord(window, r.center().y);
                    style::label(window, cx, r.center().x, mid, Align::Center, *icon, 16.0, font.clone(), c(p.panel_text));
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Tab", name: Some((*name).into()), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
                }
                let count = tabs.len();
                let m1 = tabs_me.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && e.button == MouseButton::Left && hitbox.is_hovered(window) {
                        let n = (f32::from(e.position.x - tabs_r.left()) / CELL) as usize;
                        if n < count {
                            m1.update(cx, |p, cx| p.go_to(n, cx));
                        }
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        // The grid: the lines that show.
        let lines = self.lines.clone();
        let cells = self.cells.clone();
        let (scroll, selected) = (self.scroll, self.selected);
        let names: Vec<Option<&'static str>> = self.cells.iter().map(|c| c.emoji.map(|e| self.all[e].name())).collect();
        let grid_me = me.clone();
        let grid_paint = canvas(
            move |_, window, _| window.insert_hitbox(grid, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                let font = style::ui_font(FontWeight::NORMAL);
                let bold = style::ui_font(FontWeight::BOLD);
                let dim = style::disabled_text(&p);
                let inner = style::inset(window, grid, 1.0);
                let mut spots: Vec<(Bounds<Pixels>, usize)> = Vec::new();
                window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: inner }), |window| {
                    let mut y = grid.top() - px(scroll);
                    for line in &lines {
                        let h = px(line.height());
                        if y + h < grid.top() {
                            y += h;
                            continue;
                        }
                        if y > grid.bottom() {
                            break;
                        }
                        match line {
                            Line::Header(text) => {
                                let mid = style::snap_coord(window, y + h / 2.0);
                                let b = style::label(window, cx, grid.left() + px(m::SPACING), mid, Align::Left, text.clone(), style::DETAIL_SIZE, bold.clone(), dim);
                                olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Heading", clip: Some(inner), ..olo_ui::agent::Node::label(text.clone(), b) });
                            }
                            Line::Note(text) => {
                                let mid = style::snap_coord(window, y + h / 2.0);
                                let b = style::label_fit(window, cx, grid.left() + px(m::SPACING), mid, Align::Left, text.clone(), style::PLAIN_SIZE, font.clone(), c(p.document_text), grid.size.width - px(2.0 * m::SPACING));
                                olo_ui::agent::record(window, cx, olo_ui::agent::Node { clip: Some(inner), ..olo_ui::agent::Node::label(text.clone(), b) });
                            }
                            Line::Cells(r) => {
                                for i in r.clone() {
                                    let col = (i - r.start) as f32;
                                    let b = style::snap(window, Bounds { origin: point(grid.left() + px(col * CELL), y), size: size(px(CELL), px(CELL)) });
                                    if i == selected {
                                        style::fill_rect(window, style::inset(window, b, 2.0), c(p.list_selected_background));
                                    }
                                    let mid = style::snap_coord(window, b.center().y);
                                    style::label(window, cx, b.center().x, mid, Align::Center, cells[i].text.clone(), EMOJI, font.clone(), c(p.document_text));
                                    let name = names[i].unwrap_or("Emoji");
                                    olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Button", name: Some(name.into()), value: Some(cells[i].text.clone()), bounds: b, enabled: true, clickable: true, selected: Some(i == selected), clip: Some(inner), ..Default::default() });
                                    spots.push((b, i));
                                }
                            }
                        }
                        y += h;
                    }
                });
                // The pointer over a cell selects it; a click puts it in;
                // the wheel and fingers scroll.
                let at = move |pos: olo_ui::gpui::Point<Pixels>, spots: &[(Bounds<Pixels>, usize)]| spots.iter().find(|(b, _)| b.contains(&pos)).map(|s| s.1);
                let (m1, hb, s1) = (grid_me.clone(), hitbox.clone(), spots.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase.bubble()
                        && hb.is_hovered(window)
                        && let Some(i) = at(e.position, &s1)
                        && i != selected
                    {
                        m1.update(cx, |p, cx| {
                            p.selected = i;
                            cx.notify();
                        });
                    }
                });
                let (m2, hb, s2) = (grid_me.clone(), hitbox.clone(), spots);
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && e.button == MouseButton::Left
                        && hb.is_hovered(window)
                        && let Some(i) = at(e.position, &s2)
                    {
                        m2.update(cx, |p, cx| p.choose(i, cx));
                    }
                });
                let m3 = grid_me.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if !phase.bubble() || !hitbox.is_hovered(window) {
                        return;
                    }
                    let dy = f32::from(e.delta.pixel_delta(px(CELL)).y);
                    if dy.abs() < 0.01 {
                        return;
                    }
                    m3.update(cx, |p, cx| {
                        let was = p.scroll;
                        p.scroll -= dy;
                        p.clamp_scroll();
                        if p.scroll != was {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0();

        // The status line: the emoji selected, big, what it is; or the
        // question out to the model; and the tones.
        let shown = self.cells.get(self.selected).cloned();
        let about = shown.as_ref().and_then(|c| c.emoji).map(|e| &self.all[e]);
        let (title, detail): (SharedString, SharedString) = if let Some(a) = &self.asking {
            let model = a.model.as_deref().unwrap_or("the model");
            (format!("Asking {model}… {} s", a.since.elapsed().as_secs()).into(), format!("“{}”", a.what).into())
        } else if let Some(t) = &self.trouble {
            ("Couldn’t ask for emoji".into(), t.clone().into())
        } else {
            match (&shown, about) {
                (Some(cell), about) => {
                    let name = about.map_or("Emoji".to_owned(), |a| capitalised(a.name()));
                    let detail = match (&cell.why, about) {
                        (Some(why), _) => why.clone(),
                        (None, Some(a)) => a.shortcode().map_or_else(|| a.keywords().iter().take(4).cloned().collect::<Vec<_>>().join(", "), |s| format!(":{s}:")).into(),
                        (None, None) => SharedString::default(),
                    };
                    (name.into(), detail)
                }
                (None, _) => (SharedString::default(), SharedString::default()),
            }
        };
        let big = if self.asking.is_some() || self.trouble.is_some() { None } else { shown.map(|c| c.text) };
        let tone = self.tone;
        let status_me = me.clone();
        let status_paint = canvas(
            move |_, window, _| window.insert_hitbox(status, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let p = theme::palette(cx);
                let font = style::ui_font(FontWeight::NORMAL);
                let mid = style::snap_coord(window, status.center().y);
                if let Some(big) = &big {
                    style::label(window, cx, status.left() + px(4.0 + BIG / 2.0 + 2.0), mid, Align::Center, big.clone(), BIG, font.clone(), c(p.panel_text));
                }
                // The tones, at the right.
                let tones_w = 6.0 * (TONE + 4.0);
                let tones_left = status.right() - px(tones_w);
                let mut spots = Vec::new();
                for (n, t) in Tone::ALL.iter().enumerate() {
                    let b = style::snap_centred(window, point(tones_left + px(n as f32 * (TONE + 4.0) + TONE / 2.0 + 2.0), mid), TONE + 4.0, TONE + 4.0);
                    if *t == tone {
                        style::fill_rect(window, b, c(p.document_background));
                        style::sunken(window, b, &p, false);
                    }
                    let hand = emojis::get("✋").and_then(|h| match t.0 {
                        Some(s) => h.with_skin_tone(s),
                        None => Some(h),
                    });
                    style::label(window, cx, b.center().x, mid, Align::Center, hand.map_or("✋", |h| h.as_str()), TONE - 4.0, font.clone(), c(p.panel_text));
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "RadioButton", name: Some(t.name().into()), bounds: b, enabled: true, clickable: true, checked: Some(*t == tone), ..Default::default() });
                    spots.push((b, *t));
                }
                // What it is, between.
                let x = status.left() + px(4.0 + BIG + 10.0);
                let room = tones_left - px(m::SPACING) - x;
                let top_mid = mid - px(8.0);
                let detail_mid = mid + px(8.0);
                // (Whole for agents, though cut short on screen.)
                let b = style::label_fit(window, cx, x, top_mid, Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), c(p.panel_text), room);
                if !title.is_empty() {
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(title.clone(), b));
                }
                let b = style::label_fit(window, cx, x, detail_mid, Align::Left, detail.clone(), style::DETAIL_SIZE, font.clone(), style::disabled_text(&p), room);
                if !detail.is_empty() {
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(detail.clone(), b));
                }
                let m1 = status_me.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && e.button == MouseButton::Left
                        && hitbox.is_hovered(window)
                        && let Some((_, t)) = spots.iter().find(|(b, _)| b.contains(&e.position))
                    {
                        let t = *t;
                        m1.update(cx, |p, cx| p.set_tone(t, cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        let keys = me.clone();
        div()
            .size_full()
            .relative()
            .child(backdrop)
            .child(tab_strip)
            .child(grid_paint)
            .child(status_paint)
            .child(query)
            .capture_key_down(move |e: &KeyDownEvent, _, cx| {
                if keys.update(cx, |p, cx| p.key(e, cx)) {
                    cx.stop_propagation();
                }
            })
    }
}

/// "grinning face" → "Grinning face".
fn capitalised(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle");
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        instance::listen(APP_ID, olo_ui::proxy(cx));
        // The emoji, and which the font draws: off the UI thread, once.
        let proxy = olo_ui::proxy(cx);
        let _ = std::thread::Builder::new().name("emoji-load".into()).spawn(move || {
            let check = data::font_check();
            let all = data::load(|cluster| check.as_ref().is_none_or(|draws| draws(cluster)));
            proxy.send(Found::Emoji(all));
        });
        let query = text_field(cx, |f| f.name("Search").font_size(QUERY_SIZE));
        let recents = Recents::load();
        let picker = cx.new(|cx| {
            cx.subscribe(&query, |p: &mut Picker, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => {
                    let i = p.selected;
                    p.choose(i, cx);
                }
                _ => p.query_changed(cx),
            })
            .detach();
            Picker {
                win: None,
                query,
                query_text: String::new(),
                all: Vec::new(),
                loaded: false,
                tone: recents.tone(),
                recents,
                lines: Vec::new(),
                cells: Vec::new(),
                tabs: Vec::new(),
                selected: 0,
                scroll: 0.0,
                asked: 0,
                asking: None,
                suggested: None,
                trouble: None,
                rows: ROWS,
                warmed: Vec::new(),
                shown: 0,
                forget: None,
            }
        });
        let messages = picker.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |p, cx| p.message(message, cx)));
        // A new version of a running copy re-executes with the arguments
        // it first started with (`--toggle`), which mustn't open it again.
        olo_ui::on_upgrade(cx, |_| Some("{}".into()));
        if olo_ui::restored_state().is_none() && toggle {
            picker.update(cx, |p, cx| p.toggle(cx));
        }
    });
}
