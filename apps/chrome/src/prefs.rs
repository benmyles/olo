//! Chrome as an Olo program: its profiles start out with Olo's title
//! bar (Chrome's "Use system title bar and borders": Olo's yellow tab
//! and frame, no buttons of Chrome's own) and the system's colours
//! (Chrome's "GTK" theme, which Olo's GTK themes colour; see
//! `olo_theme::toolkit`).
//!
//! Only once a profile, so a choice made afterwards in Chrome's settings
//! stays: a `.olo-defaults` file in the profile marks it done. Only
//! while Chrome isn't running (it writes its preferences back on exit).
//!
//! On an agent screen a profile starts ready to use: Chrome's first run
//! behind it (its terms of service window and welcome pages, which an
//! agent can't answer for and which stood in its way on each new
//! screen), opening on a new tab without offering to restore tabs. The
//! person's own first Chrome still shows them Google's terms.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const MARK: &str = ".olo-defaults";
/// Chrome's mark in a profile folder that its first run is over.
const FIRST_RUN: &str = "First Run";

/// Chrome's user data directory: `--user-data-dir`, else
/// `$CHROME_CONFIG_HOME` or `$XDG_CONFIG_HOME`/`~/.config`, then
/// `google-chrome`.
pub fn user_data_dir(args: &[String]) -> Option<PathBuf> {
    if let Some(dir) = args.iter().find_map(|a| a.strip_prefix("--user-data-dir=")) {
        return Some(PathBuf::from(dir));
    }
    let config = std::env::var_os("CHROME_CONFIG_HOME")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("google-chrome"))
}

/// Whether a Chrome has `dir` open (its `SingletonLock` is a symlink to
/// `host-pid`): one on this computer that still runs, or one on another
/// (a home folder shared between two), which can't be asked. A lock
/// left by a Chrome that was ended (a screen stopped: its Chrome is
/// killed with it and leaves the lock) is nobody's; taken for a running
/// Chrome, it kept a stopped screen's profile from ever being looked at
/// again (2026-11-26).
fn running(dir: &Path) -> bool {
    let Ok(lock) = std::fs::read_link(dir.join("SingletonLock")) else { return std::fs::symlink_metadata(dir.join("SingletonLock")).is_ok() };
    let here = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    match lock.to_str().and_then(|l| l.rsplit_once('-')) {
        Some((host, pid)) if host == here.trim() => pid.parse::<u32>().is_ok_and(|pid| Path::new(&format!("/proc/{pid}")).exists()),
        _ => true,
    }
}

