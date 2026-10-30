//! Olo's icons and pointer for other programs: a freedesktop icon theme,
//! "Olo" (`~/.local/share/icons/Olo`), that GTK, Qt (through its GTK
//! platform theme) and anything reading `icon-theme` take their folder,
//! file-kind, device and application icons from, in the look in use, and
//! the rest from Adwaita and hicolor, which it inherits. Its pictures are
//! links to Olo's own (`data/icons`, or the look's `data/themes/*/icons`),
//! remade when the look changes. The theme's `cursors` are
//! `xcursor.rs`'s.

use std::{
    io,
    path::{Path, PathBuf},
};

/// The theme's name (the cursors' too).
pub const THEME: &str = olo_xdg::CURSOR_THEME;

/// Freedesktop icon names, the context they go in, and Olo's icon for
/// each.
const ICONS: &[(&str, &str, &str)] = &[
    // Places.
    ("folder", "places", "folder"),
    ("folder-open", "places", "folder-open"),
    ("inode-directory", "places", "folder"),
    ("user-home", "places", "home"),
    ("user-desktop", "places", "desktop"),
    ("user-trash", "places", "trash-empty"),
    ("user-trash-full", "places", "trash-full"),
    ("network-workgroup", "places", "network"),
    ("folder-remote", "places", "network"),
    ("start-here", "places", "olo"),
    // Kinds of file.
    ("text-x-generic", "mimetypes", "text"),
    ("text-x-script", "mimetypes", "text"),
    ("image-x-generic", "mimetypes", "image"),
    ("audio-x-generic", "mimetypes", "audio"),
    ("video-x-generic", "mimetypes", "video"),
    ("package-x-generic", "mimetypes", "archive"),
    ("application-x-archive", "mimetypes", "archive"),
    ("application-zip", "mimetypes", "archive"),
    ("application-pdf", "mimetypes", "pdf"),
    ("application-x-executable", "mimetypes", "executable"),
    ("application-x-generic", "mimetypes", "document"),
    ("x-office-document", "mimetypes", "word-document"),
    ("x-office-spreadsheet", "mimetypes", "spreadsheet"),
    ("x-office-presentation", "mimetypes", "presentation"),
    ("font-x-generic", "mimetypes", "font"),
    ("unknown", "mimetypes", "document"),
    // Devices.
    ("drive-harddisk", "devices", "disk"),
    ("drive-removable-media", "devices", "usb"),
    ("media-removable", "devices", "usb"),
    ("computer", "devices", "system"),
    ("input-keyboard", "devices", "keyboard"),
    ("input-touchpad", "devices", "touchpad"),
    ("camera-web", "devices", "camera"),
    ("camera-photo", "devices", "camera"),
    ("audio-input-microphone", "devices", "microphone"),
    ("printer", "devices", "printer"),
    ("network-wired", "devices", "network"),
    ("bluetooth", "devices", "bluetooth"),
    ("video-display", "devices", "display"),
    // Applications and their kinds.
    ("utilities-terminal", "apps", "terminal"),
    ("system-file-manager", "apps", "tracker"),
    ("accessories-text-editor", "apps", "styled-edit"),
    ("accessories-calculator", "apps", "calculator"),
    ("accessories-screenshot", "apps", "screenshot"),
    ("utilities-system-monitor", "apps", "activity"),
    ("preferences-system", "apps", "preferences"),
    ("preferences-desktop-theme", "apps", "themes"),
    ("preferences-desktop-wallpaper", "apps", "backgrounds"),
    ("preferences-desktop-screensaver", "apps", "screensaver"),
    ("preferences-desktop-keyboard", "apps", "keyboard"),
    ("preferences-desktop-display", "apps", "display"),
    ("preferences-system-notifications", "apps", "notifications"),
    ("preferences-system-network", "apps", "network-prefs"),
    ("system-software-update", "apps", "software-update"),
    ("web-browser", "apps", "browser"),
    ("google-chrome", "apps", "google-chrome"),
    ("slack", "apps", "slack"),
    ("image-viewer", "apps", "show-image"),
    ("multimedia-player", "apps", "multimedia"),
    ("help-about", "apps", "about"),
    ("applications-accessories", "apps", "accessories"),
    ("applications-development", "apps", "development"),
    ("applications-games", "apps", "games"),
    ("applications-internet", "apps", "internet"),
    ("applications-multimedia", "apps", "multimedia"),
    ("applications-office", "apps", "office"),
    ("applications-system", "apps", "system"),
    ("applications-other", "apps", "other-applications"),
    // Alerts and the session.
    ("dialog-warning", "status", "warning"),
    ("dialog-information", "status", "info"),
    ("security-high", "status", "security"),
    ("system-shutdown", "actions", "shut-down"),
    ("system-reboot", "actions", "restart"),
    ("system-log-out", "actions", "log-out"),
    ("system-lock-screen", "actions", "lock-screen"),
];

