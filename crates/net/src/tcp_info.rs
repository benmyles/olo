//! One TCP connection as the kernel keeps it: its congestion control
//! (`TCP_CONGESTION`), everything TCP counts and estimates for it
//! (`TCP_INFO`: the round-trip time and its spread, the windows, what
//! was sent, retransmitted and delivered and how fast, what held it
//! back), and BBR's own model of the path where BBR runs it
//! (`TCP_CC_INFO`). Read from the socket itself, natively, for a
//! program's own connections (a speed test's, a transfer's): what `ss
//! -ti` shows for every socket, [`TcpInfo::of`] gives for one.

use std::{io, os::fd::AsRawFd};

const TCP_INFO: libc::c_int = 11;
const TCP_CONGESTION: libc::c_int = 13;
const TCP_CC_INFO: libc::c_int = 26;

/// What the peers agreed at the handshake ([`TcpInfo::options`]).
pub mod options {
    pub const TIMESTAMPS: u8 = 1;
    pub const SACK: u8 = 2;
    pub const WINDOW_SCALING: u8 = 4;
    /// Explicit congestion notification.
    pub const ECN: u8 = 8;
    /// A packet marked as able to carry it has arrived.
    pub const ECN_SEEN: u8 = 16;
    /// Data went with the handshake (Fast Open).
    pub const SYN_DATA: u8 = 32;
    /// Timestamps in microseconds.
    pub const USEC_TIMESTAMPS: u8 = 64;
}

/// Where the sender is with losses ([`TcpInfo::ca_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaState {
    /// Nothing amiss.
    Open,
    /// Duplicate ACKs or SACKs: something may be lost or out of order.
    Disorder,
    /// The window is being reduced (congestion notified, or locally).
    WindowReduced,
    /// Lost packets are being retransmitted.
    Recovery,
    /// A timeout: everything outstanding is taken as lost.
    Loss,
}

impl CaState {
    pub fn name(self) -> &'static str {
        match self {
            CaState::Open => "open",
            CaState::Disorder => "disorder",
            CaState::WindowReduced => "window reduced",
            CaState::Recovery => "recovery",
            CaState::Loss => "loss",
        }
    }
}

