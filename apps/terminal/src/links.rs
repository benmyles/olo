//! Clickable URLs and file paths: ⌘-hover underlines them, ⌘-click opens
//! them. Links come from OSC 8 hyperlinks the program wrote, or are found
//! in the text: URLs by pattern, file paths by checking that they exist
//! (relative to the working directory of whatever is running), also one
//! a program broke over lines itself (`broken`: Claude Code does, at its
//! text's edge, the rest indented on the next).

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Url(String),
    /// A file or folder, with the line (and column) when the text named
    /// one, as compilers print them: `src/main.rs:12:5`.
    Path(PathBuf, Option<(u32, Option<u32>)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub target: Target,
    /// Character range within the logical line.
    pub start: usize,
    pub end: usize,
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:https?|ftp|file|ssh|git)://[^\s<>"'`{}|\\^\x00-\x1f]+|mailto:[^\s<>"'`]+|www\.[^\s<>"'`]+\.[^\s<>"'`]+"#).unwrap())
}

fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Absolute, home-relative or relative paths with a slash, or a bare
        // name with an extension; optionally :line[:column].
        Regex::new(r"(?:~|\.{1,2})?/?[\w.@+\-~]*[\w@+\-~](?:/[\w.@+\-~]*)*(?::\d+(?::\d+)?)?").unwrap()
    })
}

/// Drop trailing punctuation that is almost never part of a link, keeping
/// a closing bracket that matches an opening one inside.
fn trim(s: &str) -> &str {
    let mut s = s;
    loop {
        let Some(last) = s.chars().last() else { return s };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            _ => false,
        };
        if !drop {
            return s;
        }
        s = &s[..s.len() - last.len_utf8()];
    }
}

fn expand(p: &str, cwd: &Path) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest)
    } else if p == "~" {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
    } else if p.starts_with('/') {
        PathBuf::from(p)
    } else {
        cwd.join(p)
    }
}

/// Split `foo.rs:12:5` into the path and position.
fn split_position(s: &str) -> (&str, Option<(u32, Option<u32>)>) {
    let mut parts = s.rsplitn(3, ':');
    let a = parts.next();
    let b = parts.next();
    let c = parts.next();
    match (c, b.and_then(|b| b.parse::<u32>().ok()), a.and_then(|a| a.parse::<u32>().ok())) {
        (Some(path), Some(line), Some(col)) => (path, Some((line, Some(col)))),
        _ => {
            let mut parts = s.rsplitn(2, ':');
            let a = parts.next();
            match (parts.next(), a.and_then(|a| a.parse::<u32>().ok())) {
                (Some(path), Some(line)) => (path, Some((line, None))),
                _ => (s, None),
            }
        }
    }
}

/// The link covering character `col` of `line`, if any. `exists` checks
/// candidate paths (a parameter so tests needn't touch the disk).
pub fn find(line: &str, col: usize, cwd: &Path, exists: impl Fn(&Path) -> bool) -> Option<Link> {
    let byte_at = |ci: usize| line.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(line.len());
    let char_of = |b: usize| line[..b].chars().count();
    let at = byte_at(col);
    for m in url_re().find_iter(line) {
        if m.start() <= at && at < m.end() {
            let text = trim(m.as_str());
            if at >= m.start() + text.len() {
                return None;
            }
            let url = if text.starts_with("www.") { format!("https://{text}") } else { text.to_owned() };
            return Some(Link { target: Target::Url(url), start: char_of(m.start()), end: char_of(m.start() + text.len()) });
        }
    }
    for m in path_re().find_iter(line) {
        if !(m.start() <= at && at < m.end()) {
            continue;
        }
        let text = trim(m.as_str());
        let (path, pos) = split_position(text);
        // Bare words are only links if they name something that exists
        // and look like a file (a slash or an extension).
        if path.is_empty() || !(path.contains('/') || path.contains('.') || path.starts_with('~')) {
            return None;
        }
        let full = expand(path, cwd);
        if !exists(&full) {
            return None;
        }
        return Some(Link { target: Target::Path(full, pos), start: char_of(m.start()), end: char_of(m.start() + text.len()) });
    }
    None
}

pub use olo_kit::links::broken;

