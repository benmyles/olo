//! Chrome as an Olo program: its profiles start out with Olo's title
//! bar (Chrome's "Use system title bar and borders": Olo's yellow tab
//! and frame, no buttons of Chrome's own) and the system's colours
//! (Chrome's "GTK" theme, which Olo's GTK themes colour; see
//! `olo_theme::toolkit`).
//!
//! Only once a profile, so a choice made afterwards in Chrome's settings
//! stays: a `.olo-defaults` file in the profile marks it done. Only
//! while Chrome isn't running (it writes its preferences back on exit).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const MARK: &str = ".olo-defaults";

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
/// `host-pid`).
fn running(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join("SingletonLock")).is_ok()
}

/// The profiles in `dir` (from `Local State`; `Default` for a new one).
fn profiles(dir: &Path) -> Vec<PathBuf> {
    let state: Option<Value> = std::fs::read(dir.join("Local State")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let mut names: Vec<String> =
        state.as_ref().and_then(|s| s.pointer("/profile/info_cache")?.as_object().map(|m| m.keys().cloned().collect())).unwrap_or_default();
    if names.is_empty() {
        names.push("Default".into());
    }
    names.into_iter().filter(|n| !n.contains('/') && n != "..").map(|n| dir.join(n)).collect()
}

/// `prefs` with Olo's defaults where nothing is set.
fn with_defaults(mut prefs: Value) -> Value {
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
    // A theme from the Chrome Web Store keeps its colours.
    if prefs.pointer("/extensions/theme/id").and_then(Value::as_str).is_none_or(str::is_empty) {
        set(&mut prefs, &["extensions", "theme", "system_theme"], json!(1));
    }
    prefs
}

/// Give `dir`'s profiles Olo's defaults, each once.
pub fn apply(dir: &Path) {
    if running(dir) {
        return;
    }
    for profile in profiles(dir) {
        if profile.join(MARK).exists() {
            continue;
        }
        if let Err(e) = apply_to(&profile) {
            eprintln!("olo-chrome: couldn't set up {}: {e}", profile.display());
        }
    }
}

fn apply_to(profile: &Path) -> std::io::Result<()> {
    let file = profile.join("Preferences");
    let prefs = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    std::fs::create_dir_all(profile)?;
    let tmp = profile.join("Preferences.olo-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&with_defaults(prefs)).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &file)?;
    std::fs::write(profile.join(MARK), "Olo set this profile's title bar, theme and password saving once; Chrome's settings change them.\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_only_gaps() {
        let p = with_defaults(json!({"browser": {"custom_chrome_frame": true}, "x": 1}));
        assert_eq!(p["browser"]["custom_chrome_frame"], json!(true));
        assert_eq!(p["extensions"]["theme"]["system_theme"], json!(1));
        assert_eq!(p["x"], json!(1));
        let themed = with_defaults(json!({"extensions": {"theme": {"id": "abc"}}}));
        assert!(themed["extensions"]["theme"].get("system_theme").is_none());
        assert_eq!(themed["browser"]["custom_chrome_frame"], json!(false));
    }

    #[test]
    fn profiles_are_set_up_once() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        apply(&dir);
        let file = dir.join("Default/Preferences");
        let p: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(p["browser"]["custom_chrome_frame"], json!(false));
        // A later choice in Chrome stays.
        std::fs::write(&file, r#"{"browser":{"custom_chrome_frame":true}}"#).unwrap();
        apply(&dir);
        assert!(std::fs::read_to_string(&file).unwrap().contains("true"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
