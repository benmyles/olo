//! Spelling: kit-base's checker ([`olo_kit_base::spell`]), its
//! dictionary read on a worker the first time one is wanted; whoever
//! asked meanwhile says what it shows again once it's ready. The
//! [`Editor`](crate::editor::Editor) underlines misspelled words when its
//! `Options::spelling` is on; an app offers [`Checker::suggest`]ions for
//! the word at the caret ([`word_at`]) in its right-click menu.

use std::sync::{Arc, Mutex};

use olo_interface::Invalidator;
pub use olo_kit_base::spell::{Checker, word_at, words};

enum Spelling {
    /// Not asked for yet.
    Unread,
    /// Being read: who to tell when it is.
    Loading(Vec<Invalidator>),
    Ready(Option<Arc<Checker>>),
}

static SPELLING: Mutex<Spelling> = Mutex::new(Spelling::Unread);

/// The checker, once its dictionary has loaded (None before, or if there
/// is none for the language); the first call starts loading it, and
/// `asking` says what it shows again when it has.
pub fn checker(asking: &Invalidator) -> Option<Arc<Checker>> {
    let mut spelling = SPELLING.lock().unwrap_or_else(|e| e.into_inner());
    match &mut *spelling {
        Spelling::Ready(c) => c.clone(),
        Spelling::Loading(waiting) => {
            waiting.push(asking.clone());
            None
        }
        Spelling::Unread => {
            *spelling = Spelling::Loading(vec![asking.clone()]);
            olo_app::pool::run(|| {
                let checker = olo_kit_base::spell::load().map(Arc::new);
                let was = std::mem::replace(&mut *SPELLING.lock().unwrap_or_else(|e| e.into_inner()), Spelling::Ready(checker));
                if let Spelling::Loading(waiting) = was {
                    for w in waiting {
                        w.invalidate();
                    }
                }
            })
            .detach();
            None
        }
    }
}

/// The checker if its dictionary has loaded already (for a command
/// about a word it has marked).
pub fn loaded() -> Option<Arc<Checker>> {
    match &*SPELLING.lock().unwrap_or_else(|e| e.into_inner()) {
        Spelling::Ready(c) => c.clone(),
        _ => None,
    }
}
