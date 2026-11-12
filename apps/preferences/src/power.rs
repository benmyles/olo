//! Power, in three tabs (Ben, 2026-11-06: "centralize the gpu power
//! settings and cpu power settings to all be under the prefs -> power
//! section"):
//!
//! - Power Modes: the power mode (Power Saver, Balanced, Performance,
//!   through the power profiles daemon: `olo_system::power_profile`; also
//!   in Deskbar's battery menu, and found from Performance) and what each
//!   mode, and running on battery, puts the processors and each GPU on
//!   (`gpus::modes`).
//! - Graphics: each GPU, its Custom settings, and what records and plays
//!   video (`gpus::graphics`).
//! - Battery & Idle: the battery, the fans' profile (a ThinkPad's: the
//!   firmware's own curve, or Olo's `olo-fans` running them ahead of the
//!   heat), when an idle machine turns its display off and sleeps,
//!   caffeinated mode (stay awake until turned off; also the cup in the
//!   Deskbar) and presence.
//!
//! The battery and the fans' keeper are read on a thread every few
//! seconds; the modes follow the daemon's signals.

use std::{sync::Arc, time::Duration};

use olo_kit::{
    Cx, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px, style, theme,
    widgets::{CheckBox, MenuField, RadioButton, StringView, TabView},
};
use olo_protocol::{PowerStatus, Settings};
use olo_sync::Mutex;
use olo_system::{
    battery::{self, Battery, State as Charge},
    fans::{self, Profile},
    power_profile::{self, Mode, Modes},
};

use crate::{
    Msg, PanelView,
    form::{Ui, divider_for},
    server::Server,
};

/// What the poller last read: None until it has; Some(None) for no battery.
type Shared = Arc<Mutex<Option<Option<Battery>>>>;

/// The fans' keeper as the poller last read it, and a change of profile
/// under way.
#[derive(Clone, Default)]
struct Fans {
    /// None until read; Some(None): this computer's fans can't be set
    /// (nothing is shown).
    state: Option<Option<fans::State>>,
    /// The profile being put in force (the administrator's password
    /// asked, the steps run): shown as chosen meanwhile.
    applying: Option<Profile>,
    /// How the last change went, until the next.
    said: Option<Result<String, String>>,
}

/// The battery, read on a thread of its own while the panel is open.
pub struct State {
    battery: Shared,
    fans: Arc<Mutex<Fans>>,
    /// The power modes: None while being read, then the daemon's (None:
    /// there is none).
    modes: Modes_,
    /// The battery's charge limit (UPower's), when it has one: read on a
    /// thread as the panel opens and after each change.
    limit: Arc<Mutex<Option<battery::ChargeLimit>>>,
    /// A presence sensor: whether the session can read it (Olo's rule
    /// in); None without one.
    presence: Option<bool>,
    /// The GPUs follow the power mode (its follower installed by Power
    /// Modes' Apply): read with the modes, off the UI thread.
    gpus_follow: Arc<std::sync::atomic::AtomicBool>,
    /// The tab showing (`TABS`).
    pub tab: usize,
    /// The plan and the GPUs (Power Modes and Graphics).
    pub gpus: crate::gpus::State,
}

/// Power's tabs.
pub const TABS: [&str; 3] = ["Power Modes", "Graphics", "Battery & Idle"];

/// Power shown on tab `name` (`--tab`; the old GPUs panel's names: Graphics).
pub fn show_tab(st: &mut State, name: &str) {
    if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(name)) {
        st.tab = i;
    }
}

type Modes_ = Arc<Mutex<Option<Option<Modes>>>>;