/// The profiles in `dir` (from `Local State`; `Default` for a new one).
fn profiles(dir: &Path) -> Vec<PathBuf> {
    let state: Option<Value> = std::fs::read(dir.join("Local State")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let mut names: Vec<String> = state.as_ref().and_then(|s| s.pointer("/profile/info_cache")?.as_object().map(|m| m.keys().cloned().collect())).unwrap_or_default();
    if names.is_empty() {
        names.push("Default".into());
    }
    names.into_iter().filter(|n| !n.contains('/') && n != "..").map(|n| dir.join(n)).collect()
}

/// `prefs` with Olo's defaults where nothing is set (an agent screen's,
/// `ready`, too).
fn with_defaults(mut prefs: Value, ready: bool) -> Value {
    if !prefs.is_object() {
        prefs = json!({});
    }
    let set = |prefs: &mut Value, path: &[&str], value: Value| {
        let mut at = prefs;
        for key in &path[..path.len() - 1] {
            let obj = at.as_object_mut().expect("object");
            at = obj.entry(*key).or_insert_with(|| json!({}));
            if !at.is_object() {
                return;
            }
        }
        at.as_object_mut().expect("object").entry(path[path.len() - 1]).or_insert(value);
    };
    set(&mut prefs, &["browser", "custom_chrome_frame"], json!(false));
    // Logins are Passwords' (the Olo Passwords extension), so Chrome
    // doesn't offer to keep them too.
    set(&mut prefs, &["credentials_enable_service"], json!(false));
    set(&mut prefs, &["profile", "password_manager_enabled"], json!(false));
    // (The New Tab page, Chrome's own default, said outright: a profile
    // that hasn't chosen offers "Continue where you left off".)
    if ready {
        set(&mut prefs, &["session", "restore_on_startup"], json!(5));
        // (A Google sign-in on a page is the page's alone: Chrome's own
        // "Sign in to Chrome?" took the keys meant for the page, an
        // agent's report, 2026-11-24. Chrome's "Allow Chrome sign-in".)
        signin_off(&mut prefs);
    }
    // A theme from the Chrome Web Store keeps its colours.
    if prefs.pointer("/extensions/theme/id").and_then(Value::as_str).is_none_or(str::is_empty) {
        set(&mut prefs, &["extensions", "theme", "system_theme"], json!(1));
    }
    prefs
}

/// Chrome's own sign-in turned off in `prefs`, where the profile hasn't
/// chosen: whether it changed anything. The setting is two: Chrome
/// starts with `signin.allowed` made what
/// `signin.allowed_on_next_startup` says, which is yes unless written.
/// `allowed` alone, as Olo first wrote it (2026-11-24, with no Chrome on
/// the dev server to try it against), was put back to yes by Chrome as
/// it started: a screen's Chrome 158 still offered "Sign in to Chrome?"
/// (seen 2026-11-26 on a host's own screen, the profile's file reading
/// `"allowed": true`).
fn signin_off(prefs: &mut Value) -> bool {
    let Some(prefs) = prefs.as_object_mut() else { return false };
    let signin = prefs.entry("signin").or_insert_with(|| json!({}));
    let Some(signin) = signin.as_object_mut() else { return false };
    if signin.contains_key("allowed_on_next_startup") {
        return false;
    }
    signin.insert("allowed_on_next_startup".into(), json!(false));
    signin.insert("allowed".into(), json!(false));
    true
}

/// Give `dir`'s profiles Olo's defaults, each once; on an agent screen
/// (`ready`), with Chrome's first run behind them.
pub fn apply(dir: &Path, ready: bool) {
    if running(dir) {
        return;
    }
    if ready
        && !dir.join(FIRST_RUN).exists()
        && let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(dir.join(FIRST_RUN), b""))
    {
        eprintln!("olo-chrome: couldn't set up {}: {e}", dir.display());
    }
    for profile in profiles(dir) {
        if profile.join(MARK).exists() {
            // (A screen's profile from before sign-in was turned off in
            // the way Chrome keeps: once more, for that alone.)
            if ready && let Err(e) = rewrite(&profile, |prefs| signin_off(prefs)) {
                eprintln!("olo-chrome: couldn't turn Chrome's sign-in off in {}: {e}", profile.display());
            }
            continue;
        }
        if let Err(e) = apply_to(&profile, ready) {
            eprintln!("olo-chrome: couldn't set up {}: {e}", profile.display());
        }
    }
}

