//! The bridge from Tabula, Olo's name before the rename. A session
//! started on a Tabula build upgrades into Olo in place, and an install
//! from before keeps everything it had; this is what carries the old
//! names over. Kept on purpose (`tabula` is the only place the old name
//! is still spelled, apart from the keyring entries Passwords and the
//! Token Pool look their keys up by, which never change).
//!
//! * [`adopt`], first thing in every program's `main`: `TABULA_*`
//!   environment variables (from a session that started on the old
//!   build) become `OLO_*`, and the settings, cache, state and data
//!   folders move from `…/tabula` to `…/olo`, with a link left behind.
//! * [`runtime_file`]: the sockets, locks and state files in
//!   `$XDG_RUNTIME_DIR` a running Tabula session made keep their names
//!   until it ends, so they are found under either.

use std::path::{Path, PathBuf};

/// The old name, as it was spelled in paths and program names.
pub const OLD: &str = "tabula";

/// Take over what a Tabula session or install left: its environment and
/// its folders. Call it first in `main`, before any thread starts (it
/// changes the environment).
pub fn adopt() {
    adopt_env();
    for base in bases() {
        if let Err(e) = move_dir(&base) {
            eprintln!("olo: could not move {}/{OLD} to {}/olo: {e}", base.display(), base.display());
        }
    }
}

/// `TABULA_X` becomes `OLO_X` (unless that is set already), and goes:
/// a variable meant for this program alone (`OLO_RESTORE`, say) mustn't
/// reach its children under the old name.
fn adopt_env() {
    let old: Vec<_> = std::env::vars_os().filter(|(k, _)| k.to_str().is_some_and(|k| k.starts_with("TABULA_"))).collect();
    for (k, v) in old {
        let Some(rest) = k.to_str().and_then(|k| k.strip_prefix("TABULA_")) else { continue };
        let new = format!("OLO_{rest}");
        // SAFETY: called first in main, before any other thread exists.
        unsafe {
            if std::env::var_os(&new).is_none() {
                std::env::set_var(&new, &v);
            }
            std::env::remove_var(&k);
        }
    }
}

/// `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME`, `$XDG_STATE_HOME` and
/// `$XDG_DATA_HOME` (or their defaults), which hold our folders.
fn bases() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    [("XDG_CONFIG_HOME", ".config"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state"), ("XDG_DATA_HOME", ".local/share")]
        .iter()
        .map(|(var, default)| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(default)))
        .collect()
}

/// Move `base/tabula` to `base/olo` and link the old name to it, so a
/// program still running from before finds its files. Nothing to do once
/// it's moved (or if there's no old folder); two folders are left alone.
fn move_dir(base: &Path) -> std::io::Result<()> {
    let (old, new) = (base.join(OLD), base.join("olo"));
    let real_old = std::fs::symlink_metadata(&old).is_ok_and(|m| m.is_dir());
    if !real_old || std::fs::symlink_metadata(&new).is_ok() {
        return Ok(());
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => {}
        // Another program moved it first.
        Err(_) if std::fs::symlink_metadata(&new).is_ok() => return Ok(()),
        Err(e) => return Err(e),
    }
    match std::os::unix::fs::symlink("olo", &old) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// `dir/name` (`name` begins `olo-`), unless only the Tabula session's
/// `dir/tabula-…` is there: then that, so a running session that started
/// on the old build keeps finding its sockets and files.
pub fn runtime_file(dir: &Path, name: &str) -> PathBuf {
    let new = dir.join(name);
    if std::fs::symlink_metadata(&new).is_ok() {
        return new;
    }
    match name.strip_prefix("olo-").map(|rest| dir.join(format!("{OLD}-{rest}"))) {
        Some(old) if std::fs::symlink_metadata(&old).is_ok() => old,
        _ => new,
    }
}

/// Our program `name` (`olo-…`) under the name it had (`tabula-…`).
pub fn old_program(name: &str) -> Option<String> {
    name.strip_prefix("olo-").map(|rest| format!("{OLD}-{rest}"))
}

/// An app id from before the rename (`tabula.X`) as it is now (`olo.X`);
/// anything else is itself.
pub fn new_app_id(id: &str) -> String {
    match id.strip_prefix(&format!("{OLD}.")) {
        Some(rest) => format!("olo.{rest}"),
        None => id.to_owned(),
    }
}

/// A program name from before the rename, as it is now: `tabula-X` is
/// `olo-X`; anything else is itself.
pub fn new_program(name: &str) -> String {
    match name.strip_prefix(&format!("{OLD}-")) {
        Some(rest) => format!("olo-{rest}"),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("olo-legacy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_old_folder_moves_and_leaves_a_link() {
        let base = scratch("move");
        std::fs::create_dir_all(base.join("tabula/sub")).unwrap();
        std::fs::write(base.join("tabula/sub/x.json"), "{}").unwrap();
        move_dir(&base).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("olo/sub/x.json")).unwrap(), "{}");
        assert_eq!(std::fs::read_link(base.join("tabula")).unwrap(), Path::new("olo"));
        assert_eq!(std::fs::read_to_string(base.join("tabula/sub/x.json")).unwrap(), "{}");
        // Again: nothing to do.
        move_dir(&base).unwrap();
        assert!(base.join("olo/sub/x.json").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn two_folders_are_left_alone() {
        let base = scratch("both");
        std::fs::create_dir_all(base.join("tabula")).unwrap();
        std::fs::create_dir_all(base.join("olo")).unwrap();
        std::fs::write(base.join("tabula/a"), "old").unwrap();
        move_dir(&base).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("tabula/a")).unwrap(), "old");
        assert!(!base.join("olo/a").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn runtime_files_are_found_under_the_old_name() {
        let dir = scratch("run");
        assert_eq!(runtime_file(&dir, "olo-wayland-1.sock"), dir.join("olo-wayland-1.sock"));
        std::fs::write(dir.join("tabula-wayland-1.sock"), "").unwrap();
        assert_eq!(runtime_file(&dir, "olo-wayland-1.sock"), dir.join("tabula-wayland-1.sock"));
        std::fs::write(dir.join("olo-wayland-1.sock"), "").unwrap();
        assert_eq!(runtime_file(&dir, "olo-wayland-1.sock"), dir.join("olo-wayland-1.sock"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn program_names() {
        assert_eq!(new_program("tabula-deskbar"), "olo-deskbar");
        assert_eq!(new_program("olo-deskbar"), "olo-deskbar");
        assert_eq!(new_program("bash"), "bash");
        assert_eq!(old_program("olo-session").as_deref(), Some("tabula-session"));
        assert_eq!(new_app_id("tabula.styled-edit"), "olo.styled-edit");
        assert_eq!(new_app_id("org.gnome.TextEditor"), "org.gnome.TextEditor");
    }
}