const CONTEXTS: [(&str, &str); 6] =
    [("places", "Places"), ("mimetypes", "MimeTypes"), ("devices", "Devices"), ("apps", "Applications"), ("status", "Status"), ("actions", "Actions")];

/// The theme's `index.theme` for look `look`: every size and context, and
/// what it was written from (the look, Olo's data folder, the cursors'
/// version: it is written again only when one changes).
fn index(look: &str) -> String {
    let dirs: Vec<String> = olo_xdg::ICON_SIZES.iter().flat_map(|px| CONTEXTS.iter().map(move |(c, _)| format!("{px}x{px}/{c}"))).collect();
    // (Where Olo's pictures are, too: an install elsewhere is linked to
    // afresh.)
    let from = olo_xdg::data_dirs().first().map(|d| d.display().to_string()).unwrap_or_default();
    let mut text = format!(
        "[Icon Theme]\nName={THEME}\nComment=Olo's icons and pointer, written by Olo\nInherits=Adwaita,hicolor\nDirectories={}\nX-Olo-Look={look}\nX-Olo-Data={from}\nX-Olo-Cursors={}\n",
        dirs.join(","),
        crate::xcursor::VERSION
    );
    for px in olo_xdg::ICON_SIZES {
        for (c, context) in CONTEXTS {
            text.push_str(&format!("\n[{px}x{px}/{c}]\nSize={px}\nContext={context}\nType=Fixed\n"));
        }
    }
    text
}

/// Write the theme under `icons` (`~/.local/share/icons`) for look
/// `look`, unless it is there as it would be.
pub fn write(icons: &Path, look: &str) -> io::Result<()> {
    let dir = icons.join(THEME);
    let index_path = dir.join("index.theme");
    let text = index(look);
    let before = std::fs::read_to_string(&index_path).unwrap_or_default();
    if before == text {
        return Ok(());
    }
    // The cursors only when they changed (they take drawing).
    let cursors = format!("X-Olo-Cursors={}\n", crate::xcursor::VERSION);
    if !before.contains(&cursors) || !dir.join("cursors").is_dir() {
        crate::xcursor::write_cursors(&dir.join("cursors"))?;
    }
    for px in olo_xdg::ICON_SIZES {
        for (name, context, ours) in ICONS {
            let link = dir.join(format!("{px}x{px}")).join(context).join(format!("{name}.png"));
            match olo_xdg::own_icon_exact(look, ours, *px) {
                Some(target) => point(&link, &target)?,
                None => {
                    let _ = std::fs::remove_file(&link);
                }
            }
        }
    }
    // Last, so a theme half written is written again.
    let tmp = index_path.with_extension("new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &index_path)
}

/// `link` pointing at `target` (made, or moved to it).
fn point(link: &Path, target: &PathBuf) -> io::Result<()> {
    if std::fs::read_link(link).ok().as_ref() == Some(target) {
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_olos() {
        // Each names an icon Olo has, at every size.
        for (name, context, ours) in ICONS {
            assert!(CONTEXTS.iter().any(|(c, _)| c == context), "{name}: no context {context}");
            for px in olo_xdg::ICON_SIZES {
                assert!(olo_xdg::own_icon_exact(olo_xdg::themes::CLASSIC, ours, *px).is_some(), "{name}: Olo has no {ours} at {px}");
            }
        }
    }

    #[test]
    fn writes_the_theme_for_the_look() {
        let dir = std::env::temp_dir().join(format!("olo-icon-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir, olo_xdg::themes::CLASSIC).unwrap();
        let theme = dir.join(THEME);
        let index = std::fs::read_to_string(theme.join("index.theme")).unwrap();
        assert!(index.contains("Inherits=Adwaita,hicolor") && index.contains("\n[48x48/places]\nSize=48\nContext=Places\nType=Fixed\n"));
        let folder = std::fs::read_link(theme.join("48x48/places/folder.png")).unwrap();
        assert!(folder.ends_with("icons/48/folder.png"), "{folder:?}");
        assert!(theme.join("cursors/default").is_file());
        // Another look: its own pictures where it has them.
        let other = olo_xdg::themes::THEMES.iter().find(|t| t.id != olo_xdg::themes::CLASSIC).unwrap().id;
        write(&dir, other).unwrap();
        assert!(std::fs::read_to_string(theme.join("index.theme")).unwrap().contains(&format!("X-Olo-Look={other}\n")));
        assert_eq!(std::fs::read_link(theme.join("48x48/places/folder.png")).unwrap(), olo_xdg::own_icon_exact(other, "folder", 48).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
