//! The built-in power button (Preferences ▸ Keyboard,
//! `olo_protocol::power_button`): pressed and let go before the first
//! hold's seconds, it does its press's action; held, each hold's action
//! as the hold reaches its seconds, while still down (so a three-second
//! hold can ask to restart and, held on, ten seconds shut down at once).
//! A hold reached, even one set to Nothing, makes the let go no press.
//!
//! logind's own handling of the key (it shut down on a press) is taken
//! from it in the session (power.rs's `handle-power-key` lock). Only the
//! computer's own button counts (not a USB or Bluetooth keyboard's power
//! key: by the device's sysfs path); their key goes nowhere. A headless
//! app_server (tests, agents' screens) takes a synthetic power key, for
//! tests, but never restarts or shuts the computer down: it says what it
//! would have done (`Event::PowerButton { done: false }`). Its "Asking
//! First" is Deskbar's Restart or Shut Down alert, sent as that event;
//! not while the screen is locked (nothing could be seen to answer).

use std::{
    path::Path,
    time::{Duration, Instant},
};

use olo_protocol::{
    Event, EventKind,
    power_button::{Hold, PowerAction},
};
use smithay::reexports::calloop::{
    RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// evdev's KEY_POWER.
pub const KEY_POWER: u32 = 116;

/// The button, while it's down.
#[derive(Debug, Default)]
pub struct Held {
    since: Option<Instant>,
    /// Holds reached so far (in their order).
    reached: usize,
    timer: Option<RegistrationToken>,
}

/// The holds `held_for` has reached past the `reached` already done (in
/// order), how many that makes, and how long until the next.
pub fn due(holds: &[Hold], held_for: Duration, reached: usize) -> (Vec<PowerAction>, usize, Option<Duration>) {
    let mut actions = Vec::new();
    let mut n = reached;
    while let Some(h) = holds.get(n) {
        if Duration::from_secs(u64::from(h.seconds)) > held_for {
            break;
        }
        actions.push(h.action);
        n += 1;
    }
    let next = holds.get(n).map(|h| Duration::from_secs(u64::from(h.seconds)).saturating_sub(held_for));
    (actions, n, next)
}

/// Whether the device at `syspath` is the computer's own (the ACPI power
/// button, Intel's HID events, the laptop's keyboard), not on USB or
/// Bluetooth.
pub fn built_in(syspath: Option<&Path>) -> bool {
    let Some(p) = syspath.and_then(|p| p.to_str()) else { return false };
    !(p.contains("/usb") || p.contains("uhid") || p.contains("bluetooth"))
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    /// The built-in power button went down or up.
    pub fn power_button(&mut self, pressed: bool) {
        if pressed {
            // (Down already: a second down without an up.)
            if self.power_held.since.is_some() {
                return;
            }
            self.power_held = Held { since: Some(Instant::now()), reached: 0, timer: None };
            let holds = self.settings.keyboard.power_button.holds_in_order();
            if let Some(first) = holds.first() {
                let token = self.handle.insert_source(Timer::from_duration(Duration::from_secs(u64::from(first.seconds))), |_, _, state| match state.power_hold_tick() {
                    Some(next) => TimeoutAction::ToDuration(next),
                    None => {
                        state.power_held.timer = None;
                        TimeoutAction::Drop
                    }
                });
                self.power_held.timer = token.ok();
            }
            return;
        }
        if self.power_held.since.take().is_none() {
            return;
        }
        if let Some(t) = self.power_held.timer.take() {
            self.handle.remove(t);
        }
        if std::mem::take(&mut self.power_held.reached) == 0 {
            let press = self.settings.keyboard.power_button.press;
            self.power_action(press, "pressed");
        }
    }

    /// The holds reached since the last look: done. How long until the
    /// next, if there is one.
    fn power_hold_tick(&mut self) -> Option<Duration> {
        let since = self.power_held.since?;
        let holds = self.settings.keyboard.power_button.holds_in_order();
        let (actions, reached, next) = due(&holds, since.elapsed(), self.power_held.reached);
        self.power_held.reached = reached;
        for a in actions {
            self.power_action(a, "held");
        }
        next
    }

    /// Do what the button is set to.
    fn power_action(&mut self, action: PowerAction, how: &str) {
        info!(action = action.label(), how, "power button");
        let done = match action {
            PowerAction::Nothing => true,
            PowerAction::Lock => {
                self.lock_screen();
                true
            }
            PowerAction::LogOut => {
                self.running.store(false, std::sync::atomic::Ordering::SeqCst);
                true
            }
            PowerAction::RestartAsk | PowerAction::ShutDownAsk => {
                if self.lock.locked() {
                    info!("power button: the screen is locked, so nothing asked");
                    false
                } else if !self.ipc.wanted(EventKind::PowerButton) {
                    warn!("power button: nothing to ask with (Deskbar isn't listening)");
                    false
                } else {
                    true
                }
            }
            PowerAction::Restart | PowerAction::RestartNow | PowerAction::ShutDown | PowerAction::ShutDownNow => {
                if !BackendData::IS_SESSION {
                    info!("power button: {} (not the session: not done)", action.label());
                    false
                } else {
                    // (logind, and polkit: not on the loop.)
                    std::thread::spawn(move || {
                        let r = match action {
                            PowerAction::Restart => olo_system::power::reboot(),
                            PowerAction::RestartNow => olo_system::power::reboot_now(),
                            PowerAction::ShutDown => olo_system::power::power_off(),
                            _ => olo_system::power::power_off_now(),
                        };
                        if let Err(e) = r {
                            warn!("power button: {}: {e}", action.label());
                        }
                    });
                    true
                }
            }
        };
        self.ipc.broadcast(Event::PowerButton { action, done });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds() -> Vec<Hold> {
        olo_protocol::power_button::PowerButton::default().holds_in_order()
    }

    #[test]
    fn holds_come_in_order() {
        let h = holds();
        let s = Duration::from_secs;
        assert_eq!(due(&h, Duration::from_millis(2900), 0), (vec![], 0, Some(Duration::from_millis(100))));
        assert_eq!(due(&h, s(3), 0), (vec![PowerAction::RestartAsk], 1, Some(s(7))));
        assert_eq!(due(&h, s(10), 1), (vec![PowerAction::ShutDownNow], 2, None));
        // A late look (the loop busy): both, in order.
        assert_eq!(due(&h, s(11), 0), (vec![PowerAction::RestartAsk, PowerAction::ShutDownNow], 2, None));
        assert_eq!(due(&[], s(5), 0), (vec![], 0, None));
    }

    #[test]
    fn only_the_computers_own() {
        assert!(built_in(Some(Path::new("/sys/devices/platform/PNP0C0C:00/input/input2/event2"))));
        assert!(built_in(Some(Path::new("/sys/devices/platform/INTC10CC:00/input/input32/event12"))));
        assert!(built_in(Some(Path::new("/sys/devices/platform/i8042/serio0/input/input3/event3"))));
        assert!(!built_in(Some(Path::new("/sys/devices/pci0000:00/0000:00:1c.0/usb7/7-1/7-1.1/7-1.1:1.1/0003:05AC:029F.0028/input/input64/event22"))));
        assert!(!built_in(Some(Path::new("/sys/devices/virtual/misc/uhid/0005:05AC:0267.0001/input/input70/event23"))));
        assert!(!built_in(None));
    }
}
