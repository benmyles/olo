//! Screen recording: a display or a window into an MP4, for people and
//! agents alike, all in process (`olo_video`: H.264 from OpenH264 into an
//! MP4 of Olo's writing). Frames come from app_server's capture
//! (`olo_capture`), and only when something changed: the recorder asks
//! for the next at most `--fps` times a second and waits on the
//! connection (nothing polled: woken by a frame, a signal through its
//! pipe, or `--seconds` running out), and each picture is held until the
//! next comes, so a still screen costs nothing and still keeps its time. They
//! are encoded on a worker as they come; one that comes while the worker
//! is still busy with the last waits in its place, replaced by a newer.
//!
//! ```text
//! olo-screenshot --record [--window ID] [--display NAME] [--region [X,Y,W,H]]
//!                [--seconds N] [--fps N] [--no-pointer]
//!                [--encoder auto|cpu|nvidia] [--codec h264|hevc|av1]
//!                [--sound computer|microphone|both|none]
//!                [--as-gif] [--show-clicks] [--show-keys] [--countdown N] [--background]
//!                [--pointer-size K] [--smooth-pointer] [--zoom-follow] [--camera [ID]]
//!                [--to PATH]
//! olo-screenshot --zoom-to X,Y,W,H|out [--display NAME]
//! olo-screenshot --caption TEXT [--seconds N]   ("": taken down)
//! olo-screenshot --mark TITLE    this moment marked: a chapter
//! olo-screenshot --gif VIDEO.mp4 [--to PATH] [--fps N] [--width N]
//! olo-screenshot --trim VIDEO.mp4 --start S --end E [--to PATH]
//! olo-screenshot --stop          the recording on this display, finished
//! olo-screenshot --pause         nothing recorded till --resume
//! olo-screenshot --resume
//! olo-screenshot --recording     whether one is going, as JSON
//! ```
//!
//! `--codec` codes it as H.264 (plays everywhere: the default), HEVC
//! (about half the size) or AV1 (for players that want it: no smaller on
//! screens), the last two on NVIDIA's encoder only (`--encoder auto` on the processor makes H.264 then;
//! `codec` in what's printed says which), as asked, else as Preferences ▸
//! GPUs has it.
//!
//! `--sound` records what the computer plays, the microphone, both or
//! neither (`sound.rs`: AAC beside the video), as asked, else as the
//! settings say (the computer's, by default). With no sound server or no
//! AAC encoder, the video is made without, and says so (`sound` null,
//! `sound_note` why).
//!
//! `--as-gif` (or the settings' `gif`: Screenshot's Record… ▸ Save as GIF)
//! makes the recording a GIF once it's done (no sound), the MP4 gone;
//! `--gif` makes any of Olo's recordings one (`olo_video::gif`: 10 frames
//! a second, at most 960 wide, one palette, each frame what changed).
//!
//! `--countdown N` shows N, N-1, … 1 in the middle of the screen a second
//! each before the recording begins (app_server's `Request::Countdown`),
//! while the encoder opens; ⌘⇧6 and Screenshot's Record… count down the
//! settings' `countdown` (3), agents' recordings none.
//!
//! `--show-clicks` and `--show-keys` (or the settings', Record…'s ticks)
//! have app_server show clicks (a ring) and shortcuts (a key cap) while
//! it records (`Request::SetShowing`), and stop after.
//!
//! `--camera [ID]` (or the settings', Record… ▸ Show Camera) shows the
//! camera in a bubble low in the screen's right corner while it records
//! (`bubble.rs`, its own process: on the screen, so a recording of the
//! screen or a region has it; a window's doesn't), started before the
//! countdown and ended after; `camera` in what's printed names it,
//! `camera_note` says why there's none. ID: a camera's node ("video0"),
//! else the first colour camera.
//!
//! `--mark TITLE` marks this moment of the recording going on this
//! display (SIGRTMIN+1; the title left in the runtime directory beside
//! what's going), timed as the video is (pauses left out): a chapter of
//! the video (`chpl`) and in what it prints (`marks`).
//!
//! `--caption TEXT` puts a caption in what's recorded (never on the
//! screen: app_server draws it into captures only), low in the middle on
//! a dark panel, wrapped to three lines, fading in and out, for
//! `--seconds N` or till the next (an empty TEXT takes it down): an
//! agent's narration of its demo.
//!
//! `--zoom-follow` (or the settings', Record… ▸ Zoom on Clicks) has a
//! display's recording zoom in twice around each click, follow the
//! pointer, and zoom out three seconds after the last click or key;
//! `--zoom-to X,Y,W,H` zooms it to a part of the display and holds it
//! there (`out`: back), for an agent showing a detail (app_server's
//! `zoom`: the GPU draws the capture magnified, the screen as it is).
//!
//! `--pointer-size K` draws the pointer K times its size in the
//! recording (1 to 4; the built-in cursors drawn at that size, not
//! enlarged), and `--smooth-pointer` has it glide after the real one (a
//! hand's jitter and an agent's jumps smoothed: app_server's
//! `showing::captured_pointer`), as the settings say otherwise (Record… ▸
//! Larger Pointer, 1.5×, and Smooth Pointer). Only in what's captured:
//! the screen's pointer is as it was.
//!
//! `--background` (or the settings', Record… ▸ Window on a Background)
//! shows a window's recording on the wallpaper, blurred, the window's
//! corners rounded and a shadow under it (`backdrop.rs`): the video that
//! much bigger. Only for a window; the screen and a region are as they are.
//!
//! `--region X,Y,W,H` records that part of the display (points from its
//! top left, as windows' frames are given); `--region` alone has it
//! dragged out first, on the screen frozen as for ⌘⇧4 (Escape: nothing
//! recorded). Paused, nothing is recorded and the time paused is left
//! out: the video goes straight from the moment before to the moment
//! after (`--seconds` counts what's recorded).
//!
//! `--record` records until `--stop` (or SIGINT/SIGTERM), N seconds, or
//! the window closing, then prints what it made as JSON (`path`,
//! `seconds`, `frames`, `width`, `height`, `encoder`). It encodes on the
//! most powerful encoder there is unless told (`--encoder`): NVIDIA's
//! GPU (NVENC: 4K in 13 ms a frame) where it opens, else the processor
//! (OpenH264: 4K in 111 ms); by default into the Videos
//! folder as "Screen Recording 2026-11-04 at 18.30.05.mp4", written as
//! `.part` until whole. One at a time on each display: what's going is
//! in `$XDG_RUNTIME_DIR/olo-recording-<display>.json` (so an Agent
//! Screen's and the person's don't meet).
//!
//! The whole display recorded on the GPU that draws it (VA-API's AV1 on
//! the Intel GPU app_server draws with, as on battery or `--encoder
//! vaapi`), its frames go from app_server to the encoder in the GPU's
//! memory, never through the processor: captured into a few dmabufs in
//! turn (`Capture::lend_frames`), each made NV12 by VA-API's video
//! processor and encoded (`frames_on_gpu` in what it prints). Otherwise,
//! or `OLO_RECORDING_COPY=1`, frames are copied out and converted on the
//! processor.

