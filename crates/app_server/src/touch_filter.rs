//! Olo's touch filter (Preferences ▸ Touchpad): a libinput plugin,
//! `touchpad.lua`, that app_server writes with the settings filled in and
//! loads into its own libinput, so that on MacBook pads a hovering finger
//! is not a touch, palms are left out, and a palm can't click. libinput
//! plugins sit between the kernel and libinput (libinput 1.30 and later);
//! see docs/TOUCHPAD.md for the measurements behind it.
//!
//! The plugin is read once, when libinput starts, so `udev.rs` starts
//! libinput again when the filter's settings change.
//!
//! These pads say they are pressure pads (`INPUT_PROP_PRESSUREPAD`), and
//! libinput drops pressure from those before plugins see it. So with the
//! filter on, app_server also writes a quirk into the user's own quirks
//! (`$XDG_RUNTIME_DIR/libinput/`, read by libinput 1.31) that takes the
//! property away and turns off libinput's palm detection by pressure
//! (the filter does palms), and the plugin clears the pressure axis's
//! resolution, libinput's other sign of a pressure pad.

use std::path::PathBuf;

use olo_protocol::TouchpadSettings;

/// The devices it filters, by a part of their name: MacBook pads on Apple
/// silicon, which report each finger's pressure (and whose driver makes
/// the click from the pressure of everything touching).
pub const DEVICES: &[&str] = &["Apple MTP"];

const TEMPLATE: &str = include_str!("touchpad.lua");
const FILE: &str = "50-olo-touchpad.lua";
const QUIRKS_FILE: &str = "olo-touchpad.quirks";

/// The quirk that lets pressure through to the plugin.
pub fn quirks() -> String {
    let sections: Vec<String> = DEVICES
        .iter()
        .map(|d| format!("[Olo touch filter: {d}]\nMatchUdevType=touchpad\nMatchName=*{d}*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPalmPressureThreshold=0\n"))
        .collect();
    format!("# Written by Olo's app_server while its touch filter is on (docs/TOUCHPAD.md).\n\n{}", sections.join("\n"))
}

/// The plugin for these settings, or None when the filter is off.
pub fn render(t: &TouchpadSettings) -> Option<String> {
    let f = &t.filter;
    if !f.enabled {
        return None;
    }
    let devices: Vec<String> = DEVICES.iter().map(|d| format!("{d:?}")).collect();
    let config = format!(
        "local CONFIG = {{ devices = {{ {} }}, touch_on = {}, touch_off = {}, lift_ms = {}, palm_size = {}, click = {}, press = {}, release = {} }}",
        devices.join(", "),
        f.touch_on,
        f.touch_off.min(f.touch_on),
        f.lift_ms,
        f.palm_size,
        f.palm_proof_click,
        f.click_press,
        f.click_release.min(f.click_press),
    );
    Some(TEMPLATE.replacen("local CONFIG = {} -- @CONFIG@ (replaced by app_server)", &config, 1))
}

/// Where app_server keeps the plugins it writes (the session's own).
pub fn dir() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    runtime.join("olo").join("libinput-plugins")
}

/// The user's own libinput quirks.
fn quirks_dir() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    runtime.join("libinput")
}

