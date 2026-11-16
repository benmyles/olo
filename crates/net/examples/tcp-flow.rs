//! tcp-flow: TCP flows to measure a congestion control with, and what the
//! kernel says of each as it goes (`olo_net::tcp_info`). A tool for
//! tests (tools/dev/bbr3-check runs it in a KVM guest, over links made
//! with netem); nothing of Olo runs it.
//!
//!   tcp-flow sink ADDR:PORT
//!   tcp-flow send ADDR:PORT --cc NAME[,NAME…] [--secs N] [--start S[,S…]]
//!            [--every MS] [--rate MBIT] [--chunk BYTES] [--series] [--label TEXT]
//!
//! `sink` takes connections and reads each to its end. `send` makes one
//! flow per NAME (its congestion control, set before it connects),
//! started `--start` seconds in (0), each sending for `--secs` (10) as
//! fast as TCP takes it, or at `--rate` megabits a second in `--chunk`
//! byte writes (the program holding back: "application limited"). Every
//! `--every` ms (100) each flow's `TCP_INFO` is read. At the end, a line
//! of JSON: per flow its throughput (acknowledged bytes over its time),
//! retransmissions, the round-trip time's smallest, mean and 95th
//! percentile (the smoothed one, sampled), BBR's model, and with
//! `--series` every sample.

use olo_net::tcp_info::{self, TcpInfo};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    os::fd::{AsRawFd, FromRawFd},
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("sink") => sink(&args[1]),
        Some("send") => {
            let to: SocketAddr = args[1].to_socket_addrs().expect("ADDR:PORT").next().expect("an address");
            let ccs: Vec<String> = opt("--cc").expect("--cc NAME").split(',').map(str::to_owned).collect();
            let starts: Vec<f64> = opt("--start").map(|s| s.split(',').map(|v| v.parse().expect("--start seconds")).collect()).unwrap_or_default();
            let secs: f64 = opt("--secs").map_or(10.0, |v| v.parse().expect("--secs"));
            let every = Duration::from_millis(opt("--every").map_or(100, |v| v.parse().expect("--every ms")));
            let rate: Option<f64> = opt("--rate").map(|v| v.parse().expect("--rate Mbit/s"));
            let chunk: usize = opt("--chunk").map_or(65536, |v| v.parse().expect("--chunk bytes"));
            let series = args.iter().any(|a| a == "--series");
            let began = Instant::now();
            let flows: Vec<_> = ccs
                .into_iter()
                .enumerate()
                .map(|(i, cc)| {
                    let start = starts.get(i).copied().unwrap_or(0.0);
                    std::thread::spawn(move || flow(to, &cc, began, start, secs, every, rate, chunk, series))
                })
                .collect();
            let flows: Vec<String> = flows.into_iter().map(|f| f.join().expect("a flow")).collect();
            println!("{{\"label\":\"{}\",\"flows\":[{}]}}", opt("--label").unwrap_or_default(), flows.join(","));
        }
        _ => {
            eprintln!("tcp-flow sink ADDR:PORT | tcp-flow send ADDR:PORT --cc NAME[,NAME…] [--secs N] [--start S,…] [--every MS] [--rate MBIT] [--chunk BYTES] [--series] [--label TEXT]");
            std::process::exit(2);
        }
    }
}

fn sink(at: &str) {
    let listener = TcpListener::bind(at).expect("the address to listen at");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut buf = vec![0u8; 1 << 18];
            while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
        });
    }
}

/// A socket with its congestion control set before it connects, so the
/// connection begins with it.
fn connect(to: SocketAddr, cc: &str) -> std::io::Result<TcpStream> {
    let family = if to.is_ipv4() { libc::AF_INET } else { libc::AF_INET6 };
    // SAFETY: socket(2); the descriptor is owned by the stream made of it.
    let stream = unsafe {
        let fd = libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        TcpStream::from_raw_fd(fd)
    };
    tcp_info::set_congestion_control(&stream, cc)?;
    let (addr, len): (libc::sockaddr_storage, libc::socklen_t) = match to {
        SocketAddr::V4(a) => {
            // SAFETY: a zeroed sockaddr_storage is valid; sockaddr_in fits in it.
            let mut s: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let sin = unsafe { &mut *(&raw mut s).cast::<libc::sockaddr_in>() };
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = a.port().to_be();
            sin.sin_addr.s_addr = u32::from_ne_bytes(a.ip().octets());
            (s, size_of::<libc::sockaddr_in>() as libc::socklen_t)
        }
        SocketAddr::V6(a) => {
            // SAFETY: as above, for sockaddr_in6.
            let mut s: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let sin6 = unsafe { &mut *(&raw mut s).cast::<libc::sockaddr_in6>() };
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = a.port().to_be();
            sin6.sin6_addr.s6_addr = a.ip().octets();
            (s, size_of::<libc::sockaddr_in6>() as libc::socklen_t)
        }
    };
    // SAFETY: connect(2) with the address just made and its length.
    if unsafe { libc::connect(stream.as_raw_fd(), (&raw const addr).cast(), len) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stream)
}

struct Sample {
    t: f64,
    info: TcpInfo,
    bbr: Option<tcp_info::BbrInfo>,
}

