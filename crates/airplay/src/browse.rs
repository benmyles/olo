//! Asking the network who offers a service (mDNS, RFC 6762; DNS-SD, RFC
//! 6763), and putting together what each instance answers with: its
//! name, its host and port, the host's addresses, and what it says of
//! itself.
//!
//! Asked from a port of our own with the answer wanted back to us alone
//! (a "legacy unicast" question), on each network the computer is on: no
//! multicast group is joined and port 5353 isn't taken, so it runs
//! beside whatever else here speaks mDNS (Avahi, a browser).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use crate::dns::{self, Data};

/// Where a question goes and which of the computer's addresses it's
/// asked from.
#[derive(Debug, Clone)]
pub struct Via {
    pub to: SocketAddr,
    pub from: Vec<Ipv4Addr>,
}

impl Via {
    /// mDNS's own group, from every network the computer is on.
    pub fn lan() -> Via {
        Via { to: SocketAddr::from(([224, 0, 0, 251], 5353)), from: local_addresses() }
    }
}

/// An instance of a service, as it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// Its own name ("Living Room"), as people gave it.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    /// What it says of itself (its TXT record).
    pub txt: BTreeMap<String, String>,
}

/// When the question is asked, from the start: at once, and again for
/// answers lost and for what the first answers left out.
const ASKS: [Duration; 3] = [Duration::ZERO, Duration::from_millis(250), Duration::from_millis(1000)];
/// The most questions in one message (it must fit a packet).
const QUESTIONS: usize = 12;

/// The instances of `service` ("_airplay._tcp") that answer within
/// `wait`. Blocks for `wait`.
pub fn browse(service: &str, wait: Duration, via: &Via) -> std::io::Result<Vec<Service>> {
    let service = format!("{service}.local");
    let sockets: Vec<UdpSocket> = via.from.iter().filter_map(|from| socket(*from).ok()).collect();
    let mut found = Found::default();
    let began = Instant::now();
    let mut asked = 0;
    while began.elapsed() < wait && !sockets.is_empty() {
        if asked < ASKS.len() && began.elapsed() >= ASKS[asked] {
            asked += 1;
            for message in found.questions(&service) {
                for s in &sockets {
                    let _ = s.send_to(&message, via.to);
                }
            }
        }
        let mut buf = [0u8; 9000];
        for s in &sockets {
            while let Ok((n, _)) = s.recv_from(&mut buf) {
                if let Some(records) = dns::records(&buf[..n]) {
                    found.take(&service, records);
                }
            }
        }
    }
    Ok(found.services(&service))
}

/// A socket to ask from `from`: a port of its own, its multicast sent
/// out of that address's network, read a little at a time.
fn socket(from: Ipv4Addr) -> std::io::Result<UdpSocket> {
    use std::os::fd::AsRawFd as _;
    let s = UdpSocket::bind((from, 0))?;
    s.set_read_timeout(Some(Duration::from_millis(25)))?;
    s.set_multicast_ttl_v4(255)?;
    let addr = libc::in_addr { s_addr: u32::from_ne_bytes(from.octets()) };
    // SAFETY: a socket option set from a value of its own type and size.
    let r = unsafe { libc::setsockopt(s.as_raw_fd(), libc::IPPROTO_IP, libc::IP_MULTICAST_IF, (&raw const addr).cast(), std::mem::size_of::<libc::in_addr>() as libc::socklen_t) };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(s)
}

/// The computer's IPv4 addresses on networks that are up and carry
/// multicast (not the loopback).
fn local_addresses() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, read as it says and freed after.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut at = list;
        while !at.is_null() {
            let i = &*at;
            let wanted = libc::IFF_UP as u32 | libc::IFF_MULTICAST as u32;
            if !i.ifa_addr.is_null() && (*i.ifa_addr).sa_family as i32 == libc::AF_INET && i.ifa_flags & wanted == wanted && i.ifa_flags & libc::IFF_LOOPBACK as u32 == 0 {
                let a = &*(i.ifa_addr as *const libc::sockaddr_in);
                out.push(Ipv4Addr::from(a.sin_addr.s_addr.to_ne_bytes()));
            }
            at = i.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    out.sort();
    out.dedup();
    out
}

/// What has answered so far. Names are compared without their case, as
/// DNS's are.
#[derive(Default)]
struct Found {
    /// Each instance, by its name in lower case: as it wrote it.
    instances: BTreeMap<String, String>,
    srv: HashMap<String, (String, u16)>,
    txt: HashMap<String, BTreeMap<String, String>>,
    addrs: HashMap<String, BTreeSet<IpAddr>>,
}

impl Found {
    fn take(&mut self, service: &str, records: Vec<dns::Record>) {
        for r in records {
            let owner = r.name.to_lowercase();
            match r.data {
                Data::Ptr(instance) if owner == service.to_lowercase() => {
                    self.instances.insert(instance.to_lowercase(), instance);
                }
                Data::Srv { host, port } => {
                    self.srv.insert(owner, (host, port));
                }
                Data::Txt(pairs) => {
                    self.txt.insert(owner, pairs.into_iter().collect());
                }
                Data::Addr(addr) => {
                    self.addrs.entry(owner).or_default().insert(addr);
                }
                _ => {}
            }
        }
    }

    /// What to ask now: who offers the service, and of those known,
    /// what each hasn't said yet (most say it all with the first answer).
    fn questions(&self, service: &str) -> Vec<Vec<u8>> {
        let mut asked: Vec<(String, u16)> = vec![(service.to_owned(), dns::PTR)];
        for (key, instance) in &self.instances {
            match self.srv.get(key) {
                None => asked.push((instance.clone(), dns::SRV)),
                Some((host, _)) if !self.addrs.contains_key(&host.to_lowercase()) => asked.push((host.clone(), dns::A)),
                _ => {}
            }
            if !self.txt.contains_key(key) {
                asked.push((instance.clone(), dns::TXT));
            }
        }
        asked.chunks(QUESTIONS).map(|some| dns::questions(&some.iter().map(|(n, k)| (n.as_str(), *k)).collect::<Vec<_>>())).collect()
    }

