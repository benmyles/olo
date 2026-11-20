//! Opening files and URLs with the right application, natively (what
//! `xdg-open` does, without running it): Olo's own app when one is
//! installed for the type, else the user's default from mimeapps.list,
//! else any installed app that lists the type.

use olo_sync::Mutex;
use std::{
    ffi::OsString,
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
};

use crate::{
    desktop_entry::{self, DesktopEntry},
    find_program, mime,
};

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
            Handler::Ours(bin) => crate::apps::our_app(bin.trim_start_matches("olo-")).map(|(n, _)| n.to_string()).unwrap_or_else(|| bin.trim_start_matches("olo-").to_owned()),
            Handler::Entry(e) => e.name.clone(),
        }
    }

    /// Its icon's name (or path, as a desktop entry may give one).
    pub fn icon(&self) -> String {
        match self {
            Handler::Ours(bin) => crate::apps::our_app(bin.trim_start_matches("olo-")).map_or("application", |(_, i)| i).to_owned(),
            Handler::Entry(e) => e.icon.clone().unwrap_or_else(|| "application".into()),
        }
    }
}

/// Olo's own app for a type, if it is installed.
fn ours(mime: &str) -> Option<&'static str> {
    ours_among(mime, |bin| find_program(bin).is_some())
}

/// [`ours`], among the programs `installed` says are there.
fn ours_among(mime: &str, installed: impl Fn(&str) -> bool) -> Option<&'static str> {
    ours_ranked(mime).into_iter().filter(|(rank, _)| *rank == 0).map(|(_, bin)| bin).find(|bin| installed(bin))
}

/// What one of Olo's apps opens. A kind is a type ("application/pdf")
/// or every type of a family ("text/*").
struct Opens {
    bin: &'static str,
    /// The kinds it is the app for: they open in it.
    opens: &'static [&'static str],
    /// Kinds whose every narrower kind it opens too (anything that is
    /// text, or XML): what opens in it where no installed app says it
    /// opens that very kind.
    narrower: &'static [&'static str],
    /// Kinds, and the narrower kinds of them, it is offered for (Open
    /// With) without being what they open in.
    also: &'static [&'static str],
}

/// What each of Olo's apps opens, said once, here: an app is offered in
/// Open With for every kind it names, the entries other programs read
/// are written from the same (`for_others`), and the app a kind opens in
/// is the first that names it in `opens`. An app that learns to open
/// something says so here and nowhere else. (It was a rule by kind, and
/// what an app opened had to be remembered in it: a Word document was
/// offered the Viewer and not StyledEdit, which opens and writes them,
/// and nothing of Olo's was offered for a YAML file, an EPUB, a 7z or an
/// old Excel sheet, all of which Olo opens. Ben, 2026-11-17: "an app
/// should register the extensions it supports (e.g. here StyledEdit
/// supports it) and appear in the Open With menu". The test
/// `what_olos_apps_open` holds every extension the apps read to it.)
const OPENS: &[Opens] = &[
    Opens { bin: "olo-tracker", opens: &["inode/directory"], narrower: &[], also: &[] },
    Opens { bin: "olo-chrome", opens: &["text/html", "application/xhtml+xml", "x-scheme-handler/http", "x-scheme-handler/https"], narrower: &[], also: &[] },
    Opens { bin: "olo-slack", opens: &["x-scheme-handler/slack"], narrower: &[], also: &[] },
    // A number to call (`tel:`): its card in Phone, where Call calls it.
    Opens { bin: "olo-phone", opens: &["x-scheme-handler/tel"], narrower: &[], also: &[] },
    Opens { bin: "olo-preferences", opens: &[crate::mime::SETTINGS_BACKUP], narrower: &[], also: &[] },
    Opens {
        bin: "olo-styled-edit",
        opens: &[
            "text/*",
            "application/x-shellscript",
            "application/json",
            "application/xml",
            "application/toml",
            "application/yaml",
            "application/x-yaml",
            "application/javascript",
            "application/sql",
            "application/rtf",
        ],
        narrower: &["text/plain", "application/json", "application/xml"],
        also: &["application/vnd.openxmlformats-officedocument.wordprocessingml.document", "application/vnd.oasis.opendocument.text"],
    },
    Opens {
        bin: "olo-viewer",
        opens: &[
            "image/*",
            "audio/*",
            "video/*",
            "font/*",
            "application/pdf",
            // Archives (`olo_archive::kind`), and what is one inside.
            "application/zip",
            "application/x-tar",
            "application/x-compressed-tar",
            "application/x-bzip2-compressed-tar",
            "application/x-xz-compressed-tar",
            "application/x-zstd-compressed-tar",
            "application/gzip",
            "application/x-bzip2",
            "application/x-xz",
            "application/zstd",
            "application/x-7z-compressed",
            "application/vnd.rar",
            "application/x-cpio",
            "application/x-cd-image",
            "application/vnd.efi.iso",
            "application/epub+zip",
            "application/java-archive",
            "application/vnd.android.package-archive",
            // Documents, sheets and decks (`olo_preview`).
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
            "application/vnd.openxmlformats-officedocument.presentationml.template",
            "application/vnd.ms-excel",
            "application/vnd.ms-excel.sheet.macroEnabled.12",
            "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
            "application/vnd.oasis.opendocument.text",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/vnd.oasis.opendocument.presentation",
        ],
        narrower: &[],
        // Anything that is text, it shows.
        also: &["text/*", "text/plain", "application/json", "application/xml", "application/rtf"],
    },
];

