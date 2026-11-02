//! Power: the battery, when an idle machine turns its display off and
//! sleeps, and caffeinated mode (stay awake until turned off; also the cup
//! in the Deskbar). The battery is read on a thread every few seconds.

use std::{sync::Arc, time::Duration};

use olo_protocol::{PowerStatus, Settings};
use olo_sync::Mutex;
use olo_system::battery::{self, Battery, State as Charge};
use olo_ui::{
    gpui::{Context, SharedString},
    widgets::CheckBox,
};

use crate::{Msg, PanelView, form::{Ui, divider_for}, server::Server};

/// What the poller last read: None until it has; Some(None) for no battery.
type Shared = Arc<Mutex<Option<Option<Battery>>>>;

/// The battery, read on a thread of its own while the panel is open.
pub struct State {
    battery: Shared,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        State { battery: poll(olo_ui::proxy(cx)) }
    }
}

fn poll(proxy: olo_ui::Proxy) -> Shared {
    let battery: Shared = Default::default();
    let out = Arc::downgrade(&battery);
    std::thread::Builder::new()
        .name("battery".into())
        .spawn(move || {
            // Ends once the panel (the only strong reference) has closed.
            while let Some(shared) = out.upgrade() {
                let b = battery::battery();
                {
                    let mut slot = shared.lock().unwrap();
                    if slot.as_ref() != Some(&b) {
                        *slot = Some(b);
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
    ui.menu(id, names, i, |f| f.label(label).divider(divider), move |s, i| {
        if let Some(&m) = choices.get(i) {
            set(s, m);
        }
    });
}

pub fn ui(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let battery = st.battery.lock().unwrap().clone();
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
        }
    });

    let p = ui.s.power.clone();
    ui.bbox("When Idle", |ui| {
        const OFF: &str = "Turn the display off after:";
        const SLEEP_AFTER: &str = "Put the computer to sleep after:";
        let divider = divider_for(ui.window, &[OFF, SLEEP_AFTER]);
        minutes_menu(ui, "display-off", OFF, divider, DISPLAY_OFF, p.display_off_minutes, |s, m| s.power.display_off_minutes = m);
        minutes_menu(ui, "sleep", SLEEP_AFTER, divider, SLEEP, p.sleep_minutes, |s, m| s.power.sleep_minutes = m);
        ui.enabled_if(p.sleep_minutes > 0, |ui| ui.check("Only on battery power", p.sleep_on_battery_only, |s, v| s.power.sleep_on_battery_only = v));
        ui.enabled_if(p.display_off_minutes > 0, |ui| {
            ui.check("Lock the screen when the display turns off", p.lock_on_display_off, |s, v| s.power.lock_on_display_off = v)
        });
    });

    ui.bbox("Caffeinated", |ui| {
        let Some(status) = power else {
            ui.note("Waiting for app_server…");
            return;
        };
        let server = server.clone();
        ui.add(CheckBox::new("Stay awake: no display sleep, sleep, screen saver or lock", status.caffeinated).on_toggle(move |on, _, _| server.set_caffeinated(on)));
        let note = match status.app_inhibitors {
            0 => "Also from the Deskbar: click the cup.".to_owned(),
            1 => "An application is keeping the display awake.".to_owned(),
            n => format!("{n} applications are keeping the display awake."),
        };
        ui.note(note);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(percent: u32, state: Charge) -> Battery {
        Battery {
            percent,
            state,
            time_to_empty: None,
            time_to_full: None,
            cycle_count: None,
            health: None,
            charge_limit: None,
            power_watts: None,
        }
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
