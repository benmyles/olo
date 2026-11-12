//! Keyboard: layout, the keyboard profile (Mac, Windows or BeOS keys:
//! the command key, Caps Lock as Ctrl, a tap of the Super key for the
//! palette, and every shortcut, in the Shortcuts tab, shortcuts.rs), key
//! repeat, hints, the backlight; each keyboard's own (Keyboards: Alt
//! and the Windows key swapped on a PC keyboard, for a Mac's ⌘ where Alt
//! is, not on an Apple one); and the computer's own power button (Power
//! Button: a press, and holds). A platform's profile changed becomes
//! "Mac (Customized)" by itself; Save As keeps one under a name of its
//! own, to pick again.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use olo_kit::{
    Child, Cx, ParentElement as _, SharedString, Styled as _, div, metrics as m, px,
    widgets::{Button, MenuField, Slider, TabView, TextControl, TextField},
};
use olo_protocol::{Client, CommandKey, Event, EventKind, KeyboardProfile, Platform, PowerStatus, Reply, Request, Settings};

use crate::{Msg, PanelView, form::Ui, panel, server::Server};

pub struct State {
    test: Child<TextField>,
    /// The level being dragged to, until app_server's report catches up.
    backlight: Option<(f32, Instant)>,
    tab: usize,
    pub shortcuts: crate::shortcuts::State,
    /// The keyboards plugged in (app_server's `Request::Keyboards`, again
    /// as they come and go): None until it says.
    keyboards: Arc<Mutex<Option<Vec<String>>>>,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let test = olo_kit::widgets::text_field(cx, |f| f.hint("Type here to try the settings"));
        State { test, backlight: None, tab: 0, shortcuts: crate::shortcuts::State::new(cx), keyboards: watch_keyboards(olo_kit::proxy()) }
    }
}

pub fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Keyboard(st) => Some(st),
        _ => None,
    }
}

