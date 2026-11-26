use std::future::Future;

use futures_util::StreamExt;
use tracing::warn;
use zbus::{
    Connection, MessageStream,
    names::{BusName, OwnedUniqueName},
};

use crate::{match_rules::MatchRules, name_registry::NameRegistry};

use super::Peer;

/// A peer connection.
#[derive(Debug)]
pub struct Monitor {
    conn: Connection,
    outbox: super::Outbox,
    unique_name: OwnedUniqueName,
    match_rules: MatchRules,
}

impl Monitor {
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// What it is shown goes here, after what was on its way to it as a
    /// peer (the peer's own queue, kept).
    pub fn outbox(&self) -> &super::Outbox {
        &self.outbox
    }

    pub fn unique_name(&self) -> &OwnedUniqueName {
        &self.unique_name
    }

    /// # Panics
    ///
    /// Same as [`MatchRules::matches`].
    ///
    /// Olo: a message to nobody in particular (every broadcast signal)
    /// has no destination. busd unwrapped it here, for a monitor with
    /// rules: the first signal after one began to watch (`dbus-monitor
    /// "type='signal'"`) ended the task that serves the bus's own name,
    /// and the bus answered nobody from then on.
    pub fn interested(&self, msg: &zbus::Message, name_registry: &NameRegistry) -> bool {
        if self.match_rules.is_empty() || msg.header().destination() == Some(&BusName::from(&self.unique_name)) {
            return true;
        }
        self.match_rules.matches(msg, name_registry)
    }

    /// Monitor the monitor.
    ///
    /// This method returns once the monitor connection is closed.
    pub fn monitor(&self) -> impl Future<Output = ()> + 'static {
        let mut stream = MessageStream::from(&self.conn);
        let unique_name = self.unique_name.clone();
        async move {
            if let Some(Ok(_)) = stream.next().await {
                warn!("Monitor {} sent a message, which is against the rules.", unique_name);
            }
        }
    }

    pub(super) fn new(peer: Peer, match_rules: MatchRules) -> Self {
        Self { conn: peer.conn().clone(), outbox: peer.outbox().clone(), unique_name: peer.unique_name().clone(), match_rules }
    }
}