/// The kernel's `struct tcp_info`, every field of it, in its units
/// (times in microseconds but where a name says milliseconds; rates in
/// bytes a second; windows in packets but where a name says bytes).
/// A kernel older than a field leaves it zero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TcpInfo {
    /// TCP's state number (1 established).
    pub state: u8,
    ca_state: u8,
    /// Retransmissions of what's outstanding now, by timeout, in a row.
    pub retransmits: u8,
    pub probes: u8,
    pub backoff: u8,
    /// [`options`] bits.
    pub options: u8,
    pub snd_wscale: u8,
    pub rcv_wscale: u8,
    /// The delivery rate was measured while the program, not the
    /// network, limited what was sent.
    pub delivery_rate_app_limited: bool,
    /// The retransmission timeout.
    pub rto_us: u32,
    pub ato_us: u32,
    /// The sending and receiving segment sizes, bytes.
    pub snd_mss: u32,
    pub rcv_mss: u32,
    /// Packets sent and not yet acknowledged; of them selectively
    /// acknowledged, taken as lost, and retransmitted.
    pub unacked: u32,
    pub sacked: u32,
    pub lost: u32,
    pub retrans: u32,
    pub last_data_sent_ms: u32,
    pub last_data_recv_ms: u32,
    pub last_ack_recv_ms: u32,
    /// The path's MTU as known, bytes.
    pub pmtu: u32,
    pub rcv_ssthresh: u32,
    /// The smoothed round-trip time and its mean deviation.
    pub rtt_us: u32,
    pub rttvar_us: u32,
    pub snd_ssthresh: u32,
    /// The congestion window, packets.
    pub snd_cwnd: u32,
    pub advmss: u32,
    pub reordering: u32,
    /// The receiver's own estimate of the round trip, and the buffer
    /// space it's sizing for.
    pub rcv_rtt_us: u32,
    pub rcv_space: u32,
    /// Packets retransmitted, in all.
    pub total_retrans: u32,
    /// The rate packets are paced out at, and its ceiling (`u64::MAX`:
    /// none).
    pub pacing_rate: u64,
    pub max_pacing_rate: u64,
    /// Bytes the peer has acknowledged (the handshake's SYN counts as
    /// one, as the kernel has it), and bytes received.
    pub bytes_acked: u64,
    pub bytes_received: u64,
    pub segs_out: u32,
    pub segs_in: u32,
    /// Bytes written and not yet sent.
    pub notsent_bytes: u32,
    /// The smallest round-trip time seen (over the last minutes).
    pub min_rtt_us: u32,
    pub data_segs_in: u32,
    pub data_segs_out: u32,
    /// How fast data has been arriving at the peer, most recently.
    pub delivery_rate: u64,
    /// Time with data to send; of it, held by the peer's window, and by
    /// this end's send buffer.
    pub busy_time_us: u64,
    pub rwnd_limited_us: u64,
    pub sndbuf_limited_us: u64,
    /// Packets delivered, in all, and of them marked as having met
    /// congestion.
    pub delivered: u32,
    pub delivered_ce: u32,
    /// Bytes sent, retransmissions included, and the retransmitted.
    pub bytes_sent: u64,
    pub bytes_retrans: u64,
    /// Retransmissions the peer said it didn't need.
    pub dsack_dups: u32,
    /// Times packets were seen to arrive out of order.
    pub reord_seen: u32,
    pub rcv_ooopack: u32,
    /// The peer's window and this end's, bytes.
    pub snd_wnd: u32,
    pub rcv_wnd: u32,
    pub rehash: u32,
    /// Timeouts, in all; recoveries from them, and the time they took.
    pub total_rto: u16,
    pub total_rto_recoveries: u16,
    pub total_rto_time_ms: u32,
}

impl TcpInfo {
    /// `socket`'s, as of now.
    pub fn of(socket: &impl AsRawFd) -> io::Result<TcpInfo> {
        let mut buf = [0u8; 512];
        let len = getsockopt(socket, TCP_INFO, &mut buf)?;
        Ok(TcpInfo::parse(&buf[..len]))
    }

    /// From the kernel's bytes (as many as it gave: an older kernel's are
    /// fewer).
    pub fn parse(b: &[u8]) -> TcpInfo {
        let u8_at = |at: usize| b.get(at).copied().unwrap_or(0);
        let u16_at = |at: usize| b.get(at..at + 2).map_or(0, |s| u16::from_ne_bytes(s.try_into().expect("two")));
        let u32_at = |at: usize| b.get(at..at + 4).map_or(0, |s| u32::from_ne_bytes(s.try_into().expect("four")));
        let u64_at = |at: usize| b.get(at..at + 8).map_or(0, |s| u64::from_ne_bytes(s.try_into().expect("eight")));
        TcpInfo {
            state: u8_at(0),
            ca_state: u8_at(1),
            retransmits: u8_at(2),
            probes: u8_at(3),
            backoff: u8_at(4),
            options: u8_at(5),
            snd_wscale: u8_at(6) & 0xf,
            rcv_wscale: u8_at(6) >> 4,
            delivery_rate_app_limited: u8_at(7) & 1 != 0,
            rto_us: u32_at(8),
            ato_us: u32_at(12),
            snd_mss: u32_at(16),
            rcv_mss: u32_at(20),
            unacked: u32_at(24),
            sacked: u32_at(28),
            lost: u32_at(32),
            retrans: u32_at(36),
            last_data_sent_ms: u32_at(44),
            last_data_recv_ms: u32_at(52),
            last_ack_recv_ms: u32_at(56),
            pmtu: u32_at(60),
            rcv_ssthresh: u32_at(64),
            rtt_us: u32_at(68),
            rttvar_us: u32_at(72),
            snd_ssthresh: u32_at(76),
            snd_cwnd: u32_at(80),
            advmss: u32_at(84),
            reordering: u32_at(88),
            rcv_rtt_us: u32_at(92),
            rcv_space: u32_at(96),
            total_retrans: u32_at(100),
            pacing_rate: u64_at(104),
            max_pacing_rate: u64_at(112),
            bytes_acked: u64_at(120),
            bytes_received: u64_at(128),
            segs_out: u32_at(136),
            segs_in: u32_at(140),
            notsent_bytes: u32_at(144),
            min_rtt_us: u32_at(148),
            data_segs_in: u32_at(152),
            data_segs_out: u32_at(156),
            delivery_rate: u64_at(160),
            busy_time_us: u64_at(168),
            rwnd_limited_us: u64_at(176),
            sndbuf_limited_us: u64_at(184),
            delivered: u32_at(192),
            delivered_ce: u32_at(196),
            bytes_sent: u64_at(200),
            bytes_retrans: u64_at(208),
            dsack_dups: u32_at(216),
            reord_seen: u32_at(220),
            rcv_ooopack: u32_at(224),
            snd_wnd: u32_at(228),
            rcv_wnd: u32_at(232),
            rehash: u32_at(236),
            total_rto: u16_at(240),
            total_rto_recoveries: u16_at(242),
            total_rto_time_ms: u32_at(244),
        }
    }

