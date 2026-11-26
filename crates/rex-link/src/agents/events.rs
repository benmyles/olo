//! What happens to a child, in order: a numbered log that a watcher
//! reads from where it last stopped (`olo agent watch --since CURSOR`),
//! so nothing between one watch and the next is lost.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::state::SubagentLiveState;

/// One monotonic event recorded when a child subagent transitions state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentEvent {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub host_key: String,
    pub session_id: String,
    pub parent_window_id: String,
    /// Full ancestor window chain so `watch --all` still matches exited grandchildren (`M11`).
    #[serde(default)]
    pub ancestor_windows: Vec<String>,
    pub id: String,
    pub name: Option<String>,
    pub window_id: String,
    #[serde(default)]
    pub block_id: String,
    #[serde(default)]
    pub resume_id: Option<String>,
    pub cli_session_id: Option<String>,
    pub event: String,
    pub state: SubagentLiveState,
    pub summary: Option<String>,
    pub last_message: Option<String>,
    pub approval: Option<Value>,
    pub error: Option<String>,
    pub tail: Vec<String>,
    /// Internal key (`approval:<key>`, `question:<hash>`, `msg:<hash>`, `exited`, etc.) used to deduplicate
    /// identical consecutive pushes per child (`H5`/`H7`) without dropping A->B transitions.
    /// Written down with the event, so a service that takes over knows
    /// what was last said of each child; never part of what a watcher is told.
    #[serde(default)]
    pub dedupe_key: String,
}

const MAX_EVENT_LOG: usize = 2000;

/// Monotonic ring buffer of child subagent state transitions so back-to-back
/// `olo agent watch --all --since CURSOR` calls never lose an event.
#[derive(Debug, Default)]
pub struct SubagentEventLog {
    next_seq: u64,
    events: VecDeque<SubagentEvent>,
    /// Last `(state, dedupe_key, seq)` pushed per child `id` so duplicate pushes are dropped cleanly (`H7`).
    last_by_id: HashMap<String, (SubagentLiveState, String, u64)>,
}

impl SubagentEventLog {
    pub fn with_start_seq(start_seq: u64) -> Self {
        Self { next_seq: start_seq, events: VecDeque::new(), last_by_id: HashMap::new() }
    }

    /// The log as a service before this one wrote it down: its events
    /// (the newest kept), numbered on from the last of them or from
    /// `start_seq`, whichever is later, and what was last said of each
    /// child (so the same thing isn't said twice across the two).
    pub fn restored(mut events: Vec<SubagentEvent>, start_seq: u64) -> Self {
        events.sort_by_key(|e| e.seq);
        let mut log = Self::with_start_seq(start_seq.max(events.last().map_or(0, |e| e.seq)));
        let skip = events.len().saturating_sub(MAX_EVENT_LOG);
        for ev in events.into_iter().skip(skip) {
            let key = if ev.dedupe_key.is_empty() { ev.event.clone() } else { ev.dedupe_key.clone() };
            log.last_by_id.insert(ev.id.clone(), (ev.state, key, ev.seq));
            log.events.push_back(ev);
        }
        log
    }

    pub fn latest_seq(&self) -> u64 {
        self.next_seq
    }

    /// Sequence number of the oldest event currently retained in the ring buffer (0 when empty).
    pub fn oldest_seq(&self) -> u64 {
        self.events.front().map_or(0, |e| e.seq)
    }

    /// True if `since` is older than the oldest buffered event (meaning events between `since` and
    /// `oldest_seq()` were evicted from the 2000-event ring buffer — `M12`).
    pub fn has_gap(&self, since: u64) -> bool {
        self.events.len() >= MAX_EVENT_LOG && since > 0 && self.oldest_seq() > since.saturating_add(1)
    }

    /// Clears per-child dedupe state (called on `send`/`approve` so the next transition is never swallowed — `H5`).
    pub fn reset_child_dedupe(&mut self, id: &str) {
        self.last_by_id.remove(id);
    }

    pub fn push(&mut self, mut ev: SubagentEvent) -> u64 {
        let key = if ev.dedupe_key.is_empty() { ev.event.clone() } else { ev.dedupe_key.clone() };
        if let Some((last_state, last_key, last_seq)) = self.last_by_id.get(&ev.id)
            && *last_state == ev.state
            && *last_key == key
        {
            return *last_seq;
        }
        self.next_seq += 1;
        ev.seq = self.next_seq;
        self.last_by_id.insert(ev.id.clone(), (ev.state, key, self.next_seq));
        self.events.push_back(ev);
        while self.events.len() > MAX_EVENT_LOG {
            self.events.pop_front();
        }
        self.next_seq
    }

    /// Written down beside what happened to the child, not as a state
    /// of its own: what its minder has for the parent (`escalation`).
    /// Always a new event (two alike are two), and what was last said of
    /// the child's own state stands, so that state isn't said again
    /// after it.
    pub fn push_beside(&mut self, mut ev: SubagentEvent) -> u64 {
        self.next_seq += 1;
        ev.seq = self.next_seq;
        self.events.push_back(ev);
        while self.events.len() > MAX_EVENT_LOG {
            self.events.pop_front();
        }
        self.next_seq
    }

    /// The last thing written of the child `id` itself (how it stood:
    /// not what was said beside it): what stands of it, as far as the
    /// log knows.
    pub fn last_state_of(&self, id: &str) -> Option<&SubagentEvent> {
        self.events.iter().rev().find(|e| e.id == id && e.event != super::minder::ESCALATION)
    }

    /// Finds the earliest event strictly after `since` matching `predicate`.
    pub fn find_since<F>(&self, since: u64, mut predicate: F) -> Option<SubagentEvent>
    where
        F: FnMut(&SubagentEvent) -> bool,
    {
        self.events.iter().find(|e| e.seq > since && predicate(e)).cloned()
    }
}
