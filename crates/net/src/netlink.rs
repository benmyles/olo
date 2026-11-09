//! rtnetlink, as much as watching the way out needs: the kernel's news of
//! links, addresses and routes as they change (a socket in their groups),
//! and asked outright (dumps, and which way a packet to an address would
//! go now: `RTM_GETROUTE`, what `ip route get` asks).

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

/// The groups watched: links, and IPv4's and IPv6's addresses and routes.
pub const GROUPS: u32 = libc::RTMGRP_LINK as u32 | libc::RTMGRP_IPV4_IFADDR as u32 | libc::RTMGRP_IPV4_ROUTE as u32 | libc::RTMGRP_IPV6_IFADDR as u32 | libc::RTMGRP_IPV6_ROUTE as u32;

const IFLA_IFNAME: u16 = 3;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PREFSRC: u16 = 7;

/// A netlink socket of the routing family, closed when dropped.
pub struct Netlink {
    fd: OwnedFd,
    seq: std::cell::Cell<u32>,
}

/// One message: its type and what follows its header.
pub type Message = (u16, Vec<u8>);

fn align(n: usize) -> usize {
    (n + 3) & !3
}

impl Netlink {
    /// In `groups` (0: none, for asking).
    pub fn open(groups: u32) -> io::Result<Netlink> {
        // SAFETY: plain socket(2).
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_ROUTE) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: just made, ours alone.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: a zeroed sockaddr_nl is valid; ours, its length given.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = groups;
        // SAFETY: as above.
        if unsafe { libc::bind(raw, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if groups != 0 {
            // Room for a burst (a VPN's routes coming up at once); one
            // that overflows it anyway is ENOBUFS, and read whole again.
            let size: libc::c_int = 1 << 20;
            // SAFETY: an int option, its size given.
            unsafe { libc::setsockopt(raw, libc::SOL_SOCKET, libc::SO_RCVBUF, (&size as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as u32) };
        }
        Ok(Netlink { fd, seq: std::cell::Cell::new(1) })
    }

    pub fn raw(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    fn send(&self, kind: u16, flags: u16, body: &[u8]) -> io::Result<u32> {
        let seq = self.seq.get().wrapping_add(1);
        self.seq.set(seq);
        let len = 16 + body.len();
        let mut msg = Vec::with_capacity(len);
        msg.extend_from_slice(&(len as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(&(libc::NLM_F_REQUEST as u16 | flags).to_ne_bytes());
        msg.extend_from_slice(&seq.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(body);
        // SAFETY: a zeroed sockaddr_nl is the kernel's address.
        let mut to: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        to.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: the buffer and address are ours, their lengths given.
        let sent = unsafe { libc::sendto(self.raw(), msg.as_ptr().cast(), msg.len(), 0, (&to as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(seq)
    }

    /// The messages of one datagram (waiting for one if `wait`; none if
    /// there isn't one and not). ENOBUFS: some were lost.
    pub fn recv(&self, wait: bool) -> io::Result<Vec<(u16, u32, Vec<u8>)>> {
        let mut buf = vec![0u8; 1 << 16];
        // SAFETY: reading into our buffer, its length given.
        let n = unsafe { libc::recv(self.raw(), buf.as_mut_ptr().cast(), buf.len(), if wait { 0 } else { libc::MSG_DONTWAIT }) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                return Ok(Vec::new());
            }
            return Err(e);
        }
        Ok(split(&buf[..n as usize]))
    }

    /// Whether there's something to read within `ms`.
    pub fn readable(&self, ms: i32) -> bool {
        let mut p = libc::pollfd { fd: self.raw(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd of ours.
        unsafe { libc::poll(&mut p, 1, ms) > 0 }
    }

    /// Ask (`kind` with `body`, a dump if `dump`) and gather the answer:
    /// every message up to the dump's end, or the one answer.
    fn ask(&self, kind: u16, dump: bool, body: &[u8]) -> io::Result<Vec<Message>> {
        let seq = self.send(kind, if dump { libc::NLM_F_DUMP as u16 } else { 0 }, body)?;
        let mut out = Vec::new();
        loop {
            for (t, s, payload) in self.recv(true)? {
                if s != seq {
                    continue;
                }
                match t {
                    NLMSG_DONE => return Ok(out),
                    NLMSG_ERROR => {
                        let errno = payload.get(..4).map_or(0, |b| i32::from_ne_bytes(b.try_into().unwrap()));
                        if errno == 0 {
                            return Ok(out);
                        }
                        return Err(io::Error::from_raw_os_error(-errno));
                    }
                    _ => {
                        out.push((t, payload));
                        if !dump {
                            return Ok(out);
                        }
                    }
                }
            }
        }
    }

    /// Every link: its index, and what's known of it.
    pub fn links(&self) -> io::Result<BTreeMap<u32, Link>> {
        let mut out = BTreeMap::new();
        for (t, m) in self.ask(RTM_GETLINK, true, &[0u8; 16])? {
            if t == RTM_NEWLINK
                && let Some((index, link)) = link(&m)
            {
                out.insert(index, link);
            }
        }
        Ok(out)
    }

    /// Every address on every link: the link's index and the address.
    pub fn addresses(&self) -> io::Result<BTreeSet<(u32, IpAddr)>> {
        let mut out = BTreeSet::new();
        for (t, m) in self.ask(RTM_GETADDR, true, &[0u8; 8])? {
            if t == RTM_NEWADDR
                && let Some(a) = address(&m)
            {
                out.insert(a);
            }
        }
        Ok(out)
    }

    /// The way a packet to `to` would go now, if there's one.
    pub fn way_to(&self, to: IpAddr) -> Option<Way> {
        let mut body = vec![0u8; 12];
        let octets: Vec<u8> = match to {
            IpAddr::V4(a) => {
                body[0] = libc::AF_INET as u8;
                body[1] = 32;
                a.octets().to_vec()
            }
            IpAddr::V6(a) => {
                body[0] = libc::AF_INET6 as u8;
                body[1] = 128;
                a.octets().to_vec()
            }
        };
        put_attr(&mut body, RTA_DST, &octets);
        let reply = self.ask(RTM_GETROUTE, false, &body).ok()?;
        reply.into_iter().find(|(t, _)| *t == RTM_NEWROUTE).and_then(|(_, m)| way(&m))
    }
}

/// The messages in a datagram: each one's type, sequence number and body.
fn split(mut data: &[u8]) -> Vec<(u16, u32, Vec<u8>)> {
    let mut out = Vec::new();
    while data.len() >= 16 {
        let len = u32::from_ne_bytes(data[0..4].try_into().unwrap()) as usize;
        if len < 16 || len > data.len() {
            break;
        }
        let kind = u16::from_ne_bytes(data[4..6].try_into().unwrap());
        let seq = u32::from_ne_bytes(data[8..12].try_into().unwrap());
        out.push((kind, seq, data[16..len].to_vec()));
        data = &data[align(len).min(data.len())..];
    }
    out
}

fn put_attr(out: &mut Vec<u8>, kind: u16, data: &[u8]) {
    let len = 4 + data.len();
    out.extend_from_slice(&(len as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(data);
    out.resize(align(out.len()), 0);
}

/// A message's attributes, after its fixed part of `head` bytes.
fn attrs(m: &[u8], head: usize) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut at = head;
    while at + 4 <= m.len() {
        let len = u16::from_ne_bytes(m[at..at + 2].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(m[at + 2..at + 4].try_into().unwrap()) & 0x3fff;
        if len < 4 || at + len > m.len() {
            break;
        }
        out.push((kind, &m[at + 4..at + len]));
        at += align(len);
    }
    out
}

fn ip(data: &[u8]) -> Option<IpAddr> {
    match data.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(data).ok()?))),
        16 => Some(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(data).ok()?))),
        _ => None,
    }
}

/// A link (an interface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    /// Up, and with a carrier (`IFF_RUNNING`): packets can go.
    pub running: bool,
    pub loopback: bool,
}

/// A link message (`ifinfomsg`): its index and what it says.
pub fn link(m: &[u8]) -> Option<(u32, Link)> {
    if m.len() < 16 {
        return None;
    }
    let index = i32::from_ne_bytes(m[4..8].try_into().ok()?) as u32;
    let flags = u32::from_ne_bytes(m[8..12].try_into().ok()?);
    let name = attrs(m, 16).into_iter().find(|(k, _)| *k == IFLA_IFNAME).map(|(_, v)| String::from_utf8_lossy(v.split(|b| *b == 0).next().unwrap_or_default()).into_owned()).unwrap_or_default();
    let up = flags & libc::IFF_UP as u32 != 0;
    Some((index, Link { name, running: up && flags & libc::IFF_RUNNING as u32 != 0, loopback: flags & libc::IFF_LOOPBACK as u32 != 0 }))
}

/// An address message (`ifaddrmsg`): the link's index and the address
/// (IPv4's own is `IFA_LOCAL`, `IFA_ADDRESS` being a point-to-point
/// link's other end; IPv6 has only `IFA_ADDRESS`).
pub fn address(m: &[u8]) -> Option<(u32, IpAddr)> {
    if m.len() < 8 {
        return None;
    }
    let index = u32::from_ne_bytes(m[4..8].try_into().ok()?);
    let a = attrs(m, 8);
    let local = a.iter().find(|(k, _)| *k == IFA_LOCAL).or_else(|| a.iter().find(|(k, _)| *k == IFA_ADDRESS))?;
    Some((index, ip(local.1)?))
}

/// Which way packets go: out of which link, from which address, through
/// which gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Way {
    pub link: u32,
    pub from: Option<IpAddr>,
    pub gateway: Option<IpAddr>,
}

/// A route message (`rtmsg`), as `RTM_GETROUTE` answers.
pub fn way(m: &[u8]) -> Option<Way> {
    if m.len() < 12 {
        return None;
    }
    let mut w = Way { link: 0, from: None, gateway: None };
    for (k, v) in attrs(m, 12) {
        match k {
            RTA_OIF => w.link = u32::from_ne_bytes(v.get(..4)?.try_into().ok()?),
            RTA_PREFSRC => w.from = ip(v),
            RTA_GATEWAY => w.gateway = ip(v),
            _ => {}
        }
    }
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_attributes_are_read() {
        // An address message: AF_INET, /24, link 3, IFA_LOCAL 192.168.1.20.
        let mut m = vec![libc::AF_INET as u8, 24, 0, 0];
        m.extend_from_slice(&3u32.to_ne_bytes());
        put_attr(&mut m, IFA_ADDRESS, &[192, 168, 1, 20]);
        put_attr(&mut m, IFA_LOCAL, &[192, 168, 1, 20]);
        assert_eq!(address(&m), Some((3, "192.168.1.20".parse().unwrap())));
        // Two of them in a datagram, as the kernel sends them.
        let mut d = Vec::new();
        for seq in [7u32, 8] {
            d.extend_from_slice(&((16 + m.len()) as u32).to_ne_bytes());
            d.extend_from_slice(&RTM_DELADDR.to_ne_bytes());
            d.extend_from_slice(&0u16.to_ne_bytes());
            d.extend_from_slice(&seq.to_ne_bytes());
            d.extend_from_slice(&0u32.to_ne_bytes());
            d.extend_from_slice(&m);
        }
        let got = split(&d);
        assert_eq!(got.len(), 2);
        assert_eq!((got[1].0, got[1].1), (RTM_DELADDR, 8));
        assert_eq!(address(&got[0].2), address(&m));
    }

    /// The kernel's own answers, here: there's a loopback link, and a
    /// way to it.
    #[test]
    fn the_kernel_answers() {
        let nl = Netlink::open(0).unwrap();
        let links = nl.links().unwrap();
        assert!(links.values().any(|l| l.loopback && l.running), "{links:?}");
        let addrs = nl.addresses().unwrap();
        assert!(addrs.iter().any(|(_, a)| a.is_loopback()), "{addrs:?}");
        let w = nl.way_to("127.0.0.1".parse().unwrap()).unwrap();
        assert!(links.get(&w.link).is_some_and(|l| l.loopback), "{w:?}");
    }
}