#[allow(clippy::too_many_arguments)]
fn flow(to: SocketAddr, cc: &str, began: Instant, start: f64, secs: f64, every: Duration, rate: Option<f64>, chunk: usize, series: bool) -> String {
    let wait = Duration::from_secs_f64(start).saturating_sub(began.elapsed());
    std::thread::sleep(wait);
    let mut stream = match connect(to, cc) {
        Ok(s) => s,
        Err(e) => return format!("{{\"cc\":\"{cc}\",\"error\":\"{e}\"}}"),
    };
    let sampled = stream.try_clone().expect("a second handle");
    let t0 = Instant::now();
    let until = t0 + Duration::from_secs_f64(secs);
    let sampler = std::thread::spawn(move || {
        let mut samples = Vec::new();
        let mut next = t0 + every;
        while next < until {
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            let Ok(info) = TcpInfo::of(&sampled) else { break };
            samples.push(Sample { t: t0.elapsed().as_secs_f64(), info, bbr: tcp_info::bbr_info(&sampled).ok().flatten() });
            next += every;
        }
        samples
    });
    let buf = vec![0x5au8; chunk];
    let mut sent = 0u64;
    while Instant::now() < until {
        if let Some(mbit) = rate {
            // (Held back to the rate: never ahead of what it allows by now.)
            let due = sent as f64 * 8.0 / (mbit * 1e6);
            let ahead = due - t0.elapsed().as_secs_f64();
            if ahead > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(ahead.min(0.5)));
                continue;
            }
        }
        match stream.write(&buf) {
            Ok(n) => sent += n as u64,
            Err(_) => break,
        }
    }
    let wrote_for = t0.elapsed().as_secs_f64();
    let at_end = TcpInfo::of(&stream).unwrap_or_default();
    let bbr = tcp_info::bbr_info(&stream).ok().flatten();
    let samples = sampler.join().unwrap_or_default();
    // What was written is seen through before the flow is counted done
    // (not timed: the throughput is of the sending time).
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.read(&mut [0u8; 16]);
    let last = TcpInfo::of(&stream).unwrap_or_default();

    let mut rtts: Vec<f64> = samples.iter().map(|s| s.info.rtt_us as f64 / 1000.0).collect();
    rtts.sort_by(f64::total_cmp);
    let pick = |q: f64| rtts.get(((rtts.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    let mean = if rtts.is_empty() { 0.0 } else { rtts.iter().sum::<f64>() / rtts.len() as f64 };
    let mut out = format!(
        "{{\"cc\":\"{}\",\"start\":{start},\"secs\":{wrote_for:.3},\"bytes_acked\":{},\"mbps\":{:.3},\"bytes_sent\":{},\"bytes_retrans\":{},\"retrans_pct\":{:.3},\"total_retrans\":{},\"data_segs_out\":{},\"total_rto\":{},\"dsack_dups\":{},\"reord_seen\":{},\"min_rtt_ms\":{:.3},\"rtt_ms_mean\":{mean:.3},\"rtt_ms_p50\":{:.3},\"rtt_ms_p95\":{:.3},\"rtt_ms_max\":{:.3},\"mss\":{},\"cwnd\":{}",
        tcp_info::congestion_control(&stream).unwrap_or_default(),
        at_end.bytes_acked,
        at_end.bytes_acked as f64 * 8.0 / 1e6 / wrote_for,
        last.bytes_sent,
        last.bytes_retrans,
        last.retransmitted() * 100.0,
        last.total_retrans,
        last.data_segs_out,
        last.total_rto,
        last.dsack_dups,
        last.reord_seen,
        at_end.min_rtt_us as f64 / 1000.0,
        pick(0.5),
        pick(0.95),
        pick(1.0),
        at_end.snd_mss,
        at_end.snd_cwnd,
    );
    if let Some(b) = bbr {
        out += &format!(
            ",\"bbr\":{{\"bw_mbps\":{:.3},\"min_rtt_ms\":{:.3},\"pacing_gain\":{:.3},\"cwnd_gain\":{:.3},\"phase\":\"{}\"}}",
            b.bandwidth as f64 * 8.0 / 1e6,
            b.min_rtt_us as f64 / 1000.0,
            b.pacing_gain,
            b.cwnd_gain,
            b.phase()
        );
    }
    if series {
        let mut prev = (0.0f64, 0u64);
        let rows: Vec<String> = samples
            .iter()
            .map(|s| {
                let mbps = (s.info.bytes_acked - prev.1) as f64 * 8.0 / 1e6 / (s.t - prev.0).max(1e-6);
                prev = (s.t, s.info.bytes_acked);
                let bbr = s.bbr.map_or(String::new(), |b| format!(",\"bw\":{:.2},\"gain\":{:.2},\"phase\":\"{}\"", b.bandwidth as f64 * 8.0 / 1e6, b.pacing_gain, b.phase()));
                format!(
                    "{{\"t\":{:.2},\"mbps\":{mbps:.2},\"cwnd\":{},\"flight\":{},\"rtt\":{:.2},\"pacing\":{:.2},\"delivery\":{:.2},\"retrans\":{},\"lost\":{},\"ca\":\"{}\"{bbr}}}",
                    s.t,
                    s.info.snd_cwnd,
                    s.info.in_flight(),
                    s.info.rtt_us as f64 / 1000.0,
                    s.info.pacing_rate as f64 * 8.0 / 1e6,
                    s.info.delivery_rate as f64 * 8.0 / 1e6,
                    s.info.total_retrans,
                    s.info.lost,
                    s.info.ca_state().name(),
                )
            })
            .collect();
        out += &format!(",\"series\":[{}]", rows.join(","));
    }
    out + "}"
}
