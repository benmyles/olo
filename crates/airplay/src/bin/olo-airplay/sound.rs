//! The sound of what's shown, on the TV only, as macOS does it: an output
//! of the showing's own ("Den (AirPlay)", a PipeWire sink made in process
//! by a stream of class `Audio/Sink`), and the sound that goes with what
//! is shown moved to it, so it plays on the TV and not on the computer's
//! speakers too (a moment ahead of the TV's: an echo).
//!
//! What goes: for a window, its app's streams (its process's, and its
//! children's: a browser plays from a helper), whatever output they were
//! on; for a display or a workspace, every stream on the default output,
//! the computer's sound (a stream the person sent elsewhere, a call on
//! headphones, stays there). Streams that start while it's shown are
//! moved as they come (`audio::Streams` follows them, no polling). The
//! default output isn't changed (it's the person's choice, kept by
//! WirePlumber across logins): streams are moved, one by one.
//!
//! When it ends, or is paused, each stream moved goes back where it was:
//! to the output it was on, or, if it followed the default, to the
//! default again (which PipeWire takes as "follow the default"). If the
//! showing ends without a word (killed), its output goes with its
//! process and WirePlumber sends each stream on it to the default:
//! nothing is left playing nowhere.
//!
//! The volume keys set the receiver's own volume (AirPlay's, as macOS
//! does): it is told the default output's volume as it changes, and the
//! samples go as they are, never made quieter here (which would lose
//! their low bits). The TV's output made the default by the person has a
//! volume of its own, which PipeWire applies; the receiver's is left as
//! it was then.
//!
//! The output's samples come to the process callback at 44.1 kHz stereo
//! (PipeWire converts), a cycle at a time (21 ms at most), on the loop's
//! own thread, never the real-time one: they go to the receiver
//! (`Mirror::sound`, which seals and sends them). It runs while showing even with nothing playing
//! (`node.always-process`), so the receiver's sound never has a gap.
//! When PipeWire goes (an upgrade of it), the output is made again once
//! it's back, and the streams found again.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::SeqCst},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use olo_system::audio::Streams;
use pipewire as pw;
use pw::{properties::properties, spa};

/// The receiver's rate (PipeWire converts to it).
pub(crate) const RATE: u32 = 44_100;

/// Whose sound goes to the TV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Whose {
    /// A window's app: its process's streams and its children's.
    Process(u32),
    /// An agent screen's programs (with sound, they play on the
    /// computer's): each started on it says so (`OLO_SCREEN`).
    Screen(String),
    /// The computer's: every stream on the default output.
    Computer,
}

/// Streams moved to the TV's output, each with the output it was on
/// (None: it followed the default).
#[derive(Default)]
struct Moved {
    /// Moving now (not paused, not ended).
    on: bool,
    streams: HashMap<u32, Option<String>>,
}

struct Shared {
    whose: Mutex<Whose>,
    /// The output's node name ("olo-airplay.PID").
    sink: String,
    moved: Mutex<Moved>,
    /// The volume the receiver was last told (0 to 1, as the default
    /// output's slider has it; 0 muted), and how to tell it another.
    volume: Mutex<f32>,
    tell_volume: Box<dyn Fn(f32) + Send + Sync>,
    stop: AtomicBool,
    trace: bool,
}

/// What ends the router's wait for the sound server (its connection
/// shut), once it has one.
type Closer = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

/// Where the output's samples go.
type Got = Box<dyn FnMut(&[f32], Instant) + Send>;

/// The TV's output, while it's shown: its sound there, moved back when
/// dropped.
pub(crate) struct TvSound {
    shared: Arc<Shared>,
    quit: pw::channel::Sender<()>,
    output: Option<JoinHandle<()>>,
    router: Option<JoinHandle<()>>,
    close: Closer,
}