use std::{
    os::fd::{AsRawFd as _, FromRawFd as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

use olo_capture::{Capture, Source};
use serde::Serialize;

/// Asked to stop (SIGINT, SIGTERM: `--stop`).
static STOP: AtomicBool = AtomicBool::new(false);

/// Paused (SIGUSR1: `--pause`) till SIGUSR2 (`--resume`).
static PAUSED: AtomicBool = AtomicBool::new(false);

/// The write end of a pipe the recorder's wait watches: each handler
/// writes to it, so the wait wakes for a signal whichever thread took it
/// (the region picker's leave some behind), and one that came just
/// before the wait isn't missed (nothing is woken on a timer to look).
static WAKE: AtomicI32 = AtomicI32::new(-1);

fn wake() {
    let fd = WAKE.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe; a full pipe already wakes.
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
    wake();
}

extern "C" fn on_pause(_: libc::c_int) {
    PAUSED.store(true, Ordering::SeqCst);
    wake();
}

extern "C" fn on_resume(_: libc::c_int) {
    PAUSED.store(false, Ordering::SeqCst);
    wake();
}

/// Marks asked for (SIGRTMIN+1: `--mark`), their titles waiting in
/// `marks_path`.
static MARK: AtomicBool = AtomicBool::new(false);

extern "C" fn on_mark(_: libc::c_int) {
    MARK.store(true, Ordering::SeqCst);
    wake();
}

/// The signal `--mark` sends.
fn mark_signal() -> libc::c_int {
    libc::SIGRTMIN() + 1
}

/// Where `--mark` leaves titles for the recorder on this display.
fn marks_path() -> PathBuf {
    going_path().with_extension("marks")
}

/// The titles left since the last look, taken (the file moved aside
/// first, so a mark made meanwhile starts a new one).
fn take_marks() -> Vec<String> {
    let taking = marks_path().with_extension("marks.taking");
    if std::fs::rename(marks_path(), &taking).is_err() {
        return Vec::new();
    }
    let titles = std::fs::read_to_string(&taking).unwrap_or_default().lines().map(str::to_owned).collect();
    let _ = std::fs::remove_file(&taking);
    titles
}

/// `--mark TITLE`: this moment of the recording on this display marked,
/// a chapter of the video (`chpl`) and in what it prints (`marks`).
pub fn mark(args: &[String]) -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    let title = option(args, "--mark").unwrap_or("").replace(['\n', '\r'], " ").trim().to_owned();
    let line = format!("{title}\n");
    let written = std::fs::OpenOptions::new().create(true).append(true).open(marks_path()).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    if let Err(e) = written {
        eprintln!("olo-screenshot: {e}");
        return 1;
    }
    // SAFETY: a signal to the recorder's process.
    unsafe { libc::kill(g.pid as i32, mark_signal()) };
    println!("{}", serde_json::json!({ "path": g.path, "mark": title }));
    0
}

use olo_protocol::recording::{Going, going};

fn going_path() -> PathBuf {
    olo_protocol::recording::path_for(&olo_protocol::recording::this_display())
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// Where a recording goes unless told: the Videos folder, named for when.
fn default_path() -> PathBuf {
    let dir = olo_xdg::user_dir("VIDEOS").unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos"));
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r fills `tm` from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    dir.join(format!("Screen Recording {:04}-{:02}-{:02} at {:02}.{:02}.{:02}.mp4", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec))
}

/// A picture waiting for the encoder: its pixels (or its dmabuf, lent
/// by the capture: then no pixels) and when it came.
struct Waiting {
    data: Vec<u8>,
    lent: Option<olo_capture::Lent>,
    stride: usize,
    width: u32,
    height: u32,
    at: Duration,
}

/// What a recording made.
#[derive(Debug, Clone, Serialize)]
pub struct Made {
    pub path: PathBuf,
    pub seconds: f64,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    /// What encoded it ("NVIDIA's GPU (NVENC)").
    pub encoder: String,
    /// What it's coded as: "H.264", "HEVC" or "AV1".
    pub codec: &'static str,
    /// Frames that went to the encoder in the GPU's memory, never copied
    /// through the processor.
    pub frames_on_gpu: usize,
    /// Why it stopped before it was told to (the encoder failed twice,
    /// the disk filled): what was recorded till then is kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_early: Option<String>,
    /// An encoder that failed and the one that went on in its place
    /// (`encoder` is the last): "NVIDIA's GPU (NVENC) failed (…); went on
    /// with the processor (OpenH264)".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_note: Option<String>,
    /// The sound in it ("computer", "microphone", "both"), if any.
    pub sound: Option<String>,
    /// Why there's none, when some was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_note: Option<String>,
    /// Made a GIF: its frames and size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif: Option<GifMade>,
    /// The camera shown in its bubble, if one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Why there's none, when one was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_note: Option<String>,
    /// Its marks (`--mark`): when (seconds into it) and their titles,
    /// its chapters.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

/// A moment of a recording marked.
#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    pub at: f64,
    pub title: String,
}

/// A GIF made of a recording.
#[derive(Debug, Clone, Serialize)]
pub struct GifMade {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// `--trim VIDEO --start S --end E`: a recording cut to that stretch,
/// copied (nothing encoded again: `olo_video::trim`), beside it as
/// "NAME (trimmed).mp4" unless `--to`; what was made printed.
pub fn trim(args: &[String]) -> i32 {
    let run = || -> Result<serde_json::Value, String> {
        let src = PathBuf::from(option(args, "--trim").ok_or("--trim takes the video to trim")?);
        let secs = |flag: &str| option(args, flag).map(str::parse::<f64>).transpose().map_err(|_| format!("{flag} takes seconds"));
        let start = secs("--start")?.unwrap_or(0.0);
        let end = secs("--end")?.ok_or("--end takes where it ends (seconds)")?;
        let dst = option(args, "--to").map_or_else(
            || {
                let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                src.with_file_name(format!("{stem} (trimmed).mp4"))
            },
            PathBuf::from,
        );
        let part = dst.with_extension("mp4.part");
        let t = olo_video::trim::trim(&src, &part, start, end).inspect_err(|_| {
            let _ = std::fs::remove_file(&part);
        })?;
        std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        Ok(serde_json::json!({ "path": dst, "seconds": t.seconds, "frames": t.frames, "sound": t.sound }))
    };
    match run() {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: the trim failed: {e}");
            1
        }
    }
}

/// `A-B` (seconds) stretches, comma separated ("0-12.5,20-31").
fn stretches(v: &str) -> Option<Vec<(f64, f64)>> {
    v.split(',').filter(|p| !p.trim().is_empty()).map(|p| p.split_once('-').and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))).collect()
}

/// What's kept of a `seconds`-long video: `keep`'s stretches (all of it
/// if none), less `cut`'s.
fn kept_less(keep: Vec<(f64, f64)>, cut: &[(f64, f64)], seconds: f64) -> Vec<(f64, f64)> {
    let mut out = if keep.is_empty() { vec![(0.0, seconds)] } else { keep };
    for &(ca, cb) in cut {
        out = out.into_iter().flat_map(|(a, b)| [(a, b.min(ca)), (a.max(cb), b)]).filter(|(a, b)| b > a).collect();
    }
    out
}

/// `--export VIDEO [--to PATH] [--keep A-B,…] [--cut A-B,…] [--crop
/// X,Y,W,H] [--mute] [--format mp4|gif] [--width N] [--fps N] [--quality
/// high|medium|low] [--encoder …] [--codec …] [--progress]`: a recording
/// edited and made again (`olo_video::export`, the Viewer's editor's
/// Export…): what's kept (stretches in seconds; `--cut` takes stretches
/// out), each picture cropped (its pixels) and made `--width` wide, at
/// most `--fps` pictures a second, as finely as `--quality`, its sound
/// left out with `--mute`. `--progress` prints `{"progress": 0.42}` lines
/// as it goes; SIGINT or SIGTERM stops it (nothing left). Prints what it
/// made.
pub fn export(args: &[String]) -> i32 {
    let run = || -> Result<serde_json::Value, String> {
        let src = PathBuf::from(option(args, "--export").ok_or("--export takes the video to export")?);
        let format = option(args, "--format").map_or(Ok(olo_video::export::Format::Mp4), |f| olo_video::export::Format::parse(f).ok_or("--format is mp4 or gif"))?;
        let number = |flag: &str| option(args, flag).map(str::parse::<u32>).transpose().map_err(|_| format!("{flag} takes a number"));
        let seconds = olo_video::read::Video::open(&src)?.seconds();
        let keep = option(args, "--keep").map(|v| stretches(v).ok_or("--keep takes A-B stretches in seconds")).transpose()?.unwrap_or_default();
        let cut = option(args, "--cut").map(|v| stretches(v).ok_or("--cut takes A-B stretches in seconds")).transpose()?.unwrap_or_default();
        let crop = option(args, "--crop").map(|v| {
            let n: Vec<u32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            <[u32; 4]>::try_from(n).map_err(|_| "--crop takes X,Y,W,H (the video's pixels)")
        });
        let edit = olo_video::export::Edit { keep: kept_less(keep, &cut, seconds), crop: crop.transpose()?, mute: args.iter().any(|a| a == "--mute"), marks: None };
        let opts = olo_video::export::Options {
            format,
            width: number("--width")?.unwrap_or(0),
            fps: number("--fps")?.unwrap_or(0),
            quality: option(args, "--quality").map_or(Ok(olo_video::Quality::High), |q| olo_video::Quality::parse(q).ok_or("--quality is high, medium or low"))?,
            encoder: option(args, "--encoder").map_or(Ok(olo_video::Choice::Auto), |e| olo_video::Choice::parse(e).ok_or("--encoder is auto, cpu, nvidia or vaapi"))?,
            codec: option(args, "--codec").map_or(Ok(olo_video::Codec::H264), |c| olo_video::Codec::parse(c).ok_or("--codec is h264, hevc or av1"))?,
        };
        let dst = option(args, "--to").map_or_else(
            || {
                let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                src.with_file_name(format!("{stem} (edited).{}", format.extension()))
            },
            PathBuf::from,
        );
        // SAFETY: handlers that only set a flag and write to a pipe.
        unsafe {
            libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        }
        let tell = args.iter().any(|a| a == "--progress");
        let last = std::cell::Cell::new(-1i32);
        let progress = |p: f32| {
            // (A line each percent.)
            let pc = (p * 100.0) as i32;
            if tell && pc != last.get() {
                last.set(pc);
                println!("{}", serde_json::json!({ "progress": (f64::from(p) * 100.0).round() / 100.0 }));
            }
        };
        let made = olo_video::export::export(&src, &dst, &edit, &opts, &STOP, &progress)?;
        Ok(serde_json::json!({
            "path": dst, "seconds": made.seconds, "frames": made.frames, "width": made.width, "height": made.height,
            "bytes": made.bytes, "sound": made.sound, "encoder": made.encoder, "kept": edit.keep,
        }))
    };
    match run() {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: the export failed: {e}");
            1
        }
    }
}

