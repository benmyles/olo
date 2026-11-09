//! `olo-voice`: the voice processing service, and its command line.
//!
//! - `olo-voice --service`: run it (olo-session does, in a real session).
//! - `olo-voice --status [--json]`: what it's doing and its settings.
//! - `olo-voice --set KEY=VALUE…`: change a setting: `enabled`, `echo`,
//!   `normalize` (on/off), `mode` (calls/recording), `noise`
//!   (off/moderate/strong/full), `model` (deepfilternet3/low-delay).

mod engine;

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use olo_protocol::voice::{Mode, Model, NODE, Noise, Settings, State};
use olo_system::audio::{self, Devices, Kind, Volume};

fn main() -> ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--service") => service(),
        Some("--status") => {
            let (state, settings) = (State::load(), Settings::load());
            if args.iter().any(|a| a == "--json") {
                let v = serde_json::json!({ "state": state, "settings": settings });
                println!("{v}");
            } else {
                print_status(&state, &settings);
            }
            ExitCode::SUCCESS
        }
        Some("--set") => match set(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("olo-voice: {e}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!(
                "usage: olo-voice --service | --status [--json] | --set KEY=VALUE… (enabled, echo, normalize: on/off; mode: calls/recording; noise: off/moderate/strong/full; model: deepfilternet3/low-delay)"
            );
            ExitCode::from(2)
        }
    }
}

fn print_status(state: &State, settings: &Settings) {
    let on = |b: bool| if b { "on" } else { "off" };
    println!("voice processing: {}", on(settings.enabled));
    println!(
        "  mode: {}, noise: {} ({}), echo cancellation: {}, level: {}",
        settings.mode.label(),
        settings.noise.label(),
        settings.model.label(),
        on(settings.echo_cancellation),
        on(settings.normalize)
    );
    if state.pid == 0 {
        println!("  service: not running");
        return;
    }
    if let Some(e) = &state.error {
        println!("  error: {e}");
    }
    if let Some(m) = &state.microphone {
        println!("  listening to: {} ({m})", state.microphone_label.as_deref().unwrap_or(m));
    }
    println!(
        "  {}; echo cancelling now: {}; adds {} ms; a frame takes {} µs; dropouts: {}",
        if state.active { "recording" } else { "idle" },
        on(state.echo_cancelling),
        state.latency_ms,
        state.frame_us,
        state.dropouts
    );
}

fn set(pairs: &[String]) -> Result<(), String> {
    let mut s = Settings::load();
    for pair in pairs {
        let (k, v) = pair.split_once('=').ok_or_else(|| format!("{pair}: KEY=VALUE"))?;
        let flag = || match v {
            "on" | "true" | "yes" | "1" => Ok(true),
            "off" | "false" | "no" | "0" => Ok(false),
            _ => Err(format!("{k}: on or off")),
        };
        match k {
            "enabled" => s.enabled = flag()?,
            "echo" => s.echo_cancellation = flag()?,
            "normalize" => s.normalize = flag()?,
            "mode" => s.mode = Mode::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(v)).ok_or("mode: calls or recording")?,
            "noise" => {
                s.noise = match v {
                    "voice-only" | "full" => Noise::Full,
                    _ => Noise::ALL.into_iter().find(|n| n.label().eq_ignore_ascii_case(v)).ok_or("noise: off, moderate, strong or full")?,
                }
            }
            "model" => {
                s.model = match v {
                    "ll" | "deepfilternet3-ll" => Model::LowDelay,
                    _ => Model::ALL.into_iter().find(|m| m.word() == v || m.label().eq_ignore_ascii_case(v)).ok_or("model: deepfilternet3 or low-delay")?,
                }
            }
            _ => return Err(format!("{k}: enabled, echo, normalize, mode, noise or model")),
        }
    }
    s.save().map_err(|e| e.to_string())
}

enum Event {
    Devices(Option<Devices>),
    Settings,
    Engine,
    Quit,
}

