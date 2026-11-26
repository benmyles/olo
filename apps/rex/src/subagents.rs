//! The children of Rex tabs (`olo agent`), as this app shows them.
//!
//! They are their host's: the agents service there (`olo-rex-link agent
//! serve`) starts, reads, answers and stops them and keeps who is whose
//! child, with or without an Olo desktop connected
//! (`olo_rex_link::agents`, HACKING, Rex, "The children of a tab"). This
//! app follows each connected host's service (`RexHubInner::
//! follow_agents`) and shows what it says: only top-level tabs in the
//! tab strip, a tab's children under its tree button and in Activity.
//! What is here is what the two agree on, and nothing else.

pub use olo_rex_link::agents::{AgentCli, DesktopPolicy, NativeSubagent, SubagentRegistry, TreeRow, now_unix_ms};

/// Where this app kept the children itself when it was their keeper
/// (before 2026-11-26): read once more for each host, to hand them to
/// its service (`RexHubInner::hand_over_kept_children`).
pub fn kept_before() -> std::path::PathBuf {
    olo_xdg::state_dir().join("rex-subagents.json")
}
