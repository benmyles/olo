//! MIME types for files and URLs, from the shared-mime-info glob database
//! (what every freedesktop app uses) plus a little content sniffing for
//! files without a telling name.

use std::{
    collections::HashMap,
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

/// The glob database, indexed: most patterns are `*.ext`, found by a
/// name's suffixes in a few lookups; the rest are matched one by one.
struct Globs {
    all: Vec<Glob>,
    /// `*.ext` patterns by what follows the `*` (lowercased unless the
    /// pattern is case-sensitive, then in `exact`).
    folded: HashMap<String, Vec<usize>>,
    exact: HashMap<String, Vec<usize>>,
    /// The rest, matched one by one: each with its pattern as characters
    /// (lowercased unless case-sensitive), made once.
    others: Vec<(usize, Vec<char>)>,
}

fn globs() -> &'static Globs {
    static GLOBS: OnceLock<Globs> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let mut all = Vec::new();
        for dir in mime_dirs() {
            let Ok(text) = std::fs::read_to_string(dir.join("globs2")) else { continue };
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                // weight:type:pattern[:flags]
                let mut parts = line.splitn(4, ':');
                let (Some(w), Some(mime), Some(pattern)) = (parts.next(), parts.next(), parts.next()) else { continue };
                let flags = parts.next().unwrap_or("");
                all.push(Glob { weight: w.parse().unwrap_or(50), mime: mime.to_owned(), pattern: pattern.to_owned(), case_sensitive: flags.split(',').any(|f| f == "cs") });
            }
        }
        let (mut folded, mut exact, mut others) = (HashMap::<String, Vec<usize>>::new(), HashMap::<String, Vec<usize>>::new(), Vec::new());
        for (i, g) in all.iter().enumerate() {
            match g.pattern.strip_prefix('*').filter(|rest| rest.starts_with('.') && !rest.contains(['*', '?', '['])) {
                Some(rest) if g.case_sensitive => exact.entry(rest.to_owned()).or_default().push(i),
                Some(rest) => folded.entry(rest.to_lowercase()).or_default().push(i),
                None => others.push((i, if g.case_sensitive { g.pattern.chars().collect() } else { g.pattern.to_lowercase().chars().collect() })),
            }
        }
        Globs { all, folded, exact, others }
    })
}

/// A shared-mime-info table of `type other` lines (`subclasses`,
/// `aliases`), from every MIME directory.
fn table(file: &str) -> HashMap<String, Vec<String>> {
    let mut t = HashMap::<String, Vec<String>>::new();
    for dir in mime_dirs() {
        let Ok(text) = std::fs::read_to_string(dir.join(file)) else { continue };
        for (a, b) in text.lines().filter_map(|l| l.split_once(' ')) {
            t.entry(a.to_owned()).or_default().push(b.to_owned());
        }
    }
    t
}

/// The type `mime` is an alias of (`application/x-pdf`: `application/pdf`),
/// or itself.
fn canonical(mime: &str) -> String {
    static ALIASES: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    ALIASES.get_or_init(|| table("aliases")).get(mime).and_then(|v| v.first()).cloned().unwrap_or_else(|| mime.to_owned())
}

/// Whether a file of type `mime` is one of `of`: the same type (or an
/// alias), one of a family (`image/*`), or a kind of it by
/// shared-mime-info's subclasses (`text/x-csrc` is a `text/plain`, as
/// every text type is; everything is an `application/octet-stream`).
pub fn is_a(mime: &str, of: &str) -> bool {
    static SUBCLASSES: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    let of = canonical(of);
    if matches!(of.as_str(), "application/octet-stream" | "*/*" | "*") {
        return true;
    }
    let subclasses = SUBCLASSES.get_or_init(|| table("subclasses"));
    let family = of.strip_suffix("/*");
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![canonical(mime)];
    while let Some(m) = todo.pop() {
        if m == of || family.is_some_and(|f| m.split('/').next() == Some(f)) {
            return true;
        }
        if !seen.insert(m.clone()) {
            continue;
        }
        if m.starts_with("text/") && m != "text/plain" {
            todo.push("text/plain".into());
        }
        todo.extend(subclasses.get(&m).into_iter().flatten().map(|p| canonical(p)));
    }
    false
}

