//! Olo is Wayland only: no X11 code or dependencies (CLAUDE.md). What
//! the workspace builds is in Cargo.lock, so the crates that are X11's
//! own mustn't be there: winit's X11 backend and the Xlib and XCB
//! bindings. (They came in once through the vendored Smithay asking
//! winit for its "x11" feature, and were built into app_server until
//! 2026-11-12.)
//!
//! `x11rb` is let be: the vendored Smithay names it as an optional
//! dependency of its X11 backend and of Xwayland, which nothing turns
//! on, and a lock file lists optional dependencies too. Its features
//! being off is the other half, checked here in Smithay's own manifest.

use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace's root")
}

#[test]
fn no_x11_crate_is_built() {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("Cargo.lock");
    let names: Vec<&str> = lock.lines().filter_map(|l| l.strip_prefix("name = \"")).filter_map(|l| l.strip_suffix('"')).collect();
    assert!(names.contains(&"smithay"), "the lock file read isn't the workspace's");
    let x11: Vec<&&str> = names.iter().filter(|n| matches!(**n, "winit-x11" | "x11-dl" | "x11" | "xcb" | "x11-clipboard" | "xkbcommon-x11") || n.starts_with("xcb-")).collect();
    assert!(x11.is_empty(), "X11 crates in the build (Olo is Wayland only): {x11:?}");
}

#[test]
fn smithay_is_asked_for_nothing_of_x11() {
    // app_server's own features of it, and what it asks winit for.
    let ours = std::fs::read_to_string(root().join("crates/app_server/Cargo.toml")).expect("app_server's manifest");
    for feature in ["smithay/backend_x11", "smithay/xwayland", "smithay/x11rb_event_source"] {
        assert!(!ours.contains(feature), "app_server turns on {feature}");
    }
    let smithay = std::fs::read_to_string(root().join("smithay/smithay/Cargo.toml")).expect("Smithay's manifest");
    let winit = smithay.lines().find(|l| l.starts_with("winit = ")).expect("Smithay's winit dependency");
    assert!(!winit.contains("\"x11\""), "the vendored Smithay asks winit for X11: {winit}");
    let default = smithay.lines().find(|l| l.starts_with("default = ")).expect("Smithay's default features");
    assert_eq!(default.trim(), "default = []", "the vendored Smithay has default features again");
}
