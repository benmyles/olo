//! Freedesktop plumbing shared by app_server and the kit (which re-exports
//! it): Olo's data directories, `.desktop` entries, and icon lookup
//! (Olo's own set in the current [`themes`] theme first: an icon's
//! drawing, `svg/NAME.svg`, rendered for the size asked ([`svg`]), else
//! its pre-scaled pictures; then freedesktop icon themes) and decoding.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub mod apps;
pub mod desktop_entry;
pub mod electron;
pub mod legacy;
pub mod locale;
pub mod mime;
pub mod open;
pub mod quirks;
pub mod realtime;
pub mod scope;
pub mod session;
pub mod settings;
pub mod svg;
pub mod themes;
pub mod trash;
pub mod wallpapers;

/// The device-pixel sizes Olo's icons are given to other programs at
/// (the icon theme app_server writes: a drawing is rendered at any size,
/// a freedesktop theme holds a few).
pub const ICON_SIZES: &[u32] = &[16, 24, 32, 48, 64, 96, 128, 256];

/// Olo's own drawing of icon `name` for theme `theme` (the theme's own,
/// else the classic set's, which the theme shows in its colours), if
/// Olo has one.
pub fn own_icon(theme: &str, name: &str) -> Option<PathBuf> {
    themes::icon_dirs(theme).map(|d| d.join("svg").join(format!("{name}.svg"))).find(|p| p.is_file())
}

/// Olo's own icon `name` rendered at `px` device pixels in theme
/// `theme`, as Olo shows it, and not kept: for what's written somewhere
/// of its own (the icon theme other programs read, a web page).
pub fn render_own_icon(theme: &str, name: &str, px: u32) -> Option<Image> {
    let path = own_icon(theme, name)?;
    let drawing = std::fs::read(&path).ok()?;
    match themes::recolour(theme).filter(|_| !themes::is_its_own(theme, &path)) {
        Some(rules) => svg::render(rules.apply(name, std::str::from_utf8(&drawing).ok()?).as_bytes(), px, true),
        None => svg::render(&drawing, px, true),
    }
}

/// What changes when any of Olo's icons `names` would be drawn
/// differently in theme `theme`: the drawings' files (which, changed
/// when, how long) and the theme's rules for them.
pub fn own_icons_stamp<'a>(theme: &str, names: impl Iterator<Item = &'a str>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    themes::recolour(theme).map(svg::Recolour::key).hash(&mut h);
    for name in names {
        let path = own_icon(theme, name);
        let meta = path.as_ref().and_then(|p| std::fs::metadata(p).ok());
        (path, meta.as_ref().map(std::fs::Metadata::len), meta.and_then(|m| m.modified().ok())).hash(&mut h);
    }
    h.finish()
}

/// Directories holding Olo's data (icons/, backgrounds/, ...).
pub fn data_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut v = Vec::new();
        if let Some(d) = std::env::var_os("OLO_DATA") {
            v.push(PathBuf::from(d));
        }
        // Development: target/<profile>/<bin> → <repo>/data; a test's
        // binary can be deeper (target/debug/build/<crate>/<hash>/out/,
        // as kache lays them out): looking only four up, tests read the
        // installed data instead, stale or missing (the veil's tests
        // passed on the old logo's installed files).
        if let Ok(exe) = std::env::current_exe() {
            for up in exe.ancestors().skip(1).take(7) {
                let d = up.join("data");
                if d.join("icons").is_dir() {
                    v.push(d);
                    break;
                }
            }
        }
        // Run by cargo (`cargo test`, `cargo run`) with its builds kept
        // somewhere else (`CARGO_TARGET_DIR`: an agent's own folder, a
        // scratch one): the tree of the package being run, which cargo
        // names. Without it such a test read the installed data, stale
        // or missing; with the catalogs data too, the wrong words.
        if let Some(package) = std::env::var_os("CARGO_MANIFEST_DIR") {
            for up in Path::new(&package).ancestors().take(5) {
                let d = up.join("data");
                if d.join("icons").is_dir() {
                    if !v.contains(&d) {
                        v.push(d);
                    }
                    break;
                }
            }
        }
        // An Olo installed in a place of its own (PREFIX/bin/x beside
        // PREFIX/share/olo): its own data, before the home folder's. A
        // Rex host's, for its agents' screens, is one
        // (~/.local/share/olo/desktop: `olo screen install`), on a host
        // whose home has none of Olo's data. (For ~/.local it is the
        // home folder's own, below.)
        if let Ok(exe) = std::env::current_exe()
            && let Some(prefix) = exe.parent().and_then(Path::parent)
        {
            let d = prefix.join("share/olo");
            if d.join("icons").is_dir() && !v.contains(&d) {
                v.push(d);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let installed = Path::new(&home).join(".local/share/olo");
            // The tree it was built from (recorded by the installer): new
            // data shows up there before an upgrade copies it over.
            let tree = std::fs::read_to_string(installed.join("build-dir")).ok().and_then(|t| Path::new(t.trim()).ancestors().nth(2).map(|r| r.join("data")));
            if !v.contains(&installed) {
                v.push(installed);
            }
            v.extend(tree);
        }
        v.push("/usr/local/share/olo".into());
        v.push("/usr/share/olo".into());
        v.retain(|d| d.is_dir());
        v
    })
}

