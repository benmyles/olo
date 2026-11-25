//! Open in Tracker, the one way every app shows a folder, or a file in its
//! folder, in Tracker — locally or on a Rex host (`rex://HOST/PATH`).
//!
//! Wherever something has a folder or file behind it (a Terminal or Rex tab's
//! working folder, a document, a picture), its context menu, and its File/Pane
//! menu where it has one, offers [`LABEL`] (command [`COMMAND`]), which calls
//! [`open`] or [`open_uri`].

use std::{
    io,
    path::{Path, PathBuf},
};

/// The item's label, the same everywhere.
pub const LABEL: &str = "Open in Tracker";
/// Its menu command.
pub const COMMAND: &str = "open-in-tracker";
/// URI prefix for a path on a Rex host.
pub const REX_SCHEME: &str = "rex://";

/// A canonical path on a Rex host (`rex://HOST/PATH`).
///
/// `host` is the host's label (or id) as Rex knows it; `path` is either `"~"` /
/// `"~/..."` (resolved against the host account's `$HOME`) or an absolute path
/// starting with `/` (no trailing `/` except `"/"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RexPath {
    pub host: String,
    pub path: String,
}

impl RexPath {
    /// Parse a `rex://HOST/PATH` URI. Accepts `rex://HOST` (as `/`),
    /// `rex://HOST/~/sub` (keeps `~/sub`), and percent-encoded host labels.
    pub fn parse(raw: &str) -> Option<Self> {
        let rest = raw.trim().strip_prefix(REX_SCHEME)?;
        if rest.is_empty() {
            return None;
        }
        let (host_enc, tail) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let host = percent_decode(host_enc);
        if host.is_empty() {
            return None;
        }
        let path = normalize_remote_path(tail);
        Some(Self { host, path })
    }

    /// Parse a `Path` if it holds a `rex://HOST/PATH` URI.
    pub fn from_path(p: &Path) -> Option<Self> {
        p.to_str().and_then(Self::parse)
    }

    /// True if `p` is a `rex://` URI (including the bare `"rex://"` host root).
    pub fn is_rex(p: &Path) -> bool {
        p.to_str().is_some_and(|s| s.starts_with(REX_SCHEME))
    }

    /// Construct a `RexPath` from a host label and a path on that host.
    pub fn new(host: impl Into<String>, path: impl AsRef<str>) -> Self {
        let raw = path.as_ref().trim();
        let norm = if raw == "~" || raw.starts_with("~/") {
            normalize_remote_path(&format!("/{raw}"))
        } else if raw.starts_with('/') {
            normalize_remote_path(raw)
        } else if raw.is_empty() || raw == "." {
            "~".to_owned()
        } else {
            normalize_remote_path(&format!("/~/{raw}"))
        };
        Self { host: host.into(), path: norm }
    }

    /// Format as canonical `rex://HOST/PATH`.
    pub fn to_uri(&self) -> String {
        let host = percent_encode_host(&self.host);
        if self.path.starts_with('/') { format!("{REX_SCHEME}{host}{}", self.path) } else { format!("{REX_SCHEME}{host}/{}", self.path) }
    }

    /// Store in a `PathBuf` (used throughout Tracker's windows, history and
    /// entries).
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.to_uri())
    }

    /// Parent directory on the same host, or `None` at `"/"`.
    pub fn parent(&self) -> Option<Self> {
        if self.path == "/" || self.path == "~" {
            return None;
        }
        let trimmed = self.path.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) => Some(Self { host: self.host.clone(), path: "/".into() }),
            Some(i) => Some(Self { host: self.host.clone(), path: trimmed[..i].to_owned() }),
            None => None,
        }
    }

    /// Append a child segment (or relative sub-path) on the same host.
    pub fn join(&self, child: &str) -> Self {
        let child = child.trim_start_matches('/');
        if child.is_empty() {
            return self.clone();
        }
        let combined = if self.path == "/" {
            format!("/{child}")
        } else if self.path.starts_with('/') {
            format!("{}/{child}", self.path.trim_end_matches('/'))
        } else {
            format!("/{}/{child}", self.path.trim_end_matches('/'))
        };
        Self { host: self.host.clone(), path: normalize_remote_path(&combined) }
    }

    /// Leaf name (`"/"` at root, `"~"` at home).
    pub fn file_name(&self) -> &str {
        if self.path == "/" {
            return "/";
        }
        self.path.trim_end_matches('/').rsplit('/').next().unwrap_or(&self.path)
    }

    /// Window tab title (e.g. `"olo — dev-box"`, or `"dev-box"` at root).
    pub fn window_title(&self) -> String {
        let leaf = self.file_name();
        if leaf == "/" || leaf == "~" { self.host.clone() } else { format!("{leaf} — {}", self.host) }
    }
}

