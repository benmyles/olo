//! What follows a picture or a recording saved (Ben, 2026-11-28: "when i
//! take a screenshot the thumbnail appears in the bottom left like it's
//! not a proper notification it should have been a notification in the
//! bottom right that uses the existing notification paths and it should
//! support dragging and dropping into the destination"): a notification,
//! through the desktop's notifications (Olo's: apps/notifications), in
//! the screen's lower right with the rest, under Screenshot's name and
//! icon, as Screenshot's choices in Preferences ▸ Notifications and Do
//! Not Disturb have it, and kept in the list. Its picture shown large,
//! whole (a recording's: a moment in, [`poster`]); pressed and dragged
//! out of the notification, the picture is the file (a recording's, its
//! video): to a folder, a chat, a mail. A click opens it; Delete, set
//! apart at the start of its row, and Copy, and in its menu and in the
//! list Show in Tracker too (and a video's Edit).
//!
//! Its actions work for as long as it's in the list, an hour on as well
//! as at once: while this program runs (holding the path it put on the
//! clipboard) they come back to it; once it has gone, the notifications
//! centre starts it again for each (`x-olo-runs`, `olo-screenshot --saved
//! ACTION PATH RECEIPT`). Either way the same ([`act`]), and only on the
//! very file that save made, while it is still that file: its receipt is
//! the file as it was saved ([`FileId`]), kept as one of Screenshot's own
//! as it's saved ([`remember`]). Any program on the bus may send a
//! notification that says it's Screenshot's, and have its buttons start
//! Screenshot with what it likes; and a name is given again (delete a
//! screenshot, take another: it has the first free name), so a name
//! alone is no file's.
//!
//! Never in a picture: whatever is about to take a picture of the screen
//! (a screenshot, the region picker, a recording) has Screenshot's
//! banners put away first, and waits until they're off the screen
//! ([`put_away`]); they stay in the list. Another app's notification is
//! a picture of it, as the person may want.

use std::{
    io::{Read as _, Write as _},
    os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use olo_kit::{
    held_clipboard,
    notify::{Note, NoteEvent, Notifier},
};
use olo_protocol::notifications::FileId;
use serde::{Deserialize, Serialize};

/// Screenshot's desktop entry: its notifications are its own (its name
/// and icon, its choices in Preferences ▸ Notifications).
pub const ENTRY: &str = "olo-screenshot";
/// How long whatever is about to take a picture waits for Screenshot's
/// banners to go.
const AWAY_WITHIN: Duration = Duration::from_millis(1500);
/// How long a program about to end waits for its notification to have
/// been sent.
const SENT_WITHIN: Duration = Duration::from_secs(3);
/// A recording's picture is made this large at most (pixels on a side):
/// a banner's at 2× and a wide card's in the list.
const POSTER: u32 = 640;
/// Posters of recordings no longer kept go once they're this old (the
/// list keeps a month of what's cleared).
const POSTERS_KEPT: Duration = Duration::from_secs(31 * 24 * 3600);
/// Past this many saved files kept, those no longer there (or no longer
/// that file) go: nothing could be done to them.
const KEPT: usize = 1000;
/// And past this many, the oldest: what's kept stays quick to read.
const KEPT_MOST: usize = 20_000;
/// The most of a picture Copy puts on the clipboard as itself (bytes; a
/// larger one goes as its file alone).
const COPY_MOST: u64 = 128 << 20;

/// What a notification is of: what Copy offers, how its picture is
/// made, and whether it may be edited as a video. Kept with each file
/// saved; said by whoever saved it, never by its name (a recording may
/// be given any name, a video one ending ".gif").
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A video.
    Recording,
    /// A recording made as a GIF.
    Gif,
    /// A screenshot.
    Picture,
}

impl Kind {
    /// A recording (a video or a GIF made of one): what plays.
    fn plays(self) -> bool {
        matches!(self, Kind::Recording | Kind::Gif)
    }
}

/// What's known of a file Screenshot tells of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Identity {
    /// Just saved by Screenshot: the file as it was written (read from
    /// the file itself as it was).
    Written(FileId),
    /// Just saved, but not known as written (it changed as it took its
    /// name): shown, and nothing offered to do to it.
    Unknown,
    /// Named by whoever asked (`--after`): what's offered to do to it, only
    /// if Screenshot already kept that very file.
    Named,
}

/// Whether a notification is shown here at all. Not on an Agent Screen:
/// who watches one sees its pictures as a recording has them, and a
/// click meant for what shows there would land on a banner's Delete.
/// Nor for the Screenshot portal's (`--for-app`: the app that asked has
/// the picture), nor where whoever runs this says so (`--no-card`, a
/// script's own picture; `--no-notification`).
pub fn shown_here() -> bool {
    std::env::var_os("OLO_SCREEN").is_none() && !std::env::args().any(|a| a == "--for-app" || a == "--no-card" || a == "--no-notification")
}

/// Screenshot's banners off the screen, and this returns once they are
/// (`AWAY_WITHIN` at most: nothing waits on them for ever): before a
/// picture of the screen is taken. They stay in the list, unread, their
/// actions as they were.
pub fn put_away() {
    if !olo_kit::notify::put_away(ENTRY, AWAY_WITHIN) {
        tracing::debug!("screenshot: its banners weren't seen to go before the picture");
    }
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

/// Whether the open file is a GIF by what it holds (its first bytes say
/// so), for one whose recorder didn't say.
fn gif_by_its_bytes(file: &std::fs::File) -> bool {
    use std::os::unix::fs::FileExt as _;
    let mut head = [0u8; 6];
    file.read_exact_at(&mut head, 0).is_ok() && (&head == b"GIF87a" || &head == b"GIF89a")
}

/// A file opened to be read, never through a link and never waited on
/// (a pipe given its name), if it's a file of its own: for one whose
/// receipt isn't known.
fn open_plain(path: &Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path).ok()?;
    file.metadata().is_ok_and(|m| m.file_type().is_file()).then_some(file)
}

/// What a media reader reads the open `file` as: the very file opened,
/// whatever has its name since (each reader its own, opened again
/// through the descriptor, which this keeps open while it's used).
fn input_of(file: &std::sync::Arc<std::fs::File>, path: &Path) -> olo_preview::input::Input {
    let file = file.clone();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    olo_preview::input::Input::new(&ext, &path.display().to_string(), move || {
        let f = std::fs::File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        let size = f.metadata()?.len();
        Ok((Box::new(f) as Box<dyn olo_preview::input::ReadSeek>, size))
    })
}