/// Olo's apps for `mime` ([`OPENS`]), each with how it has it: 0, the
/// app for the kind; 1, by a broader kind it takes the narrower ones
/// of; 2, offered.
fn ours_ranked(mime: &str) -> Vec<(u8, &'static str)> {
    let names = |kind: &str, t: &str| kind == t || kind.strip_suffix("/*").is_some_and(|family| t.split_once('/').is_some_and(|(major, _)| major == family));
    let broader = ancestors(mime);
    let in_chain = |kinds: &[&str]| kinds.iter().any(|k| names(k, mime) || broader.iter().any(|t| names(k, t)));
    let mut out: Vec<(u8, &'static str)> = Vec::new();
    for app in OPENS {
        if app.opens.iter().any(|k| names(k, mime)) {
            out.push((0, app.bin));
        } else if app.narrower.iter().any(|k| broader.iter().any(|t| t == k)) {
            out.push((1, app.bin));
        } else if in_chain(app.also) {
            out.push((2, app.bin));
        }
    }
    out.sort_by_key(|(rank, _)| *rank);
    out
}

/// Olo's apps that open `mime`, the one it opens in first, then the
/// others offered for it.
fn ours_for(mime: &str) -> Vec<&'static str> {
    ours_ranked(mime).into_iter().map(|(_, bin)| bin).collect()
}

/// Olo's apps that open files for other programs too: binary, what kind
/// of app it is.
const OPENERS: &[(&str, &str)] = &[("olo-viewer", "Image, Media and Document Viewer"), ("olo-styled-edit", "Text Editor"), ("olo-tracker", "File Manager")];

/// What marks a desktop entry as one [`for_others`] wrote.
pub const WRITTEN: &str = "X-Olo-Written=true";

/// What other programs need to open files in Olo's apps as Olo's own
/// [`handler`] does (xdg-desktop-portal's OpenURI, `gio open` and so
/// xdg-open, GTK's show-uri): a hidden desktop entry for each of Olo's
/// apps that opens files (file name, text), named by its app id
/// (`olo.viewer.desktop`), listing every type shared-mime-info knows that
/// Olo opens with it; and an `olo-mimeapps.list` making them the defaults
/// (read before `mimeapps.list` while `XDG_CURRENT_DESKTOP` names Olo).
/// Web pages and links stay the person's choice. `bin_dir` is where
/// Olo's programs are.
pub fn for_others(bin_dir: &Path) -> (Vec<(String, String)>, String) {
    let installed = |bin: &str| bin_dir.join(bin).is_file();
    let mut types: Vec<(&str, Vec<String>)> = OPENERS.iter().map(|(bin, _)| (*bin, Vec::new())).collect();
    for t in mime::known() {
        if is_web(&t) {
            continue;
        }
        if let Some(bin) = ours_among(&t, installed)
            && let Some((_, list)) = types.iter_mut().find(|(b, _)| *b == bin)
        {
            list.push(t);
        }
    }
    let mut entries = Vec::new();
    let mut defaults =
        String::from("# Written by olo-session: other programs open files in Olo's apps, as\n# Olo does (web pages and links stay with your default browser).\n[Default Applications]\n");
    for ((bin, kind), (_, list)) in OPENERS.iter().zip(&types) {
        let (Some(app_id), false) = (crate::apps::app_id_of(bin), list.is_empty()) else { continue };
        let (name, icon) = crate::apps::our_app(&app_id).unwrap_or((bin, "application"));
        let exec = bin_dir.join(bin);
        entries.push((
            format!("{app_id}.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nGenericName={kind}\nExec=\"{}\" %F\nIcon={icon}\nNoDisplay=true\nStartupWMClass={app_id}\nMimeType={};\n{WRITTEN}\n",
                exec.display(),
                list.join(";")
            ),
        ));
        for t in list {
            defaults.push_str(&format!("{t}={app_id}.desktop;\n"));
        }
    }
    // The person's own choices over Olo's (Preferences ▸ Default Apps).
    if let Ok(text) = std::fs::read_to_string(choices_file()) {
        let mut inside = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                inside = line == DEFAULTS;
            } else if inside && let Some((t, ids)) = line.split_once('=') {
                let ids: Vec<String> = ids.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
                defaults = with_listed(&defaults, DEFAULTS, t.trim(), &ids);
            }
        }
    }
    (entries, defaults)
}

/// Olo's Google Chrome's desktop entry (`apps/chrome`), in
/// `~/.local/share/applications`.
pub const CHROME_ENTRY: &str = "google-chrome.desktop";

/// Chrome offered as the default browser: what goes in `olo-mimeapps.list`
/// in `apps` (`~/.local/share/applications`), naming Olo's Chrome for web
/// pages and links, or None while it isn't installed there. That file is
/// read after every `mimeapps.list` in the config directories, so a
/// browser the person chose (Chrome's own "Make Default", another
/// browser's) still wins; it is only ahead of the system's lists, which
/// would otherwise pick whichever browser came with the system.
pub fn browser_offer(apps: &Path) -> Option<String> {
    apps.join(CHROME_ENTRY).is_file().then(|| {
        let mut text = String::from("# Written by Olo: web pages and links open in Google Chrome unless you\n# choose another browser (in ~/.config/mimeapps.list).\n[Default Applications]\n");
        for t in ["x-scheme-handler/http", "x-scheme-handler/https", "text/html", "application/xhtml+xml"] {
            text.push_str(&format!("{t}={CHROME_ENTRY};\n"));
        }
        text
    })
}

