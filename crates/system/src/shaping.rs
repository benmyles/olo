//! Traffic shaping, as a Mac's Network Link Conditioner does it: a link
//! made slower, later and lossier than it is, each way, to try a program
//! on a poor network. The kernel's traffic control does it, spoken to over
//! rtnetlink here (no `tc`): a `netem` queue on the link for what's sent
//! (its rate, delay, jitter and loss), and for what's received an `ifb`
//! device of Olo's own (`olo-ifbN`, N the link's index) that the link's
//! ingress hands everything to (a `matchall` filter with a `mirred`
//! redirect), with its own `netem`. Setting it needs root
//! (`olo-net-admin`); reading it doesn't. It lasts until turned off, the
//! link goes away or the computer restarts.

use std::io;

use serde::{Deserialize, Serialize};

/// One way's shaping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Kbit/s; 0 for as fast as the link goes.
    pub rate_kbit: u64,
    /// Added to every packet's way, in ms.
    pub delay_ms: u32,
    /// The delay varies by up to this, in ms.
    pub jitter_ms: u32,
    /// Packets dropped, in percent.
    pub loss: f32,
}

impl Shape {
    pub fn is_nothing(&self) -> bool {
        self.rate_kbit == 0 && self.delay_ms == 0 && self.jitter_ms == 0 && self.loss <= 0.0
    }

    pub fn check(&self) -> Result<(), String> {
        if !(0.0..=100.0).contains(&self.loss) || self.loss.is_nan() {
            return Err("loss is between 0 and 100 %".into());
        }
        if self.delay_ms > 60_000 || self.jitter_ms > 60_000 {
            return Err("a delay is at most a minute".into());
        }
        if self.jitter_ms > self.delay_ms {
            return Err("the jitter is at most the delay".into());
        }
        Ok(())
    }
}

/// A link's shaping, each way (None: as it is).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Shaping {
    /// What the link receives.
    pub down: Option<Shape>,
    /// What it sends.
    pub up: Option<Shape>,
}

impl Shaping {
    pub fn is_on(&self) -> bool {
        self.down.is_some() || self.up.is_some()
    }
}

/// Networks to try a program on, as a link conditioner offers them.
pub struct Preset {
    pub name: &'static str,
    pub down: Shape,
    pub up: Shape,
}

const fn shape(rate_kbit: u64, delay_ms: u32, loss: f32) -> Shape {
    Shape { rate_kbit, delay_ms, jitter_ms: 0, loss }
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Edge", down: shape(240, 400, 0.0), up: shape(200, 440, 0.0) },
    Preset { name: "3G", down: shape(780, 100, 0.0), up: shape(330, 100, 0.0) },
    Preset { name: "LTE", down: shape(50_000, 50, 0.0), up: shape(10_000, 65, 0.0) },
    Preset { name: "DSL", down: shape(2_000, 5, 0.0), up: shape(256, 5, 0.0) },
    Preset { name: "Busy Wi-Fi", down: shape(20_000, 20, 2.0), up: shape(10_000, 20, 2.0) },
    Preset { name: "Very Bad Network", down: shape(1_000, 500, 10.0), up: shape(1_000, 500, 10.0) },
    Preset { name: "Offline (Everything Lost)", down: shape(0, 0, 100.0), up: shape(0, 0, 100.0) },
];

/// The device that carries what `interface` receives, while shaped.
pub fn ifb_name(index: u32) -> String {
    format!("olo-ifb{index}")
}

fn index_of(name: &str) -> io::Result<u32> {
    let c = std::ffi::CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a NUL-terminated name.
    match unsafe { libc::if_nametoindex(c.as_ptr()) } {
        0 => Err(io::Error::new(io::ErrorKind::NotFound, format!("no link {name}"))),
        i => Ok(i),
    }
}

/// How `interface` is shaped now (read, as anyone may).
pub fn read(interface: &str) -> io::Result<Shaping> {
    let index = index_of(interface)?;
    let ifb = index_of(&ifb_name(index)).ok();
    let mut sock = nl::Socket::open()?;
    let mut out = Shaping::default();
    for q in sock.qdiscs()? {
        if q.parent != TC_H_ROOT || q.kind != "netem" {
            continue;
        }
        if q.index == index {
            out.up = q.netem;
        } else if Some(q.index) == ifb {
            out.down = q.netem;
        }
    }
    Ok(out)
}

