//! What is on its way to one peer.
//!
//! Olo: busd wrote each message to its recipient's socket from the task
//! that routes them, and waited for it. A program that had stopped
//! reading (stopped, hung, slow) with its socket full held that write,
//! and the route with it: a signal to everyone stopped at it, and since
//! the bus's own answers are routed by one task, the first signal the
//! bus itself sent (a name's new owner, at every connect) stopped the
//! bus answering anybody. Each peer has a queue of its own now and a
//! task that writes it out in order: routing never waits for a
//! recipient. As dbus-daemon and dbus-broker do, and as they do, a
//! recipient too far behind is let go ([`LIMIT`]).

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::Relaxed},
};

use anyhow::{Result, bail};
use tracing::{debug, warn};
use zbus::{Connection, Message};

/// How much may wait for one peer: past it, the peer is disconnected
/// (dbus-daemon's session buses allow far more, 1 GB; a screen's bus
/// carries little, and this is memory held for a program that isn't
/// reading).
pub const LIMIT: usize = 64 << 20;

/// A peer's queue: cloned freely, its task ends with the last of them.
#[derive(Clone, Debug)]
pub struct Outbox {
    queue: smol::channel::Sender<Message>,
    waiting: Arc<AtomicUsize>,
    conn: Connection,
}

impl Outbox {
    pub fn new(conn: Connection) -> Self {
        let (queue, next) = smol::channel::unbounded::<Message>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let (to, behind) = (conn.clone(), waiting.clone());
        crate::spawn(async move {
            while let Ok(msg) = next.recv().await {
                let sent = to.send(&msg).await;
                behind.fetch_sub(size(&msg), Relaxed);
                if let Err(e) = sent {
                    // (Gone: its stream ends too, and the peer with it.)
                    debug!("a message wasn't sent: {e}");
                    break;
                }
            }
        });
        Self { queue, waiting, conn }
    }

    /// `msg` after what already waits for it. Never waits itself; an
    /// error if the peer has gone, or is let go for being [`LIMIT`]
    /// behind.
    pub fn send(&self, msg: &Message) -> Result<()> {
        let size = size(msg);
        if self.waiting.fetch_add(size, Relaxed) + size > LIMIT {
            self.waiting.fetch_sub(size, Relaxed);
            if self.queue.close() {
                warn!("{:?} reads nothing and has {} MB waiting for it: disconnected", self.conn.unique_name(), LIMIT >> 20);
                let conn = self.conn.clone();
                crate::spawn(async move { conn.close().await });
            }
            bail!("the peer is too far behind");
        }
        if self.queue.try_send(msg.clone()).is_err() {
            self.waiting.fetch_sub(size, Relaxed);
            bail!("the peer has gone");
        }
        Ok(())
    }
}

fn size(msg: &Message) -> usize {
    msg.data().bytes().len()
}
