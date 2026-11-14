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
//! hands bottom right, kept.
//!
//! Each time it comes up it's a new window, on a looper of its own,
//! with a picker of its own; what lasts between (every emoji, what was
//! picked lately, the tone) is the program's.
//!
//! It stays running between uses so it opens at once: app_server starts
//! `olo-emoji-picker --toggle`, which hands the toggle to the running
//! copy.

mod ai;
mod data;

use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use data::{Emoji, GROUPS, Recents, Tone};
use olo_kit::{
    Bounds, Child, Cx, Element, FontWeight, InteractiveElement as _, KeyDownEvent, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, Painter, ParentElement as _, Pixels,
    Point, ScrollWheelEvent, SharedString, Styled as _, View, WindowHandle, WindowOptions, agent, canvas, div, icons,
    instance::{self, Launch},
    metrics as m, point, px, size,
    style::{self, Align, c},
    theme, view_canvas,
    widgets::{TextField, TextFieldEvent, text_field},
};
use olo_protocol::{Client, Reply, Request};

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
const FORGET: Duration = Duration::from_secs(600);

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

/// The picker's window is closing.
struct Closed;

/// Counts the questions to the model, whichever window asked: a late
/// answer is to no one.
static ASKED: AtomicU64 = AtomicU64::new(0);
/// The scales and tones the emoji were drawn ahead for (`Picker::warm`).
static WARMED: Mutex<Vec<(u32, Tone)>> = Mutex::new(Vec::new());

/// The model being asked.
struct Asking {
    what: String,
    model: Option<String>,
    since: Instant,
}

/// What lasts between its windows, on the program's looper.
struct Hub {
    /// Every emoji, once read.
    all: Option<Arc<Vec<Emoji>>>,
    recents: Arc<Mutex<Recents>>,
    window: Option<WindowHandle<Picker>>,
    /// Bumped each time it's put away; the kept emoji let go `FORGET`
    /// after, unless it was shown again.
    shown: u64,
}

impl Hub {
    fn showing(&self) -> Option<&WindowHandle<Picker>> {
        self.window.as_ref().filter(|w| w.is_open())
    }

    fn toggle(&mut self) {
        match self.showing() {
            Some(window) => window.close(),
            None => {
                let (all, recents) = (self.all.clone(), self.recents.clone());
                self.window = Some(olo_kit::open_window(overlay(), move |cx| Picker::new(all, recents, cx)));
            }
        }
    }
}

/// What a picker's window is told.
enum PickerMsg {
    /// Every emoji has been read.
    Loaded(Arc<Vec<Emoji>>),
    Model(u64, Result<String, String>),
    Answer(u64, String, Result<Vec<ai::Suggestion>, String>),
    /// A second of waiting for the model has gone by.
    Tick,
}

/// Where its parts are in its window (the whole display).
#[derive(Clone, Copy, Default, PartialEq)]
struct Geometry {
    panel: Bounds<Pixels>,
    field: Bounds<Pixels>,
    tabs: Bounds<Pixels>,
    grid: Bounds<Pixels>,
    status: Bounds<Pixels>,
    /// The search field's magnifying glass.
    glass: Bounds<Pixels>,
    scale: f32,
}

/// `b` on whole device pixels (`style::snap`, without a painter).
fn snapped(scale: f32, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let at = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    Bounds::from_corners(point(at(b.origin.x), at(b.origin.y)), point(at(b.origin.x + b.size.width), at(b.origin.y + b.size.height)))
}

/// A `w` × `h` box centred on `center` on whole device pixels
/// (`style::snap_centred`, without a painter).
fn snapped_centred(scale: f32, center: Point<Pixels>, w: f32, h: f32) -> Bounds<Pixels> {
    let (dw, dh) = ((w * scale).round(), (h * scale).round());
    let min = point(px(((f32::from(center.x) * scale - dw / 2.0).round()) / scale), px(((f32::from(center.y) * scale - dh / 2.0).round()) / scale));
    Bounds { origin: min, size: size(px(dw / scale), px(dh / scale)) }
}

