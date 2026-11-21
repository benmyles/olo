//! Which makers' EGL libraries this program loads.
//!
//! glvnd, the library behind `libEGL`, asks every maker's library in
//! turn for a display, NVIDIA's first. On a computer whose NVIDIA card
//! only sits beside the GPU everything is drawn on, that loaded
//! NVIDIA's whole EGL stack into app_server as it started, the
//! person's and each agent's screen's: NVIDIA's code run against a card
//! that should be left asleep, megabytes of it kept in each, and, on
//! the day that card had stopped answering, a library talking to a
//! driver that no longer could (2026-11-20). There app_server gives
//! glvnd Mesa's library alone (`olo_kit_base::gpu::mesa_egl_alone`
//! decides; never where the person chose, nor where a display could be
//! the card's).
//!
//! glvnd reads its choice from the environment, so it is set there,
//! once, first thing in `main`, and marked as app_server's own: what
//! app_server starts doesn't inherit it (`not_inherited`: a program on
//! NVIDIA's card on purpose needs NVIDIA's library), and an in-place
//! restart, which keeps the environment, decides afresh.

use std::sync::atomic::{AtomicBool, Ordering};

const VENDORS: &str = "__EGL_VENDOR_LIBRARY_FILENAMES";
/// Set beside `VENDORS` where it is app_server's own doing.
const OURS: &str = "OLO_EGL_VENDORS_OURS";
static CHOSEN: AtomicBool = AtomicBool::new(false);

/// Decides, and sets this program's environment; `headless`: it draws
/// and drives no display. What it gave glvnd, if anything.
///
/// # Safety
/// The environment is written: no other thread may exist yet.
pub unsafe fn choose(headless: bool) -> Option<String> {
    // SAFETY: the caller's word, no other thread.
    unsafe {
        // (From before an in-place restart: decided again.)
        if std::env::var_os(OURS).is_some() {
            std::env::remove_var(VENDORS);
            std::env::remove_var(OURS);
        }
        let mesa = olo_kit_base::gpu::mesa_egl_alone(!headless)?;
        std::env::set_var(VENDORS, &mesa);
        std::env::set_var(OURS, "1");
        CHOSEN.store(true, Ordering::Relaxed);
        Some(mesa)
    }
}

/// `cmd`'s program starts without what app_server chose for itself.
pub fn not_inherited(cmd: &mut std::process::Command) {
    if CHOSEN.load(Ordering::Relaxed) {
        cmd.env_remove(VENDORS).env_remove(OURS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_app_server_chose_for_itself_isnt_handed_on() {
        let removed = |cmd: &std::process::Command, name: &str| cmd.get_envs().any(|(k, v)| k == name && v.is_none());
        let mut cmd = std::process::Command::new("true");
        not_inherited(&mut cmd);
        assert!(!removed(&cmd, VENDORS), "nothing chosen: the environment as it is");
        CHOSEN.store(true, Ordering::Relaxed);
        not_inherited(&mut cmd);
        assert!(removed(&cmd, VENDORS) && removed(&cmd, OURS));
        CHOSEN.store(false, Ordering::Relaxed);
    }
}
