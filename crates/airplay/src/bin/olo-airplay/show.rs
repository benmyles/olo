//! `olo-airplay show`: a window, a display, a workspace or an agent
//! screen shown live
//! on a receiver, with what the computer plays ([`olo_airplay::mirror`]).
//!
//! Frames come from app_server's capture (`olo_capture`, as the screen
//! recorder's do), asked for at most `--fps` times a second and given
//! only once what's captured has changed, so a still screen costs
//! nothing here and sends nothing but the receiver's heartbeat. The
//! newest frame waits for the encoder's worker (one that comes while the
//! last is being encoded takes its place). Frames come at the size the
//! receiver takes (`Screen::fit`: a 2× display made smaller, its shape
//! kept), drawn so by app_server on the GPU (`Capture::fit`; an older
//! app_server's are made smaller by the worker); the worker encodes
//! each (`olo_video::Encoder`: the Intel or AMD GPU through VA-API where
//! frames are drawn on it and its driver encodes H.264, else NVIDIA's,
//! else the processor; a picture out for each in, no B-frames) and sends
//! it stamped with its capture's moment. Frames for an encoder on the GPU
//! that draws them are lent to it as they are (`Capture::lend_frames`:
//! dmabufs made NV12 by its video processor), never copied through the
//! processor; the rest are copied out. The sound is the computer's
//! (`olo_system::recording`, the default output's monitor at 44.1 kHz,
//! 10 ms at a time), sent as it comes; while paused, silence, and the
//! last picture stays.
//!
//! When app_server goes (a reload, an upgrade), the showing waits for it
//! and captures again what `Capture::again` says (`reconnect`); the
//! receiver keeps the last picture meanwhile. Signals as `play` takes
//! them: Ctrl-C or SIGTERM stops (`olo-airplay stop`), SIGUSR1 pauses,
//! SIGUSR2 goes on. Where Deskbar's tray finds
//! it: `$XDG_RUNTIME_DIR/olo-airplay/PID.json` (`olo_protocol::airplay`),
//! its state "showing" or "paused".

use std::{
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

use olo_airplay::{
    kept,
    mirror::{Mirror, Options, Screen},
    our_name,
};
use olo_capture::{Capture, Source};
use olo_protocol::airplay::{self, Playing};

use super::{find, option, tell, words_in};

static STOP: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);

/// What is shown, and how it's named to the person.
pub(crate) struct What {
    /// The app_server to capture from (None: this one's).
    display: Option<String>,
    pub(crate) source: Source,
    pub(crate) title: String,
    /// Whether the computer's sound goes with it (an agent screen without
    /// sound has none).
    sound: bool,
}

/// An agent screen's Wayland display and whether it has sound, by its
/// name (as Agent Screens list them).
fn agent_screen(name: &str) -> Option<(String, bool)> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = olo_xdg::legacy::runtime_file(&runtime, "olo-screens").join(format!("{name}.json"));
    let said: serde_json::Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    Some((said.get("wayland_display")?.as_str()?.to_owned(), said.get("sound").and_then(serde_json::Value::as_bool).unwrap_or(false)))
}

pub(crate) fn what_of(args: &[String]) -> Result<What, String> {
    let titled = option(args, "--title").map(str::to_owned);
    if let Some(id) = option(args, "--window") {
        let id = id.parse::<u64>().map_err(|_| format!("{id} isn't a window's number"))?;
        return Ok(What { display: None, source: Source::Window { id, app_id: String::new(), title: String::new() }, title: titled.unwrap_or_else(|| format!("Window {id}")), sound: true });
    }
    if let Some(name) = option(args, "--screen") {
        let (display, sound) = agent_screen(name).ok_or_else(|| format!("no agent screen {name} is running (olo-screen list says which are)"))?;
        return Ok(What { display: Some(display), source: Source::Output(None), title: titled.unwrap_or_else(|| format!("Agent screen {name}")), sound });
    }
    let display = option(args, "--display").map(str::to_owned);
    if let Some(n) = option(args, "--workspace") {
        let n = n.parse::<u32>().ok().filter(|n| *n >= 1).ok_or("--workspace is a number from 1")?;
        let title = titled.unwrap_or_else(|| format!("Workspace {n}"));
        return Ok(What { display: None, source: Source::Workspace { output: display, index: n - 1 }, title, sound: true });
    }
    let title = titled.unwrap_or_else(|| display.as_ref().map_or_else(|| "The screen".to_owned(), |d| format!("Display {d}")));
    Ok(What { display: None, source: Source::Output(display), title, sound: true })
}

