//! Olo's icons and pointer for other programs: a freedesktop icon theme,
//! "Olo" (`~/.local/share/icons/Olo`), that GTK, Qt (through its GTK
//! platform theme) and anything reading `icon-theme` take their folder,
//! file-kind, device and application icons from, in the look in use, and
//! the rest from Adwaita and hicolor, which it inherits. Its pictures are
//! Olo's own drawings rendered at each size as Olo shows them, in the
//! look's colours (`olo_xdg::render_own_icon`), written as PNGs: other
//! programs' renderers don't know a drawing's sizes' classes, and would
//! show its large detail and its small stand-ins at once. Made again when
//! the look or a drawing changes (on `toolkits::publish`'s thread, never
//! the compositor's). The theme's `cursors` are `xcursor.rs`'s.

use std::{io, path::Path};

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

const CONTEXTS: [(&str, &str); 6] = [("places", "Places"), ("mimetypes", "MimeTypes"), ("devices", "Devices"), ("apps", "Applications"), ("status", "Status"), ("actions", "Actions")];

/// The theme's `index.theme` for look `look`: every size and context, and
/// what it was written from (the look, Olo's data folder, the drawings as
/// they are, the cursors' version: it is written again only when one
/// changes).
fn index(look: &str) -> String {
    let dirs: Vec<String> = olo_xdg::ICON_SIZES.iter().flat_map(|px| CONTEXTS.iter().map(move |(c, _)| format!("{px}x{px}/{c}"))).collect();
    // (Where Olo's pictures are, too: an install elsewhere is linked to
    // afresh.)
    let from = olo_xdg::data_dirs().first().map(|d| d.display().to_string()).unwrap_or_default();
    let drawn = olo_xdg::own_icons_stamp(look, ICONS.iter().map(|(_, _, ours)| *ours));
    let mut text = format!(
        "[Icon Theme]\nName={THEME}\nComment=Olo's icons and pointer, written by Olo\nInherits=Adwaita,hicolor\nDirectories={}\nX-Olo-Look={look}\nX-Olo-Data={from}\nX-Olo-Drawn={drawn:x}\nX-Olo-Cursors={}\n",
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
        // (One of Olo's icons goes by several names: rendered once.)
        let mut rendered: std::collections::HashMap<&str, Option<Vec<u8>>> = std::collections::HashMap::new();
        for (name, context, ours) in ICONS {
            let file = dir.join(format!("{px}x{px}")).join(context).join(format!("{name}.png"));
            match rendered.entry(ours).or_insert_with(|| olo_xdg::render_own_icon(look, ours, *px).and_then(|img| img.to_png())) {
                Some(png) => put(&file, png)?,
                None => {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
    }
    // Last, so a theme half written is written again.
    let tmp = index_path.with_extension("new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &index_path)
}

/// `file` holding `bytes`: written beside and named into place, whole or
/// not there, and in place of what was (a link to one of Olo's pictures,
/// from when the theme was links, is replaced, never written through).
fn put(file: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_olos() {
        // Each names an icon Olo has drawn.
        for (name, context, ours) in ICONS {
            assert!(CONTEXTS.iter().any(|(c, _)| c == context), "{name}: no context {context}");
            assert!(olo_xdg::own_icon(olo_xdg::themes::CLASSIC, ours).is_some(), "{name}: Olo has no {ours}");
        }
    }

    #[test]
    fn writes_the_theme_for_the_look() {
        // (On disk: /tmp is memory here.)
        let dir = Path::new("/var/tmp").join(format!("olo-icon-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let theme = dir.join(THEME);
        // What was there was a link to one of Olo's pictures: replaced,
        // and what it pointed at left as it is.
        let old = dir.join("old-folder.png");
        std::fs::create_dir_all(theme.join("48x48/places")).unwrap();
        std::fs::write(&old, "a picture").unwrap();
        std::os::unix::fs::symlink(&old, theme.join("48x48/places/folder.png")).unwrap();
        let t = std::time::Instant::now();
        write(&dir, olo_xdg::themes::CLASSIC).unwrap();
        println!("the icon theme written in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "a picture");
        let index = std::fs::read_to_string(theme.join("index.theme")).unwrap();
        assert!(index.contains("Inherits=Adwaita,hicolor") && index.contains("\n[48x48/places]\nSize=48\nContext=Places\nType=Fixed\n"));
        // A picture of its size, drawn by Olo: a PNG, 48 pixels a side.
        let file = theme.join("48x48/places/folder.png");
        assert!(!file.is_symlink());
        let classic = std::fs::read(&file).unwrap();
        let folder = olo_xdg::decode_png(&file).unwrap();
        assert_eq!((folder.width, folder.height), (48, 48));
        assert!(theme.join("256x256/apps/utilities-terminal.png").is_file());
        assert!(theme.join("cursors/default").is_file());
        // As it would be: left alone.
        let t = std::time::Instant::now();
        let written = std::fs::metadata(&file).unwrap().modified().unwrap();
        write(&dir, olo_xdg::themes::CLASSIC).unwrap();
        println!("found as it would be in {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), written);
        // Another look: the same drawings in its colours.
        let other = olo_xdg::themes::THEMES.iter().find(|t| t.id != olo_xdg::themes::CLASSIC).unwrap().id;
        write(&dir, other).unwrap();
        assert!(std::fs::read_to_string(theme.join("index.theme")).unwrap().contains(&format!("X-Olo-Look={other}\n")));
        assert_ne!(std::fs::read(&file).unwrap(), classic, "{other}'s folder is the classic one");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
