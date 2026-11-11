//! What folder windows and the desktop share: an item's icon and label
//! (laid out, hit, drawn selected or not), dragging items out, the rules
//! for what dragged things a folder takes, spring-loaded folders and
//! editing a name in place.

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, Cx, DragAction, DropAccept, DropOffer, Element, Font, FontStyle, FontWeight, Hsla, LineFragment, Painter, Pixels, Point, SharedString, View, WindowCx, agent, icons, metrics as m,
    point, px, size,
    style::{self, Align},
    widgets::{TextField, text_field},
};
use olo_sync::Mutex;
use olo_theme::Palette;

use crate::fs::{self, Entry};

/// Icon view: 48-point icons in 96 × 84 cells (icon, gap, two label
/// lines).
pub const ICON: f32 = 48.0;
pub const ICON_CELL: (f32, f32) = (96.0, 84.0);
/// Mini-icon view: 16-point icons with the label beside them.
pub const MINI: f32 = 16.0;
pub const MINI_CELL: (f32, f32) = (184.0, m::LIST_ROW);
/// Room between a selected label's text and its highlight (the field
/// editing a name has the same, so they line up).
const LABEL_PAD: (f32, f32) = (4.0, 4.0);
/// A selected icon is darkened, as on BeOS.
fn selected_tint() -> Hsla {
    olo_kit::hsla(0.0, 0.0, 185.0 / 255.0, 1.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Icons,
    Mini,
}

/// The font a name is set in: italic for a link, as Tracker's.
pub fn name_font(link: bool) -> Font {
    let mut f = style::ui_font(FontWeight::NORMAL);
    if link {
        f.style = FontStyle::Italic;
    }
    f
}

/// One line's height of names.
fn row_height() -> f32 {
    style::PLAIN_SIZE * style::LINE_HEIGHT
}

/// Where an item's icon and label land in its cell.
#[derive(Clone, Debug)]
pub struct ItemLayout {
    pub icon: Bounds<Pixels>,
    /// The label's lines and their widths.
    lines: Rc<[(SharedString, Pixels)]>,
    /// The lines' centre (icons) or left (mini icons).
    x: Pixels,
    centred: bool,
    /// The first line's baseline, and from one to the next.
    baseline: Pixels,
    step: Pixels,
    pub font: Font,
    /// Its whole name (for agents).
    pub name: SharedString,
}

impl ItemLayout {
    /// What a click takes as the item: its icon and its label.
    pub fn hit(&self) -> Bounds<Pixels> {
        self.icon.union(&self.highlight())
    }

    /// The label's highlight when selected.
    pub fn highlight(&self) -> Bounds<Pixels> {
        let w = self.lines.iter().map(|l| l.1).fold(px(0.0), Pixels::max);
        let left = if self.centred { self.x - w / 2.0 } else { self.x };
        let rows = self.lines.len().max(1) - 1;
        let top = self.baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT + LABEL_PAD.1);
        let bottom = self.baseline + self.step * rows as f32 + px(LABEL_PAD.1);
        Bounds::from_corners(point(left - px(LABEL_PAD.0), top), point(left + w + px(LABEL_PAD.0), bottom))
    }

    /// Where a field editing the name goes: over the first line, as wide
    /// as `width` of text (and room for the caret), anchored as the
    /// label is.
    pub fn field(&self, width: Pixels) -> Bounds<Pixels> {
        let left = if self.centred { self.x - width / 2.0 } else { self.x };
        let top = self.baseline - px(style::PLAIN_SIZE * style::CAP_HEIGHT + LABEL_PAD.1);
        let bottom = self.baseline + px(LABEL_PAD.1);
        Bounds::from_corners(point(left - px(LABEL_PAD.0), top), point(left + width + px(LABEL_PAD.0 + 2.0), bottom))
    }
}

/// A label as laid out: its lines (with their widths) and its whole
/// name.
type Label = (Rc<[(SharedString, Pixels)]>, SharedString);

/// What decides how a label is laid out besides its text and font.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LabelShape {
    width: u32,
    /// Lines at most; 0 for one line cut short, as the mini-icon view's.
    rows: u8,
    scale: u32,
}

/// Labels by their text and font (hashed) and shape, each with the
/// text and font it was laid out for.
type LabelCache = HashMap<(u64, LabelShape), (SharedString, Font, Label)>;

