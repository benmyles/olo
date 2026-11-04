//! The overview (⌃Space): every display's workspaces at once, each a
//! pane with its windows spread out so none covers another, all of them
//! live (they go on drawing, those on other workspaces too). Clicking a
//! window, or typing the letter on it, makes it the active window, on its
//! workspace, and leaves; the arrows walk the windows and Return picks
//! one. Esc, ⌃Space again or a click on nothing leaves everything as it
//! was.
//!
//! BeOS had nothing like it: its Workspaces applet draws every
//! workspace's windows as outlines, too small to tell apart. This is
//! macOS's Mission Control with every workspace out together, lettered
//! as keyboard hints letter things (`hints.rs`). Windows on the workspace
//! showing glide from where they are into place and back; the rest, the
//! panes and the letters fade in and out.
//!
//! Agents: `{"type":"overview"}` says what it shows (each window's place
//! and letter, each pane), `{"show":true}` opens it and `false` leaves it.

use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportAll, ImportMem, Renderer,
            element::{
                Kind,
                memory::MemoryRenderBuffer,
                utils::{ConstrainAlign, ConstrainScaleBehavior, constrain_as_render_elements},
            },
        },
    },
    desktop::space::SpaceElement,
    output::Output,
    utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Size, Transform},
};
use tiny_skia::Pixmap;

use crate::{
    render::{CustomRenderElements, OutputRenderElements},
    shell::{WindowElement, WindowRenderElement},
    state::{AnvilState, Backend},
    text, theme,
};

/// How long opening and leaving take.
const OPEN: Duration = Duration::from_millis(220);
const CLOSE: Duration = Duration::from_millis(180);
/// Logical sizes: around the panes, between them, the line naming each
/// above it, and between windows in a pane.
const MARGIN: f64 = 32.0;
const GAP: f64 = 24.0;
const LABEL: f64 = 24.0;
const SPACING: f64 = 16.0;
const FONT: f32 = 12.0;
/// How far in from a window's foot its title's plate (`tabtip::plate`)
/// sits.
const PLATE_IN: f64 = 8.0;
/// How far in from a window's corner its letter sits.
const LETTER_IN: f64 = 6.0;
/// The ring round the window chosen: how far out and how thick.
const RING_OUT: f64 = 3.0;
const RING: f64 = 2.0;
/// How dark the desktop goes behind it, and how light a pane is on it.
const DIM: f32 = 0.5;
const PANE_FILL: f32 = 0.08;
const PANE_LINE: f32 = 0.35;

pub type Rect = Rectangle<f64, Logical>;

/// A workspace on a display.
#[derive(Debug, Clone)]
pub struct Pane {
    pub output: String,
    pub workspace: u32,
    /// What it is called: "Workspace 2", or with several displays,
    /// which one's ("D1 Workspace 2").
    pub name: String,
    pub rect: Rect,
    /// The workspace its display shows.
    pub current: bool,
}

/// A window, where it goes.
#[derive(Debug, Clone)]
pub struct Cell {
    pub window: WindowElement,
    pub output: String,
    pub workspace: u32,
    /// Where it is on screen (its frame, tab and all), if it shows: it
    /// glides from there; one on another workspace fades in.
    pub from: Option<Rect>,
    pub to: Rect,
    pub letter: Option<char>,
    /// Its place in the stacking, front highest, while it glides.
    z: usize,
}

impl std::fmt::Debug for Overview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overview").field("panes", &self.panes.len()).field("cells", &self.cells.len()).field("closing", &self.closing).finish()
    }
}

pub struct Overview {
    opened: Instant,
    /// Leaving: since when, and how far out it was then.
    closing: Option<(Instant, f64)>,
    pub panes: Vec<Pane>,
    pub cells: Vec<Cell>,
    /// The window under the pointer.
    pub hovered: Option<u64>,
    /// The window the arrows chose (until the pointer moves).
    pub selected: Option<u64>,
    /// Keys and buttons it took, whose releases are its too.
    swallowed: Vec<u32>,
    buttons: Vec<u32>,
    /// Something else chose a window or workspace: it leaves next frame.
    interrupted: bool,
    drawn: RefCell<HashMap<String, Drawn>>,
}

