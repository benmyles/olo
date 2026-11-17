//! The test itself: what the connection carries each way, and what can
//! be measured or read of it as it does.
//!
//! Against Cloudflare's speed test endpoints (`speed.cloudflare.com`:
//! `GET /__down?bytes=N` sends N bytes, `POST /__up` takes what's sent;
//! the nearest of its data centres answers, and says which), over
//! connections of its own ([`crate::http`]), in this order:
//!
//! 1. **Connecting**: the name looked up, one connection opened (TCP's
//!    handshake and TLS's, each timed), and where the server is.
//! 2. **Latency**, the line idle: the round trip of a request with no
//!    body, [`Config::idle_probes`] times; and TCP's own least round
//!    trip, which has no server's work in it.
//! 3. **Download**, then **Upload**: [`Config::streams`] connections at
//!    once for [`Config::download`] and [`Config::upload`], each asking
//!    for (sending) piece after piece, sized to take a few seconds (each
//!    connection its own time: a piece's end is a round trip with
//!    nothing moving, and four ending together showed as a dip in the
//!    rate every two seconds); one
//!    more connection goes on measuring the round trip meanwhile (the
//!    line loaded: how much later everything else arrives while it's
//!    busy). Ten times a second the bytes moved are counted (down, as
//!    they're read; up, as the kernel says the server has acknowledged
//!    them: what's written to a socket has only reached its buffer),
//!    and the kernel asked what it knows of each socket (`olo_net::tcp_info`:
//!    sending, the window, the pacing and delivery rates, the round
//!    trip, what was sent again, BBR's model; receiving, what arrived
//!    out of order). The server says its own side of each connection's
//!    TCP with every answer (`Server-Timing`'s `cfL4`): what it sent
//!    again and lost on the way down.
//!
//! Everything is told as it happens ([`Update`]), and stopped at once
//! when asked ([`run`]'s `stop`): the connections are cut, and what was
//! measured so far is the result.