thread_local! {
    /// Labels laid out, by their text and font (hashed) and shape:
    /// scrolling and band-selecting lay the same items out every frame,
    /// and wrapping a name shapes it again.
    static LABELS: RefCell<LabelCache> = RefCell::default();
}

/// `text` in `font`, at most `rows` lines `width` wide (0: one line cut
/// short), from the cache if laid out so before.
fn label(text: &str, font: &Font, width: Pixels, rows: usize) -> Label {
    let shape = LabelShape { width: f32::from(width).to_bits(), rows: rows as u8, scale: olo_kit::scale_factor().to_bits() };
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    font.hash(&mut h);
    let key = (h.finish(), shape);
    let known = LABELS.with_borrow(|c| c.get(&key).filter(|(t, f, _)| t.as_ref() == text && f == font).map(|(_, _, l)| l.clone()));
    if let Some(l) = known {
        return l;
    }
    let lines: Rc<[(SharedString, Pixels)]> = if rows == 0 {
        let fitted = style::fit_with(&olo_kit::thread_text_system(), text.to_owned().into(), style::PLAIN_SIZE, font, width);
        let w = style::text_width(fitted.clone(), style::PLAIN_SIZE, font.clone());
        Rc::from([(fitted, w)])
    } else {
        wrap(text, font, width, rows).into()
    };
    let name = SharedString::from(text.to_owned());
    let l = (lines, name.clone());
    LABELS.with_borrow_mut(|c| {
        // (A few folders' worth; a folder of thousands scrolled through
        // starts it again.)
        if c.len() >= 16_384 {
            c.clear();
        }
        c.insert(key, (name, font.clone(), l.clone()));
    });
    l
}

/// `text` broken into at most `rows` lines `width` wide, the last cut
/// short with "…" when it doesn't all fit.
fn wrap(text: &str, font: &Font, width: Pixels, rows: usize) -> Vec<(SharedString, Pixels)> {
    let ts = olo_kit::thread_text_system();
    let mut wrapper = olo_kit::text_system().line_wrapper(font.clone(), px(style::PLAIN_SIZE));
    let fragments = [LineFragment::text(text)];
    let mut starts = vec![0];
    starts.extend(wrapper.wrap_line(&fragments, width).map(|b| b.ix));
    drop(wrapper);
    let mut lines = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        if k + 1 == rows && k + 1 < starts.len() {
            // The rest, cut short.
            lines.push(style::fit_with(&ts, text[start..].trim().to_owned().into(), style::PLAIN_SIZE, font, width));
            break;
        }
        let end = starts.get(k + 1).copied().unwrap_or(text.len());
        lines.push(SharedString::from(text[start..end].trim().to_owned()));
    }
    lines
        .into_iter()
        .map(|l| {
            let w = style::text_width(l.clone(), style::PLAIN_SIZE, font.clone());
            (l, w)
        })
        .collect()
}

/// `text` broken into at most `rows` lines `width` wide (the last cut
/// short with "…").
pub fn wrap_lines(text: &str, font: &Font, width: Pixels, rows: usize) -> Vec<SharedString> {
    wrap(text, font, width, rows).into_iter().map(|(l, _)| l).collect()
}

/// `v` on the device pixel grid of the window this looper is working
/// for (as `style::snap_coord`, where there's no painter: a layout
/// worked out as its view's `body` runs, or for a press).
fn snap(v: Pixels) -> Pixels {
    let s = olo_kit::scale_factor();
    px((f32::from(v) * s).round() / s)
}

/// Lays out an item in `cell`: an icon above a centred label of two
/// lines (four when selected, so its whole name shows), or a small icon
/// with its name beside it. (For the window this looper is working for:
/// its scale; no painter is needed, so a press can ask where a label
/// is.)
pub fn layout_item(cell: Bounds<Pixels>, label: &str, look: Look, selected: bool, link: bool) -> ItemLayout {
    let font = name_font(link);
    match look {
        Look::Icons => {
            let icon = Bounds { origin: point(cell.center().x - px(ICON / 2.0), cell.top() + px(m::TIGHT)), size: size(px(ICON), px(ICON)) };
            let width = cell.size.width - px(2.0 * m::TIGHT);
            let (lines, name) = self::label(label, &font, width, if selected { 4 } else { 2 });
            let top = icon.bottom() + px(m::TIGHT);
            let baseline = snap(top + px(row_height() / 2.0) + px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0));
            let step = snap(px(row_height()));
            ItemLayout { icon, lines, x: snap(cell.center().x), centred: true, baseline, step, font, name }
        }
        Look::Mini => {
            let icon = Bounds { origin: point(cell.left() + px(m::TIGHT), cell.center().y - px(MINI / 2.0)), size: size(px(MINI), px(MINI)) };
            let x = icon.right() + px(m::MARK_GAP);
            let width = (cell.right() - x - px(m::TIGHT)).max(px(0.0));
            let (lines, name) = self::label(label, &font, width, 0);
            let baseline = snap(cell.center().y + px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0));
            ItemLayout { icon, lines, x, centred: false, baseline, step: px(row_height()), font, name }
        }
    }
}