/// Olo's own fonts (`data/fonts`: Noto Sans and the faces that fill in
/// its symbols), from the first data directory that has them.
pub fn fonts_dir() -> Option<PathBuf> {
    data_dirs().iter().map(|d| d.join("fonts")).find(|d| d.is_dir())
}

/// The freedesktop data directories, most important first ($HOME first).
pub fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    v.push("/var/lib/flatpak/exports/share".into());
    v
}

/// Find the best source file for `name` at `px` device pixels, in the
/// current theme.
pub fn find_icon(name: &str, px: u32) -> Option<PathBuf> {
    find_icon_in(themes::current().id, name, px)
}

/// As [`find_icon`], in theme `theme` (e.g. to show it before it is
/// chosen).
pub fn find_icon_in(theme: &str, name: &str, px: u32) -> Option<PathBuf> {
    if name.starts_with('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    // Ours: its drawing, rendered for the size: the theme's own, else the
    // classic set's (which `load_icon_in` shows in the theme's colours).
    if let Some(drawn) = own_icon(theme, name) {
        return Some(drawn);
    }
    // Freedesktop themes.
    for base in xdg_data_dirs() {
        for theme in ["hicolor", "Adwaita", "breeze"] {
            let root = base.join("icons").join(theme);
            if !root.is_dir() {
                continue;
            }
            let mut best: Option<(u32, PathBuf)> = None;
            for sub in ["apps", "mimetypes", "places", "devices", "categories", "actions", "status"] {
                let svg = root.join("scalable").join(sub).join(format!("{name}.svg"));
                if svg.is_file() {
                    return Some(svg);
                }
                for s in [16u32, 22, 24, 32, 48, 64, 96, 128, 256, 512] {
                    let p = root.join(format!("{s}x{s}")).join(sub).join(format!("{name}.png"));
                    if p.is_file() {
                        let better = match &best {
                            None => true,
                            Some((b, _)) => (s >= px && (*b < px || s < *b)) || (*b < px && s > *b),
                        };
                        if better {
                            best = Some((s, p));
                        }
                    }
                }
            }
            if let Some((_, p)) = best {
                return Some(p);
            }
        }
        for ext in ["svg", "png"] {
            let p = base.join("pixmaps").join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// A decoded icon: RGBA8, `premultiplied` or straight alpha as decoded.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub premultiplied: bool,
}

impl Image {
    /// Convert to premultiplied alpha (what compositors and tiny-skia want).
    pub fn into_premultiplied(mut self) -> Image {
        if !self.premultiplied {
            for p in self.rgba.as_chunks_mut::<4>().0 {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
            self.premultiplied = true;
        }
        self
    }
}

/// Icon `name` (ours, a theme name, or an absolute path) for `px` device
/// pixels: Olo's own drawing rendered at `px` (any SVG is), or another
/// program's best pre-scaled PNG.
pub fn load_icon(name: &str, px: u32) -> Option<Image> {
    load_icon_in(themes::current().id, name, px)
}

/// As [`load_icon`], in theme `theme`.
pub fn load_icon_in(theme: &str, name: &str, px: u32) -> Option<Image> {
    let path = find_icon_in(theme, name, px)?;
    if path.extension().is_some_and(|e| e == "svg") {
        // (Olo's own drawings are drawn for the size, once: `svg::own`;
        // the classic set's, filling in for a theme, in the theme's
        // colours.)
        if !svg::is_own(&path) {
            svg::render(&std::fs::read(&path).ok()?, px, false)
        } else if let Some(rules) = themes::recolour(theme).filter(|_| !themes::is_its_own(theme, &path)) {
            svg::own_recoloured(&path, px, themes::resolve(theme).id, rules)
        } else {
            svg::own(&path, px)
        }
    } else {
        decode_png(&path)
    }
}

impl Image {
    /// The picture as a PNG file's bytes (straight alpha, as PNG has it).
    pub fn to_png(&self) -> Option<Vec<u8>> {
        let mut rgba = self.rgba.clone();
        if self.premultiplied {
            for p in rgba.as_chunks_mut::<4>().0 {
                let a = u32::from(p[3]);
                if a > 0 && a < 255 {
                    for c in &mut p[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
        writer.finish().ok()?;
        Some(out)
    }
}

pub fn decode_png(path: &Path) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    // Any PNG as 8 bits a channel: a palette's (an AppImage's icon), 16
    // bits' and a transparent colour's too.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba: Vec<u8> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf[..w * h * 4].to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf[..w * h * 3].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Image { width: w as u32, height: h as u32, rgba, premultiplied: false })
}

/// Whether Olo's own set (the Classic one, which every theme draws in
/// full) has an icon named `name`: our own take on another program's
/// (`google-chrome`, `slack`). Read once.
pub fn has_own_icon(name: &str) -> bool {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| {
            let mut names = std::collections::HashSet::new();
            for dir in data_dirs() {
                if let Ok(rd) = std::fs::read_dir(dir.join("icons/svg")) {
                    names.extend(rd.flatten().filter_map(|f| Some(f.path().file_stem()?.to_string_lossy().into_owned())));
                }
            }
            names
        })
        .contains(name)
}

/// The icon name a desktop app declares (`Icon=` in its .desktop file),
/// looked up by app id / desktop file name.
pub fn icon_for_app(app_id: &str) -> Option<String> {
    desktop_entry::find(app_id)?.icon
}

/// Where program `name` is installed: beside the running executable (so a
/// dev build finds its sibling dev builds), then on PATH.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(name)));
    let path = std::env::var_os("PATH").unwrap_or_default();
    beside.into_iter().chain(std::env::split_paths(&path).map(|d| d.join(name))).find(|p| p.is_file())
}

/// Olo's pointer as an XCursor theme, for programs that draw their own
/// (app_server's `xcursor.rs` writes it; olo-session names it in
/// `XCURSOR_THEME` and `XCURSOR_SIZE`): its name, and its size at 1×.
pub const CURSOR_THEME: &str = "Olo";
pub const CURSOR_SIZE: u32 = 22;

/// What an Olo session tells programs it is (`XDG_CURRENT_DESKTOP`): Olo,
/// then GNOME, whose settings it serves (`org.gnome.desktop.interface` in
/// dconf, the GTK portals, GTK themes in Olo's colours). Programs that
/// only know the big desktops follow Olo's look by it: Qt takes its GTK
/// theme for GNOME (Olo's palette and fonts, light or dark, live), but
/// its plain one for a name it doesn't know.
pub const CURRENT_DESKTOP: &str = "Olo:GNOME";

/// GIO (GLib's files, in GTK and GNOME programs, Chrome) without GNOME's
/// gvfs: its own local files and the system's mounts (`unix`), so no
/// gvfsd, gvfsd-fuse, gvfsd-metadata or volume monitors start for the
/// programs Olo starts. (Trash and recent files are GIO's own on local
/// files; Olo's Tracker is the file manager.) Set for what olo-session
/// starts, never exported to systemd, so a GNOME login after has gvfs.
pub const GIO_LOCAL: [(&str, &str); 2] = [("GIO_USE_VFS", "local"), ("GIO_USE_VOLUME_MONITOR", "unix")];

/// For an Olo program re-executing in place after an upgrade: bring up to
/// date what the environment kept from a session started on an older build
/// (`XDG_CURRENT_DESKTOP` said only "Olo"; no [`GIO_LOCAL`]), so the
/// programs it starts from then on have them without a new session.
pub fn upgrade_env(cmd: &mut std::process::Command) {
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d == "Olo") {
        cmd.env("XDG_CURRENT_DESKTOP", CURRENT_DESKTOP);
    }
    if std::env::var_os("OLO_SESSION").is_some() {
        for (k, v) in GIO_LOCAL {
            if std::env::var_os(k).is_none() {
                cmd.env(k, v);
            }
        }
        // (A session from Olo's own login had no language at all:
        // `locale`. Each program takes the system's as it re-executes,
        // and what it starts from then on has it.)
        for (k, v) in locale::missing_here() {
            cmd.env(k, v);
        }
    }
}

/// The user's runtime directory (`XDG_RUNTIME_DIR`, where the session's
/// sockets and per-display files are). A program started with a bare
/// environment (a coding agent's MCP server: Codex passes only HOME, PATH
/// and a few more) finds the standard one, /run/user/UID, when it's
/// there and the user's; else the temporary directory.
pub fn runtime_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return dir.into();
    }
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    let standard = std::path::PathBuf::from(format!("/run/user/{uid}"));
    match std::fs::metadata(&standard) {
        Ok(m) if m.is_dir() && std::os::unix::fs::MetadataExt::uid(&m) == uid => standard,
        _ => std::env::temp_dir(),
    }
}

/// What a session service exits with when it isn't wanted (turned off in
/// its settings): olo-session doesn't start it again (until an upgrade
/// asks its services to start once more).
pub const SERVICE_NOT_WANTED: i32 = 75;

/// Passwords' SSH agent's socket for this session (`SSH_AUTH_SOCK`): in
/// the runtime folder, one a display (an Agent Screen's Passwords has its
/// own).
pub fn ssh_agent_socket() -> std::path::PathBuf {
    let display: String = display_name().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    runtime_dir().join(format!("olo-ssh-agent-{}.sock", if display.is_empty() { "wayland-0".into() } else { display }))
}

/// This session's Wayland display by its name (`wayland-1`; empty if
/// none), for files and sockets kept per display: `WAYLAND_DISPLAY` may
/// name the socket by its full path instead.
/// Where state that's usually not there is said in the runtime folder
/// (an upgrade offered, a recording going, notifications waiting, what's
/// shared, AirPlay's plays): `$XDG_RUNTIME_DIR/olo-state/`, made as it's
/// asked for. A folder of its own, quiet: something waiting for one of
/// these to come watches its folder, and a watch of the runtime folder
/// itself wakes for every change in it (Token Pool's meters and the
/// supervisor's state several times a second: Deskbar woke 5 times a
/// second idle so, 2026-11-15).
pub fn state_runtime_dir() -> std::path::PathBuf {
    let dir = runtime_dir().join("olo-state");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn display_name() -> String {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    display.rsplit('/').next().unwrap_or_default().to_owned()
}

/// Run a shell command line detached from us (own session, no stdin or
/// stdout; Olo's own programs keep our stderr, the journal).
/// Our own programs are found beside us first, as `find_program` finds
/// them: the login screen gives a session a bare PATH that needn't
/// include where Olo is installed (~/.local/bin).
pub fn spawn_detached(cmd: &str) -> std::io::Result<()> {
    spawn_detached_pid(cmd, &[]).map(drop)
}

/// `spawn_detached` for desktop entry `app_id`, in a scope of its own
/// (`scope`; not Olo's own programs). The shell runs a lone command in
/// its own place, so the scope holds the program.
pub fn spawn_detached_app(cmd: &str, app_id: &str) -> std::io::Result<()> {
    spawn_detached_app_with(cmd, app_id, &[])
}

/// [`spawn_detached_app`] with `env` set for it (`NEW_INSTANCE`).
pub fn spawn_detached_app_with(cmd: &str, app_id: &str, env: &[(&str, &str)]) -> std::io::Result<()> {
    let pid = spawn_detached_pid(cmd, env)?;
    if cmd.split_whitespace().next().is_some_and(|p| !scope::ours(p.as_ref())) {
        scope::place(pid, app_id);
    }
    Ok(())
}

/// Set for a program started as a copy of its own beside the one running
/// (Deskbar's ⌘-click): the kit's `instance::forward` hands nothing over
/// then, and clears it, so what the copy starts isn't one too.
pub const NEW_INSTANCE: &str = "OLO_NEW_INSTANCE";

/// Wait for a launched child on a worker and return its pid to the launcher.
pub fn reap_child(mut child: std::process::Child) -> u32 {
    let pid = child.id();
    std::thread::spawn(move || child.wait());
    pid
}

fn spawn_detached_pid(cmd: &str, env: &[(&str, &str)]) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(cmd);
    // (A copy of its own only when asked: never inherited.)
    c.env_remove(NEW_INSTANCE);
    // (Its language, where we were started with none: `locale`.)
    locale::give(&mut c);
    c.envs(env.iter().copied());
    if let Some(program) = cmd.split_whitespace().next() {
        electron::on_screen(&mut c, program.as_ref());
        quirks::apply(&mut c, program.as_ref());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.to_owned())) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        if let Ok(path) = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path))) {
            c.env("PATH", path);
        }
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    // Olo's own programs log where we do (the session's journal); others'
    // chatter is theirs.
    if !cmd.split_whitespace().next().is_some_and(|p| scope::ours(p.as_ref())) {
        c.stderr(std::process::Stdio::null());
    }
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map(reap_child)
}

