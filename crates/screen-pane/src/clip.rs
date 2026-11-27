//! While the person controls a local screen, its clipboard and theirs are one:
//! what they copy on the screen is on their own clipboard, and what they
//! copied on their display pastes on the screen (text, pictures, files:
//! every form it was copied in), both ways, for as long as they're in
//! control. When they hand it back, what of theirs is on the screen's
//! clipboard is taken off it (an agent's screen never has the person's
//! clipboard on its own); what they copied from the screen stays on
//! theirs. Each side's clipboard says when it changes (`clipboard::watch`):
//! nothing is asked for meanwhile.

use std::{
    hash::{Hash, Hasher},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_kit::held_clipboard::{self as clipboard, Clip, Held, Watch};

#[derive(Default)]
struct Shared {
    /// The person's clipboard, held on the screen's.
    on_screen: Option<Held>,
    /// What was last put on each side from the other, so its copy there
    /// isn't sent straight back.
    to_screen: Option<u64>,
    to_person: Option<u64>,
}

pub(crate) struct Bridge {
    _person: Watch,
    _screen: Watch,
    shared: Arc<Mutex<Shared>>,
}

fn digest(clip: &Clip) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (mime, data) in clip {
        (mime, &**data).hash(&mut h);
    }
    h.finish()
}

impl Bridge {
    /// Screen display `screen`'s clipboard joined to the person's (the
    /// display this runs on), honouring `cancelled` before installing watches
    /// and before forwarding clipboard contents.
    fn start(screen: &str, cancelled: &Arc<AtomicBool>) -> Result<Bridge, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let shared = Arc::<Mutex<Shared>>::default();
        let (to, on, stop_to) = (shared.clone(), screen.to_owned(), cancelled.clone());
        // The person's, now and as it changes: onto the screen.
        let person = clipboard::watch(None, true, move |clip| {
            if stop_to.load(Ordering::Acquire) {
                return;
            }
            let d = digest(&clip);
            {
                let Ok(s) = to.lock() else { return };
                if stop_to.load(Ordering::Acquire) || s.to_person == Some(d) {
                    return;
                }
            }
            // Perform synchronous Wayland round-trips OUTSIDE the `Shared`
            // mutex so dropping the bridge never blocks on a stalled screen.
            match clipboard::hold_on(Some(&on), clip) {
                Ok(held) => {
                    if stop_to.load(Ordering::Acquire) {
                        drop(held);
                        return;
                    }
                    let old = {
                        let Ok(mut s) = to.lock() else {
                            drop(held);
                            return;
                        };
                        if stop_to.load(Ordering::Acquire) {
                            drop(s);
                            drop(held);
                            return;
                        }
                        s.to_screen = Some(d);
                        s.on_screen.replace(held)
                    };
                    drop(old);
                }
                Err(e) => tracing::warn!("couldn't put the clipboard on screen {on}: {e}"),
            }
        })?;
        if cancelled.load(Ordering::Acquire) {
            drop(person);
            let old = shared.lock().ok().and_then(|mut s| s.on_screen.take());
            drop(old);
            return Err("cancelled".into());
        }
        let (back, stop_back) = (shared.clone(), cancelled.clone());
        // The screen's, as it changes (not what an agent copied before):
        // onto the person's.
        let screen = clipboard::watch(Some(screen), false, move |clip| {
            if stop_back.load(Ordering::Acquire) {
                return;
            }
            let d = digest(&clip);
            {
                let Ok(s) = back.lock() else { return };
                if stop_back.load(Ordering::Acquire) || s.to_screen == Some(d) {
                    return;
                }
            }
            match clipboard::hold_on(None, clip) {
                Ok(held) => {
                    // Recheck cancellation AFTER Wayland round-trips and BEFORE
                    // promoting `held` onto the person's persistent clipboard.
                    if stop_back.load(Ordering::Acquire) {
                        drop(held);
                        return;
                    }
                    held.keep();
                    if let Ok(mut s) = back.lock() {
                        s.to_person = Some(d);
                    }
                }
                Err(e) => tracing::warn!("couldn't put the screen's clipboard on yours: {e}"),
            }
        })?;
        Ok(Bridge { _person: person, _screen: screen, shared })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // What of the person's is on the screen goes (dropped outside `Shared`).
        let old = self.shared.lock().ok().and_then(|mut s| s.on_screen.take());
        drop(old);
    }
}