/// Draws an item. On the desktop the label is in the desktop's text
/// colour over a shadow; elsewhere on the document's ground. With `label`
/// false only the icon is drawn (its name is being edited).
#[allow(clippy::too_many_arguments)]
pub fn paint_item(window: &mut Painter, l: &ItemLayout, icon: &str, selected: bool, desktop: bool, label: bool, p: &Palette) {
    agent::record(window, agent::Node { role: "ListItem", name: Some(l.name.clone()), bounds: l.hit(), enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
    if selected && label {
        icons::paint_tinted(window, l.icon, icon, selected_tint());
    } else {
        icons::paint(window, l.icon, icon);
    }
    if !label {
        return;
    }
    let align = if l.centred { Align::Center } else { Align::Left };
    let line = |window: &mut Painter, k: usize, text: &SharedString, dx: Pixels, color: Hsla| {
        let base = l.baseline + l.step * k as f32;
        style::label_on_baseline(window, l.x + dx, base + dx, align, text.clone(), style::PLAIN_SIZE, l.font.clone(), color);
    };
    if selected {
        // BeOS inverts a selected icon's label; in dark mode that makes
        // it light on dark-on-light, which reads the same way.
        let h = style::snap(window, l.highlight());
        window.paint_quad(olo_kit::fill(h, style::c(p.document_text)).corner_radii(px(2.0)));
        for (k, (text, _)) in l.lines.iter().enumerate() {
            line(window, k, text, px(0.0), style::c(p.document_background));
        }
    } else if desktop {
        for (k, (text, _)) in l.lines.iter().enumerate() {
            line(window, k, text, px(1.0), style::c(p.desktop_text_shadow));
            line(window, k, text, px(0.0), style::c(p.desktop_text));
        }
    } else {
        for (k, (text, _)) in l.lines.iter().enumerate() {
            line(window, k, text, px(0.0), style::c(p.document_text));
        }
    }
    // Cut short: the whole name in a tip for the pointer resting on it.
    if l.lines.last().is_some_and(|(t, _)| t.ends_with('…')) && !l.name.ends_with('…') {
        olo_kit::cut::note(window, l.name.clone(), l.highlight());
    }
}

/// DiskBackup's tick (`backups`): its current version is backed up. A
/// green disc at the icon's lower right, edged a shade darker, a white
/// tick in it; said to agents as "Backed up: NAME".
pub fn paint_mark(window: &mut Painter, icon: Bounds<Pixels>, name: &str, p: &Palette) {
    let d = (f32::from(icon.size.width) * 0.42).round().clamp(8.0, 14.0);
    let at = point(icon.right() - px(d / 2.0 - 1.0), icon.bottom() - px(d / 2.0 - 1.0));
    let disc = style::snap_centred(window, at, d, d);
    let green = style::c(p.success);
    let line = style::hairline(window);
    window.paint_quad(olo_kit::quad(disc, px(d / 2.0), green, line, style::shade(p, p.success, 0.75), olo_kit::BorderStyle::Solid));
    let (l, t) = (f32::from(disc.left()), f32::from(disc.top()));
    let mut tick = olo_kit::PathBuilder::stroke(px((d * 0.15).max(1.3)));
    tick.move_to(point(px(l + 0.28 * d), px(t + 0.53 * d)));
    tick.line_to(point(px(l + 0.44 * d), px(t + 0.69 * d)));
    tick.line_to(point(px(l + 0.73 * d), px(t + 0.35 * d)));
    if let Ok(path) = tick.build() {
        window.paint_path(path, olo_kit::white());
    }
    agent::record(window, agent::Node { role: "Image", name: Some(format!("Backed up: {name}").into()), bounds: disc, ..Default::default() });
}

/// How far along a download is, as a bar across the foot of its icon
/// (as Finder shows one).
pub fn paint_progress(window: &mut Painter, icon: Bounds<Pixels>, done: f32, p: &Palette) {
    let h = (f32::from(icon.size.height) / 8.0).clamp(5.0, 8.0).round();
    let w = (f32::from(icon.size.width) * 0.75).round();
    let bar = style::snap(window, Bounds { origin: point(icon.center().x - px(w / 2.0), icon.bottom() - px(h)), size: size(px(w), px(h)) });
    let mut well = style::c(p.document_text);
    well.a = 46.0 / 255.0;
    window.paint_quad(olo_kit::fill(bar, well).corner_radii(px(h / 2.0)));
    // As much of it filled as is done, never less than its round end.
    let end = style::snap_coord(window, bar.left() + bar.size.width * done.clamp(0.0, 1.0)).max(bar.left() + bar.size.height);
    window.paint_quad(olo_kit::fill(Bounds::from_corners(bar.origin, point(end, bar.bottom())), style::c(p.status_bar)).corner_radii(px(h / 2.0)));
}

/// The icon for an entry (folders that are special have their own).
pub fn icon_of(e: &Entry) -> &'static str {
    if e.is_dir {
        let home = fs::home();
        if e.path == home {
            return "home";
        }
        if e.path == home.join("Desktop") {
            return "desktop";
        }
        if e.path == fs::trash_files() {
            return if fs::trash_is_empty() { "trash-empty" } else { "trash-full" };
        }
    }
    e.app_icon.or_else(|| crate::appimages::icon(e)).unwrap_or_else(|| e.kind.icon())
}

// ---- dragging out -----------------------------------------------------------

/// Marks a drag started with the secondary button: the place it is
/// dropped on asks what to do (Move Here, Copy Here, Create Link Here),
/// as on BeOS.
pub const ASK_TYPE: &str = "application/x-olo-ask";

/// The type (no data) naming the file system the dragged files are on,
/// so a folder they're dragged over knows before the drop whether it
/// would move or copy them.
fn device_type(dev: u64) -> String {
    format!("application/x-olo-device-{dev}")
}

/// The type (no data) naming the folder they come from (a hash of its
/// path), so a window can refuse them dropped back where they are.
fn folder_type(dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    format!("application/x-olo-from-folder-{:016x}", h.finish())
}

/// Items to drag, and how.
pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// The icon of the one the pointer is on, its size, and where on it
    /// the pointer is.
    pub icon: &'static str,
    pub size: f32,
    pub hotspot: Point<Pixels>,
    /// With the secondary button.
    pub ask: bool,
    pub name: String,
}

