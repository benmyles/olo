//! The network's changes, as a program that talks over it needs them (Ben,
//! 2026-11-09: "make sure that if the active network changes mid backup
//! that diskbackup will seamlessly start using the new network and not
//! hang or error. same for the token pool app and any other olo apps that
//! use network").
//!
//! A connection lives on the network it was made on: its address is that
//! network's. When the way out changes (a cable pulled, Wi-Fi joined or
//! left, a VPN up), a connection whose address has gone is dead, and
//! nothing says so: whoever reads or writes it waits for TCP to give up,
//! a quarter of an hour. One whose address is still there may or may not
//! still work (its packets go out another way now).
//!
//! [`watch`] (which everything here starts, once a process) has the kernel
//! tell it of each change (rtnetlink: a thread asleep until there's news,
//! never polling), and then, in this order:
//! - the [`generation`] moves on, and [`online`] says whether there's a
//!   way out now, so what's kept for the network ([`PerNetwork`]: an HTTP
//!   client and its idle connections) is made again and the next request
//!   goes out the new way, and a request that failed meanwhile knows why
//!   ([`across_changes`]);
//! - this process's connections whose address went are cut (`shutdown`:
//!   whoever waits on one is told at once, and makes another, on the
//!   network there is now);
//! - those whose way out changed are watched closely: the kernel ends one
//!   by itself if its peer stops answering for 15 s, and leaves it be
//!   while it answers (keepalive, `TCP_USER_TIMEOUT`);
//! - whoever waits for a network ([`wait_online`]) looks again, and the
//!   hooks ([`on_change`]) run.
//!
//! Loopback connections, listeners and other processes' sockets are never
//! touched. With the `ureq` feature, `http::Download` is a download that
//! goes on where it broke off.

mod netlink;
mod sockets;

#[cfg(feature = "ureq")]
pub mod http;

use olo_sync::{Condvar, Mutex};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        Arc, LazyLock, Once,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

use netlink::{Link, Netlink, Way};

struct Net {
    generation: AtomicU64,
    online: AtomicBool,
    waiting: AtomicUsize,
    /// Moved on by every change and [`nudge`]: what waiters wait on.
    wake: Mutex<u64>,
    cond: Condvar,
    hooks: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

static NET: LazyLock<Net> = LazyLock::new(|| Net {
    generation: AtomicU64::new(0),
    // (Until known, or if it can't be: there is one, so nothing waits.)
    online: AtomicBool::new(true),
    waiting: AtomicUsize::new(0),
    wake: Mutex::new(0),
    cond: Condvar::new(),
    hooks: Mutex::new(Vec::new()),
});

static START: Once = Once::new();

/// Stand-ins for the internet: the kernel is asked which way a packet to
/// each would go (nothing is ever sent to them).
const INTERNET: [IpAddr; 2] = [IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111))];

/// How long after a change its burst is taken to go on (an address comes
/// with its routes, a link going down takes its addresses with it).
const SETTLE_MS: i32 = 150;

/// The network as the kernel has it.
#[derive(Debug, Clone, PartialEq, Default)]
struct Snapshot {
    links: BTreeMap<u32, Link>,
    /// Every address but loopback's, and its link.
    addrs: BTreeSet<(u32, IpAddr)>,
    /// Which way IPv4's and IPv6's packets to the internet go.
    ways: [Option<Way>; 2],
}

impl Snapshot {
    fn read(nl: &Netlink) -> Snapshot {
        Snapshot { links: nl.links().unwrap_or_default(), addrs: nl.addresses().unwrap_or_default().into_iter().filter(|(_, a)| !a.is_loopback()).collect(), ways: INTERNET.map(|to| nl.way_to(to)) }
    }

    fn online(&self) -> bool {
        self.ways.iter().flatten().any(|w| self.links.get(&w.link).is_some_and(|l| l.running))
    }

    /// What a change is told by: the addresses on links that are up, and
    /// the ways out.
    fn key(&self) -> (BTreeSet<(u32, IpAddr)>, [Option<Way>; 2]) {
        (self.addrs.iter().filter(|(i, _)| self.links.get(i).is_some_and(|l| l.running)).copied().collect(), self.ways)
    }

    /// The ways out, by their links' names (for the log: no addresses).
    fn describe(&self) -> String {
        let mut names: Vec<&str> = self.ways.iter().flatten().filter_map(|w| self.links.get(&w.link)).filter(|l| l.running).map(|l| l.name.as_str()).collect();
        names.dedup();
        if names.is_empty() { "no way out".into() } else { format!("out through {}", names.join(" and ")) }
    }
}