/// A picture file's size in pixels, from its header, read from the open
/// file.
fn dimensions_of(file: &std::sync::Arc<std::fs::File>) -> Option<(u32, u32)> {
    let f = std::fs::File::open(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
    image::ImageReader::new(std::io::BufReader::new(f)).with_guessed_format().ok()?.into_dimensions().ok()
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

/// What's said of a file under its name: its size in pixels and its
/// bytes ("1280 × 800 · 4.2 MB"; a recording's length is on its picture).
fn details((width, height): (u32, u32), size: u64) -> String {
    let mut said = Vec::new();
    if width > 0 && height > 0 {
        said.push(format!("{width} × {height}"));
    }
    said.push(bytes(size));
    said.join(" · ")
}

/// Move `path` to the Trash (freedesktop's: Tracker restores it), into a
/// place reserved for it alone (`reserve_slot`): renamed, never over
/// anything; to another file system, copied into a file made new for it.
pub(crate) fn to_trash(path: &Path) -> std::io::Result<PathBuf> {
    let (target, info) = reserve_slot(path)?;
    let moved = match rename_new(path, &target) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => copy_then_remove(path, &target),
        other => other,
    };
    if moved.is_err() {
        let _ = std::fs::remove_file(&info);
    }
    moved.map(|()| target)
}

/// `from` renamed `to`, never over a file there (`RENAME_NOREPLACE`).
pub(crate) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput));
    let (from, to) = (c(from)?, c(to)?);
    // SAFETY: two NUL-terminated paths that live across the call.
    if unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Whether the entry at `path` is the file saved as `receipt` (a file of
/// its own, not a link).
fn is_saved(path: &Path, receipt: FileId) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file() && FileId::of_meta(&m) == receipt)
}

/// The file saved as `receipt` at `path` moved to the Trash, and nothing
/// else, ever. Another file of its name is left where it is, untouched.
/// The very file saved is moved aside first, under a short name nothing
/// else has, looked at again there (a moment's swap is caught), and goes
/// on to the Trash (`slot`: a place there and its record of where it
/// was, reserved for this alone, `reserve_slot`): renamed, never over
/// anything; copied only to another file system, into a file this makes
/// new, the original kept until the copy is whole. Anything else is put
/// back, never over a file that has come since: then beside it, under a
/// name of its own, and said where. Only what this made is ever removed.
fn trash_saved(path: &Path, receipt: FileId, slot: impl FnOnce(&Path) -> std::io::Result<(PathBuf, PathBuf)>) -> Result<PathBuf, String> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err(format!("{} names no file", path.display())) };
    let not_it = || format!("{} is not the file Screenshot saved there (another has its name now): left alone", path.display());
    // (Another file of its name: not moved at all, not even for a moment.)
    if !is_saved(path, receipt) {
        return Err(not_it());
    }
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let aside = dir.join(format!(".olo-trashing-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    rename_new(path, &aside).map_err(|e| format!("couldn't move {} to the Trash: {e}", path.display()))?;
    let put_back = |why: String| match rename_new(&aside, path) {
        Ok(()) => why,
        // (Its name taken meanwhile: beside it, where it can be seen.)
        Err(_) => match rename_new(&aside, &olo_xdg::trash::unique(dir, &name.to_string_lossy())) {
            Ok(()) => format!("{why}; another file has its name now, so it's beside it under another"),
            Err(e) => format!("{why}; what was there is at {} ({e})", aside.display()),
        },
    };
    if !is_saved(&aside, receipt) {
        return Err(put_back(not_it()));
    }
    let (target, info) = slot(path).map_err(|e| put_back(format!("couldn't move {} to the Trash: {e}", path.display())))?;
    let moved = match rename_new(&aside, &target) {
        Ok(()) => Ok(()),
        // Another file system: copied into a file made new for it, the
        // original kept until the copy is whole.
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => copy_then_remove(&aside, &target),
        // (A place taken after all: nothing of it is touched.)
        Err(e) => Err(e),
    };
    match moved {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = std::fs::remove_file(&info);
            Err(put_back(format!("couldn't move {} to the Trash: {e}", path.display())))
        }
    }
}

/// `from` copied to `to`, a file made new for it (never one that's
/// there), synced; on failure, what this made is removed.
fn copy_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut source = std::fs::File::open(from)?;
    let mut made = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    // (Its permissions as they were: a recording kept private stays so.)
    let copied = std::io::copy(&mut source, &mut made).and_then(|_| made.set_permissions(source.metadata()?.permissions())).and_then(|()| made.sync_all());
    if copied.is_err() {
        let _ = std::fs::remove_file(to);
    }
    copied
}

/// `from` copied whole to `to` (`copy_new`), then `from` removed; if it
/// can't be, the copy goes too (never a file in the Trash with no record,
/// beside the original put back).
fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()> {
    copy_new(from, to)?;
    std::fs::remove_file(from).inspect_err(|_| {
        let _ = std::fs::remove_file(to);
    })
}

/// A place in the freedesktop Trash for `path`, and its record of where
/// it was, reserved for this Delete alone: the record made new (`O_EXCL`,
/// as the Trash's own specification asks), a taken name passed over for
/// the next. Two Deletes of files of one name never share a place.
fn reserve_slot(path: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    reserve_slot_in(&olo_xdg::trash::trash_dir(), path)
}