impl TvSound {
    /// The output made, named for the receiver, its samples handed to
    /// `got` (stereo, interleaved, and when the first was played); the
    /// sound of `whose` moved to it; the default output's volume, when
    /// it isn't `volume` (what the receiver was told at the start), told
    /// with `tell_volume`. An error: no PipeWire.
    pub(crate) fn start(
        receiver: &str,
        whose: Whose,
        trace: bool,
        volume: f32,
        tell_volume: impl Fn(f32) + Send + Sync + 'static,
        got: impl FnMut(&[f32], Instant) + Send + 'static,
    ) -> Result<TvSound, String> {
        let sink = format!("olo-airplay.{}", std::process::id());
        let shared = Arc::new(Shared {
            whose: Mutex::new(whose),
            sink,
            moved: Mutex::new(Moved { on: true, ..Moved::default() }),
            volume: Mutex::new(volume),
            tell_volume: Box::new(tell_volume),
            stop: AtomicBool::new(false),
            trace,
        });
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (ready, made) = mpsc::channel();
        let description = format!("{receiver} (AirPlay)");
        let output = {
            let shared = shared.clone();
            std::thread::Builder::new().name("airplay sound".into()).spawn(move || output(&shared, &description, quit_rx, Box::new(got), ready)).map_err(|e| e.to_string())?
        };
        match made.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => {
                let _ = output.join();
                return Err(why);
            }
            Err(_) => {
                shared.stop.store(true, SeqCst);
                let _ = quit.send(());
                return Err("PipeWire didn't make the output".into());
            }
        }
        let close: Closer = Closer::default();
        let router = {
            let (shared, close) = (shared.clone(), close.clone());
            std::thread::Builder::new().name("airplay streams".into()).spawn(move || route(&shared, &close)).map_err(|e| e.to_string())?
        };
        Ok(TvSound { shared, quit, output: Some(output), router: Some(router), close })
    }

    /// Whose sound it is now (a window's app came back as another
    /// process: app_server reloaded, the app was upgraded): its streams
    /// moved too.
    pub(crate) fn follow(&self, whose: Whose) {
        {
            let mut now = self.shared.whose.lock().unwrap_or_else(|e| e.into_inner());
            if *now == whose {
                return;
            }
            *now = whose;
        }
        let mut moved = self.shared.moved.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = Streams::open().and_then(|mut s| pass(&mut s, &self.shared, &mut moved)) {
            eprintln!("olo-airplay: the sound's streams: {e}");
        }
    }

    /// Paused: the sound back where it was (the TV keeps the last
    /// picture, silent); going on: moved to the TV again.
    pub(crate) fn pause(&self, paused: bool) {
        let mut moved = self.shared.moved.lock().unwrap_or_else(|e| e.into_inner());
        moved.on = !paused;
        let r = Streams::open().and_then(|mut s| if paused { restore(&mut s, &self.shared, &mut moved) } else { pass(&mut s, &self.shared, &mut moved) });
        if let Err(e) = r {
            eprintln!("olo-airplay: the sound's streams: {e}");
        }
    }
}

impl Drop for TvSound {
    fn drop(&mut self) {
        self.shared.stop.store(true, SeqCst);
        if let Some(close) = &*self.close.lock().unwrap_or_else(|e| e.into_inner()) {
            close();
        }
        if let Some(router) = self.router.take() {
            router.thread().unpark();
            let _ = router.join();
        }
        // Back where they were, before the output goes.
        self.pause(true);
        let _ = self.quit.send(());
        if let Some(output) = self.output.take() {
            let _ = output.join();
        }
    }
}

/// Whether process `pid` is `ancestor` or one of its descendants.
fn descends(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..64 {
        if pid == ancestor {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        let Some(parent) = parent_of(pid) else { return false };
        pid = parent;
    }
    false
}

/// Whether process `pid` (or one it was started by) runs on agent
/// screen `name`, as its environment says.
fn on_screen(mut pid: u32, name: &str) -> bool {
    let said = format!("OLO_SCREEN={name}");
    for _ in 0..8 {
        if pid <= 1 {
            return false;
        }
        if std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|env| env.split(|b| *b == 0).any(|v| v == said.as_bytes())) {
            return true;
        }
        let Some(parent) = parent_of(pid) else { return false };
        pid = parent;
    }
    false
}