/// Olo's settings directory (`$XDG_CONFIG_HOME/olo`); apps keep their
/// preferences in `NAME.json` there.
pub fn config_dir() -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config")).join("olo")
}

/// Olo Doctor's opt-in welcome for newly opened interactive Terminal shells.
pub fn terminal_welcome_path() -> PathBuf {
    config_dir().join("terminal-welcome")
}

/// Olo's cache directory (`$XDG_CACHE_HOME/olo`).
pub fn cache_dir() -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".cache")).join("olo")
}

/// Olo's data directory (`$XDG_DATA_HOME/olo`): what the person makes
/// with Olo's apps that isn't a document of theirs elsewhere (To-Do's
/// lists).
pub fn data_dir() -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/share")).join("olo")
}

/// Olo's state directory (`$XDG_STATE_HOME/olo`): what programs
/// keep between runs that is neither a setting nor a cache (histories).
pub fn state_dir() -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/state")).join("olo")
}

/// The agent screen this process is on (`OLO_SCREEN`; None: the person's
/// own session, or a test's).
pub fn screen() -> Option<String> {
    std::env::var("OLO_SCREEN").ok().filter(|s| !s.is_empty() && !s.contains('/'))
}

/// Where a program keeps what belongs to the session it serves, not to
/// the person's programs wherever they show: a screen's own folder on an
/// agent screen (`state_dir()/screens/NAME`), the state directory
/// itself otherwise. An agent screen is the person's account and home,
/// so its services would otherwise write the person's own files: every
/// screen's notification centre kept its history in the person's, each
/// saving over the others' (2026-11-18).
pub fn session_state_dir() -> PathBuf {
    match screen() {
        Some(name) => state_dir().join("screens").join(name),
        None => state_dir(),
    }
}

