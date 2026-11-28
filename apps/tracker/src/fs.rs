//! Files as Tracker sees them: entries with a kind and an icon, plus the
//! operations (trash, copy, move, rename, open) behind its menus.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use olo_kit::SharedString;
use olo_xdg::{mime, open as opener};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    /// A link whose target is gone (deleted, moved, on a disk that isn't
    /// there): nothing to measure or open. Its Kind says "Broken Link",
    /// its Size nothing, and opening it says why it can't be (it showed
    /// the length of the target's name as its size, and opened an empty
    /// StyledEdit window: QA, 2026-11-28).
    pub broken: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: u32,
    pub kind: Kind,
    /// The icon of the Olo application this program (or a link to it)
    /// is.
    pub app_icon: Option<&'static str>,
    /// A download being written (Chrome's `NAME.crdownload`): shown as
    /// `NAME`, with how far along it is when it says how big it will be
    /// (`Some`, `olo_kit_base::files::EXPECTED_SIZE`).
    pub downloading: Option<Option<u64>>,
    /// Its texts in a list, made when it's first drawn.
    pub shown: Shown,
}

/// What a list shows of an [`Entry`] (its name, Size, Modified and
/// Kind), made the first time it's drawn and kept with it: a list drawn
/// again (a live resize, a theme) doesn't format them again; and what it
/// is, as a number (`Entry::look`), worked out the first time a view
/// asks whether it changed. An item changed on disk comes as a new
/// entry, so they're never stale. (Boxed: a folder's entries mostly
/// never show.)
#[derive(Clone, Default)]
pub struct Shown(std::sync::OnceLock<Box<ShownTexts>>, std::sync::OnceLock<u64>);

#[derive(Clone)]
pub struct ShownTexts {
    pub name: SharedString,
    pub size: SharedString,
    pub modified: SharedString,
    pub kind: SharedString,
}

/// (Made of the entry's other fields: two entries alike in those are
/// alike.)
impl PartialEq for Shown {
    fn eq(&self, _: &Shown) -> bool {
        true
    }
}

impl std::fmt::Debug for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Shown")
    }
}

impl Entry {
    /// What it is, as a number that differs from another entry's when
    /// anything a view shows of it from its own fields does (its path,
    /// size, date, kind, whether it's a link or a download): hashed once
    /// (`folder::row_key` adds what can change about it, its icon and
    /// mark).
    pub fn look(&self) -> u64 {
        *self.shown.1.get_or_init(|| olo_kit::style::key((&self.path, self.is_link, self.broken, self.size, self.modified, self.kind.name(), self.downloading)))
    }

    /// Its texts in a list, made the first time they're asked for.
    pub fn shown(&self) -> &ShownTexts {
        self.shown.0.get_or_init(|| {
            let kind = match (self.downloading, self.broken) {
                (None, false) => SharedString::new_static(self.kind.name()),
                _ => self.kind_text().into(),
            };
            Box::new(ShownTexts { name: self.label().to_owned().into(), size: self.size_text().into(), modified: format_time(self.modified).into(), kind })
        })
    }

    /// The name shown: a download's as it will be once done.
    pub fn label(&self) -> &str {
        match self.downloading {
            Some(_) => self.name.strip_suffix(olo_kit_base::files::DOWNLOADING).filter(|n| !n.is_empty()).unwrap_or(&self.name),
            None => &self.name,
        }
    }

    /// How far along a download being written is (0–1), if it knows.
    pub fn progress(&self) -> Option<f32> {
        let total = self.downloading.flatten().filter(|&t| t > 0)?;
        Some((self.size as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    /// Its Size column: a download's "3.1 of 10 MB"; nothing for a
    /// broken link, which has nothing to measure.
    pub fn size_text(&self) -> String {
        match (self.is_dir || self.broken, self.downloading.flatten()) {
            (true, _) => "—".into(),
            (false, Some(total)) => olo_kit_base::files::format_progress(self.size, total),
            (false, None) => format_size(self.size),
        }
    }

    /// Its Size column's text in `room` points: a download's "3.1 of 10
    /// MB" where that fits, else what's done so far, "3.1 MB" (its Kind
    /// says how far along it is), not "3.1 of 1…"; else `shown`'s.
    pub fn size_text_within(&self, room: f32) -> SharedString {
        let size = &self.shown().size;
        let font = olo_kit::style::ui_font(olo_kit::FontWeight::NORMAL);
        if self.downloading.is_some() && f32::from(olo_kit::style::text_width(size.clone(), olo_kit::style::PLAIN_SIZE, font)) > room {
            return format_size(self.size).into();
        }
        size.clone()
    }

    /// Its Kind column: "Downloading, 31%" for a download, "Broken Link"
    /// for a link whose target is gone.
    pub fn kind_text(&self) -> String {
        if self.broken {
            return olo_kit::tr!("Broken Link").into();
        }
        match (self.downloading, self.progress()) {
            (Some(_), Some(p)) => format!("Downloading, {}%", (p * 100.0).floor() as u32),
            (Some(_), None) => "Downloading".into(),
            (None, _) => self.kind.name().into(),
        }
    }
}

/// Why a link can't be opened, as it is when it's opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProblem {
    /// What it points to isn't there.
    Missing,
    /// What it points to is a link to something that isn't there.
    Chain,
    /// What it points to is where this person may not go.
    NoPermission,
    /// It points through links that lead round in a loop.
    Loop,
    /// Anything else (a disk that doesn't answer; logged).
    Unreachable,
    /// It isn't there itself any more (gone since it was listed).
    Gone,
}

/// A link that can't be opened: its label, the folder it is in, why,
/// and what it points to (`.` and `..` taken out), for an alert.
#[derive(Debug, Clone, PartialEq)]
pub struct Unopened {
    pub label: String,
    pub folder: Option<PathBuf>,
    pub problem: LinkProblem,
    pub target: Option<PathBuf>,
}

/// Why the link at `path` can't be opened, looked at now (not as it was
/// listed: what it points to may be back, or gone, since), with what it
/// points to; None where `path` isn't a link or what it points to is
/// there. It reads the disk: for a worker thread.
pub fn link_problem(path: &Path) -> Option<(LinkProblem, Option<PathBuf>)> {
    if !fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let e = fs::metadata(path).err()?;
    let problem = match (e.kind(), e.raw_os_error()) {
        // (A file where the way to it has a folder: as good as missing.)
        (std::io::ErrorKind::NotFound, _) | (_, Some(libc::ENOTDIR)) => LinkProblem::Missing,
        (std::io::ErrorKind::PermissionDenied, _) => LinkProblem::NoPermission,
        (_, Some(libc::ELOOP)) => LinkProblem::Loop,
        _ => {
            tracing::info!("link {}: {e}", path.display());
            LinkProblem::Unreachable
        }
    };
    // What it points to as written, joined to its folder: that is what's
    // looked at (with its "." and ".." taken out, "missing/../notes.txt"
    // was taken for the "notes.txt" beside it, which may well be there:
    // review, 2026-11-28); and shown so where taking them out could change
    // what it names (`collapses_safely`).
    let raw = fs::read_link(path).ok().map(|t| path.parent().map_or_else(|| t.clone(), |p| p.join(&t)));
    // (Where what it points to is there and a link itself, it's that
    // link's that's missing.)
    let problem = match &raw {
        Some(t) if problem == LinkProblem::Missing && fs::symlink_metadata(t).is_ok_and(|m| m.file_type().is_symlink()) => LinkProblem::Chain,
        _ => problem,
    };
    let target = raw.map(|t| if collapses_safely(&t) { lexical(&t) } else { t });
    Some((problem, target))
}

/// Whether taking `path`'s ".." out as written names what it names: each
/// ".." is after a folder that's there and is a folder, not a link (one
/// that isn't there, or a link to another, makes ".." mean something else
/// on the disk). It reads the disk: for a worker thread.
fn collapses_safely(path: &Path) -> bool {
    use std::path::Component;
    let mut at = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                if !fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
                    return false;
                }
                at.pop();
            }
            Component::CurDir => {}
            c => at.push(c),
        }
    }
    true
}

