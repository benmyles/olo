use anyhow::{Context, Result, bail};
use async_lock::RwLock;
use event_listener::EventListener;
use futures_util::{
    future::{Either, select},
    stream::StreamExt,
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use crate::spawn;
use tracing::{debug, trace, warn};
use zbus::{
    AuthMechanism, Message, OwnedGuid,
    connection::socket::BoxedSplit,
    message,
    names::{BusName, OwnedUniqueName, UniqueName},
    zvariant::Optional,
};

use crate::{
    fdo,
    match_rules::MatchRules,
    name_registry::{NameOwnerChanged, NameRegistry},
    peer::{Monitor, Peer, Stream},
};

#[derive(Debug)]
pub struct Peers {
    peers: RwLock<BTreeMap<OwnedUniqueName, Peer>>,
    monitors: RwLock<BTreeMap<OwnedUniqueName, Monitor>>,
    name_registry: RwLock<NameRegistry>,
    /// Olo: each call delivered and not yet answered, by who has it, who
    /// asked and the call's serial: the call itself, to answer for it
    /// (`NoReply`) if who has it goes without answering, as dbus-daemon
    /// and dbus-broker do. busd didn't, and a zbus caller (no timeout of
    /// its own) waited forever: Preferences' NetworkManager worker on a
    /// test bus whose stand-in NetworkManager was stopped mid-call.
    pending: std::sync::Mutex<BTreeMap<(OwnedUniqueName, OwnedUniqueName, u32), Message>>,
}

impl Peers {
    pub fn new() -> Arc<Self> {
        let name_registry = NameRegistry::default();

        Arc::new(Self { peers: RwLock::new(BTreeMap::new()), monitors: RwLock::new(BTreeMap::new()), name_registry: RwLock::new(name_registry), pending: std::sync::Mutex::new(BTreeMap::new()) })
    }

    pub async fn add(self: &Arc<Self>, guid: &OwnedGuid, id: usize, socket: BoxedSplit, auth_mechanism: AuthMechanism) -> Result<()> {
        // Olo: who it is first, the list's lock after. busd 0.5 took the
        // lock and then waited for the handshake under it: a program
        // that connected and said nothing more held the list for ever,
        // and with it every other program's connecting, going and
        // messages (each reads the list).
        let (peer, peer_stream) = Peer::new(guid.clone(), id, socket, auth_mechanism).await?;
        let mut peers = self.peers_mut().await;
        let unique_name = peer.unique_name().clone();
        match peers.get(&unique_name) {
            Some(peer) => panic!("Unique name `{}` re-used. We're in deep trouble if this happens", peer.unique_name()),
            None => {
                // (In the list first, and the list let go, before anything
                // of its is served: its `Hello` finds it there.)
                let listener = peer.listen_cancellation();
                peers.insert(unique_name.clone(), peer);
                drop(peers);
                spawn(self.clone().serve_peer(peer_stream, listener, unique_name));
            }
        }

        Ok(())
    }

    pub async fn add_us(self: &Arc<Self>, messages: zbus::MessageStream) {
        let mut peers = self.peers_mut().await;
        let (peer, peer_stream) = Peer::new_us(messages).await;
        let unique_name = peer.unique_name().clone();
        match peers.get(&unique_name) {
            Some(peer) => panic!("Unique name `{}` re-used. We're in deep trouble if this happens", peer.unique_name()),
            None => {
                // (In the list first, and the list let go, before anything
                // of its is served: its `Hello` finds it there.)
                let listener = peer.listen_cancellation();
                peers.insert(unique_name.clone(), peer);
                drop(peers);
                spawn(self.clone().serve_peer(peer_stream, listener, unique_name));
            }
        }
    }

    pub async fn peers(&self) -> impl Deref<Target = BTreeMap<OwnedUniqueName, Peer>> + '_ {
        self.peers.read().await
    }

    pub async fn peers_mut(&self) -> impl DerefMut<Target = BTreeMap<OwnedUniqueName, Peer>> + '_ {
        self.peers.write().await
    }

    pub async fn name_registry(&self) -> impl Deref<Target = NameRegistry> + '_ {
        self.name_registry.read().await
    }

    pub async fn name_registry_mut(&self) -> impl DerefMut<Target = NameRegistry> + '_ {
        self.name_registry.write().await
    }

    pub async fn make_monitor(self: &Arc<Self>, peer_name: &UniqueName<'_>, match_rules: MatchRules) -> bool {
        let monitor = {
            let mut peers = self.peers_mut().await;
            let peer = match peers.remove(peer_name.as_str()) {
                Some(peer) => peer,
                None => {
                    return false;
                }
            };

            peer.become_monitor(match_rules)
        };

        let monitor_monitoring_fut = monitor.monitor();
        let unique_name = monitor.unique_name().clone();
        let peers = self.clone();
        self.monitors.write().await.insert(unique_name.clone(), monitor);

        spawn(async move {
            monitor_monitoring_fut.await;
            peers.monitors.write().await.remove(&unique_name);
            debug!("Monitor {} disconnected", unique_name);
        });

        true
    }

    pub async fn notify_name_changes(&self, name_owner_changed: NameOwnerChanged) -> Result<()> {
        let name = BusName::from(name_owner_changed.name);
        let old_owner = name_owner_changed.old_owner.map(UniqueName::from);
        let new_owner = name_owner_changed.new_owner.map(UniqueName::from);

        // First broadcast the name change signal.
        let msg = Message::signal(fdo::DBus::PATH, fdo::DBus::INTERFACE, "NameOwnerChanged").unwrap().sender(fdo::BUS_NAME).unwrap().build(&(
            &name,
            Optional::from(old_owner.clone()),
            Optional::from(new_owner.clone()),
        ))?;
        self.broadcast_msg(msg).await;

        // Now unicast the appropriate signal to the old and new owners.
        if let Some(old_owner) = old_owner {
            let msg = Message::signal(fdo::DBus::PATH, fdo::DBus::INTERFACE, "NameLost").unwrap().sender(fdo::BUS_NAME).unwrap().destination(old_owner.clone()).unwrap().build(&name)?;
            if let Err(e) = self.send_msg_to_unique_name(msg, old_owner.clone()).await {
                debug!("Couldn't notify inexistant peer {old_owner} about loosing name {name}: {e}")
            }
        }
        if let Some(new_owner) = new_owner {
            let msg = Message::signal(fdo::DBus::PATH, fdo::DBus::INTERFACE, "NameAcquired").unwrap().sender(fdo::BUS_NAME).unwrap().destination(new_owner.clone()).unwrap().build(&name)?;
            if let Err(e) = self.send_msg_to_unique_name(msg, new_owner.clone()).await {
                warn!("Couldn't notify peer {new_owner} about acquiring name {name}: {e}")
            }
        }

        Ok(())
    }

    async fn serve_peer(self: Arc<Self>, mut peer_stream: Stream, mut cancellation_listener: EventListener, unique_name: OwnedUniqueName) -> Result<()> {
        loop {
            let msg = match select(cancellation_listener, peer_stream.next()).await {
                Either::Left(_) | Either::Right((None, _)) => {
                    trace!("Peer `{}` disconnected", unique_name);

                    break;
                }
                Either::Right((Some(msg), listener)) => {
                    cancellation_listener = listener;

                    match msg {
                        Ok(msg) => msg,
                        Err(e) => {
                            debug!("{e}");

                            continue;
                        }
                    }
                }
            };

            match msg.message_type() {
                message::Type::Signal => self.broadcast_msg(msg).await,
                _ => match msg.header().destination() {
                    Some(dest) => {
                        // Olo: an answer settles the call it answers.
                        if let (message::Type::MethodReturn | message::Type::Error, Some(serial), BusName::Unique(caller)) = (msg.message_type(), msg.header().reply_serial(), dest) {
                            self.pending.lock().unwrap().remove(&(unique_name.clone(), caller.to_owned().into(), serial.get()));
                        }
                        // Olo: a call is kept (for `NoReply`, should who
                        // has it go without answering) from before it is
                        // sent. Kept after, a recipient that went in
                        // between had already answered for what it had:
                        // the call was kept for nobody, and its caller
                        // waited for good.
                        let to = self.owner_of(dest).await;
                        let kept = to.as_ref().and_then(|to| self.expect_answer(&msg, to, &unique_name));
                        let sent = match &to {
                            Some(to) => self.send_msg_to_unique_name(msg.clone(), to.as_ref()).await,
                            None => Err(anyhow::anyhow!("unknown destination: {dest}")),
                        };
                        if let Err(e) = sent {
                            debug!("{}", e);
                            // Olo: a call to a name nobody has is answered
                            // at once, as other brokers answer it (busd left
                            // the caller waiting out its own timeout: GLib's
                            // is 25 s, a GTK app's start held up that long).
                            // Once: not where its recipient's going has
                            // answered it meanwhile (the kept call is gone).
                            if kept.is_none_or(|key| self.pending.lock().unwrap().remove(&key).is_some()) {
                                self.answer_unknown(&msg, dest).await;
                            }
                        }
                    }
                    // peer::Stream ensures a valid destination so this isn't exactly needed.
                    _ => bail!("invalid message: {:?}", msg),
                },
            };
        }

        // Stream is done means the peer disconnected or it became a monitor. Remove it from the
        // list of peers.
        let was_peer = self.peers_mut().await.remove(&unique_name).is_some();

        // Olo: the calls it had and never answered are answered for it, and
        // its own calls need no answer now. After it is out of the list,
        // so that no call is sent to it once its calls have been answered
        // (a call is kept before it is sent: above).
        self.answer_for(&unique_name).await;

        if !was_peer {
            // This means peer was turned into a monitor. `Monitoring` iface will emit the signals.
            return Ok(());
        }
        let names_changes = self.name_registry_mut().await.release_all(unique_name.inner().clone()).await;
        for changed in names_changes {
            self.notify_name_changes(changed).await?;
        }
        self.notify_name_changes(NameOwnerChanged { name: BusName::from(unique_name.clone()).into(), old_owner: Some(unique_name.clone()), new_owner: None }).await?;

        Ok(())
    }

    /// A method call to `dest`, which nobody has: its caller is told so
    /// (`ServiceUnknown`, from the bus), unless it wants no reply.
    async fn answer_unknown(&self, msg: &Message, dest: &BusName<'_>) {
        let header = msg.header();
        if msg.message_type() != message::Type::MethodCall || header.primary().flags().contains(message::Flags::NoReplyExpected) {
            return;
        }
        let Some(sender) = header.sender().map(|s| s.to_owned()) else { return };
        let reply = Message::error(&header, "org.freedesktop.DBus.Error.ServiceUnknown")
            .and_then(|b| b.sender("org.freedesktop.DBus"))
            .and_then(|b| b.build(&(format!("The name {dest} was not provided by any .service files"),)));
        match reply {
            Ok(reply) => {
                if let Err(e) = self.send_msg_to_unique_name(reply, sender.as_ref()).await {
                    debug!("couldn't tell {sender} that {dest} isn't there: {e}");
                }
            }
            Err(e) => warn!("couldn't make an error reply: {e}"),
        }
    }

    /// A call `msg` from `caller`, about to be sent to `to`, kept until
    /// it's answered, unless no answer is wanted; what it is kept by.
    fn expect_answer(&self, msg: &Message, to: &OwnedUniqueName, caller: &OwnedUniqueName) -> Option<(OwnedUniqueName, OwnedUniqueName, u32)> {
        if msg.message_type() != message::Type::MethodCall || msg.primary_header().flags().contains(message::Flags::NoReplyExpected) {
            return None;
        }
        let key = (to.clone(), caller.clone(), msg.primary_header().serial_num().get());
        self.pending.lock().unwrap().insert(key.clone(), msg.clone());
        Some(key)
    }

    /// `gone` went: each call it had and never answered gets the bus's
    /// `NoReply`, as dbus-daemon and dbus-broker answer it, and the calls
    /// it made are forgotten.
    async fn answer_for(&self, gone: &OwnedUniqueName) {
        let mut unanswered: Vec<Message> = Vec::new();
        self.pending.lock().unwrap().retain(|(to, caller, _), call| {
            if to == gone && caller != gone {
                unanswered.push(call.clone());
            }
            to != gone && caller != gone
        });
        for call in unanswered {
            let header = call.header();
            let Some(caller) = header.sender().map(|s| s.to_owned()) else { continue };
            let reply = Message::error(&header, "org.freedesktop.DBus.Error.NoReply")
                .and_then(|b| b.sender("org.freedesktop.DBus"))
                .and_then(|b| b.build(&("Message recipient disconnected from message bus without replying",)));
            match reply {
                Ok(reply) => {
                    if let Err(e) = self.send_msg_to_unique_name(reply, caller.as_ref()).await {
                        debug!("couldn't tell {caller} that {gone} went without answering: {e}");
                    }
                }
                Err(e) => warn!("couldn't make an error reply: {e}"),
            }
        }
    }

    /// Who has `destination` now: itself if it is a connection's own
    /// name, else whoever has that name; None if nobody has.
    async fn owner_of(&self, destination: &BusName<'_>) -> Option<OwnedUniqueName> {
        match destination {
            BusName::Unique(dest) => Some(dest.to_owned().into()),
            BusName::WellKnown(name) => self.name_registry().await.lookup(name.clone()),
        }
    }

    /// `msg` to the peer `destination`, and to the monitors. Into each
    /// one's queue (`Outbox`): nothing here waits for a recipient.
    ///
    /// Olo: one that has become a monitor is no peer, and is sent what
    /// is for it as a monitor is (`Monitor::interested`). busd found no
    /// peer and dropped it: `BecomeMonitor` was never answered, and
    /// dbus-monitor and busctl waited on it for good.
    async fn send_msg_to_unique_name(&self, msg: Message, destination: UniqueName<'_>) -> Result<()> {
        let to = self.peers.read().await.get(destination.as_str()).map(|peer| peer.outbox().clone());
        match to {
            Some(to) => to.send(&msg).context("failed to send message")?,
            None if self.monitors.read().await.contains_key(destination.as_str()) => {}
            None => bail!("no peer for destination `{destination}`"),
        }
        let name_registry = self.name_registry().await;
        self.broadcast_to_monitors(msg, &name_registry).await;

        Ok(())
    }

    async fn broadcast_msg(&self, msg: Message) {
        trace!("Broadcasting message: {:?}", msg);
        let name_registry = self.name_registry().await;
        for peer in self.peers.read().await.values() {
            if !peer.interested(&msg, &name_registry) {
                trace!("Peer {} not interested in {msg:?}", peer.unique_name());
                continue;
            }

            if let Err(e) = peer.outbox().send(&msg).context("failed to send message") {
                debug!("Error sending message: {}", e);
            }
        }

        self.broadcast_to_monitors(msg, &name_registry).await;
    }

    async fn broadcast_to_monitors(&self, msg: Message, name_registry: &NameRegistry) {
        let monitors = self.monitors.read().await;
        if monitors.is_empty() {
            return;
        }
        trace!("Broadcasting message to {} monitors: {:?}", monitors.len(), msg);
        for monitor in monitors.values() {
            if !monitor.interested(&msg, name_registry) {
                trace!("Monitor {} not interested in {msg:?}", monitor.unique_name());
                continue;
            }

            if let Err(e) = monitor.outbox().send(&msg).context("failed to send message") {
                debug!("Error sending message: {}", e);
            }
        }
    }
}