use crate::http::{Connection, Opening, Response, Server};
use olo_net::tcp_info::{self, TcpInfo};
use std::{
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Config {
    pub server: Server,
    /// Connections at once, each way.
    pub streams: usize,
    pub download: Duration,
    pub upload: Duration,
    /// Round trips measured with the line idle.
    pub idle_probes: usize,
    /// How long anything waits for the server before giving up.
    pub patience: Duration,
}

impl Default for Config {
    fn default() -> Config {
        // (For trying it without the internet: a server of Olo's own on
        // this computer, `olo-speedtest --serve`.)
        let local = std::env::var("OLO_SPEEDTEST_SERVER").ok().and_then(|s| s.rsplit_once(':').and_then(|(host, port)| Some(Server { host: host.to_owned(), port: port.parse().ok()?, tls: false })));
        Config {
            server: local.unwrap_or(Server { host: "speed.cloudflare.com".into(), port: 443, tls: true }),
            streams: 4,
            download: Duration::from_secs(8),
            upload: Duration::from_secs(8),
            idle_probes: 20,
            patience: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Phase {
    Connecting,
    Latency,
    Download,
    Upload,
}

/// What happens, as it happens.
#[derive(Debug, Clone)]
pub enum Update {
    Phase(Phase),
    /// The first connection is open: where to, and how long it took.
    Opened {
        address: String,
        place: Place,
        opening: Opening,
    },
    /// A round trip measured, idle or during a transfer.
    Latency {
        ms: f64,
    },
    /// The idle round trips, summed up, and TCP's own least.
    Idle {
        latency: Latency,
        tcp_rtt_ms: f64,
    },
    /// One way's transfer is over: all of it.
    Measured {
        phase: Phase,
        transfer: Box<Transfer>,
    },
    /// The rate over the last tenth of a second, `at` seconds in.
    Rate {
        phase: Phase,
        at: f64,
        mbps: f64,
    },
    /// The kernel's figures for the transfer's sockets, as of now.
    Tcp {
        phase: Phase,
        ours: Ours,
    },
}

/// Where the server that answered is, as it says.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Place {
    /// Its data centre's code (an airport's: "SEA").
    pub colo: String,
    /// Where it takes this computer to be, and through whose network
    /// (the autonomous system's number).
    pub city: String,
    pub country: String,
    pub asn: String,
}

/// Round trips, in milliseconds.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Latency {
    pub samples: usize,
    pub min: f64,
    pub median: f64,
    pub mean: f64,
    pub p90: f64,
    pub max: f64,
    /// How much one differs from the next, on average.
    pub jitter: f64,
}

impl Latency {
    pub fn of(ms: &[f64]) -> Latency {
        if ms.is_empty() {
            return Latency::default();
        }
        let mut sorted = ms.to_vec();
        sorted.sort_by(f64::total_cmp);
        let pick = |q: f64| sorted[((sorted.len() as f64 - 1.0) * q).round() as usize];
        let jitter = if ms.len() > 1 { ms.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (ms.len() - 1) as f64 } else { 0.0 };
        Latency { samples: ms.len(), min: sorted[0], median: pick(0.5), mean: ms.iter().sum::<f64>() / ms.len() as f64, p90: pick(0.9), max: sorted[sorted.len() - 1], jitter }
    }
}

/// BBR's model of the path, over the transfer's connections.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bbr {
    /// The bottleneck's bandwidth as estimated, the connections' summed.
    pub bandwidth_mbps: f64,
    pub min_rtt_ms: f64,
    /// What each connection is doing ("cruise", "probe up").
    pub phases: Vec<String>,
}

/// The kernel's figures for this computer's end of a transfer's
/// connections: summed over them where they add up (bytes, packets,
/// rates, windows), the least or the mean where they don't, as each
/// says.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ours {
    pub congestion_control: String,
    /// The segment size and the path's MTU, bytes.
    pub mss: u32,
    pub pmtu: u32,
    /// TCP's round trip: the least seen, the smoothed one (the
    /// connections' mean) and its deviation.
    pub min_rtt_ms: f64,
    pub rtt_ms: f64,
    pub rttvar_ms: f64,
    // Sending:
    pub bytes_sent: u64,
    pub bytes_retransmitted: u64,
    /// Packets sent again, and how many times a timeout did it.
    pub retransmissions: u32,
    pub timeouts: u32,
    /// Retransmissions the peer said it hadn't needed, and times packets
    /// were seen to arrive out of order.
    pub spurious: u32,
    pub reordering: u32,
    /// The congestion windows (packets), the pacing rates and the
    /// delivery rates, summed.
    pub cwnd: u32,
    pub pacing_mbps: f64,
    pub delivery_mbps: f64,
    /// Of the time with data to send: held by the peer's window, and by
    /// this end's send buffer (percent).
    pub rwnd_limited_pct: f64,
    pub sndbuf_limited_pct: f64,
    pub bbr: Option<Bbr>,
    // Receiving:
    pub bytes_received: u64,
    /// Packets that arrived out of order (a hole before them: lost on
    /// the way, or overtaken).
    pub out_of_order: u32,
    /// The receiver's own estimate of the round trip, and its window.
    pub rcv_rtt_ms: f64,
    pub rcv_window: u32,
}

impl Ours {
    /// Of what was sent, the share sent again (percent).
    pub fn retransmitted_pct(&self) -> f64 {
        if self.bytes_sent == 0 { 0.0 } else { self.bytes_retransmitted as f64 * 100.0 / self.bytes_sent as f64 }
    }

