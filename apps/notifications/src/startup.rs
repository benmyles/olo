//! Olo's start-up sound, played once as a session starts (Notifications
//! starts with it; not when re-executed by an upgrade, nor in a test or
//! nested session), unless turned off (Preferences ▸ Sound, "Play a
//! sound when Olo starts"), or already played as the computer started
//! (`olo-chime`, apps/chime: once a boot). The sound is made by
//! `olo_chime` (crates/chime), written as a WAV in the cache and played
//! at a quarter of the output's full volume whatever the person's volume
//! is, and not at all when the sound is muted (`olo_chime::level`: the
//! chime's own stream is as much louder or softer as their volume is
//! below or above a quarter; their volume isn't touched).
//! `olo-notifications --startup-sound [olo|swell]` plays one so and
//! exits (Preferences ▸ Sound's Play).

use std::path::PathBuf;

use olo_protocol::StartupChime;

/// `chime` as a WAV file in Olo's cache folder (made again each start:
/// a few tens of milliseconds).
pub fn file(chime: StartupChime) -> Option<PathBuf> {
    // (On disk, in the cache: the runtime folder is memory.)
    let dir = olo_xdg::cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("startup.wav");
    std::fs::write(&path, olo_chime::wav(&olo_chime::render(chime))).ok()?;
    Some(path)
}

/// Play it, if this is a real session starting and it isn't turned off:
/// once a session (a mark by the session's pid in the runtime folder, so
/// the service re-executed by an upgrade, or started again after a crash,
/// stays quiet).
pub fn at_start() {
    if !olo_xdg::session::real() {
        return;
    }
    let Some(session) = std::env::var("OLO_SESSION_PID").ok().filter(|p| !p.is_empty()) else { return };
    let mark = olo_xdg::runtime_dir().join(format!("olo-startup-sound-{session}"));
    // (Played as the computer started: not again this boot.)
    if mark.exists() || std::path::Path::new(olo_protocol::CHIME_PLAYED).exists() {
        return;
    }
    let _ = std::fs::write(&mark, b"");
    let settings = olo_kit_base::settings::current();
    if !settings.as_ref().is_none_or(|s| s.startup_sound) {
        return;
    }
    if let Some(path) = file(settings.map(|s| s.startup_chime).unwrap_or_default()) {
        std::thread::spawn(move || play(&path));
    }
}

/// The chime played to its end at its own loudness: a quarter of the
/// output's full volume, not at all muted. The output's volume is lent
/// to it only where it must be (`olo_chime::level::lend`: a card's own
/// volume set too low), and put back when it's done.
fn play(path: &std::path::Path) {
    use olo_preview::audio;
    let lent = match audio::output() {
        Ok(o) => olo_chime::level::lend(o.level, o.muted, o.hardware).and_then(|level| audio::lend_output(level).map_err(|e| tracing::warn!("the start-up sound: {e}")).ok()),
        Err(_) => None,
    };
    crate::sound::play_to_end_at(path, loudness());
    drop(lent);
}

/// How loud the chime's stream plays for an output: so that it comes
/// out at a quarter of full volume; not at all muted.
fn loudness() -> olo_preview::audio::Loudness {
    std::sync::Arc::new(|o: olo_preview::audio::Output| olo_chime::level::stream_level(o.level, o.muted, o.hardware))
}

/// `--startup-sound [olo|swell]`: `chime` played now, to its end
/// (Preferences ▸ Sound's Play, an agent's listen).
pub fn play_now(chime: StartupChime) -> i32 {
    let Some(path) = file(chime) else {
        eprintln!("olo-notifications: the start-up sound couldn't be written");
        return 1;
    };
    play(&path);
    0
}

/// `--startup-level`: how the chime would play now, said and not played
/// (an agent's look; nothing changes): the output's volume, and the
/// chime's own stream's against it.
pub fn say_level() -> i32 {
    match olo_preview::audio::output() {
        Ok(o) => {
            let whose = if o.hardware { "the card's own" } else { "the sound server's" };
            let muted = if o.muted { ", muted" } else { "" };
            println!("the output is at {:.1}%{muted} (its volume is {whose})", o.level * 100.0);
            let lent = olo_chime::level::lend(o.level, o.muted, o.hardware);
            let level = lent.unwrap_or(o.level);
            if let Some(to) = lent {
                println!("the output's volume is lent to the start-up sound: set to {:.1}% while it plays, then put back", to * 100.0);
            }
            match olo_chime::level::stream_level(level, o.muted, o.hardware) {
                Some(at) => println!("the start-up sound's stream plays at {:.1}% of that: it comes out at {:.1}% of full volume", at * 100.0, at * level * 100.0),
                None => println!("the start-up sound isn't played"),
            }
            0
        }
        Err(e) => {
            eprintln!("olo-notifications: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OLO_STARTUP_WAV=/var/tmp/startup.wav cargo test -p olo-notifications startup_wav`:
    /// the sounds, to listen to ("olo" there, the swell beside it with
    /// "-swell" in its name).
    #[test]
    fn startup_wav() {
        let Some(out) = std::env::var_os("OLO_STARTUP_WAV").map(PathBuf::from) else { return };
        std::fs::copy(file(StartupChime::Olo).unwrap(), &out).unwrap();
        let swell = out.with_file_name(format!("{}-swell.wav", out.file_stem().unwrap().to_string_lossy()));
        std::fs::copy(file(StartupChime::Swell).unwrap(), swell).unwrap();
    }
}
