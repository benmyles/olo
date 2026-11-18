//! Other toolkits' programs in a real session: what GTK 3, GTK 4,
//! libadwaita and Chrome read for their look, published from Olo's
//! palette (see `olo_theme::toolkit`). Qt reads it too, through its GTK 3
//! platform theme, which it takes because the session says it is GNOME
//! as well as Olo (`olo_xdg::CURRENT_DESKTOP`).
//!
//! - GSettings `org.gnome.desktop.interface` (through dconf, which GTK and
//!   the portals read): the colour scheme, the GTK theme (the look's,
//!   `toolkit::gtk_theme_name`), the accent, Olo's fonts hinted as the
//!   kit hints them, Olo's icons and pointer (`icon_theme.rs`,
//!   `xcursor.rs`), and BeOS's
//!   close-left, zoom-right title buttons in
//!   `org.gnome.desktop.wm.preferences`.
//! - The look's GTK 3 themes, light and dark, in `~/.local/share/themes`
//!   (the old looks' are cleared away once GTK has the new name).
//! - GTK 4's user stylesheet `~/.config/gtk-4.0/gtk.css` imports Olo's
//!   colours from the runtime directory, where they are only while a
//!   Olo session runs (olo-session removes them at the end), so a
//!   GNOME session keeps its own look.

use std::path::{Path, PathBuf};

use olo_protocol::Appearance;
use olo_theme::{ThemeMode, toolkit};
use tracing::warn;

/// The stylesheet GTK 4 imports, while the session runs.
fn gtk4_runtime_css() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("olo").join("gtk-4.0.css"))
}

fn home_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(fallback)))
}

