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
//! - switches the card's speakers on as WirePlumber would (its use-case
//!   manager, UCM: the "HiFi" verb, the "Speaker" device: on a laptop like
//!   the ThinkPad P1 the speakers' path is set only then), plays the chime
//!   (`olo_chime::render`) to the speakers' PCM at the volume the
//!   speakers were last set to (the card's own, as ALSA put it back;
//!   silent when they were muted), and switches the speakers off again,
//!   leaving the card as it found it for PipeWire.
//!
//! `olo-chime --pcm NAME [olo|swell]` plays it to ALSA PCM NAME now (a
//! test's `null`; the card PipeWire holds is busy). `olo-chime --which`
//! says which card and speakers it would play to, as /proc says, without
//! opening the card.

mod alsa;

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
            match alsa::Alsa::load().and_then(|a| a.pcm(pcm)?.play(&olo_chime::render(chime))) {
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
                if let Ok(a) = alsa::Alsa::load()
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
                ExitCode::SUCCESS
            }
            None => {
                println!("no built-in sound card");
                ExitCode::FAILURE
            }
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: olo-chime --boot | --pcm NAME [olo|swell] | --which");
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
    let samples = olo_chime::render(chime);
    match alsa::Alsa::load().and_then(|a| play_on(&a, &card, &samples)) {
        Ok(pcm) => {
            eprintln!("olo-chime: played on card {} ({}), {pcm}", card.index, card.id);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("olo-chime: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The chime played to `card`'s speakers: through its use cases where it
/// has them (switched on, played, switched off again), else to its
/// speakers' (or first analog) PCM as its mixer is. The PCM played to.
fn play_on(a: &alsa::Alsa, card: &Card, samples: &[i16]) -> Result<String, String> {
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
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
    let mut latest: Option<(std::time::SystemTime, StartupChime)> = None;
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let (Some(uid), Some(home)) = (f.get(2).and_then(|u| u.parse::<u32>().ok()), f.get(5)) else { continue };
        if !(1000..60000).contains(&uid) {
            continue;
        }
        let path = root.join(home.trim_start_matches('/')).join(".config/olo/app_server.json");
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
        a.pcm("null").expect("null PCM").play(&olo_chime::render(StartupChime::Olo)).expect("played");
        // (The null PCM takes it as fast as it's written.)
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(a.pcm("hw:99,0").is_err(), "no such card");
    }
}
