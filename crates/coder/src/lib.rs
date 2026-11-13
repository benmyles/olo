//! Olo Coder's interfaces share this service and its versioned command API.
//! No interface owns a turn: disconnecting a client leaves the turn running.
pub mod commands;
pub mod engine;
pub mod git;
pub mod github;
pub mod mcp;
pub mod model;
pub mod provider;
pub mod rpc;
pub mod sandbox;
pub mod store;
pub mod todos;
pub mod tools;
pub use engine::Engine;
pub use model::*;

mod coordination;

mod chat;
pub mod compaction;

pub mod integrations;
pub mod oauth;
pub mod supervisor;

pub mod profiles;
pub mod providers;