/// Write `text` to `path` (atomically) unless it is there already.
fn put(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("olo-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `gtk.css` with the import of `css` first (CSS wants imports before
/// anything else), keeping what the user wrote there.
fn with_import(existing: &str, css: &Path) -> Option<String> {
    let line = format!("@import url(\"file://{}\");", css.display());
    if existing.lines().any(|l| l.trim() == line) {
        return None;
    }
    Some(format!("/* Olo's colours while an Olo session runs (written by Olo). */\n{line}\n{existing}"))
}

/// The GTK themes for the look `a` has (its theme, as the person has
/// it): (name, palette), light then dark.
fn gtk_themes(a: &Appearance) -> [(String, olo_theme::Palette); 2] {
    let custom = a.custom_key();
    [ThemeMode::Light, ThemeMode::Dark].map(|m| (toolkit::gtk_theme_name(&a.look, &custom, m), a.palette_of(&a.look, m)))
}

/// The files: the look's GTK 3 themes, GTK 4's stylesheet and its import.
pub fn write_files(data: &Path, config: &Path, runtime_css: &Path, a: &Appearance) -> std::io::Result<()> {
    let [light, dark] = gtk_themes(a);
    for (name, p) in [&light, &dark] {
        let dir = data.join("themes").join(name);
        put(&dir.join("index.theme"), &toolkit::gtk_index(name))?;
        put(&dir.join("gtk-3.0").join("gtk.css"), &toolkit::gtk3_css(p))?;
        put(&dir.join("gtk-4.0").join("gtk.css"), &toolkit::gtk4_theme_css(p))?;
    }
    put(runtime_css, &toolkit::gtk4_css(&light.1, &dark.1))?;
    let user = config.join("gtk-4.0").join("gtk.css");
    let existing = std::fs::read_to_string(&user).unwrap_or_default();
    if let Some(text) = with_import(&existing, runtime_css) {
        put(&user, &text)?;
    }
    Ok(())
}

/// Clear away the GTK themes Olo wrote for other looks (known by their
/// `index.theme`; the `Olo` and `Olo-dark` of before too).
pub fn clear_old_themes(data: &Path, a: &Appearance) {
    let keep = gtk_themes(a).map(|(n, _)| n);
    let Ok(dir) = std::fs::read_dir(data.join("themes")) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name == "Olo" || name.starts_with(toolkit::GTK_THEME_PREFIX);
        if ours && !keep.contains(&name) && std::fs::read_to_string(entry.path().join("index.theme")).is_ok_and(|t| t.contains(toolkit::GTK_INDEX_COMMENT)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Publish the look `a` has, in the mode in effect: its theme as the
/// person has it (their colours, their icon set). Off the compositor
/// thread (dconf and the files can be slow), one at a time, the latest
/// asked for: a colour dragged in Customize asks many times a second, and
/// two at once would each clear the other's themes away.
pub fn publish(a: Appearance) {
    use std::sync::{Condvar, Mutex, OnceLock};
    static NEXT: Mutex<Option<Appearance>> = Mutex::new(None);
    static ASKED: Condvar = Condvar::new();
    static WORKER: OnceLock<()> = OnceLock::new();
    *NEXT.lock().unwrap_or_else(|e| e.into_inner()) = Some(a);
    ASKED.notify_one();
    // (Asleep until asked: it costs nothing between.)
    WORKER.get_or_init(|| {
        let made = std::thread::Builder::new().name("toolkits".into()).spawn(|| {
            loop {
                let mut next = NEXT.lock().unwrap_or_else(|e| e.into_inner());
                let a = loop {
                    match next.take() {
                        Some(a) => break a,
                        None => next = ASKED.wait(next).unwrap_or_else(|e| e.into_inner()),
                    }
                };
                drop(next);
                publish_now(&a);
            }
        });
        if let Err(e) = made {
            warn!("no thread to tell other toolkits the look: {e}");
        }
    });
}

fn publish_now(a: &Appearance) {
    {
        let dirs = (home_dir("XDG_DATA_HOME", ".local/share"), home_dir("XDG_CONFIG_HOME", ".config"), gtk4_runtime_css());
        if let (Some(data), Some(config), Some(css)) = &dirs
            && let Err(e) = write_files(data, config, css, a)
        {
            warn!("could not write the GTK themes: {e}");
        }
        // Olo's icons in the look's icon set, and its pointer for
        // programs that draw their own (icon_theme.rs, xcursor.rs).
        if let Some(data) = &dirs.0
            && let Err(e) = crate::icon_theme::write(&data.join("icons"), a.icons())
        {
            warn!("could not write the icon theme: {e}");
        }
        // (The same olo-portal serves through the portal's Settings.)
        let settings = toolkit::desktop_settings(&a.look, &a.custom_key(), a.effective_theme, crate::icon_theme::THEME, crate::xcursor::SIZE);
        let paths: Vec<String> = settings.iter().map(|s| s.path()).collect();
        use olo_system::dconf::Setting::{Int, Str};
        let keys: Vec<(&str, olo_system::dconf::Setting<'_>)> = settings
            .iter()
            .zip(&paths)
            .map(|(s, p)| {
                (
                    p.as_str(),
                    match &s.value {
                        toolkit::DesktopValue::Str(v) => Str(v.as_str()),
                        toolkit::DesktopValue::Int(v) => Int(*v),
                    },
                )
            })
            .collect();
        match olo_system::dconf::set(&keys) {
            Ok(()) => {
                if let Some(data) = &dirs.0 {
                    clear_old_themes(data, a);
                }
            }
            Err(e) => warn!("could not set the toolkit settings: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_keep_the_users_css() {
        let dir = std::env::temp_dir().join(format!("olo-toolkits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (data, config, css) = (dir.join("data"), dir.join("config"), dir.join("run/olo/gtk-4.0.css"));
        std::fs::create_dir_all(config.join("gtk-4.0")).unwrap();
        std::fs::write(config.join("gtk-4.0/gtk.css"), "label { color: red; }\n").unwrap();
        // An old look's theme, and someone else's that looks like Olo's.
        for (name, index) in
            [("Olo-dark", toolkit::gtk_index("Olo-dark")), ("Olo-enamel-beos", toolkit::gtk_index("x")), ("Olo-platinum-beos-dark", toolkit::gtk_index("x")), ("Olo-mine", "[Desktop Entry]\n".into())]
        {
            std::fs::create_dir_all(data.join("themes").join(name)).unwrap();
            std::fs::write(data.join("themes").join(name).join("index.theme"), index).unwrap();
        }
        let look = Appearance { look: "beos".into(), ..Appearance::default() };
        write_files(&data, &config, &css, &look).unwrap();
        write_files(&data, &config, &css, &look).unwrap();
        let user = std::fs::read_to_string(config.join("gtk-4.0/gtk.css")).unwrap();
        assert_eq!(user.matches("@import").count(), 1, "{user}");
        assert!(user.lines().nth(1).unwrap().starts_with("@import") && user.ends_with("label { color: red; }\n"));
        assert!(data.join("themes/Olo-beos-dark/gtk-3.0/gtk.css").exists() && css.exists());
        let g3 = std::fs::read_to_string(data.join("themes/Olo-beos-dark/gtk-3.0/gtk.css")).unwrap();
        assert!(g3.contains("@define-color theme_bg_color #2b2b2b;"), "BeOS's dark panel");
        clear_old_themes(&data, &look);
        let left = |data: &Path| {
            let mut left: Vec<String> = std::fs::read_dir(data.join("themes")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            left.sort();
            left
        };
        assert_eq!(left(&data), ["Olo-beos", "Olo-beos-dark", "Olo-mine"]);
        // The person changes a colour of the theme: themes of another
        // name, with it, and the theme's own cleared away.
        let mut mine = look.clone();
        mine.custom.entry("beos".into()).or_default().dark.insert("panel".into(), "#202840".into());
        let key = mine.custom_key();
        write_files(&data, &config, &css, &mine).unwrap();
        clear_old_themes(&data, &mine);
        assert_eq!(left(&data), [format!("Olo-beos-{key}"), format!("Olo-beos-{key}-dark"), "Olo-mine".to_owned()]);
        let g3 = std::fs::read_to_string(data.join(format!("themes/Olo-beos-{key}-dark/gtk-3.0/gtk.css"))).unwrap();
        assert!(g3.contains("@define-color theme_bg_color #202840;"), "the panel as the person has it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
