//! Other toolkits' programs in a real session: what GTK 3, GTK 4,
//! libadwaita and Chrome read for their look, published from Olo's
//! palette (see `olo_theme::toolkit`). Qt reads it too, through its GTK 3
//! platform theme, which it takes because the session says it is GNOME
//! as well as Olo (`olo_xdg::CURRENT_DESKTOP`).
//!
//! - GSettings `org.gnome.desktop.interface` (through dconf, which GTK and
//!   the portals read): the colour scheme, the GTK theme (the look's,
//!   `toolkit::gtk_theme_name`), the accent, Olo's fonts, and BeOS's
//!   close-left, zoom-right title buttons in
//!   `org.gnome.desktop.wm.preferences`.
//! - The look's GTK 3 themes, light and dark, in `~/.local/share/themes`
//!   (the old looks' are cleared away once GTK has the new name).
//! - GTK 4's user stylesheet `~/.config/gtk-4.0/gtk.css` imports Olo's
//!   colours from the runtime directory, where they are only while a
//!   Olo session runs (olo-session removes them at the end), so a
//!   GNOME session keeps its own look.

use std::path::{Path, PathBuf};

use olo_theme::{ThemeMode, fonts, looks, toolkit};
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

/// The GTK themes for theme `theme` in colour set `colors`: (name,
/// palette), light then dark.
fn gtk_themes(theme: &str, colors: &str) -> [(String, olo_theme::Palette); 2] {
    [ThemeMode::Light, ThemeMode::Dark].map(|m| (toolkit::gtk_theme_name(theme, colors, m), looks::palette(theme, colors, m)))
}

/// The files: the look's GTK 3 themes, GTK 4's stylesheet and its import.
pub fn write_files(data: &Path, config: &Path, runtime_css: &Path, theme: &str, colors: &str) -> std::io::Result<()> {
    let [light, dark] = gtk_themes(theme, colors);
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
pub fn clear_old_themes(data: &Path, theme: &str, colors: &str) {
    let keep = gtk_themes(theme, colors).map(|(n, _)| n);
    let Ok(dir) = std::fs::read_dir(data.join("themes")) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name == "Olo" || name.starts_with(toolkit::GTK_THEME_PREFIX);
        if ours
            && !keep.contains(&name)
            && std::fs::read_to_string(entry.path().join("index.theme")).is_ok_and(|t| t.contains(toolkit::GTK_INDEX_COMMENT))
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Publish theme `theme` in colour set `colors`, in `mode` (off the
/// compositor thread: dconf and the files can be slow).
pub fn publish(theme: String, colors: String, mode: ThemeMode) {
    std::thread::spawn(move || {
        let dirs = (home_dir("XDG_DATA_HOME", ".local/share"), home_dir("XDG_CONFIG_HOME", ".config"), gtk4_runtime_css());
        if let (Some(data), Some(config), Some(css)) = &dirs
            && let Err(e) = write_files(data, config, css, &theme, &colors)
        {
            warn!("could not write the GTK themes: {e}");
        }
        let scheme = match mode {
            ThemeMode::Light => "default",
            ThemeMode::Dark => "prefer-dark",
        };
        let gtk_theme = toolkit::gtk_theme_name(&theme, &colors, mode);
        // GTK sizes fonts in points (at 96 dpi), Olo in pixels.
        let font = |faces: &[&str]| format!("{} {}", faces[0], (fonts::PLAIN_SIZE * 0.75).round());
        let (ui, mono) = (font(fonts::UI), font(fonts::MONO));
        let keys = [
            ("/org/gnome/desktop/interface/color-scheme", scheme),
            ("/org/gnome/desktop/interface/gtk-theme", gtk_theme.as_str()),
            ("/org/gnome/desktop/interface/accent-color", "blue"),
            ("/org/gnome/desktop/interface/font-name", ui.as_str()),
            ("/org/gnome/desktop/interface/document-font-name", ui.as_str()),
            ("/org/gnome/desktop/interface/monospace-font-name", mono.as_str()),
            ("/org/gnome/desktop/wm/preferences/button-layout", "close:maximize"),
        ];
        match olo_system::dconf::set_strings(&keys) {
            Ok(()) => {
                if let Some(data) = &dirs.0 {
                    clear_old_themes(data, &theme, &colors);
                }
            }
            Err(e) => warn!("could not set the toolkit settings: {e}"),
        }
    });
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
        for (name, index) in [("Olo-dark", toolkit::gtk_index("Olo-dark")), ("Olo-enamel-beos", toolkit::gtk_index("x")), ("Olo-mine", "[Desktop Entry]\n".into())] {
            std::fs::create_dir_all(data.join("themes").join(name)).unwrap();
            std::fs::write(data.join("themes").join(name).join("index.theme"), index).unwrap();
        }
        write_files(&data, &config, &css, "toybox", "catppuccin").unwrap();
        write_files(&data, &config, &css, "toybox", "catppuccin").unwrap();
        let user = std::fs::read_to_string(config.join("gtk-4.0/gtk.css")).unwrap();
        assert_eq!(user.matches("@import").count(), 1, "{user}");
        assert!(user.lines().nth(1).unwrap().starts_with("@import") && user.ends_with("label { color: red; }\n"));
        assert!(data.join("themes/Olo-toybox-catppuccin-dark/gtk-3.0/gtk.css").exists() && css.exists());
        let g3 = std::fs::read_to_string(data.join("themes/Olo-toybox-catppuccin-dark/gtk-3.0/gtk.css")).unwrap();
        assert!(g3.contains("@define-color theme_bg_color #24273a;"), "Macchiato's base");
        clear_old_themes(&data, "toybox", "catppuccin");
        let left: Vec<String> = std::fs::read_dir(data.join("themes")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        let mut left = left;
        left.sort();
        assert_eq!(left, ["Olo-mine", "Olo-toybox-catppuccin", "Olo-toybox-catppuccin-dark"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
