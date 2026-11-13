//! `olo-airplay show`: a window, a display or an agent screen shown live
//! on a receiver, with what the computer plays ([`olo_airplay::mirror`]).
//!
//! Frames come from app_server's capture (`olo_capture`, as the screen
//! recorder's do), asked for at most `--fps` times a second and given
//! only once what's captured has changed, so a still screen costs
//! nothing here and sends nothing but the receiver's heartbeat. The
//! newest frame waits for the encoder's worker (one that comes while the
//! last is being encoded takes its place), which makes it the size the
//! receiver takes (`Screen::fit`: a 2× display made smaller, its shape
//! kept), encodes it (`olo_video::Encoder`: NVIDIA's GPU where it opens,
//! else the processor; a picture out for each in, no B-frames) and sends
//! it stamped with its capture's moment. The sound is the computer's
//! (`olo_system::recording`, the default output's monitor at 44.1 kHz,
//! 10 ms at a time), sent as it comes; while paused, silence, and the
//! last picture stays.
//!
//! Signals as `play` takes them: Ctrl-C or SIGTERM stops (`olo-airplay
//! stop`), SIGUSR1 pauses, SIGUSR2 goes on. Where Deskbar's tray finds
//! it: `$XDG_RUNTIME_DIR/olo-airplay/PID.json` (`olo_protocol::airplay`),
//! its state "showing" or "paused".

use std::{
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering::SeqCst},
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
struct What {
    /// The app_server to capture from (None: this one's).
    display: Option<String>,
    source: Source,
    title: String,
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

fn what_of(args: &[String]) -> Result<What, String> {
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

/// A frame waiting for the encoder: its rows, and when it was captured.
struct Waiting {
    data: Vec<u8>,
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
        Some(e) => olo_video::Choice::parse(e).filter(|c| *c != olo_video::Choice::Vaapi).ok_or("--encoder is auto, nvidia or cpu")?,
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

    // The encoder's worker.
    let slot: Slot = Arc::default();
    let fps = Arc::new(AtomicU32::new(fps_asked.unwrap_or(30).min(screen.fps)));
    let worker = {
        let (slot, mirror, fps, poke) = (slot.clone(), mirror.clone(), fps.clone(), poke.clone());
        std::thread::Builder::new()
            .name("airplay encoder".into())
            .spawn(move || {
                let why = encode(&slot, &mirror, screen, choice, fps_asked, &fps, trace);
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
    let conn = capture.fd()?;
    let mut asked_at: Option<Instant> = None;
    let mut paused = false;
    let result: Result<Option<String>, String> = (|| {
        loop {
            wake.drain();
            if STOP.load(SeqCst) {
                return Ok(None);
            }
            if let Some(why) = mirror.ended() {
                return Ok(Some(why));
            }
            if capture.stopped() {
                return Ok(Some("what was shown went away".into()));
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
                let _ = capture.poll()?;
                continue;
            }
            // Ask for the next once its time has come, then wait for it.
            if !capture.waiting() {
                let every = Duration::from_secs_f64(1.0 / f64::from(fps.load(SeqCst).max(1)));
                let due = asked_at.map_or(Duration::ZERO, |a| (a + every).saturating_duration_since(Instant::now()));
                if due > Duration::ZERO {
                    wait(conn.as_raw_fd(), wake.read.as_raw_fd(), Some(due));
                    continue;
                }
                capture.request()?;
                asked_at = Some(Instant::now());
            }
            wait(conn.as_raw_fd(), wake.read.as_raw_fd(), None);
            if let Some(Ok(())) = capture.poll()? {
                let at = Instant::now();
                let Some((w, h)) = capture.frame_size() else { continue };
                let (m, cv) = &*slot;
                let mut held = m.lock().unwrap_or_else(|e| e.into_inner());
                // (A buffer used again: one the encoder didn't get to, or
                // one it's done with.)
                let mut data = held.waiting.take().map(|w| w.data).or_else(|| held.spare.take()).unwrap_or_default();
                data.resize(w as usize * h as usize * 4, 0);
                capture.copy_into(&mut data, w as usize * 4)?;
                held.waiting = Some(Waiting { data, width: w, height: h, at });
                cv.notify_one();
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

/// The encoder's worker: each frame made the receiver's size, encoded
/// and sent, until stopped. An error: no encoder, or the receiver gone.
fn encode(slot: &Slot, mirror: &Mirror, screen: Screen, choice: olo_video::Choice, fps_asked: Option<u32>, fps: &AtomicU32, trace: bool) -> Result<(), String> {
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
        let frame = {
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
            let e = olo_video::Encoder::new(vw, vh, wanted, choice, olo_video::Codec::H264)?;
            // (The processor keeps up with fewer.)
            let rate = if e.choice() == olo_video::Choice::Cpu { fps_asked.unwrap_or(30).min(wanted) } else { wanted };
            fps.store(rate, SeqCst);
            eprintln!("olo-airplay: {vw}×{vh} at up to {rate} a second, encoded on {}", e.name());
            encoder = Some(e);
        }
        let e = encoder.as_mut().expect("made above");
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
                let median = |i: usize| {
                    let mut v: Vec<f64> = t.iter().map(|s| s[i] * 1000.0).collect();
                    v.sort_by(f64::total_cmp);
                    (v[v.len() / 2], v[v.len() * 95 / 100])
                };
                let (r, c, l) = (median(0), median(1), median(2));
                eprintln!("  frames: resize {:.1} ms (95%: {:.1}), encode {:.1} ms ({:.1}), captured to sent {:.1} ms ({:.1})", r.0, r.1, c.0, c.1, l.0, l.1);
                t.clear();
            }
        }
        // (Its buffer back, for the next frame.)
        slot.0.lock().unwrap_or_else(|e| e.into_inner()).spare = Some(frame.data);
    }
}
