//! Screen recording: a display or a window into an MP4, for people and
//! agents alike, all in process (`olo_video`: H.264 from OpenH264 into an
//! MP4 of Olo's writing). Frames come from app_server's capture
//! (`olo_capture`), and only when something changed: the recorder asks
//! for the next at most `--fps` times a second and waits on the
//! connection (nothing polled), and each picture is held until the next
//! comes, so a still screen costs nothing and still keeps its time. They
//! are encoded on a worker as they come; one that comes while the worker
//! is still busy with the last waits in its place, replaced by a newer.
//!
//! ```text
//! olo-screenshot --record [--window ID] [--display NAME] [--seconds N]
//!                [--fps N] [--no-pointer] [--encoder auto|cpu|nvidia] [--to PATH]
//! olo-screenshot --stop          the recording on this display, finished
//! olo-screenshot --recording     whether one is going, as JSON
//! ```
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
    os::fd::AsRawFd as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_capture::{Capture, Source};
use serde::Serialize;

/// Asked to stop (SIGINT, SIGTERM: `--stop`).
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
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
/// on a finished file.
pub fn run(args: &[String]) -> i32 {
    match record(args) {
        Ok(made) => {
            println!("{}", serde_json::to_string_pretty(&made).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("olo-screenshot: the recording failed: {e}");
            1
        }
    }
}

fn record(args: &[String]) -> Result<Made, String> {
    if let Some(g) = going() {
        return Err(format!("this display is being recorded already (into {}; olo-screenshot --stop)", g.path.display()));
    }
    let source = match option(args, "--window") {
        Some(id) => Source::Window { id: id.parse().map_err(|_| "--window takes a window's id")?, app_id: String::new(), title: String::new() },
        None => Source::Output(option(args, "--display").map(str::to_owned)),
    };
    let fps: u32 = option(args, "--fps").map_or(Ok(30), str::parse).map_err(|_| "--fps takes a number")?;
    let fps = fps.clamp(1, 60);
    let choice = match option(args, "--encoder") {
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
    // SAFETY: handlers that only set a flag.
    unsafe {
        libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
    }
    let what = match &source {
        Source::Window { id, .. } => format!("window {id}"),
        Source::Output(_) => "display".into(),
    };
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let g = Going { pid: std::process::id(), path: path.clone(), started: now_secs, what };
    let _ = std::fs::write(going_path(), serde_json::to_vec(&g).unwrap_or_default());
    let result = record_into(&mut capture, &path, fps, seconds, choice);
    let _ = std::fs::remove_file(going_path());
    result
}

fn record_into(capture: &mut Capture, path: &Path, fps: u32, seconds: Option<f64>, choice: olo_video::Choice) -> Result<Made, String> {
    let start = Instant::now();
    let fd = capture.fd()?;
    let every = Duration::from_secs_f64(1.0 / f64::from(fps));
    // The encoder's: one picture waiting at most (a newer takes its place).
    let slot: Arc<(Mutex<Option<Waiting>>, std::sync::Condvar)> = Arc::default();
    let ended = Arc::new(AtomicBool::new(false));
    let part = path.with_extension("mp4.part");
    let worker = {
        let (slot, ended, part) = (slot.clone(), ended.clone(), part.clone());
        std::thread::Builder::new().name("recording encoder".into()).spawn(move || encode(&slot, &ended, &part, fps, choice)).map_err(|e| e.to_string())?
    };
    let mut asked_at: Option<Instant> = None;
    let result: Result<(), String> = (|| {
        loop {
            if STOP.load(Ordering::SeqCst) || seconds.is_some_and(|s| start.elapsed().as_secs_f64() >= s) {
                return Ok(());
            }
            if capture.stopped() {
                // (The window closed, or app_server is going away.)
                return Ok(());
            }
            // Ask for the next once its time has come (a frame every
            // `every` at most), then wait for it on the connection.
            if !capture.waiting() {
                let due = asked_at.map_or(Duration::ZERO, |a| (a + every).saturating_duration_since(Instant::now()));
                if due > Duration::ZERO {
                    wait_fd(fd.as_raw_fd(), due.min(Duration::from_millis(250)));
                    continue;
                }
                capture.request()?;
                asked_at = Some(Instant::now());
            }
            // (Woken now and then to see whether to stop: a still screen
            // sends nothing.)
            wait_fd(fd.as_raw_fd(), Duration::from_millis(250));
            match capture.poll()? {
                Some(Ok(())) => {
                    let Some((w, h)) = capture.frame_size() else { continue };
                    let stride = w as usize * 4;
                    let mut data = vec![0u8; stride * h as usize];
                    capture.copy_into(&mut data, stride)?;
                    let at = start.elapsed();
                    let (m, cv) = &*slot;
                    *m.lock().unwrap_or_else(|e| e.into_inner()) = Some(Waiting { data, stride, width: w, height: h, at });
                    cv.notify_one();
                }
                // (A size changed: the next request makes a new buffer.)
                Some(Err(_)) | None => {}
            }
        }
    })();
    let total = start.elapsed();
    ended.store(true, Ordering::SeqCst);
    slot.1.notify_one();
    let made = worker.join().map_err(|_| "the encoder failed".to_string())?;
    result?;
    let (frames, (width, height), encoder) = made?;
    // The file finished by the worker, then put where it goes.
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Made { path: path.to_path_buf(), seconds: (total.as_millis() as f64 / 1000.0), frames, width, height, encoder: encoder.to_owned() })
}

/// Wait until `fd` has something to read, or `timeout`.
fn wait_fd(fd: i32, timeout: Duration) {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd, ours.
    unsafe { libc::poll(&mut p, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
}

/// The encoder's loop: each picture as it's handed over, held until the
/// next; at the end, the last held till then and the file finished.
/// Frames encoded, and the video's size.
fn encode(slot: &(Mutex<Option<Waiting>>, std::sync::Condvar), ended: &AtomicBool, part: &Path, fps: u32, choice: olo_video::Choice) -> Result<(usize, (u32, u32), &'static str), String> {
    let (m, cv) = slot;
    // Milliseconds: durations as they came.
    let mut video: Option<(olo_video::Encoder, olo_video::Mp4, Vec<u8>)> = None;
    let mut held: Option<(Vec<u8>, bool, Duration)> = None;
    let mut frames = 0usize;
    let started = Instant::now();
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
        let ms = started.elapsed().saturating_sub(at).as_millis().max(1) as u32;
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
        Ok(made) => {
            tell(&made);
            println!("{}", serde_json::to_string_pretty(&made).unwrap_or_default());
            0
        }
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

/// `--recording`: what's being recorded on this display, as JSON (null:
/// nothing).
pub fn status() -> i32 {
    println!("{}", serde_json::to_string_pretty(&going()).unwrap_or_default());
    0
}
