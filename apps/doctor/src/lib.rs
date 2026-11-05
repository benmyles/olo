//! Olo Doctor and Olo Suggestions, one body: the checks and the
//! suggestions, the steps that fix or apply them, the privileged helper,
//! the session's watch, and the window and command line both share
//! (`run`, by `Kind`).

mod app;
pub mod checks;
pub mod crashes;
pub mod fix;
pub mod helper;
pub mod notify;
pub mod releases;
pub mod suggest;

pub use app::{Kind, run};