impl State {
    pub fn new(_cx: &mut Cx<PanelView>) -> State {
        // The power modes, as the daemon says, kept up to date by its
        // signals (a ThinkPad's Fn+L/M/H too). Tests: made up.
        let modes: Modes_ = Default::default();
        let gpus_follow: Arc<std::sync::atomic::AtomicBool> = Default::default();
        if cfg!(test) {
            *modes.lock().unwrap() = Some(Some(Modes { active: Mode::Balanced, available: Mode::ALL.to_vec(), degraded: None, drivers: vec!["intel_pstate".into(), "platform_profile".into()] }));
        } else {
            let (into, proxy, follow) = (modes.clone(), olo_kit::proxy(), gpus_follow.clone());
            power_profile::watch(move |m| {
                let on = std::path::Path::new("/etc/systemd/system/olo-gpu-follow.service").exists();
                follow.store(on, std::sync::atomic::Ordering::Relaxed);
                *into.lock().unwrap() = Some(m);
                proxy.send(Msg::Redraw);
            });
        }
        let limit: Arc<Mutex<Option<battery::ChargeLimit>>> = Default::default();
        read_limit(&limit, olo_kit::proxy());
        // (Tests: a ThinkPad's, readable.)
        let presence = if cfg!(test) { Some(true) } else { olo_system::presence::sensor().map(|s| olo_system::presence::readable(&s)) };
        // (Tests: a ThinkPad's fans kept cool, at work.)
        let fans: Arc<Mutex<Fans>> = Default::default();
        if cfg!(test) {
            fans.lock().unwrap().state = Some(Some(fans::State { profile: Profile::Cool, installed: true, control: true, held: Some(("4".into(), "19 W".into())) }));
        }
        State { battery: poll(olo_kit::proxy(), fans.clone()), fans, modes, limit, presence, gpus_follow, tab: 0, gpus: crate::gpus::State::new() }
    }
}

/// Put `profile` in force: Olo Doctor does it (`olo-doctor --fans`: its
/// helper, as root through polkit, which asks for the password), on a
/// thread; how it went is said under the menu.
fn set_fans(shared: Arc<Mutex<Fans>>, proxy: olo_kit::Proxy, profile: Profile) {
    {
        let mut f = shared.lock().unwrap();
        if f.applying.is_some() {
            return;
        }
        f.applying = Some(profile);
        f.said = None;
    }
    proxy.send(Msg::Redraw);
    std::thread::Builder::new()
        .name("fans-apply".into())
        .spawn(move || {
            let said = crate::ask_doctor(&["--fans", profile.word()]);
            let now = fans::look(&fans::Places::here());
            let mut f = shared.lock().unwrap();
            f.applying = None;
            f.said = Some(said);
            f.state = Some(now);
            drop(f);
            proxy.send(Msg::Redraw);
        })
        .expect("thread");
}

/// Read the battery's charge limit (UPower's), on a thread.
fn read_limit(into: &Arc<Mutex<Option<battery::ChargeLimit>>>, proxy: olo_kit::Proxy) {
    // Tests (the layout lint): a ThinkPad's.
    if cfg!(test) {
        *into.lock().unwrap() = Some(battery::ChargeLimit { enabled: false, start: 75, end: 80, path: String::new() });
        return;
    }
    let into = into.clone();
    std::thread::spawn(move || {
        let now = battery::charge_limit();
        *into.lock().unwrap() = now;
        proxy.send(Msg::Redraw);
    });
}

fn poll(proxy: olo_kit::Proxy, fans: Arc<Mutex<Fans>>) -> Shared {
    let battery: Shared = Default::default();
    // Tests (the layout lint): a battery with all there is to say, and
    // not this machine's.
    if cfg!(test) {
        *battery.lock().unwrap() = Some(Some(Battery {
            percent: 87,
            state: Charge::Discharging,
            time_to_empty: Some(Duration::from_secs(5 * 3600 + 47 * 60)),
            time_to_full: None,
            cycle_count: Some(1284),
            health: Some(91),
            charge_limit: Some(80),
            power_watts: Some(12.4),
        }));
        return battery;
    }
    let out = Arc::downgrade(&battery);
    std::thread::Builder::new()
        .name("battery".into())
        .spawn(move || {
            // Ends once the panel (the only strong reference) has closed.
            let places = fans::Places::here();
            while let Some(shared) = out.upgrade() {
                let b = battery::battery();
                {
                    let mut slot = shared.lock().unwrap();
                    if slot.as_ref() != Some(&b) {
                        *slot = Some(b);
                        proxy.send(Msg::Redraw);
                    }
                }
                // The fans' keeper with it (a few small files): the
                // level it holds changes as the computer works.
                let now = fans::look(&places);
                {
                    let mut f = fans.lock().unwrap();
                    if f.state.as_ref() != Some(&now) {
                        f.state = Some(now);
                        proxy.send(Msg::Redraw);
                    }
                }
                drop(shared);
                std::thread::sleep(Duration::from_secs(5));
            }
        })
        .expect("thread");
    battery
}