/// One display's pictures, made for its scale and the look.
#[derive(Default)]
struct Drawn {
    key: (i64, u64),
    badges: HashMap<char, (MemoryRenderBuffer, (i32, i32))>,
    names: HashMap<(String, bool), (MemoryRenderBuffer, (i32, i32))>,
    title: Option<((u64, String, i32), (MemoryRenderBuffer, (i32, i32)))>,
    /// Single pixels stretched into lines and fills, each used once a
    /// frame (an element's id is its buffer's).
    solids: HashMap<[u8; 4], Vec<MemoryRenderBuffer>>,
}

/// Ease out: fast, then settling.
fn ease(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

fn lerp(a: Rect, b: Rect, t: f64) -> Rect {
    let m = |x: f64, y: f64| x + (y - x) * t;
    Rectangle::new((m(a.loc.x, b.loc.x), m(a.loc.y, b.loc.y)).into(), (m(a.size.w, b.size.w), m(a.size.h, b.size.h)).into())
}

/// Where `n` panes go in `area`, each `aspect` (width / height) with a
/// line for its name above it: in the grid that makes them biggest,
/// centred, a short last row centred too.
pub fn panes(area: Rect, n: usize, aspect: f64) -> Vec<Rect> {
    if n == 0 || aspect <= 0.0 {
        return Vec::new();
    }
    let mut best = (f64::MIN, 1);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let w = (area.size.w - (cols - 1) as f64 * GAP) / cols as f64;
        let h = (area.size.h - rows as f64 * LABEL - (rows - 1) as f64 * GAP) / rows as f64;
        let pw = w.min(h * aspect);
        if pw > best.0 {
            best = (pw, cols);
        }
    }
    let (pw, cols) = (best.0.max(0.0), best.1);
    let ph = pw / aspect;
    let rows = n.div_ceil(cols);
    let height = rows as f64 * (LABEL + ph) + (rows - 1) as f64 * GAP;
    let top = area.loc.y + (area.size.h - height) / 2.0;
    (0..n)
        .map(|i| {
            let (row, col) = (i / cols, i % cols);
            let in_row = if row == rows - 1 { n - row * cols } else { cols };
            let width = in_row as f64 * pw + (in_row - 1) as f64 * GAP;
            let left = area.loc.x + (area.size.w - width) / 2.0;
            let y = top + row as f64 * (LABEL + ph + GAP) + LABEL;
            Rectangle::new((left + col as f64 * (pw + GAP), y).into(), (pw, ph).into())
        })
        .collect()
}

