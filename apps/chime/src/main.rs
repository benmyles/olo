//! olo-chime: Olo's start-up chime as the computer starts, as a Mac's
//! (Ben, 2026-11-13: "i want that changed to the beos-inspired olo
//! startup screen with the olo sound played like how a mac plays the
//! apple sound"), while Olo's start-up screen shows, long before anyone
//! logs in and PipeWire runs.
//!
//! `olo-chime --boot` is olo-chime.service's (data/systemd, installed by
//! Olo Doctor's "Play Olo's Chime as the Computer Starts"; root, started
//! with the first sound card: `WantedBy=sound.target`). Once a boot (the
//! mark `olo_protocol::CHIME_PLAYED`, in /run, which also keeps the
//! session's start-up screen quiet that boot) it:
//!
//! - waits for the computer's built-in sound card (not USB, not only
//!   HDMI or DisplayPort: `built_in`) and for udev to have finished with
//!   it, which is when ALSA has put back its mixer as it last was
//!   (alsactl, from 90-alsa-restore.rules): woken by inotify on udev's
//!   database, 20 seconds at most;
//! - plays nothing if anyone on the computer turned the start-up sound off
//!   (Preferences ▸ Sound: each person's Olo settings, readable by root
//!   at boot); else the chime the one who changed their settings last
//!   chose (`choice`);
//! - plays nothing if the speakers are muted: as the person who used
//!   the sound last left them (their sound server's kept state,
//!   `state::muted`: on a card whose speakers have no volume of their
//!   own, the only place a mute is), or switched off on the card itself
//!   (`speakers_volume`);
//! - switches the card's speakers on as WirePlumber would (its use-case
//!   manager, UCM: the "HiFi" verb, the "Speaker" device: on a laptop like
//!   the ThinkPad P1 the speakers' path is set only then), plays the chime
//!   (`olo_chime::render`) to the speakers' PCM at a quarter of their
//!   full volume whatever the person's volume is (`olo_chime::level`:
//!   the samples themselves made softer, by less where the card's own
//!   mixer is already turned down; the mixer is read, never set), and
//!   switches the speakers off again, leaving the card as it found it
//!   for PipeWire. (It played at the card's own volume: full, on a card
//!   whose volume is the sound server's.)
//!
//! `olo-chime --pcm NAME [olo|swell]` plays it so to ALSA PCM NAME now
//! (a test's `null`; the card PipeWire holds is busy). `olo-chime
//! --which` says which card and speakers it would play to, how far the
//! card's own volume is turned down, whether the speakers are muted and
//! how loud it would play, reading only (the card isn't opened to
//! play, nothing on it is set). `olo-chime --control hw:1 Master` says
//! one playback control of a card as it is now, read the same way.

mod alsa;
mod state;

