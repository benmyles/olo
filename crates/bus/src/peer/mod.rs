mod stream;
use event_listener::{Event, EventListener};
pub use stream::*;
mod monitor;
pub use monitor::*;

use anyhow::Result;
use tracing::trace;
use zbus::{
    AuthMechanism, Connection, MessageStream, OwnedGuid, OwnedMatchRule,
    connection::{self, socket::BoxedSplit},
    names::{BusName, OwnedUniqueName},
};

use crate::{fdo, match_rules::MatchRules, name_registry::NameRegistry};

/// A peer connection.
#[derive(Debug)]
pub struct Peer {
    conn: Connection,
    unique_name: OwnedUniqueName,
    match_rules: MatchRules,
    greeted: bool,
    canceled_event: Event,
}

impl Peer {
    /// The peer at `socket`, once it has said who it is, and the stream
    /// of what it sends.
    ///
    /// Olo: the stream is listening before the connection starts to
    /// read (`build_message_stream`, which zbus has for a bus). A client
    /// sends its `Hello` in the same write as the end of its handshake,
    /// so the bus has read it by the time the connection is made, and
    /// the connection's reader hands it on at once, on a thread of its
    /// own: a stream made after (`MessageStream::from(&conn)`, as busd
    /// 0.5 had it) missed it whenever that thread ran first, zbus
    /// dropping what nobody listens for. The client then waited for
    /// ever for its `Hello` to be answered: 19 of 1,500 buses started
    /// 96 at once never said "ready", waiting for their own services'
    /// client, and a test sat 25 minutes (2026-11-26).
    pub async fn new(guid: OwnedGuid, id: usize, socket: BoxedSplit, auth_mechanism: AuthMechanism) -> Result<(Self, Stream)> {
        let unique_name = OwnedUniqueName::try_from(format!(":busd.{id}")).unwrap();
        let messages = connection::Builder::socket(socket).server(guid)?.p2p().auth_mechanism(auth_mechanism).build_message_stream().await?;
        let conn = Connection::from(&messages);
        trace!("created: {:?}", conn);

        let peer = Self { conn, unique_name, match_rules: MatchRules::default(), greeted: false, canceled_event: Event::new() };
        let stream = Stream::for_peer(&peer, messages);
        Ok((peer, stream))
    }

    // This the the bus itself, serving the FDO D-Bus API: its end of the
    // pair, and the stream of what the bus's own service sends (listening
    // from before its connection read, as every peer's).
    pub async fn new_us(messages: MessageStream) -> (Self, Stream) {
        let unique_name = OwnedUniqueName::try_from(fdo::BUS_NAME).unwrap();

        let peer = Self { conn: Connection::from(&messages), unique_name, match_rules: MatchRules::default(), greeted: true, canceled_event: Event::new() };
        let stream = Stream::for_peer(&peer, messages);
        (peer, stream)
    }

    pub fn unique_name(&self) -> &OwnedUniqueName {
        &self.unique_name
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn listen_cancellation(&self) -> EventListener {
        self.canceled_event.listen()
    }

    /// # Panics
    ///
    /// Same as [`MatchRules::matches`].
    pub fn interested(&self, msg: &zbus::Message, name_registry: &NameRegistry) -> bool {
        msg.header().destination() == Some(&BusName::Unique(self.unique_name.as_ref())) || self.match_rules.matches(msg, name_registry)
    }

    pub fn add_match_rule(&mut self, rule: OwnedMatchRule) {
        self.match_rules.add(rule);
    }

    /// Remove the first rule that matches.
    pub fn remove_match_rule(&mut self, rule: OwnedMatchRule) -> zbus::fdo::Result<()> {
        self.match_rules.remove(rule)
    }

    /// This can only be called once.
    pub async fn hello(&mut self) -> zbus::fdo::Result<()> {
        if self.greeted {
            return Err(zbus::fdo::Error::Failed("Can only call `Hello` method once".to_string()));
        }
        self.greeted = true;

        Result::Ok(())
    }

    pub fn become_monitor(self, match_rules: MatchRules) -> Monitor {
        Monitor::new(self, match_rules)
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.canceled_event.notify(usize::MAX);
    }
}
