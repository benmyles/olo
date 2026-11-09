//! WebRTC's network side, for RTCPeerConnection (rtc.js): its ICE
//! candidates gathered as Chrome gathers them for a page that has no
//! camera or microphone permission. That means one UDP socket on the
//! default route, its host candidate (an mDNS name rtc.js makes, never the
//! address), and its server-reflexive address from each STUN server (an
//! RFC 8489 Binding request, retransmitted as the RFC says). Also the
//! certificate DTLS would use: a self-signed ECDSA P-256 one, "CN=WebRTC",
//! valid from a day ago for thirty days, as Chrome makes each connection's.
//!
//! Sites check this: Cloudflare's challenge gathers candidates from its
//! STUN servers and compares the address with the request's.
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::worker::Waker;

/// What a gathering found, in order.
pub enum Event {
    /// The socket is bound: the host candidate's port.
    Host(u16),
    /// A STUN server's answer: the address it saw the socket at.
    Srflx { address: IpAddr, port: u16, url: String },
    /// A STUN server that never answered, or wasn't found (Chrome's
    /// icecandidateerror: 701 for both).
    Failed { url: String, text: String },
    /// Every server answered or timed out.
    Done,
}

/// A gathering under way: its events, and the socket's thread, stopped
/// when this is dropped (the connection closed, or the page gone).
pub struct Gathering {
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Drop for Gathering {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

const MAGIC: u32 = 0x2112_a442;
/// RFC 8489 §6.2.1: an RTO of 500 ms, doubled each time, seven sends, the
/// last waited on for 16 RTOs (39.5 s in all).
const SENDS_MS: [u64; 7] = [0, 500, 1500, 3500, 7500, 15500, 31500];
const GIVE_UP_MS: u64 = 39_500;

/// Gathers from `servers` (each `host:port`, from a `stun:` URL), waking
/// the page as each event comes.
pub fn gather(servers: Vec<String>, waker: Option<Waker>) -> std::io::Result<Gathering> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let port = socket.local_addr()?.port();
    let (tx, events) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    std::thread::Builder::new().name("page-ice".into()).spawn(move || run(socket, port, servers, tx, waker, thread_stop))?;
    Ok(Gathering { events, stop })
}

fn run(socket: UdpSocket, port: u16, servers: Vec<String>, tx: Sender<Event>, waker: Option<Waker>, stop: Arc<AtomicBool>) {
    let send = |e: Event| {
        let _ = tx.send(e);
        if let Some(w) = &waker {
            w();
        }
    };
    send(Event::Host(port));
    struct Probe {
        url: String,
        to: Option<SocketAddr>,
        id: [u8; 12],
        sent: usize,
        done: bool,
    }
    let mut probes: Vec<Probe> = servers
        .into_iter()
        .map(|url| {
            // (Names resolved here, off the page's thread; IPv4, as the
            // socket is.)
            let to = url.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4));
            let mut id = [0u8; 12];
            let _ = getrandom::fill(&mut id);
            Probe { url, to, id, sent: 0, done: false }
        })
        .collect();
    for p in probes.iter_mut().filter(|p| p.to.is_none()) {
        p.done = true;
        send(Event::Failed { url: p.url.clone(), text: "STUN host lookup received error.".into() });
    }
    let start = Instant::now();
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) && probes.iter().any(|p| !p.done) {
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= GIVE_UP_MS {
            for p in probes.iter_mut().filter(|p| !p.done) {
                p.done = true;
                send(Event::Failed { url: p.url.clone(), text: "STUN binding request timed out.".into() });
            }
            break;
        }
        for p in probes.iter_mut().filter(|p| !p.done) {
            if p.sent < SENDS_MS.len()
                && elapsed >= SENDS_MS[p.sent]
                && let Some(to) = p.to
            {
                let _ = socket.send_to(&binding_request(&p.id), to);
                p.sent += 1;
            }
        }
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        let Some((id, mapped)) = binding_response(&buf[..n]) else { continue };
        if let Some(p) = probes.iter_mut().find(|p| !p.done && p.id == id && p.to == Some(from)) {
            p.done = true;
            send(Event::Srflx { address: mapped.ip(), port: mapped.port(), url: p.url.clone() });
        }
    }
    if !stop.load(Ordering::Relaxed) {
        send(Event::Done);
    }
}

