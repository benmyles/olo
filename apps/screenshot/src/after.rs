//! What follows a person's recording or screenshot (`olo-screenshot
//! --after PATH`, which `--toggle-record` starts once the file is whole,
//! in the saved notification's place, and a screenshot saved by a
//! shortcut or from the window starts for its picture: Ben, 2026-11-27,
//! "after taking a screenshot please make it send a notification that
//! comes up in the bottom left with a thumbnail of the screenshot and the
//! thumbnail should support drag and drop so i can drag it into an app i
//! want to use the screenshot in (just like in macos)"): a card in the
//! screen's lower left, as CleanShot X's quick-access overlay and macOS's
//! floating thumbnail. Its picture, its name, how long (a recording) and
//! how big it is, and Show in Tracker, Copy, Edit (a recording) or Open
//! (a picture) and Delete, with a close box; the picture dragged out is
//! the file (to a folder, a chat, a mail). It goes by itself `STAYS`
//! after the pointer left it (over it, it stays), when closed, or once
//! something was done with it. Copied, the window goes and the process
//! stays only to serve the clipboard, until something else is copied.
//!
//! One card at a time, and never in a picture: a new card puts the one
//! before it away, and so does whatever is about to take a picture of
//! the screen (a screenshot, the region picker, a recording:
//! [`put_away`]), which waits until it has gone. (A screenshot is the
//! screen as it is: the card of the shot before was in the next one.)
//! And app_server leaves its layer (`olo_protocol::recording::
//! AFTER_NAMESPACE`) out of every recording and share, as it does the
//! recording's controls: a card that comes up while the screen is being
//! recorded is on the screen, not in the recording.
//!
//! The picture is what the card is for, and it is shown so that it can
//! be told what it is of ([`shown`]): fitted whole in the card's column
//! where it is larger; a few pixels dragged out of the screen as blocks,
//! each pixel a whole number of device pixels (never smoothed), until it
//! is tall enough to see; a strip from its start, as it is, its far end
//! fading out. The frame is the picture's own edge, always.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use olo_kit::{
    Bounds, BoxShadow, ClickEvent, Corners, CursorStyle, Cx, DragAction, DragImage, Element, FontWeight, Hsla, InteractiveElement as _, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Pixels, Point, RenderImage, Styled as _, View, WindowOptions, canvas, div, dnd, fill, held_clipboard, hsla,
    instance::{self, Launch},
    linear_color_stop, linear_gradient, metrics as m, point, px, size, style, theme,
    widgets::{Button, SMALL_BUTTON_HEIGHT, StringView},
};
use olo_protocol::recording::AFTER_NAMESPACE;

const APP_ID: &str = "olo.screenshot-after";
/// The card's column (points): the picture's frame at its widest, the
/// text and the buttons.
const PICTURE_W: f32 = 280.0;
/// The frame at its tallest, and the least a picture is shown at on its
/// short side where it can be (a 16:10 screen's picture fills the column
/// at 177).
const PICTURE_MOST_H: f32 = 177.0;
const PICTURE_LEAST_H: f32 = 48.0;
/// A strip's cut end goes out into the card over this much, its frame
/// with it: the frame is open there (closed, its edge said "the picture
/// stops here" where the fading said "it goes on").
const FADE: f32 = 24.0;
/// From the frame to the name, the name's row and the facts', and from
/// the facts to the buttons.
const GAP_NAME: f32 = 8.0;
const NAME_ROW: f32 = 16.0;
const FACTS_ROW: f32 = 15.0;
const GAP_BUTTONS: f32 = 12.0;
/// The close box's square.
const CLOSE: f32 = 16.0;
/// From the screen's edges to the card.
const MARGIN: f32 = 16.0;
/// Room round the card for its shadow, which ends inside it (it was cut
/// off at the window's edge: a straight seam over a bright wallpaper).
const SHADOW: f32 = 12.0;
/// How long it stays once the pointer has left it.
const STAYS: Duration = Duration::from_secs(20);
/// How far a press moves before it's a drag (points).
const DRAG_AFTER: f32 = 4.0;
/// How long whatever is about to take a picture waits for a card to go.
const AWAY_WITHIN: Duration = Duration::from_millis(1500);

/// Copied: the window is gone and the process serves the clipboard. An
/// upgrade in place waits for that to end (`upgrade_waits_while`: begun
/// again as the upgrade's copy, the card came back and the copied
/// picture was gone from the clipboard).
static HOLDING: AtomicBool = AtomicBool::new(false);

enum Msg {
    /// The picture made for a scale (and the picture a drag carries), or
    /// none (it couldn't be made).
    Picture(f32, Option<(Arc<RenderImage>, Option<DragImage>)>),
    /// The file couldn't be read as a picture.
    NoPicture,
    /// Asked to go (the next picture is about to be taken, or the next
    /// card is coming). Decided here, on the window's looper, where Copy
    /// is: one that only holds the clipboard stays.
    PutAway,
    /// The original picture, to make again at another scale.
    Original(Arc<image::RgbaImage>),
    /// `STAYS` since the pointer left (the wait numbered so: an older
    /// one, or one the pointer came back in, is dropped).
    Leave(u64),
    /// The file dragged out was dropped as this (None: nowhere).
    Dropped(Option<DragAction>),
}

/// What the card is of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// A video, or a GIF made of one.
    Recording,
    /// A screenshot (`--picture`, said by whoever saved it: a recording
    /// may be given any name).
    Picture,
}