fn normalize_remote_path(raw: &str) -> String {
    // Preserve leading `/~` or `/~/...` as `~` / `~/...` so the host expands `$HOME`.
    if raw == "/~" || raw == "/~/" {
        return "~".into();
    }
    if let Some(rest) = raw.strip_prefix("/~/") {
        let mut parts: Vec<&str> = Vec::new();
        for seg in rest.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        return if parts.is_empty() { "~".into() } else { format!("~/{}", parts.join("/")) };
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() { "/".into() } else { format!("/{}", parts.join("/")) }
}

fn percent_encode_host(host: &str) -> String {
    let mut out = String::with_capacity(host.len());
    for b in host.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where Olo Rex listens for Tracker's remote filesystem requests (one per
/// Wayland display, or overridden with `OLO_REX_FILES_SOCKET` for tests).
pub fn rex_files_socket() -> PathBuf {
    if let Ok(path) = std::env::var("OLO_REX_FILES_SOCKET")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let safe: String = display.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    olo_xdg::runtime_dir().join(format!("olo-rex-files-{safe}.sock"))
}

/// Show `path` in Tracker (`olo-tracker --reveal PATH`): a folder in its
/// window, a file selected in its folder's, on the workspace showing (one
/// there, else a new one). Tracker starts if it isn't running.
pub fn open(path: &Path) -> io::Result<()> {
    open_uri(&path.display().to_string())
}

/// Show a local path or `rex://HOST/PATH` URI in Tracker.
pub fn open_uri(target: &str) -> io::Result<()> {
    let args = ["--reveal".to_owned(), target.to_owned()];
    if crate::instance::forward("olo.tracker", &args) {
        return Ok(());
    }
    let tracker = crate::find_program("olo-tracker").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Tracker isn’t installed"))?;
    let mut argv = vec![tracker.into_os_string()];
    argv.extend(args.iter().map(Into::into));
    crate::open::spawn(&argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rex_path_roundtrip_parent_and_join() {
        let p = RexPath::parse("rex://dev-box/home/dev/src/olo/").unwrap();
        assert_eq!(p.host, "dev-box");
        assert_eq!(p.path, "/home/dev/src/olo");
        assert_eq!(p.to_uri(), "rex://dev-box/home/dev/src/olo");
        assert_eq!(p.file_name(), "olo");
        assert_eq!(p.window_title(), "olo — dev-box");

        let up = p.parent().unwrap();
        assert_eq!(up.to_uri(), "rex://dev-box/home/dev/src");
        assert_eq!(up.join("../notes/a.md").to_uri(), "rex://dev-box/home/dev/notes/a.md");

        let root = RexPath::parse("rex://dev-box").unwrap();
        assert_eq!(root.path, "/");
        assert_eq!(root.parent(), None);
        assert_eq!(root.window_title(), "dev-box");
        assert_eq!(root.join("etc").to_uri(), "rex://dev-box/etc");

        let home = RexPath::parse("rex://my%20server/~/work").unwrap();
        assert_eq!(home.host, "my server");
        assert_eq!(home.path, "~/work");
        assert_eq!(home.to_uri(), "rex://my%20server/~/work");
        assert_eq!(RexPath::parse(&home.to_uri()), Some(home));
    }
}
