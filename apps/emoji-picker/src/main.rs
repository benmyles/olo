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
//!
//! `olo-emoji-picker --pick` is for an app that asks for an emoji (a
//! reaction, in Telegram or Phone: React ▸ Other…): a copy of its own,
//! shown at once, that prints the one picked (and a newline) and ends,
//! or ends with nothing printed if it's put away.

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
/// More of the well under its rows: the row it cuts at its foot shows
/// its emoji's eyes, a half row that says there's more (at the rows
/// alone, only their foreheads: Opus's look, 2026-11-28).
const PEEK: f32 = 4.0;
const FIELD_HEIGHT: f32 = 32.0;
const QUERY_SIZE: f32 = 16.0;
/// The tabs: one for what was picked lately, and one a group, each a
/// cell wide.
const TAB: f32 = 30.0;
/// The line at the bottom: the selected emoji, what it is, the tones.
const STATUS: f32 = 44.0;
const BIG: f32 = 30.0;
/// The box the big emoji is centred in: as wide as a colour emoji is at
/// `BIG` (1.25 of its size), so what it is starts a mark's gap after it,
/// not 4.6 points (the review's crowded: QA, 2026-11-28).
const BIG_BOX: f32 = 38.0;
const TONE: f32 = 18.0;
/// A tone's box (the chosen one's sunken): room round its hand, drawn at
/// `TONE - 4`, on every side, and whole device pixels at every scale, so
/// the boxes side by side don't round into each other (at `TONE + 4` the
/// hand had 2 points either side, and the boxes overlapped at 1.25×).
const TONE_BOX: f32 = 28.0;
/// How far down the screen it sits.
const TOP: f32 = 0.2;
/// Room on the screen's right for Deskbar and a gap beside it, which the
/// panel keeps clear of.
const DESKBAR_ROOM: f32 = m::DESKBAR_WIDTH + 8.0;
/// The notice that the colour font isn't installed: its width at most
/// (two lines), and the info icon beside it.
const NOTICE_W: f32 = 170.0;
const NOTICE_ICON: f32 = 16.0;
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
    /// The fonts looked at, before the emoji are read (`Drawn::early`).
    Drawn(data::Drawn),
    Emoji(Vec<Emoji>, data::Drawn),
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
    /// How they can be drawn.
    drawn: Arc<data::Drawn>,
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
                let (all, drawn, recents) = (self.all.clone(), self.drawn.clone(), self.recents.clone());
                self.window = Some(olo_kit::open_window(overlay(), move |cx| Picker::new(all, drawn, recents, cx)));
            }
        }
    }
}

/// What a picker's window is told.
enum PickerMsg {
    /// Every emoji has been read, and how they can be drawn.
    Loaded(Arc<Vec<Emoji>>, Arc<data::Drawn>),
    /// How they can be drawn, known before they're read.
    Drawn(Arc<data::Drawn>),
    /// A while since it opened, still reading: say so.
    Slow,
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
    /// The grid's well, its sunken frame included.
    grid: Bounds<Pixels>,
    /// Inside the well's frame: where the cells and headings scroll, as
    /// wide as a row of cells and as tall as `Picker::view_height`.
    cells: Bounds<Pixels>,
    status: Bounds<Pixels>,
    /// The search field's magnifying glass.
    glass: Bounds<Pixels>,
    scale: f32,
}

/// The size to draw an emoji `wide` points across at `EMOJI` in a cell:
/// `EMOJI`, or less where that runs past the cell's room (a sequence
/// drawn as two glyphs where no colour font joins them: with no Noto
/// Color Emoji every emoji shows, and "face in clouds" ran 46 points
/// across, over the next cell).
fn emoji_size(wide: f32) -> f32 {
    fitted(wide, EMOJI, CELL - 2.0 * m::TIGHT)
}

/// The size to draw what is `wide` points across at `size` so it is no
/// wider than `room`: `size`, or less.
fn fitted(wide: f32, size: f32, room: f32) -> f32 {
    if wide > room { size * room / wide } else { size }
}

/// How far in from a well's edges its sunken frame's two lines reach at
/// `scale` (`style::sunken`: a device pixel each, two at 2×).
fn well_edge(scale: f32) -> f32 {
    2.0 * style::line_pixels(scale) / scale
}

