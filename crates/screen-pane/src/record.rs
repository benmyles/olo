//! The frames a screen drew, kept beside its history of actions
//! (`history`): `olo-screen record NAME` runs with each screen (in its
//! process group, so it ends with it), looks at the screen when it has
//! drawn something new (its app_server says so, `EventKind::Painted`:
//! at most eight times a second for a few seconds after each action and
//! once a second otherwise), and keeps only what changed: the 64-pixel
//! tiles that differ from the frame before, zstd-compressed, with a
//! whole frame where the last one is 30 s old (a keyframe, to start
//! playing from). A screen that draws nothing is not looked at, and
//! nothing is written for it: it used to be asked for its whole picture
//! every second whatever it did, and written whole every 30 s (eleven
//! idle screens: eleven pictures a second read back from the GPU the
//! person's desktop draws on, and compared). Frames go in segments of
//! five minutes, `frames-START.olof` in the history's folder, the oldest
//! dropped past the limits in Preferences ▸ AI (`history::limits`).
//!
//! A segment is `MAGIC`, then records: a little-endian u32 length and
//! that many bytes: when (u64 ms), the frame's width and height (u32),
//! whether it is a keyframe (u8), its tile count (u32), and each tile's
//! x, y, width, height (u16) and compressed length (u32) before its bytes.

use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub use olo_screen_codec::olof::{Entry, Frame, Player, Segment, Writer, frame_at, index_dir, last_size, prune};

use crate::{history, screens};

/// How often the screen is looked at at the most, after an action and
/// otherwise (and how often it is looked at whatever it drew, where its
/// app_server doesn't say when it draws: one from before it did).
const BUSY: Duration = Duration::from_millis(125);
const IDLE: Duration = Duration::from_secs(1);
/// With nothing drawn, how often the recorder still wakes: to see that
/// its screen is there and whether an upgrade asks it to become its new
/// version (the session stops, 90 s on, a program still the old one).
const WAKE: Duration = Duration::from_secs(30);
/// How long after an action it is looked at often.
const BUSY_FOR: Duration = Duration::from_secs(5);

/// Screen `name`'s frames, segment by segment, oldest first (read from
/// the records' heads: their pixels are left on disk).
pub fn index(name: &str) -> Vec<Segment> {
    history::dir_checked(name).map(|d| index_dir(&d)).unwrap_or_default()
}

/// Read an RGBA capture written by `app_server` safely: opens with
/// `O_NONBLOCK | O_NOFOLLOW`, validates `S_ISREG`, reads the 8-byte `(w, h)`
/// header first, checks `checked_local_frame_bytes(w, h)` and exact file
/// length `8 + pixel_bytes` *before* allocating the pixel buffer.
pub(crate) fn read_rgba_frame_bounded(path: &std::path::Path) -> Result<Frame, String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path).map_err(|e| format!("open capture {}: {e}", path.display()))?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err(format!("capture {} is not a regular file", path.display()));
    }
    let file_len = meta.len();
    if file_len < 8 {
        return Err(format!("capture {} truncated ({file_len} bytes)", path.display()));
    }
    let mut head = [0u8; 8];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    let width = u32::from_le_bytes(head[0..4].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    let pixel_bytes = olo_screen_codec::checked_local_frame_bytes(width, height)?;
    let expected = 8u64.checked_add(pixel_bytes as u64).ok_or_else(|| "capture byte length overflow".to_owned())?;
    if file_len != expected {
        return Err(format!("capture {} size {file_len} != expected {expected} ({width}×{height})", path.display()));
    }
    let mut rgba = vec![0u8; pixel_bytes];
    f.read_exact(&mut rgba).map_err(|e| e.to_string())?;
    Ok(Frame { width, height, rgba })
}

/// Frames written to `dir` as the recorder writes them: each (picture,
/// when, whether a keyframe though 30 s haven't passed).
#[cfg(test)]
pub(crate) fn write_for_test(dir: &std::path::Path, frames: Vec<(Frame, u64, bool)>) {
    Writer::write_frames(dir, frames).unwrap();
}