/// Write (or remove) the plugin and quirk for `t`, before libinput
/// starts; the plugin written.
pub fn install(t: &TouchpadSettings) -> Option<String> {
    let plugin = render(t);
    let (dir, quirks_dir) = (dir(), quirks_dir());
    let (path, quirks_path) = (dir.join(FILE), quirks_dir.join(QUIRKS_FILE));
    match &plugin {
        Some(text) => {
            let written = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(&path, text))
                .and_then(|_| std::fs::create_dir_all(&quirks_dir))
                .and_then(|_| std::fs::write(&quirks_path, quirks()));
            if let Err(e) = written {
                tracing::warn!(%e, path = %path.display(), "could not write the touch filter");
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(&quirks_path);
                return None;
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&quirks_path);
        }
    }
    plugin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_settings() {
        let mut t = TouchpadSettings::default();
        let lua = render(&t).unwrap();
        assert!(lua.contains("local CONFIG = { devices = { \"Apple MTP\" }, touch_on = 13, touch_off = 3, lift_ms = 60, palm_size = 1600, click = true, press = 120, release = 75 }"));
        assert!(!lua.contains("@CONFIG@"));
        t.filter.enabled = false;
        assert_eq!(render(&t), None);
    }

    #[test]
    fn the_quirk_lets_pressure_through() {
        let q = quirks();
        assert!(q.contains("[Olo touch filter: Apple MTP]\nMatchUdevType=touchpad\nMatchName=*Apple MTP*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPalmPressureThreshold=0\n"));
    }

    const SLOT: u32 = 0x3_002f;
    const MAJOR: u32 = 0x3_0030;
    const MINOR: u32 = 0x3_0031;
    const ID: u32 = 0x3_0039;
    const PRESSURE: u32 = 0x3_003a;
    const BTN_LEFT: u32 = 0x1_0110;

    /// The plugin in a stand-in for libinput's Lua API (usages are
    /// `type << 16 | code`; a frame is a list of `{usage, value}`).
    struct Pad {
        lua: mlua::Lua,
        time: i64,
    }

    impl Pad {
        fn new(t: &TouchpadSettings) -> Pad {
            let lua = mlua::Lua::new();
            lua.load(format!(
                "evdev = {{ ABS_MT_SLOT = {SLOT}, ABS_MT_TOUCH_MAJOR = {MAJOR}, ABS_MT_TOUCH_MINOR = {MINOR}, \
                 ABS_MT_TRACKING_ID = {ID}, ABS_MT_PRESSURE = {PRESSURE}, ABS_PRESSURE = 0x30018, BTN_LEFT = {BTN_LEFT} }}
                 hooks = {{}}
                 resolution = {{ [{PRESSURE}] = 1, [0x30018] = 1 }}
                 libinput = {{ register = function() end, log_info = function() end,
                               connect = function(_, _, f) hooks.new = f end }}
                 pad = {{ name = function() return 'Apple MTP multi-touch' end,
                          usages = function() return {{ [{PRESSURE}] = true, [{MAJOR}] = true, [{MINOR}] = true }} end,
                          absinfos = function() return {{ [{PRESSURE}] = {{ resolution = resolution[{PRESSURE}] }}, [0x30018] = {{ resolution = resolution[0x30018] }} }} end,
                          set_absinfo = function(_, u, a) if a.resolution then resolution[u] = a.resolution end end,
                          connect = function(_, _, f) hooks.frame = f end }}"
            ))
            .exec()
            .unwrap();
            lua.load(render(t).unwrap()).exec().unwrap();
            lua.load("hooks.new(pad)").exec().unwrap();
            Pad { lua, time: 0 }
        }

        /// One frame, `ms` after the last; what libinput is given.
        fn frame(&mut self, ms: i64, events: &[(u32, i32)]) -> Vec<(u32, i32)> {
            self.time += ms * 1000;
            let frame = self.lua.create_table().unwrap();
            for &(usage, value) in events {
                let e = self.lua.create_table().unwrap();
                e.set("usage", usage).unwrap();
                e.set("value", value).unwrap();
                frame.push(e).unwrap();
            }
            let hook: mlua::Function = self.lua.load("hooks.frame").eval().unwrap();
            // nil: the frame goes on as it came.
            let Some(out): Option<mlua::Table> = hook.call((mlua::Value::Nil, frame, self.time)).unwrap() else { return events.to_vec() };
            out.sequence_values::<mlua::Table>().map(|e| e.unwrap()).map(|e| (e.get("usage").unwrap(), e.get("value").unwrap())).collect()
        }
    }

    fn sizes(out: &[(u32, i32)]) -> Vec<(u32, i32)> {
        out.iter().copied().filter(|&(u, _)| u == MAJOR || u == MINOR).collect()
    }

    #[test]
    fn hover_and_palms_have_no_size_and_cant_click() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        // The pressure axes lose their resolution, so libinput keeps them.
        let res: (i64, i64) = pad.lua.load(format!("return resolution[{PRESSURE}], resolution[0x30018]")).eval().unwrap();
        assert_eq!(res, (0, 0));
        // A finger hovering (no pressure) is reported with no size.
        let out = pad.frame(8, &[(SLOT, 0), (ID, 1), (PRESSURE, 0), (MAJOR, 300), (MINOR, 500)]);
        assert_eq!(sizes(&out), [(MAJOR, 0), (MINOR, 0)]);
        // Pressing, it gets its size, major first.
        let out = pad.frame(8, &[(PRESSURE, 30)]);
        assert_eq!(sizes(&out), [(MAJOR, 500), (MINOR, 300)]);
        assert_eq!(out.last(), Some(&(SLOT, 0)));
        // A palm lands in slot 1 and the driver clicks from the sum.
        let out = pad.frame(8, &[(SLOT, 1), (ID, 2), (PRESSURE, 400), (MAJOR, 3000), (MINOR, 3200), (BTN_LEFT, 1)]);
        assert!(!out.contains(&(BTN_LEFT, 1)), "{out:?}");
        assert_eq!(sizes(&out), [(MAJOR, 0), (MINOR, 0)], "a palm has no size");
        // It stays a palm when it shrinks, and the finger clicks alone.
        let out = pad.frame(8, &[(MAJOR, 600), (MINOR, 600), (SLOT, 0), (PRESSURE, 130)]);
        assert_eq!(sizes(&out), []);
        assert!(out.contains(&(BTN_LEFT, 1)), "{out:?}");
        // ...and lets go when the finger does, though the palm presses on.
        let out = pad.frame(8, &[(PRESSURE, 70), (BTN_LEFT, 0)]);
        assert!(out.contains(&(BTN_LEFT, 0)), "{out:?}");
    }

    /// What the 16" M2 Max's pad looked like through libinput: sizes,
    /// widths, orientation and position, but never a pressure. The frames
    /// must pass untouched (they once made every touch a hover).
    #[test]
    fn without_pressure_frames_pass_untouched() {
        const WIDTH: u32 = 0x3_0032;
        const X: u32 = 0x3_0035;
        let mut pad = Pad::new(&TouchpadSettings::default());
        let landing = [(ID, 18115), (MAJOR, 54), (MINOR, 98), (WIDTH, 1564), (X, 3191), (BTN_LEFT, 1)];
        assert_eq!(pad.frame(8, &landing), landing);
        let flat = [(MAJOR, 1030), (MINOR, 674), (BTN_LEFT, 0)];
        assert_eq!(pad.frame(8, &flat), flat);
    }

    #[test]
    fn a_touch_ends_only_after_staying_light() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        pad.frame(8, &[(SLOT, 0), (ID, 1), (PRESSURE, 20), (MAJOR, 400), (MINOR, 300)]);
        // A light dip while moving keeps the touch...
        assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, 2)])), []);
        assert_eq!(sizes(&pad.frame(40, &[(PRESSURE, 1)])), []);
        assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, 9)])), []);
        // ...staying light for lift_ms ends it.
        pad.frame(8, &[(PRESSURE, 2)]);
        assert_eq!(sizes(&pad.frame(70, &[(PRESSURE, 2)])), [(MAJOR, 0), (MINOR, 0)]);
        // A new contact in the slot starts afresh.
        let out = pad.frame(8, &[(ID, -1)]);
        assert_eq!(sizes(&out), []);
        let out = pad.frame(8, &[(ID, 2), (PRESSURE, 25), (MAJOR, 400), (MINOR, 300)]);
        assert_eq!(sizes(&out), [(MAJOR, 400), (MINOR, 300)]);
    }
}