/// Write [`browser_offer`] in `apps` (or take it away once Chrome is
/// gone). True if it changed.
pub fn offer_browser(apps: &Path) -> io::Result<bool> {
    let path = apps.join("olo-mimeapps.list");
    match browser_offer(apps) {
        Some(text) => {
            if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
                return Ok(false);
            }
            let tmp = apps.join(".olo-mimeapps.list.new");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)?;
            Ok(true)
        }
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        },
    }
}

/// The `mimeinfo.cache` for the desktop entries in `dir` and below, as
/// update-desktop-database writes it (GIO finds which apps open a type
/// only through it): each type, and the entries that open it.
pub fn mime_cache(dir: &Path) -> String {
    fn walk(dir: &Path, prefix: &str, out: &mut std::collections::BTreeMap<String, Vec<String>>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for p in files {
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            if p.is_dir() {
                walk(&p, &format!("{prefix}{name}-"), out);
            } else if name.ends_with(".desktop")
                && let Some(e) = desktop_entry::parse(&p)
            {
                for t in e.mime_types {
                    out.entry(t).or_default().push(format!("{prefix}{name}"));
                }
            }
        }
    }
    let mut types = std::collections::BTreeMap::new();
    walk(dir, "", &mut types);
    let mut text = String::from("[MIME Cache]\n");
    for (t, ids) in types {
        text.push_str(&format!("{t}={};\n", ids.join(";")));
    }
    text
}

/// Links and pages: what a web browser opens.
fn is_web(mime: &str) -> bool {
    matches!(mime, "x-scheme-handler/http" | "x-scheme-handler/https" | "text/html" | "application/xhtml+xml")
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// mimeapps.list files, most important first.
fn mimeapps_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_lowercase).collect();
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
    let mut out: Vec<String> = mime::broader_of(mime).to_vec();
    if mime.starts_with("text/") && mime != "text/plain" && !out.iter().any(|p| p == "text/plain") {
        out.push("text/plain".into());
    }
    out
}

/// Every broader type `mime` is a kind of, nearest first (an Android
/// package is a Java archive, which is a zip).
fn ancestors(mime: &str) -> Vec<String> {
    let mut out = parents(mime);
    let mut k = 0;
    while k < out.len() && out.len() < 16 {
        for p in parents(&out[k]) {
            if p != mime && !out.contains(&p) {
                out.push(p);
            }
        }
        k += 1;
    }
    out
}

/// Who opens `mime`: the person's choice for it (Preferences ▸ Default
/// Apps, Always Open With), else Olo's own app, else the defaults
/// mimeapps.list names, else any app that says it opens it.
pub fn handler(mime: &str) -> Option<Handler> {
    if let Some(h) = chosen(mime) {
        return Some(h);
    }
    let files: Vec<String> = mimeapps_files().iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    // The web browser is the person's choice (Olo's own Chrome
    // otherwise); everything else opens in Olo's apps first.
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
    // Then whoever is the default for this very kind, or says it opens
    // it; then Olo's app that takes it as a narrower kind of what it
    // opens (a kind of text: StyledEdit); then the same for the broader
    // kinds, nearest first.
    let all = desktop_entry::all_cached();
    let of = |t: &str| -> Option<Handler> {
        let default = files.iter().flat_map(|text| defaults_in(text, t)).find_map(|file| entry_by_file(&file));
        default.or_else(|| all.iter().find(|e| e.mime_types.iter().any(|m| m == t)).cloned()).map(|e| Handler::Entry(Box::new(e)))
    };
    if let Some(h) = of(mime) {
        return Some(h);
    }
    if let Some((_, bin)) = ours_ranked(mime).into_iter().find(|(rank, bin)| *rank == 1 && find_program(bin).is_some()) {
        return Some(Handler::Ours(bin));
    }
    ancestors(mime).iter().find_map(|t| of(t))
}

// ---- the person's choices ---------------------------------------------

/// Where the person's own choices are kept, in mimeapps.list's form: the
/// app each kind opens in (`[Default Applications]`, Preferences ▸ Default
/// Apps or Open With ▸ Always Open With) and the apps Open With offers
/// for it, in order (`[Open With]`). Olo reads them before its own apps;
/// `olo-mimeapps.list` carries the defaults to other programs.
pub fn choices_file() -> PathBuf {
    config_home().join("olo/default-apps.list")
}

const DEFAULTS: &str = "[Default Applications]";
const OFFERED: &str = "[Open With]";

/// The ids listed for `mime` in `section` of a mimeapps-style `text`.
fn listed(text: &str, section: &str, mime: &str) -> Vec<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == section;
            continue;
        }
        if inside
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == mime
        {
            return v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        }
    }
    Vec::new()
}