/// [`reserve_slot`] in the Trash at `trash`.
fn reserve_slot_in(trash: &Path, path: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    use std::io::Write as _;
    let files = trash.join("files");
    let info = trash.join("info");
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    // (Its full path is in the record; its name in the Trash is cut short,
    // on a character's edge, so that the record's own name, with
    // ".trashinfo" and a number after it, fits a file name's 255 bytes.)
    let stem = clamp_on_char(&stem, NAME_MAX.saturating_sub(ext.len() + ".trashinfo".len() + " 9999".len()));
    let said = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", trash_escape(&std::path::absolute(path)?), local_now());
    for i in 1..10_000 {
        let tname = if i == 1 { format!("{stem}{ext}") } else { format!("{stem} {i}{ext}") };
        let target = files.join(&tname);
        if std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        let record = info.join(format!("{tname}.trashinfo"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&record) {
            Ok(mut f) => {
                // (Not written, the disk full: said at once, nothing left;
                // never taken for a name taken, nine thousand records made
                // and removed before "no free place".)
                if let Err(e) = f.write_all(said.as_bytes()) {
                    let _ = std::fs::remove_file(&record);
                    return Err(e);
                }
                // (Its place taken without a record meanwhile: let go of.)
                if std::fs::symlink_metadata(&target).is_ok() {
                    let _ = std::fs::remove_file(&record);
                    continue;
                }
                return Ok((target, record));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free place in the Trash"))
}

/// The longest file name (bytes).
const NAME_MAX: usize = 255;

/// `s` cut to `most` bytes at most, on a character's edge.
fn clamp_on_char(s: &str, most: usize) -> &str {
    if s.len() <= most {
        return s;
    }
    let mut end = most;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A path as the Trash's records have it (URL-style escapes).
fn trash_escape(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Now, local, as the Trash's records have it (`2026-11-28T09:30:00`).
fn local_now() -> String {
    // SAFETY: localtime_r writes the zeroed `tm` it's given; `time` takes null.
    let tm = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The file saved as `receipt` at `path`, opened: a file of its own (not
/// a link, a pipe or a device; never waited on) and the very one saved.
/// What's read from it is that file's, whatever has its name since.
fn open_saved(path: &Path, receipt: FileId) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path).map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
    let m = file.metadata().map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
    if !m.file_type().is_file() || FileId::of_meta(&m) != receipt {
        return Err(format!("{} is not the file Screenshot saved there (another has its name now): left alone", path.display()));
    }
    Ok(file)
}

/// One file Screenshot saved, as it was then (its receipt), and what it
/// is.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct Kept {
    path: PathBuf,
    id: FileId,
    kind: Kind,
}

/// Where what Screenshot saved is kept (a line each), and the lock its
/// writers take.
fn record() -> (PathBuf, PathBuf) {
    let dir = olo_xdg::state_dir().join("screenshot");
    (dir.join("saved.jsonl"), dir.join("saved.lock"))
}

/// Holds `lock` (its file, made if need be) while `f` runs: two pictures
/// saved at once (two shortcuts pressed) each keep their line, and a
/// record made shorter never loses one.
fn locked<R>(lock: &Path, f: impl FnOnce() -> R) -> std::io::Result<R> {
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock)?;
    // SAFETY: flock on a descriptor we hold open for as long as it's held.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let r = f();
    // (Let go of with the file, closed as it's dropped.)
    drop(file);
    Ok(r)
}

/// What's kept, oldest first: each line read as bytes, one that isn't a
/// whole record (cut short by a crash, or not text) passed over, and
/// what follows it read all the same. Nothing kept yet: none.
fn read_kept(at: &Path) -> std::io::Result<Vec<Kept>> {
    let mut bytes = Vec::new();
    match std::fs::File::open(at) {
        Ok(mut f) => {
            f.read_to_end(&mut bytes)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    }
    Ok(bytes.split(|b| *b == b'\n').filter_map(|line| serde_json::from_slice::<Kept>(line).ok()).collect())
}

/// Keeps `path`, just saved by Screenshot as the file `id` (read from
/// the file it wrote), as one of its own: what its notification's
/// actions may act on ([`act`]), for as long as it's that file. Past
/// `KEPT`, those no longer there go (past `KEPT_MOST`, the oldest).
/// The receipt its actions carry; None if it couldn't be kept (its
/// notification then offers nothing that couldn't be done).
pub fn remember(path: &Path, id: FileId, kind: Kind) -> Option<FileId> {
    match remember_in(&record(), path, id, kind) {
        Ok(()) => Some(id),
        Err(e) => {
            eprintln!("olo-screenshot: couldn't keep {} as one of Screenshot's (its notification offers nothing to do to it): {e}", path.display());
            None
        }
    }
}

fn remember_in((at, lock): &(PathBuf, PathBuf), path: &Path, id: FileId, kind: Kind) -> std::io::Result<()> {
    locked(lock, || -> std::io::Result<()> {
        let mut kept = read_kept(at)?;
        kept.retain(|k| k.path != path || k.id != id);
        kept.push(Kept { path: path.to_owned(), id, kind });
        if kept.len() > KEPT {
            kept.retain(|k| k.path == path && k.id == id || FileId::of(&k.path) == Some(k.id));
        }
        if kept.len() > KEPT_MOST {
            kept.drain(..kept.len() - KEPT_MOST);
        }
        // (Written whole and put in its place: read at any moment, it's
        // what was or what is, and a line cut short is gone with it.)
        let mut text = Vec::new();
        for k in &kept {
            serde_json::to_writer(&mut text, k)?;
            text.push(b'\n');
        }
        let new = at.with_extension("jsonl.new");
        let mut f = std::fs::File::create(&new)?;
        f.write_all(&text)?;
        f.sync_all()?;
        std::fs::rename(&new, at)
    })?
}

/// What Screenshot saved as `receipt` at `path`, if it did.
fn kept(at: &Path, path: &Path, receipt: FileId) -> Option<Kind> {
    read_kept(at).ok()?.into_iter().rev().find(|k| k.path == path && k.id == receipt).map(|k| k.kind)
}

/// The file at `path` now, if Screenshot saved it (and it's still that
/// file): its receipt and what it is.
fn kept_now(at: &Path, path: &Path) -> Option<(FileId, Kind)> {
    // (Its own entry, never a link's target: as everything else that acts
    // on a saved file looks at it, `is_saved`, `open_saved`.)
    let id = std::fs::symlink_metadata(path).ok().filter(|m| m.file_type().is_file()).map(|m| FileId::of_meta(&m))?;
    kept(at, path, id).map(|kind| (id, kind))
}

/// The poster files the recordings kept are shown with (by name).
fn posters_kept(at: &Path) -> std::collections::HashSet<String> {
    read_kept(at).unwrap_or_default().into_iter().filter(|k| k.kind.plays()).map(|k| poster_name(k.id)).collect()
}

fn poster_name(id: FileId) -> String {
    format!("{id}.png")
}

/// When a recording's picture is taken: a tenth of the way, a second at
/// most (past a countdown's last frame); a second in if it's not known
/// how long it is.
fn poster_at(seconds: Option<f64>) -> f64 {
    seconds.map_or(1.0, |s| (s * 0.1).min(1.0))
}

/// A GIF's frame as it shows `at` seconds in (the last, if it's over
/// by then), read from `file`.
fn gif_frame(file: std::fs::File, at: f64) -> Option<image::RgbaImage> {
    use image::AnimationDecoder as _;
    let file = std::io::BufReader::new(file);
    let frames = image::codecs::gif::GifDecoder::new(file).ok()?.into_frames();
    let mut shown = None;
    let mut start = 0.0;
    for frame in frames {
        let Ok(frame) = frame else { break };
        if shown.is_some() && start > at {
            break;
        }
        let (n, d) = frame.delay().numer_denom_ms();
        start += f64::from(n) / f64::from(d.max(1)) / 1000.0;
        shown = Some(frame.into_buffer());
    }
    shown
}

/// A recording's picture, a moment in ([`poster_at`]), made small in
/// Screenshot's cache for its notification to show (the video is what's
/// dragged out): read from the open `file` (`path`'s, as it was opened),
/// a GIF's frame where it's one (`kind`). Those of recordings no longer
/// kept go as new ones are made, once they're old.
fn poster(file: &std::sync::Arc<std::fs::File>, path: &Path, kind: Kind, id: FileId, seconds: Option<f64>) -> Option<PathBuf> {
    let dir = olo_xdg::cache_dir().join("screenshot").join("posters");
    std::fs::create_dir_all(&dir).ok()?;
    sweep(&dir, &posters_kept(&record().0));
    let out = dir.join(poster_name(id));
    if out.is_file() {
        return Some(out);
    }
    let at = poster_at(seconds);
    let picture = if kind == Kind::Gif {
        gif_frame(std::fs::File::open(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?, at)?
    } else {
        let frame = olo_preview::video::picture_at(&input_of(file, path), at).ok()?;
        image::RgbaImage::from_raw(frame.size[0] as u32, frame.size[1] as u32, frame.rgba.clone())?
    };
    if picture.width() == 0 || picture.height() == 0 {
        return None;
    }
    let k = (POSTER as f32 / picture.width().max(picture.height()) as f32).min(1.0);
    let (w, h) = (((picture.width() as f32 * k).round() as u32).max(1), ((picture.height() as f32 * k).round() as u32).max(1));
    let small = if k < 1.0 { image::imageops::thumbnail(&picture, w, h) } else { picture };
    // (Whole or not at all: the notifications read it as it's told of.
    // Written into a file of its own, made exclusively, so another making
    // the same poster at once, a recorder and an old helper's note, never
    // shares it; put in place over nothing, and if another put it there
    // first, that one is the poster. Removed however it fails.)
    let (part, file) = poster_part(&dir, id)?;
    let part = crate::recording::Part::new(part);
    let mut w = std::io::BufWriter::new(file);
    small.write_to(&mut w, image::ImageFormat::Png).ok()?;
    let file = w.into_inner().ok()?;
    file.sync_all().ok()?;
    match rename_new(&part.0, &out) {
        Ok(()) => part.done(),
        // (Made by another meanwhile: that one.)
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    Some(out)
}

/// A file of its own in `dir` to write recording `id`'s poster into, made
/// exclusively (`.ID.PID-N.png.part`).
fn poster_part(dir: &Path, id: FileId) -> Option<(PathBuf, std::fs::File)> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    for _ in 0..100 {
        let part = dir.join(format!(".{id}.{}-{}.png.part", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => return Some((part, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Posters past `POSTERS_KEPT` go, but those of recordings still kept
/// (a pinned one's, a month on, as well).
fn sweep(dir: &Path, keep: &std::collections::HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        if keep.contains(&*e.file_name().to_string_lossy()) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > POSTERS_KEPT);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// What a notification of a saved file says and does.
struct Saved<'a> {
    path: &'a Path,
    kind: Kind,
    /// The file as it was saved, as kept: what its actions act on. None:
    /// one not known to be Screenshot's, which is shown but offers
    /// nothing to do to it.
    receipt: Option<FileId>,
    /// The picture it shows (a recording's poster).
    picture: Option<PathBuf>,
    /// "1280 × 800 · 4.2 MB".
    details: String,
    /// A recording's length, shown on its picture.
    length: Option<f64>,
    /// What went wrong on the way (an encoder that failed).
    problem: Option<String>,
}

impl Saved<'_> {
    /// The notification: its name, what it is, its picture (dragged out,
    /// the file), and its actions, each saying what it runs once this
    /// program has gone (`--saved ACTION PATH RECEIPT`, the same as now).
    fn note(&self) -> Note {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let body = match &self.problem {
            Some(problem) => format!("{}\n{problem}", self.details),
            None => self.details.clone(),
        };
        let mut note = Note::new(name).body(body);
        if let Some(picture) = &self.picture {
            note = note.image(picture);
        }
        if let Some(s) = self.length {
            note = note.length(s);
        }
        // (One not known to be Screenshot's: shown, never handed on, and
        // nothing offered to do to it.)
        let Some(receipt) = self.receipt else { return note.file(self.path).shown_only() };
        let p = self.path.to_string_lossy().into_owned();
        let args = |action: &str| ["--saved".to_owned(), action.to_owned(), p.clone(), receipt.to_string()];
        note = note.file_as(self.path, receipt).action("default", "Open").shows(args("open")).action("copy", "Copy").runs("copy", args("copy"));
        // (A video's, in Viewer's editor: a GIF opens there as a picture.)
        if self.kind == Kind::Recording {
            note = note.action("edit", "Edit").runs("edit", args("edit"));
        }
        note.action("show", "Show in Tracker").runs("show", args("show")).action("delete", "Delete").runs("delete", args("delete")).apart("delete")
    }
}

/// A notification of a saved file, ready to send: made where the file
/// was saved (what it says read from the file, its receipt kept), so
/// what waits on the clipboard only sends it.
pub struct Prepared {
    note: Note,
    path: PathBuf,
    receipt: Option<FileId>,
}

/// The notification of `path`, a picture or a recording (`made`) just
/// saved by Screenshot (`Identity::Written`: the file as written): kept
/// as one of Screenshot's own ([`remember`]), its actions carrying that
/// receipt. Not known as written (`Unknown`), or named by whoever asked
/// (`Named`, `--after`, from a recorder started before an upgrade, and
/// not a file Screenshot already kept as it is now), it's shown, never
/// handed on, with nothing to do to it. What it says is read from the
/// file opened once (never through a link, never waited on: a pipe given
/// its name holds nothing up) and checked as opened; what can't be read
/// isn't said (its name and nothing else, rather than no note). None
/// where it isn't to be told ([`shown_here`]).
pub fn prepare(path: &Path, identity: Identity, kind: Kind, made: Option<&crate::recording::Made>) -> Option<Prepared> {
    if !shown_here() {
        return None;
    }
    // (Whole from the root: what's dragged out, copied and deleted is a
    // file's address, and a name alone is nobody's.)
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let (receipt, kind) = match identity {
        Identity::Written(id) => (remember(&path, id, kind), kind),
        Identity::Unknown => (None, kind),
        Identity::Named => kept_now(&record().0, &path).map_or((None, kind), |(id, kind)| (Some(id), kind)),
    };
    let opened = match receipt {
        Some(id) => open_saved(&path, id).ok(),
        None => open_plain(&path),
    };
    // (A recording named by whoever asked: a GIF by what it holds.)
    let kind = if identity == Identity::Named && receipt.is_none() && kind == Kind::Recording && opened.as_ref().is_some_and(gif_by_its_bytes) { Kind::Gif } else { kind };
    let size = opened.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
    let opened = opened.map(std::sync::Arc::new);
    let (seconds, dims) = match (kind, made, &opened) {
        (Kind::Recording | Kind::Gif, Some(m), _) => (Some(m.seconds), (m.width, m.height)),
        (_, _, None) => (None, (0, 0)),
        (Kind::Picture | Kind::Gif, _, Some(f)) => (None, dimensions_of(f).unwrap_or((0, 0))),
        (Kind::Recording, None, Some(f)) => match olo_preview::video::probe(&input_of(f, &path)) {
            Ok(info) => (info.duration, (info.width, info.height)),
            Err(_) => (None, (0, 0)),
        },
    };
    let picture = match (kind, &opened) {
        (Kind::Picture, Some(_)) => Some(path.clone()),
        (Kind::Recording | Kind::Gif, Some(f)) => FileId::of_file(f).and_then(|id| poster(f, &path, kind, id, seconds)),
        (_, None) => None,
    };
    let problem = made.and_then(|m| {
        let mut said = Vec::new();
        if m.encoder_note.is_some() {
            said.push(format!("An encoder failed; finished on {}.", m.encoder));
        }
        if m.stopped_early.is_some() {
            said.push("Stopped early: the encoder failed. What was recorded is kept.".to_owned());
        }
        (!said.is_empty()).then(|| said.join("\n"))
    });
    let length = seconds.filter(|_| kind.plays());
    let note = Saved { path: &path, kind, receipt, picture, details: details(dims, size), length, problem }.note();
    Some(Prepared { note, path, receipt })
}

impl Prepared {
    /// Sends it. Its actions come back here while this program runs,
    /// each done as it would be once it has gone: by Screenshot started
    /// again for it. Keep what's given for as long as the program runs,
    /// and say [`sent`] before it ends.
    pub fn send(self) -> Notifier {
        let Prepared { note, path, receipt } = self;
        // While this runs, a click on it is this program's to answer: done
        // as the centre would once it has gone (Copy then takes the
        // clipboard from this program, which ends).
        let notes = Notifier::with_events("Screenshot", ENTRY, move |e| {
            if let (NoteEvent::Action { key, .. }, Some(receipt)) = (e, receipt) {
                again(if key == "default" { "open" } else { &key }, &path, receipt);
            }
        });
        notes.show(None, note);
        notes
    }
}

/// [`prepare`] and [`Prepared::send`], at once.
pub fn tell(path: &Path, identity: Identity, kind: Kind, made: Option<&crate::recording::Made>) -> Option<Notifier> {
    prepare(path, identity, kind, made).map(Prepared::send)
}

/// Screenshot started again to do `action` to the file saved as
/// `receipt` at `path` (`--saved`), on its own: what the notifications
/// centre starts once this has gone. The program installed now, as the
/// centre finds it (this one's own, replaced by an upgrade while it held
/// the clipboard, is "… (deleted)").
fn again(action: &str, path: &Path, receipt: FileId) {
    let Some(exe) = olo_xdg::find_program(ENTRY) else {
        eprintln!("olo-screenshot: couldn't {action} {}: Screenshot isn't installed", path.display());
        return;
    };
    let argv = [exe.into_os_string(), "--saved".into(), action.into(), path.as_os_str().to_owned(), receipt.to_string().into()];
    if let Err(e) = olo_xdg::open::spawn(&argv) {
        eprintln!("olo-screenshot: couldn't {action} {}: {e}", path.display());
    }
}

/// Waits (a little, at most) for `notes`' notification to have been
/// sent: a program about to end. Said if it wasn't.
pub fn sent(notes: Option<&Notifier>) {
    if let Some(n) = notes
        && !n.sync(SENT_WITHIN)
    {
        eprintln!("olo-screenshot: its notification wasn't seen sent (no notifications running?)");
    }
}

/// `--saved ACTION PATH RECEIPT`, just those: what a saved file's
/// notification does (`open`, `copy`, `show`, `delete`, a video's
/// `edit`) to the file saved as RECEIPT at PATH, here or after this
/// program has gone (the notifications centre starts it so). Copy holds
/// the clipboard, with no window, until something else is copied.
pub fn act_on(args: &[String]) -> i32 {
    let [flag, action, path, receipt] = args else {
        eprintln!("olo-screenshot: --saved takes what to do (open, copy, show, delete, edit), the file's path and its receipt, and nothing else");
        return 2;
    };
    let Some(receipt) = (flag == "--saved").then(|| FileId::parse(receipt)).flatten() else {
        eprintln!("olo-screenshot: --saved {action} {path}: that's not a receipt Screenshot gives ({receipt})");
        return 2;
    };
    match act(action, Path::new(path), receipt) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            1
        }
    }
}

/// Does `action` to the file Screenshot saved as `receipt` at `path`,
/// while it's still that file: anything else is left alone, whoever
/// asks.
pub fn act(action: &str, path: &Path, receipt: FileId) -> Result<(), String> {
    act_in(&record().0, action, path, receipt, reserve_slot)
}

fn act_in(at: &Path, action: &str, path: &Path, receipt: FileId, slot: impl FnOnce(&Path) -> std::io::Result<(PathBuf, PathBuf)>) -> Result<(), String> {
    let refused = || format!("{} is not a picture or a recording Screenshot saved (or not that one any more): left alone", path.display());
    if !path.is_absolute() {
        return Err(refused());
    }
    let Some(kind) = kept(at, path, receipt) else { return Err(refused()) };
    // (Opened by its name below: whoever opens it, now; it's that file
    // as they're asked.)
    // (Its own entry, never through a link: Open, Show and Edit look at it as
    // Copy and Delete do.)
    let still = || if is_saved(path, receipt) { Ok(()) } else { Err(refused()) };
    match action {
        "open" => still().and_then(|()| olo_xdg::open::open(&olo_xdg::open::Target::Path(path.to_owned())).map_err(|e| format!("couldn't open {}: {e}", path.display()))),
        "show" => still().and_then(|()| olo_kit::tracker::open(path).map(|_| ()).map_err(|e| format!("couldn't show {} in Tracker: {e}", path.display()))),
        "edit" if kind == Kind::Recording => still().and_then(|()| {
            let viewer = olo_xdg::find_program("olo-viewer").ok_or("no Viewer to edit it in")?;
            olo_xdg::open::spawn(&[viewer.into_os_string(), "--edit".into(), path.as_os_str().to_owned()]).map_err(|e| format!("couldn't open the editor: {e}"))
        }),
        // (In the Trash, which Tracker restores from.)
        "delete" => trash_saved(path, receipt, slot).map(|_| ()),
        // The file on the clipboard (as a file manager copies it), a
        // picture as the picture itself first (what a chat or an editor
        // pastes), read from the very file saved; held, with no window,
        // until something else is copied.
        "copy" => {
            let clip = copied(path, receipt, kind)?;
            held_clipboard::hold(clip).map_err(|e| format!("couldn't copy {}: {e}", path.display()))
        }
        other => Err(format!("--saved {other}: not something this file's notification does")),
    }
}

/// What Copy puts on the clipboard for the file saved as `receipt`: a
/// picture's bytes (read from that file, opened once; never past
/// `COPY_MOST`, however it grows as it's read: then the file alone),
/// then the file (a recording: the file alone).
fn copied(path: &Path, receipt: FileId, kind: Kind) -> Result<held_clipboard::Clip, String> {
    let file = open_saved(path, receipt)?;
    let mut clip = held_clipboard::Clip::new();
    if kind == Kind::Picture
        && let Some(mime) = picture_type(path)
        && file.metadata().is_ok_and(|m| m.len() <= COPY_MOST)
    {
        let mut data = Vec::new();
        (&file).take(COPY_MOST + 1).read_to_end(&mut data).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        if data.len() as u64 <= COPY_MOST {
            clip.push((mime.to_owned(), data.into()));
        }
    }
    clip.extend(held_clipboard::files(&[path.to_owned()], false));
    Ok(clip)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of a test's own, gone when it's done.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(what: &str) -> Scratch {
            let dir = PathBuf::from(format!("/var/tmp/olo-shot-saved-{what}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn record(&self) -> (PathBuf, PathBuf) {
            (self.0.join("state").join("saved.jsonl"), self.0.join("state").join("saved.lock"))
        }

        /// A trash of its own (what `trash_slot` gives): a file for it and its record.
        fn slot(&self) -> impl FnOnce(&Path) -> std::io::Result<(PathBuf, PathBuf)> + use<> {
            let dir = self.0.join("trash");
            move |p: &Path| {
                std::fs::create_dir_all(&dir)?;
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                let info = dir.join(format!("{name}.trashinfo"));
                std::fs::write(&info, format!("Path={}\n", p.display()))?;
                Ok((dir.join(name), info))
            }
        }

        /// What's in the folder (but the test's own state and trash).
        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "state" && n != "trash").collect();
            v.sort();
            v
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn says_how_big() {
        assert_eq!(bytes(900), "900 bytes");
        assert_eq!(bytes(52 * 1024), "52 KB");
        assert_eq!(bytes(4_400_000), "4.2 MB");
        assert_eq!(details((2560, 1600), 412 * 1024), "2560 × 1600 · 412 KB");
        assert_eq!(details((1280, 800), 4_400_000), "1280 × 800 · 4.2 MB");
        assert_eq!(details((0, 0), 10), "10 bytes");
        assert_eq!((poster_at(Some(4.0)), poster_at(Some(60.0)), poster_at(None)), (0.4, 1.0, 1.0));
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

    /// A screenshot's notification: its name, what it is, its picture dragged out as the file, Open on a
    /// click, and each action saying what it runs once this program has gone, with the receipt of the very
    /// file saved; Delete set apart. A recording's shows its poster and its length, drags out its video, and
    /// has Edit (a GIF's doesn't: Viewer opens a GIF as a picture). One not known to be Screenshot's offers
    /// nothing to do to it.
    #[test]
    fn what_a_saved_files_notification_says_and_does() {
        let path = Path::new("/home/a/Pictures/screenshot4.png");
        let receipt = Some(FileId(66305, 42, 7));
        let shot = Saved { path, kind: Kind::Picture, receipt, picture: Some(path.to_owned()), details: "2560 × 1600 · 412 KB".into(), length: None, problem: None };
        let note = shot.note();
        assert_eq!((note.summary.as_str(), note.body.as_str()), ("screenshot4.png", "2560 × 1600 · 412 KB"));
        assert_eq!((note.look.image.as_deref(), note.look.file.as_deref(), note.look.apart.as_deref()), (Some(path.to_str().unwrap()), Some(path.to_str().unwrap()), Some("delete")));
        assert_eq!(note.actions.iter().map(|(k, l)| format!("{k}:{l}")).collect::<Vec<_>>(), ["default:Open", "copy:Copy", "show:Show in Tracker", "delete:Delete"]);
        assert_eq!(note.shows, ["--saved", "open", "/home/a/Pictures/screenshot4.png", "66305-42-7"]);
        let runs: Vec<String> = note.runs.iter().map(|(k, a)| format!("{k}={}", a.join(" "))).collect();
        assert_eq!(
            runs,
            [
                "copy=--saved copy /home/a/Pictures/screenshot4.png 66305-42-7",
                "show=--saved show /home/a/Pictures/screenshot4.png 66305-42-7",
                "delete=--saved delete /home/a/Pictures/screenshot4.png 66305-42-7"
            ]
        );
        assert_eq!(note.look.length, None);
        // (Its file as written goes with it: the service takes the file only while it's that one.)
        assert_eq!((note.look.file_id, note.look.shown_only), (receipt, false));
        // Not known to be Screenshot's: shown, never handed on, and nothing offered.
        let unknown = Saved { receipt: None, ..shot }.note();
        assert!(unknown.actions.is_empty() && unknown.runs.is_empty() && unknown.shows.is_empty() && unknown.look.apart.is_none());
        assert_eq!((unknown.look.file.as_deref(), unknown.look.shown_only, unknown.look.file_id), (Some(path.to_str().unwrap()), true, None));
        assert_eq!(unknown.look.dragged(), None, "a file Screenshot can't vouch for is offered as a drag");
        let video = Path::new("/home/a/Videos/Screen Recording 1.mp4");
        let poster = PathBuf::from("/home/a/.cache/olo/screenshot/posters/1-2-3.png");
        let rec = Saved { path: video, kind: Kind::Recording, receipt, picture: Some(poster.clone()), details: "1280 × 800".into(), length: Some(12.2), problem: Some("Stopped early.".into()) };
        let note = rec.note();
        assert_eq!((note.look.image, note.look.file.as_deref()), (Some(poster.to_string_lossy().into_owned()), Some(video.to_str().unwrap())));
        assert_eq!((note.body.as_str(), note.look.length), ("1280 × 800\nStopped early.", Some(12)));
        assert!(note.actions.iter().any(|(k, _)| k == "edit") && note.runs.iter().any(|(k, a)| k == "edit" && a[1] == "edit"));
        // A GIF is one by what made it, whatever its name: no Edit; a video named ".gif" keeps it.
        let made = |path, kind| Saved { path, kind, receipt, picture: Some(poster.clone()), details: String::new(), length: None, problem: None }.note();
        let gif = made(Path::new("/home/a/Videos/Screen Recording 2.mp4"), Kind::Gif);
        assert!(!gif.actions.iter().any(|(k, _)| k == "edit") && !gif.runs.iter().any(|(k, _)| k == "edit"));
        let named_gif = made(Path::new("/home/a/Videos/demo.gif"), Kind::Recording);
        assert!(named_gif.actions.iter().any(|(k, _)| k == "edit"));
    }

    /// Screenshot acts only on the very file a save made, while it's still that file: screenshot A deleted,
    /// and B saved under its name since, A's actions (its old row in the list) leave B alone, B's own work;
    /// a file it never saved, or a receipt for another, never was. Its record keeps the newest.
    #[test]
    fn only_the_file_a_save_made_is_acted_on() {
        let s = Scratch::new("acted");
        let rec = s.record();
        let shot = s.0.join("screenshot1.png");
        std::fs::write(&shot, b"one").unwrap();
        let a = FileId::of(&shot).unwrap();
        let other = s.0.join("notes.txt");
        std::fs::write(&other, b"mine").unwrap();
        assert_eq!(kept(&rec.0, &shot, a), None, "nothing kept yet");
        assert!(act_in(&rec.0, "delete", &shot, a, s.slot()).is_err() && shot.exists());
        remember_in(&rec, &shot, a, Kind::Picture).unwrap();
        assert_eq!(kept(&rec.0, &shot, a), Some(Kind::Picture));
        let theirs = FileId::of(&other).unwrap();
        assert!(act_in(&rec.0, "delete", &other, theirs, s.slot()).is_err() && other.exists(), "never Screenshot's");
        assert!(act_in(&rec.0, "delete", &shot, theirs, s.slot()).is_err() && shot.exists(), "a receipt for another file");
        // A deleted (to the Trash), and B saved under its name.
        std::fs::rename(&shot, s.0.join("trashed.png")).unwrap();
        std::fs::write(&shot, b"two").unwrap();
        let b = FileId::of(&shot).unwrap();
        assert_ne!(a, b);
        remember_in(&rec, &shot, b, Kind::Picture).unwrap();
        // A's Delete (and Copy, Open, Show) leave B alone…
        for action in ["delete", "copy", "open", "show"] {
            let r = act_in(&rec.0, action, &shot, a, s.slot());
            assert!(r.is_err(), "{action} with A's receipt: {r:?}");
        }
        assert_eq!(std::fs::read(&shot).unwrap(), b"two");
        assert_eq!(s.names(), ["notes.txt", "screenshot1.png", "trashed.png"], "nothing left aside");
        // …and B's own Delete takes B to the Trash.
        act_in(&rec.0, "delete", &shot, b, s.slot()).unwrap();
        assert!(!shot.exists() && std::fs::read(s.0.join("trash").join("screenshot1.png")).unwrap() == b"two");
        assert_eq!(s.names(), ["notes.txt", "trashed.png"]);
        // Not a whole path, or not something it does.
        assert!(act_in(&rec.0, "delete", Path::new("screenshot1.png"), b, s.slot()).is_err());
        assert!(act_in(&rec.0, "edit", &shot, b, s.slot()).is_err(), "a picture isn't edited as a video");
    }

    /// Whatever has the file's name as Delete comes to it: moved aside, found not to be the file saved, put
    /// back where it was (or, if another has come there since, left where it is and said so); never in the
    /// Trash. A copy reads the very file saved, never what has its name since, and never waits on a pipe.
    #[test]
    fn delete_and_copy_take_only_the_file_saved() {
        let s = Scratch::new("aside");
        let shot = s.0.join("screenshot2.png");
        std::fs::write(&shot, b"saved").unwrap();
        let saved = FileId::of(&shot).unwrap();
        std::fs::rename(&shot, s.0.join("elsewhere.png")).unwrap();
        std::fs::write(&shot, b"another").unwrap();
        // (Never moved at all, not even for a moment: its inode's change time is as it was.)
        let ctime = |p: &Path| {
            std::os::unix::fs::MetadataExt::ctime_nsec(&std::fs::symlink_metadata(p).unwrap()) + 1_000_000_000 * std::os::unix::fs::MetadataExt::ctime(&std::fs::symlink_metadata(p).unwrap())
        };
        let before = ctime(&shot);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let e = trash_saved(&shot, saved, s.slot()).unwrap_err();
        assert!(e.contains("not the file Screenshot saved"), "{e}");
        assert_eq!(std::fs::read(&shot).unwrap(), b"another");
        assert_eq!(ctime(&shot), before, "the newer file was moved, if only for a moment");
        assert_eq!(s.names(), ["elsewhere.png", "screenshot2.png"]);
        assert!(!s.0.join("trash").exists());
        // Copy: the file saved, by its descriptor; another of its name, a link or a pipe, refused.
        let back = s.0.join("screenshot3.png");
        std::fs::write(&back, b"\x89PNG three").unwrap();
        let three = FileId::of(&back).unwrap();
        let clip = copied(&back, three, Kind::Picture).unwrap();
        assert_eq!(clip.first().map(|(m, d)| (m.as_str(), d.to_vec())), Some(("image/png", b"\x89PNG three".to_vec())));
        assert!(clip.iter().any(|(m, _)| m == "text/uri-list"));
        let video = copied(&back, three, Kind::Recording).unwrap();
        assert!(!video.iter().any(|(m, _)| m == "image/png"), "a recording goes as its file alone");
        assert!(copied(&back, saved, Kind::Picture).is_err());
        let link = s.0.join("link.png");
        std::os::unix::fs::symlink(&back, &link).unwrap();
        assert!(copied(&link, three, Kind::Picture).is_err(), "a link isn't the file saved");
        let fifo = s.0.join("pipe.png");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(copied(&fifo, three, Kind::Picture).is_err(), "a pipe isn't waited on");
    }

    /// Delete never takes a place in the Trash that's taken: the file saved goes back where it was, and what
    /// was in that place is untouched (two Deletes of files of one name, each with the same place). A real
    /// reservation never hands out a place twice, and passes over one taken.
    #[test]
    fn delete_never_takes_a_taken_place_in_the_trash() {
        let s = Scratch::new("taken");
        let shot = s.0.join("screenshot5.png");
        std::fs::write(&shot, b"mine").unwrap();
        let mine = FileId::of(&shot).unwrap();
        let trash = s.0.join("trash");
        std::fs::create_dir_all(&trash).unwrap();
        let taken = trash.join("screenshot5.png");
        std::fs::write(&taken, b"theirs").unwrap();
        let record = trash.join("screenshot5.png.trashinfo.mine");
        let (t, r) = (taken.clone(), record.clone());
        let e = trash_saved(&shot, mine, move |_: &Path| {
            std::fs::write(&r, b"[Trash Info]\n")?;
            Ok((t, r))
        })
        .unwrap_err();
        assert!(e.contains("couldn't move"), "{e}");
        assert_eq!(std::fs::read(&taken).unwrap(), b"theirs", "what was in the place was touched");
        assert_eq!(std::fs::read(&shot).unwrap(), b"mine", "the file saved isn't back where it was");
        assert!(!record.exists(), "its own record is let go of");
        assert_eq!(s.names(), ["screenshot5.png"]);
        // A reservation: each its own place and record; a place taken without a record passed over too.
        let at = s.0.join("Trash");
        let first = reserve_slot_in(&at, &shot).unwrap();
        let second = reserve_slot_in(&at, &shot).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.0.file_name().unwrap(), "screenshot5.png");
        assert_eq!(second.0.file_name().unwrap(), "screenshot5 2.png");
        std::fs::write(at.join("files").join("screenshot5 3.png"), b"x").unwrap();
        let third = reserve_slot_in(&at, &shot).unwrap();
        assert_eq!(third.0.file_name().unwrap(), "screenshot5 4.png");
        let said = std::fs::read_to_string(&third.1).unwrap();
        assert!(said.starts_with("[Trash Info]\nPath=/") && said.contains("screenshot5.png\nDeletionDate="), "{said}");
        // A name near the longest a file system takes: its place's name cut short on a character's edge, so
        // that its record's name fits too; the whole path in the record.
        let long = s.0.join(format!("{}.png", "é".repeat(125)));
        std::fs::write(&long, b"x").unwrap();
        let (place, record) = reserve_slot_in(&at, &long).unwrap();
        let record_name = record.file_name().unwrap().len();
        assert!(record_name <= NAME_MAX, "{record_name} bytes");
        assert!(place.file_name().unwrap().to_str().unwrap().ends_with("é.png"));
        assert!(std::fs::read_to_string(&record).unwrap().contains(&trash_escape(&long)));
        assert_eq!(clamp_on_char("aé", 2), "a");
        // A copy across file systems whose original can't be removed: the copy goes too, nothing left in the
        // Trash without a record.
        let ro = s.0.join("ro");
        std::fs::create_dir_all(&ro).unwrap();
        let held = ro.join("held.png");
        std::fs::write(&held, b"held").unwrap();
        let mut perms = std::fs::metadata(&ro).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&ro, perms).unwrap();
        let copy = at.join("files").join("held.png");
        let r = copy_then_remove(&held, &copy);
        let mut back = std::fs::metadata(&ro).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut back, 0o700);
        std::fs::set_permissions(&ro, back).unwrap();
        // (As root, the folder's permissions stop nothing: then there's nothing to see here.)
        if r.is_err() {
            assert!(!copy.exists(), "a copy was left in the Trash with no record");
            assert!(held.exists());
        }
    }

    /// What's kept is read past a line that isn't one (cut short by a crash, or not text), and the next save
    /// puts it right; past KEPT, those no longer there go first.
    #[test]
    fn what_screenshot_keeps_survives_a_damaged_line() {
        let s = Scratch::new("kept");
        let rec = s.record();
        let one = s.0.join("one.png");
        std::fs::write(&one, b"1").unwrap();
        let id1 = FileId::of(&one).unwrap();
        remember_in(&rec, &one, id1, Kind::Picture).unwrap();
        let mut text = std::fs::read(&rec.0).unwrap();
        text.extend_from_slice(b"\xff\xfe not text\n{\"path\":\"/cut");
        std::fs::write(&rec.0, &text).unwrap();
        let two = s.0.join("two.mp4");
        std::fs::write(&two, b"2").unwrap();
        let id2 = FileId::of(&two).unwrap();
        remember_in(&rec, &two, id2, Kind::Recording).unwrap();
        assert_eq!((kept(&rec.0, &one, id1), kept(&rec.0, &two, id2)), (Some(Kind::Picture), Some(Kind::Recording)));
        let written = std::fs::read_to_string(&rec.0).unwrap();
        assert_eq!(written.lines().count(), 2, "{written}");
        assert!(written.ends_with('\n'));
        assert_eq!(kept_now(&rec.0, &two), Some((id2, Kind::Recording)));
        assert!(posters_kept(&rec.0).contains(&poster_name(id2)));
        // Past KEPT, those gone go (those kept since stay until it's past KEPT again); what's still
        // there stays, however old.
        std::fs::remove_file(&two).unwrap();
        for i in 0..KEPT {
            let p = s.0.join(format!("gone{i}.png"));
            remember_in(&rec, &p, FileId(1, i as u64, 0), Kind::Picture).unwrap();
        }
        let last = s.0.join("last.png");
        std::fs::write(&last, b"3").unwrap();
        let id3 = FileId::of(&last).unwrap();
        remember_in(&rec, &last, id3, Kind::Picture).unwrap();
        let left: Vec<PathBuf> = read_kept(&rec.0).unwrap().into_iter().map(|k| k.path).collect();
        assert_eq!(left, [one.clone(), s.0.join(format!("gone{}.png", KEPT - 2)), s.0.join(format!("gone{}.png", KEPT - 1)), last.clone()]);
    }

    /// A GIF's picture is the frame that shows the moment asked for (a recording's first frames may be
    /// blank), the last if it's over by then.
    #[test]
    fn a_gifs_picture_is_taken_a_moment_in() {
        let s = Scratch::new("gif");
        let path = s.0.join("rec.gif");
        {
            let mut enc = image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
            for (shade, ms) in [(0u8, 300), (128, 300), (255, 300)] {
                let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([shade, shade, shade, 255]));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let at = |s: f64| gif_frame(std::fs::File::open(&path).unwrap(), s).map(|f| f.get_pixel(0, 0).0[0]);
        assert_eq!((at(0.0), at(0.4), at(0.7), at(5.0)), (Some(0), Some(128), Some(255), Some(255)));
        // A GIF by what it holds (for one its recorder didn't say), whatever its name; a video isn't one.
        let opened = std::fs::File::open(&path).unwrap();
        assert!(gif_by_its_bytes(&opened));
        let video = s.0.join("rec.mp4");
        std::fs::write(&video, b"\0\0\0\x20ftypisom").unwrap();
        assert!(!gif_by_its_bytes(&std::fs::File::open(&video).unwrap()));
    }

    /// What a notification says of a file is read from the file opened once, never through a link and never
    /// waited on: a pipe given a screenshot's name is no file (a note with nothing said, not a hang), and the
    /// picture's size is read from the file opened, whatever has its name since.
    #[test]
    fn what_a_note_says_is_read_from_the_file_as_opened() {
        let s = Scratch::new("opened");
        let fifo = s.0.join("screenshot9.png");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(open_plain(&fifo).is_none() && open_saved(&fifo, FileId(1, 2, 3)).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "a pipe was waited on");
        let shot = s.0.join("screenshot10.png");
        image::RgbaImage::from_pixel(8, 5, image::Rgba([1, 2, 3, 255])).save(&shot).unwrap();
        let opened = std::sync::Arc::new(open_saved(&shot, FileId::of(&shot).unwrap()).unwrap());
        std::fs::rename(&shot, s.0.join("moved.png")).unwrap();
        image::RgbaImage::from_pixel(30, 20, image::Rgba([1, 2, 3, 255])).save(&shot).unwrap();
        assert_eq!(dimensions_of(&opened), Some((8, 5)), "the size of what has its name since");
        let link = s.0.join("link.png");
        std::os::unix::fs::symlink(&shot, &link).unwrap();
        assert!(open_plain(&link).is_none(), "a link isn't a file of its own");
    }
}
