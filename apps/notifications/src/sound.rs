//! Notification sounds: a file, or a name from the freedesktop sound
//! theme (`message-new-instant` → …/sounds/freedesktop/stereo/
//! message-new-instant.oga), played in process through tabula-preview's
//! player (symphonia, and the sound server's own protocol).

use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use crate::server::{Kind, Sound};

/// The sound a note asks for, or ours for its kind: only important ones
/// and errors sound when the app doesn't say.
pub fn for_note(sound: &Sound, kind: Kind) -> Option<PathBuf> {
    match sound {
        Sound::File(p) => p.is_file().then(|| p.clone()),
        Sound::Name(n) => themed(n),
        Sound::Default => match kind {
            Kind::Important => themed("message-new-instant"),
            Kind::Error => themed("dialog-warning"),
            _ => None,
        },
    }
}

/// A sound theme name's file: the freedesktop theme in the data dirs.
pub fn themed(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    for dir in tabula_xdg::xdg_data_dirs() {
        for sub in ["stereo", ""] {
            for ext in ["oga", "ogg", "wav"] {
                let p = dir.join("sounds/freedesktop").join(sub).join(format!("{name}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Play `path` once, off the UI thread (a short sound; gives up after
/// ten seconds).
pub fn play(path: &Path) {
    let input = tabula_preview::input::Input::file(path);
    std::thread::spawn(move || {
        let player = tabula_preview::audio::Player::start(&input);
        let start = Instant::now();
        while !player.state.finished.load(Ordering::Relaxed) && player.state.error.lock().unwrap().is_none() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Let the server drain what it has before the stream goes.
        std::thread::sleep(Duration::from_millis(200));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names() {
        assert_eq!(themed("../etc/passwd"), None);
        assert_eq!(for_note(&Sound::Default, Kind::Information), None);
        // Where the freedesktop theme is installed, the usual names resolve.
        if Path::new("/usr/share/sounds/freedesktop/stereo/bell.oga").exists() {
            assert_eq!(themed("bell").as_deref(), Some(Path::new("/usr/share/sounds/freedesktop/stereo/bell.oga")));
            assert!(for_note(&Sound::Default, Kind::Error).is_some());
        }
    }
}
