//! Opening files and URLs with the right application, natively (what
//! `xdg-open` does, without running it): Tabula's own app when one is
//! installed for the type, else the user's default from mimeapps.list,
//! else any installed app that lists the type.

use std::{
    ffi::OsString,
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
};

use crate::{desktop_entry::{self, DesktopEntry}, find_program, mime};

/// What to open.
#[derive(Debug, Clone)]
pub enum Target {
    Path(PathBuf),
    Url(String),
}

impl Target {
    /// A path, a file: URL, or any other URL.
    pub fn parse(s: &str) -> Target {
        if let Some(p) = s.strip_prefix("file://") {
            Target::Path(PathBuf::from(percent_decode(p)))
        } else if s.contains("://") || s.starts_with("mailto:") {
            Target::Url(s.to_owned())
        } else {
            Target::Path(PathBuf::from(s))
        }
    }

    pub fn mime(&self) -> String {
        match self {
            Target::Path(p) => mime::for_path(p),
            Target::Url(u) => mime::for_url(u),
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("x"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Who opens a type.
#[derive(Debug, Clone, PartialEq)]
pub enum Handler {
    /// One of ours, by program name.
    Ours(&'static str),
    Entry(Box<DesktopEntry>),
}

impl Handler {
    pub fn name(&self) -> String {
        match self {
            Handler::Ours(bin) => crate::apps::our_app(bin.trim_start_matches("tabula-"))
                .map(|(n, _)| n.to_string())
                .unwrap_or_else(|| bin.trim_start_matches("tabula-").to_owned()),
            Handler::Entry(e) => e.name.clone(),
        }
    }
}

/// Tabula's own app for a type, if it is installed.
fn ours(mime: &str) -> Option<&'static str> {
    let (major, _) = mime.split_once('/').unwrap_or((mime, ""));
    let candidates: &[&'static str] = match (major, mime) {
        (_, "inode/directory") => &["tabula-tracker"],
        (_, "text/html" | "application/xhtml+xml") => &["tabula-chrome", "tabula-styled-edit"],
        ("text", _) | (_, "application/x-shellscript" | "application/json" | "application/xml" | "application/toml") => {
            &["tabula-styled-edit", "tabula-viewer"]
        }
        ("image" | "audio" | "video" | "font", _) => &["tabula-viewer"],
        (
            _,
            "application/pdf"
            | "application/zip"
            | "application/x-tar"
            | "application/x-compressed-tar"
            | "application/gzip"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.oasis.opendocument.text"
            | "application/vnd.oasis.opendocument.spreadsheet"
            | "application/vnd.oasis.opendocument.presentation",
        ) => &["tabula-viewer"],
        (_, "x-scheme-handler/http" | "x-scheme-handler/https") => &["tabula-chrome"],
        (_, crate::mime::SETTINGS_BACKUP) => &["tabula-preferences"],
        _ => &[],
    };
    candidates.iter().copied().find(|bin| find_program(bin).is_some())
}

/// Links and pages: what a web browser opens.
fn is_web(mime: &str) -> bool {
    matches!(mime, "x-scheme-handler/http" | "x-scheme-handler/https" | "text/html" | "application/xhtml+xml")
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// mimeapps.list files, most important first.
fn mimeapps_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut config_dirs = vec![config_home()];
    let sys = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    config_dirs.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    for dir in &config_dirs {
        for d in &desktops {
            v.push(dir.join(format!("{d}-mimeapps.list")));
        }
        v.push(dir.join("mimeapps.list"));
    }
    for dir in desktop_entry::app_dirs() {
        for d in &desktops {
            v.push(dir.join(format!("{d}-mimeapps.list")));
        }
        v.push(dir.join("mimeapps.list"));
        v.push(dir.join("defaults.list"));
    }
    v
}

/// The entries listed for `mime` under `[Default Applications]`.
fn defaults_in(text: &str, mime: &str) -> Vec<String> {
    let mut in_defaults = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
            continue;
        }
        if in_defaults
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == mime
        {
            return v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        }
    }
    Vec::new()
}

fn entry_by_file(file: &str) -> Option<DesktopEntry> {
    desktop_entry::app_dirs().into_iter().find_map(|d| desktop_entry::parse(&d.join(file)))
}

/// The broader types `mime` is a kind of (text/x-rust → text/plain), from
/// shared-mime-info's subclasses list.
fn parents(mime: &str) -> Vec<String> {
    let mut out = Vec::new();
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in sys.split(':').filter(|s| !s.is_empty()) {
        let Ok(text) = std::fs::read_to_string(Path::new(dir).join("mime/subclasses")) else { continue };
        for line in text.lines() {
            if let Some((child, parent)) = line.split_once(' ')
                && child == mime
                && !out.iter().any(|p| p == parent)
            {
                out.push(parent.to_owned());
            }
        }
    }
    if mime.starts_with("text/") && mime != "text/plain" && !out.iter().any(|p| p == "text/plain") {
        out.push("text/plain".into());
    }
    out
}

/// Who opens `mime`.
pub fn handler(mime: &str) -> Option<Handler> {
    let files: Vec<String> = mimeapps_files().iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    // The web browser is the person's choice (Tabula's own Chrome
    // otherwise); everything else opens in Tabula's apps first.
    if is_web(mime) {
        for text in &files {
            for file in defaults_in(text, mime) {
                if let Some(e) = entry_by_file(&file) {
                    return Some(Handler::Entry(Box::new(e)));
                }
            }
        }
    }
    if let Some(bin) = ours(mime) {
        return Some(Handler::Ours(bin));
    }
    let mut types = vec![mime.to_owned()];
    types.extend(parents(mime));
    for t in &types {
        for text in &files {
            for file in defaults_in(text, t) {
                if let Some(e) = entry_by_file(&file) {
                    return Some(Handler::Entry(Box::new(e)));
                }
            }
        }
    }
    let all = desktop_entry::all();
    types.iter().find_map(|t| all.iter().find(|e| e.mime_types.iter().any(|m| m == t)).cloned().map(|e| Handler::Entry(Box::new(e))))
}

/// [`handler`], remembered per type for a while (it reads many files).
pub fn handler_cached(mime: &str) -> Option<Handler> {
    use std::{collections::HashMap, sync::{Mutex, OnceLock}, time::{Duration, Instant}};
    type Cache = Mutex<HashMap<String, (Instant, Option<Handler>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    const FRESH: Duration = Duration::from_secs(30);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, h)) = cache.lock().unwrap().get(mime)
        && at.elapsed() < FRESH
    {
        return h.clone();
    }
    let h = handler(mime);
    cache.lock().unwrap().insert(mime.to_owned(), (Instant::now(), h.clone()));
    h
}

/// The command line for `entry` opening `target`, with the Exec field codes
/// (%f %F %u %U %i %c %k) expanded. Arguments are passed as-is, never
/// through a shell.
pub fn command_line(entry: &DesktopEntry, target: &Target) -> Option<Vec<OsString>> {
    let exec = entry.exec.as_ref()?;
    let (as_file, as_url): (OsString, OsString) = match target {
        Target::Path(p) => (p.clone().into_os_string(), format!("file://{}", p.display()).into()),
        Target::Url(u) => (u.into(), u.into()),
    };
    let mut argv = Vec::new();
    let mut used = false;
    for word in split_exec(exec) {
        match word.as_str() {
            "%f" | "%F" => {
                argv.push(as_file.clone());
                used = true;
            }
            "%u" | "%U" => {
                argv.push(as_url.clone());
                used = true;
            }
            "%i" => {
                if let Some(icon) = &entry.icon {
                    argv.push("--icon".into());
                    argv.push(icon.into());
                }
            }
            "%c" => argv.push(entry.name.clone().into()),
            "%k" => argv.push(entry.path.clone().into_os_string()),
            w if w.len() == 2 && w.starts_with('%') => {}
            w => argv.push(w.replace("%%", "%").into()),
        }
    }
    if !used {
        argv.push(as_file);
    }
    if entry.terminal {
        let mut t: Vec<OsString> = vec!["tabula-terminal".into(), "-e".into()];
        t.extend(argv);
        argv = t;
    }
    (!argv.is_empty()).then_some(argv)
}

/// Split an Exec value into words, honouring its double quotes and
/// backslash escapes.
fn split_exec(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut escaped, mut any) = (false, false, false);
    for ch in exec.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
            any = true;
        } else if ch.is_whitespace() && !quoted {
            if any || !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            any = false;
        } else {
            cur.push(ch);
        }
    }
    if any || !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Open `target` with its handler, detached from us.
