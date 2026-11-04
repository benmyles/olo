//! What app_server hands the app_server after it, restarting in place
//! (an upgrade): the state a person set that no settings file holds and
//! that they'd notice gone. Caffeinated (and until when, and with the lid
//! closed), and approving without asking (until when), in
//! `$XDG_RUNTIME_DIR/olo-handover-$WAYLAND_DISPLAY.json`; the password
//! approving goes with, in a file of its own with no permissions but the
//! person's, both written as the old app_server exits and removed as the
//! new one reads them (the runtime directory is memory). Windows' places
//! go the same way (`layout`). Ben, 2026-11-01: "when upgrades happen it
//! keeps the keychain unlocked if it already was and remembers my
//! caffeine/stay awake and approve without asking settings".

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Handover {
    caffeinated: bool,
    caffeinated_until: Option<u64>,
    caffeinated_lid: bool,
    approve_until: Option<u64>,
    /// In Olo Rescue (it goes on): the workspace each display showed
    /// before it, by the display's key.
    #[serde(default)]
    rescue: Option<std::collections::HashMap<String, u32>>,
}

fn paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    let dir = std::path::Path::new(&dir);
    Some((dir.join(format!("olo-handover-{display}.json")), dir.join(format!("olo-handover-{display}.secret"))))
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Written as this app_server exits (with the window layout).
    pub fn save_handover(&self) {
        let Some((state_path, secret_path)) = paths() else { return };
        let (caffeinated, caffeinated_until, caffeinated_lid) = self.power.caffeine_handover();
        let approve = self.auto_approve_handover();
        let h = Handover {
            caffeinated,
            caffeinated_until,
            caffeinated_lid,
            approve_until: approve.as_ref().and_then(|(t, _)| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            rescue: self.rescue.as_ref().map(|r| r.before.clone()),
        };
        if !h.caffeinated && h.approve_until.is_none() && h.rescue.is_none() {
            let _ = std::fs::remove_file(&state_path);
            let _ = std::fs::remove_file(&secret_path);
            return;
        }
        if let Ok(text) = serde_json::to_string(&h) {
            let _ = std::fs::write(&state_path, text);
        }
        if let Some((_, secret)) = approve {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            match std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&secret_path) {
                Ok(mut f) => {
                    let _ = f.write_all(secret.as_bytes());
                }
                Err(e) => warn!("the approving password can't be handed over: {e}"),
            }
        }
    }

    /// Read as this app_server starts, and taken on where still due.
    pub fn restore_handover(&mut self) {
        // Started in Rescue (`OLO_RESCUE`): once the displays are up.
        if crate::rescue::starts_in_rescue() {
            info!("starting in Olo Rescue");
            self.enter_rescue_soon(None);
        }
        let Some((state_path, secret_path)) = paths() else { return };
        let text = std::fs::read_to_string(&state_path);
        let secret = std::fs::read_to_string(&secret_path);
        let _ = std::fs::remove_file(&state_path);
        let _ = std::fs::remove_file(&secret_path);
        let Ok(text) = text else { return };
        let Ok(h) = serde_json::from_str::<Handover>(&text) else { return };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if h.caffeinated {
            // A timed stretch goes on for what is left of it (whole minutes
            // up); one past its time is over.
            match h.caffeinated_until {
                Some(until) if until <= now => {}
                Some(until) => self.set_caffeinated(true, Some(((until - now).div_ceil(60)).max(1) as u32), h.caffeinated_lid),
                None => self.set_caffeinated(true, None, h.caffeinated_lid),
            }
            info!(lid = h.caffeinated_lid, "caffeinated, as before the restart");
        }
        if let (Some(until), Ok(secret)) = (h.approve_until, secret)
            && until > now
            && !secret.is_empty()
        {
            self.restore_auto_approve(UNIX_EPOCH + Duration::from_secs(until), secret);
        }
        if let Some(before) = h.rescue {
            info!("in Olo Rescue, as before the restart");
            self.enter_rescue_soon(Some(before));
        }
    }

    /// Into Rescue once the loop runs (the displays set up by then), the
    /// displays' workspaces from before it as they were (`before`).
    fn enter_rescue_soon(&mut self, before: Option<std::collections::HashMap<String, u32>>) {
        self.handle.insert_idle(move |state| state.enter_rescue_from(before));
    }
}