/// Open a link: web addresses in the browser; files and folders in
/// Tracker (a file selected in its folder's window).
pub fn open(target: &Target) {
    use olo_xdg::open as opener;
    let target = target.clone();
    // Finding the handler reads files: not on the UI thread.
    std::thread::spawn(move || {
        let done = match &target {
            Target::Url(u) => opener::open(&opener::Target::parse(u)).map_err(|e| e.to_string()),
            Target::Path(p, _) => olo_kit_base::tracker::open(p).map_err(|e| e.to_string()),
        };
        if let Err(e) = done {
            tracing::warn!("open link: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(line: &str, col: usize) -> Option<Link> {
        find(line, col, Path::new("/work"), |p| ["/work/src/main.rs", "/etc/hosts", "/work/README.md", "/work/src"].iter().any(|e| Path::new(e) == p))
    }

    #[test]
    fn urls() {
        let l = link("see https://example.com/a(b)c. ok", 8).unwrap();
        assert_eq!(l.target, Target::Url("https://example.com/a(b)c".into()));
        assert_eq!((l.start, l.end), (4, 29));
        assert_eq!(link("(https://x.org/y)", 3).unwrap().target, Target::Url("https://x.org/y".into()));
        assert_eq!(link("go to www.rust-lang.org.", 8).unwrap().target, Target::Url("https://www.rust-lang.org".into()));
        assert!(link("see https://example.com", 1).is_none());
    }

    /// Rows of `cols` columns, as a terminal shows them.
    fn screen(lines: &[&str], cols: usize) -> Vec<Vec<char>> {
        lines.iter().map(|l| l.chars().chain(std::iter::repeat(' ')).take(cols).collect()).collect()
    }

    #[test]
    fn broken_words() {
        // Claude Code's way: a long URL at its text's edge, the rest
        // indented on the next rows.
        let rows = screen(&["⏺ See https://github.com/anthropics/claude-", "  code/issues/1234?q=a and more.", "  next"], 44);
        let (text, at, cells) = broken(&rows, 0, 10, 44).unwrap();
        assert_eq!(text, "https://github.com/anthropics/claude-code/issues/1234?q=a");
        assert_eq!(at, 4);
        assert_eq!(cells[0], (0, 6));
        assert_eq!(*cells.last().unwrap(), (1, 21));
        // From the second piece too.
        let (text, at, _) = broken(&rows, 1, 4, 44).unwrap();
        assert_eq!(text, "https://github.com/anthropics/claude-code/issues/1234?q=a");
        assert_eq!(at, 39);
        let link = find(&text, at, Path::new("/"), |_| false).unwrap();
        assert_eq!(link.target, Target::Url(text.clone()));
        // A row that ends short of the edge isn't broken.
        let rows = screen(&["see https://x.org/a", "  next row"], 44);
        assert!(broken(&rows, 0, 6, 44).is_none());
        // Nor across a frame.
        let rows = screen(&["│ https://example.com/aaaaaaaaaaaaaaaaaa │", "│ bbb                                   │"], 42);
        assert!(broken(&rows, 0, 4, 42).is_none());
    }

    #[test]
    fn claude_code_lines() {
        let l = link("⏺ Read(src/main.rs)", 8).unwrap();
        assert_eq!(l.target, Target::Path("/work/src/main.rs".into(), None));
        let l = link("  ⎿  Updated src/main.rs with 3 additions", 16).unwrap();
        assert_eq!(l.target, Target::Path("/work/src/main.rs".into(), None));
        let l = link("see `src/main.rs:994` there", 7).unwrap();
        assert_eq!(l.target, Target::Path("/work/src/main.rs".into(), Some((994, None))));
        let l = link("in src/main.rs:195-235.", 5).unwrap();
        assert_eq!(l.target, Target::Path("/work/src/main.rs".into(), Some((195, None))));
        let l = link("[the docs](https://docs.rs/egui/latest/egui/).", 20).unwrap();
        assert_eq!(l.target, Target::Url("https://docs.rs/egui/latest/egui/".into()));
        let l = link("**README.md** says", 4).unwrap();
        assert_eq!(l.target, Target::Path("/work/README.md".into(), None));
    }

    #[test]
    fn paths() {
        let l = link("error at src/main.rs:12:5: oops", 10).unwrap();
        assert_eq!(l.target, Target::Path("/work/src/main.rs".into(), Some((12, Some(5)))));
        assert_eq!(link("cat /etc/hosts", 6).unwrap().target, Target::Path("/etc/hosts".into(), None));
        assert_eq!(link("README.md  src", 2).unwrap().target, Target::Path("/work/README.md".into(), None));
        assert_eq!(link("README.md  src/", 12).unwrap().target, Target::Path("/work/src".into(), None));
        // Plain words and missing files aren't links.
        assert!(link("hello world", 2).is_none());
        assert!(link("missing.txt", 2).is_none());
    }
}