    /// The connections' figures made one.
    fn of(sockets: &[(TcpInfo, String, Option<tcp_info::BbrInfo>)]) -> Ours {
        let mut o = Ours::default();
        let n = sockets.len().max(1) as f64;
        let (mut busy, mut rwnd, mut sndbuf) = (0u64, 0u64, 0u64);
        o.min_rtt_ms = f64::MAX;
        for (i, cc, bbr) in sockets {
            if o.congestion_control.is_empty() {
                o.congestion_control = cc.clone();
            }
            o.mss = o.mss.max(i.snd_mss);
            o.pmtu = o.pmtu.max(i.pmtu);
            if i.min_rtt_us > 0 {
                o.min_rtt_ms = o.min_rtt_ms.min(i.min_rtt_us as f64 / 1000.0);
            }
            o.rtt_ms += i.rtt_us as f64 / 1000.0 / n;
            o.rttvar_ms += i.rttvar_us as f64 / 1000.0 / n;
            o.bytes_sent += i.bytes_sent;
            o.bytes_retransmitted += i.bytes_retrans;
            o.retransmissions += i.total_retrans;
            o.timeouts += i.total_rto as u32;
            o.spurious += i.dsack_dups;
            o.reordering += i.reord_seen;
            o.cwnd += i.snd_cwnd;
            // (A pacing rate of "none" is the kernel's largest number.)
            if i.pacing_rate < u64::MAX / 2 {
                o.pacing_mbps += i.pacing_rate as f64 * 8.0 / 1e6;
            }
            o.delivery_mbps += i.delivery_rate as f64 * 8.0 / 1e6;
            busy += i.busy_time_us;
            rwnd += i.rwnd_limited_us;
            sndbuf += i.sndbuf_limited_us;
            o.bytes_received += i.bytes_received;
            o.out_of_order += i.rcv_ooopack;
            o.rcv_rtt_ms += i.rcv_rtt_us as f64 / 1000.0 / n;
            o.rcv_window += i.rcv_wnd;
            if let Some(b) = bbr {
                let all = o.bbr.get_or_insert_with(|| Bbr { min_rtt_ms: f64::MAX, ..Bbr::default() });
                all.bandwidth_mbps += b.bandwidth as f64 * 8.0 / 1e6;
                all.min_rtt_ms = all.min_rtt_ms.min(b.min_rtt_us as f64 / 1000.0);
                all.phases.push(b.phase().to_owned());
            }
        }
        if o.min_rtt_ms == f64::MAX {
            o.min_rtt_ms = 0.0;
        }
        if busy > 0 {
            o.rwnd_limited_pct = rwnd as f64 * 100.0 / busy as f64;
            o.sndbuf_limited_pct = sndbuf as f64 * 100.0 / busy as f64;
        }
        o
    }
}

/// The server's side of a transfer's connections, as it said with its
/// last answer on each (`Server-Timing`'s `cfL4`): its packets summed,
/// its round trips the connections' mean and least.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Theirs {
    pub rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub rttvar_ms: f64,
    /// Packets it sent and received, those it took for lost, and those
    /// it sent again.
    pub sent: u64,
    pub received: u64,
    pub lost: u64,
    pub retransmissions: u64,
    pub delivery_mbps: f64,
    pub cwnd: u64,
}

impl Theirs {
    /// One connection's, from a response's `Server-Timing` headers.
    pub fn parse(response: &Response) -> Option<Theirs> {
        let desc = response.headers("server-timing").find_map(|v| v.split("cfL4").nth(1))?;
        let query = desc.split('"').nth(1)?.trim_start_matches('?');
        let get = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('='))).and_then(|v| v.parse::<f64>().ok());
        Some(Theirs {
            rtt_ms: get("rtt")? / 1000.0,
            min_rtt_ms: get("min_rtt").unwrap_or(0.0) / 1000.0,
            rttvar_ms: get("rtt_var").unwrap_or(0.0) / 1000.0,
            sent: get("sent").unwrap_or(0.0) as u64,
            received: get("recv").unwrap_or(0.0) as u64,
            lost: get("lost").unwrap_or(0.0) as u64,
            retransmissions: get("retrans").unwrap_or(0.0) as u64,
            delivery_mbps: get("delivery_rate").unwrap_or(0.0) * 8.0 / 1e6,
            cwnd: get("cwnd").unwrap_or(0.0) as u64,
        })
    }

    fn of(each: &[Theirs]) -> Option<Theirs> {
        if each.is_empty() {
            return None;
        }
        let n = each.len() as f64;
        let mut all = Theirs { min_rtt_ms: f64::MAX, ..Theirs::default() };
        for t in each {
            all.rtt_ms += t.rtt_ms / n;
            all.rttvar_ms += t.rttvar_ms / n;
            all.min_rtt_ms = all.min_rtt_ms.min(t.min_rtt_ms);
            all.sent += t.sent;
            all.received += t.received;
            all.lost += t.lost;
            all.retransmissions += t.retransmissions;
            all.delivery_mbps += t.delivery_mbps;
            all.cwnd += t.cwnd;
        }
        Some(all)
    }

    /// Of the packets it sent, the share sent again (percent).
    pub fn retransmitted_pct(&self) -> f64 {
        if self.sent == 0 { 0.0 } else { self.retransmissions as f64 * 100.0 / self.sent as f64 }
    }
}