fn service() -> ExitCode {
    // Olo's audio tier, before any thread starts (they inherit it,
    // PipeWire's too; its own are made realtime besides).
    if let Err(e) = olo_system::priority::raise_self(olo_protocol::performance::AUDIO_NICE) {
        tracing::info!("voice: can't raise its priority: {e}");
    }
    let (tx, rx) = mpsc::channel();
    on_signals(tx.clone());
    std::thread::Builder::new()
        .name("devices".into())
        .spawn({
            let tx = tx.clone();
            move || audio::watch_devices_raw(move |d| _ = tx.send(Event::Devices(d)))
        })
        .expect("thread");
    let _watch = olo_kit_base::watch::watch([Settings::path()], {
        let tx = std::sync::Mutex::new(tx.clone());
        move |_| _ = tx.lock().unwrap().send(Event::Settings)
    });

    let mut settings = Settings::load();
    let mut devices: Option<Devices> = None;
    // The microphone chosen before a restart or an upgrade (the state file
    // outlives its process).
    let mut microphone: Option<String> = std::fs::read_to_string(State::path()).ok().and_then(|t| serde_json::from_str::<State>(&t).ok()).and_then(|s| s.microphone);
    let mut engine: Option<engine::Engine> = None;
    let mut config: Option<engine::Config> = None;
    let mut state = State { pid: std::process::id(), ..State::default() };
    let mut last_frames = (0u64, 0u64);
    let mut followed: Option<Followed> = None;

    loop {
        // Only while recording is there anything to report as time goes.
        let event = if state.active { rx.recv_timeout(Duration::from_secs(2)).ok() } else { rx.recv().ok().or(Some(Event::Quit)) };
        match event {
            Some(Event::Devices(d)) => devices = d,
            Some(Event::Settings) => settings = Settings::load(),
            Some(Event::Engine) | None => {}
            Some(Event::Quit) => break,
        }

        let Some(d) = &devices else {
            state.running = false;
            state.error = Some("no sound server".into());
            let _ = state.save();
            continue;
        };
        let default_in = d.default_device(Kind::Input).map(|x| x.name.clone());
        let real = |n: &str| n != NODE && d.inputs.iter().any(|i| i.name == n);
        // The person (or something) chose a microphone: that's the one.
        if let Some(def) = default_in.as_deref().filter(|n| real(n)) {
            microphone = Some(def.to_owned());
        }
        if !microphone.as_deref().is_some_and(real) {
            microphone = d.inputs.iter().find(|i| i.name != NODE).map(|i| i.name.clone());
        }
        state.microphone_label = microphone.as_ref().and_then(|m| d.inputs.iter().find(|i| &i.name == m)).map(|i| i.description.clone());
        state.microphone = microphone.clone();

        if settings.enabled && microphone.is_some() {
            let speakers = d.default_device(Kind::Output).filter(|o| !o.private).map(|o| o.name.clone());
            let want = engine::Config {
                microphone: microphone.clone().unwrap_or_default(),
                label: state.microphone_label.clone().unwrap_or_default(),
                speakers: speakers.filter(|_| settings.echo_cancellation),
                settings,
            };
            match &engine {
                None => {
                    let tx = std::sync::Mutex::new(tx.clone());
                    engine = Some(engine::Engine::start(want.clone(), move || _ = tx.lock().unwrap().send(Event::Engine)));
                }
                Some(e) if config.as_ref() != Some(&want) => e.reconfigure(want.clone()),
                Some(_) => {}
            }
            state.echo_cancelling = want.speakers.is_some();
            config = Some(want);
            // Ours, once it's there, is where recordings come from.
            if d.inputs.iter().any(|i| i.name == NODE)
                && default_in.as_deref() != Some(NODE)
                && let Err(e) = audio::set_default(Kind::Input, NODE)
            {
                tracing::warn!("voice: can't make the processed microphone the default: {e}");
            }
            state.running = true;
            state.error = None;
            if let (Some(e), Some(m)) = (&engine, &microphone) {
                follow(&mut followed, d, m, e);
            }
        } else {
            give_back(engine.take(), &default_in, &microphone);
            followed = None;
            config = None;
            state.running = false;
            state.echo_cancelling = false;
            state.error = (settings.enabled && microphone.is_none()).then(|| "no microphone".into());
        }

        if let Some(e) = &engine {
            use std::sync::atomic::Ordering::Relaxed;
            state.active = e.stats.active.load(Relaxed);
            state.dropouts = e.stats.dropouts.load(Relaxed);
            state.latency_ms = (e.stats.latency_samples.load(Relaxed) as u64 * 1000 / 48_000) as u32;
            let now = (e.stats.frames.load(Relaxed), e.stats.frame_ns.load(Relaxed));
            if now.0 > last_frames.0 {
                state.frame_us = ((now.1 - last_frames.1) / (now.0 - last_frames.0) / 1000) as u32;
            }
            last_frames = now;
        } else {
            state.active = false;
        }
        let _ = state.save();
    }

    // Leaving: the microphone goes back to being the default itself.
    let default_in = devices.as_ref().and_then(|d| d.default_device(Kind::Input)).map(|x| x.name.clone());
    give_back(engine.take(), &default_in, &microphone);
    let _ = std::fs::remove_file(State::path());
    ExitCode::SUCCESS
}

