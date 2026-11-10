//! Olo's Application Kit: loopers, handlers and typed messages.
//!
//! BeOS's Application Kit, as it makes sense now. A [`Looper`] is a
//! thread with a queue: it takes what is posted to it, one at a time, in
//! the order it came, and sleeps when there is nothing (no polling). A
//! [`Handler`] lives in a looper and takes the messages sent to it; a
//! [`Messenger`] is how any thread sends them. Where BeOS's `BMessage`
//! was a bag of named fields checked as it was read, a message here is a
//! Rust type (most often an enum): what a handler can be told is its
//! `Message`, and the compiler holds every sender to it.
//!
//! What BeOS taught, kept:
//! - every window (and the app itself) has a looper of its own, so one
//!   being busy never holds another up;
//! - state belongs to one looper and is changed only there, by messages;
//! - a `BMessageRunner` sends a message later or at intervals
//!   ([`Looper::after`], [`Looper::every`]).
//!
//! What it taught the hard way, left out:
//! - no `Lock()`: nothing outside a looper's thread ever touches what it
//!   owns, so there is nothing to lock (the compiler checks it: what a
//!   looper owns isn't `Send`), and no two loopers can deadlock on each
//!   other's locks;
//! - no reply waited for on a looper's thread: [`ask`] gives an
//!   [`Answer`] that a looper awaits in a task (or takes as a message),
//!   and only a plain worker thread may block on;
//! - no port that fills and blocks its sender: queues grow, and what
//!   would flood one (pointer motion, a resize, progress) is the
//!   sender's to gather ([`Latest`]).
//!
//! Work that can take longer than a frame runs off the loopers:
//! [`pool::spawn`] for a future, [`pool::run`] for a blocking call, each
//! answering with a message.

mod answer;
mod latest;
mod looper;
mod messenger;
pub mod pool;
mod task;

pub use answer::{Answer, Reply, ask};
pub use latest::Latest;
pub use looper::{Handler, HandlerId, Looper, LooperHandle, TimerId};
pub use messenger::Messenger;
pub use task::Task;