/// `--gif VIDEO`: a recording made a GIF; what was made printed.
pub fn gif(args: &[String]) -> i32 {
    let Some(src) = option(args, "--gif").map(PathBuf::from) else {
        eprintln!("olo-screenshot: --gif takes the video to make a GIF of");
        return 2;
    };
    let dst = option(args, "--to").map_or_else(|| src.with_extension("gif"), PathBuf::from);
    let mut opts = olo_video::gif::Options::default();
    if let Some(fps) = option(args, "--fps").and_then(|v| v.parse().ok()) {
        opts.fps = fps;
    }
    if let Some(w) = option(args, "--width").and_then(|v| v.parse().ok()) {
        opts.width = w;
    }
    match make_gif(&src, &dst, &opts) {
        Ok(g) => {
            println!("{}", serde_json::json!({ "path": dst, "frames": g.frames, "width": g.width, "height": g.height, "bytes": g.bytes }));
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: the GIF failed: {e}");
            1
        }
    }
}

/// `src` made a GIF at `dst` (written beside it, then put in place);
/// stopped by SIGINT/SIGTERM as a recording is.
fn make_gif(src: &Path, dst: &Path, opts: &olo_video::gif::Options) -> Result<GifMade, String> {
    let part = dst.with_extension("gif.part");
    let made = olo_video::gif::from_mp4(src, &part, opts, &STOP, &|_| {});
    let made = match made {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    std::fs::rename(&part, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(GifMade { frames: made.frames, width: made.width, height: made.height, bytes: made.bytes })
}

/// `--record`: record until told to stop; what it made printed. Exit 0
/// on a finished file (or a region picked: none, cancelled).
pub fn run(args: &[String]) -> i32 {
    match record(args) {
        Ok(Some(made)) => {
            println!("{}", serde_json::to_string_pretty(&made).unwrap_or_default());
            0
        }
        Ok(None) => 0,
        Err(e) => {
            eprintln!("olo-screenshot: the recording failed: {e}");
            1
        }
    }
}

/// `X,Y,W,H`.
fn rect(v: &str) -> Option<[f64; 4]> {
    let n: Vec<f64> = v.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let r: [f64; 4] = n.try_into().ok()?;
    (r[2] > 0.0 && r[3] > 0.0 && r[0] >= 0.0 && r[1] >= 0.0).then_some(r)
}

/// The scale of display `name` (pixels a point).
fn scale_of(name: &str) -> f64 {
    let reply = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&olo_protocol::Request::Outputs).ok());
    let Some(olo_protocol::Reply::Outputs { outputs }) = reply else { return 1.0 };
    outputs.iter().find(|o| o.name == name).map_or(1.0, |o| o.scale)
}

/// The scale of the display window `id` is on.
fn window_scale(id: u64) -> f64 {
    let reply = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&olo_protocol::Request::ListWindows).ok());
    let Some(olo_protocol::Reply::Windows { windows }) = reply else { return 1.0 };
    windows.iter().find(|w| w.id == id).and_then(|w| w.output.clone()).map_or(1.0, |o| scale_of(&o))
}

/// Points (`r`, from the display's top left) at `scale`: its pixels.
fn in_pixels(r: [f64; 4], scale: f64) -> [u32; 4] {
    let x = (r[0] * scale).round();
    let y = (r[1] * scale).round();
    [x as u32, y as u32, ((r[0] + r[2]) * scale).round().max(x + 1.0) as u32 - x as u32, ((r[1] + r[3]) * scale).round().max(y + 1.0) as u32 - y as u32]
}

/// What a frame `w` × `h` is cut to (`crop`, in its pixels): within it,
/// and as it is if nothing of it is left.
fn crop_within(crop: Option<[u32; 4]>, w: u32, h: u32) -> [u32; 4] {
    let Some([x, y, cw, ch]) = crop else { return [0, 0, w, h] };
    if x >= w || y >= h {
        return [0, 0, w, h];
    }
    [x, y, cw.min(w - x), ch.min(h - y)]
}

/// How much a recording that follows clicks zooms in.
const ZOOM: f64 = 2.0;