const DISPLAY_OFF: &[u32] = &[1, 2, 5, 10, 15, 30, 60, 0];
const SLEEP: &[u32] = &[5, 10, 15, 30, 60, 120, 180, 0];

fn minutes_text(m: u32) -> String {
    match m {
        0 => "Never".into(),
        1 => "1 minute".into(),
        60 => "1 hour".into(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

pub fn duration_text(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// "87% — 2 h 10 min remaining" and friends.
pub fn summary(b: &Battery) -> String {
    let detail = match b.state {
        Charge::Full => "fully charged".to_owned(),
        Charge::Charging => match b.time_to_full {
            Some(t) => format!("charging, full in {}", duration_text(t)),
            None => "charging".to_owned(),
        },
        Charge::Discharging => match b.time_to_empty {
            Some(t) => format!("{} remaining", duration_text(t)),
            None => "on battery".to_owned(),
        },
        Charge::NotCharging => match b.charge_limit {
            Some(l) if l < 100 && b.percent + 2 >= l => format!("plugged in, held at the {l}% limit"),
            _ => "plugged in, not charging".to_owned(),
        },
        Charge::Unknown => String::new(),
    };
    if detail.is_empty() { format!("{}%", b.percent) } else { format!("{}% — {detail}", b.percent) }
}

/// A pop-up menu of `choices` (minutes; 0 is never) setting `value`.
pub fn minutes_menu(ui: &mut Ui, id: &str, label: &str, divider: f32, choices: &'static [u32], value: u32, set: impl Fn(&mut Settings, u32) + 'static) {
    let mut names: Vec<SharedString> = choices.iter().map(|&m| minutes_text(m).into()).collect();
    let i = match choices.iter().position(|&m| m == value) {
        Some(i) => i,
        None => {
            names.push(minutes_text(value).into());
            names.len() - 1
        }
    };
    let label = SharedString::from(label.to_owned());
    ui.menu(
        id,
        names,
        i,
        |f| f.label(label).divider(divider),
        move |s, i| {
            if let Some(&m) = choices.get(i) {
                set(s, m);
            }
        },
    );
}

/// Power Saver, Balanced or Performance: the system's power profiles
/// daemon makes each what the machine has (the processors' energy
/// preference and governor, the firmware's platform profile).
fn power_mode(ui: &mut Ui, modes: Option<Option<Modes>>, shared: &Modes_, gpus_follow: bool) {
    ui.bbox("Power Mode", |ui| {
        let Some(modes) = modes else {
            ui.note("Reading…");
            return;
        };
        let Some(modes) = modes else {
            ui.note("No power profiles daemon is running (power-profiles-daemon, or tuned-ppd on Fedora): Olo sets the power mode through it.");
            return;
        };
        for mode in Mode::ALL.into_iter().filter(|m| modes.available.contains(m)) {
            let shared = shared.clone();
            let pick = ui.on_view(move |_, _: &(), cx| {
                // (Shown at once; the daemon's signal confirms it.)
                if let Some(Some(m)) = shared.lock().unwrap().as_mut() {
                    m.active = mode;
                }
                std::thread::spawn(move || {
                    if let Err(e) = power_profile::set(mode) {
                        tracing::warn!(?mode, "power mode: {e}");
                    }
                });
                cx.notify();
            });
            ui.add(RadioButton::new(mode.label(), modes.active == mode).id(format!("power-{}", mode.id())).on_select(move |_window, cx| pick(&(), cx)));
            // (Under the label, past the mark.)
            ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new(mode.about()).dim(true).wrap(true)));
        }
        if let Some(why) = modes.degraded_text() {
            ui.note(why);
        }
        let what = if modes.drivers.is_empty() { "the system's power profiles daemon".to_string() } else { format!("the power profiles daemon, through {}", modes.drivers.join(" and ")) };
        ui.note(format!("Set for the whole computer by {what}{}.", if gpus_follow { ", and on every GPU as below" } else { "" }));
    });
}

pub fn ui(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let items = ui.collect(|ui| match st.tab {
        0 => {
            let modes = st.modes.lock().unwrap().clone();
            power_mode(ui, modes, &st.modes, st.gpus_follow.load(std::sync::atomic::Ordering::Relaxed));
            crate::gpus::modes(ui, &mut st.gpus);
        }
        1 => crate::gpus::graphics(ui, &mut st.gpus),
        _ => battery_and_idle(ui, power, server, st),
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let crate::panel::State::Power(st) = &mut v.state {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("power-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

/// Battery & Idle: two columns, so it fits a laptop's screen: the battery,
/// and when idle, caffeinated and presence beside it.
fn battery_and_idle(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let battery = st.battery.lock().unwrap().clone();
    let left = ui.collect(|ui| {
        battery_box(ui, battery, st);
        fans_box(ui, st);
    });
    let presence = st.presence;
    let right = ui.collect(|ui| {
        idle_boxes(ui, power, server);
        presence_box(ui, presence);
    });
    let column = |items: Vec<olo_kit::Element>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
    ui.add(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(column(left)).child(column(right)));
}

fn battery_box(ui: &mut Ui, battery: Option<Option<Battery>>, st: &State) {
    ui.bbox("Battery", |ui| match battery {
        None => ui.note("Reading…"),
        Some(None) => ui.note("This computer has no battery."),
        Some(Some(b)) => {
            ui.label(summary(&b));
            let mut facts = Vec::new();
            if let Some(h) = b.health {
                facts.push(format!("Health: {h}% of original capacity"));
            }
            if let Some(c) = b.cycle_count {
                facts.push(format!("{c} cycles"));
            }
            if !facts.is_empty() {
                ui.note(facts.join(" · "));
            }
            // Charging stopped short, to spare a battery that's mostly
            // plugged in (UPower's threshold: a ThinkPad's).
            if let Some(l) = st.limit.lock().unwrap().clone() {
                let (shared, proxy, it) = (st.limit.clone(), olo_kit::proxy(), l.clone());
                let toggle = move |on: &bool, _: &mut WindowCx| {
                    let (l, shared, proxy, on) = (it.clone(), shared.clone(), proxy.clone(), *on);
                    std::thread::spawn(move || {
                        if let Err(e) = battery::set_charge_limit(&l, on) {
                            tracing::warn!("charge limit: {e}");
                        }
                        read_limit(&shared, proxy);
                    });
                };
                let label = format!("Stop charging at {}% (and start again below {}%)", l.end, l.start);
                ui.add(CheckBox::new(label, l.enabled).on_toggle(toggle));
                ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new("A battery kept full wears faster: for a computer that's mostly plugged in.").dim(true).wrap(true)));
            }
        }
    });
}

/// The fans' profile, where they can be set (a ThinkPad's): the
/// firmware's own curve or Olo's, and what the fans are doing now.
/// Choosing one asks for an administrator's password (the service is
/// the system's).
fn fans_box(ui: &mut Ui, st: &State) {
    let f = st.fans.lock().unwrap().clone();
    let Some(Some(state)) = f.state else { return };
    ui.bbox("Fans", |ui| {
        let shown = f.applying.unwrap_or(state.profile);
        let names: Vec<SharedString> = Profile::ALL.iter().map(|p| p.label().into()).collect();
        let chosen = Profile::ALL.iter().position(|p| *p == shown).unwrap_or(0);
        let (shared, proxy) = (st.fans.clone(), olo_kit::proxy());
        let pick = move |i: &usize, _: &mut WindowCx| {
            if let Some(&profile) = Profile::ALL.get(*i) {
                set_fans(shared.clone(), proxy.clone(), profile);
            }
        };
        ui.add(div().flex().child(MenuField::new("fan-profile", names, chosen).label("Fan profile:").enabled(f.applying.is_none()).on_change(pick)));
        ui.note(shown.about());
        match (&f.applying, &f.said) {
            (Some(_), _) => ui.note("Setting it: an administrator's password is asked…"),
            (None, Some(Err(why))) => ui.add(StringView::new(format!("Not changed. {why}")).wrap(true).color(style::c(theme::palette().failure))),
            _ => ui.note(state.now()),
        }
    });
}

fn idle_boxes(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server) {
    let p = ui.s.power.clone();
    ui.bbox("When Idle", |ui| {
        const OFF: &str = "Turn the display off after:";
        const SLEEP_AFTER: &str = "Put the computer to sleep after:";
        let divider = divider_for(&[OFF, SLEEP_AFTER]);
        minutes_menu(ui, "display-off", OFF, divider, DISPLAY_OFF, p.display_off_minutes, |s, m| s.power.display_off_minutes = m);
        minutes_menu(ui, "sleep", SLEEP_AFTER, divider, SLEEP, p.sleep_minutes, |s, m| s.power.sleep_minutes = m);
        ui.enabled_if(p.sleep_minutes > 0, |ui| ui.check("Only on battery power", p.sleep_on_battery_only, |s, v| s.power.sleep_on_battery_only = v));
        ui.enabled_if(p.display_off_minutes > 0, |ui| ui.check("Lock the screen when the display turns off", p.lock_on_display_off, |s, v| s.power.lock_on_display_off = v));
    });

    ui.bbox("Caffeinated", |ui| {
        let Some(status) = power else {
            ui.note("Waiting for app_server…");
            return;
        };
        let server = server.clone();
        ui.add(CheckBox::new("Stay awake: no display sleep, sleep, screen saver or lock", status.caffeinated).on_toggle(move |on, _| server.set_caffeinated(*on)));
        let note = match status.app_inhibitors {
            0 => "Also from the Deskbar: click the cup.".to_owned(),
            1 => "An application is keeping the display awake.".to_owned(),
            n => format!("{n} applications are keeping the display awake."),
        };
        ui.note(note);
    });
}

/// Seconds after the person walks away before the screen locks.
const AWAY: &[u32] = &[0, 10, 30, 60, 120, 300];

fn away_text(seconds: u32) -> String {
    match seconds {
        0 => "Never".into(),
        s if s < 60 => format!("After {s} seconds"),
        60 => "After a minute".into(),
        s => format!("After {} minutes", s / 60),
    }
}

/// A presence sensor (a ThinkPad's): lock when the person walks away,
/// wake when they come back. Only on a computer that has one.
fn presence_box(ui: &mut Ui, sensor: Option<bool>) {
    let Some(readable) = sensor else { return };
    let p = ui.s.power.clone();
    ui.bbox("Presence", |ui| {
        const LOCK: &str = "Lock when you walk away:";
        let divider = divider_for(&[LOCK]);
        let mut choices: Vec<u32> = AWAY.to_vec();
        if !choices.contains(&p.lock_when_away_seconds) {
            choices.push(p.lock_when_away_seconds);
        }
        let names: Vec<SharedString> = choices.iter().map(|&s| away_text(s).into()).collect();
        let selected = choices.iter().position(|&s| s == p.lock_when_away_seconds).unwrap_or(0);
        ui.menu("lock-away", names, selected, |f| f.label(SharedString::from(LOCK)).divider(divider), move |s, i| s.power.lock_when_away_seconds = choices.get(i).copied().unwrap_or(0));
        ui.check("Wake the display when you come back", p.wake_on_return, |s, v| s.power.wake_on_return = v);
        ui.check("Dim the display while you look away", p.dim_when_looking_away, |s, v| s.power.dim_when_looking_away = v);
        ui.note(if readable { "This computer's sensor sees when someone is in front of it." } else { "Olo can't read the sensor yet: Olo Doctor installs what it needs." });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(percent: u32, state: Charge) -> Battery {
        Battery { percent, state, time_to_empty: None, time_to_full: None, cycle_count: None, health: None, charge_limit: None, power_watts: None }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(&b(100, Charge::Full)), "100% — fully charged");
        let mut d = b(57, Charge::Discharging);
        d.time_to_empty = Some(Duration::from_secs(2 * 3600 + 600));
        assert_eq!(summary(&d), "57% — 2 h 10 min remaining");
        let mut l = b(80, Charge::NotCharging);
        l.charge_limit = Some(80);
        assert_eq!(summary(&l), "80% — plugged in, held at the 80% limit");
        assert_eq!(minutes_text(120), "2 hours");
        assert_eq!(minutes_text(0), "Never");
    }
}