/// Where the picker's parts go on a screen `screen` points big at
/// `scale`: how many rows of cells its grid shows, and the parts. The
/// well holds a row of cells inside its sunken frame's lines (a row as
/// wide as the well, the cells ran under them, cut, and the sixth row
/// under its foot: QA, 2026-11-28), and the field, tabs and status line
/// are as wide as it, the tabs over the cells' columns.
fn lay_out(screen: olo_kit::Size<Pixels>, scale: f32) -> (f32, Geometry) {
    let edge = well_edge(scale);
    let row_w = COLS as f32 * CELL;
    let grid_w = row_w + 2.0 * edge;
    let width = grid_w + 2.0 * m::SPACING;
    // As many rows as fit, up to six; a fifth of the way down, or higher
    // if it must.
    let fixed = m::SPACING + FIELD_HEIGHT + m::TIGHT + TAB + m::TIGHT + m::TIGHT + STATUS + m::TIGHT;
    let screen_h = f32::from(screen.height);
    let rows = ((screen_h - 2.0 * m::SPACING - fixed - 2.0 * edge - PEEK) / CELL).floor().clamp(2.0, ROWS);
    let grid_h = rows * CELL + PEEK + 2.0 * edge;
    let height = fixed + grid_h;
    let top = (screen_h * TOP).round().min(screen_h - height - m::SPACING).max(0.0);
    // (Centred; on a screen too narrow for that beside Deskbar, as far left
    // as it takes to keep clear of it: it sat over Deskbar's clock at 2×,
    // 720 points across. Opus's look, 2026-11-28.)
    let left = (f32::from(screen.width) - width) / 2.0;
    let left = left.min(f32::from(screen.width) - DESKBAR_ROOM - width).max(0.0);
    let panel = snapped(scale, Bounds { origin: point(px(left), px(top)), size: size(px(width), px(height)) });
    let left = panel.left() + px(m::SPACING);
    let field = Bounds { origin: point(left, panel.top() + px(m::SPACING)), size: size(px(grid_w), px(FIELD_HEIGHT)) };
    // (Its top on a device pixel, as its frame is drawn, and the cells
    // from there: a well 86 points down the panel is half a pixel off at
    // 1.25×. Its size is whole pixels already: rounding its far corner
    // too came out a pixel short.)
    let on_pixel = |b: Bounds<Pixels>| Bounds { origin: snapped(scale, b).origin, size: b.size };
    let tabs = on_pixel(Bounds { origin: point(left + px(edge), field.bottom() + px(m::TIGHT)), size: size(px(row_w), px(TAB)) });
    let grid = on_pixel(Bounds { origin: point(left, tabs.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(grid_h)) });
    let cells = Bounds { origin: point(grid.left() + px(edge), grid.top() + px(edge)), size: size(px(row_w), px(rows * CELL + PEEK)) };
    let status = Bounds { origin: point(left, grid.bottom() + px(m::TIGHT)), size: size(px(grid_w), px(STATUS)) };
    let glass = snapped_centred(scale, point(field.left() + px(6.0 + 8.0), field.center().y), 16.0, 16.0);
    (rows, Geometry { panel, field, tabs, grid, cells, status, glass, scale })
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

    /// The tones' boxes, at the status line's right: each edge once, on
    /// a device pixel, the one between two boxes shared by both (each box
    /// rounded on its own, two side by side shared a pixel at 120%, a
    /// press there in both: review, 2026-11-28).
    fn tones(&self) -> [(Bounds<Pixels>, Tone); 6] {
        let s = self.scale;
        let on = |v: f32| px((v * s).round() / s);
        let mid = f32::from(self.status.center().y);
        let (top, bottom) = (on(mid - TONE_BOX / 2.0), on(mid + TONE_BOX / 2.0));
        let left = f32::from(self.status.right()) - 6.0 * TONE_BOX;
        let edge = |k: usize| on(left + k as f32 * TONE_BOX);
        let mut k = 0;
        Tone::ALL.map(|t| {
            let b = Bounds::from_corners(point(edge(k), top), point(edge(k + 1), bottom));
            k += 1;
            (b, t)
        })
    }
}