use std::{
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use olo_chime::cards::{Card, built_in, cards};
use olo_protocol::StartupChime;

/// How long the built-in card is waited for.
const WAIT: Duration = Duration::from_secs(20);

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--boot") => boot(),
        Some("--pcm") => {
            let Some(pcm) = args.get(1) else { return usage() };
            let chime = match args.get(2) {
                Some(c) => match StartupChime::parse(c) {
                    Some(c) => c,
                    None => return usage(),
                },
                None => StartupChime::default(),
            };
            let samples = olo_chime::level::scaled(&olo_chime::render(chime), olo_chime::level::card_gain(0.0));
            match alsa::Alsa::load().and_then(|a| a.pcm(pcm)?.play(&samples)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("olo-chime: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--which") => match built_in(&cards(Path::new("/"))) {
            Some(c) => {
                println!("card {} ({}): {}", c.index, c.id, c.speakers().map_or("no playback but HDMI".into(), |(d, id)| format!("device {d}, {id}")));
                // What its use cases say, read only (no verb set: nothing
                // on the card changes).
                let a = alsa::Alsa::load();
                if let Ok(a) = &a
                    && let Ok(ucm) = a.ucm(&format!("hw:{}", c.index))
                {
                    for verb in ucm.list("_verbs") {
                        let devices = ucm.list(&format!("_devices/{verb}"));
                        println!("use case {verb}: {}", devices.join(", "));
                        if devices.iter().any(|d| d == "Speaker") {
                            println!("  speakers: {}", ucm.get(&format!("PlaybackPCM/Speaker/{verb}")).unwrap_or_default());
                        }
                    }
                }
                // How loud it would play, and whether at all.
                let own = a.as_ref().map(|a| speakers_volume(a, &c)).unwrap_or_default();
                match own.controls.as_slice() {
                    [] => println!("the speakers' volume: the sound server's (the card has none of its own for them)"),
                    controls => {
                        let each: Vec<String> = controls
                            .iter()
                            .map(|(name, k)| format!("{name} {}{}", k.db.map_or("no volume".into(), |d| format!("{:.1} dB down", -d)), if k.on == Some(false) { ", off" } else { "" }))
                            .collect();
                        println!("the speakers' volume: the card's own, {:.1} dB below its loudest ({})", -own.db, each.join("; "));
                    }
                }
                let root = Path::new("/");
                println!("as the sound server names it: {}", state::card_name(root, c.index).unwrap_or_else(|| "unknown".into()));
                if own.off {
                    println!("the speakers are switched off on the card: the chime isn't played");
                } else if state::muted(root, c.index) {
                    println!("the speakers were left muted: the chime isn't played");
                } else {
                    let gain = olo_chime::level::card_gain(own.db);
                    println!("the chime plays {:.1} dB below its own loudness: a quarter of the speakers' full volume", -olo_chime::level::db(gain));
                }
                ExitCode::SUCCESS
            }
            None => {
                println!("no built-in sound card");
                ExitCode::FAILURE
            }
        },
        // A card's playback control as it is now, read (`--control hw:1
        // Master`): what `--which` goes by, for any card.
        Some("--control") => {
            let (Some(card), Some(named)) = (args.get(1), args.get(2)) else { return usage() };
            let (name, index) = control_name(named);
            match alsa::Alsa::load().and_then(|a| Ok(a.mixer(card)?.playback(&name, index))) {
                Ok(Some(c)) => {
                    let volume = c.db.map_or("no volume".into(), |d| format!("{:.1} dB below its loudest", -d));
                    println!(
                        "{name}: {volume}{}",
                        match c.on {
                            Some(true) => ", on",
                            Some(false) => ", off",
                            None => "",
                        }
                    );
                    ExitCode::SUCCESS
                }
                Ok(None) => {
                    println!("{card} has no playback control named {name}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("olo-chime: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: olo-chime --boot | --pcm NAME [olo|swell] | --which | --control CARD NAME");
    ExitCode::from(2)
}

/// As the computer starts (olo-chime.service).
fn boot() -> ExitCode {
    let mark = Path::new(olo_protocol::CHIME_PLAYED);
    if mark.exists() {
        return ExitCode::SUCCESS;
    }
    let Some(chime) = choice(Path::new("/")) else {
        eprintln!("olo-chime: the start-up sound is turned off");
        return ExitCode::SUCCESS;
    };
    let Some(card) = wait_for_card() else {
        eprintln!("olo-chime: no built-in sound card came within {} seconds", WAIT.as_secs());
        return ExitCode::SUCCESS;
    };
    // (Marked first: whatever comes of it, the session's start-up screen
    // stays quiet, and a second start of the service does nothing.)
    if let Some(dir) = mark.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(mark, b"");
    let a = match alsa::Alsa::load() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("olo-chime: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Muted (as the person left the speakers, or on the card itself):
    // nothing is played.
    let own = speakers_volume(&a, &card);
    if own.off || state::muted(Path::new("/"), card.index) {
        eprintln!("olo-chime: the speakers are muted: not played");
        return ExitCode::SUCCESS;
    }
    // A quarter of the speakers' full volume, whatever the person's is.
    let gain = olo_chime::level::card_gain(own.db);
    let samples = olo_chime::level::scaled(&olo_chime::render(chime), gain);
    match play_on(&a, &card, &samples) {
        Ok(pcm) => {
            eprintln!("olo-chime: played on card {} ({}), {pcm}, {:.1} dB below its own loudness (the card's own volume {:.1} dB down)", card.index, card.id, -olo_chime::level::db(gain), -own.db);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("olo-chime: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The speakers' own volume on a card, read.
#[derive(Debug, Default)]
struct Own {
    /// How far below its loudest, decibels (0: at its loudest, or the
    /// card has no volume for them).
    db: f32,
    /// Switched off on the card.
    off: bool,
    /// The controls it is made of, as each is.
    controls: Vec<(String, alsa::Control)>,
}

/// `card`'s speakers' own volume, as its mixer is now (read, never
/// set): the controls its use cases name for the speakers (the one the
/// sound server sets their volume by, and its master), or, on a card
/// without use cases, those a sound server makes the speakers' volume
/// of (Master, Speaker, PCM). None of them: the card has no volume for
/// its speakers (the sound server's is the only one).
fn speakers_volume(a: &alsa::Alsa, card: &Card) -> Own {
    let hw = format!("hw:{}", card.index);
    let mut names: Vec<String> = Vec::new();
    match a.ucm(&hw) {
        Ok(ucm) => {
            let verbs = ucm.list("_verbs");
            if let Some(verb) = verbs.iter().find(|v| *v == "HiFi").or(verbs.first()) {
                names.extend(["PlaybackMixerElem", "PlaybackMasterElem"].iter().filter_map(|what| ucm.get(&format!("{what}/Speaker/{verb}"))).filter(|n| !n.is_empty()));
            }
        }
        Err(_) => names.extend(["Master", "Speaker", "PCM"].map(String::from)),
    }
    names.dedup();
    if names.is_empty() {
        return Own::default();
    }
    let Ok(mixer) = a.mixer(&hw) else { return Own::default() };
    let controls: Vec<(String, alsa::Control)> = names
        .into_iter()
        .filter_map(|n| {
            let (name, index) = control_name(&n);
            Some((n.clone(), mixer.playback(&name, index)?))
        })
        .collect();
    Own { db: controls.iter().filter_map(|(_, c)| c.db).sum(), off: controls.iter().any(|(_, c)| c.on == Some(false)), controls }
}

/// A control as use cases name it: `Speaker`, `Master,1`, `'Speaker
/// Digital',0`.
fn control_name(named: &str) -> (String, u32) {
    let named = named.trim();
    let (name, index) = match named.rsplit_once(',') {
        Some((name, index)) if index.trim().parse::<u32>().is_ok() => (name, index.trim().parse().unwrap_or(0)),
        _ => (named, 0),
    };
    (name.trim().trim_matches(|c| c == '\'' || c == '"').to_owned(), index)
}

/// The chime played to `card`'s speakers: through its use cases where it
/// has them (switched on, played, switched off again), else to its
/// speakers' (or first analog) PCM as its mixer is. The PCM played to.
fn play_on(a: &alsa::Alsa, card: &Card, samples: &[i32]) -> Result<String, String> {
    if let Ok(ucm) = a.ucm(&format!("hw:{}", card.index)) {
        let verbs = ucm.list("_verbs");
        if let Some(verb) = verbs.iter().find(|v| *v == "HiFi").or(verbs.first()) {
            ucm.set("_verb", verb)?;
            let devices = ucm.list(&format!("_devices/{verb}"));
            if let Some(device) = devices.iter().find(|d| *d == "Speaker") {
                let played = ucm.get(&format!("PlaybackPCM/{device}")).ok_or("the speakers have no PCM".to_owned()).and_then(|pcm| {
                    ucm.set("_enadev", device)?;
                    // (Its plug, else as it's named.)
                    let plugged = plug(&pcm);
                    let done = match a.pcm(&plugged) {
                        Ok(p) => p.play(samples).map(|()| plugged),
                        Err(_) => a.pcm(&pcm).and_then(|p| p.play(samples)).map(|()| pcm.clone()),
                    };
                    let _ = ucm.set("_disdev", device);
                    done
                });
                let _ = ucm.set("_verb", "Inactive");
                return played;
            }
            let _ = ucm.set("_verb", "Inactive");
        }
    }
    let (device, _) = card.speakers().ok_or("the card has no speakers' PCM")?;
    let pcm = format!("plughw:{},{device}", card.index);
    a.pcm(&pcm)?.play(samples)?;
    Ok(pcm)
}

/// `hw:CARD,DEV` as `plughw:CARD,DEV` (alsa-lib makes the card's format
/// and channels of the chime's), a use case's own too
/// (`_ucm0001.hw:sofsoundwire,2`: a name in its manager's configuration,
/// as WirePlumber opens it).
fn plug(pcm: &str) -> String {
    if let Some(rest) = pcm.strip_prefix("hw:") {
        return format!("plughw:{rest}");
    }
    match pcm.split_once(".hw:") {
        Some((manager, rest)) => format!("{manager}.plughw:{rest}"),
        None => pcm.to_owned(),
    }
}

/// Whether udev has finished with card `index`'s control device (its
/// database entry is there: ALSA's state put back, which udev runs as it
/// adds it).
fn settled(index: u32) -> bool {
    let Ok(dev) = std::fs::read_to_string(format!("/sys/class/sound/controlC{index}/dev")) else { return false };
    Path::new("/run/udev/data").join(format!("c{}", dev.trim())).exists()
}

/// The built-in card, once it's there and settled: looked for again each
/// time udev's database changes (inotify), `WAIT` at most.
fn wait_for_card() -> Option<Card> {
    let start = Instant::now();
    // SAFETY: plain inotify calls on our own descriptor.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if fd >= 0 {
        let dir = c"/run/udev/data";
        // SAFETY: fd is ours, the path a C string.
        unsafe { libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE) };
    }
    let found = loop {
        if let Some(c) = built_in(&cards(Path::new("/"))).filter(|c| settled(c.index)) {
            break Some(c);
        }
        let left = WAIT.saturating_sub(start.elapsed());
        if left.is_zero() || fd < 0 {
            break None;
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, ours.
        unsafe { libc::poll(&mut p, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        let mut buf = [0u8; 4096];
        // SAFETY: draining our own descriptor into buf.
        while unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    };
    if fd >= 0 {
        // SAFETY: ours.
        unsafe { libc::close(fd) };
    }
    found
}

/// Which chime, if any (`root`: "/"): none if anyone on the computer
/// turned Olo's start-up sound off; else the chime chosen in the settings
/// changed last (Olo's own, with no settings at all). Each person's are
/// their home's (`~/.config/olo/app_server.json`), the people being the
/// accounts from 1000 up in /etc/passwd.
fn choice(root: &Path) -> Option<StartupChime> {
    let mut latest: Option<(std::time::SystemTime, StartupChime)> = None;
    for home in state::homes(root) {
        let path = home.join(".config/olo/app_server.json");
        let Some(settings) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<olo_protocol::Settings>(&t).ok()) else { continue };
        if !settings.startup_sound {
            return None;
        }
        let when = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if latest.is_none_or(|(t, _)| when > t) {
            latest = Some((when, settings.startup_chime));
        }
    }
    Some(latest.map(|(_, c)| c).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(format!("/var/tmp/olo-chime-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// Anyone's "off" is off; else the chime of the settings changed last;
    /// Olo's own with none.
    #[test]
    fn whose_choice() {
        let root = scratch("choice");
        write(&root, "etc/passwd", "root:x:0:0:root:/root:/bin/bash\nben:x:1000:1000::/home/ben:/bin/brush\nsam:x:1001:1001::/home/sam:/bin/bash\n");
        assert_eq!(choice(&root), Some(StartupChime::Olo), "no settings");
        write(&root, "home/sam/.config/olo/app_server.json", r#"{"startup_chime":"swell"}"#);
        assert_eq!(choice(&root), Some(StartupChime::Swell));
        write(&root, "home/ben/.config/olo/app_server.json", r#"{"startup_chime":"olo"}"#);
        assert_eq!(choice(&root), Some(StartupChime::Olo), "ben's changed last");
        write(&root, "home/sam/.config/olo/app_server.json", r#"{"startup_sound":false}"#);
        assert_eq!(choice(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn controls_as_use_cases_name_them() {
        assert_eq!(control_name("Speaker"), ("Speaker".into(), 0));
        assert_eq!(control_name("Master,1"), ("Master".into(), 1));
        assert_eq!(control_name("'Speaker Digital',0"), ("Speaker Digital".into(), 0));
        assert_eq!(control_name("Front, Rear"), ("Front, Rear".into(), 0));
    }

    #[test]
    fn hw_is_played_through_plug() {
        assert_eq!(plug("hw:sofsoundwire,2"), "plughw:sofsoundwire,2");
        assert_eq!(plug("_ucm0001.hw:sofsoundwire,2"), "_ucm0001.plughw:sofsoundwire,2");
        assert_eq!(plug("default"), "default");
    }

    /// The chime through alsa-lib to its null PCM: loaded, opened, set
    /// up, written whole and drained (all but a card).
    #[test]
    fn plays_through_alsa_lib() {
        let a = alsa::Alsa::load().expect("alsa-lib");
        let start = Instant::now();
        let samples = olo_chime::level::scaled(&olo_chime::render(StartupChime::Olo), olo_chime::level::card_gain(0.0));
        a.pcm("null").expect("null PCM").play(&samples).expect("played");
        // (The null PCM takes it as fast as it's written.)
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(a.pcm("hw:99,0").is_err(), "no such card");
    }
}