    pub fn ca_state(&self) -> CaState {
        match self.ca_state {
            0 => CaState::Open,
            1 => CaState::Disorder,
            2 => CaState::WindowReduced,
            3 => CaState::Recovery,
            _ => CaState::Loss,
        }
    }

    /// Packets on their way now: sent, and neither acknowledged nor
    /// taken as lost (as the kernel reckons its own "in flight").
    pub fn in_flight(&self) -> u32 {
        (self.unacked + self.retrans).saturating_sub(self.sacked + self.lost)
    }

    /// The share of the bytes sent that were retransmissions.
    pub fn retransmitted(&self) -> f64 {
        if self.bytes_sent == 0 { 0.0 } else { self.bytes_retrans as f64 / self.bytes_sent as f64 }
    }
}

/// BBR's model of the path, as the kernel gives it for a connection BBR
/// runs (`tcp_bbr_info`; the same of BBRv1 and of Olo's `bbr3`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BbrInfo {
    /// The bottleneck's bandwidth as estimated, bytes a second.
    pub bandwidth: u64,
    /// The path's round-trip time with no queue, as estimated.
    pub min_rtt_us: u32,
    /// How much of that bandwidth packets are paced out at now (1.25
    /// while probing for more, under 1 while draining what the probe
    /// queued), and how many round trips' worth may be in flight.
    pub pacing_gain: f32,
    pub cwnd_gain: f32,
}

impl BbrInfo {
    pub fn parse(b: &[u8]) -> Option<BbrInfo> {
        let u = |at: usize| b.get(at..at + 4).map(|s| u32::from_ne_bytes(s.try_into().expect("four")));
        Some(BbrInfo { bandwidth: u(0)? as u64 | (u(4)? as u64) << 32, min_rtt_us: u(8)?, pacing_gain: u(12)? as f32 / 256.0, cwnd_gain: u(16)? as f32 / 256.0 })
    }

    /// What BBR is doing, told by its gains: "startup" (finding the
    /// bandwidth, doubling each round trip), "drain" (emptying the queue
    /// startup left), "probe up" (trying for more), "probe down" (giving
    /// back what the probe queued), "probe rtt" (in flight cut down, to
    /// see the round-trip time with no queue), or "cruise". (BBR's
    /// gains: 2.77 and 2 starting up, 0.35 draining, 1.25 probing up,
    /// 0.91 down (BBRv1: 0.75), and a window of one round trip's worth,
    /// not two, while probing the round-trip time.)
    pub fn phase(&self) -> &'static str {
        match (self.pacing_gain, self.cwnd_gain) {
            (p, _) if p > 2.0 => "startup",
            (p, _) if p < 0.5 => "drain",
            (_, c) if c < 1.5 => "probe rtt",
            (p, _) if p > 1.05 => "probe up",
            (p, _) if p < 0.95 => "probe down",
            _ => "cruise",
        }
    }
}