/// A pipe signals and the receiver's end write to, waking the capture's
/// wait.
struct Wake {
    read: OwnedFd,
    write: RawFd,
}

impl Wake {
    fn new() -> Result<Wake, String> {
        let mut fds = [0; 2];
        // SAFETY: pipe2 into two ints; the read end is ours from here on
        // (the write end lives as long as the program: signals write it).
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Wake { read: unsafe { OwnedFd::from_raw_fd(fds[0]) }, write: fds[1] })
    }

    fn poker(&self) -> impl Fn() + Send + Sync + Clone + 'static {
        let fd = self.write;
        move || {
            // SAFETY: a byte to our pipe's write end (full: it's awake anyway).
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }

    fn drain(&self) {
        let mut buf = [0u8; 64];
        // SAFETY: reading our own non-blocking pipe.
        while unsafe { libc::read(self.read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }
}

/// Wait on the capture's connection and the wake pipe, `at_most` (None:
/// till one of them).
fn wait(conn: RawFd, wake: RawFd, at_most: Option<Duration>) {
    let mut fds = [libc::pollfd { fd: conn, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 }];
    let ms = at_most.map_or(-1, |d| d.as_millis().clamp(1, i32::MAX as u128) as i32);
    // SAFETY: poll on two pollfds of ours.
    unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
}

/// How long after app_server ends a capture the connection is waited on
/// to say whether app_server is going (a reload: shown again once it's
/// back) or only what was shown went (a window closed: the end).
const GONE_AFTER: Duration = Duration::from_secs(2);

/// The capture again once app_server is back (a reload takes a second or
/// two; windows come back a little later): tried every quarter second
/// for half a minute, or until stopped. Fitted to the receiver's screen
/// again.
fn reconnect(old: &Capture, display: Option<&str>, screen: Screen, wake: &Wake) -> Result<Capture, String> {
    let source = old.again();
    let until = Instant::now() + Duration::from_secs(30);
    let mut why = String::from("stopped");
    while Instant::now() < until && !STOP.load(SeqCst) {
        wait(-1, wake.read.as_raw_fd(), Some(Duration::from_millis(250)));
        wake.drain();
        // (This display's by its direct socket, as at first.)
        let opened = match display {
            Some(display) => Capture::on(Some(display), &source, true),
            None => Capture::new(&source, true),
        };
        match opened {
            Ok(mut c) => {
                let _ = c.fit(screen.width, screen.height);
                return Ok(c);
            }
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// A frame waiting for the encoder: its rows (or, lent, its buffer on
/// the GPU), and when it was captured.
struct Waiting {
    data: Vec<u8>,
    lent: Option<olo_capture::Lent>,
    width: u32,
    height: u32,
    at: Instant,
}

/// The frame waiting, and a buffer the encoder is done with.
#[derive(Default)]
struct Held {
    waiting: Option<Waiting>,
    spare: Option<Vec<u8>>,
}

type Slot = Arc<(Mutex<Held>, Condvar)>;

pub fn show(args: &[String]) -> Result<(), String> {
    let notify = args.iter().any(|a| a == "--notify");
    let shown = shown(args);
    if notify && let Err(why) = &shown {
        let receiver = option(args, "--on").map(|on| kept::all().into_iter().find(|k| k.device_id.eq_ignore_ascii_case(on)).map_or_else(|| on.to_owned(), |k| k.name));
        let title = what_of(args).map(|w| w.title).unwrap_or_else(|_| "it".into());
        tell(receiver.as_deref().unwrap_or("AirPlay"), &title, why);
    }
    shown
}

fn shown(args: &[String]) -> Result<(), String> {
    let (trace, json) = (args.iter().any(|a| a == "--trace"), args.iter().any(|a| a == "--json"));
    let what = what_of(args)?;
    let with_sound = what.sound && !args.iter().any(|a| a == "--no-sound");
    let fps_asked = option(args, "--fps").map(|f| f.parse::<u32>().map_err(|_| format!("{f} isn't a number of frames a second"))).transpose()?.map(|f| f.clamp(1, 120));
    let choice = match option(args, "--encoder") {
        Some(e) => olo_video::Choice::parse(e).ok_or("--encoder is auto, vaapi, nvidia or cpu")?,
        None => olo_video::Choice::Auto,
    };
    let lead = option(args, "--lead").map(|ms| ms.parse::<u64>().map(Duration::from_millis).map_err(|_| format!("{ms} isn't milliseconds"))).transpose()?;
    let words = words_in(args);
    let receiver = option(args, "--on").map(str::to_owned).or_else(|| (!words.is_empty()).then(|| words.join(" "))).ok_or(super::USAGE)?;
    let found = find(&receiver, args)?;
    let name = found.name.clone();
    let kept = kept::of(&found.device_id, &name).ok_or_else(|| format!("{name} isn't paired with (olo-airplay pair)"))?;
    let credentials = kept.credentials().ok_or("its pairing here can't be read: pair with it again")?;

    // Signals, waited for by a thread of their own (blocked in every other,
    // made after this): stop, pause, go on, each waking the capture's wait.
    let wake = Wake::new()?;
    let poke = wake.poker();
    let signals = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2] {
            libc::sigaddset(&mut set, signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    {
        let poke = poke.clone();
        std::thread::spawn(move || {
            loop {
                let mut signal = 0;
                unsafe { libc::sigwait(&signals, &mut signal) };
                match signal {
                    libc::SIGUSR1 => PAUSED.store(true, SeqCst),
                    libc::SIGUSR2 => PAUSED.store(false, SeqCst),
                    _ => STOP.store(true, SeqCst),
                }
                poke();
            }
        });
    }

    // What's shown, first: a window that isn't there is said before the
    // receiver is woken.
    // (On this display, by its direct socket, as `Capture::new` finds it.)
    let mut capture = match &what.display {
        Some(display) => Capture::on(Some(display), &what.source, true)?,
        None => Capture::new(&what.source, true)?,
    };
    let tracer: Option<olo_airplay::play::Trace> = trace.then(|| Arc::new(|line: &str| eprintln!("  {line}")) as olo_airplay::play::Trace);
    let ended = {
        let poke = poke.clone();
        Arc::new(poke) as Arc<dyn Fn() + Send + Sync>
    };
    let options = Options { name: our_name(), sound: with_sound, lead, ended: Some(ended), trace: tracer, ..Options::default() };
    let mirror = Arc::new(Mirror::begin(found.at, &credentials, found.features, &found.version, &options).map_err(|e| e.to_string())?);
    let (screen, lead_ms, sounds) = (mirror.screen(), mirror.lead().as_millis() as u64, mirror.sounds());
    // Frames the receiver's size, made so by app_server as it draws them
    // (on the GPU), not shrunk here each frame (an older app_server: here).
    match capture.fit(screen.width, screen.height) {
        Ok(true) if trace => eprintln!("  frames fitted to {}×{} by app_server", screen.width, screen.height),
        Ok(_) => {}
        Err(e) => eprintln!("olo-airplay: frames made smaller here: {e}"),
    }
    let say = |state: &str, why: Option<&str>| {
        if json {
            println!("{}", serde_json::json!({ "state": state, "receiver": name, "what": what.title, "screen": screen, "lead_ms": lead_ms, "sound": sounds, "why": why }));
        } else {
            match state {
                "showing" => println!("Showing {} on {name}{}.", what.title, if sounds { ", with the computer's sound" } else { "" }),
                "paused" => println!("Paused: {name} shows the last picture."),
                "stopped" => println!("Stopped."),
                _ => println!("{name} stopped showing it{}.", why.map(|w| format!(": {w}")).unwrap_or_default()),
            }
        }
    };
    say("showing", None);
    let (pid, started) = (std::process::id(), airplay::now_ms() / 1000);
    let publish = |paused: bool| {
        let p = Playing {
            pid,
            receiver: name.clone(),
            title: what.title.clone(),
            path: None,
            state: if paused { "paused" } else { "showing" }.into(),
            position: None,
            duration: None,
            rate: if paused { 0.0 } else { 1.0 },
            as_of: airplay::now_ms(),
            started,
            live: true,
        };
        let _ = p.write();
    };
    publish(false);

    // The encoder's worker, on the GPU that draws the frames if it can
    // (they're lent to it then, never copied).
    let slot: Slot = Arc::default();
    let fps = Arc::new(AtomicU32::new(fps_asked.unwrap_or(30).min(screen.fps)));
    let drawn_on = capture.render_device();
    let prime = Arc::new(AtomicU64::new(0));
    let worker = {
        let (slot, mirror, fps, poke, prime) = (slot.clone(), mirror.clone(), fps.clone(), poke.clone(), prime.clone());
        std::thread::Builder::new()
            .name("airplay encoder".into())
            .spawn(move || {
                let why = encode(&slot, &mirror, screen, (choice, drawn_on), fps_asked, &fps, &prime, trace);
                if let Err(why) = &why {
                    eprintln!("olo-airplay: {why}");
                }
                STOP.store(true, SeqCst);
                poke();
            })
            .map_err(|e| e.to_string())?
    };

    // The computer's sound, as it comes (silence while paused).
    let sound = if mirror.sounds() {
        let mirror = mirror.clone();
        let mut pcm: Vec<i16> = Vec::with_capacity(1024);
        match olo_system::recording::Capture::start_as(Some(olo_system::recording::COMPUTER), 2, 44_100, 10, "AirPlay", move |samples: &[f32]| {
            let at = Instant::now() - Duration::from_secs_f64(samples.len() as f64 / 2.0 / 44_100.0);
            pcm.clear();
            let paused = PAUSED.load(SeqCst);
            pcm.extend(samples.iter().map(|s| if paused { 0 } else { (s.clamp(-1.0, 1.0) * 32767.0) as i16 }));
            mirror.sound(&pcm, at);
        }) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("olo-airplay: showing without sound: {e}");
                None
            }
        }
    } else {
        None
    };

    // Frames, as they come.
    let mut conn = capture.fd()?;
    let mut asked_at: Option<Instant> = None;
    let mut paused = false;
    // app_server went (a reload, an upgrade: it comes back), and when it
    // ended the capture (what was shown went, or app_server is going,
    // which the connection soon says).
    let mut lost = false;
    let mut stopped_at: Option<Instant> = None;
    // Frames lent to the encoder (its GPU's, at the receiver's size).
    let mut lending = false;
    let result: Result<Option<String>, String> = (|| {
        loop {
            wake.drain();
            if STOP.load(SeqCst) {
                return Ok(None);
            }
            if let Some(why) = mirror.ended() {
                return Ok(Some(why));
            }
            if lost {
                // The receiver keeps the last picture meanwhile.
                eprintln!("olo-airplay: app_server went away; showing again once it's back");
                let again = match reconnect(&capture, what.display.as_deref(), screen, &wake) {
                    Ok(again) => again,
                    Err(_) if STOP.load(SeqCst) => return Ok(None),
                    Err(why) => return Ok(Some(format!("app_server didn't come back ({why})"))),
                };
                capture = again;
                conn = capture.fd()?;
                (lost, stopped_at, asked_at, lending) = (false, None, None, false);
                if trace {
                    eprintln!("  capturing {} again", capture.name());
                }
                continue;
            }
            if capture.stopped() {
                let since = *stopped_at.get_or_insert_with(Instant::now);
                let left = GONE_AFTER.saturating_sub(since.elapsed());
                if left.is_zero() {
                    return Ok(Some("what was shown went away".into()));
                }
                wait(conn.as_raw_fd(), wake.read.as_raw_fd(), Some(left));
                lost = capture.poll().is_err();
                continue;
            }
            let now_paused = PAUSED.load(SeqCst);
            if now_paused != paused {
                paused = now_paused;
                publish(paused);
                say(if paused { "paused" } else { "showing" }, None);
                // (The screen as it is now, at once.)
                asked_at = None;
            }
            if paused {
                wait(conn.as_raw_fd(), wake.read.as_raw_fd(), None);
                lost = capture.poll().is_err();
                continue;
            }
            // Frames lent from the next asked for, if the encoder is on the
            // GPU they're drawn on; copied again if it can't take them.
            let lend = prime.load(SeqCst);
            if lend != 0 && Some(lend) == capture.render_device() && !lending {
                capture.lend_frames(Some(olo_video::vaapi::imports));
                lending = true;
            } else if lend == 0 && lending {
                capture.lend_frames(None);
                lending = false;
            }
            // Ask for the next once its time has come, then wait for it.
            if !capture.waiting() {
                let every = Duration::from_secs_f64(1.0 / f64::from(fps.load(SeqCst).max(1)));
                let due = asked_at.map_or(Duration::ZERO, |a| (a + every).saturating_duration_since(Instant::now()));
                if due > Duration::ZERO {
                    wait(conn.as_raw_fd(), wake.read.as_raw_fd(), Some(due));
                    continue;
                }
                if capture.request().is_err() {
                    lost = true;
                    continue;
                }
                asked_at = Some(Instant::now());
            }
            wait(conn.as_raw_fd(), wake.read.as_raw_fd(), None);
            match capture.poll() {
                Err(_) => lost = true,
                Ok(Some(Ok(()))) => {
                    let at = Instant::now();
                    let Some((w, h)) = capture.frame_size() else { continue };
                    // Lent as it is when it's the receiver's size (app_server
                    // fitted it), else copied.
                    let lent = if lending && screen.fit(w, h) == (w, h) { capture.lend() } else { None };
                    let (m, cv) = &*slot;
                    let mut held = m.lock().unwrap_or_else(|e| e.into_inner());
                    // (A buffer used again: one the encoder didn't get to,
                    // or one it's done with.)
                    let mut data = held.waiting.take().map(|w| w.data).filter(|d| d.capacity() > 0).or_else(|| held.spare.take()).unwrap_or_default();
                    if lent.is_none() {
                        data.resize(w as usize * h as usize * 4, 0);
                        capture.copy_into(&mut data, w as usize * 4)?;
                    } else if held.spare.is_none() {
                        held.spare = Some(std::mem::take(&mut data));
                    }
                    held.waiting = Some(Waiting { data, lent, width: w, height: h, at });
                    cv.notify_one();
                }
                Ok(_) => {}
            }
        }
    })();
    STOP.store(true, SeqCst);
    slot.1.notify_all();
    drop(sound);
    let _ = worker.join();
    let _ = std::fs::remove_file(airplay::path_of(pid));
    let ended = match &result {
        Ok(Some(why)) => Some(why.clone()),
        _ => None,
    };
    drop(mirror);
    match result {
        Ok(None) => {
            say("stopped", None);
            Ok(())
        }
        Ok(Some(why)) => {
            say("ended", ended.as_deref());
            if why.contains("can't be reached") { Err(why) } else { Ok(()) }
        }
        Err(e) => Err(e),
    }
}

/// The encoder: as asked (`choice`); asked for none, VA-API's where it
/// encodes on the GPU frames are drawn on (`drawn_on`, a render node's
/// device number), else `Auto`'s.
fn encoder_for(vw: u32, vh: u32, fps: u32, (choice, drawn_on): (olo_video::Choice, Option<u64>)) -> Result<olo_video::Encoder, String> {
    use olo_video::{Choice, Codec, Encoder};
    if choice == Choice::Auto
        && let Some(gpu) = drawn_on
        && let Ok(e) = Encoder::new(vw, vh, fps, Choice::Vaapi, Codec::H264)
        && e.prime_device() == Some(gpu)
    {
        return Ok(e);
    }
    Encoder::new(vw, vh, fps, choice, Codec::H264)
}

/// With --trace: 120 frames' times (resize, encode, captured to sent),
/// said as medians and 95th percentiles, then forgotten.
fn say_timings(t: &mut Vec<[f64; 3]>) {
    let median = |i: usize| {
        let mut v: Vec<f64> = t.iter().map(|s| s[i] * 1000.0).collect();
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() * 95 / 100])
    };
    let (r, c, l) = (median(0), median(1), median(2));
    eprintln!("  frames: resize {:.1} ms (95%: {:.1}), encode {:.1} ms ({:.1}), captured to sent {:.1} ms ({:.1})", r.0, r.1, c.0, c.1, l.0, l.1);
    t.clear();
}

/// The encoder's worker: each frame made the receiver's size, encoded
/// and sent, until stopped; `prime` set to the GPU it encodes frames in
/// that GPU's memory on (0: none, or it took none). An error: no
/// encoder, or the receiver gone.
#[allow(clippy::too_many_arguments)]
fn encode(slot: &Slot, mirror: &Mirror, screen: Screen, choice: (olo_video::Choice, Option<u64>), fps_asked: Option<u32>, fps: &AtomicU32, prime: &AtomicU64, trace: bool) -> Result<(), String> {
    use fast_image_resize::{
        PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::{Image, ImageRef},
    };
    let mut encoder: Option<olo_video::Encoder> = None;
    let mut resizer = Resizer::new();
    let mut small: Option<Image<'static>> = None;
    // (With --trace: how long each frame takes, every 120.)
    let mut timings: Option<Vec<[f64; 3]>> = trace.then(Vec::new);
    loop {
        let mut frame = {
            let (m, cv) = &**slot;
            let mut held = m.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if STOP.load(SeqCst) {
                    return Ok(());
                }
                if let Some(frame) = held.waiting.take() {
                    break frame;
                }
                held = cv.wait(held).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = Instant::now();
        let (vw, vh) = screen.fit(frame.width, frame.height);
        if encoder.as_ref().is_none_or(|e| e.size() != (vw, vh)) {
            // (A window that changed size: an encoder for its new size, its
            // first picture a key frame, the receiver told the new sets.)
            let wanted = fps_asked.unwrap_or(60).min(screen.fps);
            // (The one before let go first: a GPU's encoder is one at a time.)
            drop(encoder.take());
            let e = encoder_for(vw, vh, wanted, choice)?;
            // (The processor keeps up with fewer.)
            let rate = if e.choice() == olo_video::Choice::Cpu { fps_asked.unwrap_or(30).min(wanted) } else { wanted };
            fps.store(rate, SeqCst);
            prime.store(e.prime_device().unwrap_or(0), SeqCst);
            let lent = if e.prime_device().is_some_and(|d| Some(d) == choice.1) { ", frames lent from the GPU" } else { "" };
            eprintln!("olo-airplay: {vw}×{vh} at up to {rate} a second, encoded on {}{lent}", e.name());
            encoder = Some(e);
        }
        let e = encoder.as_mut().expect("made above");
        // A frame lent: encoded where it is (made NV12 by the GPU), given
        // back at once. One the encoder can't take: dropped, and the next
        // ones copied.
        if let Some(lent) = frame.lent.take() {
            let picture =
                olo_video::vaapi::Prime { key: lent.key, fd: lent.fd(), modifier: lent.modifier, offset: lent.offset, pitch: lent.pitch, size: lent.size, width: lent.width, height: lent.height };
            let coded = e.encode_prime(&picture);
            drop(lent);
            let encoded = Instant::now();
            match coded {
                Ok((annex_b, _)) => {
                    mirror.show(&annex_b, (vw, vh), frame.at)?;
                    if let Some(t) = timings.as_mut() {
                        t.push([0.0, (encoded - started).as_secs_f64(), (encoded - frame.at).as_secs_f64()]);
                    }
                }
                Err(why) => {
                    eprintln!("olo-airplay: the encoder took no frame from the GPU ({why}): copying them");
                    prime.store(0, SeqCst);
                }
            }
            if let Some(t) = timings.as_mut().filter(|t| t.len() == 120) {
                say_timings(t);
            }
            if frame.data.capacity() > 0 {
                slot.0.lock().unwrap_or_else(|e| e.into_inner()).spare.get_or_insert(frame.data);
            }
            continue;
        }
        let (data, stride): (&[u8], usize) = if (vw, vh) == (frame.width, frame.height) {
            (&frame.data, frame.width as usize * 4)
        } else {
            let src = ImageRef::new(frame.width, frame.height, &frame.data, PixelType::U8x4).map_err(|e| e.to_string())?;
            let dst = small.get_or_insert_with(|| Image::new(vw, vh, PixelType::U8x4));
            if (dst.width(), dst.height()) != (vw, vh) {
                *dst = Image::new(vw, vh, PixelType::U8x4);
            }
            // (BGRx: the fourth byte isn't alpha, and may be 0. Weighed as
            // alpha, every pixel came out black.)
            let how = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(fast_image_resize::FilterType::Bilinear)).use_alpha(false);
            resizer.resize(&src, dst, &how).map_err(|e| e.to_string())?;
            (dst.buffer(), vw as usize * 4)
        };
        let resized = Instant::now();
        let (annex_b, _) = e.encode(data, stride, olo_video::Order::Bgra)?;
        let encoded = Instant::now();
        mirror.show(&annex_b, (vw, vh), frame.at)?;
        if let Some(t) = timings.as_mut() {
            t.push([(resized - started).as_secs_f64(), (encoded - resized).as_secs_f64(), (encoded - frame.at).as_secs_f64()]);
            if t.len() == 120 {
                say_timings(t);
            }
        }
        // (Its buffer back, for the next frame.)
        slot.0.lock().unwrap_or_else(|e| e.into_inner()).spare = Some(frame.data);
    }
}