/// `path` with its "." and ".." taken out as written, not following any
/// link on the way (a link to "../Documents/a.txt" points to the
/// Documents beside its folder, and says so by that name).
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // ("/.." is "/".)
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            c => out.push(c),
        }
    }
    out
}

/// `entries` parted, each local one looked at as it is now (not as it
/// was listed: a file since a link to nothing, a link since gone): those
/// that can be opened, read again, and those that can't, and why. It reads
/// the disk: for a worker thread.
pub fn reachable(entries: Vec<Entry>) -> (Vec<Entry>, Vec<Unopened>) {
    let (mut fine, mut unopened) = (Vec::new(), Vec::new());
    for e in entries {
        if is_remote(&e.path) {
            fine.push(e);
            continue;
        }
        let unopened_as = |problem, target| Unopened { label: e.label().to_owned(), folder: e.path.parent().map(Path::to_owned), problem, target };
        match link_problem(&e.path) {
            Some((problem, target)) => unopened.push(unopened_as(problem, target)),
            // (Gone itself: not the row it was, opened as if there.)
            None => match entry(&e.path) {
                Some(fresh) => fine.push(fresh),
                None => unopened.push(unopened_as(LinkProblem::Gone, None)),
            },
        }
    }
    (fine, unopened)
}

/// What an alert says of links that can't be opened: which, in bold,
/// and why under it ("“a” can’t be opened." over "The item it points to
/// is missing: “b”.": both visual reviews, 2026-11-28), with what each
/// points to by name (and the folder that is in, where that isn't the
/// link's own); several together, once. None for none.
pub fn cannot_open_text(unopened: &[Unopened]) -> Option<String> {
    let (first, rest) = unopened.split_first()?;
    if rest.is_empty() {
        let name = first.target.as_ref().and_then(|t| t.file_name()).map(|n| n.to_string_lossy().into_owned());
        let folder = first.target.as_ref().and_then(|t| t.parent()).filter(|f| Some(*f) != first.folder.as_deref());
        let what = name.map(|name| match folder {
            Some(folder) => olo_kit::tr!("“{}” in {}", name, display_path(folder)),
            None => olo_kit::tr!("“{}”", name),
        });
        let why = match (first.problem, what) {
            // (The name last: a long one, which can't be broken, takes the
            // next line whole, not "is missing." left alone on it: Gemini's
            // look, 2026-11-28.)
            (LinkProblem::Missing, Some(what)) => olo_kit::tr!("The item it points to is missing: {}.", what),
            (LinkProblem::Missing, None) => olo_kit::tr!("The item it points to is missing.").to_owned(),
            (LinkProblem::Chain, Some(what)) => olo_kit::tr!("The link it points to, {}, points to an item that’s missing.", what),
            (LinkProblem::Chain, None) => olo_kit::tr!("The link it points to points to an item that’s missing.").to_owned(),
            (LinkProblem::NoPermission, Some(what)) => olo_kit::tr!("You don’t have permission to open the item it points to, {}.", what),
            (LinkProblem::NoPermission, None) => olo_kit::tr!("You don’t have permission to open the item it points to.").to_owned(),
            (LinkProblem::Loop, _) => olo_kit::tr!("It points through links that lead round in a loop.").to_owned(),
            (LinkProblem::Unreachable, Some(what)) => olo_kit::tr!("The item it points to, {}, can’t be reached.", what),
            (LinkProblem::Unreachable, None) => olo_kit::tr!("The item it points to can’t be reached.").to_owned(),
            (LinkProblem::Gone, _) => olo_kit::tr!("It’s no longer there.").to_owned(),
        };
        return Some(format!("{}\n\n{why}", olo_kit::tr!("“{}” can’t be opened.", first.label)));
    }
    let which = match rest {
        [second] => olo_kit::tr!("“{}” and “{}” can’t be opened.", first.label, second.label),
        _ => olo_kit::tr!("“{}” and {} other links can’t be opened.", first.label, rest.len()),
    };
    let all = |p: LinkProblem| unopened.iter().all(|u| u.problem == p);
    let why = if unopened.iter().all(|u| matches!(u.problem, LinkProblem::Missing | LinkProblem::Chain)) {
        olo_kit::tr!("The items they point to are missing.")
    } else if all(LinkProblem::NoPermission) {
        olo_kit::tr!("You don’t have permission to open the items they point to.")
    } else {
        olo_kit::tr!("The items they point to are missing or can’t be reached.")
    };
    Some(format!("{which}\n\n{why}"))
}