/// A recording, or None: a region was to be picked, and none was.
fn record(args: &[String]) -> Result<Option<Made>, String> {
    if let Some(g) = going() {
        return Err(format!("this display is being recorded already (into {}; olo-screenshot --stop)", g.path.display()));
    }
    let mut source = match option(args, "--window") {
        Some(id) => Source::Window { id: id.parse().map_err(|_| "--window takes a window's id")?, app_id: String::new(), title: String::new() },
        None => Source::Output(option(args, "--display").map(str::to_owned)),
    };
    // Part of a display: given (points), else dragged out now (pixels of
    // the display it was dragged on).
    let mut region: Option<[f64; 4]> = None;
    let mut crop: Option<[u32; 4]> = None;
    // A region dragged out: kept for Restart (on which display).
    let mut picked: Option<Option<String>> = None;
    if let Some(i) = args.iter().position(|a| a == "--region") {
        if matches!(source, Source::Window { .. }) {
            return Err("--region is part of a display, not of a window".into());
        }
        match args.get(i + 1).filter(|v| !v.starts_with("--")) {
            Some(v) => region = Some(rect(v).ok_or("--region takes X,Y,W,H (points from the display's top left), or nothing to drag one out")?),
            None => {
                let Some((output, r)) = crate::region::pick_region() else { return Ok(None) };
                picked = Some(output.clone());
                source = Source::Output(output);
                crop = Some(r);
            }
        }
    }
    let fps: u32 = option(args, "--fps").map_or(Ok(30), str::parse).map_err(|_| "--fps takes a number")?;
    let fps = fps.clamp(1, 60);
    let all = olo_protocol::Client::connect().ok().and_then(|mut c| match c.request(&olo_protocol::Request::GetSettings).ok()? {
        olo_protocol::Reply::Settings { settings } => Some(settings),
        _ => None,
    });
    let settings = all.as_ref().map(|s| s.screen_recording.clone());
    // As asked, else as Preferences ▸ Power ▸ Graphics has it, else the most
    // powerful there is.
    let chosen = option(args, "--encoder").map(str::to_owned).or_else(|| settings.as_ref().map(|s| s.encoder.clone()));
    // A window on a background: what it's made of.
    let backdrop = match &source {
        Source::Window { id, .. } if args.iter().any(|a| a == "--background") || settings.as_ref().is_some_and(|s| s.background) => Some(crate::backdrop::Look {
            wallpaper: all.as_ref().and_then(|s| s.wallpaper.clone()),
            dark: all.as_ref().is_some_and(|s| s.appearance.effective_theme == olo_ui::palette::ThemeMode::Dark),
            colour: all.as_ref().and_then(|s| s.workspace_colors.first().copied()).unwrap_or([51, 102, 152]),
            scale: window_scale(*id),
        }),
        _ => None,
    };
    let as_gif = args.iter().any(|a| a == "--as-gif") || settings.as_ref().is_some_and(|s| s.gif);
    let show_clicks = args.iter().any(|a| a == "--show-clicks") || settings.as_ref().is_some_and(|s| s.show_clicks);
    let show_keys = args.iter().any(|a| a == "--show-keys") || settings.as_ref().is_some_and(|s| s.show_keys);
    let pointer_size: f64 = match option(args, "--pointer-size") {
        Some(v) => v.parse().ok().filter(|k: &f64| (1.0..=4.0).contains(k)).ok_or("--pointer-size takes 1 to 4 (times the pointer's size)")?,
        None => settings.as_ref().map_or(1.0, |s| s.pointer_size.clamp(1.0, 4.0)),
    };
    let smooth_pointer = args.iter().any(|a| a == "--smooth-pointer") || settings.as_ref().is_some_and(|s| s.smooth_pointer);
    // Zoom following clicks: the whole display only (a region's part and a
    // window are as they are).
    let zoom_follow = args.iter().any(|a| a == "--zoom-follow") || settings.as_ref().is_some_and(|s| s.zoom_follow);
    let countdown: u32 = option(args, "--countdown").map_or(Ok(0), str::parse).map_err(|_| "--countdown takes seconds")?;
    let sound = match option(args, "--sound").map(str::to_owned).or_else(|| settings.as_ref().map(|s| s.sound.clone())) {
        Some(s) => crate::sound::Which::parse(&s).ok_or("--sound is computer, microphone, both or none")?,
        None => crate::sound::Which::Computer,
    };
    // (A GIF has no sound: none recorded.)
    let sound = if as_gif { crate::sound::Which::Nothing } else { sound };
    let choice = match chosen.as_deref() {
        Some(e) => olo_video::Choice::parse(e).ok_or("--encoder is auto, cpu, nvidia or vaapi")?,
        None => olo_video::Choice::Auto,
    };
    // H.264, HEVC or AV1: as asked, else as Preferences ▸ Power ▸ Graphics has it (on
    // the processor, H.264 whatever it says: the processor makes only that;
    // through VA-API, AV1, the only one Fedora's drivers make).
    let codec = match option(args, "--codec") {
        Some(c) => olo_video::Codec::parse(c).ok_or("--codec is h264, hevc or av1")?,
        None if choice == olo_video::Choice::Vaapi => olo_video::Codec::Av1,
        None => match settings.as_ref().and_then(|s| olo_video::Codec::parse(&s.codec)) {
            Some(_) if choice == olo_video::Choice::Cpu => olo_video::Codec::H264,
            Some(c) => c,
            None => olo_video::Codec::H264,
        },
    };
    let seconds: Option<f64> = option(args, "--seconds").map(str::parse).transpose().map_err(|_| "--seconds takes a number")?;
    let path = option(args, "--to").map_or_else(default_path, PathBuf::from);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let pointer = !args.iter().any(|a| a == "--no-pointer");
    let mut capture = Capture::new(&source, pointer)?;
    if let Some(r) = region {
        crop = Some(in_pixels(r, scale_of(capture.name())));
    }
    // The handlers' pipe (`WAKE`), then them: they set a flag and write.
    let mut ends = [-1; 2];
    // SAFETY: pipe2 fills `ends`; each end is owned below.
    if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
        return Err(format!("a pipe: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: both ends are new and ours.
    let (woken, waker) = unsafe { (std::os::fd::OwnedFd::from_raw_fd(ends[0]), std::os::fd::OwnedFd::from_raw_fd(ends[1])) };
    WAKE.store(waker.as_raw_fd(), Ordering::SeqCst);
    // SAFETY: handlers that only set a flag and write(2).
    unsafe {
        libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGUSR1, on_pause as *const () as libc::sighandler_t);
        libc::signal(libc::SIGUSR2, on_resume as *const () as libc::sighandler_t);
        libc::signal(mark_signal(), on_mark as *const () as libc::sighandler_t);
    }
    let what = match &source {
        Source::Window { id, .. } => format!("window {id}"),
        Source::Output(_) if crop.is_some() => "region".into(),
        Source::Output(_) | Source::Display(_) => "display".into(),
    };
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let g = Going {
        pid: std::process::id(),
        path: path.clone(),
        started: now_secs,
        what,
        paused: false,
        recording: false,
        recorded_ms: 0,
        as_of: 0,
        args: again(args, picked, crop, scale_of(capture.name())),
    };
    let _ = std::fs::write(going_path(), serde_json::to_vec(&g).unwrap_or_default());
    // The status says it goes until what it makes is whole (a GIF made
    // from it too, or it deleted when discarded), however this ends.
    let _status = Status;
    // Clicks and keys shown while it records, the pointer as asked, and
    // as before after.
    let zoom = if zoom_follow && matches!(source, Source::Output(_)) && crop.is_none() { ZOOM } else { 0.0 };
    let showing = show_clicks || show_keys || pointer_size != 1.0 || smooth_pointer || zoom > 1.0;
    let set_showing = |clicks: bool, keys: bool, pointer_size: f64, smooth_pointer: bool, zoom: f64| {
        if let Ok(mut c) = olo_protocol::Client::connect() {
            let _ = c.request(&olo_protocol::Request::SetShowing { clicks, keys, pointer_size, smooth_pointer, zoom });
        }
    };
    if showing {
        set_showing(show_clicks, show_keys, pointer_size, smooth_pointer, zoom);
    }
    // The camera in its bubble, on the screen from before the countdown
    // till the end (a window's recording is only the window: not there).
    let camera_wanted: Option<String> = match args.iter().position(|a| a == "--camera") {
        Some(i) => Some(args.get(i + 1).filter(|v| !v.starts_with("--")).cloned().unwrap_or_else(|| "auto".into())),
        None => settings.as_ref().map(|s| s.camera.clone()).filter(|c| !c.is_empty()),
    };
    // A window on a background has the camera drawn on its background's
    // corner by the recorder itself (`inset.rs`); a window alone, none.
    let mut inset: Option<crate::inset::Inset> = None;
    let (bubble, camera, camera_note) = match camera_wanted {
        None => (None, None, None),
        Some(id) if matches!(source, Source::Window { .. }) && backdrop.is_some() => {
            let (w, h) = capture.size().unwrap_or((1280, 800));
            let m = crate::backdrop::margin(w, h);
            let (vw, vh) = olo_video::even(w + 2 * m, h + 2 * m);
            match crate::inset::Inset::start(&id, vw, vh) {
                Ok(i) => {
                    let name = i.name.clone();
                    inset = Some(i);
                    (None, Some(name), None)
                }
                Err(e) => {
                    eprintln!("olo-screenshot: recording without the camera: {e}");
                    (None, None, Some(e))
                }
            }
        }
        Some(_) if matches!(source, Source::Window { .. }) => {
            (None, None, Some("a window's recording is only the window: the camera shows in recordings of the screen or a region, or of a window on a background".to_owned()))
        }
        Some(id) => match CameraBubble::start(&id) {
            Ok((b, name)) => (Some(b), Some(name), None),
            Err(e) => {
                eprintln!("olo-screenshot: recording without the camera: {e}");
                (None, None, Some(e))
            }
        },
    };
    let result = record_into(&mut capture, &path, fps, seconds, (choice, codec), crop, backdrop, inset, sound, countdown.min(10), &g, woken.as_raw_fd());
    if showing {
        set_showing(false, false, 1.0, false, 0.0);
    }
    drop(bubble);
    WAKE.store(-1, Ordering::SeqCst);
    drop(waker);
    let mut made = result?;
    (made.camera, made.camera_note) = (camera, camera_note);
    // Discarded (`--discard`: the controls' Discard or Restart): nothing kept.
    let discard = olo_protocol::recording::discard_path_for(&olo_protocol::recording::this_display());
    if discard.exists() {
        let _ = std::fs::remove_file(&made.path);
        let _ = std::fs::remove_file(&discard);
        return Ok(None);
    }
    if as_gif {
        // Made a GIF beside it, the video gone (a stop now stops this).
        STOP.store(false, Ordering::SeqCst);
        let gif_path = made.path.with_extension("gif");
        let g = make_gif(&made.path, &gif_path, &olo_video::gif::Options::default())?;
        let _ = std::fs::remove_file(&made.path);
        made.path = gif_path;
        (made.width, made.height) = (g.width, g.height);
        made.gif = Some(g);
    }
    Ok(Some(made))
}

/// The recording's status file, removed when this goes: `--stop` and the
/// controls know the recording finished by its going (inotify).
struct Status;

impl Drop for Status {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(going_path());
    }
}