/// Every type shared-mime-info knows (its `types` lists), in order.
pub fn known() -> Vec<String> {
    let mut v: Vec<String> = mime_dirs()
        .iter()
        .filter_map(|d| std::fs::read_to_string(d.join("types")).ok())
        .flat_map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Whether `name` matches the shell glob `pattern` (`*`, `?`, `[...]`).
pub fn glob(pattern: &str, name: &str) -> bool {
    glob_match(&pattern.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
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
    // The person's own kinds first (Preferences ▸ Default Apps).
    if let Some(m) = own_kind_of(name) {
        return Some(m);
    }
    from_name_globs(name)
}

/// [`from_name`] by shared-mime-info's globs alone.
fn from_name_globs(name: &str) -> Option<String> {
    let db = globs();
    let lower = name.to_lowercase();
    // Each suffix from a dot (`.tar.gz`, `.gz`), looked up; then the rest.
    let suffixes = |s: &str| s.match_indices('.').map(|(i, _)| s[i..].to_owned()).collect::<Vec<_>>();
    let mut hits: Vec<usize> = suffixes(&lower).iter().filter_map(|k| db.folded.get(k)).flatten().copied().collect();
    hits.extend(suffixes(name).iter().filter_map(|k| db.exact.get(k)).flatten().copied());
    let (lower_chars, exact_chars): (Vec<char>, Vec<char>) = (lower.chars().collect(), name.chars().collect());
    hits.extend(db.others.iter().filter(|(i, p)| glob_match(p, if db.all[*i].case_sensitive { &exact_chars } else { &lower_chars })).map(|(i, _)| *i));
    hits.into_iter().map(|i| &db.all[i]).max_by_key(|g| (g.weight, g.pattern.len())).map(|g| g.mime.clone())
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
    let texty = utf8_prefix(head) && !head.iter().any(|&b| b == 0 || (b < 0x20 && !b"\t\n\r\x0c\x1b".contains(&b)));
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
        if globs().all.is_empty() {
            return;
        }
        assert_eq!(from_name("x.PNG").as_deref(), Some("image/png"));
        assert_eq!(from_name("a.tar.gz").as_deref(), Some("application/x-compressed-tar"));
        assert_eq!(from_name("notes.txt").as_deref(), Some("text/plain"));
        assert_eq!(from_name("Makefile").as_deref(), Some("text/x-makefile"));
        assert_eq!(from_name("libfoo.so.1").as_deref(), Some("application/x-sharedlib"));
        assert!(is_a("text/x-csrc", "text/plain"));
        assert!(is_a("image/png", "image/*"));
        assert!(is_a("application/x-pdf", "application/pdf"));
        assert!(!is_a("image/png", "text/plain"));
        assert!(is_a("image/png", "application/octet-stream"));
        // A Markdown file is text (a subclass of text/plain).
        assert!(is_a(&from_name("a.md").unwrap(), "text/plain"));
    }
}

// ---- the person's own kinds -------------------------------------------

/// A kind of file the person named themselves (Preferences ▸ Default Apps
/// ▸ Your Own Kinds): its name, the extensions it covers (without the
/// dot, lower case), and its type: the one its extensions already had,
/// else one of Olo's own (`application/x-olo-kind-EXT`), which Olo knows
/// by it ([`from_name`] reads these first) and opens with the app the
/// person chose for it (`open::set_default`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnKind {
    pub name: String,
    pub extensions: Vec<String>,
    pub mime: String,
}

/// Where they're kept: a line each, `name|ext;ext|type`.
pub fn own_kinds_file() -> PathBuf {
    crate::config_dir().join("kinds.list")
}

fn parse_kinds(text: &str) -> Vec<OwnKind> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.splitn(3, '|');
            let (name, exts, mime) = (parts.next()?.trim(), parts.next()?, parts.next()?.trim());
            let extensions: Vec<String> = exts.split(';').map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect();
            (!name.is_empty() && !extensions.is_empty() && mime.contains('/')).then(|| OwnKind { name: name.to_owned(), extensions, mime: mime.to_owned() })
        })
        .collect()
}

fn write_kinds(kinds: &[OwnKind]) -> String {
    let mut text = String::from("# Your own kinds of file (Preferences ▸ Default Apps): name|extensions|type\n");
    for k in kinds {
        text.push_str(&format!("{}|{}|{}\n", k.name.replace(['|', '\n'], " "), k.extensions.join(";"), k.mime));
    }
    text
}