pub use olo_kit_base::files::{Kind, format_size, format_time, kind_for};
pub use olo_kit_base::tracker::{REX_SCHEME, RexPath};

/// Whether `path` addresses a remote Rex host (`rex://HOST/PATH`).
pub fn is_remote(path: &Path) -> bool {
    RexPath::is_rex(path)
}

/// Parse `path` as a `RexPath`, if it is one.
pub fn rex_path(path: &Path) -> Option<RexPath> {
    RexPath::from_path(path)
}

/// Parent directory of a local or `rex://` path.
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    if let Some(rex) = rex_path(path) {
        return rex.parent().map(|p| p.to_path_buf());
    }
    if is_remote(path) {
        return None;
    }
    path.parent().map(Path::to_owned)
}

/// Append a child segment to a local or `rex://` folder path.
pub fn join_name(dir: &Path, name: &str) -> PathBuf {
    if let Some(rex) = rex_path(dir) {
        return rex.join(name).to_path_buf();
    }
    dir.join(name)
}

/// Leaf name of a local or `rex://` path.
pub fn leaf_name(path: &Path) -> String {
    if let Some(rex) = rex_path(path) {
        return rex.file_name().to_owned();
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
}

/// Whether `path` equals `ancestor` or lies beneath it (handles both local and `rex://`).
pub fn starts_with_dir(path: &Path, ancestor: &Path) -> bool {
    match (rex_path(path), rex_path(ancestor)) {
        (Some(p), Some(a)) => {
            if !p.host.eq_ignore_ascii_case(&a.host) {
                return false;
            }
            if a.path == "/" {
                return p.path.starts_with('/');
            }
            p.path == a.path || p.path.strip_prefix(&a.path).is_some_and(|rest| rest.starts_with('/'))
        }
        (None, None) => path.starts_with(ancestor),
        _ => false,
    }
}

/// Child path segments from `ancestor` down to `path` (empty if equal, `None` if unrelated).
pub fn relative_segments(path: &Path, ancestor: &Path) -> Option<Vec<String>> {
    match (rex_path(path), rex_path(ancestor)) {
        (Some(p), Some(a)) => {
            if !p.host.eq_ignore_ascii_case(&a.host) {
                return None;
            }
            if p.path == a.path {
                return Some(Vec::new());
            }
            let tail = if a.path == "/" { p.path.strip_prefix('/')? } else { p.path.strip_prefix(&a.path)?.strip_prefix('/')? };
            Some(tail.split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect())
        }
        (None, None) => path.strip_prefix(ancestor).ok().map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()),
        _ => None,
    }
}

pub fn entry(path: &Path) -> Option<Entry> {
    if let Some(rex) = rex_path(path) {
        return crate::rex::stat(&rex).ok().and_then(|s| s.entry);
    }
    if is_remote(path) {
        return None;
    }
    let lmeta = fs::symlink_metadata(path).ok()?;
    let is_link = lmeta.file_type().is_symlink();
    // A link is what it points to; one pointing to nothing is broken,
    // and has only its own (its size the length of the target's name).
    let (meta, broken) = match is_link.then(|| fs::metadata(path)) {
        Some(Ok(target)) => (target, false),
        Some(Err(_)) => (lmeta, true),
        None => (lmeta, false),
    };
    let is_dir = meta.is_dir();
    let mode = meta.permissions().mode();
    // A download being written: where it goes once done, and how big it
    // will be if it says.
    let downloading = (!is_dir)
        .then(|| path.to_str()?.strip_suffix(olo_kit_base::files::DOWNLOADING).filter(|p| !p.ends_with('/')).map(PathBuf::from))
        .flatten()
        .map(|done| (done, olo_kit_base::files::expected_size(path)));
    Some(Entry {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
        path: path.to_owned(),
        is_dir,
        is_link,
        broken,
        size: if is_dir || broken { 0 } else { meta.size() },
        modified: meta.modified().ok(),
        mode,
        // A download is the kind it will be.
        kind: kind_for(downloading.as_ref().map_or(path, |(done, _)| done.as_path()), is_dir, mode),
        app_icon: if is_dir || broken { None } else { app_icon(path, is_link) },
        downloading: downloading.map(|(_, total)| total),
        shown: Shown::default(),
    })
}

/// One of our applications' icons, for its program or a link to it.
fn app_icon(path: &Path, is_link: bool) -> Option<&'static str> {
    let target = if is_link { fs::read_link(path).ok()? } else { path.to_owned() };
    let name = target.file_name()?.to_str()?;
    olo_xdg::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == name).map(|(_, _, icon)| *icon)
}

/// The app id of the running application program `e` is (or links to),
/// when it can open another window: New Window asks it for one.
pub fn running_app(e: &Entry) -> Option<String> {
    e.app_icon?;
    let target = if e.is_link { fs::read_link(&e.path).ok()? } else { e.path.clone() };
    let bin = target.file_name()?.to_str()?;
    if !olo_xdg::apps::NEW_WINDOW.contains(&bin) {
        return None;
    }
    let id = olo_xdg::apps::app_id_of(bin)?;
    olo_kit_base::instance::running(&id).then_some(id)
}