/// Common layouts: xkb name and what to call it.
const LAYOUTS: &[(&str, &str)] = &[
    ("us", "English (US)"),
    ("gb", "English (UK)"),
    ("au", "English (Australia)"),
    ("ca", "French (Canada)"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("br", "Portuguese (Brazil)"),
    ("nl", "Dutch"),
    ("se", "Swedish"),
    ("no", "Norwegian"),
    ("dk", "Danish"),
    ("fi", "Finnish"),
    ("ch", "Swiss"),
    ("pl", "Polish"),
    ("cz", "Czech"),
    ("ru", "Russian"),
    ("jp", "Japanese"),
];

/// The keyboards plugged in, kept up to date on a thread (subscribed to
/// app_server's device events before asking, as Touchpad's devices are).
fn watch_keyboards(proxy: olo_kit::Proxy) -> Arc<Mutex<Option<Vec<String>>>> {
    let shared: Arc<Mutex<Option<Vec<String>>>> = Default::default();
    // Tests (the layout lint), and a look headless (`OLO_KEYBOARDS_DEMO`):
    // a laptop's and an Apple one, named as they come.
    if cfg!(test) || std::env::var_os("OLO_KEYBOARDS_DEMO").is_some() {
        *shared.lock().unwrap() = Some(vec![BUILT_IN.into(), "Apple Inc. Magic Keyboard with Touch ID and Numeric Keypad".into()]);
        return shared;
    }
    let out = Arc::downgrade(&shared);
    std::thread::Builder::new()
        .name("keyboards".into())
        .spawn(move || {
            loop {
                let events = Client::connect().ok().and_then(|c| c.into_events_of(&[EventKind::InputDevices]).ok());
                let ask = || {
                    let names = Client::connect().ok().and_then(|mut c| match c.request(&Request::Keyboards) {
                        Ok(Reply::Keyboards { names }) => Some(names),
                        _ => None,
                    });
                    let Some(shared) = out.upgrade() else { return false };
                    let mut slot = shared.lock().unwrap();
                    if names.is_some() && *slot != names {
                        *slot = names;
                        proxy.send(Msg::Redraw);
                    }
                    true
                };
                if !ask() {
                    return;
                }
                for e in events.into_iter().flatten() {
                    let Ok(Event::InputDevicesChanged { .. }) = e else { break };
                    if !ask() {
                        return;
                    }
                }
                if out.strong_count() == 0 {
                    return;
                }
                // app_server went (a restart): connect again once it's back.
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("thread");
    shared
}

/// A laptop's own keyboard, as the kernel names it (PS/2, through the
/// keyboard controller).
const BUILT_IN: &str = "AT Translated Set 2 keyboard";

/// What to call a keyboard.
fn keyboard_name(name: &str) -> String {
    if name == BUILT_IN { "Built-in Keyboard".into() } else { name.to_owned() }
}

/// The Keyboards tab shown (the layout lint's).
#[cfg(test)]
pub fn keyboards_tab(st: &mut State) {
    st.tab = 2;
}

/// The Power Button tab shown (the layout lint's).
#[cfg(test)]
pub fn power_button_tab(st: &mut State) {
    st.tab = 3;
}

/// Size of a known keyboard's picture (points).
const KEYBOARD_PICTURE: f32 = 64.0;

/// A keyboard Olo knows: its picture beside what it is, and its Touch ID
/// as it stands here.
fn known_keyboard(ui: &mut Ui, model: &'static olo_protocol::keyboards::Model, name: &str) {
    use olo_kit::{canvas, widgets::StringView};
    let mut facts = vec![format!("{} {}", model.maker, model.model)];
    facts.push(if model.keypad { "full size, with a numeric keypad".into() } else { "compact".into() });
    facts.push(model.connection.to_owned());
    let picture = canvas(move |g, b| {
        let at = olo_kit::style::snap_centred(g, b.center(), KEYBOARD_PICTURE, KEYBOARD_PICTURE);
        olo_kit::icons::paint(g, at, model.icon);
    })
    .w(px(KEYBOARD_PICTURE))
    .h(px(KEYBOARD_PICTURE))
    .flex_shrink_0();
    let touch_id = model.touch_id.map(|t| match t {
        olo_protocol::keyboards::TouchId::SealedToAMac => {
            "Touch ID isn't used here: its sensor works only with the Mac it's paired to, whose Secure Enclave alone can read what it senses. Use the computer's own fingerprint reader, or your password."
        }
    });
    ui.bbox(model.name, |ui| {
        let words =
            div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).child(StringView::new(facts.join(" · ")).wrap(true)).children(touch_id.map(|t| StringView::new(t).dim(true).wrap(true)));
        ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(picture).child(words));
        if model.fn_row {
            // Its own: the top row's way round (by its name, as Alt and
            // the Windows key), and the Globe key (every Apple keyboard).
            let k = &ui.s.keyboard;
            let (first, globe) = (k.fkeys_first.iter().any(|n| n == name), k.globe_emoji);
            let key = name.to_owned();
            ui.check("Use F1, F2, etc. as standard function keys", first, move |s, v| {
                let list = &mut s.keyboard.fkeys_first;
                list.retain(|n| *n != key);
                if v {
                    list.push(key.clone());
                }
            });
            ui.note(if first { "Hold Fn (🌐) for what's printed on them: brightness, Mission Control, Spotlight, media and volume." } else { "Hold Fn (🌐) for F1 to F12." });
            ui.check("Press 🌐 alone to show emoji", globe, |s, v| s.keyboard.globe_emoji = v);
        }
    });
}

/// Each keyboard's own settings: Alt and the Windows key swapped, or not
/// (a check box for each, by name).
fn keyboards(ui: &mut Ui, st: &State) {
    let plugged = st.keyboards.lock().unwrap().clone();
    let Some(plugged) = plugged else {
        ui.note("Reading…");
        return;
    };
    let swapped = ui.s.keyboard.swap_alt_super.clone();
    // Those plugged in, then those set up and away (to undo).
    let mut all: Vec<(String, bool)> = plugged.iter().map(|n| (n.clone(), true)).collect();
    all.extend(swapped.iter().filter(|n| !plugged.contains(n)).map(|n| (n.clone(), false)));
    if all.is_empty() {
        ui.note("No keyboards are plugged in.");
        return;
    }
    // Keyboards Olo knows: each in a box of its own, with its picture and
    // what it has.
    for (name, model) in plugged.iter().filter_map(|n| olo_protocol::keyboards::identify(n).map(|m| (n, m))) {
        known_keyboard(ui, model, name);
    }
    ui.bbox("Swap Alt and the Windows Key", |ui| {
        for (name, here) in all {
            let label = if here { keyboard_name(&name) } else { format!("{} (not connected)", keyboard_name(&name)) };
            let key = name.clone();
            ui.check(label, swapped.contains(&name), move |s, v| {
                let list = &mut s.keyboard.swap_alt_super;
                list.retain(|n| *n != key);
                if v {
                    list.push(key.clone());
                }
            });
        }
    });
    ui.note("On a PC keyboard, Alt is where a Mac's ⌘ is: swapped, the key beside the space bar is ⌘ (with the Mac profile) and the Windows key is Option. Each keyboard keeps its own setting, so an Apple keyboard plugged in stays as it is.");
}

pub fn ui(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let items = ui.collect(|ui| match st.tab {
        0 => keyboard(ui, power, server, st),
        1 => crate::shortcuts::ui(ui, server, &mut st.shortcuts),
        2 => keyboards(ui, st),
        _ => power_button(ui),
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Keyboard", "Shortcuts", "Keyboards", "Power Button"].into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("keyboard-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

fn keyboard(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let k = ui.s.keyboard.clone();
    let mut names: Vec<SharedString> = LAYOUTS.iter().map(|(_, n)| SharedString::from(*n)).collect();
    let i = LAYOUTS.iter().position(|(id, _)| *id == k.layout).unwrap_or_else(|| {
        // Something set elsewhere: show it as is.
        names.push(k.layout.clone().into());
        names.len() - 1
    });
    ui.menu(
        "layout",
        names,
        i,
        |f| f.label("Layout:").divider(60.0),
        |s, i| {
            if let Some((id, _)) = LAYOUTS.get(i)
                && *id != s.keyboard.layout
            {
                s.keyboard.layout = id.to_string();
                s.keyboard.variant.clear();
            }
        },
    );

    let test = &st.test;
    ui.bbox("Key Repeat", |ui| {
        let rate = k.repeat_rate_hz as f32;
        ui.slider(
            "repeat-rate",
            rate,
            5.0..=60.0,
            |s| s.step(1.0).label("Repeat rate").value_text(format!("{rate:.0} per second")).limits("Slow", "Fast"),
            |s, v| s.keyboard.repeat_rate_hz = v as i32,
        );
        let delay = k.repeat_delay_ms as f32;
        ui.slider(
            "repeat-delay",
            delay,
            150.0..=1000.0,
            |s| s.step(25.0).label("Delay until repeat").value_text(format!("{delay:.0} ms")).limits("Short", "Long"),
            |s, v| s.keyboard.repeat_delay_ms = v as i32,
        );
        ui.add(TextControl::new(ui.cx, test).label("Typing test:"));
    });

    ui.check("Num Lock on when Olo starts", k.num_lock, |s, v| s.keyboard.num_lock = v);

    ui.bbox("Keyboard Hints", |ui| {
        ui.check("Press Control twice for keyboard hints", k.hints, |s, v| s.keyboard.hints = v);
        let speed = k.double_press_ms as f32;
        ui.enabled_if(k.hints, |ui| {
            ui.slider(
                "double-press",
                speed,
                150.0..=800.0,
                |s| s.step(25.0).label("Double-press speed").value_text(format!("{speed:.0} ms")).limits("Fast", "Slow"),
                |s, v| s.keyboard.double_press_ms = v as u32,
            );
        });
        ui.note("A letter goes on everything you can pick: controls, menu items, windows, Deskbar. Type one to pick it, or to letter what’s inside; Backspace goes back, Esc puts them away.");
    });

    ui.bbox("Backlight", |ui| match power.and_then(|p| p.keyboard_backlight) {
        Some(level) => {
            if st.backlight.is_some_and(|(_, at)| at.elapsed() > crate::form::HELD) {
                st.backlight = None;
            }
            let v = st.backlight.map_or(level, |(l, _)| l);
            let text = if v == 0.0 { "Off".to_owned() } else { format!("{:.0}%", v * 100.0) };
            let server = server.clone();
            let on = ui.on_view(move |view, v: &f32, cx| {
                if let Some(st) = state_of(view) {
                    st.backlight = Some((*v, Instant::now()));
                }
                server.set_keyboard_backlight(*v);
                crate::form::again_once_let_go(cx);
                cx.notify();
            });
            ui.add(Slider::new("backlight", v, 0.0..=1.0).step(1.0 / 64.0).label("Brightness").value_text(text).limits("Off", "Bright").on_change(on));
        }
        None => ui.note("This keyboard has no backlight."),
    });
}

/// Whether Olo's helper for Immediately is installed. Never, to the
/// layout lint: it draws the tab at its tallest, and its settings index
/// comes out the same on every computer.
fn helper_installed() -> bool {
    !cfg!(test) && olo_system::power::helper_installed()
}

/// The Power Button tab: what the computer's own button does pressed, and
/// held for each hold's seconds (app_server's power_button.rs does it).
fn power_button(ui: &mut Ui) {
    use olo_protocol::power_button::{HOLD_SECONDS, PowerAction};
    let pb = ui.s.keyboard.power_button.clone();
    let actions: Vec<SharedString> = PowerAction::ALL.iter().map(|a| SharedString::from(a.label())).collect();
    let of = |a: PowerAction| PowerAction::ALL.iter().position(|x| *x == a).unwrap_or(0);
    let divider = crate::form::divider_for(&["When pressed:", "When held for:"]);
    ui.menu("power-press", actions.clone(), of(pb.press), |f| f.label("When pressed:").divider(divider), |s, i| s.keyboard.power_button.press = PowerAction::ALL[i]);
    // Each hold, by its seconds (the order they come in).
    let mut order: Vec<usize> = (0..pb.holds.len()).collect();
    order.sort_by_key(|&i| pb.holds[i].seconds);
    for i in order {
        let hold = pb.holds[i];
        let seconds: Vec<SharedString> = HOLD_SECONDS.iter().map(|n| SharedString::from(if *n == 1 { "1 second".to_owned() } else { format!("{n} seconds") })).collect();
        let at = HOLD_SECONDS.iter().position(|n| *n == hold.seconds).unwrap_or(0);
        let actions = actions.clone();
        ui.row(|ui| {
            ui.menu(
                &format!("power-hold-{i}"),
                seconds,
                at,
                |f| f.label("When held for:").divider(divider),
                move |s, k| {
                    if let Some(h) = s.keyboard.power_button.holds.get_mut(i) {
                        h.seconds = HOLD_SECONDS[k];
                    }
                },
            );
            ui.menu(
                &format!("power-hold-{i}-does"),
                actions,
                of(hold.action),
                |f| f,
                move |s, k| {
                    if let Some(h) = s.keyboard.power_button.holds.get_mut(i) {
                        h.action = PowerAction::ALL[k];
                    }
                },
            );
        });
    }
    if pb.wants_helper() && !helper_installed() {
        ui.note("Immediately needs a small helper that only the administrator can install. Until it's installed, Immediately restarts or shuts down as Restart and Shut Down do.");
        ui.button("Install the Helper…", |_, _cx| {
            let proxy = olo_kit::proxy();
            // (Olo Doctor's fix: polkit asks for the password.)
            std::thread::spawn(move || {
                match std::process::Command::new("olo-doctor").args(["--fix", "power-now"]).status() {
                    Ok(s) if !s.success() => tracing::info!("preferences: the power button's helper wasn't installed ({s})"),
                    Err(e) => tracing::warn!("preferences: olo-doctor: {e}"),
                    _ => {}
                }
                proxy.send(Msg::Redraw);
            });
        });
    }
    ui.note("Restart and Shut Down end every program and stop the system in order, as Deskbar's menu does; Immediately doesn't wait for that. Held long, most computers' own firmware cuts the power whatever is set here (often after 4 to 10 seconds).");
}

/// The profile, at the top of Shortcuts: which (Save As… keeps it under
/// a name of its own), its command key and its keys' ways. `saving` is
/// the name being typed for Save As, while it is.
pub fn profile(ui: &mut Ui, saving: Option<&Child<TextField>>) {
    let k = ui.s.keyboard.clone();
    ui.bbox("Keyboard Profile", |ui| {
        let saved = !Platform::ALL.iter().any(|p| p.name() == k.profile.name);
        match saving {
            None => {
                let choices = k.choices();
                let names: Vec<SharedString> = choices.iter().map(|p| SharedString::from(p.name.clone())).collect();
                let i = choices.iter().position(|p| p.name == k.profile.name).unwrap_or(0);
                let pick = ui.edit(move |s: &mut Settings, i: usize| {
                    let choices = s.keyboard.choices();
                    if let Some(p) = choices.get(i)
                        && p.name != s.keyboard.profile.name
                    {
                        let name = p.name.clone();
                        s.keyboard.pick_profile(&name);
                    }
                });
                let delete = ui.edit_click(|s| {
                    let k = &mut s.keyboard;
                    let name = k.profile.name.clone();
                    k.profiles.retain(|p| p.name != name);
                    // Back to its platform's, as it comes.
                    k.profile = KeyboardProfile::builtin(k.profile.platform);
                });
                let start = if saved && !k.profile.name.ends_with("(Customized)") { k.profile.name.clone() } else { String::new() };
                let save_as = ui.on_click_view(move |v, cx| {
                    if let Some(st) = state_of(v) {
                        let field = crate::form::text_field(cx, |f| f.hint("My keys"));
                        field.update(cx, |f, cx| {
                            f.set_text(start.clone(), cx);
                            f.select_all_text(cx);
                            cx.focus();
                        });
                        st.shortcuts.saving = Some(field);
                    }
                    cx.notify();
                });
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().child(MenuField::new("profile", names, i).label("Profile:").divider(DIVIDER).on_change(pick)))
                        .child(Button::new("Save As…").on_click(save_as))
                        .child(Button::new("Delete").enabled(saved).on_click(delete)),
                );
            }
            Some(field) => {
                let name = field.read(ui.cx).text().trim().to_owned();
                let taken = Platform::ALL.iter().any(|p| p.name() == name || KeyboardProfile::customized_name(*p) == name);
                let ok = !name.is_empty() && !taken;
                let save = ui.on_click_view(move |v, cx| {
                    let Some(st) = state_of(v) else { return };
                    let Some(field) = st.shortcuts.saving.take() else { return };
                    let name = field.read(cx).text().trim().to_owned();
                    crate::prefs().edit_as("", "Save Profile", |s| {
                        s.keyboard.profile.name = name;
                        s.keyboard.save_profile();
                    });
                    cx.notify();
                });
                let cancel = ui.on_click_view(|v, cx| {
                    if let Some(st) = state_of(v) {
                        st.shortcuts.saving = None;
                    }
                    cx.notify();
                });
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().child(TextControl::new(ui.cx, &field).label("Name:").divider(DIVIDER)))
                        .child(Button::new("Save").default(true).enabled(ok).on_click(save))
                        .child(Button::new("Cancel").on_click(cancel)),
                );
            }
        }
        // A change to the profile's keys makes a platform's "… (Customized)".
        let titles: Vec<SharedString> = CommandKey::ALL.iter().map(|c| SharedString::from(c.title())).collect();
        let c = CommandKey::ALL.iter().position(|c| *c == k.profile.command_key).unwrap_or(0);
        ui.menu(
            "command-key",
            titles,
            c,
            |f| f.label("Command key:").divider(DIVIDER),
            |s, i| {
                if s.keyboard.profile.command_key != CommandKey::ALL[i] {
                    s.keyboard.profile.command_key = CommandKey::ALL[i];
                    s.keyboard.profile_changed();
                }
            },
        );
        ui.check("Caps Lock is another Control key", k.profile.caps_lock_control, |s, v| {
            if s.keyboard.profile.caps_lock_control != v {
                s.keyboard.profile.caps_lock_control = v;
                s.keyboard.profile_changed();
            }
        });
        let super_key = if k.profile.platform == Platform::Windows { "the Windows key" } else { "⌘ (the Super key)" };
        ui.check(format!("Tap {super_key} on its own for the command palette"), k.profile.palette_on_tap, |s, v| {
            if s.keyboard.profile.palette_on_tap != v {
                s.keyboard.profile.palette_on_tap = v;
                s.keyboard.profile_changed();
            }
        });
    });
}

/// Where the profile box's fields begin.
const DIVIDER: f32 = 96.0;
