//! What the portal's dialogs share: a window over the app that asked (its
//! exported handle, xdg-foreign), and an answer given once. A dialog that
//! goes without answering (closed from its tab, cancelled by the app,
//! the portal ending) answers no as it's dropped, so a request never
//! waits on a window that isn't there.

use olo_ui::{
    Frame, WindowOptions,
    gpui::{App, AppContext as _, Context, Entity, Render, Window, WindowHandle},
};

/// An answer to a request, given once: `give`n, or `cancel` when dropped
/// unanswered.
pub struct Answer<T> {
    reply: async_channel::Sender<T>,
    cancel: Option<T>,
}

impl<T> Answer<T> {
    pub fn new(reply: async_channel::Sender<T>, cancel: T) -> Answer<T> {
        Answer { reply, cancel: Some(cancel) }
    }

    /// Answers with `value` (only the first answer counts).
    pub fn give(&mut self, value: T) {
        if self.cancel.take().is_some() {
            let _ = self.reply.try_send(value);
        }
    }
}

impl<T> Drop for Answer<T> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = self.reply.try_send(cancel);
        }
    }
}

/// `options` over `parent` (as a portal names it: `wayland:HANDLE`, or "").
pub fn over(options: WindowOptions, parent: &str) -> WindowOptions {
    if parent.is_empty() { options } else { options.foreign_parent(parent) }
}

/// Opens a dialog showing `view` (made by `build`), with the keyboard. A
/// close from its tab drops it (answering no).
pub fn open<V: Render + 'static>(cx: &mut App, options: WindowOptions, build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static) -> Option<WindowHandle<Frame>> {
    olo_ui::open_window(cx, options, move |window, cx| {
        window.activate_window();
        let view: Entity<V> = cx.new(|cx| build(window, cx));
        view
    })
}