    /// Those that have said where they are (an instance with no host or
    /// port yet isn't one to send to).
    fn services(&self, service: &str) -> Vec<Service> {
        let suffix = format!(".{}", service.to_lowercase());
        self.instances
            .iter()
            .filter_map(|(key, instance)| {
                let (host, port) = self.srv.get(key)?.clone();
                let name = instance.get(..key.strip_suffix(&suffix)?.len())?.to_owned();
                let mut addrs: Vec<IpAddr> = self.addrs.get(&host.to_lowercase()).into_iter().flatten().copied().collect();
                // (IPv4 first: what a receiver is reached by here.)
                addrs.sort_by_key(|a| (a.is_ipv6(), *a));
                Some(Service { name, host, port, addrs, txt: self.txt.get(key).cloned().unwrap_or_default() })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::Record;

    /// A responder of the test's own on the loopback: answers each
    /// question it's sent with `answer(question number)`, back to the
    /// asker. Its address.
    fn responder(answer: impl Fn(usize, &[u8]) -> Option<Vec<u8>> + Send + 'static) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = s.local_addr().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 2000];
            let mut n = 0;
            while let Ok((len, from)) = s.recv_from(&mut buf) {
                if let Some(reply) = answer(n, &buf[..len]) {
                    let _ = s.send_to(&reply, from);
                }
                n += 1;
            }
        });
        at
    }

    fn rec(name: &str, data: Data) -> Record {
        Record { name: name.into(), data }
    }

    /// A receiver that says everything at once, as most do; another that
    /// says only who it is until it's asked the rest; and what answers
    /// for another service isn't taken for this one's.
    #[test]
    fn instances_are_put_together_from_what_answers() {
        let to = responder(|n, question| {
            let asked = String::from_utf8_lossy(question).into_owned();
            let all_at_once = [
                rec("_airplay._tcp.local", Data::Ptr("Front Room._airplay._tcp.local".into())),
                rec("Front Room._airplay._tcp.local", Data::Srv { host: "Front-Room.local".into(), port: 7000 }),
                rec("Front Room._airplay._tcp.local", Data::Txt(vec![("model".into(), "AppleTV14,1".into()), ("features".into(), "0x4A7FDFD5,0x3C177FDE".into())])),
                rec("front-room.local", Data::Addr("fe80::1".parse().unwrap())),
                rec("front-room.local", Data::Addr("10.0.0.7".parse().unwrap())),
                rec("_airplay._tcp.local", Data::Ptr("Slow One._airplay._tcp.local".into())),
                rec("_raop._tcp.local", Data::Ptr("AABBCC@Other._raop._tcp.local".into())),
            ];
            match n {
                0 => Some(dns::response(&all_at_once)),
                // Asked again, with what the slow one left out: it says it now.
                _ if asked.contains("Slow One") => Some(dns::response(&[
                    rec("Slow One._airplay._tcp.local", Data::Srv { host: "slow.local".into(), port: 7100 }),
                    rec("Slow One._airplay._tcp.local", Data::Txt(vec![("model".into(), "Mac16,8".into())])),
                    rec("slow.local", Data::Addr("10.0.0.9".parse().unwrap())),
                ])),
                _ => None,
            }
        });
        let via = Via { to, from: vec![Ipv4Addr::LOCALHOST] };
        let found = browse("_airplay._tcp", Duration::from_millis(700), &via).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        let front = &found[0];
        assert_eq!((front.name.as_str(), front.host.as_str(), front.port), ("Front Room", "Front-Room.local", 7000));
        assert_eq!(front.addrs, ["10.0.0.7".parse::<IpAddr>().unwrap(), "fe80::1".parse().unwrap()]);
        assert_eq!(front.txt["model"], "AppleTV14,1");
        let slow = &found[1];
        assert_eq!((slow.name.as_str(), slow.port, slow.txt["model"].as_str()), ("Slow One", 7100, "Mac16,8"));
        assert_eq!(slow.addrs, ["10.0.0.9".parse::<IpAddr>().unwrap()]);
    }

    /// Nobody answers, or there's no network to ask on: nothing found,
    /// and no longer waited than asked.
    #[test]
    fn nobody_there() {
        let to = responder(|_, _| None);
        let began = Instant::now();
        assert!(browse("_airplay._tcp", Duration::from_millis(300), &Via { to, from: vec![Ipv4Addr::LOCALHOST] }).unwrap().is_empty());
        assert!(began.elapsed() < Duration::from_millis(600));
        assert!(browse("_airplay._tcp", Duration::from_secs(5), &Via { to, from: Vec::new() }).unwrap().is_empty());
        assert!(began.elapsed() < Duration::from_millis(1200));
    }

    /// What isn't DNS at all (anything can be sent to a port) is passed over.
    #[test]
    fn rubbish_is_passed_over() {
        let to = responder(|n, _| match n {
            0 => Some(b"not a DNS message at all".to_vec()),
            _ => Some(dns::response(&[rec("_airplay._tcp.local", Data::Ptr("A._airplay._tcp.local".into())), rec("A._airplay._tcp.local", Data::Srv { host: "a.local".into(), port: 7000 })])),
        });
        let found = browse("_airplay._tcp", Duration::from_millis(500), &Via { to, from: vec![Ipv4Addr::LOCALHOST] }).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "A");
    }
}