/// A nameless file on disk to gather bytes in rather than memory (not in
/// /tmp, which is memory too): in `cache_dir()/scratch`, gone once closed.
pub fn scratch_file() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = cache_dir().join("scratch");
    std::fs::create_dir_all(&dir)?;
    std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_TMPFILE).mode(0o600).open(&dir)
}

/// A path for display, with the home folder shortened to `~`.
pub fn display_path(p: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return p.display().to_string() };
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// A user folder from `user-dirs.dirs` (`"PICTURES"`, `"DESKTOP"`, ...), if
/// it is set and exists; otherwise None (callers fall back to home).
pub fn user_dir(name: &str) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    parse_user_dir(&text, name, &home).filter(|d| d.is_dir())
}

fn parse_user_dir(text: &str, name: &str, home: &Path) -> Option<PathBuf> {
    let key = format!("XDG_{name}_DIR=");
    let value = text.lines().map(str::trim).find_map(|l| l.strip_prefix(key.as_str()))?;
    let value = value.trim().trim_matches('"');
    let dir = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    // "$HOME/" alone means the folder is disabled.
    (dir != home && dir.is_absolute()).then_some(dir)
}

/// Put `op` beside `olo-op` in `dir` (a link to it: Olo's stand-in
/// for 1Password's command-line tool), or take it away. An `op` there
/// that isn't ours (1Password's own, say) is left alone.
pub fn link_op(dir: &std::path::Path, on: bool) -> std::io::Result<()> {
    let link = dir.join("op");
    let target = std::fs::read_link(&link).ok();
    // Ours, or ours from before the rename (`tabula-op`, then made new).
    let old = target.as_ref().is_some_and(|t| t.file_name().is_some_and(|n| n == "tabula-op"));
    let ours = old || target.as_ref().is_some_and(|t| t.file_name().is_some_and(|n| n == "olo-op"));
    let there = std::fs::symlink_metadata(&link).is_ok();
    if there && !ours {
        return Ok(());
    }
    if on && (!there || old) && dir.join("olo-op").is_file() {
        let tmp = dir.join(".op.link");
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink("olo-op", &tmp)?;
        std::fs::rename(&tmp, &link)?;
    } else if !on && ours {
        std::fs::remove_file(&link)?;
    }
    Ok(())
}

