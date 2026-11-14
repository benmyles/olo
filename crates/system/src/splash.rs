//! Olo's boot splash, as what installs it and what checks it share: where
//! Plymouth keeps it and its settings, the scale it's made at (the
//! computer's own screen's in Olo: `boot_scale`), and the stamp a theme
//! is made with (`Made`, `olo-made.json` in it), so Olo Doctor can tell
//! one made at another scale, or by an older drawing, without drawing it
//! again. app_server's plymouth.rs draws it; Olo Doctor and Olo Setup
//! install it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the theme is installed (its `ImageDir`).
pub const THEME_DIR: &str = "/usr/share/plymouth/themes/olo";
/// Plymouth's settings: which theme, and its device scale.
pub const CONF: &str = "/etc/plymouth/plymouthd.conf";
/// What was there before Olo's, kept for the way back.
pub const CONF_BEFORE: &str = "/etc/plymouth/plymouthd.conf.before-olo";
/// Olo's plymouthd.conf: its theme, device pixels (the pictures are
/// made for the screen, never stretched).
pub const OLO_CONF: &str = "# Olo's boot splash (Olo Doctor, Olo Setup or tools/install-login.sh\n# --plymouth; each can put back what was here).\n[Daemon]\nTheme=olo\nDeviceScale=1\n";
/// The plugin that draws it (installed with Plymouth, as Fedora's own
/// themes are; its package `plymouth-plugin-two-step`).
pub const PLUGIN: &str = "usr/lib64/plymouth/two-step.so";
pub const PLUGIN_PACKAGE: &str = "plymouth-plugin-two-step";
/// The stamp's name in the theme.
pub const STAMP: &str = "olo-made.json";
/// The drawing's version: one more each time plymouth.rs draws the
/// theme differently, so an installed one is seen to be old.
pub const FORMAT: u32 = 1;

/// What a theme was made with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Made {
    pub format: u32,
    pub scale: f64,
}

/// How the boot splash is on the system under `root`.
#[derive(Debug, Clone, PartialEq)]
pub enum Installed {
    /// Plymouth shows another theme (its name, if it says).
    Not(Option<String>),
    /// Olo's, made by another drawing or at another scale (what it was
    /// made with: none from before the stamp).
    Old(Option<Made>),
    /// Olo's and current, but changed since the start-up image was made:
    /// not shown until it's made again.
    NotInImage,
    Current,
}

/// The theme Plymouth's settings under `root` name, if any.
pub fn theme(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(CONF.trim_start_matches('/'))).ok()?;
    text.lines().filter_map(|l| l.trim().strip_prefix("Theme=")).last().map(|t| t.trim().to_owned())
}

/// How Olo's boot splash is installed under `root`, against one made now
/// at `scale`, for the kernel running (`release`, its start-up image
/// /boot/initramfs-<release>.img).
pub fn installed(root: &Path, scale: f64, release: &str) -> Installed {
    let named = theme(root);
    let dir = root.join(THEME_DIR.trim_start_matches('/'));
    if named.as_deref() != Some("olo") || !dir.join("olo.plymouth").exists() {
        return Installed::Not(named.filter(|t| t != "olo"));
    }
    let made: Option<Made> = std::fs::read(dir.join(STAMP)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if made.is_none_or(|m| m.format != FORMAT || (m.scale - scale).abs() > 0.01) {
        return Installed::Old(made);
    }
    // (The theme's and the settings' newest change, against the image's.)
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let changed = [dir.join(STAMP), root.join(CONF.trim_start_matches('/'))].iter().filter_map(|p| modified(p)).max();
    let image = modified(&root.join(format!("boot/initramfs-{release}.img")));
    match (changed, image) {
        (Some(c), Some(i)) if c > i => Installed::NotInImage,
        _ => Installed::Current,
    }
}

/// The scale the computer's own screen is at in Olo (`display`, app_server's
/// display settings): its built-in panel's (eDP, LVDS, DSI) when one is
/// connected, else the main display's, else Olo's default. `sys` is
/// /sys (a test's folder).
pub fn boot_scale(display: &olo_protocol::DisplaySettings, sys: &Path) -> f64 {
    if let Some(name) = built_in_panels(sys).into_iter().next() {
        let (maker, model, serial) = crate::edid::for_connector(&name).map(|e| (e.maker, e.model.unwrap_or_default(), e.serial.unwrap_or_default())).unwrap_or_default();
        return display.scale_for(&olo_protocol::display_key(&name, &maker, &model, &serial), &name);
    }
    match &display.main {
        Some(main) => display.scale_for(main, main),
        None => display.scale,
    }
}

/// The connected built-in panels' connectors ("eDP-1"), in order.
pub fn built_in_panels(sys: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(sys.join("class/drm"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (_, connector) = name.split_once('-')?;
            let built_in = ["eDP-", "LVDS-", "DSI-"].iter().any(|p| connector.starts_with(p));
            (built_in && std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "connected")).then(|| connector.to_owned())
        })
        .collect();
    v.sort();
    v
}

/// The person's display settings, from app_server's file in their config
/// folder (`config`: ~/.config), as app_server reads them.
pub fn display_settings(config: &Path) -> olo_protocol::DisplaySettings {
    std::fs::read_to_string(config.join("olo/app_server.json")).ok().and_then(|t| serde_json::from_str::<olo_protocol::Settings>(&t).ok()).map(|s| s.display).unwrap_or_default()
}

/// The theme's folder under `root`.
pub fn theme_dir(root: &Path) -> PathBuf {
    root.join(THEME_DIR.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// Fedora's, then Olo's old and current, then changed after the
    /// image was made.
    #[test]
    fn how_it_is_installed() {
        let root = PathBuf::from(format!("/var/tmp/olo-splash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "etc/plymouth/plymouthd.conf", "[Daemon]\nTheme=bgrt\n");
        assert_eq!(installed(&root, 2.0, "6.1"), Installed::Not(Some("bgrt".into())));
        write(&root, "etc/plymouth/plymouthd.conf", OLO_CONF);
        write(&root, "usr/share/plymouth/themes/olo/olo.plymouth", "");
        assert_eq!(installed(&root, 2.0, "6.1"), Installed::Old(None), "from before the stamp");
        write(&root, "usr/share/plymouth/themes/olo/olo-made.json", r#"{"format":1,"scale":1.5}"#);
        assert_eq!(installed(&root, 2.0, "6.1"), Installed::Old(Some(Made { format: 1, scale: 1.5 })), "made at another scale");
        write(&root, "usr/share/plymouth/themes/olo/olo-made.json", r#"{"format":1,"scale":2.0}"#);
        write(&root, "boot/initramfs-6.1.img", "");
        // (The image older than the theme: as a file's time set back.)
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(root.join("boot/initramfs-6.1.img")).unwrap().set_modified(past).unwrap();
        assert_eq!(installed(&root, 2.0, "6.1"), Installed::NotInImage);
        write(&root, "boot/initramfs-6.1.img", "made again");
        assert_eq!(installed(&root, 2.0, "6.1"), Installed::Current);
        let _ = std::fs::remove_dir_all(&root);
    }
}
