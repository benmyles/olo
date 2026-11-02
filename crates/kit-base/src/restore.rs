//! The state an app hands its new self across an in-place upgrade: the
//! old process writes it to a file in XDG_RUNTIME_DIR and names it in
//! `OLO_RESTORE` for the binary it re-executes as. Both kits claim the
//! file as they start ([`claim`]: read, then removed, asked for or not),
//! so a version that doesn't ask, or asks only on some paths, leaves
//! nothing behind in the runtime directory (memory, on tmpfs).

use olo_sync::Mutex;
use std::{path::PathBuf, process::Command, sync::OnceLock};

/// The variable naming the file.
pub const ENV: &str = "OLO_RESTORE";

fn slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| {
        let text = std::env::var_os(ENV).and_then(|path| {
            // SAFETY: claimed at start-up (the kits' `run`, first thing),
            // before the app starts threads that read the environment.
            unsafe { std::env::remove_var(ENV) };
            let text = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
            text
        });
        Mutex::new(text.filter(|t| !t.is_empty()))
    })
}

/// Takes the file the previous version left, if any, now: kept for
/// [`take`], the file gone.
pub fn claim() {
    slot();
}

/// What the previous version left (once; None on a normal start, or if
/// it left nothing).
pub fn take() -> Option<String> {
    slot().lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Has `cmd` (the binary re-executing as the new version) carry
/// `state`: a file named for this process (whose pid it keeps), unless
/// there is nothing to carry.
pub fn hand_over(cmd: &mut Command, state: Option<&str>) {
    let Some(state) = state.filter(|s| !s.is_empty()) else { return };
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = dir.join(format!("olo-restore-{}.json", std::process::id()));
    if std::fs::write(&file, state).is_ok() {
        cmd.env(ENV, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_carry_writes_nothing() {
        let mut cmd = Command::new("true");
        hand_over(&mut cmd, Some(""));
        hand_over(&mut cmd, None);
        assert!(cmd.get_envs().all(|(k, _)| k != ENV));
    }
}