/// The congestion control `socket` runs ("cubic", "bbr", "bbr3").
pub fn congestion_control(socket: &impl AsRawFd) -> io::Result<String> {
    let mut buf = [0u8; 16];
    let len = getsockopt(socket, TCP_CONGESTION, &mut buf)?;
    let name = &buf[..len];
    Ok(String::from_utf8_lossy(name.split(|&c| c == 0).next().unwrap_or(name)).into_owned())
}

/// Have `socket` run the congestion control `name` (one the kernel has
/// or can load, and lets this user choose: `tcp_allowed_congestion_control`).
pub fn set_congestion_control(socket: &impl AsRawFd, name: &str) -> io::Result<()> {
    // SAFETY: setsockopt(2) with the name's bytes and their length.
    let r = unsafe { libc::setsockopt(socket.as_raw_fd(), libc::IPPROTO_TCP, TCP_CONGESTION, name.as_ptr().cast(), name.len() as libc::socklen_t) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// BBR's model of `socket`'s path, if a BBR runs it (by its name: other
/// congestion controls give other things for the same question).
pub fn bbr_info(socket: &impl AsRawFd) -> io::Result<Option<BbrInfo>> {
    if !congestion_control(socket)?.starts_with("bbr") {
        return Ok(None);
    }
    let mut buf = [0u8; 64];
    let len = getsockopt(socket, TCP_CC_INFO, &mut buf)?;
    Ok(BbrInfo::parse(&buf[..len]))
}

/// Which BBR this kernel's own "bbr" is, if it has one loaded: 1 (as
/// mainline's has been), or the version it says (Google's BBRv3, in a
/// kernel built with its patches, says 3). Told by what the kernel gives
/// for a socket that would run it: BBRv1's `tcp_bbr_info` is 20 bytes;
/// the patched kernel's is longer, with the version in it. None if
/// there's no "bbr" to choose (not built in, and its module not loaded:
/// only root's choosing it would load it, and this never does).
pub fn kernels_bbr_version() -> Option<u8> {
    let available = std::fs::read_to_string("/proc/sys/net/ipv4/tcp_available_congestion_control").ok()?;
    if !available.split_whitespace().any(|n| n == "bbr") {
        return None;
    }
    // SAFETY: socket(2); the descriptor is owned by the OwnedFd made of it.
    let socket = unsafe {
        use std::os::fd::FromRawFd as _;
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return None;
        }
        std::os::fd::OwnedFd::from_raw_fd(fd)
    };
    set_congestion_control(&socket, "bbr").ok()?;
    let mut buf = [0u8; 128];
    let len = getsockopt(&socket, TCP_CC_INFO, &mut buf).ok()?;
    Some(bbr_version_of(&buf[..len]))
}

/// (`tcp_bbr_info` with Google's patches: BBRv1's five words, four more
/// (the bandwidth's bounds), then a byte each for the mode, the phase,
/// padding and the version.)
fn bbr_version_of(info: &[u8]) -> u8 {
    info.get(39).copied().filter(|&v| v > 1).unwrap_or(1)
}

fn getsockopt(socket: &impl AsRawFd, what: libc::c_int, buf: &mut [u8]) -> io::Result<usize> {
    let mut len = buf.len() as libc::socklen_t;
    // SAFETY: getsockopt(2) writes at most `len` bytes into `buf` and
    // says how many.
    let r = unsafe { libc::getsockopt(socket.as_raw_fd(), libc::IPPROTO_TCP, what, buf.as_mut_ptr().cast(), &mut len) };
    if r == 0 { Ok((len as usize).min(buf.len())) } else { Err(io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
    };

    /// The kernel's own bytes for a real connection read back as what
    /// was done on it; a short answer (an older kernel's) reads as far
    /// as it goes.
    #[test]
    fn a_connections_figures_are_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(&[7u8; 30_000]).unwrap();
        let mut got = vec![0u8; 30_000];
        server.read_exact(&mut got).unwrap();
        // (The last ACK is on its way back: a moment.)
        let mut info = TcpInfo::of(&client).unwrap();
        for _ in 0..200 {
            if info.bytes_acked > 30_000 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            info = TcpInfo::of(&client).unwrap();
        }
        assert_eq!(info.state, 1, "established");
        assert_eq!(info.bytes_acked, 30_001, "the data and the SYN");
        assert_eq!(info.bytes_sent, 30_000);
        assert_eq!(info.ca_state(), CaState::Open);
        assert!(info.snd_mss > 1000 && info.snd_cwnd >= 2 && info.rtt_us > 0 && info.min_rtt_us > 0, "{info:?}");
        assert!(info.options & options::SACK != 0);
        assert_eq!(info.in_flight(), 0);
        assert_eq!(TcpInfo::of(&server).unwrap().bytes_received, 30_000);

        let mut raw = [0u8; 512];
        let len = getsockopt(&client, TCP_INFO, &mut raw).unwrap();
        assert!(len >= 232, "this kernel's tcp_info is {len} bytes");
        let short = TcpInfo::parse(&raw[..104]);
        assert_eq!((short.snd_cwnd, short.bytes_acked, short.delivery_rate), (info.snd_cwnd, 0, 0));
    }

    #[test]
    fn the_congestion_control_is_read_and_set() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let default = congestion_control(&client).unwrap();
        assert!(!default.is_empty() && !default.contains('\0'), "{default:?}");
        // (Reno is in every kernel, and anyone's to choose.)
        set_congestion_control(&client, "reno").unwrap();
        assert_eq!(congestion_control(&client).unwrap(), "reno");
        assert_eq!(bbr_info(&client).unwrap(), None);
        assert!(set_congestion_control(&client, "no-such-control").is_err());
    }

    #[test]
    fn bbrs_model_is_read_and_its_phase_told() {
        let mut b = Vec::new();
        for v in [0x9000_0000u32, 1, 23_000, 320, 512] {
            b.extend_from_slice(&v.to_ne_bytes());
        }
        let info = BbrInfo::parse(&b).unwrap();
        assert_eq!((info.bandwidth, info.min_rtt_us), (0x1_9000_0000, 23_000));
        assert_eq!((info.pacing_gain, info.cwnd_gain), (1.25, 2.0));
        assert_eq!(info.phase(), "probe up");
        let phase = |pacing: u32, cwnd: u32| BbrInfo { bandwidth: 1, min_rtt_us: 1, pacing_gain: pacing as f32 / 256.0, cwnd_gain: cwnd as f32 / 256.0 }.phase();
        assert_eq!([phase(710, 512), phase(88, 512), phase(256, 256), phase(232, 512), phase(192, 512), phase(256, 512)], ["startup", "drain", "probe rtt", "probe down", "probe down", "cruise"]);
        assert_eq!(BbrInfo::parse(&b[..16]), None);
    }

    /// BBRv1's answer is its five words; a kernel with Google's patches
    /// says more, its version among it.
    #[test]
    fn the_kernels_bbr_says_its_version() {
        assert_eq!(bbr_version_of(&[0u8; 20]), 1);
        let mut v3 = [0u8; 52];
        v3[39] = 3;
        assert_eq!(bbr_version_of(&v3), 3);
        assert_eq!(bbr_version_of(&[]), 1);
        // (Whatever this kernel has: no "bbr" loaded, or one that answers.)
        assert!(kernels_bbr_version().is_none_or(|v| v >= 1));
    }
}