/// Shape `interface` as `s` says (root): what was Olo's taken away first;
/// nothing else of the link's touched unless shaped.
pub fn apply(interface: &str, s: &Shaping) -> io::Result<()> {
    for shape in [s.down, s.up].into_iter().flatten() {
        shape.check().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    }
    clear(interface)?;
    let index = index_of(interface)?;
    let mut sock = nl::Socket::open()?;
    if let Some(up) = s.up {
        sock.netem(index, &up)?;
    }
    if let Some(down) = s.down {
        let name = ifb_name(index);
        sock.new_ifb(&name).map_err(|e| explain(e, "ifb"))?;
        let ifb = index_of(&name)?;
        sock.set_up(ifb)?;
        sock.netem(ifb, &down)?;
        sock.ingress(index)?;
        sock.redirect(index, ifb).map_err(|e| explain(e, "cls_matchall and act_mirred"))?;
    }
    Ok(())
}

/// Take away Olo's shaping of `interface` (root).
pub fn clear(interface: &str) -> io::Result<()> {
    let index = index_of(interface)?;
    let mut sock = nl::Socket::open()?;
    let ifb = index_of(&ifb_name(index)).ok();
    for q in sock.qdiscs()? {
        // Its own netem at the root, and the ingress queue that hands to
        // Olo's ifb (with its filter).
        if q.index == index && ((q.parent == TC_H_ROOT && q.kind == "netem") || (q.parent == TC_H_INGRESS && ifb.is_some())) {
            sock.del_qdisc(index, q.parent)?;
        }
    }
    if let Some(ifb) = ifb {
        sock.del_link(ifb)?;
    }
    Ok(())
}

/// Says which part of the kernel is missing when it is.
fn explain(e: io::Error, modules: &str) -> io::Error {
    match e.raw_os_error() {
        Some(libc::ENOENT) | Some(libc::EOPNOTSUPP) => io::Error::new(e.kind(), format!("{e} (the kernel's {modules} isn't there)")),
        _ => e,
    }
}

const TC_H_ROOT: u32 = 0xFFFF_FFFF;
const TC_H_INGRESS: u32 = 0xFFFF_FFF1;

/// rtnetlink, as much as shaping needs.
mod nl {
    use std::{
        io,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };

    use super::{Shape, TC_H_INGRESS, TC_H_ROOT};

    const RTM_NEWLINK: u16 = 16;
    const RTM_DELLINK: u16 = 17;
    const RTM_NEWQDISC: u16 = 36;
    const RTM_DELQDISC: u16 = 37;
    const RTM_GETQDISC: u16 = 38;
    const RTM_NEWTFILTER: u16 = 44;
    const NLMSG_ERROR: u16 = 2;
    const NLMSG_DONE: u16 = 3;
    const F_REQUEST: u16 = 0x1;
    const F_ACK: u16 = 0x4;
    const F_DUMP: u16 = 0x300;
    const F_REPLACE: u16 = 0x100;
    const F_EXCL: u16 = 0x200;
    const F_CREATE: u16 = 0x400;
    const NESTED: u16 = 0x8000;
    const TCA_KIND: u16 = 1;
    const TCA_OPTIONS: u16 = 2;
    const IFLA_IFNAME: u16 = 3;
    const IFLA_LINKINFO: u16 = 18;
    const IFLA_INFO_KIND: u16 = 1;
    const TCA_NETEM_RATE: u16 = 6;
    const TCA_NETEM_RATE64: u16 = 8;
    const TCA_NETEM_LATENCY64: u16 = 10;
    const TCA_NETEM_JITTER64: u16 = 11;
    const TCA_MATCHALL_ACT: u16 = 2;
    const TCA_ACT_KIND: u16 = 1;
    const TCA_ACT_OPTIONS: u16 = 2;
    const TCA_MIRRED_PARMS: u16 = 2;
    const TC_ACT_STOLEN: i32 = 4;
    const TCA_EGRESS_REDIR: i32 = 1;
    const ETH_P_ALL: u16 = 0x0003;
    /// The ingress queue's handle (ffff:).
    const INGRESS_HANDLE: u32 = 0xFFFF_0000;

    /// A message's body: its fixed part, then attributes.
    pub(super) struct Body {
        buf: Vec<u8>,
        open: Vec<usize>,
    }

    impl Body {
        fn new(fixed: &[u8]) -> Body {
            Body { buf: fixed.to_vec(), open: Vec::new() }
        }

        fn pad(&mut self) {
            while !self.buf.len().is_multiple_of(4) {
                self.buf.push(0);
            }
        }

        fn attr(&mut self, kind: u16, data: &[u8]) -> &mut Self {
            self.buf.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
            self.buf.extend_from_slice(&kind.to_ne_bytes());
            self.buf.extend_from_slice(data);
            self.pad();
            self
        }

