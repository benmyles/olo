//! olo-fans: the fans of a ThinkPad run ahead of the heat, so the
//! computer isn't slowed down for it. The firmware's own curve (`auto`)
//! waits for the processor to be hot before the fans speed up, and they
//! fall behind: the board's own sensors pass the temperature at which the
//! firmware throttles the processor (their zones' passive trip, 65 °C on
//! a P1) and it is slowed until they cool. This raises the fans earlier,
//! by three things: how near each sensor is to where it would be
//! throttled, how fast it is rising (looking a few seconds ahead), and
//! how much power the processor is drawing (heat that hasn't reached a
//! sensor yet).
//!
//! It runs as root (systemd's `olo-fans.service`), as the fans are
//! root's to set: `/proc/acpi/ibm/fan` of thinkpad_acpi, loaded with
//! `fan_control=1`. `/etc/olo/fans` holds the profile, one word:
//!
//! - `auto`: the firmware's own curve. Nothing is read or written:
//!   asleep until the file changes.
//! - `cool`: the firmware's curve while everything is cool, the fans
//!   raised early as anything warms, up to full speed.
//!
//! Every sensor the computer has is watched (`sensors`): each core and
//! the package (coretemp), the firmware's own (thinkpad: CPU, the
//! discrete GPU, the board's), every thermal zone with a trip point (the
//! board's SEN zones, whose passive trip is the throttling), the SSD and
//! the memory. Each has where the fans start for it and where they are
//! at full speed (`Sensor::span`), set from its own limit; the worst one
//! sets the level.
//!
//! Safe by the firmware's own watchdog: while it holds a level it says
//! so every few seconds, and if it stops (killed, hung) the firmware
//! takes the fans back to `auto` within [`WATCHDOG`] seconds. Ending, it
//! hands them back itself.
//!
//! It never ends for something it can't do. Where the driver doesn't
//! take levels (loaded without `fan_control=1`: every write is refused,
//! "Invalid argument"), the fans stay the firmware's, it says so, and it
//! puts the file that turns levels on where the driver's next loading
//! reads it (`/etc/modprobe.d/olo-fans.conf`), so the computer's next
//! start has them; it looks again once a minute, for a driver loaded
//! again by hand. A level the fans didn't take is said and tried again
//! half a minute later. (Ben's P1, 2026-11-12: levels had been turned on
//! by loading the driver again by hand, which a restart undid. The
//! service ended at its first write, twice, came up a third time into a
//! cool moment, and sat there, running and useless, its status never
//! written: Activity Monitor said it wasn't running.)
//!
//! `olo-fans --status` prints what it reads and would do, changing
//! nothing (as anyone). `/run/olo-fans/status` says what the service is
//! doing, a line of `key=value`: there from its start to its end,
//! written again when the level or the profile changes.

