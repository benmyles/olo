//! The network as the kernel tells it over netlink, natively (what `ss`
//! and `ip -s link` read): every TCP and UDP socket with its addresses,
//! state, owner's inode and, for TCP, the bytes it has sent and received
//! (`sock_diag`, `tcp_info`), and each interface's byte counters
//! (`RTM_GETLINK`, `IFLA_STATS64`). One dump each, instead of reading
//! and parsing `/proc/net`'s text.

use std::{io, net::IpAddr};

const NETLINK_SOCK_DIAG: i32 = 4;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const RTM_GETLINK: u16 = 18;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const INET_DIAG_INFO: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_STATS64: u16 = 23;

/// One socket.
#[derive(Debug, Clone, PartialEq)]
pub struct Socket {
    pub tcp: bool,
    /// TCP's state number (1 established … 10 listen); UDP's 7 unconnected, 1 connected.
    pub state: u8,
    pub local: (IpAddr, u16),
    pub remote: (IpAddr, u16),
    /// The socket's inode (its owner's `/proc/PID/fd` links say `socket:[inode]`).
    pub inode: u64,
    /// Bytes sent (acknowledged) and received so far, for TCP.
    pub sent: u64,
    pub received: u64,
}

/// A netlink socket of `protocol`, closed when dropped.
struct Netlink(i32);

impl Drop for Netlink {
    fn drop(&mut self) {
        // SAFETY: our own descriptor, closed once.
        unsafe { libc::close(self.0) };
    }
}