        fn text(&mut self, kind: u16, s: &str) -> &mut Self {
            let mut d = s.as_bytes().to_vec();
            d.push(0);
            self.attr(kind, &d)
        }

        /// An attribute holding attributes (or, `nested` false, a struct
        /// and then attributes: netem's options).
        fn begin(&mut self, kind: u16, nested: bool) -> &mut Self {
            self.open.push(self.buf.len());
            self.buf.extend_from_slice(&0u16.to_ne_bytes());
            self.buf.extend_from_slice(&(if nested { kind | NESTED } else { kind }).to_ne_bytes());
            self
        }

        fn raw(&mut self, data: &[u8]) -> &mut Self {
            self.buf.extend_from_slice(data);
            self.pad();
            self
        }

        fn end(&mut self) -> &mut Self {
            let at = self.open.pop().expect("an attribute begun");
            let len = (self.buf.len() - at) as u16;
            self.buf[at..at + 2].copy_from_slice(&len.to_ne_bytes());
            self
        }
    }

    fn tcmsg(index: u32, handle: u32, parent: u32, info: u32) -> Vec<u8> {
        let mut m = vec![0u8; 4];
        m.extend_from_slice(&(index as i32).to_ne_bytes());
        m.extend_from_slice(&handle.to_ne_bytes());
        m.extend_from_slice(&parent.to_ne_bytes());
        m.extend_from_slice(&info.to_ne_bytes());
        m
    }

    fn ifinfomsg(index: u32, flags: u32, change: u32) -> Vec<u8> {
        let mut m = vec![0u8; 4];
        m.extend_from_slice(&(index as i32).to_ne_bytes());
        m.extend_from_slice(&flags.to_ne_bytes());
        m.extend_from_slice(&change.to_ne_bytes());
        m
    }

    /// netem's options: `tc_netem_qopt` (the delay, jitter and loss; the
    /// times again in ns, as the kernel takes them now), then the rate.
    pub(super) fn netem_options(b: &mut Body, s: &Shape) {
        let loss = ((s.loss.clamp(0.0, 100.0) as f64 / 100.0) * u32::MAX as f64).round() as u32;
        let mut q = Vec::new();
        for v in [0u32, 1000, loss, 0, 0, 0] {
            q.extend_from_slice(&v.to_ne_bytes());
        }
        b.begin(TCA_OPTIONS, false).raw(&q);
        b.attr(TCA_NETEM_LATENCY64, &(s.delay_ms as i64 * 1_000_000).to_ne_bytes());
        b.attr(TCA_NETEM_JITTER64, &(s.jitter_ms as i64 * 1_000_000).to_ne_bytes());
        if s.rate_kbit > 0 {
            let bytes = s.rate_kbit * 1000 / 8;
            let mut r = Vec::new();
            for v in [bytes.min(u32::MAX as u64) as u32, 0, 0, 0] {
                r.extend_from_slice(&v.to_ne_bytes());
            }
            b.attr(TCA_NETEM_RATE, &r);
            if bytes >= u32::MAX as u64 {
                b.attr(TCA_NETEM_RATE64, &bytes.to_ne_bytes());
            }
        }
        b.end();
    }

    /// A qdisc as a dump says it.
    #[derive(Debug)]
    pub(super) struct Qdisc {
        pub index: u32,
        pub parent: u32,
        pub kind: String,
        pub netem: Option<Shape>,
    }

    fn attrs(mut data: &[u8]) -> Vec<(u16, &[u8])> {
        let mut out = Vec::new();
        while data.len() >= 4 {
            let len = u16::from_ne_bytes([data[0], data[1]]) as usize;
            let kind = u16::from_ne_bytes([data[2], data[3]]) & 0x3FFF;
            if len < 4 || len > data.len() {
                break;
            }
            out.push((kind, &data[4..len]));
            data = &data[len.next_multiple_of(4).min(data.len())..];
        }
        out
    }