use std::{
    fs::File,
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FAN: &str = "/proc/acpi/ibm/fan";
const PROFILE: &str = "/etc/olo/fans";
const STATUS: &str = "/run/olo-fans/status";
/// Whether thinkpad_acpi takes levels (`Y`): its `fan_control`, set as
/// it's loaded and not after.
const CONTROL: &str = "/sys/module/thinkpad_acpi/parameters/fan_control";
/// What tells it to, at its next loading (olo_system::fans::CONF and
/// Olo Doctor's helper write the same).
const MODPROBE: &str = "/etc/modprobe.d/olo-fans.conf";
const CONF: &str = "# Olo's fan profile (Preferences > Power): the fans' level may be set.\noptions thinkpad_acpi fan_control=1\n";
/// How long after a level the fans didn't take before another is tried.
const AGAIN: Duration = Duration::from_secs(30);
/// How often a driver that doesn't take levels is looked at again.
const LOOK_AGAIN: Duration = Duration::from_secs(60);
/// Seconds without a word from us after which the firmware takes the
/// fans back.
const WATCHDOG: u32 = 30;
/// How often a held level is said again (the watchdog's food).
const KEEP: Duration = Duration::from_secs(8);
/// A level is held this long before the fans come down a step, and
/// twice as long when they came up again soon after last coming down
/// (hunting between two levels under a steady load: heard as the fans
/// never settling; 286 changes in an hour of builds, at 15 seconds and
/// 3 degrees).
const HOLD: Duration = Duration::from_secs(30);
/// Coming up within this of coming down is hunting.
const SOON: Duration = Duration::from_secs(90);
/// Coming down needs this much room under what brought them up (°C).
const HYSTERESIS: f32 = 6.0;
/// How far ahead a rising temperature is looked (seconds), and the most
/// that adds (°C).
const AHEAD: f32 = 5.0;
const AHEAD_MOST: f32 = 12.0;
/// How often each sensor is read. Reading isn't free: a core's wakes
/// that core if it sleeps, the board's are asked of the embedded
/// controller, the SSD's over NVMe. The package (which is the hottest
/// core, as the processor says it) and the GPU every time round; the
/// cores one by one and the board, which warm slowly, every few
/// seconds; the parts seldom.
const OFTEN: Duration = Duration::from_secs(1);
const SOMETIMES: Duration = Duration::from_secs(3);
const SELDOM: Duration = Duration::from_secs(8);
/// The most looking ahead adds for the processor's own sensors, which
/// jump by ten degrees in a second of work and fall back as fast: its
/// power says what's coming better (`level_for_power`).
const AHEAD_MOST_CPU: f32 = 4.0;

/// A fan level: 0 is the firmware's own (`auto`), 1 to 7 its steps, 8
/// full speed.
type Level = u8;
const AUTO: Level = 0;
const FULL: Level = 8;
/// The lowest step worth setting by hand (below it `auto` does as well).
const FIRST: Level = 3;

fn level_word(l: Level) -> String {
    match l {
        AUTO => "auto".into(),
        FULL => "full-speed".into(),
        n => n.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Profile {
    Auto,
    Cool,
}

fn profile() -> Profile {
    match std::fs::read_to_string(PROFILE).unwrap_or_default().trim() {
        "cool" => Profile::Cool,
        _ => Profile::Auto,
    }
}

/// What kind of thing a sensor is on, which says where the fans start
/// and are at full speed for it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// The processor (its package, a core, the firmware's reading of
    /// it): hot fast, cool fast, throttled at its limit (100 °C).
    Cpu,
    /// The discrete GPU as the firmware reads it.
    Gpu,
    /// A zone whose passive trip throttles the processor (the board's
    /// sensors): slow to warm and slow to cool, so cooled early.
    Board,
    /// Something with a limit of its own (the SSD, the memory).
    Part,
}

struct Sensor {
    name: String,
    kind: Kind,
    /// Where it is throttled or in trouble (°C).
    limit: f32,
    file: File,
    every: Duration,
    /// What it last read, and when it's next read.
    reading: Option<f32>,
    due: Instant,
    /// The last reading and when, and how fast it was rising (°C/s,
    /// smoothed).
    last: Option<(f32, Instant)>,
    rise: f32,
}

impl Sensor {
    /// Where the fans start for it and where they're at full speed (°C).
    fn span(&self) -> (f32, f32) {
        span(self.kind, self.limit)
    }

    /// Its temperature now (None: not there now, or nonsense), its rise
    /// brought up to date.
    fn read(&mut self, now: Instant) -> Option<f32> {
        self.due = now + self.every - Duration::from_millis(100);
        self.reading = None;
        let t = read_number(&mut self.file)? / 1000.0;
        if !(5.0..=130.0).contains(&t) {
            return None;
        }
        self.reading = Some(t);
        if let Some((before, at)) = self.last {
            let dt = now.duration_since(at).as_secs_f32();
            if dt > 0.2 {
                let rise = (t - before) / dt;
                self.rise = self.rise * 0.5 + rise * 0.5;
                self.last = Some((t, now));
            }
        } else {
            self.last = Some((t, now));
        }
        Some(t)
    }
}

fn span(kind: Kind, limit: f32) -> (f32, f32) {
    match kind {
        Kind::Cpu => (limit - 45.0, limit - 10.0),
        Kind::Gpu => (limit - 32.0, limit - 6.0),
        Kind::Board => (limit - 15.0, limit - 4.0),
        Kind::Part => (limit - 30.0, limit - 8.0),
    }
}

/// The level a temperature asks for, between where the fans start for
/// its sensor and where they're at full speed.
fn level_for(t: f32, (start, full): (f32, f32)) -> Level {
    if t < start {
        return AUTO;
    }
    let f = (t - start) / (full - start).max(1.0);
    let steps = (FULL - FIRST) as f32;
    (FIRST as f32 + (f * steps).floor()).min(FULL as f32) as Level
}

/// The level the processor's power asks for by itself (watts, smoothed):
/// heat on its way that no sensor shows yet.
fn level_for_power(watts: f32) -> Level {
    match watts {
        w if w >= 80.0 => 7,
        w if w >= 60.0 => 6,
        w if w >= 45.0 => 5,
        w if w >= 30.0 => 4,
        _ => AUTO,
    }
}

fn read_number(f: &mut File) -> Option<f32> {
    let mut buf = [0u8; 32];
    f.seek(std::io::SeekFrom::Start(0)).ok()?;
    let n = f.read(&mut buf).ok()?;
    std::str::from_utf8(&buf[..n]).ok()?.trim().parse().ok()
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

fn millis(p: &Path) -> Option<f32> {
    read_trim(p)?.parse::<f32>().ok().map(|v| v / 1000.0)
}

fn entries(dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

/// Every sensor worth watching, each with its limit.
fn sensors() -> Vec<Sensor> {
    let mut out = Vec::new();
    let now = Instant::now();
    let mut add = |name: String, kind: Kind, limit: f32, path: PathBuf, every: Duration| {
        if let Ok(file) = File::open(&path) {
            out.push(Sensor { name, kind, limit, file, every, reading: None, due: now, last: None, rise: 0.0 });
        }
    };
    for h in entries("/sys/class/hwmon") {
        let Some(chip) = read_trim(&h.join("name")) else { continue };
        for input in entries(h.to_str().unwrap_or_default()) {
            let Some(base) = input.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix("_input")).filter(|n| n.starts_with("temp")) else { continue };
            let label = read_trim(&h.join(format!("{base}_label"))).unwrap_or_default();
            let crit = millis(&h.join(format!("{base}_crit"))).filter(|c| (40.0..=125.0).contains(c));
            let name = if label.is_empty() { format!("{chip} {base}") } else { format!("{chip} {label}") };
            match (chip.as_str(), label.as_str()) {
                ("coretemp", l) => add(name, Kind::Cpu, crit.unwrap_or(100.0), input, if l.starts_with("Package") { OFTEN } else { SOMETIMES }),
                ("thinkpad", "GPU") => add(name, Kind::Gpu, 87.0, input, OFTEN),
                // (Its CPU is the package again, asked of the embedded
                // controller; the board's own are the SEN zones, taken
                // below with their trips.)
                ("thinkpad", _) => {}
                // (Its limit is a zone's, below.)
                ("acpitz", _) => {}
                // Whatever else says where it is in trouble: the SSD, the
                // memory, a GPU with a driver that says.
                (chip, _) => {
                    if let Some(crit) = crit {
                        let gpu = matches!(chip, "amdgpu" | "nouveau" | "nvidia" | "xe" | "i915");
                        add(name, if gpu { Kind::Gpu } else { Kind::Part }, crit, input, if gpu { OFTEN } else { SELDOM });
                    }
                }
            }
        }
    }
    for z in entries("/sys/class/thermal") {
        if !z.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("thermal_zone")) {
            continue;
        }
        let Some(kind_name) = read_trim(&z.join("type")) else { continue };
        // The lowest trip of each kind (°C, real ones only).
        let (mut passive, mut hot) = (None::<f32>, None::<f32>);
        for i in 0..16 {
            let (Some(kind), Some(t)) = (read_trim(&z.join(format!("trip_point_{i}_type"))), millis(&z.join(format!("trip_point_{i}_temp")))) else { break };
            if !(40.0..=125.0).contains(&t) {
                continue;
            }
            let slot = if kind == "passive" { &mut passive } else { &mut hot };
            *slot = Some(slot.map_or(t, |s: f32| s.min(t)));
        }
        let name = format!("zone {kind_name}");
        // (The processor's own zones are its package again, read above.)
        if kind_name.starts_with("TCPU") || kind_name == "x86_pkg_temp" || kind_name == "acpitz" {
            continue;
        }
        match (passive, hot) {
            (Some(p), _) => add(name, Kind::Board, p, z.join("temp"), SOMETIMES),
            (None, Some(h)) => add(name, Kind::Part, h, z.join("temp"), SOMETIMES),
            (None, None) => {}
        }
    }
    out
}

/// The processor's power, from its energy counter (RAPL's package).
struct Power {
    file: Option<File>,
    last: Option<(f64, Instant)>,
    watts: f32,
}

impl Power {
    fn new() -> Power {
        let file = entries("/sys/class/powercap").into_iter().find(|p| read_trim(&p.join("name")).is_some_and(|n| n.starts_with("package"))).and_then(|p| File::open(p.join("energy_uj")).ok());
        Power { file, last: None, watts: 0.0 }
    }

    fn read(&mut self, now: Instant) -> f32 {
        let Some(f) = self.file.as_mut() else { return 0.0 };
        let mut s = String::new();
        if f.seek(std::io::SeekFrom::Start(0)).is_err() || f.read_to_string(&mut s).is_err() {
            return self.watts;
        }
        let Ok(uj) = s.trim().parse::<f64>() else { return self.watts };
        if let Some((before, at)) = self.last {
            let dt = now.duration_since(at).as_secs_f64();
            // (The counter wraps: that sample is skipped.)
            if dt > 0.2 && uj >= before {
                let w = ((uj - before) / 1e6 / dt) as f32;
                self.watts = self.watts * 0.65 + w * 0.35;
            }
        }
        self.last = Some((uj, now));
        self.watts
    }
}

/// What everything read asks of the fans: the level, and what asked for
/// it. `margin` is taken off every temperature's room (coming down
/// needs more room than going up).
fn wanted(sensors: &[Sensor], watts: f32, margin: f32) -> (Level, String) {
    let mut best = (level_for_power(watts + margin * 3.0), format!("{watts:.0} W"));
    for s in sensors {
        let Some(t) = s.reading else { continue };
        let ahead = (s.rise * AHEAD).clamp(0.0, if s.kind == Kind::Cpu { AHEAD_MOST_CPU } else { AHEAD_MOST });
        let l = level_for(t + ahead + margin, s.span());
        if l > best.0 {
            best = (l, format!("{} {t:.0}°{}", s.name, if ahead >= 1.0 { format!(" (+{ahead:.0} rising)") } else { String::new() }));
        }
    }
    best
}

/// The fans, as they were last set.
struct Fans {
    /// thinkpad_acpi's fan file, and where what's being done is said.
    file: PathBuf,
    status: PathBuf,
    level: Level,
    /// When the level last changed, and was last said.
    changed: Instant,
    said: Instant,
    /// When they last came down a step, and whether they came up again
    /// soon after (held longer, then).
    came_down: Option<Instant>,
    hunting: bool,
    /// When they last didn't take a level, and what was said of it (said
    /// once, not every time it's tried).
    refused: Option<Instant>,
    complaint: String,
}

impl Fans {
    fn new(file: &Path, status: &Path, now: Instant) -> Fans {
        Fans { file: file.to_owned(), status: status.to_owned(), level: AUTO, changed: now, said: now, came_down: None, hunting: false, refused: None, complaint: String::new() }
    }

    fn say(&self, command: &str) -> std::io::Result<()> {
        std::fs::OpenOptions::new().write(true).open(&self.file)?.write_all(command.as_bytes())
    }

    /// What the service is doing, for whoever asks (Activity Monitor,
    /// Preferences): the level held and why.
    fn note(&self, why: &str) {
        let _ = std::fs::write(&self.status, format!("level={} why={why}\n", level_word(self.level)));
    }

    fn set(&mut self, level: Level, why: &str, now: Instant) -> std::io::Result<()> {
        if level != AUTO && self.level == AUTO {
            self.say(&format!("watchdog {WATCHDOG}"))?;
        }
        self.say(&format!("level {}", level_word(level)))?;
        if level == AUTO {
            self.say("watchdog 0")?;
        }
        if level != self.level {
            if level < self.level {
                self.came_down = Some(now);
            } else {
                self.hunting = self.came_down.is_some_and(|at| now.duration_since(at) < SOON);
            }
            eprintln!("olo-fans: {} → {} ({why})", level_word(self.level), level_word(level));
            self.level = level;
            self.changed = now;
            self.note(why);
        }
        self.said = now;
        self.refused = None;
        self.complaint.clear();
        Ok(())
    }

    /// The fans didn't take a level (`e`): they're the firmware's (handed
    /// back if they can be; its watchdog takes them within [`WATCHDOG`]
    /// seconds if not), which is said, once for each thing that went
    /// wrong, and the service goes on: tried again [`AGAIN`] later.
    fn refuse(&mut self, wanted: Level, e: &std::io::Error, now: Instant) {
        let _ = self.say("level auto");
        let _ = self.say("watchdog 0");
        self.level = AUTO;
        self.changed = now;
        self.refused = Some(now);
        let complaint = format!("the fans didn't take level {}: {e}", level_word(wanted));
        if complaint != self.complaint {
            eprintln!("olo-fans: {complaint}; they're the firmware's, and tried again every {} s", AGAIN.as_secs());
            self.note(&complaint);
            self.complaint = complaint;
        }
    }
}

/// Whether the driver takes levels at all (`control`: its `fan_control`).
fn takes_levels(control: &Path) -> bool {
    read_trim(control).is_some_and(|v| v == "Y" || v == "1")
}

/// Has the driver told to take levels at its next loading (the
/// computer's next start): `conf` written where it isn't as it should
/// be. True if it was written now.
fn keep_levels(conf: &Path) -> std::io::Result<bool> {
    if std::fs::read_to_string(conf).ok().as_deref() == Some(CONF) {
        return Ok(false);
    }
    if let Some(dir) = conf.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // (Whole or not at all: modprobe reads every file there.)
    let tmp = conf.with_extension("conf.olo-new");
    std::fs::write(&tmp, CONF)?;
    std::fs::rename(&tmp, conf)?;
    Ok(true)
}

/// Waits up to `wait` (None: for good): true if the profile's file may
/// have changed, None if the service is to end.
fn wait_for(inotify: i32, signals: i32, wait: Option<Duration>) -> Option<bool> {
    let mut fds = [libc::pollfd { fd: inotify, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: signals, events: libc::POLLIN, revents: 0 }];
    let ms = wait.map_or(-1, |d| d.as_millis() as i32);
    // SAFETY: poll and read on our own descriptors, into our own buffers.
    unsafe {
        let n = libc::poll(fds.as_mut_ptr(), 2, ms);
        if n <= 0 {
            return Some(false);
        }
        if fds[1].revents != 0 {
            return None;
        }
        let mut buf = [0u8; 4096];
        while libc::read(inotify, buf.as_mut_ptr().cast(), buf.len()) > 0 {}
    }
    Some(true)
}

fn status() {
    let mut list = sensors();
    let now = Instant::now();
    let mut power = Power::new();
    power.read(now);
    for s in &mut list {
        s.read(now);
    }
    std::thread::sleep(Duration::from_secs(1));
    let now = Instant::now();
    let watts = power.read(now);
    for s in &mut list {
        s.read(now);
    }
    println!("profile: {:?} ({PROFILE})", profile());
    println!("fans: {}", std::fs::read_to_string(FAN).unwrap_or_default().lines().take(3).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(", "));
    if let Some(said) = read_trim(Path::new(STATUS)) {
        println!("the service: {said}");
    }
    println!("processor: {}", if power.file.is_some() { format!("{watts:.0} W") } else { "power not readable (root's)".into() });
    for s in &list {
        let (start, full) = s.span();
        match s.reading {
            Some(t) => {
                println!("  {:<28} {t:5.1}°  fans from {start:.0}°, full at {full:.0}° (limit {:.0}°), read every {} s: {}", s.name, s.limit, s.every.as_secs(), level_word(level_for(t, s.span())))
            }
            None => println!("  {:<28} not read", s.name),
        }
    }
    let (level, why) = wanted(&list, watts, 0.0);
    println!("cool would set: {} ({why})", level_word(level));
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--status") {
        status();
        return std::process::ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        println!("usage: olo-fans [--status]\n  (none)    run the service (root; systemd's olo-fans.service)\n  --status  what it reads and would do, changing nothing");
        return std::process::ExitCode::SUCCESS;
    }
    if !Path::new(FAN).exists() {
        eprintln!("olo-fans: no {FAN}: not a ThinkPad, or thinkpad_acpi isn't loaded");
        return std::process::ExitCode::SUCCESS;
    }
    // SAFETY: plain calls with valid arguments; the descriptors are ours.
    let (inotify, signals) = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let signals = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        let inotify = libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK);
        let _ = std::fs::create_dir_all("/etc/olo");
        let dir = std::ffi::CString::new("/etc/olo").unwrap();
        libc::inotify_add_watch(inotify, dir.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_DELETE | libc::IN_CREATE);
        (inotify, signals)
    };
    let _ = std::fs::create_dir_all("/run/olo-fans");
    let mut list = sensors();
    let mut power = Power::new();
    eprintln!("olo-fans: watching {} sensors{}", list.len(), if power.file.is_some() { " and the processor's power" } else { "" });
    let start = Instant::now();
    let mut fans = Fans::new(Path::new(FAN), Path::new(STATUS), start);
    let mut current = None;
    // Whether the driver was last seen not to take levels (said once).
    let mut untaken = false;
    loop {
        let now = Instant::now();
        let p = profile();
        if current != Some(p) {
            eprintln!("olo-fans: profile {p:?}");
            current = Some(p);
            untaken = false;
            // (There from the start: a service that says nothing reads
            // as one that isn't running.)
            fans.note("");
            // (What was read under another profile is stale.)
            for s in &mut list {
                s.last = None;
                s.rise = 0.0;
                s.due = now;
            }
            power.last = None;
        }
        let wait = match p {
            Profile::Auto => {
                if fans.level != AUTO
                    && let Err(e) = fans.set(AUTO, "profile auto", now)
                {
                    // (Its watchdog hands them back.)
                    fans.refuse(AUTO, &e, now);
                }
                None
            }
            // The driver doesn't take levels: nothing can be set until it
            // is loaded again told to (the computer's next start, by the
            // file kept here). Said once, and looked at again now and
            // then, for a driver loaded again by hand.
            Profile::Cool if !takes_levels(Path::new(CONTROL)) => {
                if !untaken {
                    untaken = true;
                    let kept = match keep_levels(Path::new(MODPROBE)) {
                        Ok(true) => format!("{MODPROBE} written: it takes them from the computer's next start"),
                        Ok(false) => "it takes them from the computer's next start".to_owned(),
                        Err(e) => format!("{MODPROBE} couldn't be written ({e}): Olo Doctor repairs it"),
                    };
                    eprintln!("olo-fans: the fans' driver doesn't take levels (thinkpad_acpi's fan_control); {kept}. The fans are the firmware's until then");
                    fans.level = AUTO;
                    fans.note("the fans' driver doesn't take levels until the computer's next start");
                }
                Some(LOOK_AGAIN)
            }
            Profile::Cool => {
                if untaken {
                    untaken = false;
                    eprintln!("olo-fans: the fans' driver takes levels now");
                    fans.note("");
                    for s in &mut list {
                        s.last = None;
                        s.rise = 0.0;
                        s.due = now;
                    }
                    power.last = None;
                }
                for s in &mut list {
                    if now >= s.due {
                        s.read(now);
                    }
                }
                let watts = power.read(now);
                let (up, why) = wanted(&list, watts, 0.0);
                let (down, _) = wanted(&list, watts, HYSTERESIS);
                let mut next = fans.level;
                if up > fans.level {
                    next = up;
                } else if down < fans.level && now.duration_since(fans.changed) >= if fans.hunting { HOLD * 2 } else { HOLD } {
                    // Down a step at a time (from the first step, to the
                    // firmware's own).
                    next = if fans.level <= FIRST { AUTO } else { fans.level - 1 };
                }
                let may = fans.refused.is_none_or(|at| now.duration_since(at) >= AGAIN);
                if may
                    && (next != fans.level || (fans.level != AUTO && now.duration_since(fans.said) >= KEEP))
                    && let Err(e) = fans.set(next, &why, now)
                {
                    fans.refuse(next, &e, now);
                }
                // Looked at every second while anything is warm, every
                // two while all is cool.
                let near = fans.level != AUTO || wanted(&list, watts, 8.0).0 != AUTO;
                Some(Duration::from_secs(if near { 1 } else { 2 }))
            }
        };
        if wait_for(inotify, signals, wait).is_none() {
            break;
        }
    }
    // The fans are the firmware's again.
    let _ = fans.say("level auto");
    let _ = fans.say("watchdog 0");
    let _ = std::fs::remove_file(STATUS);
    eprintln!("olo-fans: ending; the fans are the firmware's again");
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fans start where a sensor's span says and reach full speed at
    /// its end, a step at a time between.
    #[test]
    fn levels_rise_across_a_span() {
        let cpu = span(Kind::Cpu, 100.0);
        assert_eq!(cpu, (55.0, 90.0));
        assert_eq!(level_for(54.0, cpu), AUTO);
        assert_eq!(level_for(55.0, cpu), FIRST);
        assert_eq!(level_for(72.5, cpu), 5);
        assert_eq!(level_for(90.0, cpu), FULL);
        assert_eq!(level_for(120.0, cpu), FULL);
        let mut last = AUTO;
        for t in 40..110 {
            let l = level_for(t as f32, cpu);
            assert!(l >= last);
            last = l;
        }
    }

    /// The board's sensors, throttled at 65°, have the fans at full
    /// speed before they get there.
    #[test]
    fn the_board_is_cooled_before_its_trip() {
        let board = span(Kind::Board, 65.0);
        assert_eq!(level_for(49.0, board), AUTO);
        assert_eq!(level_for(50.0, board), FIRST);
        assert_eq!(level_for(61.0, board), FULL);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(format!("/var/tmp/olo-fans-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A level the fans don't take doesn't end the service: they're the
    /// firmware's, it's said (once), and tried again later; a level
    /// taken after that is as any other.
    #[test]
    fn a_level_not_taken_is_said_and_tried_again() {
        let dir = scratch("refused");
        // (A fan file that can't be written: a folder.)
        let now = Instant::now();
        let mut fans = Fans::new(&dir, &dir.join("status"), now);
        fans.note("");
        assert_eq!(std::fs::read_to_string(dir.join("status")).unwrap(), "level=auto why=\n");
        let e = fans.set(5, "19 W", now).unwrap_err();
        fans.refuse(5, &e, now);
        assert_eq!((fans.level, fans.refused), (AUTO, Some(now)));
        let said = std::fs::read_to_string(dir.join("status")).unwrap();
        assert!(said.starts_with("level=auto why=the fans didn't take level 5: "), "{said}");
        // A fan file that takes what's written.
        std::fs::write(dir.join("fan"), "").unwrap();
        fans.file = dir.join("fan");
        fans.set(5, "19 W", now + AGAIN).unwrap();
        assert_eq!((fans.level, fans.refused), (5, None));
        assert_eq!(std::fs::read_to_string(dir.join("status")).unwrap(), "level=5 why=19 W\n");
        // (A plain file keeps the tail of the longer "watchdog 30" said first.)
        assert!(std::fs::read_to_string(dir.join("fan")).unwrap().starts_with("level 5"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The driver is told to take levels at its next loading, once.
    #[test]
    fn the_driver_is_told_to_take_levels() {
        let dir = scratch("conf");
        let control = dir.join("fan_control");
        assert!(!takes_levels(&control));
        std::fs::write(&control, "N\n").unwrap();
        assert!(!takes_levels(&control));
        std::fs::write(&control, "Y\n").unwrap();
        assert!(takes_levels(&control));
        let conf = dir.join("modprobe.d/olo-fans.conf");
        assert!(keep_levels(&conf).unwrap());
        assert!(std::fs::read_to_string(&conf).unwrap().contains("options thinkpad_acpi fan_control=1"));
        assert!(!keep_levels(&conf).unwrap());
        std::fs::write(&conf, "options thinkpad_acpi fan_control=0\n").unwrap();
        assert!(keep_levels(&conf).unwrap());
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), CONF);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn power_asks_ahead_of_heat() {
        assert_eq!(level_for_power(10.0), AUTO);
        assert_eq!(level_for_power(50.0), 5);
        assert_eq!(level_for_power(110.0), 7);
    }
}