type KeptKinds = olo_sync::Mutex<Option<(std::time::Instant, Option<std::time::SystemTime>, std::sync::Arc<Vec<OwnKind>>)>>;
static KEPT_KINDS: OnceLock<KeptKinds> = OnceLock::new();

/// The person's own kinds, as last read (the file is looked at at most
/// once a second: every file listed asks).
pub fn own_kinds() -> std::sync::Arc<Vec<OwnKind>> {
    let kept = KEPT_KINDS.get_or_init(Default::default);
    let mut k = kept.lock().unwrap();
    if let Some((looked, _, kinds)) = k.as_ref()
        && looked.elapsed() < std::time::Duration::from_secs(1)
    {
        return kinds.clone();
    }
    let path = own_kinds_file();
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Some((_, s, kinds)) = k.as_ref()
        && *s == stamp
    {
        let kinds = kinds.clone();
        *k = Some((std::time::Instant::now(), stamp, kinds.clone()));
        return kinds;
    }
    let kinds = std::sync::Arc::new(std::fs::read_to_string(&path).map(|t| parse_kinds(&t)).unwrap_or_default());
    *k = Some((std::time::Instant::now(), stamp, kinds.clone()));
    kinds
}

/// Keep `kinds` as the person's own.
pub fn set_own_kinds(kinds: &[OwnKind]) -> std::io::Result<()> {
    let path = own_kinds_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, write_kinds(kinds))?;
    std::fs::rename(&tmp, &path)?;
    // (Read again at once in this process.)
    if let Some(k) = KEPT_KINDS.get() {
        *k.lock().unwrap() = None;
    }
    Ok(())
}

/// The type a new kind covering `extensions` goes by: the one they all
/// already have (a known type given a name and another app), else Olo's
/// own, after the first (a known one mixed with others would take the
/// others' files with it: `.sav` is SPSS's too).
pub fn type_for_extensions(extensions: &[String]) -> String {
    let exts: Vec<String> = extensions.iter().map(|e| e.trim_start_matches('.').to_lowercase()).collect();
    let mut known = exts.iter().map(|e| from_name_globs(&format!("x.{e}")));
    let first = known.next().flatten();
    if let Some(t) = first.as_ref()
        && known.all(|k| k.as_ref() == Some(t))
    {
        return t.clone();
    }
    let ext = exts.first().map(String::as_str).unwrap_or_default();
    format!("application/x-olo-kind-{}", ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>())
}

/// The person's kind a file named `name` is, if it's one.
fn own_kind_of(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let (_, ext) = lower.rsplit_once('.')?;
    own_kinds().iter().find(|k| k.extensions.iter().any(|e| e == ext)).map(|k| k.mime.clone())
}

#[cfg(test)]
mod own_kinds_tests {
    use super::*;

    #[test]
    fn own_kinds_read_and_written() {
        let kinds = vec![
            OwnKind { name: "Game saves".into(), extensions: vec!["sav".into(), "sv2".into()], mime: "application/x-olo-kind-sav".into() },
            OwnKind { name: "Logs | old".into(), extensions: vec!["log".into()], mime: "text/x-log".into() },
        ];
        let text = write_kinds(&kinds);
        let back = parse_kinds(&text);
        assert_eq!(back[0], kinds[0]);
        assert_eq!(back[1].name, "Logs   old");
        // Extensions as typed: dots, case and blanks don't matter.
        assert_eq!(parse_kinds("Mine| .FOO ; bar;|application/x-olo-kind-foo")[0].extensions, ["foo", "bar"]);
        assert!(parse_kinds("no type|x|").is_empty() && parse_kinds("|x|a/b").is_empty());
        // A kind of extensions nothing knows is Olo's own type.
        assert_eq!(type_for_extensions(&["zzqx9".into()]), "application/x-olo-kind-zzqx9");
        // Known extensions of one type keep it; mixed with others, Olo's own.
        assert_eq!(type_for_extensions(&["jpg".into(), "JPEG".into()]), "image/jpeg");
        assert_eq!(type_for_extensions(&["jpg".into(), "zzqx9".into()]), "application/x-olo-kind-jpg");
    }
}
