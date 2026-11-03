//! While the person controls a screen, its clipboard and theirs are one:
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
    sync::{Arc, Mutex},
};

use olo_ui::clipboard::{self, Clip, Held, Watch};

#[derive(Default)]
struct Shared {
    /// The person's clipboard, held on the screen's.
    on_screen: Option<Held>,
    /// What was last put on each side from the other, so its copy there
    /// isn't sent straight back.
    to_screen: Option<u64>,
    to_person: Option<u64>,
}

struct Bridge {
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
    /// display this runs on).
    fn start(screen: &str) -> Result<Bridge, String> {
        let shared = Arc::<Mutex<Shared>>::default();
        let (to, on) = (shared.clone(), screen.to_owned());
        // The person's, now and as it changes: onto the screen.
        let person = clipboard::watch(None, true, move |clip| {
            let d = digest(&clip);
            let Ok(mut s) = to.lock() else { return };
            if s.to_person == Some(d) {
                return;
            }
            match clipboard::hold_on(Some(&on), clip) {
                Ok(held) => (s.on_screen, s.to_screen) = (Some(held), Some(d)),
                Err(e) => tracing::warn!("couldn't put the clipboard on screen {on}: {e}"),
            }
        })?;
        let back = shared.clone();
        // The screen's, as it changes (not what an agent copied before):
        // onto the person's.
        let screen = clipboard::watch(Some(screen), false, move |clip| {
            let d = digest(&clip);
            let Ok(mut s) = back.lock() else { return };
            if s.to_screen == Some(d) {
                return;
            }
            match clipboard::hold_on(None, clip) {
                Ok(held) => {
                    held.keep();
                    s.to_person = Some(d);
                }
                Err(e) => tracing::warn!("couldn't put the screen's clipboard on yours: {e}"),
            }
        })?;
        Ok(Bridge { _person: person, _screen: screen, shared })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // What of the person's is on the screen goes.
        if let Ok(mut s) = self.shared.lock() {
            s.on_screen.take();
        }
    }
}

/// The clipboards joined while this is kept (made off the UI thread:
/// two displays' connections), parted when it's dropped.
pub struct Joined {
    slot: Arc<Mutex<(bool, Option<Bridge>)>>,
}

/// Join screen display `screen`'s clipboard to the person's.
pub fn join(screen: String) -> Joined {
    let slot = Arc::new(Mutex::new((false, None)));
    let s = slot.clone();
    std::thread::spawn(move || match Bridge::start(&screen) {
        Ok(bridge) => {
            if let Ok(mut s) = s.lock()
                && !s.0
            {
                s.1 = Some(bridge);
            }
        }
        Err(e) => tracing::warn!("couldn't join screen {screen}'s clipboard to yours: {e}"),
    });
    Joined { slot }
}

impl Drop for Joined {
    fn drop(&mut self) {
        // (Under the lock: a bridge still being made is let go of too.)
        if let Ok(mut s) = self.slot.lock() {
            s.0 = true;
            s.1.take();
        }
    }
}