pub fn open(target: &Target) -> io::Result<()> {
    let mime = target.mime();
    let argv: Vec<OsString> = match handler_cached(&mime) {
        Some(Handler::Ours(bin)) => {
            let arg: OsString = match target {
                Target::Path(p) => p.clone().into(),
                Target::Url(u) => u.into(),
            };
            vec![find_program(bin).map(PathBuf::into_os_string).unwrap_or_else(|| bin.into()), arg]
        }
        Some(Handler::Entry(e)) => {
            command_line(&e, target).ok_or_else(|| io::Error::other(format!("{} has no Exec line", e.name)))?
        }
        None => return Err(io::Error::new(io::ErrorKind::NotFound, format!("no application opens {mime}"))),
    };
    spawn(&argv)
}

/// Run `argv` detached (own session, no stdio), reaping it when it exits.
pub fn spawn(argv: &[OsString]) -> io::Result<()> {
    let (program, args) = argv.split_first().ok_or_else(|| io::Error::other("empty command"))?;
    let mut c = std::process::Command::new(program);
    c.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines() {
        assert_eq!(split_exec(r#"foo --x "a b" %U"#), ["foo", "--x", "a b", "%U"]);
        assert_eq!(split_exec(r#"sh -c "echo \"hi\"""#), ["sh", "-c", "echo \"hi\""]);
        let e = DesktopEntry { name: "Foo".into(), exec: Some("foo --new %U".into()), ..Default::default() };
        let cmd = command_line(&e, &Target::Url("https://x.org".into())).unwrap();
        assert_eq!(cmd, ["foo", "--new", "https://x.org"]);
        let e = DesktopEntry { exec: Some("bar".into()), ..Default::default() };
        let cmd = command_line(&e, &Target::Path("/tmp/a b".into())).unwrap();
        assert_eq!(cmd, ["bar", "/tmp/a b"]);
    }

    #[test]
    fn defaults_section() {
        let text = "[Added Associations]\nimage/png=no.desktop\n[Default Applications]\nimage/png=imv.desktop;other.desktop;\n";
        assert_eq!(defaults_in(text, "image/png"), ["imv.desktop", "other.desktop"]);
        assert!(defaults_in(text, "image/gif").is_empty());
    }

    #[test]
    fn targets() {
        assert!(matches!(Target::parse("file:///tmp/a%20b"), Target::Path(p) if p == Path::new("/tmp/a b")));
        assert!(matches!(Target::parse("https://x"), Target::Url(_)));
        assert!(matches!(Target::parse("/etc/hosts"), Target::Path(_)));
    }
}

#[cfg(test)]
mod live {
    /// Prints who opens a few types here. `cargo test -p tabula-xdg -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn handlers() {
        for m in ["x-scheme-handler/https", "image/png", "text/x-rust", "inode/directory", "application/pdf", "application/x-iso9660-image"] {
            println!("{m}: {:?}", super::handler(m).map(|h| h.name()));
        }
    }
}
