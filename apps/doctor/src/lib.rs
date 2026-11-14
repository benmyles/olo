//! Olo Doctor: the checks and the suggestions (once Olo Suggestions, an
//! app of its own), the steps that fix or apply them, the privileged
//! helper, the session's watch, and the window and command line (`run`).

mod app;
pub mod checks;
pub mod crashes;
pub mod demo;
pub mod fans;
pub mod fix;
pub mod helper;
pub mod ignored;
pub mod login_screen;
pub mod notify;
pub mod releases;
pub mod scheduler;
pub mod suggest;

pub use app::run;
