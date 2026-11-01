//! What Olo's kits share that doesn't draw: one process per app
//! (`instance`), the desktop's settings, notifications, files and
//! places, watching folders, Tracker, the spacing system (`metrics`),
//! and the socket agents and the command palette read apps through
//! (`commands`), with what the layout review finds (`review`), and a
//! list's selection (`list`).

pub mod commands;
pub mod files;
pub mod instance;
pub mod layout_review;
pub mod list;
pub mod metrics;
pub mod notify;
pub mod places;
pub mod review;
pub mod settings;
pub mod tracker;
pub mod watch;

use std::{any::Any, sync::Arc};

pub use olo_xdg::{apps, config_dir, find_program, open};

/// Send messages to the app (`App::message`) from any thread.
#[derive(Clone)]
pub struct Proxy {
    send: Arc<dyn Fn(Box<dyn Any + Send>) + Send + Sync>,
}

impl Proxy {
    /// A proxy that hands each message to `send` (which gets it to the
    /// app's thread).
    pub fn new(send: impl Fn(Box<dyn Any + Send>) + Send + Sync + 'static) -> Proxy {
        Proxy { send: Arc::new(send) }
    }

    pub fn send<T: Any + Send>(&self, message: T) {
        (self.send)(Box::new(message));
    }
}