/// A process's parent, as `/proc` says.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // (After the name, which may hold spaces and parentheses: state, ppid.)
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Streams that go to the TV moved there; the rest left alone. The
/// receiver told the default output's volume, if it changed (not the
/// TV's own output's, made the default: PipeWire applies that one).
fn pass(s: &mut Streams, shared: &Shared, moved: &mut Moved) -> olo_system::audio::Result<()> {
    let default = s.default_output()?;
    if default != shared.sink {
        let v = s.default_volume()?;
        let level = if v.muted { 0.0 } else { v.level };
        let mut told = shared.volume.lock().unwrap_or_else(|e| e.into_inner());
        if *told != level {
            *told = level;
            (shared.tell_volume)(level);
            if shared.trace {
                eprintln!("  the receiver's volume: {:.0}%", level * 100.0);
            }
        }
    }
    if !moved.on {
        return Ok(());
    }
    // (Not there yet: PipeWire is making it; told again when it is.)
    let Some(ours) = s.output(&shared.sink)? else { return Ok(()) };
    let outputs = s.outputs()?;
    let default_index = outputs.iter().find(|(_, n)| *n == default).map(|(i, _)| *i);
    let streams = s.list()?;
    moved.streams.retain(|i, _| streams.iter().any(|st| st.index == *i));
    for st in &streams {
        // (Moved already: on the TV, or sent elsewhere by the person since.)
        if st.output == ours || moved.streams.contains_key(&st.index) {
            continue;
        }
        let goes = match &*shared.whose.lock().unwrap_or_else(|e| e.into_inner()) {
            Whose::Process(pid) => st.pid.is_some_and(|p| descends(p, *pid)),
            Whose::Screen(name) => st.pid.is_some_and(|p| on_screen(p, name)),
            Whose::Computer => Some(st.output) == default_index,
        };
        if !goes {
            continue;
        }
        let was = (Some(st.output) != default_index).then(|| outputs.iter().find(|(i, _)| *i == st.output).map(|(_, n)| n.clone())).flatten();
        s.move_to(st.index, &shared.sink)?;
        if shared.trace {
            eprintln!("  {}'s sound (stream {}) moved to the TV", if st.app.is_empty() { "an app" } else { &st.app }, st.index);
        }
        moved.streams.insert(st.index, was);
    }
    Ok(())
}

/// Every stream on the TV's output back where it was (the default, for
/// one that followed it or that the person sent there).
fn restore(s: &mut Streams, shared: &Shared, moved: &mut Moved) -> olo_system::audio::Result<()> {
    let ours = s.output(&shared.sink)?;
    let mut back = std::mem::take(&mut moved.streams);
    let Some(ours) = ours else { return Ok(()) };
    let default = s.default_output()?;
    let outputs = s.outputs()?;
    for st in s.list()?.iter().filter(|st| st.output == ours) {
        let to = back.remove(&st.index).flatten().filter(|n| outputs.iter().any(|(_, o)| o == n)).unwrap_or_else(|| default.clone());
        if to != shared.sink {
            s.move_to(st.index, &to)?;
        }
    }
    Ok(())
}

