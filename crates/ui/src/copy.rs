//! Copying a secret (a password, a one-time code): marked for clipboard
//! managers to leave alone (`x-kde-passwordManagerHint: secret`, which
//! KDE's and others honour), and taken off the clipboard again after a
//! while unless something else was copied since, as the egui kit's
//! `Cx::copy_secret`.

use std::time::Duration;

use gpui::{App, ClipboardItem};

/// Puts `text` on the clipboard as a secret, for `clear_after`.
pub fn secret(cx: &mut App, text: &str, clear_after: Duration) {
    let item = ClipboardItem::new_string(text.to_owned()).and_data("x-kde-passwordManagerHint", &b"secret"[..]);
    cx.write_to_clipboard(item.clone());
    cx.spawn(async move |cx| {
        cx.background_executor().timer(clear_after).await;
        let _ = cx.update(|cx| cx.withdraw_from_clipboard(&item));
    })
    .detach();
}
