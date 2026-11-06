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

/// The ThinkPad pads it keeps palms palms on (`olo_palm::Kind::ThinkPad`:
/// the P1 Gen 9's Goodix), by a part of their name: their firmware's palm
/// held until it lifts, and their own palm model.
pub const THINKPADS: &[&str] = &["GXTP5420"];

const TEMPLATE: &str = include_str!("touchpad.lua");
const FILE: &str = "50-olo-touchpad.lua";
const QUIRKS_FILE: &str = "olo-touchpad.quirks";

/// The quirk that lets pressure through to the plugin. A ThinkPad's
/// pressure is its haptic click's force, which libinput's own quirk drops
/// (a pressure pad): let through for the palm model, libinput is told not
/// to count touches by it (`AttrPressureRange=none`, its "pressure-based
/// touch detection disabled": its default threshold, 240 of 2000, is past
/// any fingertip here) nor palms (`AttrPalmPressureThreshold=0`: its
/// default, 130, is under a fingertip's click, and a palm by pressure is
/// one for good). Every line is one libinput's loader takes ([`takes`]):
/// one it doesn't, and it loads no quirk at all, its own included (Olo's
/// `AttrPressureRange=0:0` did, 2026-11-05: libinput's own quirk for the
/// ThinkPad's pad went with it, and it counted touches by pressure, past
/// any fingertip there, and the touchpad was dead).
pub fn quirks() -> String {
    let mut sections: Vec<String> =
        DEVICES.iter().map(|d| format!("[Olo touch filter: {d}]\nMatchUdevType=touchpad\nMatchName=*{d}*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPalmPressureThreshold=0\n")).collect();
    sections.extend(
        THINKPADS
            .iter()
            .map(|d| format!("[Olo touch filter: {d}]\nMatchUdevType=touchpad\nMatchName=*{d}*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPressureRange=none\nAttrPalmPressureThreshold=0\n")),
    );
    format!("# Written by Olo's app_server while its touch filter is on (docs/TOUCHPAD.md).\n\n{}", sections.join("\n"))
}