/// The streams followed until stopped, the sound server reached again
/// whenever it goes.
fn route(shared: &Shared, close: &Closer) {
    while !shared.stop.load(SeqCst) {
        let r: olo_system::audio::Result<()> = (|| {
            let mut s = Streams::open()?;
            *close.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(s.closer()?));
            // (Stopped before it could be closed.)
            if shared.stop.load(SeqCst) {
                return Ok(());
            }
            loop {
                pass(&mut s, shared, &mut shared.moved.lock().unwrap_or_else(|e| e.into_inner()))?;
                s.wait()?;
            }
        })();
        if shared.stop.load(SeqCst) {
            break;
        }
        if let Err(e) = r {
            eprintln!("olo-airplay: the sound server: {e}; trying again");
        }
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

/// The output, on its own thread until told to quit: PipeWire's loop,
/// made again whenever PipeWire goes. `ready` hears once whether it was
/// made.
fn output(shared: &Shared, description: &str, mut quit: pw::channel::Receiver<()>, got: Got, ready: mpsc::Sender<Result<(), String>>) {
    pw::init();
    let got = Rc::new(RefCell::new(got));
    let mut ready = Some(ready);
    while !shared.stop.load(SeqCst) {
        let r = round(shared, description, quit, got.clone(), &mut ready);
        quit = match r {
            Ok(q) => q,
            Err((q, why)) => {
                match ready.take() {
                    Some(ready) => {
                        let _ = ready.send(Err(why));
                        return;
                    }
                    None => eprintln!("olo-airplay: the TV's output: {why}"),
                }
                q
            }
        };
        if !shared.stop.load(SeqCst) {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

type Quit = pw::channel::Receiver<()>;

/// One connection to PipeWire, the output made on it, until told to quit
/// or PipeWire goes.
fn round(shared: &Shared, description: &str, quit: Quit, got: Rc<RefCell<Got>>, ready: &mut Option<mpsc::Sender<Result<(), String>>>) -> Result<Quit, (Quit, String)> {
    let made = (|| -> Result<_, String> {
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
        let core = context.connect_rc(None).map_err(|e| format!("can't reach PipeWire: {e}"))?;
        Ok((mainloop, context, core))
    })();
    let (mainloop, _context, core) = match made {
        Ok(m) => m,
        Err(why) => return Err((quit, why)),
    };
    let weak = mainloop.downgrade();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, _, _, message| {
            if id == pw::core::PW_ID_CORE {
                eprintln!("olo-airplay: PipeWire: {message}");
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
            }
        })
        .register();
    let stream = match pw::stream::StreamBox::new(
        &core,
        description,
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::NODE_NAME => shared.sink.as_str(),
            *pw::keys::NODE_DESCRIPTION => description,
            *pw::keys::AUDIO_CHANNELS => "2",
            "audio.position" => "FL,FR",
            // Its samples come a cycle at a time, every 21 ms at most
            // (the receiver plays them 50 ms or more after they were
            // played here); not less, which would make the whole graph
            // wake more often.
            *pw::keys::NODE_LATENCY => "1024/48000",
            // Run while nothing plays, so the receiver's sound has no gaps.
            "node.always-process" => "true",
            // Never where sound goes by itself (only what's moved to it).
            "priority.session" => "1",
            "priority.driver" => "1",
            "device.icon-name" => "video-display",
        },
    ) {
        Ok(s) => s,
        Err(e) => return Err((quit, e.to_string())),
    };
    let (tx, made) = (ready.take(), Rc::new(RefCell::new(None::<Result<(), String>>)));
    let weak = mainloop.downgrade();
    let said = made.clone();
    let listener = stream
        .add_local_listener_with_user_data(Vec::<f32>::with_capacity(4096))
        .state_changed(move |_, _, _, new| match new {
            pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
                said.borrow_mut().get_or_insert(Ok(()));
            }
            pw::stream::StreamState::Error(e) => {
                said.borrow_mut().get_or_insert(Err(e));
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
            }
            _ => {}
        })
        .process(move |stream, samples| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let data = &mut buffer.datas_mut()[0];
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            let Some(bytes) = data.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else { return };
            samples.clear();
            samples.extend(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
            if samples.is_empty() {
                return;
            }
            let at = Instant::now() - Duration::from_secs_f64(samples.len() as f64 / 2.0 / f64::from(RATE));
            (got.borrow_mut())(samples, at);
        })
        .register();
    let _listener = match listener {
        Ok(l) => l,
        Err(e) => return Err((quit, e.to_string())),
    };
    let format = match pod(RATE) {
        Ok(f) => f,
        Err(why) => return Err((quit, why)),
    };
    let Some(param) = pw::spa::pod::Pod::from_bytes(&format) else { return Err((quit, "format".into())) };
    let mut params = [param];
    if let Err(e) = stream.connect(spa::utils::Direction::Input, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut params) {
        return Err((quit, e.to_string()));
    }
    // Made (or not) before the loop runs for good: said to the starter.
    if let Some(tx) = tx {
        let until = Instant::now() + Duration::from_secs(2);
        while made.borrow().is_none() && Instant::now() < until {
            mainloop.loop_().iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
        }
        let outcome = made.borrow().clone().unwrap_or_else(|| Err("PipeWire didn't make the output".into()));
        let failed = outcome.is_err();
        let _ = tx.send(outcome.clone());
        if failed {
            return Err((quit, outcome.err().unwrap_or_default()));
        }
    }
    let weak = mainloop.downgrade();
    let attached = quit.attach(mainloop.loop_(), move |_| {
        if let Some(l) = weak.upgrade() {
            l.quit();
        }
    });
    if !shared.stop.load(SeqCst) {
        mainloop.run();
    }
    let quit = attached.deattach();
    let _ = stream.disconnect();
    Ok(quit)
}

/// The output's format: 32-bit float stereo at `rate`.
fn pod(rate: u32) -> Result<Vec<u8>, String> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(2);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    Ok(pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object { type_: spa::sys::SPA_TYPE_OBJECT_Format, id: spa::sys::SPA_PARAM_EnumFormat, properties: info.into() }),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process is its own descendant and its children's ancestor, not
    /// another's.
    #[test]
    fn descendants() {
        let me = std::process::id();
        let parent = parent_of(me).expect("a parent");
        assert!(descends(me, me));
        assert!(descends(me, parent));
        assert!(!descends(parent, me));
        assert!(descends(me, 1));
    }

    /// A program started on an agent screen is the screen's, by its
    /// environment; this one isn't.
    #[test]
    fn programs_on_a_screen() {
        let mut child = std::process::Command::new("sleep").arg("5").env("OLO_SCREEN", "test-screen").spawn().expect("sleep");
        let pid = child.id();
        // (Its environment is its own once it has started.)
        let started = Instant::now();
        while !on_screen(pid, "test-screen") && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (on, other, me) = (on_screen(pid, "test-screen"), on_screen(pid, "test"), on_screen(std::process::id(), "test-screen"));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!((on, other, me), (true, false, false));
    }
}
