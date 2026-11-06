//! Upgrading the pool without cutting anyone off. When a new olo-pool is
//! installed, the app starts a copy of the new version beside the running
//! one, handing it the listening sockets (`for_a_copy`): the copy answers
//! every new request at once while this one, retired (`retire`), stops
//! accepting, lets the requests under way run to their end (streams too),
//! closes its connections as they fall idle, and then goes (olo-pool's
//! `ui::hand_over`). Both alive at once, sign-ins are renewed under the
//! store's lock and read again first (`overlapping`). If no copy can be
//! started, the old way: new connections wait in the kernel's queue (as
//! if the pool were only slow to answer) until nothing is under way, then
//! the process becomes the new version (the kit re-executes it) with the
//! sockets handed over (`hand_over`). A tunnel (a host the pool doesn't
//! look into) counts as under way while bytes cross it. Waiting has a
//! limit (`MAX_DRAIN`; `MAX_RETIRED` for a retired copy).

use olo_sync::Mutex;
use std::{
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// The environment variable the listening sockets are handed over in:
/// `PORT=FD`, comma separated.
pub const LISTENERS_ENV: &str = "OLO_POOL_LISTENERS";

/// A tunnel with nothing crossing it for this long isn't under way.
const TUNNEL_IDLE: Duration = Duration::from_secs(10);
/// The longest an upgrade waits for what is under way.
const MAX_DRAIN: Duration = Duration::from_secs(10 * 60);
/// The longest a copy that handed over finishes what it has.
const MAX_RETIRED: Duration = Duration::from_secs(20 * 60);

pub struct Drain {
    draining: AtomicBool,
    began: Mutex<Option<Instant>>,
    clock: Instant,
    requests: AtomicUsize,
    tunnels: AtomicUsize,
    /// When a byte last crossed a tunnel (ms after `clock`).
    tunnel_byte: AtomicU64,
    stop: tokio::sync::watch::Sender<bool>,
    /// Copies of the listening sockets, by port, to hand over.
    listeners: Mutex<Vec<(u16, OwnedFd)>>,
    /// This copy handed over to a new one (`retire`): it only finishes
    /// what it has, saving nothing but a sign-in it had to renew.
    retired: AtomicBool,
    /// The copy this one took over from, while it may still be finishing
    /// (pid and start time: `PREDECESSOR_ENV`).
    predecessor: Mutex<Option<(i32, u64)>>,
}

impl Default for Drain {
    fn default() -> Drain {
        Drain {
            draining: AtomicBool::new(false),
            began: Mutex::new(None),
            clock: Instant::now(),
            requests: AtomicUsize::new(0),
            tunnels: AtomicUsize::new(0),
            tunnel_byte: AtomicU64::new(0),
            stop: tokio::sync::watch::channel(false).0,
            listeners: Mutex::new(Vec::new()),
            retired: AtomicBool::new(false),
            predecessor: Mutex::new(predecessor_from_env()),
        }
    }
}

/// The environment variable naming the copy a new one took over from (its
/// pid): it may still be finishing what it had.
pub const PREDECESSOR_ENV: &str = "OLO_POOL_PREDECESSOR";

/// When process `pid` started (clock ticks after boot), to know it again.
fn started(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The predecessor named in the environment (taken out of it, so the
/// programs this one starts don't inherit it), if it's still running.
fn predecessor_from_env() -> Option<(i32, u64)> {
    let pid: i32 = std::env::var(PREDECESSOR_ENV).ok()?.parse().ok()?;
    // SAFETY: read once, as the gateway is made at start-up, before the
    // threads that read the environment.
    unsafe { std::env::remove_var(PREDECESSOR_ENV) };
    Some((pid, started(pid)?))
}

impl Drain {
    /// Stop taking connections and close the open ones as they fall
    /// idle: an upgrade is waiting.
    pub fn begin(&self) {
        if self.draining.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.began.lock().unwrap() = Some(Instant::now());
        self.stop.send_replace(true);
        tracing::info!("pool: an upgrade is waiting; finishing {} requests and {} tunnels first", self.requests.load(Ordering::SeqCst), self.tunnels.load(Ordering::SeqCst));
    }

    pub fn draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Handed over to a new copy (started with the listening sockets): take
    /// no more connections (the new copy takes them at once), finish what
    /// is under way, and save nothing but a sign-in renewed meanwhile.
    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.begin();
    }

    pub fn retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Whether another copy of the pool may be at work beside this one: the
    /// one it handed over to (retired), or the one it took over from, still
    /// finishing. Sign-ins are then renewed under the store's lock and read
    /// again first, and saves keep the other's renewed ones.
    pub fn overlapping(&self) -> bool {
        if self.retired() {
            return true;
        }
        let mut p = self.predecessor.lock().unwrap();
        if let Some((pid, start)) = *p
            && started(pid) != Some(start)
        {
            *p = None;
        }
        p.is_some()
    }

    /// The listening sockets, made inheritable, as `LISTENERS_ENV` names
    /// them, for a new copy started now; [`Drain::sealed`] makes them
    /// close-on-exec again once it has.
    pub fn for_a_copy(&self) -> Option<String> {
        let kept = self.listeners.lock().unwrap();
        let mut named = Vec::new();
        for (port, fd) in kept.iter() {
            // SAFETY: fcntl on a descriptor we own.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, 0) } == 0 {
                named.push(format!("{port}={}", fd.as_raw_fd()));
            }
        }
        (!named.is_empty()).then(|| named.join(","))
    }

    /// The listening sockets close-on-exec again (after [`Drain::for_a_copy`]).
    pub fn sealed(&self) {
        for (_, fd) in self.listeners.lock().unwrap().iter() {
            // SAFETY: fcntl on a descriptor we own.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
    }

    /// The upgrade didn't happen (the new version wouldn't start): take
    /// connections again, as before.
    pub fn end(&self) {
        if !self.draining.swap(false, Ordering::SeqCst) {
            return;
        }
        *self.began.lock().unwrap() = None;
        for (_, fd) in self.listeners.lock().unwrap().iter() {
            // SAFETY: fcntl on a descriptor we own.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        // SAFETY: as in `hand_over`.
        unsafe { std::env::remove_var(LISTENERS_ENV) };
        self.stop.send_replace(false);
        tracing::warn!("pool: the upgrade didn't come; taking connections again");
    }

    /// Draining for longer than an upgrade can take: it isn't coming.
    pub fn overdue(&self) -> bool {
        self.began.lock().unwrap().is_some_and(|t| t.elapsed() > MAX_DRAIN + Duration::from_secs(60))
    }

    /// Another listener on the socket kept for `port` (to take
    /// connections again after `end`).
    pub fn listener(&self, port: u16) -> Option<std::net::TcpListener> {
        let kept = self.listeners.lock().unwrap();
        let fd = kept.iter().find(|(p, _)| *p == port)?.1.try_clone().ok()?;
        let l = std::net::TcpListener::from(fd);
        l.set_nonblocking(true).ok()?;
        Some(l)
    }

    /// Follows `begin`: true once draining.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<bool> {
        self.stop.subscribe()
    }

    /// Requests under way, and whether a tunnel is.
    pub fn under_way(&self) -> (usize, bool) {
        let since = self.clock.elapsed().as_millis() as u64 - self.tunnel_byte.load(Ordering::Relaxed).min(self.clock.elapsed().as_millis() as u64);
        let tunnel = self.tunnels.load(Ordering::SeqCst) > 0 && since < TUNNEL_IDLE.as_millis() as u64;
        (self.requests.load(Ordering::SeqCst), tunnel)
    }

    /// Whether becoming the new version would cut something short (and
    /// the wait isn't over).
    pub fn busy(&self) -> bool {
        // (Handed over, nothing waits for it: longer, past the provider's
        // own read timeout, so only a request that hung is cut.)
        let limit = if self.retired() { MAX_RETIRED } else { MAX_DRAIN };
        if self.began.lock().unwrap().is_some_and(|t| t.elapsed() > limit) {
            return false;
        }
        let (requests, tunnel) = self.under_way();
        requests > 0 || tunnel
    }

    pub fn request_began(&self) {
        self.requests.fetch_add(1, Ordering::SeqCst);
    }

    /// One ended; true if it was the last while draining.
    pub fn request_ended(&self) -> bool {
        self.requests.fetch_sub(1, Ordering::SeqCst) == 1 && self.draining()
    }

    pub fn tunnel_opened(&self) {
        self.tunnels.fetch_add(1, Ordering::SeqCst);
        self.tunnel_crossed();
    }

    pub fn tunnel_closed(&self) {
        self.tunnels.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn tunnel_crossed(&self) {
        self.tunnel_byte.store(self.clock.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Keep a copy of `listener` (on `port`) to hand over.
    pub fn keep(&self, port: u16, listener: &impl AsFd) {
        match listener.as_fd().try_clone_to_owned() {
            Ok(fd) => self.listeners.lock().unwrap().push((port, fd)),
            Err(e) => tracing::warn!("pool: can't keep the listener on {port} to hand over: {e}"),
        }
    }

    /// Ready the listening sockets for the new version: inherited across
    /// its exec, named in `LISTENERS_ENV`. Only right before it.
    pub fn hand_over(&self) {
        let kept = self.listeners.lock().unwrap();
        let mut named = Vec::new();
        for (port, fd) in kept.iter() {
            // SAFETY: fcntl on a descriptor we own.
            let ok = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, 0) } == 0;
            if ok {
                named.push(format!("{port}={}", fd.as_raw_fd()));
            }
        }
        if !named.is_empty() {
            // SAFETY: set just before the exec, with nothing else reading
            // the environment by then.
            unsafe { std::env::set_var(LISTENERS_ENV, named.join(",")) };
        }
    }

    pub fn status(&self) -> Value {
        let (requests, tunnel) = self.under_way();
        json!({ "draining": self.draining(), "retired": self.retired(), "requests": requests, "tunnel": tunnel })
    }
}

/// A listening socket on loopback `port`: the one the version before
/// handed over, if it did, else a new one. (0: any port.)
pub fn listen(port: u16) -> std::io::Result<std::net::TcpListener> {
    if port != 0
        && let Some(fd) = handed_over(port)
    {
        // SAFETY: the descriptor was handed to us for this port, and is
        // taken only once (`handed_over` forgets it).
        let l = unsafe { std::net::TcpListener::from_raw_fd(fd) };
        // Its own children mustn't hold it.
        // SAFETY: fcntl on a descriptor we own.
        unsafe { libc::fcntl(l.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        if l.local_addr().is_ok_and(|a| a.port() == port) {
            l.set_nonblocking(true)?;
            tracing::info!("pool: took over the listener on {port}");
            return Ok(l);
        }
    }
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

/// The descriptor handed over for `port`, taken out of the environment.
fn handed_over(port: u16) -> Option<RawFd> {
    let all = std::env::var(LISTENERS_ENV).ok()?;
    let mut rest = Vec::new();
    let mut found = None;
    for part in all.split(',') {
        match part.split_once('=').and_then(|(p, f)| Some((p.parse::<u16>().ok()?, f.parse::<RawFd>().ok()?))) {
            Some((p, fd)) if p == port && found.is_none() => found = Some(fd),
            _ => rest.push(part),
        }
    }
    // SAFETY: the environment is read and set at start, before any
    // other thread looks at it.
    unsafe {
        if rest.is_empty() {
            std::env::remove_var(LISTENERS_ENV);
        } else {
            std::env::set_var(LISTENERS_ENV, rest.join(","));
        }
    }
    found
}

/// A request under way, until dropped (its answer sent, or the client
/// gone).
pub struct Underway {
    drain: Arc<crate::gateway::Shared>,
}

impl Underway {
    pub fn new(shared: &Arc<crate::gateway::Shared>) -> Underway {
        shared.drain.request_began();
        Underway { drain: shared.clone() }
    }
}

impl Drop for Underway {
    fn drop(&mut self) {
        if self.drain.drain.request_ended() {
            // The last: the app looks again whether to upgrade.
            self.drain.nudge();
        }
    }
}

/// An answer's body, keeping its request under way until it has gone.
pub struct Counted<B> {
    pub inner: B,
    pub underway: Underway,
}

impl<B: http_body::Body + Unpin> http_body::Body for Counted<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// A tunnel's side, noting each byte that crosses it.
pub struct Crossing<'a, S> {
    pub inner: S,
    pub drain: &'a Drain,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Crossing<'_, S> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.drain.tunnel_crossed();
        }
        r
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Crossing<'_, S> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Copy both ways between a client and the host, counted as a tunnel.
pub async fn tunnel<A, B>(drain: &Drain, client: A, host: B)
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    drain.tunnel_opened();
    let (mut a, mut b) = (Crossing { inner: client, drain }, Crossing { inner: host, drain });
    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
    drain.tunnel_closed();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_while_something_is_under_way() {
        let d = Drain::default();
        assert!(!d.busy());
        d.request_began();
        assert!(d.busy());
        assert!(!d.request_ended(), "not draining: nothing to say");
        d.request_began();
        d.begin();
        assert!(d.request_ended(), "the last while draining");
        assert!(!d.busy());
        // A tunnel is under way while bytes cross it.
        d.tunnel_opened();
        assert!(d.busy());
        d.tunnel_closed();
        assert!(!d.busy());
    }

    #[test]
    fn listeners_handed_over_by_port() {
        // SAFETY: this test alone uses the variable.
        unsafe { std::env::set_var(LISTENERS_ENV, "17891=7,17892=8") };
        assert_eq!(handed_over(17892), Some(8));
        assert_eq!(std::env::var(LISTENERS_ENV).unwrap(), "17891=7");
        assert_eq!(handed_over(17892), None);
        assert_eq!(handed_over(17891), Some(7));
        assert!(std::env::var(LISTENERS_ENV).is_err());
    }
}