impl Geometry {
    fn key(&self) -> u64 {
        let b = |r: Bounds<Pixels>| [r.origin.x, r.origin.y, r.size.width, r.size.height].map(|v| f32::from(v).to_bits());
        style::key((b(self.panel), b(self.grid), self.scale.to_bits()))
    }

    /// The tones' boxes, at the status line's right.
    fn tones(&self) -> [(Bounds<Pixels>, Tone); 6] {
        let mid = px((f32::from(self.status.center().y) * self.scale).round() / self.scale);
        let left = self.status.right() - px(6.0 * (TONE + 4.0));
        let mut n = 0.0;
        Tone::ALL.map(|t| {
            let b = snapped_centred(self.scale, point(left + px(n * (TONE + 4.0) + TONE / 2.0 + 2.0), mid), TONE + 4.0, TONE + 4.0);
            n += 1.0;
            (b, t)
        })
    }
}

struct Picker {
    query: Child<TextField>,
    query_text: String,
    all: Arc<Vec<Emoji>>,
    loaded: bool,
    recents: Arc<Mutex<Recents>>,
    tone: Tone,
    lines: Vec<Line>,
    cells: Vec<Cell>,
    /// Where each tab's heading is: (tab's emoji, its name, line).
    tabs: Vec<(&'static str, &'static str, usize)>,
    /// Bumped as the lines and cells are made again (what the grid's
    /// drawing goes by).
    generation: u64,
    selected: usize,
    /// How far down the grid is scrolled, in logical pixels.
    scroll: f32,
    /// The question out to the model (its number: late answers are dropped).
    asked: u64,
    asking: Option<Asking>,
    /// The model's five, for the description asked.
    suggested: Option<(String, Vec<ai::Suggestion>)>,
    /// What went wrong asking.
    trouble: Option<String>,
    /// Rows of cells showing (fewer than `ROWS` on a short screen).
    rows: f32,
    geometry: Geometry,
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
    fn new(all: Option<Arc<Vec<Emoji>>>, recents: Arc<Mutex<Recents>>, cx: &mut Cx<Self>) -> Picker {
        let query = text_field(cx, |f| f.name("Search").font_size(QUERY_SIZE).hint(format!("Search, or describe it and press {}", ask_keys())));
        cx.subscribe(&query, |p: &mut Picker, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => {
                let i = p.selected;
                p.choose(i, cx);
            }
            _ => p.query_changed(cx),
        });
        query.focus(cx);
        let tone = recents.lock().unwrap_or_else(|e| e.into_inner()).tone();
        let mut picker = Picker {
            query,
            query_text: String::new(),
            loaded: all.is_some(),
            all: all.unwrap_or_default(),
            recents,
            tone,
            lines: Vec::new(),
            cells: Vec::new(),
            tabs: Vec::new(),
            generation: 0,
            selected: 0,
            scroll: 0.0,
            asked: 0,
            asking: None,
            suggested: None,
            trouble: None,
            rows: ROWS,
            geometry: Geometry::default(),
        };
        picker.rebuild();
        picker
    }

    /// Every emoji drawn ahead, off the UI thread, the likeliest first (what
    /// was picked lately, then the groups in order): kept for the windows
    /// after, so a search shows them without drawing them in its frame.
    /// Once a scale and tone; let go ten minutes after the picker is put
    /// away (`Hub`).
    fn warm(&self, cx: &mut Cx<Self>) {
        let key = (cx.scale_factor().to_bits(), self.tone);
        if !self.loaded {
            return;
        }
        {
            let mut warmed = WARMED.lock().unwrap_or_else(|e| e.into_inner());
            if warmed.contains(&key) {
                return;
            }
            // Another scale's may be let go to make room for these (32 MB
            // are kept): drawn ahead again if it comes back (those still
            // kept are skipped).
            warmed.retain(|k| k.0 == key.0);
            warmed.push(key);
        }
        let tone = self.tone;
        let recent = self.recents.lock().unwrap_or_else(|e| e.into_inner()).most(&self.all, RECENT);
        let texts: Vec<SharedString> = recent.iter().copied().chain((0..self.all.len()).filter(|i| !recent.contains(i))).map(|i| SharedString::from(self.all[i].glyph(tone))).collect();
        let font = style::ui_font(FontWeight::NORMAL);
        cx.warm_text(&font, px(EMOJI), texts);
        // The tabs, the hands and the one shown big.
        cx.warm_text(&font, px(16.0), GROUPS.iter().map(|g| SharedString::from(g.1)).chain(["🕘".into()]).collect::<Vec<_>>());
        cx.warm_text(
            &font,
            px(TONE - 4.0),
            Tone::ALL.iter().filter_map(|t| emojis::get("✋").and_then(|h| t.0.map_or(Some(h), |s| h.with_skin_tone(s)))).map(|h| SharedString::from(h.as_str())).collect::<Vec<_>>(),
        );
    }

    /// Put away: its window closes (the program hears, `Closed`).
    fn hide(&mut self, cx: &mut Cx<Self>) {
        // A question still out is answered to no one.
        self.asked = 0;
        self.asking = None;
        cx.close_window();
    }

    /// The grid's lines and cells for the query, the suggestions and
    /// what was picked lately.
    fn rebuild(&mut self) {
        let mut lines = Vec::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut tabs = Vec::new();
        let tone = self.tone;
        let all = self.all.clone();
        let all = &all[..];
        let recents = self.recents.lock().unwrap_or_else(|e| e.into_inner());
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
            let recent = recents.most(all, RECENT);
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
            if !recents.most(all, 1).is_empty() {
                tabs.push(("🕘", "Frequently Used", usize::MAX));
            }
            tabs.extend(GROUPS.iter().map(|(name, icon)| (*icon, *name, usize::MAX)));
            let found = data::search(all, &self.query_text, &recents);
            let answered = self.suggested.as_ref().is_some_and(|(what, _)| what.trim() == self.query_text.trim());
            if found.is_empty() && !answered {
                lines.push(Line::Note(format!("No emoji match. {} asks AI for some.", ask_keys()).into()));
            } else {
                row(&mut lines, &mut cells, found.into_iter().map(cell).collect());
            }
        }
        drop(recents);
        self.lines = lines;
        self.cells = cells;
        self.tabs = tabs;
        self.generation += 1;
        self.selected = self.selected.min(self.cells.len().saturating_sub(1));
    }

    /// The first emoji of the grid proper (a suggestion first, if any).
    fn first_cell(&self) -> usize {
        0
    }

    fn select(&mut self, i: usize) {
        self.selected = i.min(self.cells.len().saturating_sub(1));
    }

    fn query_changed(&mut self, cx: &mut Cx<Self>) {
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
        cx.invalidate();
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
    fn go_to(&mut self, tab: usize, cx: &mut Cx<Self>) {
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
        cx.invalidate();
    }

    /// The tab whose group shows at the grid's top.
    fn current_tab(&self) -> Option<usize> {
        let top = self.scroll + 1.0;
        self.tabs.iter().rposition(|&(_, _, line)| line != usize::MAX && self.line_top(line) <= top)
    }

    fn set_tone(&mut self, tone: Tone, cx: &mut Cx<Self>) {
        self.tone = tone;
        self.recents.lock().unwrap_or_else(|e| e.into_inner()).set_tone(tone);
        self.rebuild();
        self.warm(cx);
        cx.invalidate();
    }

    /// Put cell `i` into the field that had the keyboard.
    fn choose(&mut self, i: usize, cx: &mut Cx<Self>) {
        let Some(cell) = self.cells.get(i).cloned() else { return };
        if let Some(e) = cell.emoji {
            self.recents.lock().unwrap_or_else(|e| e.into_inner()).note(self.all[e].e.as_str());
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
    fn ask(&mut self, cx: &mut Cx<Self>) {
        let what = self.query_text.trim().to_owned();
        if what.is_empty() {
            return;
        }
        let n = ASKED.fetch_add(1, Ordering::AcqRel) + 1;
        self.asked = n;
        self.trouble = None;
        let proxy = olo_kit::proxy();
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
        if self.asking.is_none() {
            cx.after(Duration::from_secs(1), PickerMsg::Tick);
        }
        self.asking = Some(Asking { what, model: None, since: Instant::now() });
        cx.invalidate();
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
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
        cx.invalidate();
        true
    }

    /// The cell at `at` in the grid as it shows, if one is there.
    fn cell_at(&self, at: Point<Pixels>) -> Option<usize> {
        let grid = self.geometry.grid;
        if !grid.contains(&at) {
            return None;
        }
        let mut y = grid.top() - px(self.scroll);
        for line in &self.lines {
            let h = px(line.height());
            if y > grid.bottom() {
                break;
            }
            if let Line::Cells(r) = line
                && y + h >= grid.top()
            {
                for i in r.clone() {
                    let b = snapped(self.geometry.scale, Bounds { origin: point(grid.left() + px((i - r.start) as f32 * CELL), y), size: size(px(CELL), px(CELL)) });
                    if b.contains(&at) {
                        return Some(i);
                    }
                }
            }
            y += h;
        }
        None
    }

    /// A press: outside the panel puts it away; on a tab, a cell or a
    /// tone, that.
    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let g = self.geometry;
        if !g.panel.contains(&e.position) {
            return self.hide(cx);
        }
        if e.button != MouseButton::Left {
            return;
        }
        if g.tabs.contains(&e.position) {
            let n = (f32::from(e.position.x - g.tabs.left()) / CELL) as usize;
            if n < self.tabs.len() {
                self.go_to(n, cx);
            }
        } else if let Some(i) = self.cell_at(e.position) {
            self.choose(i, cx);
        } else if let Some((_, tone)) = g.tones().into_iter().find(|(b, _)| b.contains(&e.position)) {
            self.set_tone(tone, cx);
        }
    }

    /// The pointer over a cell selects it.
    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        if let Some(i) = self.cell_at(e.position)
            && i != self.selected
        {
            self.selected = i;
            cx.invalidate();
        }
    }

    /// The wheel and fingers scroll the grid.
    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Cx<Self>) {
        if !self.geometry.grid.contains(&e.position) {
            return;
        }
        let dy = f32::from(e.delta.pixel_delta(px(CELL)).y);
        if dy.abs() < 0.01 {
            return;
        }
        let was = self.scroll;
        self.scroll -= dy;
        self.clamp_scroll();
        if self.scroll != was {
            cx.invalidate();
        }
    }

    /// The status line's title, detail and the emoji shown big: the one
    /// selected and what it is, or the question out to the model.
    fn status(&self) -> (SharedString, SharedString, Option<SharedString>) {
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
        (title, detail, big)
    }

    fn paint_tabs(&self, g: &mut Painter) {
        let p = theme::palette();
        let font = style::ui_font(FontWeight::NORMAL);
        let tabs_r = self.geometry.tabs;
        let current = self.current_tab();
        let at = |n: usize| Bounds { origin: point(tabs_r.left() + px(n as f32 * CELL), tabs_r.top()), size: size(px(CELL), px(TAB)) };
        for (n, (icon, name, _)) in self.tabs.iter().enumerate() {
            let r = style::snap(g, at(n));
            let on = current == Some(n);
            if on {
                style::fill_rect(g, r, c(p.list_selected_background));
            }
            let mid = style::snap_coord(g, r.center().y);
            style::label(g, r.center().x, mid, Align::Center, *icon, 16.0, font.clone(), c(p.panel_text));
            g.agent(agent::Node { role: "Tab", name: Some((*name).into()), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
        }
    }

    /// The grid: the lines that show.
    fn paint_grid(&self, g: &mut Painter) {
        let p = theme::palette();
        let font = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let dim = style::disabled_text(&p);
        let grid = self.geometry.grid;
        let inner = style::inset(g, grid, 1.0);
        let (scroll, selected) = (self.scroll, self.selected);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: inner }), |g| {
            let mut y = grid.top() - px(scroll);
            for line in &self.lines {
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
                        let mid = style::snap_coord(g, y + h / 2.0);
                        let b = style::label(g, grid.left() + px(m::SPACING), mid, Align::Left, text.clone(), style::DETAIL_SIZE, bold.clone(), dim);
                        g.agent(agent::Node { role: "Heading", clip: Some(inner), ..agent::Node::label(text.clone(), b) });
                    }
                    Line::Note(text) => {
                        let mid = style::snap_coord(g, y + h / 2.0);
                        let b = style::label_fit(
                            g,
                            grid.left() + px(m::SPACING),
                            mid,
                            Align::Left,
                            text.clone(),
                            style::PLAIN_SIZE,
                            font.clone(),
                            c(p.document_text),
                            grid.size.width - px(2.0 * m::SPACING),
                        );
                        g.agent(agent::Node { clip: Some(inner), ..agent::Node::label(text.clone(), b) });
                    }
                    Line::Cells(r) => {
                        for i in r.clone() {
                            let col = (i - r.start) as f32;
                            let b = style::snap(g, Bounds { origin: point(grid.left() + px(col * CELL), y), size: size(px(CELL), px(CELL)) });
                            // (The cell the pointer comes over is chosen:
                            // only cells' edges matter to it.)
                            g.note_pointer_edge(b.intersect(&inner));
                            if i == selected {
                                let lit = style::inset(g, b, 2.0);
                                style::fill_rect(g, lit, c(p.list_selected_background));
                            }
                            let mid = style::snap_coord(g, b.center().y);
                            let cell = &self.cells[i];
                            style::label(g, b.center().x, mid, Align::Center, cell.text.clone(), EMOJI, font.clone(), c(p.document_text));
                            let name = cell.emoji.map_or("Emoji", |e| self.all[e].name());
                            g.agent(agent::Node {
                                role: "Button",
                                name: Some(name.into()),
                                value: Some(cell.text.clone()),
                                bounds: b,
                                enabled: true,
                                clickable: true,
                                selected: Some(i == selected),
                                clip: Some(inner),
                                ..Default::default()
                            });
                        }
                    }
                }
                y += h;
            }
        });
    }

    /// The status line: the emoji selected, big, what it is; or the
    /// question out to the model; and the tones.
    fn paint_status(&self, g: &mut Painter) {
        let p = theme::palette();
        let font = style::ui_font(FontWeight::NORMAL);
        let status = self.geometry.status;
        let (title, detail, big) = self.status();
        let mid = style::snap_coord(g, status.center().y);
        if let Some(big) = &big {
            style::label(g, status.left() + px(4.0 + BIG / 2.0 + 2.0), mid, Align::Center, big.clone(), BIG, font.clone(), c(p.panel_text));
        }
        // The tones, at the right.
        let tones_left = status.right() - px(6.0 * (TONE + 4.0));
        for (n, t) in Tone::ALL.iter().enumerate() {
            let b = style::snap_centred(g, point(tones_left + px(n as f32 * (TONE + 4.0) + TONE / 2.0 + 2.0), mid), TONE + 4.0, TONE + 4.0);
            if *t == self.tone {
                style::fill_rect(g, b, c(p.document_background));
                style::sunken(g, b, &p, false);
            }
            let hand = emojis::get("✋").and_then(|h| match t.0 {
                Some(s) => h.with_skin_tone(s),
                None => Some(h),
            });
            style::label(g, b.center().x, mid, Align::Center, hand.map_or("✋", |h| h.as_str()), TONE - 4.0, font.clone(), c(p.panel_text));
            g.agent(agent::Node { role: "RadioButton", name: Some(t.name().into()), bounds: b, enabled: true, clickable: true, checked: Some(*t == self.tone), ..Default::default() });
        }
        // What it is, between.
        let x = status.left() + px(4.0 + BIG + 10.0);
        let room = tones_left - px(m::SPACING) - x;
        let top_mid = mid - px(8.0);
        let detail_mid = mid + px(8.0);
        // (Whole for agents, though cut short on screen.)
        let b = style::label_fit(g, x, top_mid, Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), c(p.panel_text), room);
        if !title.is_empty() {
            g.agent(agent::Node::label(title.clone(), b));
        }
        let b = style::label_fit(g, x, detail_mid, Align::Left, detail.clone(), style::DETAIL_SIZE, font.clone(), style::disabled_text(&p), room);
        if !detail.is_empty() {
            g.agent(agent::Node::label(detail.clone(), b));
        }
    }
}