/// The drag of ours under way: the window it started in (its number,
/// `ToHub`'s), and the folder its items come from (dropping them back
/// there does nothing). The app's: a drag out of one window is over
/// another, on another looper.
static DRAGGING: Mutex<Option<(u64, Option<PathBuf>)>> = Mutex::new(None);

/// Whether the drag over a window is ours, from window `view`.
pub fn from_view(view: u64) -> bool {
    DRAGGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(v, _)| *v == view)
}

/// The folder our drag under way comes from.
fn dragged_from() -> Option<PathBuf> {
    DRAGGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|(_, f)| f.clone())
}

/// Starts dragging `d` out of window `view` (while the button is still
/// down): the files, plus Olo's own types saying which folder and file
/// system they are from and whether the drop should ask. The view is
/// sent `done` when it's over.
pub fn start<V: View>(cx: &mut Cx<V>, view: u64, d: Drag, done: impl FnOnce() -> V::Message + 'static)
where
    V::Message: Send,
{
    use std::os::unix::fs::MetadataExt;
    let from = d.paths.first().and_then(|p| p.parent()).map(Path::to_path_buf);
    let mut drag = olo_kit::dnd::files(&d.paths, true, true);
    drag.ask = true;
    let mut extra: Vec<String> = Vec::new();
    if let Some(dir) = &from {
        extra.push(folder_type(dir));
    }
    if let Some(dev) = d.paths.first().and_then(|p| std::fs::symlink_metadata(p).ok()).map(|m| m.dev()) {
        extra.push(device_type(dev));
    }
    if d.ask {
        extra.push(ASK_TYPE.into());
    }
    for t in extra {
        drag.types.insert(0, (t, std::sync::Arc::from(Vec::new())));
    }
    let n = d.paths.len();
    let name = if n > 1 { format!("{n} items") } else { d.name.clone() };
    drag.icon = olo_kit::dnd::picture(cx, d.icon, d.size, Some(&name), d.hotspot);
    *DRAGGING.lock().unwrap_or_else(|e| e.into_inner()) = Some((view, from));
    olo_kit::dnd::start(cx, drag, move |_| {
        *DRAGGING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        done()
    });
}

// ---- taking drops -----------------------------------------------------------

/// Where a drop goes; [`DropTarget::name`] is what the window takes it
/// as, and comes back with the drop.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Into a folder (files) or as a clipping file in it (text).
    Folder(PathBuf),
    Trash,
    /// Icons dragged about the desktop: put them where they are dropped.
    Place,
}