/// Waits (half a minute at most) for the recording `g` to finish: its
/// status gone (what it made is whole) or its process ended, whichever
/// comes first; a recorder started by the desktop's shortcut lives on a
/// while after, to say where its file went.
fn wait_finished(g: &Going) {
    let (tx, rx) = std::sync::mpsc::channel();
    let watched = tx.clone();
    let _watch = olo_ui::watch::watch([going_path()], move |_| {
        let _ = watched.send(());
    });
    let pid = g.pid;
    std::thread::spawn(move || {
        // SAFETY: pidfd_open and poll on the recorder's process.
        unsafe {
            let pidfd = libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) as i32;
            if pidfd >= 0 {
                let mut p = libc::pollfd { fd: pidfd, events: libc::POLLIN, revents: 0 };
                libc::poll(&mut p, 1, 30_000);
                libc::close(pidfd);
            }
        }
        let _ = tx.send(());
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let ended = || going().is_none_or(|now| now.pid != g.pid) || !Path::new(&format!("/proc/{}", g.pid)).exists();
    while !ended() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || rx.recv_timeout(left).is_err() {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_into(
    capture: &mut Capture,
    path: &Path,
    fps: u32,
    seconds: Option<f64>,
    (choice, codec): (olo_video::Choice, olo_video::Codec),
    crop: Option<[u32; 4]>,
    backdrop: Option<crate::backdrop::Look>,
    inset: Option<crate::inset::Inset>,
    which: crate::sound::Which,
    countdown: u32,
    going: &Going,
    woken: i32,
) -> Result<Made, String> {
    // The encoder opened first, on its worker (NVENC's CUDA context takes
    // over two seconds to make, and belongs to the thread that made it):
    // till it's ready, frames would only replace each other. The video's
    // size as the capture says it'll be (a part of it, for a region).
    let size = capture.size().map(|(w, h)| {
        let [_, _, cw, ch] = crop_within(crop, w, h);
        olo_video::even(cw, ch)
    });
    // (The sound's description, once it's started, for the file.)
    let sound_in: Arc<Mutex<Option<SoundIn>>> = Arc::default();
    // Marks so far (`--mark`): milliseconds into the video, and titles.
    let marks: Arc<Mutex<Vec<(u64, String)>>> = Arc::default();
    let _ = std::fs::remove_file(marks_path());
    let slot: Arc<(Mutex<Option<Waiting>>, std::sync::Condvar)> = Arc::default();
    let ended = Arc::new(AtomicBool::new(false));
    // Where the video ends (milliseconds of it), for its last frame.
    let end = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let part = path.with_extension("mp4.part");
    // Frames lent to the encoder as they are, in the GPU's memory (see
    // the top): the whole display, as it is, on the encoder's GPU. Till
    // the encoder can't take one: then copied.
    // (A test's failure, `OLO_RECORDING_FAIL_AT`, is a copied frame's.)
    let whole = crop.is_none() && backdrop.is_none() && inset.is_none() && ["OLO_RECORDING_COPY", "OLO_RECORDING_FAIL_AT"].iter().all(|v| std::env::var_os(v).is_none());
    let copy_only = Arc::new(AtomicBool::new(false));
    // (The encoder's GPU, if it encodes pictures in its memory.)
    let (ready_tx, ready) = std::sync::mpsc::channel::<Result<Option<u64>, String>>();
    let worker = {
        let (slot, ended, end, part, sound_in, marks, copy_only) = (slot.clone(), ended.clone(), end.clone(), part.clone(), sound_in.clone(), marks.clone(), copy_only.clone());
        std::thread::Builder::new()
            .name("recording encoder".into())
            .spawn(move || encode(&slot, &ended, &end, &part, fps, (choice, codec), size, backdrop.as_ref(), inset, &ready_tx, &sound_in, &marks, &copy_only))
            .map_err(|e| e.to_string())?
    };
    // The countdown meanwhile, a number a second (a stop ends it).
    let say = |n: u32| {
        if let Ok(mut c) = olo_protocol::Client::connect() {
            let _ = c.request(&olo_protocol::Request::Countdown { n });
        }
    };
    for n in (1..=countdown).rev() {
        say(n);
        wait(-1, woken, Some(Duration::from_secs(1)));
        if STOP.load(Ordering::SeqCst) {
            break;
        }
    }
    if countdown > 0 {
        say(0);
    }
    match ready.recv() {
        Ok(Err(e)) => {
            let _ = worker.join();
            return Err(e);
        }
        Ok(Ok(Some(gpu))) if whole && capture.render_device() == Some(gpu) => capture.lend_frames(Some(olo_video::vaapi::imports)),
        _ => {}
    }
    let start = Instant::now();
    // Said where Deskbar and agents look: recording from now.
    let going = &Going { recording: true, recorded_ms: 0, as_of: now_ms(), ..going.clone() };
    let _ = std::fs::write(going_path(), serde_json::to_vec(going).unwrap_or_default());
    // The sound from now, if any asked for and there's a way.
    let (sound, sound_note) = match which {
        crate::sound::Which::Nothing => (None, None),
        w => match crate::sound::Sound::start(w, start, &PAUSED) {
            Ok(s) => (Some(s), None),
            Err(e) => {
                eprintln!("olo-screenshot: recording without sound: {e}");
                (None, Some(e))
            }
        },
    };
    *sound_in.lock().unwrap_or_else(|e| e.into_inner()) = sound.as_ref().map(|s| (s.track().clone(), s.units()));
    let fd = capture.fd()?;
    let every = Duration::from_secs_f64(1.0 / f64::from(fps));
    let mut asked_at: Option<Instant> = None;
    // Time paused, left out of the video; and since when, if it is now.
    let mut paused_for = Duration::ZERO;
    let mut paused_at: Option<Instant> = None;
    let recorded = |paused_for: Duration, paused_at: Option<Instant>| start.elapsed().saturating_sub(paused_for + paused_at.map_or(Duration::ZERO, |a| a.elapsed()));
    // A frame's pixels as they come (the whole display, when a part of
    // it is recorded), kept for the next.
    let mut whole: Vec<u8> = Vec::new();
    let result: Result<(), String> = (|| {
        loop {
            // Marks asked for, at this moment of the video.
            if MARK.swap(false, Ordering::SeqCst) {
                let at = recorded(paused_for, paused_at).as_millis() as u64;
                marks.lock().unwrap_or_else(|e| e.into_inner()).extend(take_marks().into_iter().map(|t| (at, t)));
            }
            if STOP.load(Ordering::SeqCst) || seconds.is_some_and(|s| recorded(paused_for, paused_at).as_secs_f64() >= s) {
                return Ok(());
            }
            if capture.stopped() {
                // (The window closed, or app_server is going away.)
                return Ok(());
            }
            // Paused or going on again: said where Deskbar and agents look.
            let paused = PAUSED.load(Ordering::SeqCst);
            if paused != paused_at.is_some() {
                match paused_at.take() {
                    Some(at) => {
                        paused_for += at.elapsed();
                        // The screen as it is now, at once.
                        asked_at = None;
                    }
                    None => paused_at = Some(Instant::now()),
                }
                let g = Going { paused, recorded_ms: recorded(paused_for, paused_at).as_millis() as u64, as_of: now_ms(), ..going.clone() };
                let _ = std::fs::write(going_path(), serde_json::to_vec(&g).unwrap_or_default());
            }
            if paused {
                // Nothing asked for (one asked for before is let go), and
                // nothing waited for but a signal.
                wait(fd.as_raw_fd(), woken, None);
                let _ = capture.poll()?;
                continue;
            }
            // Till `--seconds` is up.
            let left = seconds.map(|s| Duration::from_secs_f64((s - recorded(paused_for, paused_at).as_secs_f64()).max(0.0)));
            // Ask for the next once its time has come (a frame every
            // `every` at most), then wait for it on the connection.
            if !capture.waiting() {
                let due = asked_at.map_or(Duration::ZERO, |a| (a + every).saturating_duration_since(Instant::now()));
                if due > Duration::ZERO {
                    wait(fd.as_raw_fd(), woken, Some(left.map_or(due, |l| l.min(due))));
                    continue;
                }
                capture.request()?;
                asked_at = Some(Instant::now());
            }
            // (A still screen sends nothing: then only a signal, or the
            // time running out, wakes it.)
            wait(fd.as_raw_fd(), woken, left);
            match capture.poll()? {
                Some(Ok(())) if PAUSED.load(Ordering::SeqCst) => {}
                Some(Ok(())) => {
                    let Some((fw, fh)) = capture.frame_size() else { continue };
                    let at = recorded(paused_for, paused_at);
                    if copy_only.swap(false, Ordering::SeqCst) {
                        capture.lend_frames(None);
                    }
                    // (Lent as it is when it covers the video: else, a window
                    // that shrank, copied and shown on black.)
                    if let Some(lent) = size.filter(|&(vw, vh)| fw >= vw && fh >= vh).and_then(|_| capture.lend()) {
                        let (m, cv) = &*slot;
                        *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(Waiting { data: Vec::new(), lent: Some(lent), stride: 0, width: fw, height: fh, at });
                        cv.notify_one();
                        continue;
                    }
                    let [x, y, w, h] = crop_within(crop, fw, fh);
                    let stride = w as usize * 4;
                    let data = if [x, y, w, h] == [0, 0, fw, fh] {
                        let mut data = vec![0u8; stride * h as usize];
                        capture.copy_into(&mut data, stride)?;
                        data
                    } else {
                        // The part recorded, out of the whole.
                        let whole_stride = fw as usize * 4;
                        whole.resize(whole_stride * fh as usize, 0);
                        capture.copy_into(&mut whole, whole_stride)?;
                        let mut data = Vec::with_capacity(stride * h as usize);
                        for row in y as usize..(y + h) as usize {
                            let from = row * whole_stride + x as usize * 4;
                            data.extend_from_slice(&whole[from..from + stride]);
                        }
                        data
                    };
                    let (m, cv) = &*slot;
                    *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(Waiting { data, lent: None, stride, width: w, height: h, at });
                    cv.notify_one();
                }
                // (A size changed: the next request makes a new buffer.)
                Some(Err(_)) | None => {}
            }
        }
    })();
    let total = recorded(paused_for, paused_at);
    // (A mark asked for as it stopped: at its end.)
    marks.lock().unwrap_or_else(|e| e.into_inner()).extend(take_marks().into_iter().map(|t| (total.as_millis() as u64, t)));
    // The sound's last, encoded and handed over before the file is.
    let sounded = sound.as_ref().map(|_| which.name().to_owned());
    if let Some(s) = sound {
        s.finish();
    }
    end.store(total.as_millis() as u64, Ordering::SeqCst);
    ended.store(true, Ordering::SeqCst);
    slot.1.notify_one();
    let made = worker.join().map_err(|_| "the encoder failed".to_string())?;
    result?;
    let (frames, frames_on_gpu, (width, height), encoder, codec, stopped_early, encoder_note) = made?;
    // The file finished by the worker, then put where it goes.
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Made {
        path: path.to_path_buf(),
        seconds: (total.as_millis() as f64 / 1000.0),
        frames,
        width,
        height,
        encoder: encoder.to_owned(),
        codec: codec.name(),
        frames_on_gpu,
        stopped_early,
        encoder_note,
        sound: sounded,
        sound_note,
        gif: None,
        camera: None,
        camera_note: None,
        marks: marks.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(ms, title)| Mark { at: *ms as f64 / 1000.0, title: title.clone() }).collect(),
    })
}

/// Wait until `fd` has something to read, a signal came (`woken`, the
/// handlers' pipe, emptied), or `timeout` (None: no end).
fn wait(fd: i32, woken: i32, timeout: Option<Duration>) {
    let mut p = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: woken, events: libc::POLLIN, revents: 0 }];
    // (Rounded up: woken a moment early, it would only wait again.)
    let ms = timeout.map_or(-1, |t| t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32);
    // SAFETY: two pollfds, ours.
    unsafe { libc::poll(p.as_mut_ptr(), 2, ms) };
    if p[1].revents != 0 {
        let mut buf = [0u8; 64];
        // SAFETY: reading into `buf`; the pipe doesn't block.
        while unsafe { libc::read(woken, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }
}

/// The encoder's loop: each picture as it's handed over, held until the
/// next; at the end, the last held till then and the file finished.
/// Frames encoded, and the video's size.
/// (`end`: where the video ends, in milliseconds of it, once `ended`.)
#[allow(clippy::too_many_arguments)]
fn encode(
    slot: &(Mutex<Option<Waiting>>, std::sync::Condvar),
    ended: &AtomicBool,
    end: &std::sync::atomic::AtomicU64,
    part: &Path,
    fps: u32,
    (choice, codec): (olo_video::Choice, olo_video::Codec),
    size: Option<(u32, u32)>,
    look: Option<&crate::backdrop::Look>,
    mut inset: Option<crate::inset::Inset>,
    ready: &std::sync::mpsc::Sender<Result<Option<u64>, String>>,
    sound: &Mutex<Option<SoundIn>>,
    marks: &Mutex<Vec<(u64, String)>>,
    copy_only: &AtomicBool,
) -> Result<Encoded, String> {
    // The sound's access units so far, into the file beside the pictures
    // (its description given the file the first time).
    let mut described = false;
    let mut add_sound = |mp4: &mut olo_video::Mp4| -> Result<(), String> {
        let s = sound.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((track, units)) = s.as_ref() {
            if !std::mem::replace(&mut described, true) {
                mp4.with_sound(track, olo_system::recording::RATE, crate::sound::BITRATE);
            }
            let got = std::mem::take(&mut *units.lock().unwrap_or_else(|e| e.into_inner()));
            for au in got {
                mp4.add_sound(&au).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };
    let (m, cv) = slot;
    // Milliseconds: durations as they came.
    let mut video: Option<(olo_video::Encoder, olo_video::Mp4, Vec<u8>)> = None;
    // A window on a background: the video its size (made with the
    // encoder, before the recording starts).
    let mut backdrop: Option<crate::backdrop::Backdrop> = None;
    let open = |w: u32, h: u32, backdrop: &mut Option<crate::backdrop::Backdrop>| -> Result<(olo_video::Encoder, olo_video::Mp4, Vec<u8>), String> {
        let (vw, vh) = match look {
            Some(look) => {
                let b = crate::backdrop::Backdrop::new(w, h, look);
                let size = (b.width, b.height);
                *backdrop = Some(b);
                size
            }
            None => olo_video::even(w, h),
        };
        let enc = olo_video::Encoder::new(vw, vh, fps, choice, codec)?;
        let mut mp4 = olo_video::Mp4::create_for(part, vw, vh, 1000, enc.codec()).map_err(|e| e.to_string())?;
        // (Olo's: a library knows it whatever it's named.)
        mp4.mark_olo();
        Ok((enc, mp4, vec![0u8; vw as usize * 4 * vh as usize]))
    };
    // Opened now, at the size the capture says (before the recording
    // starts: the recorder waits for this); else at the first picture.
    if let Some((vw, vh)) = size {
        let opened = open(vw, vh, &mut backdrop);
        match opened {
            Ok(v) => video = Some(v),
            Err(e) => {
                let _ = ready.send(Err(e.clone()));
                return Err(e);
            }
        }
    }
    let _ = ready.send(Ok(video.as_ref().and_then(|(enc, _, _)| enc.prime_device())));
    // The last picture, till the next says how long it showed (and
    // whether another encoder's stream starts with it).
    let mut held: Option<(Vec<u8>, bool, Duration, bool)> = None;
    let (mut frames, mut on_gpu) = (0usize, 0usize);
    // Why it stopped before it was told to, if it did.
    let mut stopped: Option<String> = None;
    let fail_at: Option<(usize, usize)> = std::env::var("OLO_RECORDING_FAIL_AT").ok().and_then(|v| Some((v.trim_end_matches('!').parse().ok()?, v.len() - v.trim_end_matches('!').len())));
    // Encoders that took over from one that failed, as said.
    let mut notes: Vec<String> = Vec::new();
    loop {
        let next = {
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(w) = g.take() {
                    break Some(w);
                }
                if ended.load(Ordering::SeqCst) {
                    break None;
                }
                g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(mut w) = next else { break };
        if video.is_none() {
            video = Some(open(w.width, w.height, &mut backdrop)?);
        }
        let (enc, mp4, canvas) = video.as_mut().unwrap();
        // A picture lent, encoded as it is (its top left, the video's
        // size); one the encoder can't take dropped, the rest copied.
        // (Given back at once: the encoder has made its own of it.)
        let coded = w.lent.take().map(|lent| {
            let picture =
                olo_video::vaapi::Prime { key: lent.key, fd: lent.fd(), modifier: lent.modifier, offset: lent.offset, pitch: lent.pitch, size: lent.size, width: lent.width, height: lent.height };
            enc.encode_prime(&picture)
        });
        let coded = match coded {
            Some(Ok((bits, key))) => {
                on_gpu += 1;
                Ok(Coded { bits, key, fresh: false, note: None })
            }
            Some(Err(e)) => {
                eprintln!("olo-screenshot: the encoder took no frame from the GPU ({e}): copying them");
                copy_only.store(true, Ordering::SeqCst);
                continue;
            }
            None => encode_copied(enc, &w, &mut backdrop, &mut inset, canvas, fps, fail_at.filter(|(n, _)| *n == frames)),
        };
        let coded = match coded {
            Ok(c) => c,
            Err(e) => {
                stopped = Some(format!("the encoder failed: {e}"));
                break;
            }
        };
        notes.extend(coded.note);
        if let Some((b, k, at, fresh)) = held.take() {
            let ms = w.at.saturating_sub(at).as_millis().max(1) as u32;
            if let Err(e) = (if fresh { mp4.new_entry() } else { Ok(()) }).and_then(|()| mp4.add(&b, k, ms)) {
                stopped = Some(format!("the file took no more: {e}"));
                break;
            }
            frames += 1;
            add_sound(mp4)?;
        }
        held = Some((coded.bits, coded.key, w.at, coded.fresh));
    }
    if let Some(why) = &stopped {
        // (The recorder stops too: nothing more would be kept.)
        eprintln!("olo-screenshot: recording stopped early: {why}");
        on_stop(0);
    }
    let Some((enc, mut mp4, _)) = video else { return Err(stopped.unwrap_or_else(|| "nothing was shown to record".into())) };
    if let Some((b, k, at, fresh)) = held {
        // (Stopped early, the last picture held a moment, not to the end.)
        let till = if stopped.is_some() { at + Duration::from_millis(100) } else { Duration::from_millis(end.load(Ordering::SeqCst)) };
        let ms = till.saturating_sub(at).as_millis().max(1) as u32;
        if (if fresh { mp4.new_entry() } else { Ok(()) }).and_then(|()| mp4.add(&b, k, ms)).is_ok() {
            frames += 1;
        }
    }
    add_sound(&mut mp4)?;
    mp4.chapters(marks.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let made = mp4.size();
    mp4.finish().map_err(|e| e.to_string())?;
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    Ok((frames, on_gpu.min(frames), made, enc.name(), enc.codec(), stopped, note))
}

/// A picture encoded: its bits, whether a key frame, and, when an
/// encoder opened after the last one failed made it, that its stream
/// starts here (`Mp4::new_entry`) and, if it's another kind, what to say.
struct Coded {
    bits: Vec<u8>,
    key: bool,
    fresh: bool,
    note: Option<String>,
}

/// A picture copied out, at the video's size, encoded (`canvas`: room for
/// it): a window that grew is cut, one that shrank shown on black (or on
/// its background, centred). An encoder that fails is opened again, once:
/// its first picture a key frame, its stream going on in the same file
/// (in a sample entry of its own if its parameter sets differ). Failing
/// again, the next encoder that makes the same codec takes over (NVIDIA's
/// for the Intel GPU's AV1, the processor for NVIDIA's H.264: `others`),
/// and the recording says so. With none, what was recorded is kept: the
/// recording stops there, the file finished. (`OLO_RECORDING_FAIL_AT=N`,
/// for tests: picture N fails; `N!`, the encoder opened again fails too;
/// `N!!`, every other as well: `failing`, how many `!`.)
fn encode_copied(
    enc: &mut olo_video::Encoder,
    w: &Waiting,
    backdrop: &mut Option<crate::backdrop::Backdrop>,
    inset: &mut Option<crate::inset::Inset>,
    canvas: &mut [u8],
    fps: u32,
    failing: Option<(usize, usize)>,
) -> Result<Coded, String> {
    let (vw, vh) = enc.size();
    match backdrop.as_mut() {
        Some(b) => {
            let (anew, window) = b.compose(&w.data, w.stride, w.width, w.height, canvas);
            // The camera on the background's corner.
            if let Some(i) = inset.as_mut() {
                i.draw(canvas, vw as usize, window, anew);
            }
        }
        None => fit(w, canvas, vw, vh),
    }
    let stride = vw as usize * 4;
    let order = olo_video::Order::Bgra;
    // How many encoders fail on purpose: this one, the same again, all.
    let fails = failing.map_or(0, |(_, bangs)| bangs + 1);
    let first = if fails >= 1 { Err("failed on purpose (OLO_RECORDING_FAIL_AT)".to_owned()) } else { enc.encode(canvas, stride, order) };
    let e = match first {
        Ok((bits, key)) => return Ok(Coded { bits, key, fresh: false, note: None }),
        Err(e) => e,
    };
    let (was, others, codec) = (enc.name(), enc.others(), enc.codec());
    eprintln!("olo-screenshot: {was} failed ({e}); opening it again");
    let mut why = e;
    match olo_video::Encoder::new(vw, vh, fps, enc.choice(), codec) {
        Ok(again) => {
            *enc = again;
            match if fails >= 2 { Err("failed again on purpose".to_owned()) } else { enc.encode(canvas, stride, order) } {
                Ok((bits, key)) => return Ok(Coded { bits, key, fresh: true, note: None }),
                Err(e) => why = format!("{why}; opened again: {e}"),
            }
        }
        Err(e) => why = format!("{why}; opened again: {e}"),
    }
    for choice in others {
        let next = olo_video::Encoder::new(vw, vh, fps, choice, codec).and_then(|mut other| {
            if fails >= 3 {
                return Err("failed on purpose too".to_owned());
            }
            let coded = other.encode(canvas, stride, order)?;
            Ok((other, coded))
        });
        match next {
            Ok((other, (bits, key))) => {
                let note = format!("{was} failed ({why}); went on with {}", other.name());
                eprintln!("olo-screenshot: {note}");
                *enc = other;
                return Ok(Coded { bits, key, fresh: true, note: Some(note) });
            }
            Err(e) => why = format!("{why}; {}: {e}", choice.id()),
        }
    }
    Err(why)
}

/// The camera bubble's process (`bubble.rs`), while a recording shows
/// the camera; ended when dropped.
struct CameraBubble(std::process::Child);

impl CameraBubble {
    /// Started for camera `id` ("auto": the first colour camera), once
    /// its first frame is on the screen; and the camera's name.
    fn start(id: &str) -> Result<(CameraBubble, String), String> {
        use std::io::BufRead as _;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(exe)
            .args(["--camera-bubble", "--camera", id])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("the camera bubble: {e}"))?;
        let Some(out) = child.stdout.take() else { return Err("the camera bubble said nothing".into()) };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(out).read_line(&mut line);
            let _ = tx.send(line);
        });
        let bubble = CameraBubble(child);
        match rx.recv_timeout(Duration::from_secs(6)) {
            Ok(line) => match line.trim().strip_prefix("ready") {
                Some(name) => Ok((bubble, name.trim().to_owned())),
                None if line.trim().is_empty() => Err("the camera bubble quit".into()),
                None => Err(line.trim().to_owned()),
            },
            Err(_) => Err("the camera took too long to start".into()),
        }
    }
}

impl Drop for CameraBubble {
    fn drop(&mut self) {
        // SAFETY: a signal to our own child.
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let _ = self.0.wait();
    }
}

/// What the encoder's worker made: frames, the video's size, which encoder
/// and codec, why it stopped early if it did, and which encoders took
/// over from one that failed.
type Encoded = (usize, usize, (u32, u32), &'static str, olo_video::Codec, Option<String>, Option<String>);

/// The sound's description, and its access units as they come.
type SoundIn = (olo_video::aac::Track, Arc<Mutex<Vec<Vec<u8>>>>);

/// `w`'s picture into `canvas` (`vw` × `vh`, BGRA): cut, or padded black.
fn fit(w: &Waiting, canvas: &mut [u8], vw: u32, vh: u32) {
    let row = vw as usize * 4;
    let copy = (w.width.min(vw) as usize) * 4;
    for y in 0..vh as usize {
        let out = &mut canvas[y * row..(y + 1) * row];
        if y < w.height as usize {
            out[..copy].copy_from_slice(&w.data[y * w.stride..y * w.stride + copy]);
            out[copy..].fill(0);
        } else {
            out.fill(0);
        }
    }
}

/// `--toggle-record` (the desktop's shortcut): a recording of the screen
/// started, told of when saved, or the one going finished.
pub fn toggle(args: &[String]) -> i32 {
    if going().is_some() {
        return stop();
    }
    // A person's start: counted down, as the settings say (unless given).
    let mut args = args.to_vec();
    if option(&args, "--countdown").is_none() {
        let n = olo_ui::settings::current().map_or(3, |s| s.screen_recording.countdown);
        args.extend(["--countdown".to_owned(), n.to_string()]);
    }
    // The controls on the screen (never in the recording), for as long
    // as it goes (`controls.rs`).
    // (Waited for when they go, so they aren't left a zombie while the
    // saved notification waits.)
    if let Ok(mut controls) = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--controls").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn()) {
        std::thread::spawn(move || {
            let _ = controls.wait();
        });
    }
    match record(&args) {
        Ok(Some(made)) => {
            println!("{}", serde_json::to_string_pretty(&made).unwrap_or_default());
            // What it made, in a card in the corner (`after.rs`); told in
            // a notification if that can't start, or something went wrong
            // on the way (it says what).
            let card = (made.encoder_note.is_none() && made.stopped_early.is_none())
                .then(|| std::env::current_exe().ok())
                .flatten()
                .and_then(|exe| std::process::Command::new(exe).arg("--after").arg(&made.path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn().ok());
            if card.is_none() {
                tell(&made);
            }
            0
        }
        Ok(None) => 0,
        Err(e) => {
            eprintln!("olo-screenshot: the recording failed: {e}");
            1
        }
    }
}

/// A notification that the recording was saved, with Show in Tracker
/// (a person's recording; an agent's is said on its command line).
fn tell(made: &Made) {
    use olo_ui::notify::{Note, NoteEvent, Notifier};
    let (tx, rx) = std::sync::mpsc::channel();
    let notes = Notifier::with_events("Screenshot", "olo-screenshot", move |e| {
        let _ = tx.send(e);
    });
    let name = made.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let secs = made.seconds.round() as u64;
    let long = if secs >= 60 { format!("{} min {} s", secs / 60, secs % 60) } else { format!("{secs} s") };
    notes.show(None, Note::new("Screen recording saved").body(body(made, &name, &long)).icon("screenshot").action("default", "Show").action("show", "Show in Tracker"));
    // Waited for here (nothing polled): a click shows it, else it goes.
    let path = made.path.clone();
    while let Ok(e) = rx.recv_timeout(Duration::from_secs(60)) {
        match e {
            NoteEvent::Action { .. } => {
                let _ = olo_ui::tracker::open(&path);
                return;
            }
            NoteEvent::Closed { .. } => return,
            NoteEvent::Replied { .. } => {}
        }
    }
}

/// The saved notification's words: the file, how long and how big, and
/// what went wrong on the way (an encoder that failed and the one that
/// went on; the recording stopped early).
fn body(made: &Made, name: &str, long: &str) -> String {
    let mut out = format!("{name}\n{long}, {} × {}", made.width, made.height);
    if made.encoder_note.is_some() {
        out.push_str(&format!("\nAn encoder failed; finished on {}.", made.encoder));
    }
    if made.stopped_early.is_some() {
        out.push_str("\nStopped early: the encoder failed. What was recorded is kept.");
    }
    out
}

/// `--stop`: the recording on this display asked to finish; waits for it
/// (its file whole), then says where it went.
pub fn stop() -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    // Its end waited for (nothing polled), half a minute at most:
    // finishing writes the file's index (and a GIF from it, if asked).
    // SAFETY: a signal to the recorder's process.
    unsafe { libc::kill(g.pid as i32, libc::SIGTERM) };
    wait_finished(&g);
    let gif = g.path.with_extension("gif");
    if let Some(path) = [&g.path, &gif].into_iter().find(|p| p.exists()) {
        println!("{}", serde_json::json!({ "path": path }));
        0
    } else {
        eprintln!("olo-screenshot: the recording didn't finish ({})", g.path.display());
        1
    }
}

/// `--discard`: the recording on this display stopped and nothing kept
/// (the controls' Discard; Restart, then started again).
pub fn discard() -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    let flag = olo_protocol::recording::discard_path_for(&olo_protocol::recording::this_display());
    if let Err(e) = std::fs::write(&flag, b"") {
        eprintln!("olo-screenshot: {e}");
        return 1;
    }
    // (Its end waited for, as `--stop` does; it deletes what it made.)
    // SAFETY: a signal to the recorder's process.
    unsafe { libc::kill(g.pid as i32, libc::SIGTERM) };
    wait_finished(&g);
    let _ = std::fs::remove_file(&g.path);
    let _ = std::fs::remove_file(&flag);
    println!("{}", serde_json::json!({ "discarded": g.path }));
    0
}

/// What Restart runs again: `args`, a region that was dragged out (on
/// `display`, `crop` in its pixels at `scale`) given as the one to
/// record, so it isn't asked for again.
fn again(args: &[String], picked: Option<Option<String>>, crop: Option<[u32; 4]>, scale: f64) -> Vec<String> {
    let mut out = args.to_vec();
    let (Some(display), Some([x, y, w, h])) = (picked, crop) else { return out };
    let Some(i) = out.iter().position(|a| a == "--region") else { return out };
    let points = |v: u32| {
        let p = f64::from(v) / scale;
        if p.fract() == 0.0 { format!("{p}") } else { format!("{p:.2}") }
    };
    out.insert(i + 1, [x, y, w, h].map(points).join(","));
    if let Some(d) = display.filter(|_| option(args, "--display").is_none()) {
        out.extend(["--display".to_owned(), d]);
    }
    out
}

/// `--restart` (the controls' Restart): the recording on this display
/// discarded and the same one started again, with its own arguments (the
/// region dragged out kept), its controls with it if it had them.
pub fn restart() -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    if discard() != 0 {
        return 1;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)")),
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            return 1;
        }
    };
    match std::process::Command::new(exe).args(&g.args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn() {
        Ok(child) => {
            println!("{}", serde_json::json!({ "restarted": g.path, "pid": child.id() }));
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            1
        }
    }
}

/// Milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `--pause` (`on`) or `--resume`: the recording on this display paused
/// or going on again; says so once it has (its status file says it).
pub fn pause(on: bool) -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    // SAFETY: a signal to the recorder's process.
    unsafe { libc::kill(g.pid as i32, if on { libc::SIGUSR1 } else { libc::SIGUSR2 }) };
    println!("{}", serde_json::json!({ "path": g.path, "paused": on }));
    0
}

/// `--caption TEXT [--seconds N]`: a caption in what's recorded (and
/// shared), never on the screen: TEXT low in the middle for N seconds,
/// else till the next; an empty TEXT takes it down (app_server's
/// `Request::Caption`).
pub fn caption(args: &[String]) -> i32 {
    let text = option(args, "--caption").map(str::to_owned);
    let seconds = match option(args, "--seconds").map(str::parse::<f64>).transpose() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("olo-screenshot: --seconds takes a number");
            return 2;
        }
    };
    match olo_protocol::Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&olo_protocol::Request::Caption { text: text.clone(), seconds }).map_err(|e| e.to_string())) {
        Ok(olo_protocol::Reply::Error { message }) => {
            eprintln!("olo-screenshot: {message}");
            1
        }
        Ok(_) => {
            println!("{}", serde_json::json!({ "caption": text.filter(|t| !t.is_empty()), "seconds": seconds }));
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            1
        }
    }
}