/// One way's transfer.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transfer {
    pub secs: f64,
    pub bytes: u64,
    /// The rate once it had got going (the first fifth of the time left
    /// out), and the best second.
    pub mbps: f64,
    pub peak_mbps: f64,
    /// The rate each tenth of a second: seconds in, megabits a second.
    pub series: Vec<(f32, f32)>,
    /// Connections that carried it (of those asked for).
    pub streams: usize,
    /// Round trips on another connection meanwhile.
    pub latency: Latency,
    pub ours: Ours,
    pub theirs: Option<Theirs>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Results {
    /// When it began (seconds since 1970).
    pub when: u64,
    pub server: String,
    pub address: String,
    pub ipv6: bool,
    pub place: Place,
    pub dns_ms: f64,
    pub connect_ms: f64,
    pub tls_ms: f64,
    /// Requests' round trips with the line idle, and TCP's own least
    /// round trip then (no server's work in it).
    pub idle: Latency,
    pub idle_tcp_rtt_ms: f64,
    pub download: Option<Transfer>,
    pub upload: Option<Transfer>,
    /// Stopped before its end: what's here is what was measured by then.
    pub stopped: bool,
}

impl Results {
    /// How much later a round trip is while the line is busy than while
    /// it's idle (the worse way's median over the idle one): what a
    /// queue somewhere on the way adds.
    pub fn added_latency_ms(&self) -> Option<f64> {
        let loaded =
            [&self.download, &self.upload].into_iter().flatten().filter(|t| t.latency.samples > 0).map(|t| t.latency.median).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))?;
        (self.idle.samples > 0).then_some((loaded - self.idle.median).max(0.0))
    }

    /// Round trips a minute while the line is busy, the worse way's (as
    /// Apple's networkQuality counts responsiveness).
    pub fn responsiveness(&self) -> Option<f64> {
        let loaded =
            [&self.download, &self.upload].into_iter().flatten().filter(|t| t.latency.samples > 0).map(|t| t.latency.median).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))?;
        (loaded > 0.0).then_some(60_000.0 / loaded)
    }
}

const TENTH: Duration = Duration::from_millis(100);
/// A transfer's pieces: the first small, the rest sized to take
/// [`piece_secs`] at the rate the last one went, within what the server
/// takes.
const FIRST_PIECE: u64 = 256 * 1024;
const MOST_DOWN: u64 = 100_000_000;
const MOST_UP: u64 = 50_000_000;

/// How long connection `stream`'s pieces are sized to take: 1.5 s, 2.2,
/// 2.9, 3.6, … so no two connections' pieces end together twice.
fn piece_secs(stream: usize) -> f64 {
    1.5 + 0.7 * (stream % 6) as f64
}

fn place_of(r: &Response) -> Place {
    let get = |n: &str| r.header(n).unwrap_or_default().to_owned();
    Place { colo: get("colo"), city: get("city"), country: get("country"), asn: get("asn") }
}

/// One request with no body, there and back: how long, in milliseconds,
/// less the time the server says it spent on it (`Server-Timing`'s
/// durations: Cloudflare's take 20 to 40 ms a request, which isn't the
/// line's doing; measured, what's left is within a few milliseconds of
/// TCP's own round trip, where the whole was 44 ms on a 5 ms path).
fn probe(c: &mut Connection) -> std::io::Result<(f64, Response)> {
    let t0 = Instant::now();
    c.request("GET", "/__down?bytes=0", None)?;
    let r = c.response()?;
    let whole = t0.elapsed().as_secs_f64() * 1000.0;
    Ok(((whole - server_ms(&r)).max(0.1), r))
}

/// The time the server says it spent answering: every `dur=` of its
/// `Server-Timing` headers, summed.
fn server_ms(r: &Response) -> f64 {
    r.headers("server-timing").flat_map(|v| v.split(',')).filter_map(|entry| entry.split(';').find_map(|p| p.trim().strip_prefix("dur="))).filter_map(|d| d.trim().parse::<f64>().ok()).sum()
}