/// A picture's MIME type by its name: what Copy offers it as.
fn picture_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The card showing on this display, if one is, is put away, and this
/// returns once it has gone (or `AWAY_WITHIN` has passed: nothing waits
/// on it for ever): before a picture of the screen is taken, and before
/// another card takes its place. One only holding the clipboard (its
/// window gone) is not a card any more and stays. (Told on its own
/// launch socket by the display's name: nothing of a launch's policy,
/// which is a starting program's, is asked or changed here.)
pub fn put_away() {
    let Some(pid) = instance::running_pid(APP_ID) else { return };
    if pid == std::process::id() as i32 {
        return;
    }
    // SAFETY: pidfd_open makes a new descriptor, ours to close.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    // (By the display's name as the card listens on it: a display given
    // as a whole path has its last part for a name.)
    let told = instance::forward_on(APP_ID, &olo_xdg::display_name(), &["--put-away".to_owned()]);
    if fd < 0 {
        return;
    }
    if told {
        let until = std::time::Instant::now() + AWAY_WITHIN;
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now()).as_millis() as i32;
            // SAFETY: poll on the one pollfd above.
            let r = unsafe { libc::poll(&raw mut p, 1, left) };
            if r >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted || left == 0 {
                break;
            }
        }
    }
    // SAFETY: closing our own descriptor.
    unsafe { libc::close(fd) };
}

/// Where a picture shown in part is cut short.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cut {
    None,
    Right,
    Bottom,
}

/// How a picture is shown in the card at a scale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shown {
    /// The part of it shown, from its top left (its own pixels): all of
    /// it but for a strip.
    part: (u32, u32),
    /// The device pixels it is drawn in.
    pixels: (u32, u32),
    /// Each of its pixels a block of a whole number of device pixels
    /// (made by repeating them: never smoothed).
    blocks: bool,
    cut: Cut,
}

/// How a picture of `w` × `h` pixels is shown at `scale`, so that what
/// it is of can be told. Larger than the card's column: fitted whole
/// (its shape kept), where that leaves its short side tall enough to
/// see. A strip, whose short side would not be: at its own size (made
/// smaller only while its short side stays that tall), from its start,
/// its far end cut. Smaller than the column both ways: as blocks, the
/// least whole number of device pixels a pixel that makes it tall
/// enough, where that fits.
fn shown(w: u32, h: u32, scale: f32) -> Shown {
    let (w, h) = (w.max(1), h.max(1));
    // (What a frame of so many points has inside its two lines a side, in
    // device pixels: the frame's own arithmetic, `frame_size`, so that a
    // picture as wide as it may be has a frame exactly the column's width
    // at every scale. Counted from points less four, the frame was one to
    // three device pixels short of the column at 1.25×, 1.5× and 1.75×.)
    let lines = (4.0 * style::line_pixels(scale)) as u32;
    let inside = |points: f32| (((points * scale).round() as u32).saturating_sub(lines)).max(1);
    let (col, tall, least) = (inside(PICTURE_W), inside(PICTURE_MOST_H), inside(PICTURE_LEAST_H));
    if w <= col && h <= tall {
        // (As many points at every scale as the pixels allow: the least
        // whole number of points a pixel that makes its short side tall
        // enough at 1×, then the whole device pixels a pixel that come
        // nearest under it. Counted in device pixels alone it was 60
        // points wide at 2× and 80 at 1× and 1.5×.)
        let points = (PICTURE_LEAST_H - 4.0).round() as u32;
        let short = w.min(h);
        let by = ((points.div_ceil(short).max(1) as f32 * scale).floor() as u32).max(1);
        let by = if short * by < least { by + 1 } else { by }.min(col / w).min(tall / h).max(1);
        return Shown { part: (w, h), pixels: (w * by, h * by), blocks: by > 1, cut: Cut::None };
    }
    let by = (col as f32 / w as f32).min(tall as f32 / h as f32);
    let fit = (((w as f32 * by).round() as u32).clamp(1, col), ((h as f32 * by).round() as u32).clamp(1, tall));
    if fit.0.min(fit.1) >= least {
        return Shown { part: (w, h), pixels: fit, blocks: false, cut: Cut::None };
    }
    let by = (least as f32 / w.min(h) as f32).min(1.0);
    let whole = (((w as f32 * by).round() as u32).max(1), ((h as f32 * by).round() as u32).max(1));
    if w >= h {
        let across = whole.0.min(col);
        Shown { part: (((across as f32 / by).round() as u32).clamp(1, w), h), pixels: (across, whole.1.min(tall)), blocks: false, cut: if whole.0 > col { Cut::Right } else { Cut::None } }
    } else {
        let down = whole.1.min(tall);
        Shown { part: (w, ((down as f32 / by).round() as u32).clamp(1, h)), pixels: (whole.0.min(col), down), blocks: false, cut: if whole.1 > tall { Cut::Bottom } else { Cut::None } }
    }
}

/// The picture as [`shown`] says (BGRA, a pixel to a device pixel).
fn made(original: &image::RgbaImage, how: Shown) -> Option<Arc<RenderImage>> {
    if original.width() == 0 || original.height() == 0 {
        return None;
    }
    let part = (how.part.0.min(original.width()).max(1), how.part.1.min(original.height()).max(1));
    // (The part as a view of the picture, never a copy of it: a strip's
    // part can be most of a very large picture, to make a few pixels of.)
    let from = image::imageops::crop_imm(original, 0, 0, part.0, part.1);
    let small = if how.blocks {
        image::imageops::resize(&*from, how.pixels.0, how.pixels.1, image::imageops::FilterType::Nearest)
    } else if how.pixels == part {
        from.to_image()
    } else {
        image::imageops::thumbnail(&*from, how.pixels.0, how.pixels.1)
    };
    let (w, h) = (small.width(), small.height());
    let mut bgra = small.into_raw();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(w, h, bgra)?)])))
}

