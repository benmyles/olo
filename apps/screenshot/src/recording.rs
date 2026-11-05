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
//!                [--encoder auto|cpu|nvidia] [--to PATH]
//! olo-screenshot --stop          the recording on this display, finished
//! olo-screenshot --pause         nothing recorded till --resume
//! olo-screenshot --resume
//! olo-screenshot --recording     whether one is going, as JSON
//! ```
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

/// A picture waiting for the encoder: its pixels and when it came.
struct Waiting {
    data: Vec<u8>,
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
    if let Some(i) = args.iter().position(|a| a == "--region") {
        if matches!(source, Source::Window { .. }) {
            return Err("--region is part of a display, not of a window".into());
        }
        match args.get(i + 1).filter(|v| !v.starts_with("--")) {
            Some(v) => region = Some(rect(v).ok_or("--region takes X,Y,W,H (points from the display's top left), or nothing to drag one out")?),
            None => {
                let Some((output, r)) = crate::region::pick_region() else { return Ok(None) };
                source = Source::Output(output);
                crop = Some(r);
            }
        }
    }
    let fps: u32 = option(args, "--fps").map_or(Ok(30), str::parse).map_err(|_| "--fps takes a number")?;
    let fps = fps.clamp(1, 60);
    // As asked, else as Preferences ▸ GPUs has it, else the most
    // powerful there is.
    let chosen = option(args, "--encoder").map(str::to_owned).or_else(|| match olo_protocol::Client::connect().ok()?.request(&olo_protocol::Request::GetSettings).ok()? {
        olo_protocol::Reply::Settings { settings } => Some(settings.screen_recording.encoder),
        _ => None,
    });
    let choice = match chosen.as_deref() {
        Some(e) => olo_video::Choice::parse(e).ok_or("--encoder is auto, cpu or nvidia")?,
        None => olo_video::Choice::Auto,
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
    }
    let what = match &source {
        Source::Window { id, .. } => format!("window {id}"),
        Source::Output(_) if crop.is_some() => "region".into(),
        Source::Output(_) => "display".into(),
    };
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let g = Going { pid: std::process::id(), path: path.clone(), started: now_secs, what, paused: false };
    let _ = std::fs::write(going_path(), serde_json::to_vec(&g).unwrap_or_default());
    let result = record_into(&mut capture, &path, fps, seconds, choice, crop, &g, woken.as_raw_fd());
    let _ = std::fs::remove_file(going_path());
    WAKE.store(-1, Ordering::SeqCst);
    drop(waker);
    result.map(Some)
}