/// Run the test. `tell` hears of everything as it happens (from several
/// threads); setting `stop` ends it at once with what there is.
pub fn run(config: &Config, stop: &AtomicBool, tell: &(dyn Fn(Update) + Sync)) -> Result<Results, String> {
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    tell(Update::Phase(Phase::Connecting));
    let tls = if config.server.tls { Some(crate::http::tls_config().map_err(|e| format!("TLS couldn't be set up: {e}"))?) } else { None };
    let (addrs, dns_ms) = crate::http::resolve(&config.server).map_err(|e| format!("{} couldn't be found: {e}", config.server.host))?;
    // The first address that answers is every connection's.
    let mut opened = None;
    let mut why = String::new();
    for addr in addrs {
        match Connection::open(&config.server, addr, tls.as_ref(), config.patience) {
            Ok(c) => {
                opened = Some(c);
                break;
            }
            Err(e) => why = format!("{addr}: {e}"),
        }
    }
    let mut first = opened.ok_or_else(|| format!("{} didn't answer ({why})", config.server.host))?;
    let addr = first.peer;
    let (_, r) = probe(&mut first).map_err(|e| format!("{} didn't answer a request: {e}", config.server.host))?;
    if r.status != 200 {
        return Err(format!("{} answered {} (not a speed test server?)", config.server.host, r.status));
    }
    let place = place_of(&r);
    let opening = Opening { dns_ms, ..first.opening };
    tell(Update::Opened { address: addr.ip().to_string(), place: place.clone(), opening });
    let mut results = Results {
        when,
        server: config.server.host.clone(),
        address: addr.ip().to_string(),
        ipv6: addr.is_ipv6(),
        place,
        dns_ms,
        connect_ms: opening.connect_ms,
        tls_ms: opening.tls_ms,
        ..Results::default()
    };

    tell(Update::Phase(Phase::Latency));
    let mut idle = Vec::new();
    for _ in 0..config.idle_probes {
        if stop.load(SeqCst) {
            break;
        }
        match probe(&mut first) {
            Ok((ms, _)) => {
                idle.push(ms);
                tell(Update::Latency { ms });
            }
            Err(e) => return Err(format!("{} stopped answering: {e}", config.server.host)),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    results.idle = Latency::of(&idle);
    results.idle_tcp_rtt_ms = first.socket().ok().and_then(|s| TcpInfo::of(&s).ok()).map_or(0.0, |i| i.min_rtt_us as f64 / 1000.0);
    tell(Update::Idle { latency: results.idle.clone(), tcp_rtt_ms: results.idle_tcp_rtt_ms });
    drop(first);

    for (phase, time) in [(Phase::Download, config.download), (Phase::Upload, config.upload)] {
        if stop.load(SeqCst) || time.is_zero() {
            continue;
        }
        tell(Update::Phase(phase));
        let transfer = transfer(config, addr, tls.as_ref(), phase, time, stop, tell)?;
        tell(Update::Measured { phase, transfer: Box::new(transfer.clone()) });
        match phase {
            Phase::Download => results.download = Some(transfer),
            _ => results.upload = Some(transfer),
        }
    }
    results.stopped = stop.load(SeqCst);
    Ok(results)
}

/// One way's transfer: `config.streams` connections moving data for
/// `time`, another measuring the round trip meanwhile.
fn transfer(config: &Config, addr: SocketAddr, tls: Option<&Arc<rustls::ClientConfig>>, phase: Phase, time: Duration, stop: &AtomicBool, tell: &(dyn Fn(Update) + Sync)) -> Result<Transfer, String> {
    let down = phase == Phase::Download;
    let moved = AtomicU64::new(0);
    // Each connection's socket (to ask the kernel about, and to cut at
    // the end) and the server's last word on it.
    let sockets: Mutex<Vec<TcpStream>> = Mutex::new(Vec::new());
    let theirs: Mutex<Vec<Option<Theirs>>> = Mutex::new(vec![None; config.streams]);
    let latencies: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    let over = AtomicBool::new(false);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let mut series: Vec<(f32, f32)> = Vec::new();
    let mut last_ours = Ours::default();
    let mut rtts: Vec<f64> = Vec::new();
    let (mut secs, mut carried, mut total) = (0.0, 0usize, 0u64);

    std::thread::scope(|scope| {
        for stream in 0..config.streams {
            let (moved, sockets, theirs, over, failed) = (&moved, &sockets, &theirs, &over, &failed);
            scope.spawn(move || {
                let went = (|| -> std::io::Result<()> {
                    let mut c = Connection::open(&config.server, addr, tls, config.patience)?;
                    sockets.lock().unwrap().push(c.socket()?);
                    let mut buf = vec![0u8; 256 * 1024];
                    if !down {
                        // (Nothing a link could squeeze: not all zeros.)
                        let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ stream as u64;
                        for b in buf.iter_mut() {
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                            *b = x as u8;
                        }
                    }
                    let mut size = FIRST_PIECE;
                    let socket = c.socket()?;
                    let acked = || TcpInfo::of(&socket).map_or(0, |i| i.bytes_acked);
                    while !over.load(SeqCst) {
                        let (t0, acked_before) = (Instant::now(), acked());
                        let count = |n: usize| {
                            moved.fetch_add(n as u64, SeqCst);
                            !over.load(SeqCst)
                        };
                        let (got, response) = if down {
                            c.request("GET", &format!("/__down?bytes={size}"), None)?;
                            let r = c.response()?;
                            (c.read_body(r.content_length(), &mut buf, count)?, Some(r))
                        } else {
                            c.request("POST", "/__up", Some(size))?;
                            let sent = c.send_body(size, &buf, count)?;
                            (sent, if sent == size { Some(c.response()?) } else { None })
                        };
                        if let Some(t) = response.as_ref().and_then(Theirs::parse) {
                            theirs.lock().unwrap()[stream] = Some(t);
                        }
                        if response.as_ref().is_some_and(|r| r.status != 200) {
                            return Err(std::io::Error::other(format!("the server answered {}", response.map_or(0, |r| r.status))));
                        }
                        // The next piece: this connection's seconds' worth at
                        // this one's rate (up: at the rate it was acknowledged,
                        // not written).
                        let got = if down { got } else { acked().saturating_sub(acked_before) };
                        let rate = got as f64 / t0.elapsed().as_secs_f64().max(1e-3);
                        size = ((rate * piece_secs(stream)) as u64).clamp(FIRST_PIECE, if down { MOST_DOWN } else { MOST_UP });
                    }
                    Ok(())
                })();
                // (Cut at the end, a connection ends with an error: that's
                // not a failure.)
                if let Err(e) = went
                    && !over.load(SeqCst)
                {
                    *failed.lock().unwrap() = Some(e.to_string());
                }
            });
        }
        // The round trip meanwhile, five times a second.
        {
            let (latencies, over) = (&latencies, &over);
            scope.spawn(move || {
                let Ok(mut c) = Connection::open(&config.server, addr, tls, config.patience) else { return };
                while !over.load(SeqCst) {
                    let Ok((ms, _)) = probe(&mut c) else { return };
                    if over.load(SeqCst) {
                        break;
                    }
                    latencies.lock().unwrap().push(ms);
                    tell(Update::Latency { ms });
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }

        // Ten times a second: the bytes moved, and the kernel's figures.
        let t0 = Instant::now();
        let (mut before, mut tick, mut last_at) = (0u64, 0u32, 0.0f64);
        loop {
            tick += 1;
            let next = t0 + TENTH * tick;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            // Down: what was read. Up: what the server has acknowledged.
            let now = if down { moved.load(SeqCst) } else { sockets.lock().unwrap().iter().filter_map(|s| TcpInfo::of(s).ok()).map(|i| i.bytes_acked).sum() };
            let at = t0.elapsed().as_secs_f64();
            let mbps = now.saturating_sub(before) as f64 * 8.0 / 1e6 / (at - last_at).max(1e-3);
            (before, last_at, total) = (now.max(before), at, now.max(before));
            series.push((at as f32, mbps as f32));
            tell(Update::Rate { phase, at, mbps });
            if tick % 2 == 0 {
                let each: Vec<_> =
                    sockets.lock().unwrap().iter().filter_map(|s| Some((TcpInfo::of(s).ok()?, tcp_info::congestion_control(s).unwrap_or_default(), tcp_info::bbr_info(s).ok().flatten()))).collect();
                if !each.is_empty() {
                    carried = carried.max(each.len());
                    last_ours = Ours::of(&each);
                    rtts.push(last_ours.rtt_ms);
                    tell(Update::Tcp { phase, ours: last_ours.clone() });
                }
            }
            let all_failed = failed.lock().unwrap().is_some() && sockets.lock().unwrap().is_empty();
            if at >= time.as_secs_f64() || stop.load(SeqCst) || all_failed {
                secs = at;
                break;
            }
        }
        // Over: every connection cut where it is.
        over.store(true, SeqCst);
        for s in sockets.lock().unwrap().iter() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    });

    let bytes = total;
    if bytes == 0 {
        let why = failed.lock().unwrap().clone().unwrap_or_else(|| "nothing moved".into());
        return Err(format!("the {} didn't get going: {why}", if down { "download" } else { "upload" }));
    }
    // The rate once going: the first fifth left out. The best second.
    let skip = series.len() / 5;
    let going = &series[skip..];
    let mbps = if going.is_empty() { 0.0 } else { going.iter().map(|(_, m)| *m as f64).sum::<f64>() / going.len() as f64 };
    let peak_mbps = series.windows(10).map(|w| w.iter().map(|(_, m)| *m as f64).sum::<f64>() / 10.0).fold(0.0, f64::max).max(if series.len() < 10 { mbps } else { 0.0 });
    if !rtts.is_empty() {
        // (The smoothed round trip over the whole transfer, not its last.)
        last_ours.rtt_ms = rtts.iter().sum::<f64>() / rtts.len() as f64;
    }
    let theirs: Vec<Theirs> = theirs.into_inner().unwrap().into_iter().flatten().collect();
    Ok(Transfer { secs, bytes, mbps, peak_mbps, series, streams: carried, latency: Latency::of(&latencies.into_inner().unwrap()), ours: last_ours, theirs: Theirs::of(&theirs) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve as fake;

    fn quick(server: Server) -> Config {
        Config { server, streams: 3, download: Duration::from_millis(1200), upload: Duration::from_millis(1200), idle_probes: 5, patience: Duration::from_secs(5) }
    }

    /// A whole test against a server here that sends 4 Mbit/s a
    /// connection: each phase told in order, the rates measured what the
    /// server gives, the kernel's and the server's figures there.
    #[test]
    fn a_whole_test_measures_what_the_server_gives() {
        let fake = fake::start(500_000);
        let heard = Mutex::new(Vec::new());
        let stop = AtomicBool::new(false);
        let r = run(&quick(fake.server.clone()), &stop, &|u| heard.lock().unwrap().push(u)).unwrap();
        let heard = heard.into_inner().unwrap();
        let phases: Vec<Phase> = heard.iter().filter_map(|u| if let Update::Phase(p) = u { Some(*p) } else { None }).collect();
        assert_eq!(phases, [Phase::Connecting, Phase::Latency, Phase::Download, Phase::Upload]);
        assert!(heard.iter().any(|u| matches!(u, Update::Opened { place, .. } if place.colo == "TST")));
        assert!(heard.iter().any(|u| matches!(u, Update::Idle { latency, .. } if latency.samples == 5)));
        assert_eq!(heard.iter().filter(|u| matches!(u, Update::Measured { .. })).count(), 2);
        assert_eq!((r.server.as_str(), r.address.as_str(), r.ipv6, r.stopped), ("127.0.0.1", "127.0.0.1", false, false));
        assert_eq!(r.place, Place { colo: "TST".into(), city: "Testville".into(), country: "ZZ".into(), asn: "64496".into() });
        assert!(r.connect_ms > 0.0 && r.tls_ms == 0.0);
        assert_eq!(r.idle.samples, 5);
        assert!(r.idle.min > 0.0 && r.idle.min <= r.idle.median && r.idle.median <= r.idle.max && r.idle_tcp_rtt_ms > 0.0, "{:?}", r.idle);
        assert!(r.added_latency_ms().is_some() && r.responsiveness().unwrap() > 0.0);
        // Three connections at 0.5 MB/s each: 12 Mbit/s.
        let d = r.download.unwrap();
        assert!((9.0..15.0).contains(&d.mbps), "{} Mbit/s down", d.mbps);
        assert!(d.peak_mbps >= d.mbps * 0.9 && d.streams == 3 && d.series.len() >= 11 && (1.2..1.6).contains(&d.secs), "{d:?}");
        assert!(d.ours.bytes_received >= d.bytes && d.ours.mss > 1000 && !d.ours.congestion_control.is_empty());
        assert!(d.latency.samples >= 3, "{:?}", d.latency);
        // (The fake server counts its answers into its figures: 100 packets
        // sent, one lost and two sent again an answer.)
        let t = d.theirs.unwrap();
        assert!(t.sent >= 300 && t.lost >= 3 && t.retransmissions == t.lost * 2 && (t.rtt_ms - 5.93).abs() < 0.01 && (t.min_rtt_ms - 5.785).abs() < 0.01, "{t:?}");
        assert!((t.retransmitted_pct() - 2.0).abs() < 0.01);
        // Up: the server takes 0.5 MB/s a connection too. (What it has
        // acknowledged, not what was written: a socket's buffer takes
        // megabytes at once.)
        let u = r.upload.unwrap();
        assert!((6.0..20.0).contains(&u.mbps) && u.ours.bytes_sent >= u.bytes && u.streams == 3, "{} Mbit/s up", u.mbps);
        assert!(fake.uploaded.load(SeqCst) > 0);
        // And as JSON, for whoever asks.
        let r2 = run(&Config { download: Duration::ZERO, upload: Duration::ZERO, ..quick(fake.server.clone()) }, &stop, &|_| {}).unwrap();
        assert!(r2.download.is_none() && r2.upload.is_none());
        let back: Results = serde_json::from_str(&serde_json::to_string(&r2).unwrap()).unwrap();
        assert_eq!((back.when, &back.place, back.idle.samples, back.stopped), (r2.when, &r2.place, 5, false));
        assert!((back.connect_ms - r2.connect_ms).abs() < 1e-9);
    }

    /// Stopped part way: over at once, with what was measured by then.
    #[test]
    fn stopped_at_once_with_what_there_is() {
        let fake = fake::start(500_000);
        let stop = Arc::new(AtomicBool::new(false));
        let config = Config { download: Duration::from_secs(30), ..quick(fake.server.clone()) };
        let flag = stop.clone();
        let t0 = Instant::now();
        let r = run(&config, &stop, &move |u| {
            if matches!(u, Update::Rate { at, .. } if at > 0.4) {
                flag.store(true, SeqCst);
            }
        })
        .unwrap();
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert!(r.stopped && r.upload.is_none());
        let d = r.download.unwrap();
        assert!(d.bytes > 0 && d.secs < 1.5, "{d:?}");
    }

    #[test]
    fn what_cant_be_done_is_said() {
        let stop = AtomicBool::new(false);
        let nowhere = Config { server: Server { host: "127.0.0.1".into(), port: 1, tls: false }, patience: Duration::from_secs(2), ..Config::default() };
        let e = run(&nowhere, &stop, &|_| {}).unwrap_err();
        assert!(e.starts_with("127.0.0.1 didn't answer (127.0.0.1:1: "), "{e}");
        let unnamed = Config { server: Server { host: "no-such-host.invalid".into(), port: 443, tls: false }, ..Config::default() };
        assert!(run(&unnamed, &stop, &|_| {}).unwrap_err().starts_with("no-such-host.invalid couldn't be found"));
    }

    #[test]
    fn round_trips_are_summed_up() {
        let l = Latency::of(&[10.0, 12.0, 11.0, 30.0, 10.0]);
        assert_eq!((l.samples, l.min, l.median, l.max, l.p90), (5, 10.0, 11.0, 30.0, 30.0));
        assert!((l.mean - 14.6).abs() < 1e-9 && (l.jitter - (2.0 + 1.0 + 19.0 + 20.0) / 4.0).abs() < 1e-9);
        assert_eq!(Latency::of(&[]), Latency::default());
    }

    /// The server's own time on a request is taken off its round trip.
    #[test]
    fn the_servers_time_is_not_the_lines() {
        let fake = fake::start(0);
        let (addrs, _) = crate::http::resolve(&fake.server).unwrap();
        let mut c = Connection::open(&fake.server, addrs[0], None, Duration::from_secs(5)).unwrap();
        c.request("GET", "/__down?bytes=0", None).unwrap();
        let r = c.response().unwrap();
        // (The fake's: "cfSpeedEdge;dur=1, cfSpeedWorker;dur=2", and a
        // cfL4 entry with no duration.)
        assert_eq!(server_ms(&r), 3.0);
        let (ms, _) = probe(&mut c).unwrap();
        assert_eq!(ms, 0.1, "a round trip on this computer is under the fake's 3 ms: never less than a tenth");
    }
}