/// What a drag of the picture carries under the pointer: the picture
/// itself, small (128 points at most; a few pixels as blocks, 48 points
/// or so) and a little faded, as the kit's own drags are; the pointer in
/// its middle. Its size in device pixels is a whole number of points
/// times its scale: a buffer whose size isn't a multiple of its scale is
/// an error to the display (a 1000 × 667 window's at 2× was 256 × 171).
fn drag_picture(original: &image::RgbaImage, scale: f32) -> Option<DragImage> {
    let (size, by, blocks) = drag_size(original.width(), original.height(), scale)?;
    let small = if blocks { image::imageops::resize(original, size.0, size.1, image::imageops::FilterType::Nearest) } else { image::imageops::thumbnail(original, size.0, size.1) };
    let mut rgba = small.into_raw();
    // (Premultiplied, at seven tenths.)
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as f32 / 255.0 * 0.7;
        for c in &mut p[..3] {
            *c = (*c as f32 * a).round() as u8;
        }
        p[3] = (a * 255.0).round() as u8;
    }
    Some(DragImage { rgba: Arc::from(rgba), width: size.0, height: size.1, scale: by, hotspot: point(px(size.0 as f32 / by as f32 / 2.0), px(size.1 as f32 / by as f32 / 2.0)) })
}

/// A drag's picture for one of `w` × `h` pixels: its size in device
/// pixels, the whole scale its buffer has, and whether it is made of
/// blocks (a picture smaller than 48 points, made larger).
fn drag_size(w: u32, h: u32, scale: f32) -> Option<((u32, u32), u32, bool)> {
    if w == 0 || h == 0 {
        return None;
    }
    let by = scale.ceil().max(1.0) as u32;
    let long = w.max(h);
    // (In points, whole: then times the scale.)
    let points = |pixels: f32| ((pixels / by as f32).round() as u32).max(1);
    if long < 48 * by {
        let times = (48 * by).div_ceil(long);
        return Some(((points((w * times) as f32) * by, points((h * times) as f32) * by), by, true));
    }
    let shrink = ((128 * by) as f32 / long as f32).min(1.0);
    Some(((points(w as f32 * shrink) * by, points(h as f32 * shrink) * by), by, false))
}

/// A file's name cut in its middle to what `fits`, its end kept (the
/// kind, and the six letters before it: where names differ, a number, a
/// date), as Tracker cuts one. Whole where it fits.
fn middle_cut(name: &str, fits: &dyn Fn(&str) -> bool) -> String {
    if fits(name) {
        return name.to_owned();
    }
    // (Its kind: the last dot and at most five letters after it, counted
    // as letters.)
    let (stem, kind) = match name.rfind('.') {
        Some(at) if at > 0 && name[at..].chars().count() <= 6 => name.split_at(at),
        _ => (name, ""),
    };
    let letters: Vec<char> = stem.chars().collect();
    let kept = letters.len().saturating_sub(6);
    // (Its end from past a divider on: "….44.13.png" read as four dots.
    // Only a divider: six signs at a name's end are its end.)
    let end: String = letters[kept..].iter().collect::<String>().trim_start_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace()).to_owned() + kind;
    let with = |n: usize| format!("{}…{end}", letters[..n].iter().collect::<String>().trim_end());
    // (Where not even its end fits, the name whole: the line's own cut,
    // at its end, does what can be done.)
    if !fits(&with(0)) {
        return name.to_owned();
    }
    let (mut least, mut most) = (0, kept);
    while least < most {
        let half = (least + most).div_ceil(2);
        if fits(&with(half)) { least = half } else { most = half - 1 }
    }
    with(least)
}

/// A file as it was when its card came up (its device and its number
/// there): what the card deletes, copies and drags out is that file, and
/// not another that has its name by now.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Whether a card is shown here at all. Not on an Agent Screen: who
/// watches one sees its picture as a recording has it, the card left
/// out, and a click meant for what shows there would land on the card's
/// Delete.
pub fn shown_here() -> bool {
    std::env::var_os("OLO_SCREEN").is_none()
}

struct After {
    path: PathBuf,
    kind: Kind,
    name: String,
    /// The name as it fits its row (`middle_cut`), once measured.
    name_cut: Option<String>,
    /// "0:12 · 1280 × 800 · 4.2 MB".
    details: String,
    /// The file it came up for ([`identity`]).
    file: Option<(u64, u64)>,
    /// The card's column (points): the picture's frame at its widest, or
    /// the buttons' row where their words in this language need more.
    column: f32,
    /// The picture's size in its own pixels (a screen's shape where it
    /// isn't known).
    dims: (u32, u32),
    original: Option<Arc<image::RgbaImage>>,
    /// The picture as made, for which scale; and what a drag carries.
    picture: Option<(f32, Arc<RenderImage>)>,
    drag_picture: Option<DragImage>,
    /// A picture being made for this scale.
    asked: Option<f32>,
    close_lit: bool,
    /// The pointer is over the picture.
    picture_lit: bool,
    /// The wait for its going that counts (`Msg::Leave`).
    leaving: u64,
    /// Where a press on the picture began (points in the window).
    press: Option<Point<Pixels>>,
    /// The size the window was given (points).
    size: (f32, f32),
    /// Copied: the window is gone, the process serves the clipboard.
    holding: bool,
}

/// `m:ss` (or `h:mm:ss`).
fn length(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// "4.2 MB".
fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} bytes"),
    }
}

/// The frame of a picture shown as `how` (points): the picture and two
/// lines round it, on whole device pixels.
fn frame_size(how: Shown, scale: f32) -> (f32, f32) {
    let lines = 4.0 * style::line_pixels(scale);
    ((how.pixels.0 as f32 + lines) / scale, (how.pixels.1 as f32 + lines) / scale)
}