#[allow(clippy::too_many_arguments)]
fn record_into(capture: &mut Capture, path: &Path, fps: u32, seconds: Option<f64>, choice: olo_video::Choice, crop: Option<[u32; 4]>, going: &Going, woken: i32) -> Result<Made, String> {
    let start = Instant::now();
    let fd = capture.fd()?;
    let every = Duration::from_secs_f64(1.0 / f64::from(fps));
    // The encoder's: one picture waiting at most (a newer takes its place).
    let slot: Arc<(Mutex<Option<Waiting>>, std::sync::Condvar)> = Arc::default();
    let ended = Arc::new(AtomicBool::new(false));
    // Where the video ends (milliseconds of it), for its last frame.
    let end = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let part = path.with_extension("mp4.part");
    let worker = {
        let (slot, ended, end, part) = (slot.clone(), ended.clone(), end.clone(), part.clone());
        std::thread::Builder::new().name("recording encoder".into()).spawn(move || encode(&slot, &ended, &end, &part, fps, choice)).map_err(|e| e.to_string())?
    };
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
                let g = Going { paused, ..going.clone() };
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
                    let at = recorded(paused_for, paused_at);
                    let (m, cv) = &*slot;
                    *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(Waiting { data, stride, width: w, height: h, at });
                    cv.notify_one();
                }
                // (A size changed: the next request makes a new buffer.)
                Some(Err(_)) | None => {}
            }
        }
    })();
    let total = recorded(paused_for, paused_at);
    end.store(total.as_millis() as u64, Ordering::SeqCst);
    ended.store(true, Ordering::SeqCst);
    slot.1.notify_one();
    let made = worker.join().map_err(|_| "the encoder failed".to_string())?;
    result?;
    let (frames, (width, height), encoder) = made?;
    // The file finished by the worker, then put where it goes.
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Made { path: path.to_path_buf(), seconds: (total.as_millis() as f64 / 1000.0), frames, width, height, encoder: encoder.to_owned() })
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
fn encode(
    slot: &(Mutex<Option<Waiting>>, std::sync::Condvar),
    ended: &AtomicBool,
    end: &std::sync::atomic::AtomicU64,
    part: &Path,
    fps: u32,
    choice: olo_video::Choice,
) -> Result<(usize, (u32, u32), &'static str), String> {
    let (m, cv) = slot;
    // Milliseconds: durations as they came.
    let mut video: Option<(olo_video::Encoder, olo_video::Mp4, Vec<u8>)> = None;
    let mut held: Option<(Vec<u8>, bool, Duration)> = None;
    let mut frames = 0usize;
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
        let Some(w) = next else { break };
        if video.is_none() {
            let (vw, vh) = olo_video::even(w.width, w.height);
            let enc = olo_video::Encoder::new(vw, vh, fps, choice)?;
            let mp4 = olo_video::Mp4::create(part, vw, vh, 1000).map_err(|e| e.to_string())?;
            video = Some((enc, mp4, vec![0u8; vw as usize * 4 * vh as usize]));
        }
        let (enc, mp4, canvas) = video.as_mut().unwrap();
        let (vw, vh) = enc.size();
        // The picture at the video's size: a window that grew is cut, one
        // that shrank shown on black.
        fit(&w, canvas, vw, vh);
        let (bits, key) = enc.encode(canvas, vw as usize * 4, olo_video::Order::Bgra)?;
        if let Some((b, k, at)) = held.take() {
            let ms = w.at.saturating_sub(at).as_millis().max(1) as u32;
            mp4.add(&b, k, ms).map_err(|e| e.to_string())?;
            frames += 1;
        }
        held = Some((bits, key, w.at));
    }
    let Some((enc, mut mp4, _)) = video else { return Err("nothing was shown to record".into()) };
    if let Some((b, k, at)) = held {
        let ms = Duration::from_millis(end.load(Ordering::SeqCst)).saturating_sub(at).as_millis().max(1) as u32;
        mp4.add(&b, k, ms).map_err(|e| e.to_string())?;
        frames += 1;
    }
    let made = mp4.size();
    mp4.finish().map_err(|e| e.to_string())?;
    Ok((frames, made, enc.name()))
}

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
    match record(args) {
        Ok(Some(made)) => {
            tell(&made);
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
    notes.show(
        None,
        Note::new("Screen recording saved").body(format!("{name}\n{long}, {} × {}", made.width, made.height)).icon("screenshot").action("default", "Show").action("show", "Show in Tracker"),
    );
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

/// `--stop`: the recording on this display asked to finish; waits for it
/// (its file whole), then says where it went.
pub fn stop() -> i32 {
    let Some(g) = going() else {
        eprintln!("olo-screenshot: nothing is being recorded on this display");
        return 1;
    };
    // Its end waited for on a pidfd (nothing polled), half a minute at
    // most: finishing writes the file's index.
    // SAFETY: pidfd_open, kill and poll on the recorder's process.
    unsafe {
        let pidfd = libc::syscall(libc::SYS_pidfd_open, g.pid as libc::pid_t, 0) as i32;
        libc::kill(g.pid as i32, libc::SIGTERM);
        if pidfd >= 0 {
            let mut p = libc::pollfd { fd: pidfd, events: libc::POLLIN, revents: 0 };
            libc::poll(&mut p, 1, 30_000);
            libc::close(pidfd);
        }
    }
    if g.path.exists() {
        println!("{}", serde_json::json!({ "path": g.path }));
        0
    } else {
        eprintln!("olo-screenshot: the recording didn't finish ({})", g.path.display());
        1
    }
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

/// `--recording`: what's being recorded on this display, as JSON (null:
/// nothing).
pub fn status() -> i32 {
    println!("{}", serde_json::to_string_pretty(&going()).unwrap_or_default());
    0
}
