//! The built-in power button (Preferences ▸ Keyboard): what a press does,
//! and what holding it for a number of seconds does. app_server takes the
//! key from logind (its `handle-power-key` lock) and does it; a hold's
//! "asking first" is Deskbar's Restart or Shut Down alert
//! ([`crate::Event::PowerButton`]).

use serde::{Deserialize, Serialize};

/// What the power button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerAction {
    Nothing,
    Lock,
    LogOut,
    /// Restart as Deskbar's menu does it (every program asked to end,
    /// services stopped in order), once its alert is answered.
    RestartAsk,
    /// The same, without asking.
    Restart,
    /// At once: programs ended, the disks written out, restarted, no
    /// orderly stop (Olo's helper, `olo-power-now@`; without it, as
    /// [`PowerAction::Restart`]).
    RestartNow,
    ShutDownAsk,
    ShutDown,
    ShutDownNow,
}

impl PowerAction {
    pub const ALL: [PowerAction; 9] = [
        PowerAction::Nothing,
        PowerAction::Lock,
        PowerAction::LogOut,
        PowerAction::RestartAsk,
        PowerAction::Restart,
        PowerAction::RestartNow,
        PowerAction::ShutDownAsk,
        PowerAction::ShutDown,
        PowerAction::ShutDownNow,
    ];

    /// As Preferences lists it.
    pub fn label(self) -> &'static str {
        match self {
            PowerAction::Nothing => "Nothing",
            PowerAction::Lock => "Lock the Screen",
            PowerAction::LogOut => "Log Out",
            PowerAction::RestartAsk => "Restart, Asking First",
            PowerAction::Restart => "Restart",
            PowerAction::RestartNow => "Restart Immediately",
            PowerAction::ShutDownAsk => "Shut Down, Asking First",
            PowerAction::ShutDown => "Shut Down",
            PowerAction::ShutDownNow => "Shut Down Immediately",
        }
    }

    /// Done at once, without the orderly stop: Olo's helper does it.
    pub fn immediate(self) -> bool {
        matches!(self, PowerAction::RestartNow | PowerAction::ShutDownNow)
    }
}

/// Holding the button this long does `action` (when the hold reaches it,
/// while still held).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub seconds: u32,
    pub action: PowerAction,
}

/// What the built-in power button does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerButton {
    /// Pressed and let go before the first hold's time.
    pub press: PowerAction,
    /// Holds, each done as the hold reaches its seconds (a later one
    /// still, if the button stays down).
    pub holds: Vec<Hold>,
}

impl Default for PowerButton {
    /// Ben, 2026-11-08: a press does nothing; three seconds restart,
    /// asking first; ten shut down at once.
    fn default() -> Self {
        PowerButton { press: PowerAction::Nothing, holds: vec![Hold { seconds: 3, action: PowerAction::RestartAsk }, Hold { seconds: 10, action: PowerAction::ShutDownNow }] }
    }
}

impl PowerButton {
    /// The holds by seconds (the order they come in while held).
    pub fn holds_in_order(&self) -> Vec<Hold> {
        let mut h = self.holds.clone();
        h.sort_by_key(|h| h.seconds);
        h
    }

    /// Whether any of it is done immediately (Olo's helper wanted).
    pub fn wants_helper(&self) -> bool {
        self.press.immediate() || self.holds.iter().any(|h| h.action.immediate())
    }
}

/// The seconds a hold can be set to (Preferences' menu).
pub const HOLD_SECONDS: [u32; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 15];

/// Olo's helper for "immediately": a system unit the person at the
/// computer may start without a password (polkit rule), which restarts or
/// shuts down with `systemctl --force` (`%i`: reboot or poweroff).
pub const HELPER_UNIT_PATH: &str = "/etc/systemd/system/olo-power-now@.service";
pub const HELPER_RULES_PATH: &str = "/etc/polkit-1/rules.d/49-olo-power-now.rules";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bens_defaults() {
        let p = PowerButton::default();
        assert_eq!(p.press, PowerAction::Nothing);
        assert_eq!(p.holds_in_order(), [Hold { seconds: 3, action: PowerAction::RestartAsk }, Hold { seconds: 10, action: PowerAction::ShutDownNow }]);
        assert!(p.wants_helper());
    }

    #[test]
    fn kept_as_words() {
        let p = PowerButton::default();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#"{"press":"nothing","holds":[{"seconds":3,"action":"restart-ask"},{"seconds":10,"action":"shut-down-now"}]}"#);
        assert_eq!(serde_json::from_str::<PowerButton>(&json).unwrap(), p);
        // (Settings from before it: the defaults.)
        assert_eq!(serde_json::from_str::<PowerButton>("{}").unwrap(), p);
        assert_eq!(PowerAction::ALL.len(), 9);
        assert!(PowerAction::ALL.iter().all(|a| !a.label().is_empty()));
    }
}
