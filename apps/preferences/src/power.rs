//! Power: the battery, when an idle machine turns its display off and
//! sleeps, and caffeinated mode (stay awake until turned off; also the cup
//! in the Deskbar). The battery is read on a thread every few seconds.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tabula_kit::{
    egui::Ui,
    widgets::{BBox, CheckBox, MenuField, StringView},
};
use tabula_protocol::{PowerStatus, Settings};
use tabula_system::battery::{self, Battery, State as Charge};

use crate::server::Server;

/// What the poller last read: None until it has; Some(None) for no battery.
type Shared = Arc<Mutex<Option<Option<Battery>>>>;

/// The battery poller starts with the panel's first frame (it needs the
/// context to wake the window) and stops when the panel closes.
#[derive(Default)]
pub struct State {
    battery: Option<Shared>,
}

fn poll(ctx: &tabula_kit::egui::Context) -> Shared {
    let battery: Shared = Default::default();
    let (out, ctx) = (Arc::downgrade(&battery), ctx.clone());
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
                        ctx.request_repaint();
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

pub fn minutes_menu(ui: &mut Ui, id: &str, label: &str, choices: &[u32], value: &mut u32) {
    let names: Vec<String> = choices.iter().map(|&m| minutes_text(m)).collect();
    let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
    let custom = minutes_text(*value);
    let mut i = match choices.iter().position(|m| m == value) {
        Some(i) => i,
        None => {
            names.push(&custom);
            names.len() - 1
        }
    };
    ui.add(MenuField::new(id, &mut i, &names).label(label).divider(170.0));
    if let Some(&m) = choices.get(i) {
        *value = m;
    }
}

pub fn ui(ui: &mut Ui, s: &mut Settings, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let battery = st.battery.get_or_insert_with(|| poll(ui.ctx())).lock().unwrap().clone();
    BBox::new("Battery").show(ui, |ui| match battery {
        None => {
            ui.add(StringView::new("Reading…").dim(true));
        }
        Some(None) => {
            ui.add(StringView::new("This computer has no battery.").dim(true));
        }
        Some(Some(b)) => {
            ui.add(StringView::new(&summary(&b)));
            let mut facts = Vec::new();
            if let Some(h) = b.health {
                facts.push(format!("Health: {h}% of original capacity"));
            }
            if let Some(c) = b.cycle_count {
                facts.push(format!("{c} cycles"));
            }
            if !facts.is_empty() {
                ui.add(StringView::new(&facts.join(" · ")).dim(true));
            }
        }
    });

    let p = &mut s.power;
    BBox::new("When Idle").show(ui, |ui| {
        minutes_menu(ui, "display-off", "Turn the display off after:", DISPLAY_OFF, &mut p.display_off_minutes);
        minutes_menu(ui, "sleep", "Put the computer to sleep after:", SLEEP, &mut p.sleep_minutes);
        ui.add_enabled(p.sleep_minutes > 0, CheckBox::new(&mut p.sleep_on_battery_only, "Only on battery power"));
        ui.add_enabled(
            p.display_off_minutes > 0,
            CheckBox::new(&mut p.lock_on_display_off, "Lock the screen when the display turns off"),
        );
    });

    BBox::new("Caffeinated").show(ui, |ui| {
        let Some(status) = power else {
            ui.add(StringView::new("Waiting for app_server…").dim(true));
            return;
        };
        let mut on = status.caffeinated;
        ui.add(CheckBox::new(&mut on, "Stay awake: no display sleep, sleep, screen saver or lock"));
        if on != status.caffeinated {
            server.set_caffeinated(on);
        }
        let note = match status.app_inhibitors {
            0 => "Also from the Deskbar: click the cup.".to_owned(),
            1 => "An application is keeping the display awake.".to_owned(),
            n => format!("{n} applications are keeping the display awake."),
        };
        ui.add(StringView::new(&note).dim(true));
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
