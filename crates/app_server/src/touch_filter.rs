//! Olo's touch filter (Preferences ▸ Touchpad & Mouse): a libinput plugin,
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
    let sections: Vec<String> =
        DEVICES.iter().map(|d| format!("[Olo touch filter: {d}]\nMatchUdevType=touchpad\nMatchName=*{d}*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPalmPressureThreshold=0\n")).collect();
    format!("# Written by Olo's app_server while its touch filter is on (docs/TOUCHPAD.md).\n\n{}", sections.join("\n"))
}

/// The plugin for these settings, or None when the filter is off.
pub fn render(t: &TouchpadSettings) -> Option<String> {
    let f = &t.filter;
    if !f.enabled {
        return None;
    }
    let devices: Vec<String> = DEVICES.iter().map(|d| format!("{d:?}")).collect();
    // The palm model, when it's to be used: only one trained on the
    // person's hands (Train for Your Palm). Olo's own is where training
    // starts, not one to use untrained (Ben, 2026-11-02: "it does not
    // work well without training").
    let model = match (&f.palm_model, f.use_palm_model) {
        (Some(m), true) if m.w1.len() == olo_palm::HIDDEN * olo_palm::FEATURES => format!(", palm_model = {}", olo_palm::lua(m)),
        _ => String::new(),
    };
    let config = format!(
        "local CONFIG = {{ devices = {{ {} }}, touch_on = {}, touch_off = {}, lift_ms = {}, palm_size = {}, click = {}, press = {}, release = {}{model} }}",
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
            let written =
                std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)).and_then(|_| std::fs::create_dir_all(&quirks_dir)).and_then(|_| std::fs::write(&quirks_path, quirks()));
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
        assert!(lua.contains("local CONFIG = { devices = { \"Apple MTP\" }, touch_on = 1, touch_off = 1, lift_ms = 60, palm_size = 1600, click = true, press = 120, release = 75 }"));
        // Turned on, untrained: no model (Olo's own isn't used as it is)...
        t.filter.use_palm_model = true;
        assert!(render(&t).unwrap().contains("release = 75 }"));
        // ...trained, the person's.
        t.filter.palm_model = Some(olo_palm::baseline());
        assert!(render(&t).unwrap().contains("release = 75, palm_model = { mean = {"));
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
    const POS_X: u32 = 0x3_0035;
    const POS_Y: u32 = 0x3_0036;
    const BTN_LEFT: u32 = 0x1_0110;

    /// The plugin in a stand-in for libinput's Lua API (usages are
    /// `type << 16 | code`; a frame is a list of `{usage, value}`).
    struct Pad {
        lua: mlua::Lua,
        time: i64,
    }

    impl Pad {
        fn new(t: &TouchpadSettings) -> Pad {
            Pad::with(&render(t).unwrap())
        }

        /// The plugin as `source` (rendered) has it.
        fn with(source: &str) -> Pad {
            let lua = mlua::Lua::new();
            lua.load(format!(
                "evdev = {{ ABS_MT_SLOT = {SLOT}, ABS_MT_TOUCH_MAJOR = {MAJOR}, ABS_MT_TOUCH_MINOR = {MINOR}, \
                 ABS_MT_TRACKING_ID = {ID}, ABS_MT_PRESSURE = {PRESSURE}, ABS_PRESSURE = 0x30018, BTN_LEFT = {BTN_LEFT}, \
                 ABS_MT_POSITION_X = {POS_X}, ABS_MT_POSITION_Y = {POS_Y}, ABS_MT_ORIENTATION = 0x30034, ABS_MT_WIDTH_MAJOR = 0x30032, KEY_ESC = 0x10001, KEY_A = 0x1001e, KEY_SPACE = 0x10039, BTN_MISC = 0x10100 }}
                 hooks = {{}}
                 resolution = {{ [{PRESSURE}] = 1, [0x30018] = 1 }}
                 libinput = {{ register = function() end, log_info = function() end,
                               connect = function(_, _, f) hooks.new = f end }}
                 pad = {{ name = function() return 'Apple MTP multi-touch' end,
                          usages = function() return {{ [{PRESSURE}] = true, [{MAJOR}] = true, [{MINOR}] = true }} end,
                          absinfos = function() return {{ [{PRESSURE}] = {{ resolution = resolution[{PRESSURE}], minimum = 0, maximum = 6000 }}, [0x30018] = {{ resolution = resolution[0x30018] }},
                                                         [{POS_X}] = {{ minimum = -7700, maximum = 7700 }}, [{POS_Y}] = {{ minimum = -200, maximum = 9200 }}, [{MAJOR}] = {{ minimum = 0, maximum = 5000 }}, [0x30034] = {{ minimum = -16384, maximum = 16384 }} }} end,
                          set_absinfo = function(_, u, a) if a.resolution then resolution[u] = a.resolution end end,
                          connect = function(_, _, f) hooks.frame = f end }}"
            ))
            .exec()
            .unwrap();
            lua.load(source).exec().unwrap();
            lua.load("hooks.new(pad)").exec().unwrap();
            Pad { lua, time: 0 }
        }

        /// One frame, `ms` after the last; what libinput is given (never
        /// more than came: libinput would keep the frame as it came).
        fn frame(&mut self, ms: i64, events: &[(u32, i32)]) -> Vec<(u32, i32)> {
            let out = self.raw_frame(ms * 1000, events);
            assert!(out.len() <= events.len(), "a frame grew: {events:?} became {out:?}");
            out
        }

        /// One frame `us` after the last, as the plugin answers it.
        fn raw_frame(&mut self, us: i64, events: &[(u32, i32)]) -> Vec<(u32, i32)> {
            self.time += us;
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

    /// What a frame costs the filter, the palm model off and on (two
    /// fingers moving, a palm resting): `cargo test --release -p
    /// app_server frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn frame_cost() {
        use olo_palm::{FEATURES, HIDDEN, PalmModel};
        let model = PalmModel {
            mean: vec![0.0; FEATURES],
            std: vec![1.0; FEATURES],
            w1: vec![0.01; HIDDEN * FEATURES],
            b1: vec![0.0; HIDDEN],
            w2: vec![0.1; HIDDEN],
            b2: 0.0,
            threshold: 1e9,
            decide_ms: 250,
            ..Default::default()
        };
        for on in [false, true] {
            let mut t = TouchpadSettings::default();
            t.filter.palm_model = Some(model.clone());
            t.filter.use_palm_model = on;
            let mut pad = Pad::new(&t);
            let start = std::time::Instant::now();
            const FRAMES: i32 = 20_000;
            for k in 0..FRAMES {
                // A new set of contacts every 100 frames (each judged anew).
                let id = k / 100;
                let mut events = vec![];
                for (slot, (x, size)) in [(k * 3, 500), (k * 3 + 900, 520), (6000, 2500)].into_iter().enumerate() {
                    events.push((SLOT, slot as i32));
                    if k % 100 == 0 {
                        events.push((ID, id * 3 + slot as i32));
                    }
                    events.extend([(POS_X, x % 7000), (POS_Y, 3000), (PRESSURE, 30), (MAJOR, size), (MINOR, size - 50)]);
                }
                pad.frame(8, &events);
            }
            let us = start.elapsed().as_secs_f64() * 1e6 / FRAMES as f64;
            println!("palm model {}: {us:.2} µs a frame (3 contacts)", if on { "on" } else { "off" });
        }
    }

    /// The filter's palm model scores contacts as `olo_palm` does: with
    /// the threshold just under Rust's highest score in the contact's
    /// first `decide_ms` it's left out as a palm, just over it a finger,
    /// so the two agree to a ten-thousandth.
    #[test]
    fn palm_model_scores_as_rust_does() {
        use olo_palm::{FEATURES, HIDDEN, PadAxes, PalmModel, TouchContact, Tracker};
        // A model of no training: seeded weights, every feature in use.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut r = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        };
        let model = PalmModel {
            mean: (0..FEATURES).map(|_| r() * 0.2).collect(),
            std: (0..FEATURES).map(|_| 0.2 + r().abs()).collect(),
            w1: (0..HIDDEN * FEATURES).map(|_| r()).collect(),
            b1: (0..HIDDEN).map(|_| r()).collect(),
            w2: (0..HIDDEN).map(|_| r()).collect(),
            b2: r(),
            decide_ms: 60,
            ..Default::default()
        };
        // A contact moving, growing and pressing harder, a report every 8 ms.
        let path: Vec<TouchContact> =
            (0..12).map(|k| TouchContact { id: 5, x: -3000 + k * 140, y: 1200 + k * 90, major: 600 + k * 35, minor: 450 + k * 20, pressure: 20 + k * 4, ..Default::default() }).collect();
        let axes = PadAxes::macbook();
        let mut tracker = Tracker::default();
        let mut best = f32::MIN;
        for (k, c) in path.iter().enumerate() {
            let (_, f, age) = tracker.frame(&axes, 8000 * (k as u64 + 1), &[*c])[0];
            if age <= model.decide_ms {
                best = best.max(olo_palm::score(&model, &f));
            }
        }
        let run = |threshold: f32| {
            let mut t = TouchpadSettings::default();
            t.filter.palm_model = Some(PalmModel { threshold, ..model.clone() });
            t.filter.use_palm_model = true;
            let mut pad = Pad::new(&t);
            let mut last = Vec::new();
            for (k, c) in path.iter().enumerate() {
                let mut events = vec![(SLOT, 0)];
                if k == 0 {
                    events.push((ID, c.id));
                }
                events.extend([(POS_X, c.x), (POS_Y, c.y), (PRESSURE, c.pressure), (MAJOR, c.major), (MINOR, c.minor)]);
                last = sizes(&pad.frame(8, &events));
            }
            last
        };
        assert_eq!(run(best - 1e-4), [(MAJOR, 0), (MINOR, 0)], "a palm by Rust's score ({best})");
        let path_last = path.last().unwrap();
        assert_eq!(run(best + 1e-4), [(MAJOR, path_last.major), (MINOR, path_last.minor)], "a finger by Rust's score ({best})");
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
        // Pressing, it gets its size, major first (in place of the size
        // the pad sent, and in the room its pressure leaves).
        let out = pad.frame(8, &[(PRESSURE, 30), (MAJOR, 510)]);
        assert_eq!(sizes(&out), [(MAJOR, 510), (MINOR, 500)]);
        // A palm lands in slot 1 and the driver clicks from the sum.
        let out = pad.frame(8, &[(SLOT, 1), (ID, 2), (PRESSURE, 400), (MAJOR, 3000), (MINOR, 3200), (BTN_LEFT, 1)]);
        assert!(!out.contains(&(BTN_LEFT, 1)), "{out:?}");
        assert_eq!(sizes(&out), [(MAJOR, 0), (MINOR, 0)], "a palm has no size");
        // It stays a palm when it shrinks; the finger pressing hard enough
        // on its own makes the driver's click count.
        let out = pad.frame(8, &[(MAJOR, 600), (MINOR, 600), (SLOT, 0), (PRESSURE, 130)]);
        assert_eq!(sizes(&out), [(MAJOR, 0), (MINOR, 0)]);
        assert!(out.contains(&(BTN_LEFT, 1)), "{out:?}");
        // It lets go when the finger does, though the palm keeps the
        // driver's click down.
        let out = pad.frame(8, &[(PRESSURE, 70)]);
        assert!(out.contains(&(BTN_LEFT, 0)), "{out:?}");
        assert!(!pad.frame(8, &[(BTN_LEFT, 0)]).iter().any(|&(u, _)| u == BTN_LEFT));
    }

    /// A contact just short of a palm pressed 134 on the 16" M2 Max and the
    /// driver didn't click: neither may Olo (a click nobody felt).
    #[test]
    fn no_click_unless_the_driver_clicks() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        let out = pad.frame(8, &[(SLOT, 0), (ID, 1), (PRESSURE, 134), (MAJOR, 1576), (MINOR, 1200)]);
        assert!(!out.iter().any(|&(u, _)| u == BTN_LEFT), "{out:?}");
        // Without a palm the driver's click passes as it is, both ways.
        let out = pad.frame(8, &[(PRESSURE, 90), (BTN_LEFT, 1)]);
        assert!(out.contains(&(BTN_LEFT, 1)), "{out:?}");
        let out = pad.frame(8, &[(PRESSURE, 140)]);
        assert!(!out.iter().any(|&(u, _)| u == BTN_LEFT));
        let out = pad.frame(8, &[(PRESSURE, 60), (BTN_LEFT, 0)]);
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
    fn a_touch_ends_only_after_leaving_the_glass() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        // (In the middle of the pad: still and light at its very edge, a
        // contact is a heel resting there, to the palm model.)
        pad.frame(8, &[(SLOT, 0), (ID, 1), (MID_X, 0), (MID_Y, 4500), (PRESSURE, 20), (MAJOR, 400), (MINOR, 300)]);
        // A moment with no pressure while moving keeps the touch...
        assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, 0)])), []);
        assert_eq!(sizes(&pad.frame(40, &[(PRESSURE, 0)])), []);
        assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, 2)])), []);
        // ...staying off the glass for lift_ms ends it (no major is no
        // touch; the minor follows when there's room).
        pad.frame(8, &[(PRESSURE, 0)]);
        assert_eq!(sizes(&pad.frame(70, &[(PRESSURE, 0)])), [(MAJOR, 0)]);
        assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, 0)])), [(MINOR, 0)]);
        // A new contact in the slot starts afresh.
        let out = pad.frame(8, &[(ID, -1)]);
        assert_eq!(sizes(&out), []);
        let out = pad.frame(8, &[(ID, 2), (PRESSURE, 25), (MAJOR, 400), (MINOR, 300)]);
        assert_eq!(sizes(&out), [(MAJOR, 400), (MINOR, 300)]);
    }

    /// A contact's place (`ABS_MT_POSITION_X`, `_Y`).
    const MID_X: u32 = 0x3_0035;
    const MID_Y: u32 = 0x3_0036;

    /// A fast, light swipe on the 16" M2 Max peaked at pressure 5 over
    /// 12 mm; a threshold of 13 lost it (docs/TOUCHPAD.md).
    #[test]
    fn a_light_swipe_counts() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        let out = pad.frame(8, &[(SLOT, 0), (ID, 1), (MID_X, 0), (MID_Y, 4500), (PRESSURE, 0), (MAJOR, 300), (MINOR, 794)]);
        assert_eq!(sizes(&out), [(MAJOR, 0), (MINOR, 0)], "not yet on the glass");
        let out = pad.frame(8, &[(PRESSURE, 3), (MINOR, 790)]);
        assert_eq!(sizes(&out), [(MINOR, 300), (MAJOR, 790)]);
        for p in [5, 4, 2, 1] {
            assert_eq!(sizes(&pad.frame(8, &[(PRESSURE, p)])), []);
        }
    }

    /// libinput's view of the pad after a frame: each slot's tracking id
    /// and size, and which contacts it takes for touches (the touch-size
    /// range, 450:150 here: a touch starts above 450 and ends below 150).
    #[derive(Default)]
    struct Seen {
        slot: i32,
        slots: std::collections::HashMap<i32, (i32, i32, i32)>,
        touching: std::collections::BTreeSet<i32>,
    }

    impl Seen {
        fn apply(&mut self, frame: &[(u32, i32)]) {
            for &(u, v) in frame {
                let s = self.slots.entry(self.slot).or_insert((-1, 0, 0));
                match u {
                    SLOT => self.slot = v,
                    ID => *s = (v, s.1, s.2),
                    MAJOR => s.1 = v,
                    MINOR => s.2 = v,
                    _ => {}
                }
            }
            for (&n, &(id, major, minor)) in &self.slots {
                if id < 0 || major < 150 || minor < 1 {
                    self.touching.remove(&n);
                } else if major > 450 {
                    self.touching.insert(n);
                }
            }
        }
    }

    /// Five contacts, three pressing at once in a frame of their pressures
    /// alone: nothing grows, and libinput's view catches up as they move.
    #[test]
    fn frames_never_grow() {
        let mut pad = Pad::new(&TouchpadSettings::default());
        let mut seen = Seen::default();
        let mut frame = vec![];
        for n in 0..5 {
            frame.extend([(SLOT, n), (ID, 10 + n), (PRESSURE, 0), (MAJOR, 600), (MINOR, 500)]);
        }
        seen.apply(&pad.frame(8, &frame));
        assert!(seen.touching.is_empty(), "hovering");
        // Three press at once; the pad sends only their pressure.
        let out = pad.frame(8, &[(SLOT, 0), (PRESSURE, 20), (SLOT, 1), (PRESSURE, 20), (SLOT, 2), (PRESSURE, 20)]);
        seen.apply(&out);
        // As they move, each reports where it is (and its pressure).
        for k in 0..3 {
            let mut f = vec![];
            for n in 0..3 {
                f.extend([(SLOT, n), (0x3_0035, 100 + k), (PRESSURE, 21 + k)]);
            }
            seen.apply(&pad.frame(8, &f));
        }
        assert_eq!(seen.touching, [0, 1, 2].into());
    }

    /// Replays a capture of the pad (raw `input_event`s, as
    /// `OLO_TOUCH_CAPTURE` names; `OLO_TOUCH_OLD` an older plugin to
    /// compare) and says how often a frame would have grown, and how
    /// long libinput's touches differed from what the filter meant.
    #[test]
    #[ignore]
    fn replay_capture() {
        let Ok(path) = std::env::var("OLO_TOUCH_CAPTURE") else { return };
        let data = std::fs::read(path).unwrap();
        let mut frames: Vec<(i64, Vec<(u32, i32)>)> = vec![];
        let mut cur = vec![];
        for e in data.chunks_exact(24) {
            let sec = i64::from_ne_bytes(e[0..8].try_into().unwrap());
            let usec = i64::from_ne_bytes(e[8..16].try_into().unwrap());
            let (typ, code) = (u16::from_ne_bytes([e[16], e[17]]) as u32, u16::from_ne_bytes([e[18], e[19]]) as u32);
            let value = i32::from_ne_bytes(e[20..24].try_into().unwrap());
            if typ == 0 && code == 0 {
                frames.push((sec * 1_000_000 + usec, std::mem::take(&mut cur)));
            } else if typ != 0 {
                cur.push((typ << 16 | code, value));
            }
        }
        let t = TouchpadSettings::default();
        let new = render(&t).unwrap();
        let old = std::env::var("OLO_TOUCH_OLD")
            .ok()
            .map(|p| std::fs::read_to_string(p).unwrap().replacen("local CONFIG = {} -- @CONFIG@ (replaced by app_server)", new.lines().find(|l| l.starts_with("local CONFIG")).unwrap(), 1));
        // libinput's limit: what it was given (a frame it held back), or
        // 63 (one it hadn't).
        let run = |source: &str, limit: Option<usize>| {
            let mut pad = Pad::with(source);
            let (mut seen, mut ideal) = (Seen::default(), Seen::default());
            let (mut grown, mut differ, mut longest, mut run_us, mut last) = (0, 0, 0i64, 0i64, frames.first().map_or(0, |f| f.0));
            for (time, events) in &frames {
                let out = pad.raw_frame(time - last, events);
                ideal.apply(&out);
                if out.len() > limit.unwrap_or(events.len()) {
                    grown += 1;
                    // libinput keeps the frame as the pad sent it.
                    seen.apply(events);
                } else {
                    seen.apply(&out);
                }
                if seen.touching != ideal.touching {
                    differ += 1;
                    run_us += time - last;
                    longest = longest.max(run_us);
                } else {
                    run_us = 0;
                }
                last = *time;
            }
            println!("{} frames: {grown} too big, {differ} with libinput's touches not the filter's (longest {} ms)", frames.len(), longest / 1000);
        };
        for limit in [None, Some(63)] {
            if let Some(old) = &old {
                print!("before, limit {limit:?}: ");
                run(old, limit);
            }
            print!("now, limit {limit:?}: ");
            run(&new, limit);
        }
    }
}
