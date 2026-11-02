//! olo-airpods: AirPods, natively. `--service` is the session's service
//! (see service.rs); everything else asks it, for people and agents
//! alike, on its socket.

mod keys;
mod service;

use std::{process::ExitCode, time::Duration};

use olo_protocol::airpods::{self as ap, AirPodsState, BudMicrophone, Change, ListeningMode, Request};

const USAGE: &str = "usage: olo-airpods [--address AA:BB:CC:DD:EE:FF] [COMMAND]
  (none), --status [--json]   the AirPods paired: connected, levels, where
                              the buds are, their settings and sound
  --mode off|transparency|adaptive|anc
  --set SETTING VALUE         conversation-awareness, personalized-volume,
                              ear-detection, one-bud-noise-cancellation,
                              volume-swipe, case-sounds: on|off
                              adaptive-strength: 0-100 (more … less let in)
                              microphone: automatic|left|right
                              press-speed, hold-duration: 0|1|2
                              press-and-hold: MODE,MODE[,…]
  --rename NAME
  --connect, --disconnect
  --quality music|calls       music quality, or the headset profile
                              (call quality, with their microphone)
  --service                   run the service (the session starts it)";

fn main() -> ExitCode {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--service") {
        upgrade_on_signal();
        service::run();
    }
    let address = args.iter().position(|a| a == "--address").map(|i| {
        args.remove(i);
        if i < args.len() { args.remove(i) } else { String::new() }
    });
    let json = args.iter().any(|a| a == "--json");
    args.retain(|a| a != "--json");
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let request = match arg(0).as_str() {
        "" | "--status" => return status(json),
        "--mode" => match ListeningMode::from_word(&arg(1)) {
            Some(m) => Request::Set { address, change: Change::ListeningMode(m) },
            None => return usage(),
        },
        "--set" => match setting(&arg(1), &arg(2)) {
            Some(change) => Request::Set { address, change },
            None => return usage(),
        },
        "--rename" if !arg(1).is_empty() => Request::Set { address, change: Change::Name(args[1..].join(" ")) },
        "--connect" => Request::Connect { address },
        "--disconnect" => Request::Disconnect { address },
        "--quality" => match arg(1).as_str() {
            "music" => Request::Quality { address, high: true },
            "calls" => Request::Quality { address, high: false },
            _ => return usage(),
        },
        _ => return usage(),
    };
    match ap::ask(&request, Duration::from_secs(20)) {
        Ok(r) if r.ok => ExitCode::SUCCESS,
        Ok(r) => {
            eprintln!("olo-airpods: {}", r.error.unwrap_or_default());
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("olo-airpods: the AirPods service isn't running ({e})");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn on_off(v: &str) -> Option<bool> {
    match v {
        "on" | "yes" | "true" | "1" => Some(true),
        "off" | "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn setting(name: &str, value: &str) -> Option<Change> {
    Some(match name {
        "conversation-awareness" => Change::ConversationAwareness(on_off(value)?),
        "personalized-volume" => Change::PersonalizedVolume(on_off(value)?),
        "ear-detection" => Change::EarDetection(on_off(value)?),
        "one-bud-noise-cancellation" => Change::OneBudNoiseCancellation(on_off(value)?),
        "volume-swipe" => Change::VolumeSwipe(on_off(value)?),
        "case-sounds" => Change::CaseSounds(on_off(value)?),
        "adaptive-strength" => Change::AdaptiveStrength(value.parse().ok().filter(|v| *v <= 100)?),
        "microphone" => Change::Microphone(match value {
            "automatic" | "auto" => BudMicrophone::Automatic,
            "left" => BudMicrophone::Left,
            "right" => BudMicrophone::Right,
            _ => return None,
        }),
        "press-speed" => Change::PressSpeed(value.parse().ok().filter(|v| *v <= 2)?),
        "hold-duration" => Change::HoldDuration(value.parse().ok().filter(|v| *v <= 2)?),
        "press-and-hold" => Change::PressAndHoldModes(value.split(',').map(ListeningMode::from_word).collect::<Option<Vec<_>>>()?),
        _ => return None,
    })
}

fn status(json: bool) -> ExitCode {
    let state = match ap::ask(&Request::Status, Duration::from_secs(5)) {
        Ok(r) => r.state.unwrap_or_default(),
        // Not running: what it last published, if anything.
        Err(_) => AirPodsState::load(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&state).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    if state.devices.is_empty() {
        println!("No AirPods are paired with this computer.");
    }
    for d in &state.devices {
        let conn = match (d.connected, d.linked) {
            (true, true) => "connected",
            (true, false) => "connected (not talking yet)",
            _ => "not connected",
        };
        println!("{} ({}, {}): {conn}", d.name, d.model, d.address);
        if !d.firmware.is_empty() {
            println!("  model {}, firmware {}, serial {}", d.model_number, d.firmware, d.serial);
        }
        if d.connected {
            let levels = d.levels();
            if !levels.is_empty() {
                println!("  {levels}");
            }
            if d.abilities.buds {
                println!("  in ears: {}, in case: {}{}", d.in_ear, d.in_case, if d.speaking { ", speaking" } else { "" });
            }
            let s = &d.settings;
            if let Some(m) = s.listening_mode {
                let hold: Vec<&str> = s.press_and_hold_modes.iter().map(|m| m.label()).collect();
                println!("  listening mode: {} (press and hold: {})", m.label(), hold.join(", "));
            }
            let flag = |name: &str, v: Option<bool>| v.map(|v| format!("{name} {}", if v { "on" } else { "off" }));
            let flags: Vec<String> = [
                flag("conversation awareness", s.conversation_awareness),
                flag("personalized volume", s.personalized_volume),
                flag("ear detection", s.ear_detection),
                flag("one-bud noise cancellation", s.one_bud_noise_cancellation),
                flag("volume swipe", s.volume_swipe),
                flag("case sounds", s.case_sounds),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !flags.is_empty() {
                println!("  {}", flags.join(", "));
            }
            if !d.sound.output.is_empty() || !d.sound.input.is_empty() {
                let q = if d.sound.high_quality { "music quality" } else { "call quality" };
                println!("  sound: {} ({q})", d.sound.codec.to_uppercase());
            }
        }
    }
    ExitCode::SUCCESS
}

/// After an upgrade olo-session sends SIGURG to the programs it replaced:
/// the service starts again as the new version (the AirPods' channel
/// opens again; nothing else is kept).
fn upgrade_on_signal() {
    static FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn on_signal(_: libc::c_int) {
        let fd = FD.load(std::sync::atomic::Ordering::Relaxed);
        if fd >= 0 {
            // SAFETY: write(2) is async-signal-safe.
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }
    let mut fds = [0; 2];
    // SAFETY: a pipe and a signal handler that only writes to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        FD.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        libc::signal(libc::SIGURG, on_signal as *const () as libc::sighandler_t);
    }
    let read = fds[0];
    std::thread::Builder::new()
        .name("upgrade".into())
        .spawn(move || {
            loop {
                let mut b = [0u8; 1];
                // SAFETY: reading one byte into our buffer.
                if unsafe { libc::read(read, b.as_mut_ptr().cast(), 1) } != 1 {
                    return;
                }
                let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
                let Some(path) = exe.to_str().and_then(|p| p.strip_suffix(" (deleted)")) else { continue };
                tracing::info!("airpods: upgraded: starting again as the new version");
                let _ = std::fs::remove_file(AirPodsState::socket());
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(path).args(std::env::args_os().skip(1)).exec();
                tracing::warn!("airpods: couldn't start the new version: {e}");
            }
        })
        .expect("thread");
}