impl DropTarget {
    pub fn name(&self) -> String {
        match self {
            DropTarget::Folder(p) => format!("folder:{}", p.display()),
            DropTarget::Trash => "trash".into(),
            DropTarget::Place => "place".into(),
        }
    }

    pub fn parse(name: &str) -> Option<DropTarget> {
        match name {
            "trash" => Some(DropTarget::Trash),
            "place" => Some(DropTarget::Place),
            _ => name.strip_prefix("folder:").map(|p| DropTarget::Folder(PathBuf::from(p))),
        }
    }
}

/// The first text type it comes in.
fn text_type(o: &DropOffer) -> Option<&'static str> {
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"].into_iter().find(|t| o.has(t))
}

/// Takes what is dragged over folder `dir` if it would go there, choosing
/// as BeOS's Tracker did: move within a file system, copy across them,
/// ⌥ to copy anyway, the secondary button to ask; the Trash takes files
/// only, and moves them. Text becomes a clipping file. Returns whether it
/// would be taken (None: refused; to highlight `dir`).
pub fn take_drop(cx: &WindowCx, o: &DropOffer, dir: &Path) -> Option<DropAccept> {
    use std::os::unix::fs::MetadataExt;
    let trash = dir == fs::trash_files();
    let target = if trash { DropTarget::Trash } else { DropTarget::Folder(dir.to_owned()) };
    let accept = |mime: &str, action: DragAction| Some(DropAccept { target: target.name(), mime: mime.to_owned(), action });
    if o.has("text/uri-list") {
        let action = if o.has(ASK_TYPE) && o.ask && !trash {
            DragAction::Ask
        } else if trash {
            if !o.move_ {
                return None;
            }
            DragAction::Move
        } else {
            let same_fs = std::fs::metadata(dir).is_ok_and(|m| o.has(&device_type(m.dev())));
            let copy = cx.modifiers().alt || !same_fs || !o.move_;
            // Only our own drags move: another program might delete what
            // it gave us once told it moved.
            if copy || !o.from_us {
                if !o.copy {
                    return None;
                }
                DragAction::Copy
            } else {
                DragAction::Move
            }
        };
        accept("text/uri-list", action)
    } else if trash || !o.copy {
        None
    } else if let Some(t) = o.types.iter().find(|t| olo_kit_base::files::named_file(t).is_some()) {
        // A picture dragged out of a web page: the file itself.
        accept(t, DragAction::Copy)
    } else if o.has("text/x-moz-url") {
        // A link: a clipping of it, named by its text.
        accept("text/x-moz-url", DragAction::Copy)
    } else if let Some(t) = text_type(o) {
        accept(t, DragAction::Copy)
    } else {
        None
    }
}

/// Over a view of items in folder `here`: the item it would be dropped on
/// (`over`: its index, entry, and whether it's selected; it takes things
/// if it's a folder not itself dragged), else the folder. How it would
/// be taken (None: refused), and the item it would go on.
pub fn item_drop(cx: &WindowCx, view: u64, o: &DropOffer, over: Option<(usize, &Entry, bool)>, here: Option<&Path>) -> (Option<DropAccept>, Option<usize>) {
    let from_here = from_view(view);
    if let Some((i, e, selected)) = over
        && e.is_dir
        && !(from_here && selected)
    {
        return (take_drop(cx, o, &e.path), Some(i));
    }
    match here {
        // Dropping in the folder it came from would do nothing (another
        // column of the same window is another folder); nor from another
        // app's drag of a file here (Viewer, Quick Look).
        Some(dir) if !(from_here && dragged_from().as_deref() == Some(dir)) && !o.has(&folder_type(dir)) => (take_drop(cx, o, dir), None),
        _ => (None, None),
    }
}