/// Follow the network's changes, from now on (once a process; everything
/// here starts it). Nothing if the kernel's news can't be had (then the
/// network is taken to be there, and never to change).
pub fn watch() {
    START.call_once(|| {
        let (events, ask) = match (Netlink::open(netlink::GROUPS), Netlink::open(0)) {
            (Ok(e), Ok(a)) => (e, a),
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!("network: changes can't be followed ({e})");
                return;
            }
        };
        let was = Snapshot::read(&ask);
        NET.online.store(was.online(), SeqCst);
        tracing::debug!("network: {}", was.describe());
        if let Err(e) = std::thread::Builder::new().name("olo-net".into()).spawn(move || run(events, ask, was)) {
            tracing::warn!("network: changes can't be followed ({e})");
        }
    });
}

fn run(events: Netlink, ask: Netlink, mut was: Snapshot) {
    loop {
        // The news, then the rest of its burst.
        let mut gone: BTreeSet<IpAddr> = BTreeSet::new();
        let mut wait = true;
        loop {
            match events.recv(wait) {
                Ok(news) => {
                    for (kind, _, m) in news {
                        if kind == netlink::RTM_DELADDR
                            && let Some((_, a)) = netlink::address(&m)
                        {
                            gone.insert(a);
                        }
                    }
                }
                // Some news lost: what's there now tells.
                Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => {}
                Err(e) => {
                    tracing::warn!("network: the kernel's news can't be read ({e}): changes aren't followed from now");
                    return;
                }
            }
            wait = false;
            if !events.readable(SETTLE_MS) {
                break;
            }
        }
        let started = Instant::now();
        let now = Snapshot::read(&ask);
        // (An address that went with its news lost.)
        gone.extend(was.addrs.iter().filter(|x| !now.addrs.contains(x)).map(|(_, a)| *a));
        gone.retain(|a| !a.is_loopback());
        let moved = now.key() != was.key();
        if !moved && gone.is_empty() {
            was = now;
            continue;
        }
        // First what's known now, so whoever's request fails next knows
        // the network changed under it.
        let generation = NET.generation.fetch_add(1, SeqCst) + 1;
        NET.online.store(now.online(), SeqCst);
        let (mut cut, mut watched) = (0, 0);
        for t in sockets::all(false) {
            if gone.contains(&t.local.ip()) {
                cut += usize::from(sockets::cut(&t));
            } else if moved && ask.way_to(t.peer.ip()).and_then(|w| w.from) != Some(t.local.ip()) {
                watched += usize::from(sockets::watch_closely(&t));
            }
        }
        let us = started.elapsed().as_micros() as u64;
        tracing::info!(generation, cut, watched, us, "network: changed, {}", now.describe());
        was = now;
        told();
    }
}

/// Waiters woken, and the hooks run: the network changed.
fn told() {
    {
        let mut w = NET.wake.lock().unwrap();
        *w += 1;
        NET.cond.notify_all();
    }
    let hooks = NET.hooks.lock().unwrap().clone();
    for h in hooks {
        h();
    }
}

/// For tests of what uses this: as if the network had changed, as the
/// watcher would have it (the generation on, a way out or not as `online`
/// says, waiters woken, hooks run), this process's connections to `port`
/// (loopback's too) cut as one on an address that went would be. Returns
/// how many were.
#[doc(hidden)]
pub fn pretend_change(online: bool, port: Option<u16>) -> usize {
    watch();
    NET.generation.fetch_add(1, SeqCst);
    NET.online.store(online, SeqCst);
    let mut cut = 0;
    if let Some(port) = port {
        for t in sockets::all(true).iter().filter(|t| t.peer.port() == port) {
            cut += usize::from(sockets::cut(t));
        }
    }
    told();
    cut
}

/// A number that moves on each time the network changes (its addresses,
/// or the way out).
pub fn generation() -> u64 {
    watch();
    NET.generation.load(SeqCst)
}

/// Whether there's a way out to the internet now (a route, through a link
/// that's up).
pub fn online() -> bool {
    watch();
    NET.online.load(SeqCst)
}

/// How many are waiting for a network now ([`wait_online`]): for saying
/// so ("Waiting for the network").
pub fn waiting() -> usize {
    NET.waiting.load(SeqCst)
}

/// `f`, on the watcher's thread, after each change (quick: nothing that
/// waits).
pub fn on_change(f: impl Fn() + Send + Sync + 'static) {
    watch();
    NET.hooks.lock().unwrap().push(Arc::new(f));
}

/// Wake whoever waits here ([`wait_online`], [`rest`]) to look at their
/// `stop` again: call it after asking them to stop.
pub fn nudge() {
    let mut w = NET.wake.lock().unwrap();
    *w += 1;
    NET.cond.notify_all();
}

