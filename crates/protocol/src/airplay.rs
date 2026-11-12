//! A film playing on an AirPlay receiver (`olo-airplay play`): each
//! play is a process, and says how it goes in a file of its own,
//! `$XDG_RUNTIME_DIR/olo-airplay/<pid>.json`, there while it plays and
//! written again when what it's doing changes: for Deskbar's tray (which
//! follows the folder), `olo-airplay playing|stop|pause|resume`, and
//! agents. A play is the session's, not a display's: the receiver is in
//! the room, whoever asked.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playing {
    pub pid: u32,
    /// The receiver's name.
    pub receiver: String,
    /// What plays: the file's name, or its address on the web.
    pub title: String,
    /// The file, where it is one on this computer.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// "loading", "playing" or "paused".
    pub state: String,
    /// Where in the film it was, in seconds, when `as_of` (milliseconds
    /// since the epoch) was: a receiver says where it is when that
    /// changes, not as it goes ([`Playing::position_now`]).
    #[serde(default)]
    pub position: Option<f64>,
    #[serde(default)]
    pub duration: Option<f64>,
    /// How fast it plays (1: as filmed).
    #[serde(default)]
    pub rate: f64,
    #[serde(default)]
    pub as_of: u64,
    /// When it was asked for, in seconds since the epoch.
    pub started: u64,
}

impl Playing {
    pub fn paused(&self) -> bool {
        self.state == "paused"
    }

    /// Where in the film it is by now.
    pub fn position_now(&self) -> Option<f64> {
        let at = self.position?;
        let at = if self.state == "playing" && self.as_of > 0 { at + now_ms().saturating_sub(self.as_of) as f64 / 1000.0 * self.rate } else { at };
        Some(self.duration.map_or(at, |all| at.min(all)))
    }

    /// Kept (whole or not at all) where the others find it.
    pub fn write(&self) -> std::io::Result<()> {
        let path = path_of(self.pid);
        std::fs::create_dir_all(dir())?;
        let tmp = path.with_extension("new");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The folder the plays are said in (followed by inotify: a play coming,
/// changing and going are each a change in it).
pub fn dir() -> PathBuf {
    // (A test's plays, to a receiver of its own, are said in a folder of
    // its own: the runtime folder is the person's too, and their Deskbar
    // would show them.)
    if let Some(own) = std::env::var_os("OLO_AIRPLAY_DIR") {
        return PathBuf::from(own);
    }
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    run.join("olo-airplay")
}

pub fn path_of(pid: u32) -> PathBuf {
    dir().join(format!("{pid}.json"))
}

/// The plays going (their processes alive), the earliest first.
pub fn all() -> Vec<Playing> {
    let Ok(entries) = std::fs::read_dir(dir()) else { return Vec::new() };
    let mut all: Vec<Playing> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Playing>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|p| alive(p.pid))
        .collect();
    all.sort_by_key(|p| (p.started, p.pid));
    all
}

/// Whether this process is a play (and not gone, nor another that has
/// its number since): what is signalled, and what counts as playing.
pub fn alive(pid: u32) -> bool {
    std::fs::read_to_string(Path::new(&format!("/proc/{pid}/comm"))).is_ok_and(|name| name.trim() == "olo-airplay")
}

/// The plays on the receiver of this name, or the start of it, in any
/// case (all of them, for no name).
pub fn on(receiver: &str) -> Vec<Playing> {
    let wanted = receiver.to_lowercase();
    all().into_iter().filter(|p| p.receiver.to_lowercase().starts_with(&wanted)).collect()
}

/// Where the pairings are kept (`olo_airplay::kept`: each receiver
/// paired with, and the keys that prove each side to the other). Only
/// the person reads it.
pub fn pairings_path() -> PathBuf {
    olo_xdg::state_dir().join("airplay/pairings.json")
}

/// A receiver paired with, as a menu names it (its keys stay in the
/// file, unread here).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Paired {
    /// Its id as it announces it: the same whatever it's renamed to.
    pub device_id: String,
    pub name: String,
}

impl Paired {
    /// What names it to `olo-airplay`: its id, or (it announced none) its
    /// name.
    pub fn key(&self) -> &str {
        if self.device_id.is_empty() { &self.name } else { &self.device_id }
    }
}

/// The receivers paired with, by name: what "Play On" lists. Read from
/// the file as it is (a small one), no network asked.
pub fn paired() -> Vec<Paired> {
    let mut all: Vec<Paired> = std::fs::read(pairings_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    all.sort_by_key(|p| p.name.to_lowercase());
    all
}

/// One word for a shell, whatever is in it.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The command that plays `what` (a file's path, or an address on the
/// web) on `receiver` ([`Paired::key`], or a name) and tells the person
/// if it couldn't: what an app's "Play On" runs.
pub fn play_command(receiver: &str, what: &str) -> String {
    format!("olo-airplay play --notify --on {} {}", quoted(receiver), quoted(what))
}

/// Start playing `what` on `receiver`: a process of its own
/// (`olo-airplay play`), which outlives the window that asked, shows in
/// Deskbar's tray while it plays, and tells the person in a notification
/// if the receiver wouldn't play it.
pub fn play_on(receiver: &str, what: &str) -> std::io::Result<()> {
    olo_xdg::spawn_detached(&play_command(receiver, what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_asked_for_in_words_a_shell_takes() {
        assert_eq!(play_command("AA:BB:CC:DD:EE:FF", "/home/me/Films/A film.mp4"), "olo-airplay play --notify --on 'AA:BB:CC:DD:EE:FF' '/home/me/Films/A film.mp4'");
        assert_eq!(play_command("Ben's Room", "it's.mp4"), r"olo-airplay play --notify --on 'Ben'\''s Room' 'it'\''s.mp4'");
        let p = Paired { device_id: String::new(), name: "Den".into() };
        assert_eq!((p.key(), Paired { device_id: "AA".into(), ..p.clone() }.key()), ("Den", "AA"));
        // The pairings' file, read for its names: the keys in it aren't.
        let read: Vec<Paired> = serde_json::from_str(r#"[{"device_id":"AA:BB","name":"Front Room","credentials":"aa:bb:cc:dd"}]"#).unwrap();
        assert_eq!(read, [Paired { device_id: "AA:BB".into(), name: "Front Room".into() }]);
    }

    #[test]
    fn where_it_is_by_now() {
        let mut p = Playing {
            pid: 1,
            receiver: "Front Room".into(),
            title: "A film.mp4".into(),
            path: None,
            state: "playing".into(),
            position: Some(10.0),
            duration: Some(600.0),
            rate: 1.0,
            as_of: now_ms() - 5000,
            started: 0,
        };
        let at = p.position_now().unwrap();
        assert!((15.0..16.0).contains(&at), "{at}");
        p.rate = 2.0;
        assert!((20.0..22.0).contains(&p.position_now().unwrap()));
        // Paused, it stays; never past the end; unknown stays unknown.
        p.state = "paused".into();
        assert_eq!((p.position_now(), p.paused()), (Some(10.0), true));
        (p.state, p.position) = ("playing".into(), Some(599.0));
        assert_eq!(p.position_now(), Some(600.0));
        p.position = None;
        assert_eq!(p.position_now(), None);
        // What an older play wrote reads (fields added since have defaults).
        let old: Playing = serde_json::from_str(r#"{"pid":7,"receiver":"Den","title":"x","state":"loading","started":3}"#).unwrap();
        assert_eq!((old.position, old.rate, old.path), (None, 0.0, None));
    }
}