/// The card's size (points) round a frame `frame_h` tall in a column
/// `column` wide, its parts' positions following from it.
fn card_size(frame_h: f32, column: f32) -> (f32, f32) {
    (column + 2.0 * m::CARD_INSET, m::CARD_INSET + frame_h.ceil() + GAP_NAME + NAME_ROW + FACTS_ROW + GAP_BUTTONS + SMALL_BUTTON_HEIGHT + m::CARD_INSET)
}

/// Move `path` to the Trash (freedesktop's: Tracker restores it).
pub(crate) fn to_trash(path: &Path) -> std::io::Result<PathBuf> {
    let (target, info) = olo_xdg::trash::trash_slot(path)?;
    let moved = std::fs::rename(path, &target).or_else(|_| std::fs::copy(path, &target).and_then(|_| std::fs::remove_file(path)));
    if moved.is_err() {
        let _ = std::fs::remove_file(&info);
    }
    moved.map(|()| target)
}

/// The card's column for its buttons' words as they are shown (the
/// kit's own widths for them, in the person's language): the picture's
/// at least, and wide enough that Delete stands apart. (In French the
/// four need 304 points and in Russian 322, where the column had 280:
/// Delete ran into the rest, and Copy out of the card. The confirming
/// review.)
fn column_for(kind: Kind) -> f32 {
    let labels = ["Delete", "Show in Tracker", if kind == Kind::Picture { "Open" } else { "Edit" }, "Copy"];
    let row: f32 = labels.iter().map(|l| olo_kit::widgets::button_width(l, true)).sum::<f32>() + 2.0 * m::TIGHT + 2.0 * GAP_BUTTONS;
    row.max(PICTURE_W)
}

impl After {
    /// The file is still the one the card came up for.
    fn still_it(&self) -> bool {
        self.file.is_some() && identity(&self.path) == self.file
    }

    /// The picture made for `scale` on a worker (exact device pixels).
    fn ask_picture(&mut self, scale: f32, cx: &mut Cx<Self>) {
        let Some(original) = self.original.clone() else { return };
        if self.asked == Some(scale) || self.picture.as_ref().is_some_and(|(s, _)| *s == scale) {
            return;
        }
        self.asked = Some(scale);
        let how = shown(original.width(), original.height(), scale);
        cx.work(move || (scale, made(&original, how).map(|picture| (picture, drag_picture(&original, scale)))), |(scale, made)| Msg::Picture(scale, made));
    }

    /// It goes `STAYS` after the pointer left it. (One wait, armed as
    /// the pointer leaves and dropped as it comes back: a card under a
    /// resting pointer woke every twenty seconds to ask.)
    fn hold(&mut self, cx: &mut Cx<Self>) {
        self.leaving += 1;
        cx.after(STAYS, Msg::Leave(self.leaving));
    }

    fn show(&mut self) {
        let _ = olo_kit::tracker::open(&self.path);
        olo_kit::quit();
    }

    /// The file on the clipboard (as a file manager copies it), a
    /// picture as the picture itself first (what a chat or an editor
    /// pastes); the window goes, the process serving it till something
    /// else is copied.
    fn copy(&mut self, cx: &mut Cx<Self>) {
        if !self.still_it() {
            eprintln!("olo-screenshot: {} is not the file this card came up for any more: not copied", self.path.display());
            olo_kit::quit();
            return;
        }
        self.holding = true;
        HOLDING.store(true, Ordering::Release);
        // (No window from now on, and still running; and no card for a
        // new picture to put away: what it holds is the clipboard's.)
        olo_kit::stay_resident();
        instance::close();
        let (path, kind) = (self.path.clone(), self.kind);
        std::thread::spawn(move || {
            let mut clip = held_clipboard::Clip::new();
            if let (Kind::Picture, Some(mime)) = (kind, picture_type(&path))
                && let Ok(data) = std::fs::read(&path)
            {
                clip.push((mime.to_owned(), data.into()));
            }
            clip.extend(held_clipboard::files(std::slice::from_ref(&path), false));
            if let Err(e) = held_clipboard::hold(clip) {
                eprintln!("olo-screenshot: couldn't copy {}: {e}", path.display());
            }
            olo_kit::quit();
        });
        cx.close_window();
    }

    /// Edit: the recording in the Viewer's editor (`olo-viewer --edit`).
    /// Open: the picture in the program that opens its kind.
    fn edit(&mut self) {
        if self.kind == Kind::Picture {
            if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Path(self.path.clone())) {
                eprintln!("olo-screenshot: couldn't open {}: {e}", self.path.display());
            }
            olo_kit::quit();
            return;
        }
        match olo_xdg::find_program("olo-viewer") {
            Some(viewer) => {
                let argv = [viewer.into_os_string(), "--edit".into(), self.path.clone().into_os_string()];
                if let Err(e) = olo_xdg::open::spawn(&argv) {
                    eprintln!("olo-screenshot: couldn't open the editor: {e}");
                }
            }
            None => eprintln!("olo-screenshot: no Viewer to edit it in"),
        }
        olo_kit::quit();
    }

    fn delete(&mut self) {
        // (Another file that has its name by now is not this card's to
        // put in the Trash.)
        if !self.still_it() {
            eprintln!("olo-screenshot: {} is not the file this card came up for any more: left alone", self.path.display());
            olo_kit::quit();
            return;
        }
        if let Err(e) = to_trash(&self.path) {
            eprintln!("olo-screenshot: couldn't move {} to the Trash: {e}", self.path.display());
        }
        olo_kit::quit();
    }

    /// A press on the picture moved far enough: the file dragged out,
    /// its picture under the pointer (the kind's icon and its name where
    /// the picture isn't in yet).
    fn drag(&mut self, cx: &mut Cx<Self>) {
        self.press = None;
        if !self.still_it() {
            return;
        }
        let mut drag = dnd::files(std::slice::from_ref(&self.path), true, false);
        drag.icon =
            self.drag_picture.clone().or_else(|| dnd::picture(cx, if self.kind == Kind::Picture { "image-x-generic" } else { "video-x-generic" }, 48.0, Some(&self.name), point(px(24.0), px(24.0))));
        dnd::start(cx, drag, Msg::Dropped);
    }
}

