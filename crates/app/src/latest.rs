//! The latest of something, for a looper that may be behind.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_sync::Mutex;

use crate::LooperHandle;

/// A slot holding the latest value of something that changes faster than
/// it need be taken (where the pointer is, a window's new size, how far
/// a copy has got): putting a new one replaces what hasn't been taken
/// yet, and the looper is told once however many were put. The answer to
/// BeOS's ports filling with `B_MOUSE_MOVED`: the sender gathers, the
/// queue holds one.
pub struct Latest<T> {
    shared: Arc<Shared<T>>,
}

struct Shared<T> {
    value: Mutex<Option<T>>,
    /// A message telling the looper to take it is on its way.
    posted: AtomicBool,
}

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Latest { shared: self.shared.clone() }
    }
}

impl<T: Send + 'static> Default for Latest<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> Latest<T> {
    pub fn new() -> Latest<T> {
        Latest { shared: Arc::new(Shared { value: Mutex::new(None), posted: AtomicBool::new(false) }) }
    }

    /// Puts `value` (gathered with what's there, if any, by `merge`,
    /// which is given the one waiting and the new one) and has `take`
    /// run on the looper with whatever is latest when its turn comes.
    pub fn put_with(&self, looper: &LooperHandle, value: T, merge: impl FnOnce(T, T) -> T, take: impl FnOnce(&mut crate::Looper, T) + Send + 'static) {
        {
            let mut slot = self.shared.value.lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some(match slot.take() {
                Some(waiting) => merge(waiting, value),
                None => value,
            });
        }
        if !self.shared.posted.swap(true, Ordering::AcqRel) {
            let shared = self.shared.clone();
            looper.post(move |looper| {
                shared.posted.store(false, Ordering::Release);
                let value = shared.value.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(value) = value {
                    take(looper, value);
                }
            });
        }
    }

    /// Puts `value` in place of any not taken yet.
    pub fn put(&self, looper: &LooperHandle, value: T, take: impl FnOnce(&mut crate::Looper, T) + Send + 'static) {
        self.put_with(looper, value, |_, new| new, take);
    }
}