/// The plugin for these settings, or None when the filter is off.
pub fn render(t: &TouchpadSettings) -> Option<String> {
    let f = &t.filter;
    if !f.enabled {
        return None;
    }
    let devices: Vec<String> = DEVICES.iter().map(|d| format!("{d:?}")).collect();
    let thinkpads: Vec<String> = THINKPADS.iter().map(|d| format!("{d:?}")).collect();
    let model = model_in_use(f, olo_palm::Kind::MacBook).map(|m| format!(", palm_model = {}", olo_palm::lua(&m))).unwrap_or_default();
    let thinkpad_model = model_in_use(f, olo_palm::Kind::ThinkPad).map(|m| format!(", thinkpad_model = {}", olo_palm::lua(&m))).unwrap_or_default();
    let config = format!(
        "local CONFIG = {{ devices = {{ {} }}, thinkpads = {{ {} }}, touch_on = {}, touch_off = {}, lift_ms = {}, palm_size = {}, click = {}, press = {}, release = {}{model}{thinkpad_model} }}",
        devices.join(", "),
        thinkpads.join(", "),
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

/// The palm model the filter runs on a `kind` of pad, when it's on: the
/// person's, trained on their hands on such a pad (Train for Your Palm),
/// or Olo's own for that kind, made from real hands on it
/// (`olo_palm::Kind::base_model`; Ben, 2026-11-03: "make this one that
/// you're improving from my data the new default/untrained model";
/// 2026-11-05: "each touchpad type should have its own specific custom
/// model type and weights"). A model of another shape (from an older Olo)
/// or kind gives way to Olo's own.
pub fn model_in_use(f: &olo_protocol::touchpad::TouchFilter, kind: olo_palm::Kind) -> Option<olo_protocol::touchpad::PalmModel> {
    f.use_palm_model.then(|| kind.model(f.palm_model.as_ref()))
}

/// The kind of pad among `devices` (a ThinkPad's if one is there, else a
/// MacBook's), for what its model says of itself.
pub fn kind_among(devices: &[olo_protocol::touchpad::InputDeviceInfo]) -> olo_palm::Kind {
    if devices.iter().any(|d| d.kind == olo_protocol::touchpad::DeviceKind::Thinkpad) { olo_palm::Kind::ThinkPad } else { olo_palm::Kind::MacBook }
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
    // (A quirk libinput wouldn't take is never written: it would take
    // every quirk with it, the pads' own.)
    let quirks = quirks();
    let bad: Vec<&str> = quirks.lines().filter(|l| !takes(l)).collect();
    if !bad.is_empty() {
        tracing::error!(?bad, "the touch filter's quirk has lines libinput wouldn't take: written without it");
    }
    match &plugin {
        Some(text) => {
            let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)).and_then(|_| {
                if bad.is_empty() {
                    std::fs::create_dir_all(&quirks_dir).and_then(|_| std::fs::write(&quirks_path, &quirks))
                } else {
                    let _ = std::fs::remove_file(&quirks_path);
                    Ok(())
                }
            });
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

/// Whether libinput's quirks loader (1.32's `quirks.c`, `parse_match` and
/// `parse_attr`, with `util-prop-parsers.c`) takes `line` of a quirks
/// file, for what Olo writes: a section, a comment, and the matches and
/// attributes Olo uses, as libinput parses them. Anything else isn't
/// taken (Olo writes nothing it hasn't checked against the parser).
pub fn takes(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return true;
    }
    if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        return !name.is_empty();
    }
    let Some((key, value)) = line.split_once('=') else { return false };
    match key {
        "MatchUdevType" => matches!(value, "touchpad" | "mouse" | "pointingstick" | "keyboard" | "joystick" | "tablet" | "tablet-pad"),
        "MatchName" => !value.is_empty(),
        // `+` or `-` and a property's name (or its number, in hex), `;`
        // between them.
        "AttrInputProp" => value.split(';').all(|p| {
            let (sign, name) = p.split_at(p.len().min(1));
            matches!(sign, "+" | "-")
                && (u32::from_str_radix(name.strip_prefix("0x").unwrap_or(name), 16).is_ok_and(|n| n <= 0x1f)
                    || matches!(
                        name,
                        "INPUT_PROP_POINTER"
                            | "INPUT_PROP_DIRECT"
                            | "INPUT_PROP_BUTTONPAD"
                            | "INPUT_PROP_SEMI_MT"
                            | "INPUT_PROP_TOPBUTTONPAD"
                            | "INPUT_PROP_POINTING_STICK"
                            | "INPUT_PROP_ACCELEROMETER"
                            | "INPUT_PROP_PRESSUREPAD"
                    ))
        }),
        // `none`, or `high:low` with low under high (`0:0` is not taken).
        "AttrPressureRange" | "AttrTouchSizeRange" => {
            value == "none" || value.split_once(':').and_then(|(hi, lo)| Some((hi.parse::<i32>().ok()?, lo.parse::<i32>().ok()?))).is_some_and(|(hi, lo)| lo < hi)
        }
        "AttrPalmPressureThreshold" | "AttrThumbPressureThreshold" => value.parse::<u32>().is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_settings() {
        let mut t = TouchpadSettings::default();
        let lua = render(&t).unwrap();
        // On by default, untrained: Olo's own model...
        let own = olo_palm::lua(&olo_palm::default_model());
        let thinkpad = olo_palm::lua(&olo_palm::thinkpad_model());
        assert!(lua.contains(&format!(
            "local CONFIG = {{ devices = {{ \"Apple MTP\" }}, thinkpads = {{ \"GXTP5420\" }}, touch_on = 1, touch_off = 1, lift_ms = 60, palm_size = 1600, click = true, press = 120, release = 75, palm_model = {own}, thinkpad_model = {thinkpad} }}"
        )));
        // ...trained, the person's...
        t.filter.palm_model = Some(olo_palm::baseline());
        let theirs = render(&t).unwrap();
        assert!(theirs.contains(&format!("release = 75, palm_model = {}, thinkpad_model = {thinkpad} }}", olo_palm::lua(&olo_palm::baseline()))));
        // (A model trained on a ThinkPad is the ThinkPad's: the MacBook's
        // own stays.)
        t.filter.palm_model = Some(olo_protocol::touchpad::PalmModel { trained: 3, ..olo_palm::thinkpad_model() });
        let tp = render(&t).unwrap();
        assert!(tp.contains(&format!("palm_model = {own},")) && tp.contains(&format!("thinkpad_model = {}", olo_palm::lua(t.filter.palm_model.as_ref().unwrap()))));
        // ...one of another shape, Olo's own; off, none.
        t.filter.palm_model = Some(olo_protocol::touchpad::PalmModel { w1: vec![0.0; 7], ..olo_palm::baseline() });
        assert!(render(&t).unwrap().contains(&own));
        t.filter.use_palm_model = false;
        assert!(render(&t).unwrap().contains("release = 75 }"));
        assert!(!lua.contains("@CONFIG@"));
        t.filter.enabled = false;
        assert_eq!(render(&t), None);
    }

    #[test]
    fn the_quirk_lets_pressure_through() {
        let q = quirks();
        assert!(q.contains("[Olo touch filter: Apple MTP]\nMatchUdevType=touchpad\nMatchName=*Apple MTP*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPalmPressureThreshold=0\n"));
        // A ThinkPad's: through, but no touch or palm by it in libinput.
        assert!(q.contains("[Olo touch filter: GXTP5420]\nMatchUdevType=touchpad\nMatchName=*GXTP5420*\nAttrInputProp=-INPUT_PROP_PRESSUREPAD\nAttrPressureRange=none\nAttrPalmPressureThreshold=0\n"));
        // Every line one libinput's loader takes.
        for line in q.lines() {
            assert!(takes(line), "libinput wouldn't take {line:?}");
        }
    }

    /// As libinput 1.32's parser has them: what broke every quirk on the
    /// ThinkPad (`0:0`), and what it takes.
    #[test]
    fn quirk_lines_as_libinput_takes_them() {
        assert!(!takes("AttrPressureRange=0:0"), "0:0 fails libinput's parse_range_property (low must be under high)");
        assert!(!takes("AttrPressureRange=10:20") && takes("AttrPressureRange=20:10") && takes("AttrPressureRange=none"));
        assert!(takes("AttrInputProp=-INPUT_PROP_PRESSUREPAD") && takes("AttrInputProp=+INPUT_PROP_BUTTONPAD;-0x07"));
        assert!(!takes("AttrInputProp=INPUT_PROP_PRESSUREPAD") && !takes("AttrInputProp=-INPUT_PROP_NOPE"));
        assert!(takes("AttrPalmPressureThreshold=0") && !takes("AttrPalmPressureThreshold=-1"));
        assert!(takes("MatchUdevType=touchpad") && !takes("MatchUdevType=pad"));
        assert!(!takes("AttrSomethingNew=1"), "nothing Olo hasn't checked against the parser");
    }

    const SLOT: u32 = 0x3_002f;
    const MAJOR: u32 = 0x3_0030;
    const MINOR: u32 = 0x3_0031;
    const ID: u32 = 0x3_0039;
    const PRESSURE: u32 = 0x3_003a;
    const POS_X: u32 = 0x3_0035;
    const POS_Y: u32 = 0x3_0036;
    const BTN_LEFT: u32 = 0x1_0110;
    const TOOL: u32 = 0x3_0037;
    const ABS_X: u32 = 0x3_0000;
    const ABS_Y: u32 = 0x3_0001;

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
                 ABS_MT_POSITION_X = {POS_X}, ABS_MT_POSITION_Y = {POS_Y}, ABS_MT_ORIENTATION = 0x30034, ABS_MT_WIDTH_MAJOR = 0x30032, KEY_ESC = 0x10001, KEY_A = 0x1001e, KEY_SPACE = 0x10039, \
                 ABS_MT_TOOL_TYPE = {TOOL}, ABS_X = {ABS_X}, ABS_Y = {ABS_Y} }}
                 -- (A name libinput's table hasn't is nil there: an error here.)
                 setmetatable(evdev, {{ __index = function(_, k) error('libinput names no ' .. k) end }})
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

        /// The plugin as `t` renders it, on a ThinkPad's pad (the Goodix:
        /// no size, its firmware's palm, its pressure the click's force).
        fn thinkpad(t: &TouchpadSettings) -> Pad {
            let pad = Pad::with(&render(t).unwrap());
            pad.lua
                .load(format!(
                    "local tp = {{ name = function() return 'GXTP5420:00 27C6:0F90 Touchpad' end,
                                   usages = function() return {{ [{PRESSURE}] = true, [{TOOL}] = true, [{POS_X}] = true, [{POS_Y}] = true }} end,
                                   absinfos = function() return {{ [{PRESSURE}] = {{ resolution = 0, minimum = 0, maximum = 2000 }},
                                                                  [{POS_X}] = {{ minimum = 0, maximum = 3599, resolution = 27 }}, [{POS_Y}] = {{ minimum = 0, maximum = 1799, resolution = 21 }} }} end,
                                   set_absinfo = function() end,
                                   connect = function(_, _, f) hooks.frame = f end }}
                     hooks.new(tp)"
                ))
                .exec()
                .unwrap();
            pad
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
    fn a_keyboards_keys_are_noted_with_names_libinput_has() {
        // libinput's evdev table has no BTN_MISC (an alias): the plugin
        // compared a key with nil and libinput unloaded it at the first
        // key pressed, on every start of the ThinkPad's session.
        let pad = Pad::new(&TouchpadSettings::default());
        pad.lua
            .load(
                "local kb = { name = function() return 'Magic Keyboard' end, usages = function() return { [evdev.KEY_A] = true } end,
                              connect = function(_, _, f) hooks.key = f end }
                 hooks.new(kb)
                 hooks.key(kb, { { usage = evdev.KEY_A, value = 1 } }, 1000)",
            )
            .exec()
            .unwrap();
    }

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
        // (Off; a model without bounds, as measured before; Olo's own, with
        // its bounds, never calling a palm so every contact is judged.)
        let own = PalmModel { threshold: 1e9, ..olo_palm::default_model() };
        for (name, on, model) in [("off", false, model.clone()), ("on", true, model.clone()), ("Olo's own, bounded", true, own)] {
            let mut t = TouchpadSettings::default();
            t.filter.palm_model = Some(model);
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
            println!("palm model {name}: {us:.2} µs a frame (3 contacts)");
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
        let mean: Vec<f32> = (0..FEATURES).map(|_| r() * 0.2).collect();
        let model = PalmModel {
            std: (0..FEATURES).map(|_| 0.2 + r().abs()).collect(),
            // (Bounds that bite: the contact goes past some of them.)
            lo: mean.iter().map(|m| m - 0.05).collect(),
            hi: mean.iter().map(|m| m + 0.05).collect(),
            mean,
            w1: (0..HIDDEN * FEATURES).map(|_| r()).collect(),
            b1: (0..HIDDEN).map(|_| r()).collect(),
            w2: (0..HIDDEN).map(|_| r()).collect(),
            b2: r(),
            decide_ms: 60,
            ..Default::default()
        };
        // A contact hovering first (no pressure: not followed yet), then
        // moving, growing and pressing harder, a report every 8 ms.
        let path: Vec<TouchContact> = (0..15)
            .map(|k| TouchContact { id: 5, x: -3000 + k * 140, y: 1200 + k * 90, major: 600 + k * 35, minor: 450 + k * 20, pressure: if k < 3 { 0 } else { 20 + k * 4 }, ..Default::default() })
            .collect();
        let axes = PadAxes::macbook();
        let mut tracker = Tracker::default();
        let mut best = f32::MIN;
        for (k, c) in path.iter().enumerate() {
            let Some(&(_, f, age)) = tracker.frame(&axes, 8000 * (k as u64 + 1), &[*c]).first() else {
                assert!(k < 3, "followed from its first touch");
                continue;
            };
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

    /// Ben's hands on the ThinkPad's pad (crates/palm's captures), through
    /// the plugin as libinput would have them: each report as the kernel
    /// would send it (only what changed, a slot each contact; the
    /// single-touch axes too). What libinput is told of each contact: a
    /// palm is one from its first few reports until it lifts (across the
    /// kernel's new ids for it), however often the firmware calls it a
    /// finger again; a finger never is.
    #[test]
    fn a_thinkpads_palms_stay_palms() {
        let captures = [
            olo_palm::Capture::parse(include_bytes!("../../palm/data/thinkpad-2026-11-05.json")).unwrap(),
            olo_palm::Capture::parse(include_bytes!("../../palm/data/thinkpad-2026-11-05-fingers.json")).unwrap(),
        ];
        let mut t = TouchpadSettings::default();
        t.apply_profile("thinkpad");
        let (mut flips, mut palms_held, mut fingers) = (0, 0, 0);
        for cap in &captures {
            let mut pad = Pad::thinkpad(&t);
            // The kernel's slots (id, x, y, pressure, tool), and libinput's
            // view of each slot's tool as the plugin leaves it.
            let mut kernel: [(i32, i32, i32, i32, i32); 5] = [(-1, 0, 0, 0, 0); 5];
            let mut given = [0i32; 5];
            let (mut kernel_slot, mut seen_slot) = (0usize, 0usize);
            let mut last_us = cap.reports.first().map_or(0, |r| r.0);
            // Each contact: its label, its reports so far, and whether
            // libinput was told palm after its first few.
            let mut labels: std::collections::HashMap<i32, (bool, u32)> = Default::default();
            // (A palm's new ids: what it is is its first id's.)
            let mut same_as: std::collections::HashMap<i32, i32> = Default::default();
            let mut ended: Vec<(u64, i32, i32, i32)> = Vec::new();
            for (time_us, _, cs) in &cap.reports {
                let mut events = Vec::new();
                // Ended contacts first (their slot's id to -1).
                for s in 0..5 {
                    if kernel[s].0 >= 0 && !cs.iter().any(|c| c[0] == kernel[s].0) {
                        if kernel_slot != s {
                            events.push((SLOT, s as i32));
                            kernel_slot = s;
                        }
                        events.push((ID, -1));
                        ended.push((*time_us, kernel[s].0, kernel[s].1, kernel[s].2));
                        kernel[s].0 = -1;
                    }
                }
                for c in cs {
                    let [id, x, y, p, tool] = *c;
                    let s = match kernel.iter().position(|k| k.0 == id) {
                        Some(s) => s,
                        None => kernel.iter().position(|k| k.0 < 0).expect("a free slot"),
                    };
                    let mut e = Vec::new();
                    if kernel[s].0 != id {
                        e.push((ID, id));
                        // A palm's contact back under a new id, nearby.
                        if let Some(&(_, old, ..)) = ended.iter().rev().find(|(at, _, ex, ey)| time_us - at <= 40_000 && (ex - x).abs() < 140 && (ey - y).abs() < 110) {
                            let root = *same_as.get(&old).unwrap_or(&old);
                            same_as.insert(id, root);
                        }
                    }
                    if kernel[s].1 != x || kernel[s].0 != id {
                        e.push((POS_X, x));
                    }
                    if kernel[s].2 != y || kernel[s].0 != id {
                        e.push((POS_Y, y));
                    }
                    if kernel[s].3 != p {
                        e.push((PRESSURE, p));
                    }
                    if kernel[s].4 != tool {
                        e.push((TOOL, tool));
                    }
                    if !e.is_empty() {
                        if kernel_slot != s {
                            events.push((SLOT, s as i32));
                            kernel_slot = s;
                        }
                        events.extend(e);
                    }
                    if s == 0 && (kernel[0].1 != x || kernel[0].2 != y) {
                        // (The single-touch axes follow the first contact.)
                        events.push((ABS_X, x));
                        events.push((ABS_Y, y));
                    }
                    kernel[s] = (id, x, y, p, tool);
                }
                ended.retain(|(at, ..)| time_us - at <= 40_000);
                if events.is_empty() {
                    continue;
                }
                let us = (*time_us - last_us).max(1) as i64;
                last_us = *time_us;
                let out = pad.raw_frame(us, &events);
                assert!(out.len() <= events.len(), "a frame grew: {events:?} became {out:?}");
                for (u, v) in out {
                    if u == SLOT {
                        seen_slot = v as usize;
                    } else if u == TOOL {
                        given[seen_slot] = v;
                    }
                }
                seen_slot = kernel_slot;
                for c in cs {
                    let id = c[0];
                    let root = *same_as.get(&id).unwrap_or(&id);
                    let step_palm = cap.steps.iter().find(|s| (s.from_us..s.to_us).contains(time_us)).is_some_and(|s| s.palm);
                    let first_palm = labels.get(&root).map(|l| l.0);
                    let label = first_palm.unwrap_or(step_palm || cap.palms.contains(&id) || cap.palms.contains(&root));
                    let l = labels.entry(root).or_insert((label, 0));
                    l.1 += 1;
                    let s = kernel.iter().position(|k| k.0 == id).unwrap();
                    let told_palm = given[s] == olo_palm::MT_TOOL_PALM;
                    if l.0 {
                        if c[4] != olo_palm::MT_TOOL_PALM && l.1 > 5 {
                            flips += 1;
                        }
                        if l.1 > 5 {
                            assert!(told_palm, "a palm (contact {id}, its report {}) told a finger", l.1);
                            palms_held += 1;
                        }
                    } else {
                        assert!(!told_palm, "a finger (contact {id}, its report {}) told a palm", l.1);
                        fingers += 1;
                    }
                }
            }
        }
        // (The firmware's own flips back to a finger were there to hold.)
        assert!(flips > 100 && palms_held > 1000 && fingers > 5000, "{flips} flips, {palms_held} palm reports, {fingers} finger reports");
    }

    /// What a frame costs on a ThinkPad's pad (Ben's fingers' capture,
    /// replayed): `cargo test --release -p app_server --lib
    /// thinkpad_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn thinkpad_frame_cost() {
        let cap = olo_palm::Capture::parse(include_bytes!("../../palm/data/thinkpad-2026-11-05-fingers.json")).unwrap();
        let mut t = TouchpadSettings::default();
        t.apply_profile("thinkpad");
        // The frames as the kernel would send them: each contact's slot
        // (in order), what changed.
        let mut frames = Vec::new();
        let mut kernel: [(i32, i32, i32, i32, i32); 5] = [(-1, 0, 0, 0, 0); 5];
        for (_, _, cs) in &cap.reports {
            let mut e = Vec::new();
            for s in 0..5 {
                if kernel[s].0 >= 0 && !cs.iter().any(|c| c[0] == kernel[s].0) {
                    e.extend([(SLOT, s as i32), (ID, -1)]);
                    kernel[s].0 = -1;
                }
            }
            for c in cs {
                let s = kernel.iter().position(|k| k.0 == c[0]).or_else(|| kernel.iter().position(|k| k.0 < 0)).unwrap();
                e.push((SLOT, s as i32));
                if kernel[s].0 != c[0] {
                    e.push((ID, c[0]));
                }
                e.extend([(POS_X, c[1]), (POS_Y, c[2]), (PRESSURE, c[3]), (TOOL, c[4]), (ABS_X, c[1]), (ABS_Y, c[2])]);
                kernel[s] = (c[0], c[1], c[2], c[3], c[4]);
            }
            frames.push(e);
        }
        let mut pad = Pad::thinkpad(&t);
        let start = std::time::Instant::now();
        for _ in 0..3 {
            for f in &frames {
                pad.raw_frame(7000, f);
            }
        }
        let us = start.elapsed().as_secs_f64() * 1e6 / (3 * frames.len()) as f64;
        println!("a ThinkPad's frame: {us:.2} µs ({} frames)", frames.len());
    }
}
