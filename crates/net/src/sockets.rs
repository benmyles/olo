//! This process's own TCP connections, whoever made them (ureq's, hyper's,
//! grammers'): found among its descriptors, each with its two ends, and
//! acted on in place. A cut one (`shutdown`, never `close`: the descriptor
//! stays its owner's) fails at its next read or write, at once if one is
//! waiting, so its owner sees a broken connection and makes another; one
//! watched closely has the kernel give up on it by itself if its peer
//! stops answering (keepalive, and `TCP_USER_TIMEOUT`).

use std::{
    mem::MaybeUninit,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

/// One connection: its descriptor and its two ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tcp {
    pub fd: i32,
    pub local: SocketAddr,
    pub peer: SocketAddr,
}

/// How long a connection watched closely may go unanswered: data sent and
/// not acknowledged for this long, or this long without a keepalive
/// answered, and the kernel ends it.
pub const PATIENCE_MS: u32 = 15_000;
/// Keepalive on it: first probe after this much quiet, then every
/// `PROBE_EVERY` s.
const QUIET: i32 = 5;
const PROBE_EVERY: i32 = 3;
const PROBES: i32 = 3;

fn opt(fd: i32, level: i32, name: i32) -> Option<i32> {
    let mut v: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: an int option into our int, its size given.
    (unsafe { libc::getsockopt(fd, level, name, (&mut v as *mut libc::c_int).cast(), &mut len) } == 0).then_some(v)
}

fn set(fd: i32, level: i32, name: i32, v: i32) -> bool {
    // SAFETY: an int option from our int, its size given.
    unsafe { libc::setsockopt(fd, level, name, (&v as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as u32) == 0 }
}

fn end(fd: i32, peer: bool) -> Option<SocketAddr> {
    let mut ss = MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: into our storage, its size given.
    let r = unsafe { if peer { libc::getpeername(fd, ss.as_mut_ptr().cast(), &mut len) } else { libc::getsockname(fd, ss.as_mut_ptr().cast(), &mut len) } };
    if r != 0 {
        return None;
    }
    // SAFETY: zeroed, then filled in by the kernel.
    let ss = unsafe { ss.assume_init() };
    match ss.ss_family as i32 {
        libc::AF_INET => {
            // SAFETY: the family says it's a sockaddr_in.
            let a: libc::sockaddr_in = unsafe { std::ptr::read((&ss as *const libc::sockaddr_storage).cast()) };
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr))), u16::from_be(a.sin_port)))
        }
        libc::AF_INET6 => {
            // SAFETY: the family says it's a sockaddr_in6.
            let a: libc::sockaddr_in6 = unsafe { std::ptr::read((&ss as *const libc::sockaddr_storage).cast()) };
            let ip = Ipv6Addr::from(a.sin6_addr.s6_addr);
            // (An IPv4 address on an IPv6 socket is the IPv4 one.)
            Some(SocketAddr::new(ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4), u16::from_be(a.sin6_port)))
        }
        _ => None,
    }
}

/// The descriptor's connection, if it is a TCP one that's connected
/// (not listening, not on loopback).
pub fn tcp(fd: i32) -> Option<Tcp> {
    connection(fd).filter(|t| !t.local.ip().is_loopback())
}

/// The descriptor's connection, if it is a TCP one that's connected (not
/// listening), loopback's too.
fn connection(fd: i32) -> Option<Tcp> {
    let domain = opt(fd, libc::SOL_SOCKET, libc::SO_DOMAIN)?;
    if domain != libc::AF_INET && domain != libc::AF_INET6 {
        return None;
    }
    if opt(fd, libc::SOL_SOCKET, libc::SO_TYPE)? != libc::SOCK_STREAM || opt(fd, libc::SOL_SOCKET, libc::SO_PROTOCOL)? != libc::IPPROTO_TCP {
        return None;
    }
    if opt(fd, libc::SOL_SOCKET, libc::SO_ACCEPTCONN)? != 0 {
        return None;
    }
    let local = end(fd, false)?;
    let peer = end(fd, true)?;
    if local.ip().is_unspecified() {
        return None;
    }
    Some(Tcp { fd, local, peer })
}

/// This process's TCP connections ([`tcp`]; loopback's too if `loopback`).
pub fn all(loopback: bool) -> Vec<Tcp> {
    let Ok(dir) = std::fs::read_dir("/proc/self/fd") else { return Vec::new() };
    dir.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok()).filter_map(if loopback { connection } else { tcp }).collect()
}

/// It ends now: whoever is reading or writing it is told so at once.
/// (Only if it is still the connection found: a descriptor closed and
/// made again since is someone else's.)
pub fn cut(t: &Tcp) -> bool {
    if connection(t.fd) != Some(*t) {
        return false;
    }
    // SAFETY: shutdown(2) on a socket descriptor; it stays open.
    unsafe { libc::shutdown(t.fd, libc::SHUT_RDWR) == 0 }
}

/// The kernel ends it by itself if its peer stops answering for
/// [`PATIENCE_MS`]; while it answers, nothing changes.
pub fn watch_closely(t: &Tcp) -> bool {
    if connection(t.fd) != Some(*t) {
        return false;
    }
    set(t.fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1)
        & set(t.fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, QUIET)
        & set(t.fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, PROBE_EVERY)
        & set(t.fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT, PROBES)
        & set(t.fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT, PATIENCE_MS as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        os::fd::AsRawFd,
    };

    /// Loopback connections aren't ours to cut; listeners never are.
    #[test]
    fn loopback_and_listeners_are_left_alone() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        assert_eq!(tcp(l.as_raw_fd()), None);
        assert_eq!(tcp(c.as_raw_fd()), None);
        assert!(!all(false).iter().any(|t| t.fd == c.as_raw_fd()));
        assert!(all(true).iter().any(|t| t.fd == c.as_raw_fd()));
    }

    /// Cut, a connection waiting to read is told at once.
    #[test]
    fn a_cut_connection_wakes_its_reader() {
        // (A non-loopback address of this machine's, if it has one: a
        // connection to itself on it.)
        let nl = crate::netlink::Netlink::open(0).unwrap();
        let Some(ip) = nl.addresses().unwrap().into_iter().map(|(_, a)| a).find(|a| a.is_ipv4() && !a.is_loopback()) else { return };
        let l = TcpListener::bind((ip, 0)).unwrap();
        let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (_s, _) = l.accept().unwrap();
        let t = tcp(c.as_raw_fd()).expect("a connection on a non-loopback address");
        assert_eq!(t.local.ip(), ip);
        assert!(watch_closely(&t));
        assert_eq!(opt(t.fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT), Some(PATIENCE_MS as i32));
        let fd = t.fd;
        let reader = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut b = [0u8; 1];
            let r = c.read(&mut b);
            (r.map_err(|e| e.kind()), started.elapsed())
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(cut(&Tcp { fd, ..t }));
        let (r, took) = reader.join().unwrap();
        assert_eq!(r, Ok(0), "the reader sees its end");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
}
