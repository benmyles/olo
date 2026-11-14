//! Olo's start-up sound, played once as a session starts (Notifications
//! starts with it; not when re-executed by an upgrade, nor in a test or
//! nested session), unless turned off (Preferences ▸ Sound, "Play a
//! sound when Olo starts"), or already played as the computer started
//! (`olo-chime`, apps/chime: once a boot). The sound is made by
//! `olo_chime` (crates/chime), written as a WAV in the cache and played as
//! any sound (`sound::play`). `olo-notifications --startup-sound
//! [olo|swell]` plays one and exits (Preferences ▸ Sound's Play).

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
        crate::sound::play(&path);
    }
}

/// `--startup-sound [olo|swell]`: `chime` played now, to its end
/// (Preferences ▸ Sound's Play, an agent's listen).
pub fn play_now(chime: StartupChime) -> i32 {
    let Some(path) = file(chime) else {
        eprintln!("olo-notifications: the start-up sound couldn't be written");
        return 1;
    };
    crate::sound::play_to_end(&path);
    0
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
