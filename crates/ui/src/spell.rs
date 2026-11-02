//! Spelling in the GPUI kit: kit-base's checker
//! ([`olo_kit_base::spell`]), its dictionary read on GPUI's background
//! executor the first time one is wanted; every window draws again once
//! it's ready. The [`Editor`](crate::editor::Editor) underlines
//! misspelled words when its `Options::spelling` is on; an app offers
//! [`Checker::suggest`]ions for the word at the caret ([`word_at`]) in its
//! right-click menu.

use std::sync::Arc;

use gpui::{App, Global};

pub use olo_kit_base::spell::{Checker, word_at, words};

enum Spelling {
    Loading,
    Ready(Option<Arc<Checker>>),
}

impl Global for Spelling {}

/// The checker, once its dictionary has loaded (None before, or if there
/// is none for the language); the first call starts loading it.
pub fn checker(cx: &mut App) -> Option<Arc<Checker>> {
    match cx.try_global::<Spelling>() {
        Some(Spelling::Ready(c)) => return c.clone(),
        Some(Spelling::Loading) => return None,
        None => {}
    }
    cx.set_global(Spelling::Loading);
    let load = cx.background_executor().spawn(async { olo_kit_base::spell::load().map(Arc::new) });
    cx.spawn(async move |cx| {
        let c = load.await;
        let _ = cx.update(|cx| {
            cx.set_global(Spelling::Ready(c));
            cx.refresh_windows();
        });
    })
    .detach();
    None
}

/// The checker if its dictionary has loaded already (for a command
/// about a word it has marked).
pub fn loaded(cx: &App) -> Option<Arc<Checker>> {
    match cx.try_global::<Spelling>() {
        Some(Spelling::Ready(c)) => c.clone(),
        _ => None,
    }
}