/// Apps whose windows aren't grouped, as app_server's settings last said
/// (kept by the settings watch, so menus never wait for app_server).
pub static UNGROUPED: olo_sync::Mutex<Vec<String>> = olo_sync::Mutex::new(Vec::new());

/// The app id of the application `e` is: one of our programs (or a link
/// to one), or a `.desktop` entry (its id, which Wayland apps use).
pub fn app_id(e: &Entry) -> Option<String> {
    if e.is_dir {
        return None;
    }
    if e.app_icon.is_some() {
        let target = if e.is_link { fs::read_link(&e.path).ok()? } else { e.path.clone() };
        return olo_xdg::apps::app_id_of(target.file_name()?.to_str()?);
    }
    e.name.strip_suffix(".desktop").filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Group Windows for `e`'s app: shown when it is an application, checked
/// while its windows are grouped.
pub fn grouping_item(menu: olo_kit::Menu, e: Option<&Entry>) -> olo_kit::Menu {
    let Some(app) = e.and_then(app_id) else { return menu };
    let grouped = !UNGROUPED.lock().is_ok_and(|u| u.contains(&app));
    menu.item("Group Windows", "group-windows").checked(grouped)
}

/// Group or ungroup `e`'s app's windows (saved in app_server's settings).
pub fn toggle_grouping(e: Option<&Entry>) {
    if let Some(app) = e.and_then(app_id) {
        let grouped = !UNGROUPED.lock().is_ok_and(|u| u.contains(&app));
        olo_kit_base::settings::set_groups_windows(&app, !grouped);
    }
}

/// Ask the running app `app_id` for another window, off the UI thread.
pub fn new_window(app_id: String) {
    std::thread::spawn(move || olo_kit_base::instance::forward(&app_id, &["--new-window".to_owned()]));
}

pub fn list(dir: &Path, hidden: bool) -> std::io::Result<Vec<Entry>> {
    list_counting(dir, hidden, &std::sync::atomic::AtomicU64::new(0))
}

/// [`list`], counting items into `count` as it goes (for progress).
pub fn list_counting(dir: &Path, hidden: bool, count: &std::sync::atomic::AtomicU64) -> std::io::Result<Vec<Entry>> {
    if dir == Path::new(REX_SCHEME) {
        let mut hosts = crate::rex::hosts().map_err(std::io::Error::other)?;
        hosts.sort_by(|a, b| b.state.is_connected().cmp(&a.state.is_connected()).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase())));
        let entries: Vec<Entry> = hosts
            .into_iter()
            .map(|h| Entry {
                path: RexPath::new(&h.label, "/").to_path_buf(),
                name: h.label,
                is_dir: true,
                is_link: false,
                broken: false,
                size: 0,
                modified: None,
                mode: 0o755,
                kind: Kind::Folder,
                app_icon: Some("network"),
                downloading: None,
                shown: Shown::default(),
            })
            .collect();
        count.fetch_add(entries.len() as u64, std::sync::atomic::Ordering::Relaxed);
        return Ok(entries);
    }
    if let Some(rex) = rex_path(dir) {
        let res = crate::rex::list(&rex, hidden).map_err(std::io::Error::other)?;
        count.fetch_add(res.entries.len() as u64, std::sync::atomic::Ordering::Relaxed);
        return Ok(res.entries);
    }
    let mut v = Vec::new();
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name();
        if !hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(en) = entry(&e.path()) {
            v.push(en);
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(v)
}

pub fn home() -> PathBuf {
    // (Read once: it's asked for every row drawn, and the environment
    // is searched from its start each time.)
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())).clone()
}