/// Whether a byte waits in `signals` (`restore::upgrade_signal`): the
/// program's file was replaced. Never waits.
fn asked(signals: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd as _;
    let mut fd = libc::pollfd { fd: signals.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one descriptor of ours, no waiting.
    unsafe { libc::poll(&mut fd, 1, 0) > 0 && fd.revents & libc::POLLIN != 0 }
}

/// What the screen's app_server has said since it was last looked at.
#[derive(Default)]
struct Said {
    /// It drew something new.
    painted: bool,
    /// It is gone (its events ended).
    gone: bool,
}

/// Has `said` told whenever screen `screen` draws something new, by a
/// thread that reads its app_server's events; false where that
/// app_server can't be asked or doesn't say (the screen is then looked
/// at by the clock, as it always was).
fn follow_paints(screen: &screens::Screen, said: &Arc<(Mutex<Said>, Condvar)>) -> bool {
    use olo_protocol::{Client, Event, EventKind, Reply, Request};
    let Ok(mut client) = Client::connect_to(&screen.olo_socket) else { return false };
    if !matches!(client.request(&Request::SubscribeTo { events: vec![EventKind::Painted] }), Ok(Reply::Ok)) {
        return false;
    }
    let said = said.clone();
    let tell = move |f: &dyn Fn(&mut Said)| {
        f(&mut said.0.lock().unwrap_or_else(|e| e.into_inner()));
        said.1.notify_all();
    };
    std::thread::Builder::new()
        .name("screen-events".into())
        .spawn(move || {
            while let Ok(reply) = client.read() {
                if let Reply::Event { event: Event::Painted { .. } } = reply {
                    tell(&|s| s.painted = true);
                }
            }
            tell(&|s| s.gone = true);
        })
        .is_ok()
}

/// Record screen `name` until it is gone. One recorder a screen: a
/// second (two agents acting at once on a screen that had none) finds
/// the first's lock and leaves.
pub fn run(name: &str) -> Result<(), String> {
    let screen = screens::get(name)?;
    // (Held until this process ends or becomes its new version, which
    // takes it again itself.)
    let Some(_lock) = screens::take_recorder_lock(name) else { return Ok(()) };
    let picture = screens::recorder_frame(name);
    let _ = std::fs::create_dir_all(picture.parent().unwrap());
    // (The frame read through it is gone with the recorder: in the
    // runtime folder, it's memory, 5 MB a screen. Ended with its screen's
    // process group, `screens::stop` removes it.)
    let _frame = screens::Removed(picture.clone());
    let actions = history::dir(name).join("actions.jsonl");
    // The limits in Preferences, and what is past them already gone.
    let (max_bytes, max_age) = history::limits(Some(&screen));
    history::prune(max_age);
    let mut w = Writer::new(history::dir(name), max_bytes);
    let mut failures = 0;
    // When an upgrade replaces this program's file (olo-session's SIGURG),
    // it becomes the new version between two frames: what it wrote is
    // whole on disk, and the new version starts a segment of its own.
    // (It ignored that, so the session stopped it 90 s after every
    // upgrade as a program still on the old version, and nothing starts
    // a running screen's recorder again: five screens' frames weren't
    // kept from an upgrade on. Ben, 2026-11-18.)
    let mut upgrade = olo_kit::restore::upgrade_signal();
    // What it has drawn since the last look (the first look is due at
    // once: what the screen shows as recording begins).
    let said = Arc::new((Mutex::new(Said { painted: true, gone: false }), Condvar::new()));
    let tells = follow_paints(&screen, &said);
    let mut looked: Option<Instant> = None;
    loop {
        if !screen.alive() {
            break;
        }
        if upgrade.as_ref().is_some_and(asked) {
            let e = olo_kit::restore::exec_new_version();
            tracing::warn!("recording {name}: couldn't become the new version ({e}); going on as this one");
            // (Asked once: not tried again at every frame.)
            upgrade = None;
        }
        if tells {
            // Until it draws something new; and now and then all the
            // same, for the two things looked at above.
            let mut s = said.0.lock().unwrap_or_else(|e| e.into_inner());
            if !s.painted && !s.gone {
                s = said.1.wait_timeout(s, WAKE).unwrap_or_else(|e| e.into_inner()).0;
            }
            if s.gone {
                break;
            }
            if !std::mem::take(&mut s.painted) {
                continue;
            }
        }
        // Often for a while after an action, seldom otherwise: never
        // sooner after the last look than that. (What it draws
        // meanwhile is told again, and looked at then: the last thing
        // drawn is always kept.)
        let acted = std::fs::metadata(&actions).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        let period = if acted.is_some_and(|a| a < BUSY_FOR) { BUSY } else { IDLE };
        if let Some(last) = looked {
            std::thread::sleep(period.saturating_sub(last.elapsed()));
        }
        looked = Some(Instant::now());
        match screen.picture(&picture, None, true, olo_protocol::ImageFormat::Rgba).and_then(|()| read_rgba_frame_bounded(&picture)) {
            Ok(frame) => {
                failures = 0;
                if let Err(e) = w.add(frame, history::now_ms()) {
                    tracing::warn!("recording {name}: {e}");
                }
            }
            Err(e) => {
                failures += 1;
                tracing::warn!("recording {name}: {e}");
                if failures > 20 {
                    return Err(e);
                }
                // (What it drew is still to be kept: looked at again,
                // no sooner than the next look is due.)
                said.0.lock().unwrap_or_else(|e| e.into_inner()).painted = true;
            }
        }
    }
    let _ = std::fs::remove_file(&picture);
    Ok(())
}