/// What the processed source and its microphone were last left as: the
/// microphone's name, and their mute and volume.
#[derive(Debug, Clone, PartialEq)]
struct Followed {
    microphone: String,
    volume: Volume,
}

fn same(a: &Volume, b: &Volume) -> bool {
    a.muted == b.muted && (a.level - b.level).abs() < 0.005
}

/// What the source and the microphone are both to be: the microphone's,
/// unless the source was changed since they were last the same.
fn wanted(followed: Option<&Followed>, microphone: &str, source: &Volume, mic: &Volume) -> Volume {
    match followed {
        // The source just made (WirePlumber puts nothing of its own back:
        // `state.restore-props`), or another microphone: the microphone's.
        Some(f) if f.microphone == microphone => {
            if same(source, &f.volume) {
                // The microphone changed (Olo's controls show it), or nothing.
                *mic
            } else {
                // The source changed (an app's mute, Chrome's own gain
                // control, the microphone key): the microphone takes it.
                *source
            }
        }
        _ => *mic,
    }
}

/// The processed source's mute and volume are its microphone's, both ways
/// (Ben, 2026-11-09: "it just kept telling me that the device was muted
/// and i couldn't change that but in olo prefs it said it was not muted").
/// Olo shows the microphone in the source's place, so a mute or volume of
/// the source's own (an app's, the microphone key's, kept by WirePlumber
/// from one start to the next) was out of sight and out of reach. What's
/// changed on either is now made the other's; and as PipeWire applies the
/// source's volume to what it gives, the engine makes that up (its
/// `Stats::volume`): the voice is as loud as the microphone's volume says,
/// once, not twice.
fn follow(followed: &mut Option<Followed>, d: &Devices, microphone: &str, engine: &engine::Engine) {
    let (Some(source), Some(mic)) = (d.inputs.iter().find(|i| i.name == NODE), d.inputs.iter().find(|i| i.name == microphone)) else { return };
    let want = wanted(followed.as_ref(), microphone, &source.volume, &mic.volume);
    for (device, name) in [(source, NODE), (mic, microphone)] {
        if (device.volume.level - want.level).abs() >= 0.005 {
            if let Err(e) = audio::set_device_volume(Kind::Input, name, want.level, device.channels) {
                tracing::warn!("voice: {name}'s volume: {e}");
            }
        }
        if device.volume.muted != want.muted
            && let Err(e) = audio::set_device_muted(Kind::Input, name, want.muted)
        {
            tracing::warn!("voice: {name}'s mute: {e}");
        }
    }
    if followed.as_ref().is_none_or(|f| f.microphone != microphone || !same(&f.volume, &want)) {
        tracing::info!("voice: the source and {microphone}: {}%{}", (want.level * 100.0).round(), if want.muted { ", muted" } else { "" });
    }
    engine.stats.volume.store(want.level.powi(3).to_bits(), std::sync::atomic::Ordering::Relaxed);
    *followed = Some(Followed { microphone: microphone.to_owned(), volume: want });
}