/// A path for display, with the home folder shortened to `~`.
pub fn display_path(p: &Path) -> String {
    if let Some(rex) = rex_path(p) {
        return rex.to_uri();
    }
    match p.strip_prefix(home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// A display name for a folder: "Home" for $HOME, the volume name for "/",
/// or `"folder — host"` for a remote Rex host folder.
pub fn display_name(path: &Path) -> String {
    if let Some(rex) = rex_path(path) {
        return rex.window_title();
    }
    if path == home() {
        return "Home".into();
    }
    if path == Path::new("/") {
        return volume_label();
    }
    if path == trash_files() {
        return "Trash".into();
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// The boot volume's name, as BeOS showed it on the desktop: the
/// computer's. (`OLO_TRACKER_DEMO_VOLUME=NAME` gives it another, for
/// pictures taken on a computer whose own name isn't theirs to show:
/// tools/dev/readme-shots on a dev server.)
pub fn volume_label() -> String {
    if let Some(name) = std::env::var("OLO_TRACKER_DEMO_VOLUME").ok().filter(|n| !n.trim().is_empty()) {
        return name.trim().to_owned();
    }
    fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && s != "localhost").unwrap_or_else(|| "System".into())
}

// ---- operations ---------------------------------------------------------

pub use olo_xdg::trash::{trash_dir, trash_files, trash_is_empty, trash_origin, trash_slot, unique};

/// Where a copy of `from` dropped into `dir` goes: its own name, or
/// "name copy" when copying into the folder it is already in.
pub fn copy_target(from: &Path, dir: &Path) -> PathBuf {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if from.parent() == Some(dir) { unique(dir, &copy_name(&name)) } else { unique(dir, &name) }
}

/// "notes copy.txt" for "notes.txt"; copying a copy gives "notes copy.txt"
/// again (made unique as "notes copy 2.txt"), never "notes copy copy.txt".
pub fn copy_name(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = base.strip_suffix(" copy ").or_else(|| stem.strip_suffix(" copy")).unwrap_or(stem);
    format!("{base} copy{ext}")
}

pub fn new_folder(dir: &Path) -> std::io::Result<PathBuf> {
    if let Some(rex) = rex_path(dir) {
        let (made, _) = crate::rex::mkdir(&rex.join("New Folder"), true).map_err(std::io::Error::other)?;
        return Ok(made.to_path_buf());
    }
    let p = unique(dir, "New Folder");
    fs::create_dir(&p)?;
    Ok(p)
}

pub fn rename(path: &Path, name: &str) -> std::io::Result<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return Err(std::io::Error::other("invalid name"));
    }
    if let Some(rex) = rex_path(path) {
        let parent = rex.parent().ok_or_else(|| std::io::Error::other("cannot rename root"))?;
        let to = parent.join(name);
        let moved = crate::rex::rename(&rex, &to, false).map_err(std::io::Error::other)?;
        return Ok(moved.to_path_buf());
    }
    let to = path.with_file_name(name);
    if to.exists() && to != path {
        return Err(std::io::Error::other("a file with that name already exists"));
    }
    fs::rename(path, &to)?;
    Ok(to)
}

pub fn make_link(path: &Path) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let name = format!("{} link", path.file_name().unwrap_or_default().to_string_lossy());
    let to = unique(dir, &name);
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// A link to `path` in `dir` ("name link" if `dir` is where it is).
pub fn link_into(path: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let to = if path.parent() == Some(dir) { unique(dir, &format!("{name} link")) } else { unique(dir, &name) };
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// A link to the program `path` in `dir`, named as the application is when
/// it is one of ours ("Terminal", not "olo-terminal").
pub fn app_link_into(path: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let name = olo_xdg::apps::OUR_APPS.iter().find(|(bin, _, _)| *bin == file).map_or(file.as_str(), |(_, name, _)| *name);
    let to = unique(dir, name);
    std::os::unix::fs::symlink(path, &to)?;
    Ok(to)
}

/// Text dropped on a folder, saved there as a clipping file named after
/// its first words (BeOS's Tracker made the same).
pub fn clipping(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let first: String = text.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
    let mut name: String = first.chars().filter(|c| *c != '/' && !c.is_control()).take(40).collect();
    name = name.trim_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        name = "Clipping".into();
    }
    let file_name = format!("{name} clipping.txt");
    if let Some(rex) = rex_path(dir) {
        let made = crate::rex::write_file(&rex.join(&file_name), true, text.as_bytes()).map_err(std::io::Error::other)?;
        return Ok(made.to_path_buf());
    }
    let to = unique(dir, &file_name);
    fs::write(&to, text)?;
    Ok(to)
}

/// A file dropped by its contents (`name`, `data`) into `dir`: under its
/// name, or a free one beside it.
pub fn save_new(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    if let Some(rex) = rex_path(dir) {
        let made = crate::rex::write_file(&rex.join(name), true, data).map_err(std::io::Error::other)?;
        return Ok(made.to_path_buf());
    }
    let to = unique(dir, name);
    fs::write(&to, data)?;
    Ok(to)
}

/// A link dropped on a folder: a clipping holding its address, named by
/// its text (or its site).
pub fn link_clipping(dir: &Path, url: &str, title: &str) -> std::io::Result<PathBuf> {
    let site = url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.");
    let words = if title.trim().is_empty() || title.trim() == url { site } else { title };
    let mut name: String = words.chars().filter(|c| *c != '/' && !c.is_control()).take(40).collect();
    name = name.trim_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        name = "Link".into();
    }
    let file_name = olo_kit::tr!("{} link clipping.txt", name);
    let body = format!("{url}\n");
    if let Some(rex) = rex_path(dir) {
        let made = crate::rex::write_file(&rex.join(&file_name), true, body.as_bytes()).map_err(std::io::Error::other)?;
        return Ok(made.to_path_buf());
    }
    let to = unique(dir, &file_name);
    fs::write(&to, body)?;
    Ok(to)
}

/// A picture pasted into a folder (`image/KIND` data from the
/// clipboard), saved there as "Pasted Picture" of its kind.
pub fn pasted_picture(dir: &Path, kind: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let ext = match kind.trim_start_matches("x-") {
        "jpeg" | "jpg" | "pjpeg" => "jpg",
        "svg+xml" => "svg",
        "vnd.microsoft.icon" | "icon" => "ico",
        other => other,
    };
    let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    let file_name = olo_kit::tr!("Pasted Picture.{}", if ext.is_empty() { "img" } else { &ext });
    if let Some(rex) = rex_path(dir) {
        let made = crate::rex::write_file(&rex.join(&file_name), true, data).map_err(std::io::Error::other)?;
        return Ok(made.to_path_buf());
    }
    let to = unique(dir, &file_name);
    fs::write(&to, data)?;
    Ok(to)
}

/// The name of the application `open` would use, when it is one of ours
/// (none for a broken link, which nothing opens).
pub fn opener_name(entry: &Entry) -> Option<String> {
    if entry.broken {
        return None;
    }
    match entry.kind {
        Kind::Folder => Some("Tracker".into()),
        Kind::Executable | Kind::Application => None,
        _ => opener::handler_cached(&mime::for_path(&entry.path)).map(|h| h.name()),
    }
}

/// Show any file in its own Viewer window.
/// A new Terminal tab in `dir` (in the Terminal window last in front;
/// a window if there is none).
pub fn open_in_terminal(dir: &Path) {
    let terminal = olo_xdg::find_program("olo-terminal").map_or_else(|| "olo-terminal".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[terminal, "--tab".into(), "-w".into(), dir.as_os_str().to_owned()]) {
        tracing::warn!("terminal in {}: {e}", dir.display());
    }
}