impl View for Picker {
    type Message = PickerMsg;

    fn message(&mut self, message: PickerMsg, cx: &mut Cx<Self>) {
        match message {
            PickerMsg::Loaded(all) => {
                self.all = all;
                self.loaded = true;
                self.rebuild();
                self.warm(cx);
            }
            PickerMsg::Model(n, model) if n == self.asked => match model {
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
            PickerMsg::Answer(n, what, answer) if n == self.asked => {
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
            PickerMsg::Model(..) | PickerMsg::Answer(..) => return,
            PickerMsg::Tick => {
                if self.asking.is_none() {
                    return;
                }
                cx.after(Duration::from_secs(1), PickerMsg::Tick);
            }
        }
        cx.invalidate();
    }

    /// (However it's put away, the program hears.)
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        olo_kit::proxy().send(Closed);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = olo_kit::viewport_size();
        let scale = cx.scale_factor();
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
        let panel = snapped(scale, Bounds { origin: point((screen.width - px(width)) / 2.0, px(top)), size: size(px(width), px(height)) });
        let left = panel.left() + px(m::SPACING);
        let field = Bounds { origin: point(left, panel.top() + px(m::SPACING)), size: size(px(grid_w), px(FIELD_HEIGHT)) };
        let tabs_r = Bounds { origin: point(left, field.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(TAB)) };
        let grid = Bounds { origin: point(left, tabs_r.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(grid_h)) };
        let status = Bounds { origin: point(left, grid.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(STATUS)) };
        let glass = snapped_centred(scale, point(field.left() + px(6.0 + 8.0), field.center().y), 16.0, 16.0);
        self.geometry = Geometry { panel, field, tabs: tabs_r, grid, status, glass, scale };
        let geometry_key = self.geometry.key();
        self.warm(cx);

        // The panel and its parts' wells.
        let backdrop = canvas(move |g, _| {
            let p = theme::palette();
            style::soft_shadow(g, panel, &p);
            style::fill_rect(g, panel, c(p.panel_background));
            style::frame_rect(g, panel, style::tint(p.panel_background, 1.6));
            let inside = style::inset(g, panel, 1.0);
            style::raised(g, inside, &p);
            style::fill_rect(g, field, c(p.document_background));
            style::sunken(g, field, &p, true);
            icons::paint(g, glass, "query");
            style::fill_rect(g, grid, c(p.document_background));
            style::sunken(g, grid, &p, false);
        })
        .paint_key(geometry_key)
        .absolute()
        .inset_0();

        let text_left = glass.right() + px(8.0);
        let line = px((QUERY_SIZE * style::LINE_HEIGHT).ceil());
        let query = div().absolute().left(text_left).top(field.center().y - line / 2.0).w(field.right() - px(6.0) - text_left).h(line).child(self.query.element());

        // Each part is drawn again only when what it shows changes: the
        // pointer over another cell draws the grid and the status line,
        // not the panel, the tabs or the field.
        let tab_strip =
            view_canvas(|p: &Picker, g, _| p.paint_tabs(g)).key_by(move |p: &Picker| style::key((geometry_key, p.current_tab(), p.tabs.iter().map(|t| t.1).collect::<Vec<_>>()))).absolute().inset_0();
        let grid_paint = view_canvas(|p: &Picker, g, _| p.paint_grid(g)).key_by(move |p: &Picker| style::key((geometry_key, p.generation, p.scroll.to_bits(), p.selected))).absolute().inset_0();
        let status_paint = view_canvas(|p: &Picker, g, _| p.paint_status(g))
            .key_by(move |p: &Picker| {
                let (title, detail, big) = p.status();
                style::key((geometry_key, title, detail, big, Tone::ALL.iter().position(|t| *t == p.tone)))
            })
            .absolute()
            .inset_0();

        div()
            .size_full()
            .relative()
            .child(backdrop)
            .child(tab_strip)
            .child(grid_paint)
            .child(status_paint)
            .child(query)
            .capture_key_down(cx.listener(|p: &mut Self, e: &KeyDownEvent, cx| {
                if p.key(e, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_any_mouse_down(cx.listener(Self::mouse_down))
            .moves_matter_at_edges()
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::wheel))
    }
}

/// "grinning face" → "Grinning face".
fn capitalised(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle");
    olo_kit::run(APP_ID, move |app| {
        olo_kit::stay_resident();
        instance::listen(APP_ID, olo_kit::proxy());
        // The emoji, and which the font draws: off the UI thread, once.
        let proxy = olo_kit::proxy();
        let _ = std::thread::Builder::new().name("emoji-load".into()).spawn(move || {
            let check = data::font_check();
            let all = data::load(|cluster| check.as_ref().is_none_or(|draws| draws(cluster)));
            proxy.send(Found::Emoji(all));
        });
        let hub = Rc::new(RefCell::new(Hub { all: None, recents: Arc::new(Mutex::new(Recents::load())), window: None, shown: 0 }));
        let messages = hub.clone();
        olo_kit::message_handler(app, move |message, app| {
            let mut hub = messages.borrow_mut();
            let message = match message.downcast::<Launch>() {
                Ok(launch) => {
                    if launch.args.iter().any(|a| a == "--toggle") {
                        hub.toggle();
                    }
                    return;
                }
                Err(m) => m,
            };
            let message = match message.downcast::<Closed>() {
                Ok(_) => {
                    // (Unless another has come up since.)
                    if hub.showing().is_none() {
                        hub.window = None;
                    }
                    // Its emoji kept a while, for the next time; then let
                    // go, so a picker left alone takes little memory.
                    hub.shown += 1;
                    let (shown, later) = (hub.shown, messages.clone());
                    app.looper().after(FORGET, move |_| {
                        let hub = later.borrow();
                        if hub.shown == shown && hub.showing().is_none() {
                            olo_kit::text_system().forget_kept_glyphs();
                            WARMED.lock().unwrap_or_else(|e| e.into_inner()).clear();
                        }
                    });
                    return;
                }
                Err(m) => m,
            };
            let Ok(found) = message.downcast::<Found>() else { return };
            match *found {
                Found::Emoji(all) => {
                    let all = Arc::new(all);
                    hub.all = Some(all.clone());
                    if let Some(window) = hub.showing() {
                        window.messenger().send(PickerMsg::Loaded(all));
                    }
                }
                Found::Model(n, model) => {
                    if let Some(window) = hub.showing() {
                        window.messenger().send(PickerMsg::Model(n, model));
                    }
                }
                Found::Answer(n, what, answer) => {
                    if let Some(window) = hub.showing() {
                        window.messenger().send(PickerMsg::Answer(n, what, answer));
                    }
                }
            }
        });
        // A new version of a running copy re-executes with the arguments
        // it first started with (`--toggle`), which mustn't open it again.
        olo_kit::on_upgrade(|_| Some("{}".into()));
        if olo_kit::restored_state().is_none() && toggle {
            hub.borrow_mut().toggle();
        }
    });
}
