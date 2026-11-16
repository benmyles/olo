//! What Olo needs installed from Fedora's own repositories to run: what
//! its programs link or open (dlopen), the services a session talks to,
//! and the fonts it doesn't carry. The Installer installs those missing
//! (through PackageKit); Olo Doctor says when one is missing on a
//! computer Olo is on, and installs it (`checks::needed_packages`).
//!
//! Checked against a minimal Fedora 45 (2026-11-15: a `@core` install,
//! no desktop): it lacked Wayland's server library, which app_server
//! opens as it starts (Smithay's `Display::new`: "NoWaylandLib"); every
//! Workstation has it, so the list never named it. Each library Olo's
//! programs name to open is on this list, or comes with something on it,
//! or is a GPU maker's own (NVIDIA's) or a toolkit's looked for only to
//! know it.

/// The packages, by name.
pub const PACKAGES: &[&str] = &[
    "libwayland-server",
    "libwayland-client",
    "mesa-dri-drivers",
    "mesa-vulkan-drivers",
    "vulkan-loader",
    "mesa-libEGL",
    "mesa-libgbm",
    "libseat",
    "libdisplay-info",
    "libinput",
    "libwacom",
    "pixman",
    "lua-libs",
    "libgudev",
    "pipewire",
    "pipewire-libs",
    "wireplumber",
    "xdg-desktop-portal",
    "polkit",
    "NetworkManager",
    "bluez",
    "upower",
    "accountsservice",
    "google-noto-sans-cjk-vf-fonts",
];