/// A new Rex workspace session in `dir` (or focuses the host in Rex for a remote folder).
pub fn open_in_rex(dir: &Path) {
    let rex = olo_xdg::find_program("olo-rex").map_or_else(|| "olo-rex".into(), PathBuf::into_os_string);
    if let Some(rp) = rex_path(dir) {
        if let Err(e) = opener::spawn(&[rex, "--focus-host".into(), rp.host.into()]) {
            tracing::warn!("rex host {}: {e}", dir.display());
        }
        return;
    }
    if let Err(e) = opener::spawn(&[rex, "--dir".into(), dir.as_os_str().to_owned()]) {
        tracing::warn!("rex in {}: {e}", dir.display());
    }
}

pub fn open_in_viewer(path: &Path) {
    if let Some(rp) = rex_path(path) {
        std::thread::spawn(move || match crate::rex::read_to_cache(&rp) {
            Ok(local) => {
                let viewer = olo_xdg::find_program("olo-viewer").map_or_else(|| "olo-viewer".into(), PathBuf::into_os_string);
                if let Err(e) = opener::spawn(&[viewer, local.into_os_string()]) {
                    tracing::warn!("viewer {}: {e}", rp.to_uri());
                }
            }
            Err(e) => crate::to_hub(crate::ToHub::Said(olo_kit::tr!("Couldn’t open “{}”: {}", rp.file_name(), e))),
        });
        return;
    }
    let viewer = olo_xdg::find_program("olo-viewer").map_or_else(|| "olo-viewer".into(), PathBuf::into_os_string);
    if let Err(e) = opener::spawn(&[viewer, path.as_os_str().to_owned()]) {
        tracing::warn!("viewer {}: {e}", path.display());
    }
}

/// Open a file with the right application: ours where we have one,
/// otherwise the user's default (see `olo_xdg::open`). Programs run. A
/// link that can't be opened says why instead (`open_files`).
pub fn open(entry: &Entry) {
    open_files(std::slice::from_ref(entry), None);
}

/// Opens files, each as `open` does, the links among them looked at
/// first as they are now, on a worker thread (`reachable`: reading a
/// link waits on its disk, and what it points to may have come back, or
/// gone, since it was listed): those that can't be opened are said in
/// one alert however many there are (a selection of broken links was a
/// stack of alerts: review, 2026-11-28), and the rest opened.
pub fn open_files(entries: &[Entry], from: Option<OpenedFrom>) {
    let (local, remote): (Vec<&Entry>, Vec<&Entry>) = entries.iter().partition(|e| !is_remote(&e.path));
    for e in remote {
        open_now(e);
    }
    if local.is_empty() {
        return;
    }
    let local: Vec<Entry> = local.into_iter().cloned().collect();
    std::thread::spawn(move || {
        let (fine, unopened) = reachable(local);
        // A folder now (a link to one that wasn't there when it was
        // listed): as a folder of the selection goes, in the window it
        // was opened from where it's the only one, else a window of its
        // own (it opened a window even for a lone folder in an existing
        // one: review, 2026-11-28).
        let folders = from.as_ref().map_or(0, |f| f.folders) + fine.iter().filter(|e| e.is_dir).count();
        for e in &fine {
            match (&from, e.is_dir) {
                (_, false) => open_now(e),
                (Some(f), true) if folders == 1 => crate::to_hub(crate::ToHub::GoIn { win: f.win, path: e.path.clone() }),
                (Some(f), true) => crate::to_hub(crate::ToHub::Open { path: e.path.clone(), from: Some(f.path.clone()) }),
                (None, true) => crate::to_hub(crate::ToHub::Open { path: e.path.clone(), from: None }),
            }
        }
        if let Some(why) = cannot_open_text(&unopened) {
            cannot_open(why);
        }
    });
}

/// The folder window a selection was opened in (`open_files`): its id
/// and folder, and how many folders of the selection it opened itself
/// (as they were listed).
#[derive(Clone, Debug)]
pub struct OpenedFrom {
    pub win: u64,
    pub path: PathBuf,
    pub folders: usize,
}

/// Shows `entry` in Viewer, looked at as it is now, on a worker (a link
/// to nothing since it was listed is said so, not handed to Viewer).
pub fn open_in_viewer_checked(entry: &Entry) {
    let entry = entry.clone();
    std::thread::spawn(move || {
        let (fine, unopened) = reachable(vec![entry]);
        if let Some(why) = cannot_open_text(&unopened) {
            return cannot_open(why);
        }
        if let Some(e) = fine.first() {
            open_in_viewer(&e.path);
        }
    });
}

/// Opens `entry`, a file, as it is (`open`).
fn open_now(entry: &Entry) {
    if let Some(rp) = rex_path(&entry.path) {
        let name = entry.name.clone();
        std::thread::spawn(move || match crate::rex::read_to_cache(&rp) {
            Ok(local) => {
                if let Err(e) = opener::open(&opener::Target::Path(local)) {
                    crate::to_hub(crate::ToHub::Said(olo_kit::tr!("Couldn’t open “{}”: {}", name, e)));
                }
            }
            Err(e) => crate::to_hub(crate::ToHub::Said(olo_kit::tr!("Couldn’t open “{}”: {}", name, e))),
        });
        return;
    }
    // An archive Olo reads is looked into in Tracker, as a folder is.
    if !entry.is_dir && entry.kind == Kind::Archive && opens_inside(&entry.name) {
        crate::to_hub(crate::ToHub::OpenArchive(entry.path.clone()));
        return;
    }
    let (path, program) = (entry.path.clone(), matches!(entry.kind, Kind::Executable | Kind::Application));
    // Finding the handler reads files: not on the UI thread.
    std::thread::spawn(move || {
        let r = if program { opener::spawn(&[path.clone().into_os_string()]) } else { opener::open(&opener::Target::Path(path.clone())) };
        if let Err(e) = r {
            tracing::warn!("open {}: {e}", path.display());
        }
    });
}