/// `--zoom-to X,Y,W,H|out [--display NAME]`: the recording on this
/// display zoomed to that part (points from the display's top left, as
/// windows' frames are given), gliding there and held, or back out
/// (app_server's `Request::ZoomTo`).
pub fn zoom_to(args: &[String]) -> i32 {
    let rect = match option(args, "--zoom-to") {
        Some("out") => None,
        Some(v) => match rect(v) {
            Some(r) if r[2] > 0.0 && r[3] > 0.0 => Some(r),
            _ => {
                eprintln!("olo-screenshot: --zoom-to takes X,Y,W,H (points from the display's top left) or out");
                return 2;
            }
        },
        None => {
            eprintln!("olo-screenshot: --zoom-to takes X,Y,W,H or out");
            return 2;
        }
    };
    let display = option(args, "--display").map(str::to_owned);
    match olo_protocol::Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&olo_protocol::Request::ZoomTo { rect, display }).map_err(|e| e.to_string())) {
        Ok(olo_protocol::Reply::Error { message }) => {
            eprintln!("olo-screenshot: {message}");
            1
        }
        Ok(_) => {
            println!("{}", serde_json::json!({ "zoomed": rect }));
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            1
        }
    }
}

/// `--recording`: what's being recorded on this display, as JSON (null:
/// nothing).
pub fn status() -> i32 {
    println!("{}", serde_json::to_string_pretty(&going()).unwrap_or_default());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cuts_come_out_of_what_is_kept() {
        assert_eq!(stretches("0-12.5, 20-31"), Some(vec![(0.0, 12.5), (20.0, 31.0)]));
        assert_eq!(stretches("x"), None);
        assert_eq!(kept_less(vec![], &[(2.0, 3.0), (5.0, 6.0)], 8.0), vec![(0.0, 2.0), (3.0, 5.0), (6.0, 8.0)]);
        assert_eq!(kept_less(vec![(1.0, 7.0)], &[(0.0, 2.0)], 8.0), vec![(2.0, 7.0)]);
    }

    /// Restart records the region that was dragged out, in points, on its
    /// display, without asking for it again.
    #[test]
    fn restart_keeps_the_region_dragged_out() {
        let args = strings(&["--toggle-record", "--region", "--countdown", "3"]);
        let got = again(&args, Some(Some("eDP-1".into())), Some([200, 100, 1001, 600]), 2.0);
        assert_eq!(got, strings(&["--toggle-record", "--region", "100,50,500.50,300", "--countdown", "3", "--display", "eDP-1"]));
        // A region given, or none: as it was.
        let given = strings(&["--toggle-record", "--region", "1,2,3,4"]);
        assert_eq!(again(&given, None, Some([2, 4, 6, 8]), 2.0), given);
        let screen = strings(&["--toggle-record"]);
        assert_eq!(again(&screen, None, None, 1.0), screen);
    }
}