struct Picker {
    query: Child<TextField>,
    query_text: String,
    all: Arc<Vec<Emoji>>,
    /// How they can be drawn (with the colour font, or not).
    drawn: Arc<data::Drawn>,
    loaded: bool,
    /// Still reading a while after it opened: "Loading…" (said only then,
    /// not flashed for the moment reading takes: Opus's look, 2026-11-28).
    slow: bool,
    recents: Arc<Mutex<Recents>>,
    tone: Tone,
    lines: Vec<Line>,
    cells: Vec<Cell>,
    /// Where each tab's heading is: (tab's emoji, its name, line).
    tabs: Vec<(SharedString, &'static str, usize)>,
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
    fn new(all: Option<Arc<Vec<Emoji>>>, drawn: Arc<data::Drawn>, recents: Arc<Mutex<Recents>>, cx: &mut Cx<Self>) -> Picker {
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
            slow: false,
            all: all.unwrap_or_default(),
            drawn,
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
        if !picker.loaded {
            cx.after(Duration::from_millis(300), PickerMsg::Slow);
        }
        picker
    }

    /// The tone emoji are shown in: the one chosen, where tones can be
    /// drawn; else none (the choice kept, for when the colour font is
    /// back: a hand and a swatch were shown, with nothing to set it back).
    fn tone(&self) -> Tone {
        if self.drawn.tones { self.tone } else { Tone(None) }
    }

    /// Every emoji drawn ahead, off the UI thread, the likeliest first (what
    /// was picked lately, then the groups in order): kept for the windows
    /// after, so a search shows them without drawing them in its frame.
    /// Once a scale and tone; let go ten minutes after the picker is put
    /// away (`Hub`).
    fn warm(&self, cx: &mut Cx<Self>) {
        let key = (cx.scale_factor().to_bits(), self.tone());
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
        let tone = self.tone();
        let recent = self.recents.lock().unwrap_or_else(|e| e.into_inner()).most(&self.all, RECENT);
        let texts: Vec<SharedString> = recent.iter().copied().chain((0..self.all.len()).filter(|i| !recent.contains(i))).map(|i| SharedString::from(self.all[i].glyph(tone))).collect();
        let font = style::ui_font(FontWeight::NORMAL);
        // (At the size each is drawn at in its cell, `emoji_size`: a colour
        // emoji is a quarter wider than its size, so it's drawn smaller,
        // and warmed at `EMOJI` the raster cache held none of them as
        // drawn: review, 2026-11-28. One measured stands for all: the
        // colour font draws each a glyph as wide.)
        let size = emoji_size(f32::from(style::text_width(SharedString::from("😀"), EMOJI, font.clone())));
        cx.warm_text(&font, px(size), texts);
        // The tabs, the hands and the one shown big.
        // (The tabs' icons as they are drawn, the colour font's or plain
        // ones; the hands only where tones are drawn.)
        cx.warm_text(&font, px(16.0), self.drawn.icons.iter().flatten().map(|i| SharedString::from(i.clone())).chain([SharedString::from(self.drawn.recent.clone())]).collect::<Vec<_>>());
        if self.drawn.tones {
            cx.warm_text(
                &font,
                px(TONE - 4.0),
                Tone::ALL.iter().filter_map(|t| emojis::get("✋").and_then(|h| t.0.map_or(Some(h), |s| h.with_skin_tone(s)))).map(|h| SharedString::from(h.as_str())).collect::<Vec<_>>(),
            );
        }
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
        let tone = self.tone();
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
            // (Only those that can be drawn here: one not among them, or in
            // a tone that can't be, was shown as the model wrote it.)
            let tones = self.drawn.tones;
            let found = found
                .iter()
                .filter_map(|s| {
                    // The model's own tone, else the one chosen.
                    let i = all.iter().position(|e| e.e.as_str() == s.emoji || e.e.skin_tones().is_some_and(|mut t| t.any(|t| t.as_str() == s.emoji)))?;
                    let text = if all[i].e.as_str() == s.emoji || !tones { all[i].glyph(tone).into() } else { s.emoji.clone().into() };
                    Some(Cell { text, emoji: Some(i), why: Some(s.why.clone().into()) })
                })
                .collect();
            row(&mut lines, &mut cells, found);
        }
        if !self.loaded {
            // (The tabs from the first frame, as soon as it's known how they
            // can be drawn; "Loading…" said in the well, if it takes a while:
            // `paint_grid`.)
            tabs.extend(GROUPS.iter().zip(&self.drawn.icons).filter_map(|((name, _), icon)| Some((SharedString::from(icon.clone()?), *name, usize::MAX))));
        } else if self.query_text.trim().is_empty() {
            let recent = recents.most(all, RECENT);
            if !recent.is_empty() {
                tabs.push((SharedString::from(self.drawn.recent.clone()), "Frequently Used", lines.len()));
                lines.push(Line::Header("Frequently Used".into()));
                row(&mut lines, &mut cells, recent.into_iter().map(cell).collect());
            }
            // (A group none of whose emoji can be drawn has no tab.)
            for (g, (name, _)) in GROUPS.iter().enumerate() {
                let Some(icon) = self.drawn.icons.get(g).cloned().flatten() else { continue };
                tabs.push((icon.into(), *name, lines.len()));
                lines.push(Line::Header((*name).into()));
                row(&mut lines, &mut cells, (0..all.len()).filter(|&i| all[i].group == g).map(cell).collect());
            }
        } else {
            // The tabs stay, to go back to browsing (`go_to` clears the
            // search, then finds where each group's heading is).
            if !recents.most(all, 1).is_empty() {
                tabs.push((SharedString::from(self.drawn.recent.clone()), "Frequently Used", usize::MAX));
            }
            tabs.extend(GROUPS.iter().zip(&self.drawn.icons).filter_map(|((name, _), icon)| Some((SharedString::from(icon.clone()?), *name, usize::MAX))));
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
        self.rows * CELL + PEEK
    }

    /// Tab `n`'s box: the tabs share the well's width evenly, however
    /// many (nine, with nothing picked lately, left a cell's gap over the
    /// last column: Opus's look, 2026-11-28), each edge on a device pixel
    /// and shared by the tabs either side.
    fn tab_bounds(&self, n: usize) -> Bounds<Pixels> {
        tab_box(self.geometry.tabs, self.tabs.len(), n, self.geometry.scale)
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
        // For an app that asked: told it, and done.
        if PICKING.load(Ordering::Relaxed) {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{text}");
            let _ = out.flush();
            return;
        }
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
        let inner = self.geometry.cells;
        if !inner.contains(&at) {
            return None;
        }
        let mut y = inner.top() - px(self.scroll);
        for line in &self.lines {
            let h = px(line.height());
            if y > inner.bottom() {
                break;
            }
            if let Line::Cells(r) = line
                && y + h >= inner.top()
            {
                for i in r.clone() {
                    let b = snapped(self.geometry.scale, Bounds { origin: point(inner.left() + px((i - r.start) as f32 * CELL), y), size: size(px(CELL), px(CELL)) });
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
            if let Some(n) = (0..self.tabs.len()).find(|&n| across(self.tab_bounds(n), e.position, n + 1 == self.tabs.len())) {
                self.go_to(n, cx);
            }
        } else if let Some(i) = self.cell_at(e.position) {
            self.choose(i, cx);
        } else if let Some((_, tone)) = g.tones().into_iter().enumerate().find(|(k, (b, _))| self.drawn.tones && across(*b, e.position, *k + 1 == Tone::ALL.len())).map(|(_, t)| t) {
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
        let current = self.current_tab();
        for (n, (icon, name, _)) in self.tabs.iter().enumerate() {
            let r = self.tab_bounds(n);
            let on = current == Some(n);
            if on {
                pressed(g, r, &p);
            }
            let mid = style::snap_coord(g, r.center().y);
            style::label(g, r.center().x, mid, Align::Center, icon.clone(), 16.0, font.clone(), c(p.panel_text));
            g.agent(agent::Node { role: "Tab", name: Some((*name).into()), bounds: r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
        }
    }

    /// The grid: the lines that show.
    fn paint_grid(&self, g: &mut Painter) {
        let p = theme::palette();
        let font = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let dim = style::disabled_text(&p);
        // Inside the well's frame, where the lines scroll: what it cuts
        // at its top and foot is scrolled out of sight, as a scroll
        // view's (a group's heading puts the rows half a cell off).
        let inner = self.geometry.cells;
        olo_kit::review::scroll_area(g, inner);
        // Still reading, a while since it opened: said in the middle of the
        // well (not at its corner as if typed).
        if !self.loaded {
            if self.slow {
                let b = style::label(g, inner.center().x, style::snap_coord(g, inner.center().y), Align::Center, "Loading…", style::PLAIN_SIZE, font.clone(), style::secondary_text(&p));
                g.agent(agent::Node::label("Loading…", b));
            }
            return;
        }
        let (scroll, selected) = (self.scroll, self.selected);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: inner }), |g| {
            let mut y = inner.top() - px(scroll);
            for line in &self.lines {
                let h = px(line.height());
                if y + h < inner.top() {
                    y += h;
                    continue;
                }
                if y > inner.bottom() {
                    break;
                }
                match line {
                    Line::Header(text) => {
                        let mid = style::snap_coord(g, y + h / 2.0);
                        let b = style::label(g, inner.left() + px(m::SPACING), mid, Align::Left, text.clone(), style::DETAIL_SIZE, bold.clone(), dim);
                        g.agent(agent::Node { role: "Heading", clip: Some(inner), ..agent::Node::label(text.clone(), b) });
                    }
                    Line::Note(text) => {
                        let mid = style::snap_coord(g, y + h / 2.0);
                        let b = style::label_fit(
                            g,
                            inner.left() + px(m::SPACING),
                            mid,
                            Align::Left,
                            text.clone(),
                            style::PLAIN_SIZE,
                            font.clone(),
                            c(p.document_text),
                            inner.size.width - px(2.0 * m::SPACING),
                        );
                        g.agent(agent::Node { clip: Some(inner), ..agent::Node::label(text.clone(), b) });
                    }
                    Line::Cells(r) => {
                        for i in r.clone() {
                            let col = (i - r.start) as f32;
                            let b = style::snap(g, Bounds { origin: point(inner.left() + px(col * CELL), y), size: size(px(CELL), px(CELL)) });
                            // (The cell the pointer comes over is chosen:
                            // only cells' edges matter to it.)
                            g.note_pointer_edge(b.intersect(&inner));
                            if i == selected {
                                let lit = style::inset(g, b, 2.0);
                                style::fill_rect(g, lit, c(p.list_selected_background));
                            }
                            let mid = style::snap_coord(g, b.center().y);
                            let cell = &self.cells[i];
                            // (Centred in a cell of its own, not on a line
                            // with its neighbours: one drawn smaller to fit
                            // sits lower.)
                            olo_kit::review::own_grid(g, b);
                            let size = emoji_size(f32::from(style::text_width(cell.text.clone(), EMOJI, font.clone())));
                            style::label(g, b.center().x, mid, Align::Center, cell.text.clone(), size, font.clone(), c(p.document_text));
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
        // (The big emoji centred in its box on the whole line, not on
        // either line of what it is beside it; the tones a group of their
        // own: each judged in itself, as a count's disc is.)
        // (Flush with the well's edge above, as the tabs and the cells.)
        let big_box = Bounds { origin: point(status.left(), status.top()), size: size(px(BIG_BOX), status.size.height) };
        olo_kit::review::container(g, big_box);
        if let Some(big) = &big {
            let size = fitted(f32::from(style::text_width(big.clone(), BIG, font.clone())), BIG, BIG_BOX);
            style::label(g, big_box.center().x, mid, Align::Center, big.clone(), size, font.clone(), c(p.panel_text));
        }
        // The tones, at the right (where a press finds them); where a tone
        // can't be drawn (no colour emoji font: a hand and a swatch), why
        // some can't be shown, there instead.
        let tones = self.geometry.tones();
        let (top_mid, detail_mid) = (mid - px(8.0), mid + px(8.0));
        // Where tones can't be drawn (no colour font), why some emoji can't
        // be shown, in the tones' place: two lines at most, at the right,
        // centred on the big emoji, Olo's info icon before them (on the
        // shortcode's line it read as the emoji's own: Opus's look,
        // 2026-11-28). Nothing there until it's known.
        let right = if self.drawn.tones {
            tones[0].0.left()
        } else if self.drawn.known && !self.drawn.colour {
            let note = olo_kit::words::tr("Colour emoji aren’t installed, so some can’t be shown.");
            let lines = wrapped(note, style::DETAIL_SIZE, &font, NOTICE_W, 2);
            let wide = lines.iter().map(|l| f32::from(style::text_width(l.clone(), style::DETAIL_SIZE, font.clone()))).fold(0.0, f32::max);
            let step = style::DETAIL_SIZE * style::LINE_HEIGHT;
            let left = status.right() - px(wide);
            let first = mid - px(step * (lines.len() as f32 - 1.0) / 2.0);
            let mut shown = Bounds { origin: point(left, first), size: size(px(wide), px(0.0)) };
            for (k, line) in lines.iter().enumerate() {
                let b =
                    style::label_fit(g, left, style::snap_coord(g, first + px(step * k as f32)), Align::Left, line.clone(), style::DETAIL_SIZE, font.clone(), style::secondary_text(&p), px(NOTICE_W));
                shown = shown.union(&b);
            }
            g.agent(agent::Node::label(note, shown));
            let icon = style::snap_centred(g, point(left - px(m::MARK_GAP + NOTICE_ICON / 2.0), mid), NOTICE_ICON, NOTICE_ICON);
            icons::paint(g, icon, "info");
            icon.left()
        } else {
            status.right() + px(m::SPACING)
        };
        let (title_right, detail_right) = (right, right);
        if self.drawn.tones {
            olo_kit::review::container(g, Bounds::from_corners(tones[0].0.origin, tones[tones.len() - 1].0.bottom_right()));
        }
        for (b, t) in tones.iter().filter(|_| self.drawn.tones) {
            let b = *b;
            if *t == self.tone {
                pressed(g, b, &p);
            }
            let hand = emojis::get("✋").and_then(|h| match t.0 {
                Some(s) => h.with_skin_tone(s),
                None => Some(h),
            });
            // (No wider than its box's room: with no colour font, a toned
            // hand is two glyphs, and ran into the next.)
            let hand = hand.map_or("✋", |h| h.as_str());
            let size = fitted(f32::from(style::text_width(hand.to_owned(), TONE - 4.0, font.clone())), TONE - 4.0, TONE_BOX - 8.0);
            style::label(g, b.center().x, mid, Align::Center, hand, size, font.clone(), c(p.panel_text));
            g.agent(agent::Node { role: "RadioButton", name: Some(t.name().into()), bounds: b, enabled: true, clickable: true, checked: Some(*t == self.tone), ..Default::default() });
        }
        // What it is, between: a mark's gap after the big emoji's box.
        let x = status.left() + px(BIG_BOX + m::MARK_GAP);
        let (room, detail_room) = (title_right - px(m::SPACING) - x, detail_right - px(m::SPACING) - x);
        // (Whole for agents, though cut short on screen.)
        let b = style::label_fit(g, x, top_mid, Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), c(p.panel_text), room);
        if !title.is_empty() {
            g.agent(agent::Node::label(title.clone(), b));
        }
        // (Secondary, not greyed out: it says what the emoji is called.)
        let b = style::label_fit(g, x, detail_mid, Align::Left, detail.clone(), style::DETAIL_SIZE, font.clone(), style::secondary_text(&p), detail_room);
        if !detail.is_empty() {
            g.agent(agent::Node::label(detail.clone(), b));
        }
    }
}

impl View for Picker {
    type Message = PickerMsg;

    fn message(&mut self, message: PickerMsg, cx: &mut Cx<Self>) {
        match message {
            PickerMsg::Loaded(all, drawn) => {
                self.all = all;
                self.drawn = drawn;
                self.loaded = true;
                self.rebuild();
                self.warm(cx);
            }
            PickerMsg::Drawn(drawn) => {
                if !self.loaded {
                    self.drawn = drawn;
                    self.rebuild();
                    cx.invalidate();
                }
            }
            PickerMsg::Slow => {
                self.slow = !self.loaded;
                cx.invalidate();
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
        let (rows, geometry) = lay_out(olo_kit::viewport_size(), cx.scale_factor());
        self.rows = rows;
        self.geometry = geometry;
        let Geometry { panel, field, grid, glass, .. } = geometry;
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
        let tab_strip = view_canvas(|p: &Picker, g, _| p.paint_tabs(g))
            .key_by(move |p: &Picker| style::key((geometry_key, p.current_tab(), p.tabs.iter().map(|t| (t.0.clone(), t.1)).collect::<Vec<_>>())))
            .absolute()
            .inset_0();
        let grid_paint =
            view_canvas(|p: &Picker, g, _| p.paint_grid(g)).key_by(move |p: &Picker| style::key((geometry_key, p.generation, p.scroll.to_bits(), p.selected, p.loaded, p.slow))).absolute().inset_0();
        let status_paint = view_canvas(|p: &Picker, g, _| p.paint_status(g))
            .key_by(move |p: &Picker| {
                let (title, detail, big) = p.status();
                style::key((geometry_key, title, detail, big, Tone::ALL.iter().position(|t| *t == p.tone), p.drawn.tones, p.drawn.known, p.drawn.colour))
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

/// Tab `n` of `count` sharing `tabs` evenly at `scale` (`Picker::tab_bounds`).
fn tab_box(tabs: Bounds<Pixels>, count: usize, n: usize, scale: f32) -> Bounds<Pixels> {
    let on = |v: f32| px((v * scale).round() / scale);
    let w = f32::from(tabs.size.width) / count.max(1) as f32;
    let x = |k: usize| on(f32::from(tabs.left()) + k as f32 * w);
    Bounds::from_corners(point(x(n), on(f32::from(tabs.top()))), point(x(n + 1), on(f32::from(tabs.bottom()))))
}

/// Whether `p` is in `b`, its right edge left to the box beside it (the
/// last's, `last`, its own): two boxes side by side share an edge, and a
/// press on it went to the first (review, 2026-11-28).
fn across(b: Bounds<Pixels>, p: Point<Pixels>, last: bool) -> bool {
    p.x >= b.left() && (p.x < b.right() || last && p.x <= b.right()) && p.y >= b.top() && p.y <= b.bottom()
}

/// `text` broken into lines `width` points across at `size`, `rows` at
/// most (the last cut short with "…" when there's more).
fn wrapped(text: &str, size: f32, font: &olo_kit::Font, width: f32, rows: usize) -> Vec<SharedString> {
    let mut wrapper = olo_kit::text_system().line_wrapper(font.clone(), px(size));
    let mut lines = Vec::new();
    let mut start = 0;
    while start < text.len() && lines.len() < rows {
        let rest = &text[start..];
        let end = if lines.len() + 1 == rows { rest.len() } else { wrapper.wrap_line(&[olo_kit::LineFragment::text(rest)], px(width)).next().map_or(rest.len(), |b| b.ix) };
        lines.push(SharedString::from(rest[..end].trim().to_owned()));
        start += end;
    }
    lines
}

/// A toggle that is on (the chosen group's tab, the chosen tone):
/// pressed into the panel as a held button is, in the panel's colour,
/// not the field white or the selection's blue, which is the grid's,
/// where the keys are (Opus's look, 2026-11-28).
fn pressed(g: &mut Painter, r: Bounds<Pixels>, p: &olo_kit::palette::Palette) {
    let bg = p.panel_background;
    style::fill_rect(g, r, style::shade(p, bg, 1.08));
    style::bevel(g, r, style::shade(p, bg, 1.3), c(p.shine));
}

/// "grinning face" → "Grinning face".
fn capitalised(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Picking for an app (`--pick`): what's chosen is printed, not put in.
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pick = args.iter().any(|a| a == "--pick");
    PICKING.store(pick, Ordering::Relaxed);
    if !pick && instance::forward(APP_ID, &args) {
        return;
    }
    let toggle = args.iter().any(|a| a == "--toggle") || pick;
    olo_kit::run(APP_ID, move |app| {
        if !pick {
            olo_kit::stay_resident();
            instance::listen(APP_ID, olo_kit::proxy());
        }
        // The emoji, and which the font draws: off the UI thread, once.
        let proxy = olo_kit::proxy();
        let _ = std::thread::Builder::new().name("emoji-load".into()).spawn(move || {
            // (The colour font as the picker reads it, else as the kit draws
            // it (a bitmap one), else what the others draw whole.)
            let (colour, draws) = data::how_drawn(data::font_check(), data::colour_check, data::fallback_check);
            proxy.send(Found::Drawn(data::Drawn::early(colour, &draws)));
            let all = data::load(&draws);
            let drawn = data::Drawn::new(&all, colour, &draws);
            proxy.send(Found::Emoji(all, drawn));
        });
        let hub = Rc::new(RefCell::new(Hub { all: None, drawn: Arc::new(data::Drawn::unknown()), recents: Arc::new(Mutex::new(Recents::load())), window: None, shown: 0 }));
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
                // (Picking for an app: put away, chosen or not, it's done.)
                Ok(_) if pick => {
                    // (After this turn: the line printed is flushed, the
                    // window gone.)
                    olo_kit::quit();
                    return;
                }
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
                Found::Drawn(drawn) => {
                    // (Not over what the emoji read brought.)
                    if hub.all.is_none() {
                        let drawn = Arc::new(drawn);
                        hub.drawn = drawn.clone();
                        if let Some(window) = hub.showing() {
                            window.messenger().send(PickerMsg::Drawn(drawn));
                        }
                    }
                }
                Found::Emoji(all, drawn) => {
                    let (all, drawn) = (Arc::new(all), Arc::new(drawn));
                    hub.all = Some(all.clone());
                    hub.drawn = drawn.clone();
                    if let Some(window) = hub.showing() {
                        window.messenger().send(PickerMsg::Loaded(all, drawn));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An emoji is drawn at its size unless that runs past its cell's
    /// room, then smaller, to fit.
    #[test]
    fn an_emoji_stays_in_its_cell() {
        let room = CELL - 2.0 * m::TIGHT;
        assert_eq!(emoji_size(24.0), EMOJI);
        assert_eq!(emoji_size(room), EMOJI);
        // Two glyphs, 46 points at EMOJI: as wide as the room.
        let size = emoji_size(46.0);
        assert!(size < EMOJI && (46.0 * size / EMOJI - room).abs() < 0.01, "{size}");
    }

    /// A colour emoji, a quarter wider than its size (30 points across at
    /// 24), is drawn at what fits its cell; warmed at that size too, not
    /// at `EMOJI`, so the drawing ahead is what's drawn.
    #[test]
    fn emoji_are_warmed_as_drawn() {
        let colour = 1.25 * EMOJI;
        let drawn = emoji_size(colour);
        assert!((drawn - 22.4).abs() < 0.01, "{drawn}");
        assert!(colour * drawn / EMOJI <= CELL - 2.0 * m::TIGHT + 0.01);
    }

    /// The tabs share the well's width evenly, nine or ten: no gap over
    /// the last column, each edge on a device pixel, shared by the tabs
    /// either side.
    #[test]
    fn the_tabs_fill_the_well() {
        for scale in [1.0f32, 1.2, 1.25, 1.5, 1.75, 2.0] {
            let (_, g) = lay_out(size(px(1440.0), px(900.0)), scale);
            for count in [9, 10, 7] {
                let tabs: Vec<_> = (0..count).map(|n| tab_box(g.tabs, count, n, scale)).collect();
                assert!(
                    (f32::from(tabs[0].left()) - f32::from(g.tabs.left())).abs() < 0.01 && (f32::from(tabs[count - 1].right()) - f32::from(g.tabs.right())).abs() < 0.01,
                    "at {scale}×, {count}: {tabs:?} in {:?}",
                    g.tabs
                );
                for k in 1..count {
                    assert_eq!(tabs[k - 1].right(), tabs[k].left(), "at {scale}×, {count}: tabs {} and {k}", k - 1);
                    let wide = f32::from(tabs[k].size.width) * scale;
                    assert!((wide - f32::from(g.tabs.size.width) * scale / count as f32).abs() <= 1.0, "at {scale}×, {count}: tab {k} {wide} device pixels");
                }
                for v in tabs.iter().flat_map(|t| [t.left(), t.right(), t.top(), t.bottom()]) {
                    let d = f32::from(v) * scale;
                    assert!((d - d.round()).abs() < 0.01, "at {scale}×: an edge {d} device pixels in");
                }
            }
        }
    }

    /// With no colour emoji font, each tab shows a plain symbol of its
    /// group that can be drawn (not ♿ for Symbols, nor the group's first
    /// emoji); tones can't be; the lately picked's icon is one that can
    /// be. With it, all as they are. Not known yet: no tab, tone or notice.
    #[test]
    fn without_colour_emoji_the_tabs_show_what_can_be_drawn() {
        // (One code point drawn whole; sequences, "✈️" and toned hands, not.)
        let draws = |c: &str| data::visible_codepoints(c) == 1 && c.chars().count() == 1;
        let all = data::load(draws);
        let drawn = data::Drawn::new(&all, false, draws);
        assert!(drawn.known && !drawn.colour && !drawn.tones);
        for icon in &drawn.icons {
            let icon = icon.as_deref().expect("every group has one drawn");
            assert!(draws(icon), "{icon}");
        }
        assert_eq!(drawn.icons[7].as_deref(), Some("♻"));
        assert_ne!(drawn.icons[4].as_deref(), Some("✈️"));
        assert_eq!(drawn.recent, "🕘");
        // Early (before the emoji are read): the same plain icons.
        assert_eq!(data::Drawn::early(false, draws).icons, drawn.icons);
        assert_eq!(data::Drawn::new(&data::load(|_| true), true, |_| true), data::Drawn::colour());
    }

    /// A colour font the picker can't read itself (a bitmap one, CBDT, as
    /// Debian's and Ubuntu's Noto Color Emoji is) is a colour font all the
    /// same where the kit draws in colour with it: tabs their own, tones,
    /// no notice (it said colour emoji weren't installed over a grid of
    /// them, 2026-11-28). The one it reads comes first; with neither, what
    /// the other fonts draw.
    #[test]
    fn a_colour_font_the_kit_draws_is_one() {
        let none = None::<fn(&str) -> bool>;
        let (colour, draws) = data::how_drawn(none, || Some(|c: &str| c != "🫩"), || |_: &str| false);
        assert!(colour && draws("😀") && draws("✋🏽") && !draws("🫩"));
        let drawn = data::Drawn::new(&data::load(&draws), colour, &draws);
        assert_eq!(drawn, data::Drawn::colour());
        let (colour, draws) = data::how_drawn(Some(|_: &str| true), || -> Option<fn(&str) -> bool> { panic!("the kit asked with a font read") }, || |_: &str| false);
        assert!(colour && draws("🫩"));
        let (colour, draws) = data::how_drawn(none, || None::<fn(&str) -> bool>, || |c: &str| c == "☺");
        assert!(!colour && draws("☺") && !draws("😀"));
        let unknown = data::Drawn::unknown();
        assert!(!unknown.known && !unknown.tones && unknown.icons.iter().all(Option::is_none));
    }

    /// A sequence a font draws as one glyph is still more than one thing to
    /// draw; joiners and variation selectors draw nothing.
    #[test]
    fn what_a_cluster_draws() {
        assert_eq!(data::visible_codepoints("☺️"), 1);
        assert_eq!(data::visible_codepoints("1️⃣"), 2);
        assert_eq!(data::visible_codepoints("👨‍👩‍👧"), 3);
        assert_eq!(data::visible_codepoints("✋🏽"), 2);
    }

    /// A press on the edge two boxes share is the right-hand one's; the
    /// last box's right edge is its own.
    #[test]
    fn a_press_on_a_shared_edge_is_one_boxs() {
        let (_, g) = lay_out(size(px(1440.0), px(900.0)), 1.0);
        let tones = g.tones();
        let edge = point(tones[1].0.left(), tones[1].0.center().y);
        let hit: Vec<usize> = (0..tones.len()).filter(|&k| across(tones[k].0, edge, k + 1 == tones.len())).collect();
        assert_eq!(hit, [1]);
        let last = tones.len() - 1;
        assert!(across(tones[last].0, point(tones[last].0.right(), tones[last].0.center().y), true));
    }

    /// The panel keeps clear of Deskbar, 8 points from it: on 720 × 450
    /// (2×) it moves left of the middle; on a wide screen it stays there.
    #[test]
    fn the_panel_keeps_clear_of_deskbar() {
        let (_, g) = lay_out(size(px(720.0), px(450.0)), 2.0);
        assert!(f32::from(g.panel.right()) <= 720.0 - m::DESKBAR_WIDTH - 8.0 + 0.01, "{:?}", g.panel);
        assert!(g.panel.left() >= px(0.0));
        let (_, g) = lay_out(size(px(1440.0), px(900.0)), 1.0);
        assert!((f32::from(g.panel.center().x) - 720.0).abs() <= 1.0, "{:?}", g.panel);
    }

    /// The tones' boxes sit in the status line side by side, each with
    /// room either side of its hand (a colour emoji is 1.28 of its size
    /// across: 3.5 points either side at least, less a device pixel's
    /// rounding), at every scale.
    #[test]
    fn the_tones_have_room() {
        for scale in [1.0f32, 1.2, 1.25, 1.5, 1.75, 2.0] {
            let (_, g) = lay_out(size(px(1440.0), px(900.0)), scale);
            let tones = g.tones();
            for (i, (b, _)) in tones.iter().enumerate() {
                assert!(
                    b.left() >= g.status.left() && b.right() <= g.status.right() + px(0.01) && b.top() >= g.status.top() && b.bottom() <= g.status.bottom(),
                    "at {scale}×: {b:?} out of {:?}",
                    g.status
                );
                assert!(f32::from(b.size.width) + 1.0 / scale >= (TONE - 4.0) * 1.28 + 2.0 * 3.5, "at {scale}×: a box {:?} wide", b.size.width);
                if i > 0 {
                    assert!(tones[i - 1].0.right() <= b.left() + px(0.01), "at {scale}×: tones {} and {i} overlap", i - 1);
                }
                for v in [b.left(), b.right(), b.top(), b.bottom()] {
                    let d = f32::from(v) * scale;
                    assert!((d - d.round()).abs() < 0.01, "at {scale}×: tone {i}'s edge {d} device pixels in");
                }
            }
        }
    }

    /// At every scale and on screens small and large, the cells are
    /// inside the well's sunken frame (none under its lines), a row of
    /// them exactly across it and its rows exactly down it, the tabs over
    /// their columns, and every edge on a device pixel.
    #[test]
    fn the_cells_are_inside_the_well() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            for (w, h) in [(1280.0, 800.0), (1440.0, 900.0), (720.0, 450.0), (640.0, 400.0)] {
                let (rows, g) = lay_out(size(px(w), px(h)), scale);
                let at = format!("at {scale}× on {w} × {h}");
                let edge = well_edge(scale);
                let same = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 0.01;
                assert!(same(g.cells.left(), g.grid.left() + px(edge)) && same(g.cells.right(), g.grid.right() - px(edge)), "{at}: {:?} in {:?}", g.cells, g.grid);
                assert!(same(g.cells.top(), g.grid.top() + px(edge)) && same(g.cells.bottom(), g.grid.bottom() - px(edge)), "{at}: {:?} in {:?}", g.cells, g.grid);
                assert_eq!((f32::from(g.cells.size.width), f32::from(g.cells.size.height)), (COLS as f32 * CELL, rows * CELL + PEEK), "{at}");
                assert!(same(g.tabs.left(), g.cells.left()) && same(g.tabs.size.width, g.cells.size.width), "{at}");
                assert!(rows >= 2.0 && f32::from(g.panel.bottom()) <= h, "{at}: {rows} rows, the panel to {:?}", g.panel.bottom());
                for v in [g.grid.left(), g.grid.right(), g.grid.top(), g.grid.bottom(), g.cells.left(), g.cells.right(), g.cells.top(), g.cells.bottom(), g.tabs.left(), g.tabs.top()] {
                    let d = f32::from(v) * scale;
                    assert!((d - d.round()).abs() < 0.01, "{at}: an edge {d} device pixels in");
                }
            }
        }
    }
}