/// [`link_op`] in the directory our own programs run from (not a build
/// directory, `target/debug`), on a thread.
pub fn link_op_here(on: bool) {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.to_owned())) else { return };
    if dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "target") {
        return;
    }
    std::thread::spawn(move || {
        let _ = link_op(&dir, on);
    });
}

#[cfg(test)]
mod op_link_tests {
    #[test]
    fn links_only_ours() {
        let dir = std::env::temp_dir().join(format!("op-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("olo-op"), "").unwrap();
        super::link_op(&dir, true).unwrap();
        assert_eq!(std::fs::read_link(dir.join("op")).unwrap(), std::path::Path::new("olo-op"));
        super::link_op(&dir, false).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("op")).is_err());
        // One from before the rename is ours: made new, or taken away.
        std::os::unix::fs::symlink("tabula-op", dir.join("op")).unwrap();
        super::link_op(&dir, true).unwrap();
        assert_eq!(std::fs::read_link(dir.join("op")).unwrap(), std::path::Path::new("olo-op"));
        let _ = std::fs::remove_file(dir.join("op"));
        std::os::unix::fs::symlink("tabula-op", dir.join("op")).unwrap();
        super::link_op(&dir, false).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("op")).is_err());
        // Someone else's op stays, on or off.
        std::fs::write(dir.join("op"), "theirs").unwrap();
        super::link_op(&dir, true).unwrap();
        super::link_op(&dir, false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("op")).unwrap(), "theirs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod user_dir_tests {
    use super::*;

    #[test]
    fn launches_find_our_programs_first() {
        let out = std::env::temp_dir().join(format!("olo-spawn-test-{}", std::process::id()));
        spawn_detached(&format!("printf %s \"$PATH\" > {}", out.display())).unwrap();
        let mut path = String::new();
        for _ in 0..100 {
            path = std::fs::read_to_string(&out).unwrap_or_default();
            if !path.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(&out);
        let ours = std::env::current_exe().unwrap().parent().unwrap().to_owned();
        assert_eq!(std::env::split_paths(&path).next(), Some(ours));
    }

    #[test]
    fn parses_user_dirs() {
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Pictures\"\nXDG_MUSIC_DIR=\"$HOME/\"\nXDG_VIDEOS_DIR=\"/srv/videos\"\n";
        let home = Path::new("/home/x");
        assert_eq!(parse_user_dir(text, "PICTURES", home), Some(PathBuf::from("/home/x/Pictures")));
        assert_eq!(parse_user_dir(text, "MUSIC", home), None);
        assert_eq!(parse_user_dir(text, "VIDEOS", home), Some(PathBuf::from("/srv/videos")));
        assert_eq!(parse_user_dir(text, "TEMPLATES", home), None);
    }
}
