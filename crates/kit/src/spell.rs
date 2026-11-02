//! Spelling, natively: the system's Hunspell dictionaries
//! (`/usr/share/hunspell`, or `~/.local/share/hunspell`) read by
//! spellbook, in the person's language (`LANG`, else American English).
//! The dictionary loads on a worker the first time one is wanted
//! ([`checker`] says None until then, and asks for a frame when it is
//! ready); words learnt (Learn Spelling) are kept in
//! `~/.config/olo/words`, one a line, for every app.
//!
//! [`Editor`](crate::editor::Editor) underlines misspelled words when
//! its `Options::spelling` is on; an app offers [`Checker::suggest`]ions
//! for the word at the caret ([`word_at`]) in its right-click menu.

use std::sync::{Arc, Mutex, OnceLock};

pub use olo_kit_base::spell::{Checker, word_at, words};

enum Loading {
    NotYet,
    Busy(Vec<egui::Context>),
    Done(Option<Arc<Checker>>),
}

static STATE: OnceLock<Mutex<Loading>> = OnceLock::new();

/// The checker, once its dictionary has loaded (None before, or if there
/// is none for the language); the first call starts loading it, and
/// `ctx` gets a frame when it's ready.
pub fn checker(ctx: &egui::Context) -> Option<Arc<Checker>> {
    let state = STATE.get_or_init(|| Mutex::new(Loading::NotYet));
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    match &mut *s {
        Loading::Done(c) => c.clone(),
        Loading::Busy(waiting) => {
            if !waiting.iter().any(|c| c == ctx) {
                waiting.push(ctx.clone());
            }
            None
        }
        Loading::NotYet => {
            *s = Loading::Busy(vec![ctx.clone()]);
            let _ = std::thread::Builder::new().name("spelling".into()).spawn(|| {
                let c = olo_kit_base::spell::load().map(Arc::new);
                let state = STATE.get().expect("set before");
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                if let Loading::Busy(waiting) = std::mem::replace(&mut *s, Loading::Done(c)) {
                    for ctx in waiting {
                        ctx.request_repaint();
                    }
                }
            });
            None
        }
    }
}

/// The checker if its dictionary has loaded already (for a command
/// about a word it has marked).
pub fn loaded() -> Option<Arc<Checker>> {
    match &*STATE.get()?.lock().unwrap_or_else(|e| e.into_inner()) {
        Loading::Done(c) => c.clone(),
        _ => None,
    }
}

