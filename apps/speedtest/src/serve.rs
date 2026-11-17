//! A speed test server of Olo's own, for trying the test without the
//! internet: plain HTTP on this computer, the two requests a test makes
//! (`GET /__down?bytes=N`, `POST /__up`), answered with the headers the
//! real one sends, at a rate of its own. The tests' server, and
//! `olo-speedtest --serve PORT [BYTES-A-SECOND]`'s (which
//! `OLO_SPEEDTEST_SERVER=127.0.0.1:PORT` points the app at).

use crate::http::Server;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

// (Its counts are the tests' to read.)
#[cfg_attr(not(test), allow(dead_code))]
pub struct Fake {
    pub server: Server,
    pub addr: SocketAddr,
    /// Requests answered, and bytes taken by `/__up`.
    pub requests: Arc<AtomicU64>,
    pub uploaded: Arc<AtomicU64>,
}

/// Start one that sends at most `rate` bytes a second on each
/// connection (0: as fast as it goes).
#[cfg(test)]
pub fn start(rate: u64) -> Fake {
    start_at(0, rate).expect("a port on this computer")
}

/// The same, on `port` (0: any).
pub fn start_at(port: u16, rate: u64) -> std::io::Result<Fake> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let addr = listener.local_addr()?;
    let (requests, uploaded) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (r, u) = (requests.clone(), uploaded.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (r, u) = (r.clone(), u.clone());
            std::thread::spawn(move || serve(stream, rate, &r, &u));
        }
    });
    Ok(Fake { server: Server { host: "127.0.0.1".into(), port: addr.port(), tls: false }, addr, requests, uploaded })
}

fn serve(mut stream: TcpStream, rate: u64, requests: &AtomicU64, uploaded: &AtomicU64) {
    let mut kept = Vec::new();
    let mut buf = vec![0u8; 65536];
    let mut answered = 0u64;
    loop {
        let end = loop {
            if let Some(at) = kept.windows(4).position(|w| w == b"\r\n\r\n") {
                break at;
            }
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => kept.extend_from_slice(&buf[..n]),
                _ => return,
            }
        };
        let rest = kept.split_off(end + 4);
        let head = String::from_utf8_lossy(&kept).into_owned();
        kept = rest;
        let line = head.lines().next().unwrap_or_default().to_owned();
        let length: u64 = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
        // (A request's body: taken, at the server's rate, and counted.)
        let mut body = (kept.len() as u64).min(length);
        kept.drain(..body as usize);
        let t0 = Instant::now();
        while body < length {
            if rate > 0 {
                let ahead = body as f64 / rate as f64 - t0.elapsed().as_secs_f64();
                if ahead > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(ahead.min(0.05)));
                    continue;
                }
            }
            let want = if rate > 0 { buf.len().min((rate / 20).max(1024) as usize) } else { buf.len() };
            match stream.read(&mut buf[..want]) {
                Ok(n) if n > 0 => body += n as u64,
                _ => return,
            }
        }
        uploaded.fetch_add(length, SeqCst);
        let bytes: u64 = line.split("bytes=").nth(1).and_then(|v| v.split(|c: char| !c.is_ascii_digit()).next()).and_then(|v| v.parse().ok()).unwrap_or(0);
        answered += 1;
        requests.fetch_add(1, SeqCst);
        // (As the real one answers: where it is, how long it took, and
        // its own side of the connection's TCP.)
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {bytes}\r\nConnection: keep-alive\r\ncolo: TST\r\ncity: Testville\r\ncountry: ZZ\r\nasn: 64496\r\nServer-Timing: cfSpeedEdge;dur=1, cfSpeedWorker;dur=2\r\nserver-timing: cfL4;desc=\"?proto=TCP&rtt=5930&min_rtt=5785&rtt_var=2273&sent={}&recv=6&lost={}&retrans={}&sent_bytes=3946&recv_bytes=1782&delivery_rate=750907&cwnd=53&unsent_bytes=0&cid=de81&ts=42&x=0\"\r\n\r\n",
            answered * 100,
            answered,
            answered * 2
        );
        if stream.write_all(reply.as_bytes()).is_err() {
            return;
        }
        let chunk = vec![0x5au8; 16384];
        let (t0, mut sent) = (Instant::now(), 0u64);
        while sent < bytes {
            if rate > 0 {
                let due = sent as f64 / rate as f64;
                let ahead = due - t0.elapsed().as_secs_f64();
                if ahead > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(ahead.min(0.05)));
                    continue;
                }
            }
            let n = chunk.len().min((bytes - sent) as usize);
            if stream.write_all(&chunk[..n]).is_err() {
                return;
            }
            sent += n as u64;
        }
    }
}
