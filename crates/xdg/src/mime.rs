//! MIME types for files and URLs, from the shared-mime-info glob database
//! (what every freedesktop app uses) plus a little content sniffing for
//! files without a telling name.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

struct Glob {
    weight: u32,
    mime: String,
    pattern: String,
    case_sensitive: bool,
}

fn mime_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share/mime"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("mime")));
    v
}

fn globs() -> &'static [Glob] {
    static GLOBS: OnceLock<Vec<Glob>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let mut v = Vec::new();
        for dir in mime_dirs() {
            let Ok(text) = std::fs::read_to_string(dir.join("globs2")) else { continue };
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                // weight:type:pattern[:flags]
                let mut parts = line.splitn(4, ':');
                let (Some(w), Some(mime), Some(pattern)) = (parts.next(), parts.next(), parts.next()) else { continue };
                let flags = parts.next().unwrap_or("");
                v.push(Glob {
                    weight: w.parse().unwrap_or(50),
                    mime: mime.to_owned(),
                    pattern: pattern.to_owned(),
                    case_sensitive: flags.split(',').any(|f| f == "cs"),
                });
            }
        }
        v
    })
}

/// Shell-style match supporting `*`, `?` and `[...]` (what globs2 uses).
fn glob_match(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|i| glob_match(rest, &name[i..])),
        Some(('?', rest)) => !name.is_empty() && glob_match(rest, &name[1..]),
        Some(('[', rest)) => {
            let Some(close) = rest.iter().position(|&c| c == ']') else { return false };
            let (set, after) = (&rest[..close], &rest[close + 1..]);
            let Some((&c, tail)) = name.split_first() else { return false };
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    hit |= (set[i]..=set[i + 2]).contains(&c);
                    i += 3;
                } else {
                    hit |= set[i] == c;
                    i += 1;
                }
            }
            hit && glob_match(after, tail)
        }
        Some((&p, rest)) => name.first() == Some(&p) && glob_match(rest, &name[1..]),
    }
}

/// A backup of Olo's settings (Preferences ▸ Backup opens it).
pub const SETTINGS_BACKUP: &str = "application/x-olo-backup";

/// The type the file name suggests, if any: highest weight wins, then the
/// longest pattern (so `*.tar.gz` beats `*.gz`).
pub fn from_name(name: &str) -> Option<String> {
    // Ours (or Tabula's, from before the rename), which no
    // shared-mime-info database knows.
    if name.to_lowercase().ends_with(".olo-backup") || name.to_lowercase().ends_with(".tabula-backup") {
        return Some(SETTINGS_BACKUP.into());
    }
    let lower: Vec<char> = name.to_lowercase().chars().collect();
    let exact: Vec<char> = name.chars().collect();
    globs()
        .iter()
        .filter(|g| {
            let p: Vec<char> = if g.case_sensitive { g.pattern.chars().collect() } else { g.pattern.to_lowercase().chars().collect() };
            glob_match(&p, if g.case_sensitive { &exact } else { &lower })
        })
        .max_by_key(|g| (g.weight, g.pattern.len()))
        .map(|g| g.mime.clone())
}

/// The type of the file at `path`: folders, then the name, then a look at
/// the first bytes.
pub fn for_path(path: &Path) -> String {
    if path.is_dir() {
        return "inode/directory".into();
    }
    if let Some(m) = path.file_name().and_then(|n| from_name(&n.to_string_lossy())) {
        return m;
    }
    let mut head = [0u8; 512];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    sniff(&head[..n]).into()
}

fn sniff(head: &[u8]) -> &'static str {
    const MAGIC: &[(&[u8], &str)] = &[
        (b"%PDF-", "application/pdf"),
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF8", "image/gif"),
        (b"PK\x03\x04", "application/zip"),
        (b"\x7fELF", "application/x-executable"),
        (b"#!", "application/x-shellscript"),
        (b"<?xml", "application/xml"),
        (b"<!DOCTYPE html", "text/html"),
        (b"<html", "text/html"),
    ];
    if let Some((_, m)) = MAGIC.iter().find(|(magic, _)| head.starts_with(magic)) {
        return m;
    }
    if head.is_empty() {
        return "text/plain";
    }
    // Text if it decodes and has no NULs or stray control bytes.
    let texty = utf8_prefix(head)
        && !head.iter().any(|&b| b == 0 || (b < 0x20 && !b"\t\n\r\x0c\x1b".contains(&b)));
    if texty { "text/plain" } else { "application/octet-stream" }
}

/// Valid UTF-8, allowing a character cut off by the read.
fn utf8_prefix(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4,
    }
}

/// The type a URL is opened as: its scheme's handler, or the file's type
/// for file: URLs.
pub fn for_url(url: &str) -> String {
    match url.split_once(':') {
        Some(("file", rest)) => for_path(Path::new(rest.trim_start_matches("//"))),
        Some((scheme, _)) if !scheme.is_empty() => format!("x-scheme-handler/{}", scheme.to_lowercase()),
        _ => for_path(Path::new(url)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, n: &str) -> bool {
        glob_match(&p.chars().collect::<Vec<_>>(), &n.chars().collect::<Vec<_>>())
    }

    #[test]
    fn globbing() {
        assert!(m("*.txt", "a.txt"));
        assert!(!m("*.txt", "a.txt~"));
        assert!(m("Makefile", "Makefile"));
        assert!(m("*.[ch]", "x.h"));
        assert!(m("README*", "README.md"));
        assert!(m("*.?z", "a.gz"));
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff(b"hello\n"), "text/plain");
        assert_eq!(sniff(b"\x00\x01\x02"), "application/octet-stream");
        assert_eq!(sniff("héllo".as_bytes()), "text/plain");
        assert_eq!(for_url("https://example.com"), "x-scheme-handler/https");
        assert_eq!(for_url("mailto:a@b"), "x-scheme-handler/mailto");
    }

    #[test]
    fn database() {
        // Needs shared-mime-info, which every desktop has.
        if globs().is_empty() {
            return;
        }
        assert_eq!(from_name("x.PNG").as_deref(), Some("image/png"));
        assert_eq!(from_name("a.tar.gz").as_deref(), Some("application/x-compressed-tar"));
        assert_eq!(from_name("notes.txt").as_deref(), Some("text/plain"));
    }
}
