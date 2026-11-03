//! Olo Doctor and Olo Suggestions, one body: the checks and the
//! suggestions, the steps that fix or apply them, the privileged helper,
//! the session's watch, and the window and command line both share
//! (`run`, by `Kind`).

pub mod checks;
pub mod fix;
pub mod helper;
pub mod notify;
pub mod suggest;
mod app;

pub use app::{Kind, run};