impl View for After {
    type Message = Msg;

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            // (For the scale it was asked for, and shown only at that
            // one: a slow one for a scale since left would take the
            // place of the right one, and be asked for again.)
            Msg::Picture(scale, made) => {
                if self.asked == Some(scale) {
                    self.asked = None;
                }
                if scale == cx.scale_factor() {
                    match made {
                        Some((picture, drag)) => {
                            self.picture = Some((scale, picture));
                            self.drag_picture = drag;
                        }
                        None => self.picture = None,
                    }
                    cx.invalidate();
                }
            }
            Msg::NoPicture => {
                self.picture = None;
                cx.invalidate();
            }
            Msg::PutAway => {
                if !self.holding {
                    olo_kit::quit();
                }
            }
            Msg::Original(original) => {
                self.dims = (original.width(), original.height());
                self.original = Some(original);
                self.ask_picture(cx.scale_factor(), cx);
                cx.invalidate();
            }
            Msg::Leave(n) if n == self.leaving && !self.holding => olo_kit::quit(),
            Msg::Leave(_) => {}
            // Dropped somewhere: done with.
            Msg::Dropped(Some(_)) => olo_kit::quit(),
            // (A drag takes the pointer without its leaving being told:
            // it isn't over the card now, wherever it was let go.)
            Msg::Dropped(None) => {
                self.picture_lit = false;
                self.hold(cx);
                cx.invalidate();
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let scale = cx.scale_factor();
        self.ask_picture(scale, cx);
        let how = shown(self.dims.0, self.dims.1, scale);
        let (fw, fh) = frame_size(how, scale);
        let wide = self.column;
        let (cw, ch) = card_size(fh, wide);
        let (w, h) = (cw + 2.0 * SHADOW, ch + 2.0 * SHADOW);
        if (w, h) != self.size {
            self.size = (w, h);
            cx.resize_window(size(px(w), px(h)));
        }
        let card_at = point(px(SHADOW), px(SHADOW * 0.7));
        let column = card_at.x + px(m::CARD_INSET);
        let frame_at = point(column + px((wide - fw) / 2.0), card_at.y + px(m::CARD_INSET));
        let name_y = f32::from(card_at.y) + m::CARD_INSET + fh.ceil() + GAP_NAME;
        // (Its × has a margin of its own in its box: the box stands that
        // far past the column's edge, so the × ends on the edge; on the
        // name's row, its middle the name's. Measured in its pictures.)
        let close_at = point(column + px(wide - CLOSE + 3.0), px(name_y + (NAME_ROW - CLOSE) / 2.0));
        let name_w = wide - CLOSE - GAP_NAME;
        let name = self.name_cut.get_or_insert_with(|| middle_cut(&self.name, &|text| style::text_width(text.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD)) <= px(name_w))).clone();
        let ink = style::card_ink(&p);
        let back = cx
            .canvas(move |a: &After, g, b| {
                let p = theme::palette();
                let ink = style::card_ink(&p);
                let card = Bounds::new(b.origin + card_at, size(px(cw), px(ch)));
                // Over whatever is on the screen: a shadow that lifts it
                // (ending inside the window's room for it), then the
                // card's own.
                let radius = Corners::all(px(m::CARD_RADIUS));
                g.paint_shadows(card, radius, &[BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.3), offset: point(px(0.0), px(2.0)), blur_radius: px(5.0), spread_radius: px(0.0) }]);
                style::card(g, card, &p, false, false);
                // The frame on whole device pixels, its inside exactly the
                // picture's (made for this scale: a pixel to a pixel, and
                // no line of anything between it and its frame).
                let at = b.origin + frame_at;
                let frame = Bounds::new(point(style::snap_coord(g, at.x), style::snap_coord(g, at.y)), size(px(fw), px(fh)));
                let inner = style::inset(g, frame, 2.0);
                match a.picture.as_ref().filter(|(s, _)| *s == g.scale_factor()) {
                    Some((_, image)) => {
                        let _ = g.paint_image(inner, Corners::default(), image.clone(), 0, false);
                    }
                    None => style::fill_rect(g, inner, style::c(p.panel_background)),
                }
                // (Under the pointer its edge is the keyboard's colour, in
                // the frame's own lines, against the picture: it can be
                // taken hold of. A ring drawn round the frame left the
                // frame's lines between it and the picture.)
                style::sunken(g, frame, &p, a.picture_lit);
                // (A strip's far end goes out into the card's paper, its
                // frame's lines with it: there is more of it.)
                let clear = Hsla { a: 0.0, ..ink.card };
                match how.cut {
                    Cut::Right => {
                        let fade = px(FADE.min(fw / 2.0));
                        g.paint_quad(fill(
                            Bounds::new(point(frame.right() - fade, frame.origin.y), size(fade, frame.size.height)),
                            linear_gradient(90.0, linear_color_stop(clear, 0.0), linear_color_stop(ink.card, 1.0)),
                        ));
                    }
                    Cut::Bottom => {
                        let fade = px(FADE.min(fh / 2.0));
                        g.paint_quad(fill(
                            Bounds::new(point(frame.origin.x, frame.bottom() - fade), size(frame.size.width, fade)),
                            linear_gradient(180.0, linear_color_stop(clear, 0.0), linear_color_stop(ink.card, 1.0)),
                        ));
                    }
                    Cut::None => {}
                }
                olo_kit::agent::record(
                    g,
                    olo_kit::agent::Node {
                        role: "Image",
                        name: Some(format!("{}: {} (drag it out as the file)", if a.kind == Kind::Picture { "Screenshot" } else { "Recording" }, a.name).into()),
                        bounds: frame,
                        enabled: true,
                        ..Default::default()
                    },
                );
                let close = Bounds::new(b.origin + close_at, size(px(CLOSE), px(CLOSE)));
                style::close_box(g, close, if a.close_lit { style::c(p.panel_text) } else { style::disabled_text(&p) });
            })
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full();
        div()
            .id("after")
            .size_full()
            .relative()
            // (Its wait is dropped as the pointer comes, and begins as it goes.)
            .on_hover(cx.listener(|a: &mut After, inside: &bool, cx| {
                if *inside {
                    a.leaving += 1;
                } else {
                    a.hold(cx);
                }
            }))
            .child(back)
            // The picture: pressed and moved, the file dragged out. (A
            // little more than its frame to take hold of: a strip's is
            // thin.)
            .child(
                div()
                    .id("picture")
                    .absolute()
                    .left(frame_at.x)
                    .top(frame_at.y - px(4.0))
                    .w(px(fw))
                    .h(px(fh + 8.0))
                    .cursor(CursorStyle::OpenHand)
                    .on_hover(cx.listener(|a: &mut After, inside: &bool, cx| {
                        a.picture_lit = *inside;
                        cx.invalidate();
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(|a: &mut After, e: &MouseDownEvent, _| a.press = Some(e.position)))
                    .moves_matter_while_pressed()
                    .on_mouse_move(cx.listener(|a: &mut After, e: &MouseMoveEvent, cx| {
                        let Some(at) = a.press else { return };
                        if e.pressed_button != Some(MouseButton::Left) {
                            a.press = None;
                        } else if f32::from((e.position - at).x).hypot(f32::from((e.position - at).y)) >= DRAG_AFTER {
                            a.drag(cx);
                        }
                    })),
            )
            // The name (cut in its middle to its room, its end kept) and
            // the facts under it, with the picture they are of.
            .child(
                div()
                    .absolute()
                    .left(column)
                    .top(px(name_y))
                    .w(px(name_w))
                    .flex()
                    .flex_col()
                    .child(StringView::new(name).color(ink.text).bold(true).height(NAME_ROW).cut(true))
                    .child(StringView::new(self.details.clone()).small(true).dim(true).height(FACTS_ROW).cut(true)),
            )
            .child(
                div()
                    .id("close")
                    .absolute()
                    .left(close_at.x)
                    .top(close_at.y)
                    .w(px(CLOSE))
                    .h(px(CLOSE))
                    .child(canvas(|g, b| olo_kit::agent::record(g, olo_kit::agent::Node::button("Close", b))).size_full())
                    .on_hover(cx.listener(|a: &mut After, inside: &bool, cx| {
                        a.close_lit = *inside;
                        cx.invalidate();
                    }))
                    .on_click(|_: &ClickEvent, _| olo_kit::quit()),
            )
            // Delete by itself at the row's start, far from the close
            // box; the rest at its end, Copy last, where a pointer coming
            // from the screen meets it first.
            .child(
                div()
                    .absolute()
                    .left(column)
                    .top(px(name_y + NAME_ROW + FACTS_ROW + GAP_BUTTONS))
                    .w(px(wide))
                    .flex()
                    .justify_between()
                    .child(Button::new("Delete").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.delete())))
                    .child(
                        div()
                            .flex()
                            .gap(px(m::TIGHT))
                            .child(Button::new("Show in Tracker").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.show())))
                            .child(Button::new(if self.kind == Kind::Picture { "Open" } else { "Edit" }).small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.edit())))
                            .child(Button::new("Copy").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, cx| a.copy(cx)))),
                    ),
            )
    }
}