/// Stop processing; if ours was the default, the microphone is again.
fn give_back(engine: Option<engine::Engine>, default_in: &Option<String>, microphone: &Option<String>) {
    if default_in.as_deref() == Some(NODE)
        && let Some(m) = microphone
    {
        let _ = audio::set_default(Kind::Input, m);
    }
    drop(engine);
}

/// SIGTERM and SIGINT: leave tidily (the microphone given back). SIGURG,
/// after an upgrade (olo-session sends it to the programs it replaced):
/// start again as the new version.
fn on_signals(tx: mpsc::Sender<Event>) {
    static FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn on_signal(sig: libc::c_int) {
        let fd = FD.load(std::sync::atomic::Ordering::Relaxed);
        if fd >= 0 {
            let b = sig as u8;
            // SAFETY: write(2) is async-signal-safe.
            unsafe { libc::write(fd, [b].as_ptr().cast(), 1) };
        }
    }
    let mut fds = [0; 2];
    // SAFETY: a pipe and signal handlers that only write to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        FD.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        for sig in [libc::SIGURG, libc::SIGTERM, libc::SIGINT] {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
    let read = fds[0];
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            loop {
                let mut b = [0u8; 1];
                // SAFETY: reading one byte into our buffer.
                if unsafe { libc::read(read, b.as_mut_ptr().cast(), 1) } != 1 {
                    return;
                }
                if b[0] as i32 != libc::SIGURG {
                    let _ = tx.send(Event::Quit);
                    continue;
                }
                let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
                let Some(path) = exe.to_str().and_then(|p| p.strip_suffix(" (deleted)")) else { continue };
                tracing::info!("voice: upgraded: starting again as the new version");
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(path).args(std::env::args_os().skip(1)).exec();
                tracing::warn!("voice: couldn't start the new version: {e}");
            }
        })
        .expect("thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(level: f32, muted: bool) -> Volume {
        Volume { level, muted }
    }

    /// The microphone's, unless the source was changed since they were
    /// the same; WirePlumber's or a stale state never wins at the start.
    #[test]
    fn the_source_and_its_microphone_are_one() {
        let kept = |level, muted| Followed { microphone: "mic".into(), volume: v(level, muted) };
        // New: the microphone's, whatever the source came up as.
        assert_eq!(wanted(None, "mic", &v(0.69, true), &v(0.76, false)), v(0.76, false));
        // Muted on the source (an app, the microphone key): the microphone too.
        assert_eq!(wanted(Some(&kept(0.76, false)), "mic", &v(0.76, true), &v(0.76, false)), v(0.76, true));
        // Unmuted on the microphone (Preferences, Deskbar): the source too.
        assert_eq!(wanted(Some(&kept(0.76, true)), "mic", &v(0.76, true), &v(0.76, false)), v(0.76, false));
        // Chrome's gain control lowers the source: the microphone's gain.
        assert_eq!(wanted(Some(&kept(0.76, false)), "mic", &v(0.69, false), &v(0.76, false)), v(0.69, false));
        // Another microphone: its own.
        assert_eq!(wanted(Some(&kept(0.76, true)), "airpods", &v(0.76, true), &v(1.0, false)), v(1.0, false));
        // Both as left: nothing changes.
        assert_eq!(wanted(Some(&kept(0.5, false)), "mic", &v(0.5, false), &v(0.5, false)), v(0.5, false));
    }
}