/// Outlines `r` (a view, a column) as taking what's dragged over it: a
/// line two points wide inside it in the keyboard's colour, above what's
/// in it.
pub fn outline(window: &mut Painter, r: Bounds<Pixels>, p: &Palette) {
    let ink = style::c(p.keyboard_navigation);
    style::frame_rect(window, r, ink);
    style::frame_rect(window, style::inset(window, r, 1.0), ink);
}

/// How long a drag rests on a folder before it springs open.
pub const SPRING: Duration = Duration::from_millis(1000);

/// A folder a drag is resting on (spring-loaded folders, as Tracker's):
/// after [`SPRING`] it opens, so the drag can go on into it.
#[derive(Default)]
pub struct Spring {
    at: Option<(PathBuf, Instant, bool)>,
}

impl Spring {
    /// The drag rests on `dir` now (None: on nothing that opens), as
    /// its view was asked where it would take it. Returns whether to
    /// look again once [`SPRING`] has passed ([`Spring::due`]): the
    /// pointer may rest there and nothing more be asked.
    pub fn rest(&mut self, dir: Option<&Path>) -> bool {
        let Some(dir) = dir else {
            self.at = None;
            return false;
        };
        match &self.at {
            Some((d, ..)) if d == dir => false,
            _ => {
                self.at = Some((dir.to_owned(), Instant::now(), false));
                true
            }
        }
    }

    /// The folder to open now, if the drag has rested on one long
    /// enough (once for each rest).
    pub fn due(&mut self) -> Option<PathBuf> {
        match &mut self.at {
            Some((dir, since, opened)) if !*opened && since.elapsed() >= SPRING => {
                *opened = true;
                Some(dir.clone())
            }
            _ => None,
        }
    }
}

// ---- editing a name in place ------------------------------------------------

/// A name being edited where its label was (Edit Name): the field, and
/// what it renames.
pub struct Rename {
    pub path: PathBuf,
    pub field: Child<TextField>,
    link: bool,
}

impl Rename {
    /// Starts editing `path`'s name, its name up to its extension
    /// selected (".txt" rarely changes); the field has the keyboard.
    pub fn new<V: View>(path: PathBuf, link: bool, cx: &mut Cx<V>) -> Rename {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = match name.rfind('.') {
            Some(i) if i > 0 => i,
            _ => name.len(),
        };
        let field = text_field(cx, |f| f.keeps_enter().font(name_font(link)).name("Name"));
        field.update(cx, |f, cx| {
            f.set_text(name, cx);
            f.set_selection(0..stem, cx);
        });
        field.focus(cx);
        Rename { path, field, link }
    }

    pub fn text(&self, cx: &WindowCx) -> String {
        self.field.read(cx).text().to_owned()
    }

    /// The field, over `l`'s label, its text on the label's baseline:
    /// the document's ground framed in the keyboard's colour, growing as
    /// the name does. `origin`: where what it's a child of starts.
    pub fn element(&self, cx: &WindowCx, l: &ItemLayout, origin: Point<Pixels>) -> Element {
        use olo_kit::{IntoElement as _, ParentElement as _, Styled as _};
        let text = self.text(cx);
        let measure = if text.is_empty() { "M".to_owned() } else { text };
        let w = style::text_width(measure, style::PLAIN_SIZE, name_font(self.link));
        let f = l.field(w);
        let r = Bounds::from_corners(point(snap(f.left()), snap(f.top())), point(snap(f.right()), snap(f.bottom())));
        let frame = olo_kit::canvas(move |g, b| {
            let p = olo_kit::theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            style::frame_rect(g, b, style::c(p.keyboard_navigation));
        })
        .absolute()
        .inset_0();
        olo_kit::div()
            .absolute()
            .left(r.left() - origin.x)
            .top(r.top() - origin.y)
            .w(r.size.width)
            .h(r.size.height)
            .child(frame)
            .child(olo_kit::div().absolute().left(px(LABEL_PAD.0)).right(px(1.0)).top_0().bottom_0().child(&self.field))
            .into_any_element()
    }
}
