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
    /// The copy this one took over from, while it's still finishing.
    predecessor: Mutex<Option<Predecessor>>,
    /// Retired: the end of the pipe this copy keeps until it has finished
    /// (`finishing`), which is how the new copy knows it's still at work.
    finishing: Mutex<Option<OwnedFd>>,
}

/// The copy a new one took over from.
enum Predecessor {
    /// The pipe it holds the other end of until it has finished
    /// ([`Drain::finishing`]).
    Pipe(OwnedFd),
    /// A copy from before the pipe (it named its pid: `PREDECESSOR_ENV`):
    /// its pid, when that started, and when this copy did. It finishes
    /// within `MAX_RETIRED`; its pid may stay for good after (the keeper).
    Pid(i32, u64, Instant),
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
            finishing: Mutex::new(None),
        }
    }
}

/// The environment variable naming, to a new copy, the descriptor by which
/// it knows the copy it took over from is still finishing: the read end
/// of a pipe ([`Drain::finishing`]).
pub const FINISHING_ENV: &str = "OLO_POOL_FINISHING";

/// The environment variable by which copies from before `FINISHING_ENV`
/// named themselves (their pid) to the copy they handed over to.
pub const PREDECESSOR_ENV: &str = "OLO_POOL_PREDECESSOR";

/// When process `pid` started (clock ticks after boot), to know it again.
pub(crate) fn started(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The predecessor named in the environment (taken out of it, so the
/// programs this one starts don't inherit it), if it's still at work.
fn predecessor_from_env() -> Option<Predecessor> {
    let (pipe, pid) = (std::env::var(FINISHING_ENV).ok(), std::env::var(PREDECESSOR_ENV).ok());
    // SAFETY: read once, as the gateway is made at start-up, before the
    // threads that read the environment.
    unsafe {
        std::env::remove_var(FINISHING_ENV);
        std::env::remove_var(PREDECESSOR_ENV);
    }
    if let Some(fd) = pipe.and_then(|p| p.parse::<RawFd>().ok()) {
        // SAFETY: fstat and fcntl on a descriptor number; it's taken as
        // ours only if it is the pipe handed to us (named once, to this
        // process, and forgotten above).
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            if fd > 2 && libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFIFO {
                // (Its own children mustn't hold it.)
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                return Some(Predecessor::Pipe(OwnedFd::from_raw_fd(fd)));
            }
        }
    }
    let pid: i32 = pid?.parse().ok()?;
    Some(Predecessor::Pid(pid, started(pid)?, Instant::now()))
}

/// Whether the other end of pipe `fd` is closed: nobody holds it any more.
fn hung_up(fd: &OwnedFd) -> bool {
    let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: poll on one descriptor we own, not waiting.
    let n = unsafe { libc::poll(&mut p, 1, 0) };
    n > 0 && p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
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

    /// Whether another copy of the pool is at work beside this one: the one
    /// it handed over to (retired), or the one it took over from, until
    /// that has finished. Sign-ins are then renewed under the store's lock
    /// and read again first, and saves keep the other's renewed ones.
    ///
    /// Not for as long as the predecessor's pid lives: the copy olo-session
    /// started stays for good once it has finished, as the keeper, and a
    /// pool that took that for a copy at work ran every request under the
    /// store's lock for the rest of its life (Ben's, 2026-11-18).
    pub fn overlapping(&self) -> bool {
        if self.retired() {
            return true;
        }
        let mut p = self.predecessor.lock().unwrap();
        let still = match &*p {
            None => return false,
            Some(Predecessor::Pipe(fd)) => !hung_up(fd),
            Some(Predecessor::Pid(pid, start, since)) => since.elapsed() < MAX_RETIRED + Duration::from_secs(60) && started(*pid) == Some(*start),
        };
        if !still {
            *p = None;
        }
        still
    }

    /// Retiring: the descriptor to hand the copy started now (inherited by
    /// it, named to it in `FINISHING_ENV`, and let go here once it has
    /// started), by which it knows this copy is still finishing: the read
    /// end of a pipe whose other end is kept here until [`Drain::finished`],
    /// or until this process ends or becomes another program (the keeper).
    pub fn finishing(&self) -> Option<OwnedFd> {
        let mut fds = [0; 2];
        // SAFETY: pipe2 into two ints, each then owned once; fcntl on one.
        let (r, w) = unsafe {
            if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
                return None;
            }
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        // SAFETY: as above. (Its end here never waits: what's told the
        // new copy, `tell`, is dropped rather than wait for it.)
        if unsafe { libc::fcntl(r.as_raw_fd(), libc::F_SETFD, 0) } != 0 || unsafe { libc::fcntl(w.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } != 0 {
            return None;
        }
        *self.finishing.lock().unwrap() = Some(w);
        Some(r)
    }

    /// Retired: tell the copy now at work `line` (a line of JSON, written
    /// whole or not at all: at most a pipe's atomic write), over the pipe
    /// it knows this one is finishing by. Never waits: a copy that doesn't
    /// read it (one from before) misses it.
    pub fn tell(&self, line: &str) -> bool {
        if !self.retired() || line.len() + 1 > libc::PIPE_BUF || line.contains('\n') {
            return false;
        }
        let held = self.finishing.lock().unwrap();
        let Some(w) = held.as_ref() else { return false };
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        // SAFETY: a write of our own bytes on a descriptor we own.
        let n = unsafe { libc::write(w.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        n == bytes.len() as isize
    }

    /// The copy this one took over from, as it tells what finished there
    /// after the handover (`tell`): the pipe to read it from, if it was
    /// handed one.
    pub fn told_by_predecessor(&self) -> Option<OwnedFd> {
        match &*self.predecessor.lock().unwrap() {
            Some(Predecessor::Pipe(fd)) => fd.try_clone().ok(),
            _ => None,
        }
    }

    /// For a test: this copy took over from the one holding `pipe`'s other
    /// end (as `FINISHING_ENV` names it to a copy started for real).
    pub fn took_over_from(&self, pipe: OwnedFd) {
        *self.predecessor.lock().unwrap() = Some(Predecessor::Pipe(pipe));
    }

    /// This copy, retired, has finished (or never handed over after all):
    /// the copy it handed over to has the store to itself.
    pub fn finished(&self) {
        self.finishing.lock().unwrap().take();
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
        let crossed = self.tunnel_byte.load(Ordering::Relaxed);
        let since = (self.clock.elapsed().as_millis() as u64).saturating_sub(crossed);
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

    /// Count an upgraded connection even when its relay is cancelled.
    pub(crate) fn tunnel(&self) -> Tunnel<'_> {
        self.tunnel_opened();
        Tunnel(self)
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
        // (`beside`: another copy is at work beside this one, `overlapping`.)
        json!({ "draining": self.draining(), "retired": self.retired(), "beside": self.overlapping(), "requests": requests, "tunnel": tunnel })
    }
}

/// The copies of Token Pool serving the gateway on `port`, this one aside:
/// this user's olo-pool processes holding its listening socket (the one
/// taking requests, and one that handed over, finishing what it has). The
/// session's copy keeps them when it finds one there already (olo-pool's
/// `keeper`), rather than running beside it with no gateway.
pub fn serving(port: u16) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt as _;
    let me = std::process::id() as i32;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let root = std::path::Path::new("/");
    olo_system::listening::holders(root, port, |pid| {
        pid != me && std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == uid) && olo_system::listening::program(root, pid).as_deref() == Some("olo-pool")
    })
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

pub(crate) struct Tunnel<'a>(&'a Drain);

impl Drop for Tunnel<'_> {
    fn drop(&mut self) {
        self.0.tunnel_closed();
    }
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