/// Where each window (`frames`, where they are) goes in `pane`: all at
/// one scale, at most `most`, in rows in the order they stand (top to
/// bottom, then left to right), each row and the whole centred.
pub fn spread(pane: Rect, frames: &[Rect], most: f64) -> Vec<Rect> {
    if frames.is_empty() {
        return Vec::new();
    }
    let middle = |r: &Rect| (r.loc.y + r.size.h / 2.0, r.loc.x + r.size.w / 2.0);
    let mut order: Vec<usize> = (0..frames.len()).collect();
    // (total_cmp: a sort panics on an order that isn't total, a NaN's.)
    order.sort_by(|&a, &b| {
        let (a, b) = (middle(&frames[a]), middle(&frames[b]));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    let rows_at = |s: f64| -> Option<Vec<Vec<usize>>> {
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut width = 0.0;
        for &i in &order {
            let w = frames[i].size.w * s;
            if w > pane.size.w {
                return None;
            }
            match rows.last_mut() {
                Some(row) if width + SPACING + w <= pane.size.w => {
                    row.push(i);
                    width += SPACING + w;
                }
                _ => {
                    rows.push(vec![i]);
                    width = w;
                }
            }
        }
        let tallest = |row: &Vec<usize>| row.iter().map(|&i| frames[i].size.h * s).fold(0.0, f64::max);
        let height = rows.iter().map(tallest).sum::<f64>() + (rows.len() - 1) as f64 * SPACING;
        (height <= pane.size.h).then_some(rows)
    };
    let (mut lo, mut hi) = (0.0, most);
    if rows_at(hi).is_some() {
        lo = hi;
    } else {
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if rows_at(mid).is_some() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    let s = lo;
    let rows = rows_at(s).unwrap_or_else(|| vec![order.clone()]);
    let heights: Vec<f64> = rows.iter().map(|row| row.iter().map(|&i| frames[i].size.h * s).fold(0.0, f64::max)).collect();
    let total = heights.iter().sum::<f64>() + (rows.len() - 1) as f64 * SPACING;
    let mut y = pane.loc.y + (pane.size.h - total) / 2.0;
    let mut out = vec![Rect::default(); frames.len()];
    for (row, h) in rows.iter().zip(heights) {
        let width = row.iter().map(|&i| frames[i].size.w * s).sum::<f64>() + (row.len() - 1) as f64 * SPACING;
        let mut x = pane.loc.x + (pane.size.w - width) / 2.0;
        for &i in row {
            let (w, wh) = (frames[i].size.w * s, frames[i].size.h * s);
            out[i] = Rectangle::new((x, y + (h - wh) / 2.0).into(), (w, wh).into());
            x += w + SPACING;
        }
        y += h + SPACING;
    }
    out
}

impl Overview {
    /// How far out it is: 0 the desktop as it is, 1 all out.
    pub fn progress(&self, now: Instant) -> f64 {
        let open = ease(now.saturating_duration_since(self.opened).as_secs_f64() / OPEN.as_secs_f64());
        match self.closing {
            Some((at, from)) => from * (1.0 - ease(now.saturating_duration_since(at).as_secs_f64() / CLOSE.as_secs_f64())),
            None => open,
        }
    }

    pub fn leaving(&self) -> bool {
        self.closing.is_some()
    }

    /// Gone: left, and back to the desktop.
    fn done(&self, now: Instant) -> bool {
        self.closing.is_some_and(|(at, _)| now.saturating_duration_since(at) >= CLOSE)
    }

    pub fn shows(&self, output: &str) -> bool {
        self.panes.iter().any(|p| p.output == output)
    }

    /// The window at `pos` (global), at rest.
    fn cell_at(&self, pos: Point<f64, Logical>) -> Option<&Cell> {
        self.cells.iter().find(|c| c.to.contains(pos))
    }

    /// The window ringed: the pointer's, else the arrows'.
    fn chosen(&self) -> Option<u64> {
        self.hovered.or(self.selected)
    }

    /// What it shows on `output` (its top left at `origin`, global; at
    /// `scale`), front first: the letters, the chosen window's title and
    /// ring, the windows, the panes and their names, then the dim over
    /// the desktop.
    pub fn elements<R>(&self, renderer: &mut R, output: &Output, origin: Point<i32, Logical>, screen: Size<i32, Logical>) -> Vec<OutputRenderElements<R, WindowRenderElement<R>>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Clone + Send + 'static,
    {
        let name = output.name();
        let scale = output.current_scale().fractional_scale();
        let now = Instant::now();
        let t = self.progress(now);
        let fade = t as f32;
        let palette = theme::palette();
        let mut drawn = self.drawn.borrow_mut();
        let drawn = drawn.entry(name.clone()).or_default();
        let key = ((scale * 1000.0).round() as i64, theme::generation());
        if drawn.key != key {
            *drawn = Drawn { key, ..Default::default() };
        }
        let mut used: HashMap<[u8; 4], usize> = HashMap::new();
        // Global logical to this display's device pixels.
        let phys = |x: f64, y: f64| -> Point<f64, Physical> { (((x - origin.x as f64) * scale).round(), ((y - origin.y as f64) * scale).round()).into() };
        let device = |r: Rect| -> Rectangle<i32, Physical> {
            let a = phys(r.loc.x, r.loc.y).to_i32_round::<i32>();
            let z = phys(r.loc.x + r.size.w, r.loc.y + r.size.h).to_i32_round::<i32>();
            Rectangle::new(a, (z.x - a.x, z.y - a.y).into())
        };
        let line = (scale.floor() as i32).max(1);
        let mut solid = |renderer: &mut R, drawn: &mut Drawn, c: [u8; 4], rect: Rectangle<i32, Physical>, alpha: f32| {
            let n = used.entry(c).or_insert(0);
            let list = drawn.solids.entry(c).or_default();
            if list.len() <= *n {
                list.push(MemoryRenderBuffer::from_slice(&c, Fourcc::Abgr8888, (1, 1), 1, Transform::Normal, Some(vec![Rectangle::from_size((1, 1).into())])));
            }
            let buffer = &list[*n];
            *n += 1;
            crate::render::exact(renderer, rect, buffer, (1, 1), Some(alpha), Kind::Unspecified).map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e)))
        };
        let mut front = Vec::new();
        let mut windows = Vec::new();
        let mut back = Vec::new();
        let cells: Vec<&Cell> = {
            let mut c: Vec<&Cell> = self.cells.iter().filter(|c| c.output == name && c.window.alive()).collect();
            c.sort_by_key(|c| std::cmp::Reverse(c.z));
            c
        };
        let settled = !self.leaving();
        let chosen = self.chosen();
        for cell in &cells {
            let rect = match cell.from {
                Some(from) => lerp(from, cell.to, t),
                None => cell.to,
            };
            let alpha = if cell.from.is_some() { 1.0 } else { fade };
            let reference = SpaceElement::bbox(&cell.window).to_physical_precise_round(scale);
            let constrain = device(rect);
            if constrain.size.w <= 0 || constrain.size.h <= 0 {
                continue;
            }
            windows.extend(
                // Drawn with its frame's corner at the cell's, then scaled
                // about it (as smithay's `constrain_space_element`).
                constrain_as_render_elements(&cell.window, renderer, constrain.loc - reference.loc, alpha, constrain, reference, ConstrainScaleBehavior::Fit, ConstrainAlign::CENTER, scale)
                    .map(OutputRenderElements::Preview),
            );
            // Its letter, at its top left.
            if let Some(c) = cell.letter {
                let (buffer, (w, h)) = drawn.badges.entry(c).or_insert_with(|| picture(&crate::hints::badge_pixmap(&palette, c, scale), true));
                let at = phys(rect.loc.x + LETTER_IN, rect.loc.y + LETTER_IN);
                front.extend(crate::render::picture(renderer, at, buffer, (*w, *h), Some(fade), Kind::Unspecified).map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))));
            }
            // The one chosen: ringed, its title whole on a plate at its
            // foot: as wide as it needs, past the window's sides and kept
            // on the display, cut short only past the display's width.
            if settled && chosen == Some(cell.window.id()) {
                let max_w = ((screen.w as f64 - 2.0 * crate::tabtip::MARGIN) * scale).floor() as i32;
                let want = (cell.window.id(), cell.window.title(), max_w);
                if drawn.title.as_ref().is_none_or(|(k, _)| *k != want) {
                    let pm = crate::tabtip::plate(&palette, &want.1, max_w, scale);
                    drawn.title = pm.map(|pm| (want.clone(), picture(&pm, true)));
                }
                if let Some((_, (buffer, (w, h)))) = &drawn.title {
                    let pw = *w as f64 / scale;
                    let (left, right) = (origin.x as f64 + crate::tabtip::MARGIN, (origin.x + screen.w) as f64 - crate::tabtip::MARGIN - pw);
                    let x = (rect.loc.x + rect.size.w / 2.0 - pw / 2.0).clamp(left, right.max(left));
                    let y = rect.loc.y + rect.size.h - PLATE_IN - *h as f64 / scale;
                    front.extend(crate::render::picture(renderer, phys(x, y), buffer, (*w, *h), Some(fade), Kind::Unspecified).map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))));
                }
                let out = Rectangle::new((rect.loc.x - RING_OUT, rect.loc.y - RING_OUT).into(), (rect.size.w + 2.0 * RING_OUT, rect.size.h + 2.0 * RING_OUT).into());
                let r = device(out);
                let k = ((RING * scale).round() as i32).max(1);
                for edge in [
                    Rectangle::new(r.loc, (r.size.w, k).into()),
                    Rectangle::new((r.loc.x, r.loc.y + r.size.h - k).into(), (r.size.w, k).into()),
                    Rectangle::new(r.loc, (k, r.size.h).into()),
                    Rectangle::new((r.loc.x + r.size.w - k, r.loc.y).into(), (k, r.size.h).into()),
                ] {
                    front.extend(solid(renderer, drawn, palette.keyboard_navigation, edge, fade));
                }
            }
        }
        // The panes: named above, a light fill, a line round (the
        // workspace showing in the focus colour).
        for pane in self.panes.iter().filter(|p| p.output == name) {
            let (buffer, (w, h)) = drawn.names.entry((pane.name.clone(), pane.current)).or_insert_with(|| picture(&name_pixmap(&palette, &pane.name, pane.current, scale), false));
            let at = phys(pane.rect.loc.x, pane.rect.loc.y - (LABEL + *h as f64 / scale) / 2.0);
            back.extend(crate::render::picture(renderer, at, buffer, (*w, *h), Some(fade), Kind::Unspecified).map(|e| OutputRenderElements::Custom(CustomRenderElements::Overlay(e))));
            let r = device(pane.rect);
            let (c, k, a) = if pane.current { (palette.keyboard_navigation, ((RING * scale).round() as i32).max(1), 1.0) } else { (palette.desktop_text, line, PANE_LINE) };
            for edge in [
                Rectangle::new(r.loc, (r.size.w, k).into()),
                Rectangle::new((r.loc.x, r.loc.y + r.size.h - k).into(), (r.size.w, k).into()),
                Rectangle::new(r.loc, (k, r.size.h).into()),
                Rectangle::new((r.loc.x + r.size.w - k, r.loc.y).into(), (k, r.size.h).into()),
            ] {
                back.extend(solid(renderer, drawn, c, edge, fade * a));
            }
            back.extend(solid(renderer, drawn, [255, 255, 255, 255], r, fade * PANE_FILL));
        }
        let whole = device(Rectangle::new(origin.to_f64(), screen.to_f64()));
        back.extend(solid(renderer, drawn, [0, 0, 0, 255], whole, fade * DIM));
        front.extend(windows);
        front.extend(back);
        front
    }
}