/// Says why something can't be opened, in a BeOS warning alert (from any
/// thread: an alert of the program's own).
pub fn cannot_open(why: String) {
    let _ = olo_kit::widgets::open_app_alert("Tracker", "warning", &why, &["OK"], |_| {});
}

/// Whether an archive by this name is one Tracker looks into (not a
/// package: an .rpm or .deb opens as it always has).
fn opens_inside(name: &str) -> bool {
    let n = name.to_lowercase();
    !(n.ends_with(".rpm") || n.ends_with(".deb"))
}

/// Recursive size of a folder and the number of items (files and folders)
/// inside it, for Get Info; capped by `limit` entries.
pub fn dir_size(path: &Path, limit: &mut u32) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            if *limit == 0 {
                break;
            }
            *limit -= 1;
            if let Ok(m) = e.metadata() {
                if m.is_dir() {
                    let (b, f) = dir_size(&e.path(), limit);
                    bytes += b;
                    files += f + 1; // the folder is an item too
                } else {
                    bytes += m.len();
                    files += 1;
                }
            }
        }
    }
    (bytes, files)
}

/// Mounted volumes worth showing on the desktop: the root and anything
/// under /run/media or /media.
pub fn volumes() -> Vec<(String, PathBuf, &'static str)> {
    let mut v = vec![(volume_label(), PathBuf::from("/"), "disk")];
    if let Ok(m) = fs::read_to_string("/proc/mounts") {
        for line in m.lines() {
            let mut f = line.split_whitespace();
            let (_dev, mnt) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            let mnt = mnt.replace("\\040", " ");
            if mnt.starts_with("/run/media/") || mnt.starts_with("/media/") {
                let p = PathBuf::from(&mnt);
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(mnt.clone());
                v.push((name, p, "usb"));
            }
        }
    }
    v
}

/// Free / total bytes of the file system holding `path`.
pub fn fs_space(path: &Path) -> Option<(u64, u64)> {
    if is_remote(path) {
        return None;
    }
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct we own.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64 * st.f_frsize as u64, st.f_blocks as u64 * st.f_frsize as u64))
    }
}

