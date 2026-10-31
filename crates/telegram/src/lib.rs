//! Telegram for Olo (`olo-telegram`, Preferences ▸ Integrations ▸
//! Telegram). MTProto in process through grammers: no Telegram Desktop,
//! no tdlib. [`model`] is the chats and messages as plain data;
//! [`backend`] the commands the app sends and the events it hears, from
//! [`net`] (the real account, on a tokio runtime of its own) or [`demo`]
//! (a pretend one, for tests); [`account`] what's kept (the API ID and
//! session in the keyring, the cache on disk); [`signin`] signing in
//! with a code, a QR code and the two-step password.

pub mod account;
pub mod backend;
pub mod demo;
pub mod model;
pub mod net;
pub mod signin;

pub use backend::{Backend, Cmd, Event, Outgoing, Pic, Status};
pub use model::*;