/// Until there's a way out (true), `stop` says so or `until` passes
/// (false). Woken by each change and [`nudge`], never polling.
pub fn wait_online(until: Option<Instant>, stop: &dyn Fn() -> bool) -> bool {
    watch();
    if NET.online.load(SeqCst) {
        return true;
    }
    NET.waiting.fetch_add(1, SeqCst);
    let mut w = NET.wake.lock().unwrap();
    let r = loop {
        if NET.online.load(SeqCst) {
            break true;
        }
        if stop() {
            break false;
        }
        let left = match until {
            Some(u) => match u.checked_duration_since(Instant::now()) {
                Some(l) if !l.is_zero() => l,
                _ => break false,
            },
            None => Duration::from_secs(3600),
        };
        w = NET.cond.wait_timeout(w, left).unwrap().0;
    };
    drop(w);
    NET.waiting.fetch_sub(1, SeqCst);
    r
}

/// A pause of `d`, cut short by a change or [`nudge`]: true if the network
/// changed meanwhile.
pub fn rest(d: Duration) -> bool {
    let seen = generation();
    let until = Instant::now() + d;
    let mut w = NET.wake.lock().unwrap();
    let woken = *w;
    while generation() == seen && *w == woken {
        let Some(left) = until.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()) else { break };
        w = NET.cond.wait_timeout(w, left).unwrap().0;
    }
    generation() != seen
}

/// Tries [`across_changes`] makes.
pub const TRIES: u32 = 4;

/// `attempt`, again while it fails because of the network: at once when
/// the network changed while it went (on the new one), and when there was
/// none, once one comes (waiting up to `patience`). What else fails it is
/// its own; at most [`TRIES`] tries.
pub fn across_changes<T, E: std::fmt::Display>(patience: Duration, mut attempt: impl FnMut() -> Result<T, E>) -> Result<T, E> {
    watch();
    let mut tries = 0;
    loop {
        let seen = generation();
        let e = match attempt() {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        tries += 1;
        if tries >= TRIES {
            return Err(e);
        }
        if generation() != seen {
            tracing::info!("network: changed under a request ({e}): again, the new way");
            continue;
        }
        if !online() && wait_online(Some(Instant::now() + patience), &|| false) {
            tracing::info!("network: back after a request failed ({e}): again");
            continue;
        }
        return Err(e);
    }
}

/// Something kept for the network it was made on (an HTTP client, whose
/// idle connections are that network's): made again, by `make`, the first
/// time it's wanted after the network changes. What's under way with the
/// old one goes on with it.
pub struct PerNetwork<T> {
    make: Box<dyn Fn() -> T + Send + Sync>,
    made: Mutex<Option<(u64, T)>>,
}

impl<T: Clone> PerNetwork<T> {
    pub fn new(make: impl Fn() -> T + Send + Sync + 'static) -> PerNetwork<T> {
        watch();
        PerNetwork { make: Box::new(make), made: Mutex::new(None) }
    }

    /// The one for the network there is now.
    pub fn get(&self) -> T {
        let g = generation();
        let mut m = self.made.lock().unwrap();
        if let Some((at, v)) = m.as_ref()
            && *at == g
        {
            return v.clone();
        }
        let v = (self.make)();
        *m = Some((g, v.clone()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_once_a_network() {
        let n = Arc::new(AtomicUsize::new(0));
        let counted = n.clone();
        let p = PerNetwork::new(move || counted.fetch_add(1, SeqCst));
        assert_eq!(p.get(), 0);
        assert_eq!(p.get(), 0);
        // (A change: as the watcher does.)
        NET.generation.fetch_add(1, SeqCst);
        assert_eq!(p.get(), 1);
        assert_eq!(n.load(SeqCst), 2);
    }

    #[test]
    fn a_failure_that_isnt_the_networks_is_its_own() {
        let mut tries = 0;
        let seen = generation();
        let r: Result<(), String> = across_changes(Duration::ZERO, || {
            tries += 1;
            Err("404".into())
        });
        assert!(r.is_err());
        // (Online here, and the network unchanged: no second try.)
        if online() && generation() == seen {
            assert_eq!(tries, 1);
        }
    }

    #[test]
    fn a_nudge_wakes_a_waiter() {
        let seen = generation();
        let t = Instant::now();
        let changed = rest(Duration::ZERO);
        // The watcher and other tests can change the process-wide network
        // generation even during a zero-length wait.
        assert!(!changed || generation() != seen);
        assert!(t.elapsed() < Duration::from_millis(50));
        let h = std::thread::spawn(|| {
            let t = Instant::now();
            rest(Duration::from_secs(10));
            t.elapsed()
        });
        std::thread::sleep(Duration::from_millis(100));
        nudge();
        assert!(h.join().unwrap() < Duration::from_secs(5));
    }
}