/// `text` with `mime`'s line in `section` saying `ids` (none: taken out);
/// the section made if it isn't there.
fn with_listed(text: &str, section: &str, mime: &str, ids: &[String]) -> String {
    let line = (!ids.is_empty()).then(|| format!("{mime}={};", ids.join(";")));
    let mut out = Vec::new();
    let (mut inside, mut done, mut seen) = (false, false, false);
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if inside && !done {
                out.extend(line.clone());
                done = true;
            }
            inside = t == section;
            seen |= inside;
        } else if inside && t.split_once('=').is_some_and(|(k, _)| k.trim() == mime) {
            if !done {
                out.extend(line.clone());
                done = true;
            }
            continue;
        }
        out.push(l.to_owned());
    }
    if !done && line.is_some() {
        if !seen {
            out.push(section.to_owned());
        }
        out.extend(line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The id a handler goes by in mimeapps lists (`org.gnome.Loupe.desktop`,
/// `olo.viewer.desktop`).
pub fn handler_id(h: &Handler) -> Option<String> {
    match h {
        Handler::Ours(bin) => crate::apps::app_id_of(bin).map(|id| format!("{id}.desktop")),
        Handler::Entry(e) => Some(format!("{}.desktop", e.id)),
    }
}

/// The handler an id names: one of Olo's apps if installed, else its
/// desktop entry.
pub fn handler_of_id(id: &str) -> Option<Handler> {
    let stem = id.strip_suffix(".desktop").unwrap_or(id);
    if let Some(short) = stem.strip_prefix("olo.")
        && let Some(bin) = crate::apps::OUR_APPS.iter().map(|(b, _, _)| *b).find(|b| b.strip_prefix("olo-") == Some(short))
    {
        return find_program(bin).is_some().then_some(Handler::Ours(bin));
    }
    desktop_entry::all_cached().iter().find(|e| e.id == stem).cloned().map(|e| Handler::Entry(Box::new(e)))
}

/// The person's choice of app for `mime` (or a broader kind of it), if
/// they made one and it's still there.
fn chosen(mime: &str) -> Option<Handler> {
    let text = std::fs::read_to_string(choices_file()).ok()?;
    std::iter::once(mime.to_owned()).chain(parents(mime)).find_map(|t| listed(&text, DEFAULTS, &t).iter().find_map(|id| handler_of_id(id)))
}

/// Whether the person chose `mime`'s app themselves.
pub fn is_chosen(mime: &str) -> bool {
    std::fs::read_to_string(choices_file()).is_ok_and(|t| !listed(&t, DEFAULTS, mime).is_empty())
}

/// Make `handler` the app `mime` opens in (None: Olo's choice again),
/// for Olo and, through `olo-mimeapps.list`, for other programs.
pub fn set_default(mime: &str, handler: Option<&Handler>) -> io::Result<()> {
    let ids: Vec<String> = handler.and_then(handler_id).into_iter().collect();
    let path = choices_file();
    write_atomically(&path, &with_listed(&std::fs::read_to_string(&path).unwrap_or_default(), DEFAULTS, mime, &ids))?;
    // Others: the choice, or Olo's own app again.
    let others = config_home().join("olo-mimeapps.list");
    let theirs: Vec<String> = if ids.is_empty() { ours(mime).and_then(|b| handler_id(&Handler::Ours(b))).into_iter().collect() } else { ids };
    write_atomically(&others, &with_listed(&std::fs::read_to_string(&others).unwrap_or_default(), DEFAULTS, mime, &theirs))?;
    forget_handlers();
    Ok(())
}

/// The apps Open With offers for `mime`, in order, as the person set
/// them (none: Olo's own choice, [`handlers_for`]).
pub fn set_offered(mime: &str, handlers: &[Handler]) -> io::Result<()> {
    let ids: Vec<String> = handlers.iter().filter_map(handler_id).collect();
    let path = choices_file();
    write_atomically(&path, &with_listed(&std::fs::read_to_string(&path).unwrap_or_default(), OFFERED, mime, &ids))?;
    forget_handlers();
    Ok(())
}

/// Whether the person set which apps Open With offers for `mime`.
pub fn is_offered_set(mime: &str) -> bool {
    std::fs::read_to_string(choices_file()).is_ok_and(|t| !listed(&t, OFFERED, mime).is_empty())
}

/// How many apps Open With offers at most.
const OFFER: usize = 8;

/// The apps Open With offers for `mime`: as the person set them, else a
/// curated few: the one it opens in, Olo's own that open it, then those
/// whose desktop entries say they open it (or a broader kind of it), by
/// name; never a long list. Reads files: off the UI thread, or through
/// [`offer_cached`].
pub fn handlers_for(mime: &str) -> Vec<Handler> {
    let mut out: Vec<Handler> = Vec::new();
    let add = |h: Handler, out: &mut Vec<Handler>| {
        let id = handler_id(&h);
        if !out.iter().any(|o| handler_id(o) == id) {
            out.push(h);
        }
    };
    if let Ok(text) = std::fs::read_to_string(choices_file()) {
        let set = listed(&text, OFFERED, mime);
        if !set.is_empty() {
            for h in set.iter().filter_map(|id| handler_of_id(id)) {
                add(h, &mut out);
            }
            return out;
        }
    }
    if let Some(h) = handler(mime) {
        add(h, &mut out);
    }
    for bin in ours_for(mime).into_iter().filter(|b| find_program(b).is_some()) {
        add(Handler::Ours(bin), &mut out);
    }
    let mut types = vec![mime.to_owned()];
    types.extend(ancestors(mime));
    let all = desktop_entry::all_cached();
    let mut claimed: Vec<(usize, &DesktopEntry)> =
        all.iter().filter(|e| !e.no_display && e.exec.is_some() && !e.id.starts_with("olo.")).filter_map(|e| types.iter().position(|t| e.mime_types.contains(t)).map(|k| (k, e))).collect();
    claimed.sort_by_key(|(k, e)| (*k, e.name.to_lowercase()));
    for (_, e) in claimed {
        add(Handler::Entry(Box::new(e.clone())), &mut out);
    }
    out.truncate(OFFER);
    out
}

/// What Open With offers for a kind, worked out: the id of the app it
/// opens in, and the apps offered.
#[derive(Debug, Clone, Default)]
pub struct Offer {
    pub default: Option<String>,
    pub handlers: Vec<Handler>,
}

/// Every app that can open files, for Open With ▸ Other…: Olo's that
/// open others' files, then every installed app's desktop entry (shown in
/// menus, whose command takes files), by name. Reads files: off the UI
/// thread.
pub fn every_app() -> Vec<Handler> {
    let mut out: Vec<Handler> = OPENERS.iter().map(|(bin, _)| *bin).filter(|b| find_program(b).is_some()).map(Handler::Ours).collect();
    let all = desktop_entry::all_cached();
    let mut entries: Vec<&DesktopEntry> = all.iter().filter(|e| !e.no_display && e.takes_files() && !e.id.starts_with("olo.")).collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    out.extend(entries.into_iter().map(|e| Handler::Entry(Box::new(e.clone()))));
    out
}

/// What an offer was worked out from: the person's choices and the
/// applications installed, by when each last changed.
type Stamp = (Option<std::time::SystemTime>, Vec<Option<std::time::SystemTime>>);
type Offers = Mutex<std::collections::HashMap<String, (Stamp, Offer)>>;
static OFFERS: std::sync::OnceLock<Offers> = std::sync::OnceLock::new();

fn offer_stamp() -> Stamp {
    (std::fs::metadata(choices_file()).and_then(|m| m.modified()).ok(), desktop_entry::stamps())
}

/// [`handlers_for`] (and the default) as last worked out by [`warm`], if
/// it was and still holds (the choices changed since, here or in
/// another app, or an app was installed or removed: it doesn't): what a
/// menu built on the UI thread offers. A few `stat`s. None: to work out
/// ([`offer`]).
pub fn offer_cached(mime: &str) -> Option<Offer> {
    let stamp = offer_stamp();
    OFFERS.get_or_init(Default::default).lock().unwrap().get(mime).filter(|(s, _)| *s == stamp).map(|(_, o)| o.clone())
}

/// What Open With offers for `mime`: as kept, else worked out now and
/// kept (0.1 to 0.2 ms once the desktop entries are read, 6 ms the first
/// time in a program or after an app is installed).
pub fn offer(mime: &str) -> Offer {
    if let Some(o) = offer_cached(mime) {
        return o;
    }
    let stamp = offer_stamp();
    let offer = Offer { default: handler(mime).as_ref().and_then(handler_id), handlers: handlers_for(mime) };
    OFFERS.get_or_init(Default::default).lock().unwrap().insert(mime.to_owned(), (stamp, offer.clone()));
    offer
}

/// Work out what Open With offers for the kinds in `mimes` (a folder's:
/// off the UI thread), so a menu finds them at once.
pub fn warm(mimes: impl IntoIterator<Item = String>) {
    for m in mimes {
        offer(&m);
    }
}

/// The kind Open With goes by for a file named `name` (by its name
/// alone: no reading it, as a menu needs), or a folder.
pub fn kind_of(name: &str, is_dir: bool) -> String {
    if is_dir { "inode/directory".to_owned() } else { mime::from_name(name).unwrap_or_else(|| "application/octet-stream".to_owned()) }
}

/// The person changed a choice: what was worked out is worked out again.
fn forget_handlers() {
    if let Some(o) = OFFERS.get() {
        o.lock().unwrap().clear();
    }
    if let Some(c) = HANDLERS.get() {
        c.lock().unwrap().clear();
    }
}

type Handlers = Mutex<std::collections::HashMap<String, (std::time::Instant, Option<std::time::SystemTime>, Option<Handler>)>>;
static HANDLERS: std::sync::OnceLock<Handlers> = std::sync::OnceLock::new();

/// [`handler`], remembered per type for a while (it reads many files),
/// and until the person changes a choice (here or in another app).
pub fn handler_cached(mime: &str) -> Option<Handler> {
    const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
    let stamp = std::fs::metadata(choices_file()).and_then(|m| m.modified()).ok();
    let cache = HANDLERS.get_or_init(Default::default);
    if let Some((at, s, h)) = cache.lock().unwrap().get(mime)
        && at.elapsed() < FRESH
        && *s == stamp
    {
        return h.clone();
    }
    let h = handler(mime);
    cache.lock().unwrap().insert(mime.to_owned(), (std::time::Instant::now(), stamp, h.clone()));
    h
}

/// The command line for `entry` opening `target`, with the Exec field codes
/// (%f %F %u %U %i %c %k) expanded. Arguments are passed as-is, never
/// through a shell.
pub fn command_line(entry: &DesktopEntry, target: &Target) -> Option<Vec<OsString>> {
    command_line_for(entry, Some(target))
}

/// The command line that starts `entry` with nothing to open (Deskbar's
/// launch, a login item): the field codes dropped, quoting kept.
pub fn launch_line(entry: &DesktopEntry) -> Option<Vec<OsString>> {
    command_line_for(entry, None)
}

fn command_line_for(entry: &DesktopEntry, target: Option<&Target>) -> Option<Vec<OsString>> {
    let exec = entry.exec.as_ref()?;
    let (as_file, as_url): (Option<OsString>, Option<OsString>) = match target {
        Some(Target::Path(p)) => (Some(p.clone().into_os_string()), Some(format!("file://{}", p.display()).into())),
        Some(Target::Url(u)) => (Some(u.into()), Some(u.into())),
        None => (None, None),
    };
    let mut argv = Vec::new();
    let mut used = false;
    for word in split_exec(exec) {
        match word.as_str() {
            "%f" | "%F" => {
                argv.extend(as_file.clone());
                used = true;
            }
            "%u" | "%U" => {
                argv.extend(as_url.clone());
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
        argv.extend(as_file);
    }
    if entry.terminal {
        let mut t: Vec<OsString> = vec!["olo-terminal".into(), "-e".into()];
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
            let argv = command_line(&e, target).ok_or_else(|| io::Error::other(format!("{} has no Exec line", e.name)))?;
            return spawn_app(&argv, &e.id);
        }
        None => return Err(io::Error::new(io::ErrorKind::NotFound, format!("no application opens {mime}"))),
    };
    spawn(&argv)
}

/// Open `paths` with `handler` (dropped on an app in Deskbar): ours get
/// them all as arguments; a desktop entry gets them all at once if its
/// Exec takes several (%F %U), else is started once for each.
pub fn open_with(handler: &Handler, paths: &[PathBuf]) -> io::Result<()> {
    let Some(first) = paths.first() else { return Ok(()) };
    match handler {
        Handler::Ours(bin) => {
            let mut argv = vec![find_program(bin).map(PathBuf::into_os_string).unwrap_or_else(|| bin.into())];
            argv.extend(paths.iter().map(|p| p.clone().into_os_string()));
            spawn(&argv)
        }
        Handler::Entry(e) => {
            let exec = e.exec.as_deref().unwrap_or_default();
            if exec.contains("%F") || exec.contains("%U") {
                let mut argv = command_line(e, &Target::Path(first.clone())).ok_or_else(|| io::Error::other(format!("{} has no Exec line", e.name)))?;
                let url = |p: &PathBuf| -> OsString { format!("file://{}", p.display()).into() };
                let at = argv.iter().position(|a| *a == *first.as_os_str() || *a == url(first)).map_or(argv.len(), |i| i + 1);
                let as_url = exec.contains("%U");
                let rest: Vec<OsString> = paths[1..].iter().map(|p| if as_url { url(p) } else { p.clone().into_os_string() }).collect();
                argv.splice(at..at, rest);
                spawn_app(&argv, &e.id)
            } else {
                for p in paths {
                    let argv = command_line(e, &Target::Path(p.clone())).ok_or_else(|| io::Error::other(format!("{} has no Exec line", e.name)))?;
                    spawn_app(&argv, &e.id)?;
                }
                Ok(())
            }
        }
    }
}

/// Run `argv` detached (own session, no stdio), reaping it when it exits.
pub fn spawn(argv: &[OsString]) -> io::Result<()> {
    spawn_pid(argv).map(drop)
}

/// Run `argv` for desktop entry `app_id`, detached, in a scope of its own
/// (`scope`; not Olo's own programs).
pub fn spawn_app(argv: &[OsString], app_id: &str) -> io::Result<()> {
    let pid = spawn_pid(argv)?;
    if argv.first().is_some_and(|p| !crate::scope::ours(p)) {
        crate::scope::place(pid, app_id);
    }
    Ok(())
}

/// `spawn`, giving the process's id.
fn spawn_pid(argv: &[OsString]) -> io::Result<u32> {
    let (program, args) = argv.split_first().ok_or_else(|| io::Error::other("empty command"))?;
    let mut c = std::process::Command::new(program);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // (Its language, where we were started with none: `locale`.)
    crate::locale::give(&mut c);
    crate::electron::on_screen(&mut c, program);
    crate::quirks::apply(&mut c, program);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map(crate::reap_child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_listed_and_changed() {
        let text = "# mine\n[Default Applications]\nimage/png=org.gnome.Loupe.desktop;\ntext/plain=a.desktop;b.desktop;\n[Added Associations]\nimage/png=x.desktop;\n";
        assert_eq!(listed(text, DEFAULTS, "text/plain"), ["a.desktop", "b.desktop"]);
        assert_eq!(listed(text, DEFAULTS, "image/png"), ["org.gnome.Loupe.desktop"]);
        assert!(listed(text, OFFERED, "image/png").is_empty());
        // Changed in place, the rest as it was.
        let t = with_listed(text, DEFAULTS, "image/png", &["olo.viewer.desktop".into()]);
        assert_eq!(listed(&t, DEFAULTS, "image/png"), ["olo.viewer.desktop"]);
        assert_eq!(listed(&t, "[Added Associations]", "image/png"), ["x.desktop"]);
        assert!(t.starts_with("# mine\n[Default Applications]\nimage/png=olo.viewer.desktop;\ntext/plain="));
        // A new kind goes at its section's end; a new section is made.
        let t = with_listed(&t, DEFAULTS, "video/mp4", &["mpv.desktop".into()]);
        assert_eq!(listed(&t, DEFAULTS, "video/mp4"), ["mpv.desktop"]);
        assert!(t.find("video/mp4").unwrap() < t.find("[Added Associations]").unwrap());
        let t = with_listed(&t, OFFERED, "image/png", &["a.desktop".into(), "b.desktop".into()]);
        assert_eq!(listed(&t, OFFERED, "image/png"), ["a.desktop", "b.desktop"]);
        // Taken out.
        let t = with_listed(&t, DEFAULTS, "text/plain", &[]);
        assert!(listed(&t, DEFAULTS, "text/plain").is_empty() && !t.contains("text/plain="));
        assert_eq!(with_listed("", DEFAULTS, "a/b", &["c.desktop".into()]), "[Default Applications]\na/b=c.desktop;\n");
        assert_eq!(handler_id(&Handler::Ours("olo-viewer")).as_deref(), Some("olo.viewer.desktop"));
    }

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
        // Started with nothing to open: the codes go, the quoting stays.
        let e = DesktopEntry { exec: Some(r#""/opt/My App/run" --hidden %U"#.into()), ..Default::default() };
        assert_eq!(launch_line(&e).unwrap(), ["/opt/My App/run", "--hidden"]);
    }

    #[test]
    fn defaults_section() {
        let text = "[Added Associations]\nimage/png=no.desktop\n[Default Applications]\nimage/png=imv.desktop;other.desktop;\n";
        assert_eq!(defaults_in(text, "image/png"), ["imv.desktop", "other.desktop"]);
        assert!(defaults_in(text, "image/gif").is_empty());
    }

    #[test]
    fn olo_opens_for_others() {
        let dir = std::env::temp_dir().join(format!("olo-for-others-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for b in ["olo-viewer", "olo-styled-edit"] {
            std::fs::write(bin.join(b), "").unwrap();
        }
        let (entries, defaults) = for_others(&bin);
        // Tracker isn't there: no entry for it.
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["olo.viewer.desktop", "olo.styled-edit.desktop"]);
        let viewer = &entries[0].1;
        assert!(viewer.contains(&format!("Exec=\"{}\" %F", bin.join("olo-viewer").display())));
        assert!(viewer.contains("NoDisplay=true") && viewer.contains(WRITTEN));
        if !mime::known().is_empty() {
            assert!(viewer.contains("image/png;") && viewer.contains("application/pdf;"));
            assert!(defaults.contains("\ntext/x-csrc=olo.styled-edit.desktop;\n"));
            // Web pages and links aren't Olo's to take.
            assert!(!defaults.contains("text/html=") && !defaults.contains("x-scheme-handler/https"));
        }
        // The cache lists them, and entries in folders by their ids.
        let apps = dir.join("applications");
        std::fs::create_dir_all(apps.join("kde")).unwrap();
        for (n, t) in &entries {
            std::fs::write(apps.join(n), t).unwrap();
        }
        std::fs::write(apps.join("kde/okular.desktop"), "[Desktop Entry]\nType=Application\nName=Okular\nMimeType=application/pdf;\n").unwrap();
        std::fs::write(apps.join("gone.desktop"), "[Desktop Entry]\nType=Application\nName=Gone\nHidden=true\nMimeType=application/pdf;\n").unwrap();
        let cache = mime_cache(&apps);
        assert!(cache.starts_with("[MIME Cache]\n"));
        if !mime::known().is_empty() {
            assert!(cache.contains("\napplication/pdf=kde-okular.desktop;olo.viewer.desktop;\n"), "{cache}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn targets() {
        assert!(matches!(Target::parse("file:///tmp/a%20b"), Target::Path(p) if p == Path::new("/tmp/a b")));
        assert!(matches!(Target::parse("https://x"), Target::Url(_)));
        assert!(matches!(Target::parse("/etc/hosts"), Target::Path(_)));
    }
}

#[cfg(test)]
mod opens {
    use super::*;

    /// What each of Olo's apps opens is said once (`OPENS`): the one a
    /// kind opens in first, then the others offered for it.
    #[test]
    fn what_olos_apps_open() {
        const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        assert_eq!(ours_for(DOCX), ["olo-viewer", "olo-styled-edit"], "a Word document opens in the Viewer; StyledEdit, which opens and writes them, is offered");
        assert_eq!(ours_for("application/vnd.oasis.opendocument.text"), ["olo-viewer", "olo-styled-edit"]);
        assert_eq!(ours_for("text/plain"), ["olo-styled-edit", "olo-viewer"]);
        assert_eq!(ours_for("text/markdown"), ["olo-styled-edit", "olo-viewer"]);
        assert_eq!(ours_for("application/rtf"), ["olo-styled-edit", "olo-viewer"]);
        assert_eq!(ours_for("text/html"), ["olo-chrome", "olo-styled-edit", "olo-viewer"]);
        assert_eq!(ours_for("image/png"), ["olo-viewer"]);
        assert_eq!(ours_for("application/epub+zip"), ["olo-viewer"]);
        assert_eq!(ours_for("application/x-7z-compressed"), ["olo-viewer"]);
        assert_eq!(ours_for("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"), ["olo-viewer"]);
        assert_eq!(ours_for("inode/directory"), ["olo-tracker"]);
        assert_eq!(ours_for("x-scheme-handler/https"), ["olo-chrome"]);
        assert_eq!(ours_for("x-scheme-handler/tel"), ["olo-phone"]);
        assert_eq!(crate::mime::for_url("tel:+16505550100"), "x-scheme-handler/tel");
        assert!(ours_for("application/x-nothing-olo-opens").is_empty());
        // A name however long and dotted is the kind its last extension
        // says.
        assert_eq!(kind_of("Compare A.B and A.B - edits CLEAN (1) (1).docx", false), DOCX);
        // Broader kinds: a kind of text or of XML is StyledEdit's, after
        // the app for the kind itself; a picture that is XML stays the
        // Viewer's, and can be edited.
        assert_eq!(ours_ranked("application/x-desktop"), [(1, "olo-styled-edit"), (2, "olo-viewer")]);
        assert_eq!(ours_for("image/svg+xml"), ["olo-viewer", "olo-styled-edit"]);
        assert_eq!(ours("application/x-desktop"), None, "not said to other programs, nor before an app that says it opens the very kind");
        // Every app named is one of Olo's programs, and every kind is a
        // type or a family.
        for app in OPENS {
            assert!(app.bin.starts_with("olo-"), "{}", app.bin);
            for k in app.opens.iter().chain(app.narrower).chain(app.also) {
                assert!(k.split_once('/').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty()), "{}: {k}", app.bin);
            }
        }
        // Every extension the apps read (`olo_preview::load`, StyledEdit's
        // Open) is a kind its app is offered for, where this computer's
        // shared-mime-info knows the name at all.
        if mime::known().is_empty() {
            return;
        }
        let viewer = "svg svgz pdf pptx ppsx potx md markdown docx odt rtf odp xlsx xlsm xlsb xls ods csv tsv ttf otf ttc opus aiff wma mka mp3 flac ogg wav m4a mp4 mkv webm mov avi \
                      jar apk epub zip tar gz tgz bz2 xz zst 7z rar tar.gz tar.bz2 tar.xz tar.zst cpio iso png jpg jpeg gif webp avif heic tiff bmp ico jxl txt log json toml yaml xml html rs py sh c";
        let edit = "md markdown docx odt rtf txt log json toml yaml yml xml html rs py sh c h cpp js css csv sql";
        let mut missing = Vec::new();
        for (bin, exts) in [("olo-viewer", viewer), ("olo-styled-edit", edit)] {
            for ext in exts.split_whitespace() {
                let kind = kind_of(&format!("a.{ext}"), false);
                if kind != "application/octet-stream" && !ours_for(&kind).contains(&bin) {
                    missing.push(format!("{bin} .{ext} ({kind})"));
                }
            }
        }
        assert!(missing.is_empty(), "not offered for what they open: {missing:#?}");
        assert_eq!(kind_of("a.json", false), "application/json", "not the schema, which claims the name alike");
        assert_eq!(kind_of("a.otf", false), "font/otf");
    }

    /// How long a name's kind takes (`cargo test --release -p olo-xdg
    /// kind_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn kind_cost() {
        for name in ["notes.txt", "a.json", "a.py", "report.docx", "a.ogg", "archive.tar.gz", "README"] {
            let t = std::time::Instant::now();
            let mut n = 0;
            for _ in 0..100_000 {
                n += kind_of(std::hint::black_box(name), false).len();
            }
            println!("{name}: {:.0} ns a name ({})", t.elapsed().as_secs_f64() * 1e9 / 100_000.0, n / 100_000);
        }
    }

    /// How long working out one kind's Open With takes, cold (`cargo test
    /// --release -p olo-xdg open_with_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn open_with_cost() {
        for kind in ["application/vnd.openxmlformats-officedocument.wordprocessingml.document", "text/plain", "image/png", "application/x-nothing"] {
            let t = std::time::Instant::now();
            let n = handlers_for(kind).len();
            let d = handler(kind).is_some();
            println!("{kind}: {n} offered, a default {d}, in {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
        }
    }
}

#[cfg(test)]
mod live {
    /// Prints who opens a few types here. `cargo test -p olo-xdg -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn handlers() {
        for m in ["x-scheme-handler/https", "image/png", "text/x-rust", "inode/directory", "application/pdf", "application/x-iso9660-image"] {
            println!("{m}: {:?}", super::handler(m).map(|h| h.name()));
        }
    }

    /// What `for_others` makes of the installed programs, and how long it takes.
    #[test]
    #[ignore]
    fn for_others() {
        let bin = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/bin");
        let t = std::time::Instant::now();
        let (entries, defaults) = super::for_others(&bin);
        println!("{:?} for {} types", t.elapsed(), defaults.lines().count());
        for (n, e) in &entries {
            println!("{n}: {} bytes, {} types", e.len(), e.matches(';').count());
        }
        // OLO_CACHE_OF=DIR prints the cache that folder would get.
        if let Some(dir) = std::env::var_os("OLO_CACHE_OF") {
            print!("{}", super::mime_cache(std::path::Path::new(&dir)));
        }
        // OLO_FOR_OTHERS=HOME writes them there, to try with GIO.
        if let Some(home) = std::env::var_os("OLO_FOR_OTHERS").map(std::path::PathBuf::from) {
            let apps = home.join(".local/share/applications");
            std::fs::create_dir_all(&apps).unwrap();
            std::fs::create_dir_all(home.join(".config")).unwrap();
            for (n, e) in &entries {
                std::fs::write(apps.join(n), e).unwrap();
            }
            std::fs::write(apps.join("mimeinfo.cache"), super::mime_cache(&apps)).unwrap();
            std::fs::write(home.join(".config/olo-mimeapps.list"), defaults).unwrap();
        }
    }
}
