//! Messengers: how messages reach a handler from anywhere.

use std::sync::Arc;

use crate::{Looper, LooperHandle};

/// Sends messages of type `M` to something in a looper, from any thread:
/// BeOS's `BMessenger`, typed. Cheap to clone; sending never blocks and
/// never fails loudly (a looper that has ended takes nothing more, as a
/// window that closed while its worker was still finishing).
pub struct Messenger<M> {
    looper: LooperHandle,
    deliver: Arc<Deliver<M>>,
}

/// Hands a message to what it's for, on its looper's thread.
type Deliver<M> = dyn Fn(&mut Looper, M) + Send + Sync;

impl<M> Clone for Messenger<M> {
    fn clone(&self) -> Self {
        Messenger { looper: self.looper.clone(), deliver: self.deliver.clone() }
    }
}

impl<M: Send + 'static> Messenger<M> {
    /// A messenger that hands each message to `deliver` on `looper`'s
    /// thread.
    pub fn new(looper: LooperHandle, deliver: impl Fn(&mut Looper, M) + Send + Sync + 'static) -> Messenger<M> {
        Messenger { looper, deliver: Arc::new(deliver) }
    }

    /// Sends `message`: it's taken on the looper's thread after what is
    /// queued already. False if the looper has ended.
    pub fn send(&self, message: M) -> bool {
        let deliver = self.deliver.clone();
        if trace() {
            tracing::info!(to = self.looper.name(), "message: {}", std::any::type_name::<M>());
        }
        self.looper.post(move |looper| deliver(looper, message))
    }

    /// A messenger for another type of message, each made into this
    /// one's by `f` on the sending thread: how a part is handed a way to
    /// answer in its own terms (`messenger.map(Msg::Loaded)`).
    pub fn map<N: Send + 'static>(&self, f: impl Fn(N) -> M + Send + Sync + 'static) -> Messenger<N> {
        let deliver = self.deliver.clone();
        Messenger { looper: self.looper.clone(), deliver: Arc::new(move |looper, n| deliver(looper, f(n))) }
    }

    /// The looper it sends to.
    pub fn looper(&self) -> &LooperHandle {
        &self.looper
    }

    /// Whether what it sends to can still take a message.
    pub fn is_valid(&self) -> bool {
        !self.looper.is_gone()
    }
}

impl<M> std::fmt::Debug for Messenger<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Messenger<{}>({})", std::any::type_name::<M>(), self.looper.name())
    }
}

/// `OLO_MESSAGES=1`: every message sent through a messenger is logged,
/// its type and where it's going (what a program is being told, as it
/// happens).
fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_MESSAGES").is_some())
}