/// `pm` as a buffer, said to be `opaque` throughout or not.
fn picture(pm: &Pixmap, opaque: bool) -> (MemoryRenderBuffer, (i32, i32)) {
    let size: Size<i32, Buffer> = (pm.width() as i32, pm.height() as i32).into();
    let b = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, opaque.then(|| vec![Rectangle::from_size(size)]));
    (b, (size.w, size.h))
}

/// A pane's name, on the desktop as its icons' are (shadowed); bold for
/// the workspace showing.
fn name_pixmap(p: &olo_theme::Palette, name: &str, bold: bool, s: f64) -> Pixmap {
    let size = FONT * s as f32;
    let u = (s.round() as i32).max(1);
    let w = text::measure(name, size, bold).ceil() as i32 + u;
    let h = (18.0 * s).round() as i32;
    let mut pm = Pixmap::new(w.max(1) as u32, h.max(1) as u32).expect("overview name");
    let y = text::top_for_centre(h as f32 / 2.0, size, bold).round();
    text::draw(&mut pm, name, u as f32, y + u as f32, size, bold, p.desktop_text_shadow);
    text::draw(&mut pm, name, 0.0, y, size, bold, p.desktop_text);
    pm
}

impl<B: Backend + 'static> AnvilState<B> {
    /// ⌃Space: out, or back.
    pub fn toggle_overview(&mut self) {
        if self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            self.close_overview(None);
        } else {
            self.open_overview();
        }
    }

    pub fn open_overview(&mut self) {
        if self.lock.locked() || self.twitch.is_some() || self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            return;
        }
        if self.hints.active() {
            self.hints.close();
        }
        let windows = self.all_windows();
        let infos = self.output_infos();
        let stacking: Vec<WindowElement> = self.space.elements().cloned().collect();
        let mut outputs: Vec<Output> = self.space.outputs().cloned().collect();
        outputs.sort_by_key(|o| self.space.output_geometry(o).map(|g| (g.loc.x, g.loc.y)));
        let mut panes_out = Vec::new();
        let mut cells = Vec::new();
        for output in &outputs {
            let (Some(geo), Some(area)) = (self.space.output_geometry(output), self.usable_area(output)) else { continue };
            // Each display has its own workspaces.
            let current = self.showing(output);
            let number = olo_protocol::display_number(&infos, &output.name());
            let area = area.to_f64();
            let area = Rectangle::new((area.loc.x + MARGIN, area.loc.y + MARGIN).into(), ((area.size.w - 2.0 * MARGIN).max(1.0), (area.size.h - 2.0 * MARGIN).max(1.0)).into());
            // Its windows: (window, workspace, frame, where it shows).
            let mine: Vec<(WindowElement, u32, Rect, Option<Rect>)> = windows
                .iter()
                .filter(|w| w.alive() && !w.meta().minimized.get() && !(w.decorated() && w.decoration().behind))
                .filter(|w| self.display_of(w).as_ref() == Some(output))
                .map(|w| {
                    let ws = self.workspace_of(w);
                    let shown = self.space.element_bbox(w).map(|b| b.to_f64());
                    let frame = shown.unwrap_or_else(|| {
                        let b = SpaceElement::bbox(w);
                        let at = w.meta().saved_location.get() - SpaceElement::geometry(w).loc + b.loc;
                        Rectangle::new(at, b.size).to_f64()
                    });
                    (w.clone(), ws, frame, shown)
                })
                .collect();
            let mut workspaces: Vec<u32> = mine.iter().map(|m| m.1).chain([current]).collect();
            workspaces.sort();
            workspaces.dedup();
            let aspect = geo.size.w as f64 / geo.size.h.max(1) as f64;
            for (ws, rect) in workspaces.iter().zip(panes(area, workspaces.len(), aspect)) {
                let in_it: Vec<_> = mine.iter().filter(|m| m.1 == *ws).collect();
                let frames: Vec<Rect> = in_it.iter().map(|m| m.2).collect();
                let most = rect.size.w / geo.size.w.max(1) as f64;
                // Kept off the pane's edge, by as much as from each other.
                let inside = Rectangle::new((rect.loc.x + SPACING, rect.loc.y + SPACING).into(), ((rect.size.w - 2.0 * SPACING).max(1.0), (rect.size.h - 2.0 * SPACING).max(1.0)).into());
                // In reading order, so the letters run as the eye does (a
                // row's windows share their middle).
                let mut placed: Vec<_> = in_it.iter().zip(spread(inside, &frames, most)).collect();
                placed.sort_by(|a, b| (a.1.loc.y + a.1.size.h / 2.0).total_cmp(&(b.1.loc.y + b.1.size.h / 2.0)).then(a.1.loc.x.total_cmp(&b.1.loc.x)));
                for (m, to) in placed {
                    let z = stacking.iter().position(|s| s == &m.0).map_or(0, |i| i + 1);
                    cells.push(Cell { window: m.0.clone(), output: output.name(), workspace: *ws, from: m.3, to, letter: None, z });
                }
                let name = olo_protocol::workspace_name(number, *ws);
                panes_out.push(Pane { output: output.name(), workspace: *ws, name, rect, current: *ws == current });
            }
        }
        for (cell, c) in cells.iter_mut().zip(crate::hints::LETTERS.chars()) {
            cell.letter = Some(c);
        }
        let focused = self.focused_window().map(|w| w.id());
        self.overview = Some(Overview {
            opened: Instant::now(),
            closing: None,
            panes: panes_out,
            cells,
            hovered: None,
            selected: focused,
            swallowed: Vec::new(),
            buttons: Vec::new(),
            interrupted: false,
            drawn: RefCell::new(HashMap::new()),
        });
        // Off whatever window the pointer was over.
        self.warp_pointer(self.pointer.current_location());
        self.overview_hover();
    }

    /// Leave: with `chosen`, that window made the active one (on its
    /// workspace); without, everything as it was.
    pub fn close_overview(&mut self, chosen: Option<WindowElement>) {
        let now = Instant::now();
        let Some(o) = self.overview.as_mut().filter(|o| !o.leaving()) else { return };
        // Leaving first, so what the choice sets off doesn't leave again.
        o.closing = Some((now, o.progress(now)));
        if let Some(w) = chosen.filter(|w| w.alive()) {
            self.restore(&w);
        }
        // The windows showing now glide back to their places.
        let shown: Vec<(u64, Option<Rect>)> =
            self.overview.as_ref().map(|o| o.cells.iter().map(|c| (c.window.id(), self.space.element_bbox(&c.window).map(|b| b.to_f64()))).collect()).unwrap_or_default();
        let stacking: Vec<WindowElement> = self.space.elements().cloned().collect();
        if let Some(o) = self.overview.as_mut() {
            for (cell, (_, from)) in o.cells.iter_mut().zip(shown) {
                cell.from = from;
                cell.z = stacking.iter().position(|s| s == &cell.window).map_or(0, |i| i + 1);
            }
        }
    }

    /// Something else chose a window or a workspace (Deskbar, an agent,
    /// a shortcut): it steps aside, once that is done, gliding back to the
    /// desktop as it is then.
    pub fn overview_interrupted(&mut self) {
        if let Some(o) = self.overview.as_mut().filter(|o| !o.leaving()) {
            o.interrupted = true;
        }
    }

    /// Before each frame: gone once it has faded, and the window under
    /// the pointer.
    pub fn overview_tick(&mut self) {
        if self.overview.as_ref().is_some_and(|o| o.interrupted && !o.leaving()) {
            self.close_overview(None);
        }
        if self.overview.as_ref().is_some_and(|o| o.done(Instant::now())) {
            self.overview = None;
            self.warp_pointer(self.pointer.current_location());
        }
    }

    fn overview_hover(&mut self) {
        let pos = self.pointer.current_location();
        if let Some(o) = self.overview.as_mut() {
            let id = o.cell_at(pos).map(|c| c.window.id());
            if id != o.hovered {
                o.hovered = id;
                if id.is_some() {
                    o.selected = None;
                }
            }
        }
    }

    /// The pointer moved, while it may be out.
    pub fn overview_pointer(&mut self) {
        if self.overview.is_some() {
            self.overview_hover();
        }
    }

    /// A button, while it may be out: true if it was its. A click on a
    /// window picks it; on nothing, leaves.
    pub fn overview_button(&mut self, button: u32, pressed: bool) -> bool {
        let Some(o) = self.overview.as_mut() else { return false };
        if !pressed {
            let Some(i) = o.buttons.iter().position(|b| *b == button) else { return false };
            o.buttons.swap_remove(i);
            return true;
        }
        if o.leaving() {
            return false;
        }
        // Deskbar and the like over it still take clicks.
        let pos = self.pointer.current_location();
        if self.surface_under(pos).is_some() {
            return false;
        }
        let Some(o) = self.overview.as_mut() else { return false };
        o.buttons.push(button);
        let chosen = o.cell_at(pos).map(|c| c.window.clone());
        self.close_overview(chosen);
        true
    }

    /// A key, while it may be out: true if it was its. Esc leaves,
    /// a letter or Return picks, the arrows walk; its own shortcut goes
    /// on to the keymap (which leaves); anything else does nothing.
    pub fn overview_key(&mut self, key: u32, pressed: bool) -> bool {
        const ESC: u32 = 1;
        const RETURN: u32 = 28;
        const KP_ENTER: u32 = 96;
        const UP: u32 = 103;
        const LEFT: u32 = 105;
        const RIGHT: u32 = 106;
        const DOWN: u32 = 108;
        let Some(o) = self.overview.as_mut() else { return false };
        if !pressed {
            let Some(i) = o.swallowed.iter().position(|k| *k == key) else { return false };
            o.swallowed.swap_remove(i);
            return true;
        }
        if o.leaving() || olo_protocol::shortcuts::MODIFIER_KEYS.contains(&key) {
            return false;
        }
        if self.keymap.shortcut_for(key) == Some(olo_protocol::Shortcut::Overview) {
            return false;
        }
        o.swallowed.push(key);
        // Swallowed with ⌘ held: letting go of ⌘ is no tap of it.
        self.keymap.command_used();
        let Some(o) = self.overview.as_mut() else { return true };
        match key {
            ESC => self.close_overview(None),
            RETURN | KP_ENTER => {
                let chosen = o.chosen().and_then(|id| o.cells.iter().find(|c| c.window.id() == id)).map(|c| c.window.clone());
                if chosen.is_some() {
                    self.close_overview(chosen);
                }
            }
            LEFT | RIGHT | UP | DOWN => {
                let (dx, dy) = match key {
                    LEFT => (-1.0, 0.0),
                    RIGHT => (1.0, 0.0),
                    UP => (0.0, -1.0),
                    _ => (0.0, 1.0),
                };
                o.selected = step(&o.cells, o.chosen(), (dx, dy));
                o.hovered = None;
            }
            k => {
                let chosen = crate::hints::letter(k).and_then(|c| o.cells.iter().find(|cell| cell.letter == Some(c))).map(|c| c.window.clone());
                if chosen.is_some() {
                    self.close_overview(chosen);
                }
            }
        }
        true
    }

    /// Frame callbacks for the windows it shows that aren't on screen
    /// (other workspaces'), so they go on drawing.
    pub fn overview_frames(&self, output: &Output, time: Duration) {
        let Some(o) = self.overview.as_ref() else { return };
        let name = output.name();
        for c in o.cells.iter().filter(|c| c.output == name) {
            if self.space.element_location(&c.window).is_none() {
                c.window.send_frame(output, time, Some(Duration::from_secs(1)), |_, _| Some(output.clone()));
            }
        }
    }

    /// What it shows, for agents.
    pub fn overview_info(&self) -> olo_protocol::OverviewInfo {
        let frame = |r: Rect| {
            let r = r.to_i32_round::<i32>();
            olo_protocol::Frame { x: r.loc.x, y: r.loc.y, width: r.size.w, height: r.size.h }
        };
        let Some(o) = self.overview.as_ref().filter(|o| !o.leaving()) else { return olo_protocol::OverviewInfo::default() };
        olo_protocol::OverviewInfo {
            shown: true,
            panes: o.panes.iter().map(|p| olo_protocol::OverviewPane { output: p.output.clone(), workspace: p.workspace, current: p.current, frame: frame(p.rect) }).collect(),
            windows: o
                .cells
                .iter()
                .filter(|c| c.window.alive())
                .map(|c| olo_protocol::OverviewWindow {
                    id: c.window.id(),
                    title: c.window.title(),
                    app_id: c.window.app_id(),
                    output: c.output.clone(),
                    workspace: c.workspace,
                    frame: frame(c.to),
                    letter: c.letter.map(|l| l.to_ascii_uppercase().to_string()),
                    chosen: o.chosen() == Some(c.window.id()),
                })
                .collect(),
        }
    }
}