/// A Binding request (RFC 8489 §5): no attributes.
fn binding_request(id: &[u8; 12]) -> [u8; 20] {
    let mut m = [0u8; 20];
    m[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    m[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    m[8..20].copy_from_slice(id);
    m
}

/// A Binding success response's transaction id and mapped address
/// (XOR-MAPPED-ADDRESS, else MAPPED-ADDRESS).
fn binding_response(m: &[u8]) -> Option<([u8; 12], SocketAddr)> {
    if m.len() < 20 || u16::from_be_bytes([m[0], m[1]]) != 0x0101 || u32::from_be_bytes([m[4], m[5], m[6], m[7]]) != MAGIC {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([m[2], m[3]]));
    let id: [u8; 12] = m[8..20].try_into().ok()?;
    let body = m.get(20..20 + len)?;
    let (mut i, mut plain) = (0, None);
    while i + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[i], body[i + 1]]);
        let n = usize::from(u16::from_be_bytes([body[i + 2], body[i + 3]]));
        let value = body.get(i + 4..i + 4 + n)?;
        // (IPv4 only: the socket's family.)
        if (kind == 0x0020 || kind == 0x0001) && n >= 8 && value[1] == 0x01 {
            let port = u16::from_be_bytes([value[2], value[3]]);
            let ip = [value[4], value[5], value[6], value[7]];
            if kind == 0x0020 {
                let cookie = MAGIC.to_be_bytes();
                let ip = std::net::Ipv4Addr::new(ip[0] ^ cookie[0], ip[1] ^ cookie[1], ip[2] ^ cookie[2], ip[3] ^ cookie[3]);
                return Some((id, SocketAddr::new(IpAddr::V4(ip), port ^ (MAGIC >> 16) as u16)));
            }
            plain = Some(SocketAddr::new(IpAddr::V4(ip.into()), port));
        }
        i += 4 + n.div_ceil(4) * 4;
    }
    plain.map(|a| (id, a))
}

/// A connection's certificate as Chrome makes one: its DER, its SHA-256
/// fingerprint (`AA:BB:…`), and when it expires (ms since the epoch).
pub fn certificate() -> Option<(Vec<u8>, String, f64)> {
    use sha2::Digest;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).ok()?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(rcgen::DnType::CommonName, "WebRTC");
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(30);
    let mut serial = [0u8; 8];
    getrandom::fill(&mut serial).ok()?;
    serial[0] &= 0x7f;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    let cert = params.self_signed(&key).ok()?;
    let der = cert.der().to_vec();
    let fingerprint = sha2::Sha256::digest(&der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
    let expires = (params.not_after.unix_timestamp_nanos() / 1_000_000) as f64;
    Some((der, fingerprint, expires))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_binding_response_gives_its_mapped_address() {
        // An XOR-MAPPED-ADDRESS of 71.212.142.246:45016 (RFC 8489 §14.2).
        let id = [7u8; 12];
        let mut m = binding_request(&id).to_vec();
        m[0..2].copy_from_slice(&0x0101u16.to_be_bytes());
        m[2..4].copy_from_slice(&12u16.to_be_bytes());
        let cookie = MAGIC.to_be_bytes();
        let port = 45016u16 ^ (MAGIC >> 16) as u16;
        m.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
        m.extend_from_slice(&port.to_be_bytes());
        m.extend_from_slice(&[71 ^ cookie[0], 212 ^ cookie[1], 142 ^ cookie[2], 246 ^ cookie[3]]);
        let (got_id, addr) = binding_response(&m).unwrap();
        assert_eq!(got_id, id);
        assert_eq!(addr, "71.212.142.246:45016".parse().unwrap());
        assert!(binding_response(&m[..10]).is_none());
    }
    #[test]
    fn a_certificate_is_chromes_kind() {
        let (der, fingerprint, expires) = certificate().unwrap();
        assert!(der.len() > 200);
        assert_eq!(fingerprint.len(), 32 * 3 - 1);
        let days = (expires / 1000. - time::OffsetDateTime::now_utc().unix_timestamp() as f64) / 86400.;
        assert!((29.9..30.1).contains(&days));
    }
}