    fn u32_at(d: &[u8], at: usize) -> u32 {
        d.get(at..at + 4).map(|b| u32::from_ne_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    fn u64_of(d: &[u8]) -> u64 {
        d.get(..8).map(|b| u64::from_ne_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    /// netem's options read back.
    pub(super) fn parse_netem(opts: &[u8]) -> Shape {
        let loss = u32_at(opts, 8) as f64 / u32::MAX as f64 * 100.0;
        let mut s = Shape { loss: (loss * 100.0).round() as f32 / 100.0, ..Shape::default() };
        // (The kernel's ticks, where it gives no nanoseconds.)
        let mut delay_ns = u32_at(opts, 0) as u64 * 64;
        let mut jitter_ns = u32_at(opts, 20) as u64 * 64;
        for (kind, d) in attrs(opts.get(24..).unwrap_or_default()) {
            match kind {
                TCA_NETEM_LATENCY64 => delay_ns = u64_of(d),
                TCA_NETEM_JITTER64 => jitter_ns = u64_of(d),
                TCA_NETEM_RATE if s.rate_kbit == 0 => s.rate_kbit = u32_at(d, 0) as u64 * 8 / 1000,
                TCA_NETEM_RATE64 => s.rate_kbit = u64_of(d) * 8 / 1000,
                _ => {}
            }
        }
        s.delay_ms = (delay_ns / 1_000_000) as u32;
        s.jitter_ms = (jitter_ns / 1_000_000) as u32;
        s
    }

    pub(super) struct Socket {
        fd: OwnedFd,
        seq: u32,
    }

    impl Socket {
        pub fn open() -> io::Result<Socket> {
            // SAFETY: plain socket calls; the fd is owned at once.
            unsafe {
                let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_ROUTE);
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let fd = OwnedFd::from_raw_fd(fd);
                let mut addr: libc::sockaddr_nl = std::mem::zeroed();
                addr.nl_family = libc::AF_NETLINK as u16;
                if libc::bind(fd.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as u32) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Socket { fd, seq: 1 })
            }
        }

        fn send(&mut self, kind: u16, flags: u16, body: &Body) -> io::Result<u32> {
            self.seq += 1;
            let mut m = Vec::with_capacity(16 + body.buf.len());
            m.extend_from_slice(&((16 + body.buf.len()) as u32).to_ne_bytes());
            m.extend_from_slice(&kind.to_ne_bytes());
            m.extend_from_slice(&flags.to_ne_bytes());
            m.extend_from_slice(&self.seq.to_ne_bytes());
            m.extend_from_slice(&0u32.to_ne_bytes());
            m.extend_from_slice(&body.buf);
            // SAFETY: a buffer of its length.
            let n = unsafe { libc::send(self.fd.as_raw_fd(), m.as_ptr() as *const libc::c_void, m.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(self.seq)
        }

        /// The replies to `seq` until its ack or the dump's end: each
        /// message's type and body.
        fn replies(&mut self, seq: u32) -> io::Result<Vec<(u16, Vec<u8>)>> {
            let mut out = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                // SAFETY: into a buffer of its length.
                let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                let mut data = &buf[..n as usize];
                while data.len() >= 16 {
                    let len = u32_at(data, 0) as usize;
                    let kind = u16::from_ne_bytes([data[4], data[5]]);
                    let their = u32_at(data, 8);
                    if len < 16 || len > data.len() {
                        break;
                    }
                    let body = &data[16..len];
                    if their == seq {
                        match kind {
                            NLMSG_DONE => return Ok(out),
                            NLMSG_ERROR => {
                                let errno = -(u32_at(body, 0) as i32);
                                return if errno == 0 { Ok(out) } else { Err(io::Error::from_raw_os_error(errno)) };
                            }
                            _ => out.push((kind, body.to_vec())),
                        }
                    }
                    data = &data[len.next_multiple_of(4).min(data.len())..];
                }
            }
        }

        fn request(&mut self, kind: u16, flags: u16, body: &Body) -> io::Result<()> {
            let seq = self.send(kind, flags | F_REQUEST | F_ACK, body)?;
            self.replies(seq).map(drop)
        }

        pub fn qdiscs(&mut self) -> io::Result<Vec<Qdisc>> {
            let seq = self.send(RTM_GETQDISC, F_REQUEST | F_DUMP, &Body::new(&tcmsg(0, 0, 0, 0)))?;
            let mut out = Vec::new();
            for (_, body) in self.replies(seq)? {
                if body.len() < 20 {
                    continue;
                }
                let (index, parent) = (u32_at(&body, 4), u32_at(&body, 12));
                let (mut kind, mut netem) = (String::new(), None);
                let mut opts: &[u8] = &[];
                for (k, d) in attrs(&body[20..]) {
                    match k {
                        TCA_KIND => kind = String::from_utf8_lossy(d.split(|b| *b == 0).next().unwrap_or_default()).into_owned(),
                        TCA_OPTIONS => opts = d,
                        _ => {}
                    }
                }
                if kind == "netem" {
                    netem = Some(parse_netem(opts));
                }
                out.push(Qdisc { index, parent, kind, netem });
            }
            Ok(out)
        }

        pub fn netem(&mut self, index: u32, s: &Shape) -> io::Result<()> {
            let mut b = Body::new(&tcmsg(index, 0, TC_H_ROOT, 0));
            b.text(TCA_KIND, "netem");
            netem_options(&mut b, s);
            self.request(RTM_NEWQDISC, F_CREATE | F_REPLACE, &b)
        }

        pub fn ingress(&mut self, index: u32) -> io::Result<()> {
            let mut b = Body::new(&tcmsg(index, INGRESS_HANDLE, TC_H_INGRESS, 0));
            b.text(TCA_KIND, "ingress");
            match self.request(RTM_NEWQDISC, F_CREATE | F_EXCL, &b) {
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Ok(()),
                r => r,
            }
        }

        /// Everything `index` receives handed to `to`.
        pub fn redirect(&mut self, index: u32, to: u32) -> io::Result<()> {
            let info = (1u32 << 16) | ETH_P_ALL.to_be() as u32;
            let mut b = Body::new(&tcmsg(index, 0, INGRESS_HANDLE, info));
            b.text(TCA_KIND, "matchall");
            b.begin(TCA_OPTIONS, true).begin(TCA_MATCHALL_ACT, true).begin(1, true);
            b.text(TCA_ACT_KIND, "mirred");
            let mut parms = Vec::new();
            for v in [0i32, 0, TC_ACT_STOLEN, 0, 0, TCA_EGRESS_REDIR, to as i32] {
                parms.extend_from_slice(&v.to_ne_bytes());
            }
            b.begin(TCA_ACT_OPTIONS, true).attr(TCA_MIRRED_PARMS, &parms).end();
            b.end().end().end();
            self.request(RTM_NEWTFILTER, F_CREATE | F_EXCL, &b)
        }

        pub fn del_qdisc(&mut self, index: u32, parent: u32) -> io::Result<()> {
            let handle = if parent == TC_H_INGRESS { INGRESS_HANDLE } else { 0 };
            self.request(RTM_DELQDISC, 0, &Body::new(&tcmsg(index, handle, parent, 0)))
        }

        pub fn new_ifb(&mut self, name: &str) -> io::Result<()> {
            let mut b = Body::new(&ifinfomsg(0, 0, 0));
            b.text(IFLA_IFNAME, name);
            b.begin(IFLA_LINKINFO, true).text(IFLA_INFO_KIND, "ifb").end();
            self.request(RTM_NEWLINK, F_CREATE | F_EXCL, &b)
        }

        pub fn set_up(&mut self, index: u32) -> io::Result<()> {
            let up = libc::IFF_UP as u32;
            self.request(RTM_NEWLINK, 0, &Body::new(&ifinfomsg(index, up, up)))
        }

        pub fn del_link(&mut self, index: u32) -> io::Result<()> {
            self.request(RTM_DELLINK, 0, &Body::new(&ifinfomsg(index, 0, 0)))
        }
    }

    #[cfg(test)]
    pub(super) fn body_bytes(b: &Body) -> &[u8] {
        &b.buf
    }

    #[cfg(test)]
    pub(super) fn new_body() -> Body {
        Body::new(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netem_options_read_back() {
        for s in [shape(780, 100, 0.0), Shape { rate_kbit: 40_000_000, delay_ms: 20, jitter_ms: 5, loss: 2.5 }, shape(0, 0, 100.0)] {
            let mut b = nl::new_body();
            nl::netem_options(&mut b, &s);
            let bytes = nl::body_bytes(&b);
            // The attribute's own length, then its payload.
            assert_eq!(u16::from_ne_bytes([bytes[0], bytes[1]]) as usize, bytes.len());
            assert_eq!(nl::parse_netem(&bytes[4..]), s);
        }
    }

    #[test]
    fn presets_and_checks() {
        for p in PRESETS {
            assert!(p.down.check().is_ok() && p.up.check().is_ok(), "{}", p.name);
        }
        assert!(Shape { loss: 101.0, ..Shape::default() }.check().is_err());
        assert!(Shape { delay_ms: 10, jitter_ms: 20, ..Shape::default() }.check().is_err());
        assert!(Shape::default().is_nothing());
        assert_eq!(ifb_name(7), "olo-ifb7");
        // (Within IFNAMSIZ for any index a machine has.)
        assert!(ifb_name(99_999_999).len() <= 15);
    }

    /// The loopback's queues are read without root.
    #[test]
    fn reads_without_root() {
        assert_eq!(read("lo").map(|s| s.is_on()).ok(), Some(false));
        assert!(read("no-such-link").is_err());
    }
}