/// The window the arrows go to from `from` in direction `d`: the nearest
/// whose middle lies that way (on any display), else the first.
fn step(cells: &[Cell], from: Option<u64>, d: (f64, f64)) -> Option<u64> {
    let middle = |r: &Rect| (r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0);
    let Some(here) = from.and_then(|id| cells.iter().find(|c| c.window.id() == id)) else {
        return cells.first().map(|c| c.window.id());
    };
    let (hx, hy) = middle(&here.to);
    cells
        .iter()
        .filter(|c| c.window.id() != here.window.id())
        .filter_map(|c| {
            let (x, y) = middle(&c.to);
            let (dx, dy) = (x - hx, y - hy);
            let along = dx * d.0 + dy * d.1;
            // That way, and more that way than across.
            let across = (dx * d.1 - dy * d.0).abs();
            (along > 0.0 && along >= across * 0.5).then_some((along + 2.0 * across, c.window.id()))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
        .or(Some(here.window.id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn one_pane_fills_the_area() {
        let p = panes(r(0.0, 0.0, 1600.0, 1024.0), 1, 1.6);
        assert_eq!(p.len(), 1);
        // As high as it can be under its name, as wide as that allows.
        assert_eq!(p[0].size.h, 1000.0);
        assert_eq!(p[0].size.w, 1600.0);
        assert_eq!(p[0].loc, (0.0, LABEL).into());
    }

    #[test]
    fn panes_in_the_grid_that_makes_them_biggest() {
        let area = r(0.0, 0.0, 1600.0, 1000.0);
        let p = panes(area, 4, 1.6);
        assert_eq!(p.len(), 4);
        // Two by two beats four in a row or one above another.
        assert_eq!(p[0].loc.y, p[1].loc.y);
        assert!(p[2].loc.y > p[0].loc.y);
        for q in &p {
            assert!((q.size.w / q.size.h - 1.6).abs() < 1e-9);
            assert!(area.contains_rect(*q));
        }
        // Three: a short last row, centred.
        let p = panes(area, 3, 1.6);
        let middle = p[2].loc.x + p[2].size.w / 2.0;
        assert!((middle - 800.0).abs() < 1e-6, "{middle}");
    }

    #[test]
    fn windows_spread_without_covering_each_other() {
        let pane = r(0.0, 0.0, 800.0, 500.0);
        // Three windows on top of each other.
        let frames = [r(100.0, 100.0, 900.0, 600.0), r(150.0, 150.0, 900.0, 600.0), r(200.0, 120.0, 400.0, 300.0)];
        let out = spread(pane, &frames, 0.5);
        for (i, a) in out.iter().enumerate() {
            assert!(pane.contains_rect(*a), "{a:?}");
            for b in &out[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} {b:?}");
            }
        }
        // One scale for all: their sizes keep their proportions.
        let s = out[0].size.w / 900.0;
        assert!((out[2].size.w / 400.0 - s).abs() < 1e-9);
        assert!(s <= 0.5);
    }

    #[test]
    fn a_lone_window_keeps_to_the_most() {
        let pane = r(0.0, 0.0, 800.0, 500.0);
        let out = spread(pane, &[r(0.0, 0.0, 400.0, 300.0)], 0.5);
        assert_eq!(out[0].size, (200.0, 150.0).into());
        // Centred.
        assert_eq!(out[0].loc, (300.0, 175.0).into());
    }
}
