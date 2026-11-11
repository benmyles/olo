//! What the portal's dialogs share: a window over the app that asked (its
//! exported handle, xdg-foreign), and an answer given once. A dialog that
//! goes without answering (closed from its tab, cancelled by the app,
//! the portal ending) answers no as its view is dropped, so a request never
//! waits on a window that isn't there.

use olo_kit::{View, WindowHandle, WindowOptions};

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

/// A dialog on screen, as the portal keeps it: to close when the app
/// that asked cancels, and to know it's still up (an upgrade waits).
pub struct Shown {
    close: Box<dyn Fn() + Send>,
    open: Box<dyn Fn() -> bool + Send>,
}

impl Shown {
    pub fn of<V: View>(window: WindowHandle<V>) -> Shown {
        let closing = window.clone();
        Shown { close: Box::new(move || closing.close()), open: Box::new(move || window.is_open()) }
    }

    /// Closes it (it answers no as it goes).
    pub fn close(&self) {
        (self.close)()
    }

    pub fn is_open(&self) -> bool {
        (self.open)()
    }
}

/// Opens a dialog showing the view `build` makes, with the keyboard. A
/// close from its tab drops it (answering no).
pub fn open<V: View>(options: WindowOptions, build: impl FnOnce(&mut olo_kit::Cx<V>) -> V + Send + 'static) -> WindowHandle<V> {
    olo_kit::open_window(options, move |cx| {
        cx.activate_window();
        build(cx)
    })
}
