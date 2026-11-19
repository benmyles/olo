//! The session's language and formats: the system's (`/etc/locale.conf`)
//! and the person's own (`~/.config/locale.conf`), for everything the
//! session starts.
//!
//! A display manager gives its sessions these (GDM asks localed); a
//! session started from Olo's own login got none, so every program in
//! it ran in the "C" locale: no `LANG` in Terminal's shells, sorting
//! and dates by C's rules, programs that take their text encoding from
//! the locale taking ASCII (found from app_server's "failed to get
//! system locale" in the log, 2026-11-18). What the environment already
//! says is kept.
//!
//! A program can't be given a language once it runs, so one started
//! without (a session from before this, a computer whose language was
//! set after the login) hands it to what it starts instead ([`give`]):
//! the programs a person opens from Deskbar, Tracker's, Terminal's
//! shells have it at once, whatever the program that starts them has.

use std::path::Path;

/// The variables a locale file may set (locale.conf(5)).
const NAMES: [&str; 14] =
    ["LANG", "LANGUAGE", "LC_CTYPE", "LC_NUMERIC", "LC_TIME", "LC_COLLATE", "LC_MONETARY", "LC_MESSAGES", "LC_PAPER", "LC_NAME", "LC_ADDRESS", "LC_TELEPHONE", "LC_MEASUREMENT", "LC_IDENTIFICATION"];

/// The variables a session exports to what's started on demand.
pub const EXPORTED: [&str; 14] = NAMES;

/// What a locale.conf's `text` sets: `NAME=value` lines, the value bare
/// or quoted, comments and anything else passed over.
pub fn read(text: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((name, value)) = line.split_once('=') else { continue };
        let Some(name) = NAMES.iter().find(|n| **n == name.trim()) else { continue };
        let value = value.trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(value);
        if !value.is_empty() && !value.contains(['\0', '\n']) {
            out.retain(|(n, _)| n != name);
            out.push((*name, value.to_owned()));
        }
    }
    out
}

/// What to add to an environment that has `has(name)`: the person's
/// file over the system's, and neither over what's set already. Nothing
/// where the environment names a locale itself (`LANG` or `LC_ALL`).
pub fn missing(system: &str, own: &str, has: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    if has("LANG") || has("LC_ALL") {
        return Vec::new();
    }
    let mut vars = read(system);
    for (name, value) in read(own) {
        vars.retain(|(n, _)| *n != name);
        vars.push((name, value));
    }
    vars.retain(|(name, _)| !has(name));
    vars
}

/// The system's locale file, and the person's own beside their settings.
pub const SYSTEM_CONF: &str = "etc/locale.conf";

fn own_conf() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("locale.conf")
}

/// What an environment that has `has(name)` lacks of the system's and
/// the person's locale files, as they are now.
pub fn missing_for(has: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    let text = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    missing(&text(&Path::new("/").join(SYSTEM_CONF)), &text(&own_conf()), has)
}

/// What this process's environment lacks of them.
pub fn missing_here() -> Vec<(&'static str, String)> {
    missing_for(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

/// A program being started is given the language this one has none of
/// to hand down (two small files read, as it's started): everything
/// Olo starts for the person goes through this.
pub fn give(cmd: &mut std::process::Command) {
    for (name, value) in missing_here() {
        cmd.env(name, value);
    }
}

/// The locale for this process and what it starts, where the login gave
/// none. Before any thread is started.
pub fn set() {
    for (name, value) in missing_here() {
        // SAFETY: single-threaded start-up (the caller's word).
        unsafe { std::env::set_var(name, value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_file_read() {
        let text = "# Written by localectl\nLANG=\"en_US.UTF-8\"\nLC_TIME=nl_NL.UTF-8\n LC_PAPER = 'en_GB.UTF-8' \nPATH=/evil\nLC_ALL=C\nLANG=\nnonsense\n";
        assert_eq!(read(text), [("LANG", "en_US.UTF-8".to_owned()), ("LC_TIME", "nl_NL.UTF-8".to_owned()), ("LC_PAPER", "en_GB.UTF-8".to_owned())]);
        assert!(read("").is_empty());
    }

    /// A program started is given what its starter lacks, and nothing
    /// where the starter has a language to hand down. (This computer's
    /// files are what they are: only that `give` and `missing_here`
    /// agree is checked.)
    #[test]
    fn a_started_program_is_given_what_is_missing() {
        let mut cmd = std::process::Command::new("true");
        give(&mut cmd);
        let given: Vec<(String, String)> = cmd.get_envs().filter_map(|(k, v)| Some((k.to_str()?.to_owned(), v?.to_str()?.to_owned()))).collect();
        let missing: Vec<(String, String)> = missing_here().into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        assert_eq!(given, missing);
        if std::env::var_os("LANG").is_some_and(|v| !v.is_empty()) {
            assert!(given.is_empty());
        }
    }

    /// The person's file over the system's; nothing over what the login
    /// gave; nothing at all where the login named a locale.
    #[test]
    fn only_what_is_missing() {
        let (system, own) = ("LANG=en_US.UTF-8\nLC_TIME=en_US.UTF-8\n", "LC_TIME=nl_NL.UTF-8\nLC_MEASUREMENT=nl_NL.UTF-8\n");
        assert_eq!(missing(system, own, |_| false), [("LANG", "en_US.UTF-8".to_owned()), ("LC_TIME", "nl_NL.UTF-8".to_owned()), ("LC_MEASUREMENT", "nl_NL.UTF-8".to_owned())]);
        assert_eq!(missing(system, "", |name| name == "LC_TIME"), [("LANG", "en_US.UTF-8".to_owned())]);
        assert!(missing(system, own, |name| name == "LANG").is_empty());
        assert!(missing(system, own, |name| name == "LC_ALL").is_empty());
        assert!(missing("", "", |_| false).is_empty());
    }
}