/// The clipboards joined while this is kept (made off the UI thread:
/// two displays' connections), parted asynchronously when dropped so stopping
/// control never blocks the window thread on a stalled screen.
pub(crate) struct Joined<T: Send + 'static = Bridge> {
    cancelled: Arc<AtomicBool>,
    slot: Arc<Mutex<(bool, Option<T>)>>,
}

fn join_with<T: Send + 'static>(starter: impl FnOnce(&Arc<AtomicBool>) -> Option<T> + Send + 'static) -> Joined<T> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let slot = Arc::new(Mutex::new((false, None)));
    let (c, s) = (cancelled.clone(), slot.clone());
    std::thread::spawn(move || {
        if c.load(Ordering::Acquire) {
            return;
        }
        if let Some(bridge) = starter(&c) {
            let keep = {
                if let Ok(mut guard) = s.lock()
                    && !guard.0
                    && !c.load(Ordering::Acquire)
                {
                    guard.1 = Some(bridge);
                    None
                } else {
                    Some(bridge)
                }
            };
            drop(keep);
        }
    });
    Joined { cancelled, slot }
}

/// Join the controlled screen session's exact Wayland `wayland_display`
/// clipboard to the person's while the returned handle is kept.
pub(crate) fn join_display(wayland_display: String) -> Joined {
    join_with(move |cancelled| match Bridge::start(&wayland_display, cancelled) {
        Ok(bridge) => Some(bridge),
        Err(e) if e == "cancelled" => None,
        Err(e) => {
            tracing::warn!("couldn't join screen {wayland_display}'s clipboard to yours: {e}");
            None
        }
    })
}

impl<T: Send + 'static> Drop for Joined<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // Take the bridge out under a short lock and dispose of it on a worker
        // thread so Wayland teardown never blocks the window thread.
        let bridge = self.slot.lock().ok().and_then(|mut s| {
            s.0 = true;
            s.1.take()
        });
        if let Some(bridge) = bridge {
            let _ = std::thread::Builder::new().name("clip-drop".into()).spawn(move || drop(bridge));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astra_f7_dropped_join_cancels_before_starting_bridge() {
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let gate_w = gate.clone();
        let started = Arc::new(AtomicBool::new(false));
        let started_w = started.clone();
        let done = Arc::new(AtomicBool::new(false));
        let done_w = done.clone();

        let joined = join_with(move |cancelled| {
            let mut open = gate_w.0.lock().unwrap();
            while !*open {
                open = gate_w.1.wait(open).unwrap();
            }
            if cancelled.load(Ordering::Acquire) {
                done_w.store(true, Ordering::Release);
                return None;
            }
            started_w.store(true, Ordering::Release);
            done_w.store(true, Ordering::Release);
            Some(())
        });

        // Drop `Joined` before the worker gets past the scheduling delay.
        drop(joined);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !done.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!started.load(Ordering::Acquire), "clipboard bridge must never start after Joined is dropped");
    }

    #[test]
    fn review2_f30_joined_drop_does_not_block_ui_on_stalled_bridge() {
        struct SlowBridge {
            dropped: Arc<AtomicBool>,
        }
        impl Drop for SlowBridge {
            fn drop(&mut self) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                self.dropped.store(true, Ordering::Release);
            }
        }
        let installed = Arc::new(AtomicBool::new(false));
        let inst_w = installed.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let drop_w = dropped.clone();

        let joined = join_with(move |_| {
            inst_w.store(true, Ordering::Release);
            Some(SlowBridge { dropped: drop_w })
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while (!installed.load(Ordering::Acquire) || joined.slot.lock().unwrap().1.is_none()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Dropping `Joined` must return promptly (< 100 ms) even though
        // `SlowBridge::drop` blocks for 250 ms.
        let t0 = std::time::Instant::now();
        drop(joined);
        assert!(t0.elapsed() < std::time::Duration::from_millis(100), "Joined::drop blocked UI thread for {:?}", t0.elapsed());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !dropped.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(dropped.load(Ordering::Acquire), "bridge must still be dropped cleanly on worker");
    }
}
