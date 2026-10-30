//! Token pooling for Claude Code and Codex: several subscriptions' sign-ins
//! kept sealed, and a loopback gateway the clients are pointed at that
//! spends whichever account's quota expires soonest first, refreshing
//! sign-ins and reading quota as it goes. Usage is counted from the
//! providers' reports; what is asked and answered is never kept.

pub mod authority;
pub mod clients;
pub mod codex_daemon;
pub mod gateway;
pub mod import;
pub mod intercept;
pub mod peer;
pub mod model;
pub mod schedule;
pub mod store;
pub mod upstream;