impl Netlink {
    fn open(protocol: i32) -> io::Result<Netlink> {
        // SAFETY: plain socket(2).
        let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, protocol) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Netlink(fd))
    }

    /// Send one request (`kind`, `body`) as a dump, and gather every reply
    /// message's payload until the dump is done.
    fn dump(&self, kind: u16, body: &[u8]) -> io::Result<Vec<(u16, Vec<u8>)>> {
        let len = 16 + body.len();
        let mut msg = Vec::with_capacity(len);
        msg.extend_from_slice(&(len as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(&((libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16).to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(body);
        // SAFETY: a zeroed sockaddr_nl is the kernel's address.
        let mut to: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        to.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: the buffer and address are ours, their lengths given.
        let sent = unsafe {
            libc::sendto(self.0, msg.as_ptr().cast(), msg.len(), 0, (&to as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32)
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            // SAFETY: reading into our buffer, its length given.
            let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut at = 0usize;
            let n = n as usize;
            while at + 16 <= n {
                let mlen = u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap()) as usize;
                let mtype = u16::from_ne_bytes(buf[at + 4..at + 6].try_into().unwrap());
                if mlen < 16 || at + mlen > n {
                    return Ok(out);
                }
                match mtype {
                    NLMSG_DONE => return Ok(out),
                    NLMSG_ERROR => {
                        let code = i32::from_ne_bytes(buf[at + 16..at + 20].try_into().unwrap_or([0; 4]));
                        return if code == 0 { Ok(out) } else { Err(io::Error::from_raw_os_error(-code)) };
                    }
                    _ => out.push((mtype, buf[at + 16..at + mlen].to_vec())),
                }
                at += (mlen + 3) & !3;
            }
        }
    }
}

/// The attributes after a message's fixed part: (type, data).
fn attributes(mut data: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    while data.len() >= 4 {
        let len = u16::from_ne_bytes([data[0], data[1]]) as usize;
        let kind = u16::from_ne_bytes([data[2], data[3]]) & 0x3fff;
        if len < 4 || len > data.len() {
            break;
        }
        out.push((kind, &data[4..len]));
        data = &data[((len + 3) & !3).min(data.len())..];
    }
    out
}

fn address(family: u8, words: &[u8]) -> IpAddr {
    if family as i32 == libc::AF_INET6 {
        let b: [u8; 16] = words[..16].try_into().unwrap();
        let v6 = std::net::Ipv6Addr::from(b);
        // A v4 peer of a dual-stack socket says so.
        v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4)
    } else {
        IpAddr::V4(std::net::Ipv4Addr::new(words[0], words[1], words[2], words[3]))
    }
}

/// An `inet_diag_msg` and its attributes, as a socket.
fn socket(tcp: bool, m: &[u8]) -> Option<Socket> {
    // family, state, timer, retrans; the id (sport, dport, src[16], dst[16],
    // if, cookie[8]); expires, rqueue, wqueue, uid, inode.
    if m.len() < 72 {
        return None;
    }
    let family = m[0];
    let port = |at: usize| u16::from_be_bytes([m[at], m[at + 1]]);
    let mut s = Socket {
        tcp,
        state: m[1],
        local: (address(family, &m[8..24]), port(4)),
        remote: (address(family, &m[24..40]), port(6)),
        inode: u32::from_ne_bytes(m[68..72].try_into().ok()?) as u64,
        sent: 0,
        received: 0,
    };
    for (kind, data) in attributes(&m[72..]) {
        // tcp_info: tcpi_bytes_acked at 120, tcpi_bytes_received at 128.
        if kind == INET_DIAG_INFO && data.len() >= 136 {
            s.sent = u64::from_ne_bytes(data[120..128].try_into().ok()?);
            s.received = u64::from_ne_bytes(data[128..136].try_into().ok()?);
        }
    }
    Some(s)
}

/// Every TCP and UDP socket, v4 and v6.
pub fn sockets() -> io::Result<Vec<Socket>> {
    let nl = Netlink::open(NETLINK_SOCK_DIAG)?;
    let mut out = Vec::new();
    for family in [libc::AF_INET as u8, libc::AF_INET6 as u8] {
        for (protocol, tcp) in [(libc::IPPROTO_TCP as u8, true), (libc::IPPROTO_UDP as u8, false)] {
            // inet_diag_req_v2: family, protocol, ext (tcp_info), pad, every
            // state, a blank socket id.
            let mut req = vec![family, protocol, if tcp { 1 << (INET_DIAG_INFO - 1) } else { 0 }, 0];
            req.extend_from_slice(&u32::MAX.to_ne_bytes());
            req.extend_from_slice(&[0u8; 48]);
            for (kind, m) in nl.dump(SOCK_DIAG_BY_FAMILY, &req)? {
                if kind == SOCK_DIAG_BY_FAMILY
                    && let Some(s) = socket(tcp, &m)
                {
                    out.push(s);
                }
            }
        }
    }
    Ok(out)
}

/// An interface's counters so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Link {
    pub name: String,
    pub received: u64,
    pub sent: u64,
    pub errors: u64,
    pub drops: u64,
}

/// Each interface's counters.
pub fn interfaces() -> io::Result<Vec<Link>> {
    let nl = Netlink::open(libc::NETLINK_ROUTE)?;
    // ifinfomsg: family, pad, type, index, flags, change.
    let req = [0u8; 16];
    let mut out = Vec::new();
    for (kind, m) in nl.dump(RTM_GETLINK, &req)? {
        if kind != libc::RTM_NEWLINK || m.len() < 16 {
            continue;
        }
        let (mut name, mut stats) = (String::new(), None);
        for (k, data) in attributes(&m[16..]) {
            match k {
                IFLA_IFNAME => name = String::from_utf8_lossy(data).trim_end_matches('\0').to_owned(),
                // rtnl_link_stats64: rx_packets, tx_packets, rx_bytes,
                // tx_bytes, rx_errors, tx_errors, rx_dropped, tx_dropped, …
                IFLA_STATS64 if data.len() >= 64 => {
                    let word = |at: usize| u64::from_ne_bytes(data[at..at + 8].try_into().unwrap());
                    stats = Some((word(16), word(24), word(32) + word(40), word(48) + word(56)));
                }
                _ => {}
            }
        }
        if let (false, Some((received, sent, errors, drops))) = (name.is_empty(), stats) {
            out.push(Link { name, received, sent, errors, drops });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_a_connection_of_its_own() {
        // A listener and a client of it, on this computer, with a few bytes.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(&[7u8; 1000]).unwrap();
        let mut got = [0u8; 1000];
        server.read_exact(&mut got).unwrap();
        let all = sockets().unwrap();
        let mine = all.iter().find(|s| s.tcp && s.local.1 == port && s.state == 1 && s.remote.0 == IpAddr::from([127, 0, 0, 1])).expect("the server's side");
        // (The kernel counts the handshake's sequence number too.)
        assert!((1000..=1001).contains(&mine.received), "{}", mine.received);
        let theirs = all.iter().find(|s| s.tcp && s.remote.1 == port && s.state == 1).expect("the client's side");
        assert!((1000..=1001).contains(&theirs.sent), "{}", theirs.sent);
        assert!(all.iter().any(|s| s.tcp && s.local.1 == port && s.state == 10), "the listener");
        assert!(theirs.inode != 0);
    }

    #[test]
    fn counts_the_loopback() {
        let ifaces = interfaces().unwrap();
        assert!(ifaces.iter().any(|l| l.name == "lo"), "{ifaces:?}");
    }
}
