//! Copying a secret (a password, a one-time code): marked for clipboard
//! managers to leave alone (`x-kde-passwordManagerHint: secret`, which
//! KDE's and others honour), and taken off the clipboard again after a
//! while unless something else was copied since.

use std::time::Duration;

use olo_interface::ClipboardItem;

/// Puts `text` on the clipboard as a secret, for `clear_after` (from any
/// thread; a thread of its own waits to take it off again).
pub fn secret(text: &str, clear_after: Duration) {
    let item = ClipboardItem::new_string(text.to_owned()).and_data("x-kde-passwordManagerHint", &b"secret"[..]);
    olo_interface::clipboard::write(item.clone());
    let _ = std::thread::Builder::new().name("secret copied".into()).spawn(move || {
        std::thread::sleep(clear_after);
        olo_interface::clipboard::withdraw(item);
    });
}