pub fn permissions_string(mode: u32, is_dir: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_dir { 'd' } else { '-' });
    for shift in [6, 3, 0] {
        let m = (mode >> shift) & 7;
        s.push(if m & 4 != 0 { 'r' } else { '-' });
        s.push(if m & 2 != 0 { 'w' } else { '-' });
        s.push(if m & 1 != 0 { 'x' } else { '-' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lists_texts_are_made_once_and_kept() {
        let e = Entry {
            path: PathBuf::from("/f/notes.txt"),
            name: "notes.txt".into(),
            is_dir: false,
            is_link: false,
            broken: false,
            size: 2048,
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            mode: 0o644,
            kind: Kind::Text,
            app_icon: None,
            downloading: None,
            shown: Shown::default(),
        };
        let fresh = e.clone();
        let s = e.shown();
        assert_eq!((s.name.as_str(), s.size.as_str(), s.kind.as_str()), ("notes.txt", "2.0 KB", "Text"));
        assert_eq!(s.modified.as_str(), format_time(e.modified));
        // Made once: asked again, the same texts.
        assert!(std::ptr::eq(e.shown(), s));
        // An entry whose texts were made is the entry it was.
        assert_eq!(e, fresh);
        // A copy keeps them.
        assert_eq!(e.clone().shown().size.as_str(), "2.0 KB");
    }

    #[test]
    fn downloads_show_as_they_will_be() {
        let dir = std::env::temp_dir().join(format!("olo-tracker-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Report.pdf.crdownload");
        std::fs::write(&p, vec![0u8; 3 * 1024 * 1024 + 100_000]).unwrap();
        let e = entry(&p).unwrap();
        assert_eq!((e.label(), e.kind, e.downloading), ("Report.pdf", Kind::Pdf, Some(None)));
        assert_eq!(e.kind_text(), "Downloading");
        if olo_kit_base::files::set_expected_size(&p, 10 * 1024 * 1024) {
            let e = entry(&p).unwrap();
            assert_eq!(e.kind_text(), "Downloading, 30%");
            assert_eq!(e.size_text(), "3.1 of 10 MB");
        }
        // Only a name with something before the suffix.
        let odd = dir.join(".crdownload");
        std::fs::write(&odd, b"x").unwrap();
        assert_eq!(entry(&odd).unwrap().downloading, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder of the test's own under /var/tmp, made new (never one
    /// that was there: another test's, of a process with the same id in
    /// another namespace, isn't taken or deleted); the test deletes it.
    pub(crate) fn fresh_test_dir(prefix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        loop {
            let dir = PathBuf::from(format!("/var/tmp/{prefix}-{}-{now}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("a test folder: {e}"),
            }
        }
    }

    /// A link is what it points to; one whose target is gone says "Broken
    /// Link", has no size (not its target's name's length) and can't be
    /// opened, saying why, as it is when it's opened.
    #[test]
    fn broken_links_say_so() {
        let dir = fresh_test_dir("olo-tracker-links-test");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let target = dir.join("notes.txt");
        std::fs::write(&target, b"hello").unwrap();
        let (good, bad) = (dir.join("good.txt"), dir.join("broken_link.txt"));
        std::os::unix::fs::symlink(&target, &good).unwrap();
        std::os::unix::fs::symlink(dir.join("does_not_exist_target.txt"), &bad).unwrap();
        let e = entry(&good).unwrap();
        assert!(e.is_link && !e.broken);
        assert_eq!((e.size, e.kind, e.size_text().as_str(), e.kind_text().as_str()), (5, Kind::Text, "5 bytes", "Text"));
        assert_eq!(link_problem(&good), None);
        assert_eq!(link_problem(&target), None, "not a link");
        let b = entry(&bad).unwrap();
        assert!(b.is_link && b.broken);
        assert_eq!((b.size, b.size_text().as_str(), b.kind_text().as_str()), (0, "—", "Broken Link"));
        assert_eq!((b.shown().size.as_str(), b.shown().kind.as_str()), ("—", "Broken Link"));
        assert_ne!(b.look(), entry(&good).unwrap().look());
        assert_eq!(opener_name(&b), None);
        let why = |paths: &[&Path]| cannot_open_text(&reachable(paths.iter().map(|p| entry(p).unwrap()).collect()).1);
        assert_eq!(why(&[&bad]).unwrap(), "“broken_link.txt” can’t be opened.\n\nThe item it points to is missing: “does_not_exist_target.txt”.");
        assert_eq!(why(&[&good]), None);
        // To another folder, written relative with "." and ".." through
        // folders that are there: that folder said too, as it is, not as
        // written.
        let far = dir.join("inner/far.txt");
        std::os::unix::fs::symlink("./../inner/../away/./x.txt", &far).unwrap();
        let (_, unopened) = reachable(vec![entry(&far).unwrap()]);
        assert_eq!(unopened[0].target.as_deref(), Some(dir.join("away/x.txt").as_path()));
        let text = cannot_open_text(&unopened).unwrap();
        assert!(text.ends_with(&format!("missing: “x.txt” in {}.", display_path(&dir.join("away")))), "{text}");
        // Through a folder that isn't there: ".." after it isn't the
        // folder it's in, so it's looked at and shown as written; and the
        // "notes.txt" it would name taken out, there and a file, doesn't
        // make it a link to a link.
        let odd = dir.join("odd.txt");
        std::os::unix::fs::symlink("missing-dir/../notes.txt", &odd).unwrap();
        let (_, unopened) = reachable(vec![entry(&odd).unwrap()]);
        assert_eq!(unopened[0].problem, LinkProblem::Missing);
        assert_eq!(unopened[0].target.as_deref(), Some(dir.join("missing-dir/../notes.txt").as_path()));
        // Gone itself since it was listed: said so, not opened as the row
        // it was; a file since a link to nothing: looked at as it is.
        let gone_link = dir.join("gone_link.txt");
        std::os::unix::fs::symlink(&target, &gone_link).unwrap();
        let listed = entry(&gone_link).unwrap();
        std::fs::remove_file(&gone_link).unwrap();
        let (fine, unopened) = reachable(vec![listed]);
        assert!(fine.is_empty() && unopened[0].problem == LinkProblem::Gone, "{unopened:?}");
        assert!(cannot_open_text(&unopened).unwrap().ends_with("It’s no longer there."));
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, b"x").unwrap();
        let listed = entry(&plain).unwrap();
        assert!(!listed.is_link);
        std::fs::remove_file(&plain).unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere.txt"), &plain).unwrap();
        let (fine, unopened) = reachable(vec![listed]);
        assert!(fine.is_empty() && unopened[0].problem == LinkProblem::Missing, "{unopened:?}");
        // Beside it, by "..": the same folder as the link's, so not said.
        let near = dir.join("inner/near.txt");
        std::os::unix::fs::symlink("../inner/gone.txt", &near).unwrap();
        assert!(why(&[&near]).unwrap().ends_with("is missing: “gone.txt”."));
        // As it is when opened, not as listed: back since, it opens; gone
        // since, it says so.
        let listed = entry(&bad).unwrap();
        std::fs::write(dir.join("does_not_exist_target.txt"), b"back").unwrap();
        let (fine, unopened) = reachable(vec![listed]);
        assert!(unopened.is_empty() && fine.len() == 1 && !fine[0].broken && fine[0].size == 4, "{fine:?}");
        let listed = entry(&good).unwrap();
        std::fs::remove_file(&target).unwrap();
        let (fine, unopened) = reachable(vec![listed]);
        assert!(fine.is_empty() && unopened[0].problem == LinkProblem::Missing);
        // A loop of links, and a folder this person may not go into.
        let looped = dir.join("loop.txt");
        std::os::unix::fs::symlink("loop.txt", &looped).unwrap();
        assert_eq!(link_problem(&looped).map(|(p, _)| p), Some(LinkProblem::Loop));
        // A link to a broken link: what it points to is there, a link to
        // what isn't.
        let chain = dir.join("chain.txt");
        std::os::unix::fs::symlink("inner/near.txt", &chain).unwrap();
        assert_eq!(
            why(&[&chain]).unwrap(),
            "“chain.txt” can’t be opened.\n\nThe link it points to, “near.txt” in inner, points to an item that’s missing.".replace("inner", &display_path(&dir.join("inner")))
        );
        let shut = dir.join("shut");
        std::fs::create_dir(&shut).unwrap();
        std::fs::write(shut.join("secret.txt"), b"x").unwrap();
        let barred = dir.join("barred.txt");
        std::os::unix::fs::symlink(shut.join("secret.txt"), &barred).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        // (Root goes anywhere: there it isn't barred.)
        let barred_problem = link_problem(&barred).map(|(p, _)| p);
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(barred_problem, Some(LinkProblem::NoPermission));
        }
        // Several: said together, once, and only those (`good`'s target
        // back).
        std::fs::write(&target, b"hello").unwrap();
        let two = why(&[&near, &far]).unwrap();
        assert_eq!(two, "“near.txt” and “far.txt” can’t be opened.\n\nThe items they point to are missing.");
        let three = why(&[&near, &far, &looped, &good]).unwrap();
        assert_eq!(three, "“near.txt” and 2 other links can’t be opened.\n\nThe items they point to are missing or can’t be reached.");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_names() {
        assert_eq!(copy_name("notes.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy.txt"), "notes copy.txt");
        assert_eq!(copy_name("notes copy 12.txt"), "notes copy.txt");
        assert_eq!(copy_name("photo 3.png"), "photo 3 copy.png");
        assert_eq!(copy_name("2024"), "2024 copy");
        assert_eq!(copy_name(".bashrc"), ".bashrc copy");
    }
}