/// `profile`'s preferences changed by `change` and written again, if it
/// says it changed them (through a file beside, renamed into place).
fn rewrite(profile: &Path, change: impl FnOnce(&mut Value) -> bool) -> std::io::Result<()> {
    let file = profile.join("Preferences");
    let mut prefs: Value = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !change(&mut prefs) {
        return Ok(());
    }
    let tmp = profile.join("Preferences.olo-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&prefs).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &file)
}

fn apply_to(profile: &Path, ready: bool) -> std::io::Result<()> {
    let file = profile.join("Preferences");
    let prefs = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    std::fs::create_dir_all(profile)?;
    let tmp = profile.join("Preferences.olo-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&with_defaults(prefs, ready)).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &file)?;
    std::fs::write(profile.join(MARK), "Olo set this profile's title bar, theme and password saving once; Chrome's settings change them.\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_only_gaps() {
        let p = with_defaults(json!({"browser": {"custom_chrome_frame": true}, "x": 1}), false);
        assert_eq!(p["browser"]["custom_chrome_frame"], json!(true));
        assert_eq!(p["extensions"]["theme"]["system_theme"], json!(1));
        assert_eq!(p["x"], json!(1));
        let themed = with_defaults(json!({"extensions": {"theme": {"id": "abc"}}}), false);
        assert!(themed["extensions"]["theme"].get("system_theme").is_none());
        assert_eq!(themed["browser"]["custom_chrome_frame"], json!(false));
    }

    /// An agent screen's profile starts with Chrome's own sign-in off; the
    /// person's own Chrome keeps it as Chrome has it.
    #[test]
    fn a_screens_profile_signs_nothing_in() {
        let new = with_defaults(json!({}), true);
        assert_eq!((&new["signin"]["allowed"], &new["signin"]["allowed_on_next_startup"]), (&json!(false), &json!(false)));
        assert!(with_defaults(json!({}), false).get("signin").is_none());
        // A profile Chrome has started once has `allowed` as Chrome put
        // it (yes), and nothing for the next start: both are turned off.
        let mut started = json!({"signin": {"allowed": true, "accounts_metadata_dict": {}}});
        assert!(signin_off(&mut started));
        assert_eq!(started["signin"], json!({"allowed": false, "allowed_on_next_startup": false, "accounts_metadata_dict": {}}));
        // One that has chosen (Chrome's "Allow Chrome sign-in", either
        // way) keeps its choice.
        let mut chosen = json!({"signin": {"allowed": true, "allowed_on_next_startup": true}});
        assert!(!signin_off(&mut chosen));
        assert_eq!(chosen["signin"]["allowed"], json!(true));
    }

    /// A lock left by a Chrome that has ended isn't a Chrome running;
    /// one of a process that runs is, and so is another computer's.
    #[test]
    fn a_lock_left_behind_is_nobodys() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-prefs-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let here = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
        let lock = |to: String| {
            let _ = std::fs::remove_file(dir.join("SingletonLock"));
            std::os::unix::fs::symlink(to, dir.join("SingletonLock")).unwrap();
        };
        assert!(!running(&dir), "no lock");
        lock(format!("{}-{}", here.trim(), std::process::id()));
        assert!(running(&dir), "this process runs");
        // (A process number above any the kernel gives out.)
        lock(format!("{}-4194999", here.trim()));
        assert!(!running(&dir), "a process that has ended");
        lock("another-computer-1234".into());
        assert!(running(&dir), "another computer's can't be asked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A screen's profile set up before (marked) has sign-in turned off
    /// at its next start, and nothing else of it touched; the person's
    /// own Chrome's profile is left as it is.
    #[test]
    fn a_marked_screens_profile_has_sign_in_turned_off_once_more() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-prefs-marked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let profile = dir.join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        let before = json!({"signin": {"allowed": true}, "browser": {"custom_chrome_frame": true}});
        std::fs::write(profile.join("Preferences"), before.to_string()).unwrap();
        std::fs::write(profile.join(MARK), "").unwrap();
        apply(&dir, false);
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(profile.join("Preferences")).unwrap()).unwrap(), before);
        apply(&dir, true);
        let after: Value = serde_json::from_slice(&std::fs::read(profile.join("Preferences")).unwrap()).unwrap();
        assert_eq!(after, json!({"signin": {"allowed": false, "allowed_on_next_startup": false}, "browser": {"custom_chrome_frame": true}}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profiles_are_set_up_once() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        apply(&dir, false);
        let file = dir.join("Default/Preferences");
        let p: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(p["browser"]["custom_chrome_frame"], json!(false));
        // The person's own first run is still Chrome's.
        assert!(!dir.join(FIRST_RUN).exists() && p.get("session").is_none());
        // A later choice in Chrome stays.
        std::fs::write(&file, r#"{"browser":{"custom_chrome_frame":true}}"#).unwrap();
        apply(&dir, false);
        assert!(std::fs::read_to_string(&file).unwrap().contains("true"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screens_profile_starts_ready() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-ready-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        apply(&dir, true);
        assert!(dir.join(FIRST_RUN).exists(), "past Chrome's first run (its terms window)");
        let p: Value = serde_json::from_slice(&std::fs::read(dir.join("Default/Preferences")).unwrap()).unwrap();
        assert_eq!(p["session"]["restore_on_startup"], json!(5));
        // One set up before (marked done) gets past first run too.
        let _ = std::fs::remove_file(dir.join(FIRST_RUN));
        apply(&dir, true);
        assert!(dir.join(FIRST_RUN).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