/// `--after PATH [--picture] [--display NAME]`: the card, till it goes.
pub fn run(args: &[String]) -> std::process::ExitCode {
    let Some(path) = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).map(PathBuf::from) else {
        eprintln!("olo-screenshot: --after takes the recording's or the picture's path");
        return std::process::ExitCode::from(2);
    };
    // (Whole from the root: what is dragged out and copied is a file's
    // address, and a name alone is nobody's.)
    let path = std::path::absolute(&path).unwrap_or(path);
    let kind = if args.iter().any(|a| a == "--picture") { Kind::Picture } else { Kind::Recording };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let on = args.iter().position(|a| a == "--display").and_then(|i| args.get(i + 1)).cloned();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let input = olo_preview::input::Input::file(&path);
    // What it is (quick: its index), here; its picture on a worker.
    let still = kind == Kind::Picture || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    let (seconds, width, height) = if still {
        let dims = image::ImageReader::open(&path).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.into_dimensions().ok()).unwrap_or((0, 0));
        (None, dims.0, dims.1)
    } else {
        match olo_preview::video::probe(&input) {
            Ok(info) => (info.duration, info.width, info.height),
            Err(_) => (None, 0, 0),
        }
    };
    let mut details = Vec::new();
    if let Some(s) = seconds {
        details.push(length(s));
    }
    if width > 0 {
        details.push(format!("{width} × {height}"));
    }
    details.push(bytes(size));
    // (Not known: a screen's shape, empty.)
    let dims = if width > 0 && height > 0 { (width, height) } else { (1280, 800) };
    // One card at a time: the one showing goes first.
    put_away();
    let file = identity(&path);
    olo_kit::upgrade_waits_while(|| HOLDING.load(Ordering::Acquire));
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let column = column_for(kind);
        let (cw, ch) = card_size(frame_size(shown(dims.0, dims.1, 1.0), 1.0).1, column);
        let options = WindowOptions::new(if kind == Kind::Picture { "Screenshot Saved" } else { "Recording Saved" }, cw + 2.0 * SHADOW, ch + 2.0 * SHADOW)
            .no_panel()
            .layer(AFTER_NAMESPACE, LayerShellLayer::Top, (false, true, true, false))
            .layer_options(move |l| {
                // (On the display that was recorded, or pictured.)
                l.output = on;
                // (The card `MARGIN` from the edges: its shadow's room inside.)
                l.margin = (0, 0, (MARGIN - SHADOW * 1.3).max(0.0).round() as i32, (MARGIN - SHADOW).round() as i32);
                l.keyboard = LayerKeyboard::None;
            });
        let shown_path = path.clone();
        let window = olo_kit::open_window(options, move |cx| {
            let mut after = After {
                path: shown_path,
                kind,
                name,
                name_cut: None,
                details: details.join(" · "),
                file,
                column,
                dims,
                original: None,
                picture: None,
                drag_picture: None,
                asked: None,
                close_lit: false,
                picture_lit: false,
                leaving: 0,
                press: None,
                size: (0.0, 0.0),
                holding: false,
            };
            after.hold(cx);
            after
        });
        // (Told to go by whatever takes the next picture, or by the next
        // card: any launch that reaches this one is that. The window
        // decides, on its own looper, where Copy is: decided here, a
        // card just copied from was ended between the look and the end.)
        let asked = window.messenger();
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                asked.send(Msg::PutAway);
            }
        });
        // The picture: a recording's a moment in (past a countdown's last frame).
        let to = window.messenger();
        std::thread::spawn(move || {
            let picture = if still {
                image::open(&path).ok().map(|i| i.into_rgba8())
            } else {
                let at = seconds.map_or(0.0, |s| (s * 0.1).min(1.0));
                olo_preview::video::picture_at(&input, at).ok().and_then(|b| image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.rgba.clone()))
            };
            match picture.filter(|p| p.width() > 0 && p.height() > 0) {
                Some(p) => to.send(Msg::Original(Arc::new(p))),
                None => to.send(Msg::NoPicture),
            };
        });
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_and_how_big() {
        assert_eq!(length(4.4), "0:04");
        assert_eq!(length(61.6), "1:02");
        assert_eq!(length(3725.0), "1:02:05");
        assert_eq!(bytes(900), "900 bytes");
        assert_eq!(bytes(52 * 1024), "52 KB");
        assert_eq!(bytes(4_400_000), "4.2 MB");
    }

    /// How a picture is shown, so that it can be told what it is of.
    #[test]
    fn a_picture_is_shown_so_that_it_can_be_seen() {
        let plain = |part, pixels| Shown { part, pixels, blocks: false, cut: Cut::None };
        // A 16:10 screen's picture fills the column (276 × 173 points inside its frame), at each scale.
        assert_eq!(shown(1280, 800, 1.0), plain((1280, 800), (276, 173)));
        assert_eq!(shown(2560, 1600, 2.0), plain((2560, 1600), (552, 345)));
        assert_eq!(shown(2240, 1400, 1.75), plain((2240, 1400), (486, 304)));
        // A tall window's: as tall as the frame may be, as wide as its shape says.
        assert_eq!(shown(426, 720, 1.0), plain((426, 720), (102, 173)));
        // A few pixels dragged out of the screen: blocks of whole device pixels, 80 × 60 points at 1×, 1.5×
        // and 2× (64 × 48 at 1.25×, 69 × 51 at 1.75×).
        let blocks = |pixels| Shown { part: (40, 30), pixels, blocks: true, cut: Cut::None };
        assert_eq!(shown(40, 30, 1.0), blocks((80, 60)));
        assert_eq!(shown(40, 30, 1.25), blocks((80, 60)));
        assert_eq!(shown(40, 30, 1.5), blocks((120, 90)));
        assert_eq!(shown(40, 30, 1.75), blocks((120, 90)));
        assert_eq!(shown(40, 30, 2.0), blocks((160, 120)));
        // (By its short side, whichever that is: a 4 × 8 was 24 × 48, an 8 × 4 88 × 44.)
        assert_eq!(shown(4, 8, 1.0).pixels, (44, 88));
        assert_eq!(shown(8, 4, 1.0).pixels, (88, 44));
        // (One that is already tall enough: as it is.)
        assert_eq!(shown(200, 100, 1.0), plain((200, 100), (200, 100)));
        // (One no blocks of which would fit: as it is.)
        assert_eq!(shown(270, 10, 1.0), plain((270, 10), (270, 10)));
        // A strip: at its own size from its start, its far end cut (never fitted whole into a line).
        assert_eq!(shown(1280, 20, 1.0), Shown { part: (276, 20), pixels: (276, 20), blocks: false, cut: Cut::Right });
        assert_eq!(shown(20, 1200, 1.0), Shown { part: (20, 173), pixels: (20, 173), blocks: false, cut: Cut::Bottom });
        // (A long one that is thick: made smaller only until its short side is 44 points.)
        assert_eq!(shown(4000, 100, 1.0), Shown { part: (627, 100), pixels: (276, 44), blocks: false, cut: Cut::Right });
        // Nothing: no division by nothing.
        assert_eq!(shown(0, 0, 1.0).pixels, (44, 44));
        // A picture as wide as it may be has a frame exactly the column's width in device pixels, at every
        // scale (it was one to three short at 1.25×, 1.5× and 1.75×), and the frame is the picture and two
        // lines round it.
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let how = shown((1280.0 * scale) as u32, (800.0 * scale) as u32, scale);
            let (w, h) = frame_size(how, scale);
            assert_eq!((w * scale).round(), (PICTURE_W * scale).round(), "{scale}");
            assert_eq!(((w * scale).round(), (h * scale).round()), (how.pixels.0 as f32 + 4.0 * style::line_pixels(scale), how.pixels.1 as f32 + 4.0 * style::line_pixels(scale)), "{scale}");
        }
    }

    /// A drag's picture has a size its buffer may have: a whole number of points times its scale.
    #[test]
    fn a_drags_picture_is_whole_points() {
        // (A 1000 × 667 window's at 2× was 256 × 171: no multiple of 2.)
        for (w, h, scale) in [(1000, 667, 2.0), (1000, 667, 1.5), (1280, 800, 1.0), (333, 1001, 1.75), (3, 1001, 2.0), (4, 8, 2.0), (1, 1, 3.0)] {
            let ((dw, dh), by, _) = drag_size(w, h, scale).unwrap();
            assert!(dw > 0 && dh > 0 && dw % by == 0 && dh % by == 0 && by == scale.ceil() as u32, "{w} × {h} at {scale}: {dw} × {dh}, scale {by}");
            assert!(dw.max(dh) <= 128 * by + by, "{w} × {h} at {scale}: {dw} × {dh}");
        }
        // A few pixels are carried as blocks, large enough to see; nothing is nothing.
        assert_eq!(drag_size(4, 8, 1.0), Some(((24, 48), 1, true)));
        assert_eq!(drag_size(0, 8, 1.0), None);
        let tiny = image::RgbaImage::from_pixel(4, 8, image::Rgba([10, 20, 30, 255]));
        let carried = drag_picture(&tiny, 2.0).unwrap();
        assert_eq!((carried.width, carried.height, carried.scale, carried.rgba.len()), (48, 96, 2, 48 * 96 * 4));
    }

    /// The file a card acts on is the one it came up for.
    #[test]
    fn a_card_knows_its_file_from_another_of_its_name() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-shot-card-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("screenshot1.png");
        std::fs::write(&path, b"one").unwrap();
        let was = identity(&path);
        assert!(was.is_some() && identity(&path) == was);
        // Moved away, and another put under its name: not the same file.
        std::fs::rename(&path, dir.join("kept.png")).unwrap();
        assert_eq!(identity(&path), None);
        std::fs::write(&path, b"two").unwrap();
        assert!(identity(&path).is_some() && identity(&path) != was);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The picture made is exactly the pixels asked for: blocks repeated, a strip's start, never smoothed.
    #[test]
    fn the_picture_made_is_the_pixels_shown() {
        let mut tiny = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]));
        tiny.put_pixel(1, 0, image::Rgba([200, 100, 50, 255]));
        let blocks = made(&tiny, Shown { part: (2, 1), pixels: (4, 2), blocks: true, cut: Cut::None }).unwrap();
        // (BGRA: each of its two pixels a block of two by two, nothing between them.)
        assert_eq!(blocks.as_bytes(0).unwrap(), [30, 20, 10, 255, 30, 20, 10, 255, 50, 100, 200, 255, 50, 100, 200, 255].repeat(2));
        let strip = made(&tiny, Shown { part: (1, 1), pixels: (1, 1), blocks: false, cut: Cut::Right }).unwrap();
        assert_eq!(strip.as_bytes(0).unwrap(), [30, 20, 10, 255]);
        // A picture with nothing in it makes none (it made one of the wrong size).
        assert!(made(&image::RgbaImage::new(0, 50), shown(0, 50, 1.0)).is_none());
        // A long strip's start is made from a view of it, its few pixels alone: the part it shows.
        let long = image::RgbaImage::from_pixel(4000, 100, image::Rgba([1, 2, 3, 255]));
        let how = shown(4000, 100, 1.0);
        assert_eq!(made(&long, how).unwrap().size(0), olo_kit::size(olo_kit::DevicePixels(276), olo_kit::DevicePixels(44)));
    }

    /// A long name is cut in its middle, its end kept.
    #[test]
    fn a_name_is_cut_in_its_middle() {
        let fits = |most: usize| move |text: &str| text.chars().count() <= most;
        assert_eq!(middle_cut("screenshot1.png", &fits(40)), "screenshot1.png");
        assert_eq!(middle_cut("Screenshot of the quarterly planning board.png", &fits(30)), "Screenshot of the qu…board.png");
        assert_eq!(middle_cut("Screenshot 2026-11-27 at 16.44.13.png", &fits(24)), "Screenshot 202…44.13.png");
        // (No kind: its last six letters. Nothing fits: its end alone.)
        assert_eq!(middle_cut("a-very-long-name-with-no-kind", &fits(12)), "a-ver…o-kind");
        // (Not even its end fits: whole, for the line's own cut at its end. It was given back longer
        // than its room.)
        assert_eq!(middle_cut("abcdefghijkl.png", &fits(3)), "abcdefghijkl.png");
        // Six signs at a name's end are its end, kept; a kind is counted in letters.
        assert_eq!(middle_cut("holiday pictures 🎉🎉🎉🎉🎉🎉.png", &fits(20)), "holiday p…🎉🎉🎉🎉🎉🎉.png");
        assert_eq!(middle_cut("screenshot of the board.画像", &fits(18)), "screensho…board.画像");
    }

    #[test]
    fn a_pictures_type_is_told_by_its_name() {
        for (name, mime) in [
            ("screenshot3.png", Some("image/png")),
            ("Shot.JPG", Some("image/jpeg")),
            ("a.webp", Some("image/webp")),
            ("a.tiff", Some("image/tiff")),
            ("a.bmp", Some("image/bmp")),
            ("recording2.mp4", None),
            ("none", None),
        ] {
            assert_eq!(picture_type(Path::new(name)), mime, "{name}");
        }
    }
}
