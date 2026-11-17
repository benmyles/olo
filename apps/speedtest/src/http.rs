//! The test's connections: HTTP/1.1 over TCP, with TLS, made by hand.
//!
//! A speed test's figures are its sockets': how long each took to open
//! (the name looked up, the TCP handshake, TLS's), and what the kernel
//! says of each as data moves (`olo_net::tcp_info`). So each connection
//! here is a socket of ours with the least HTTP on it that the test's
//! requests need: a request line and its headers out, a status and
//! headers back, a body of a known length read or written with every
//! piece counted as it goes. Never through a proxy: what's measured is
//! the path from this computer.

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

/// Where the test is run against.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    /// HTTPS (the real thing), or plain HTTP (a test's own server).
    pub tls: bool,
}

/// How long opening a connection took, step by step.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct Opening {
    /// Looking the server's name up (0 where an address was given).
    pub dns_ms: f64,
    /// TCP's handshake: one round trip.
    pub connect_ms: f64,
    /// TLS's (two round trips, or one where a session was resumed; 0
    /// without TLS).
    pub tls_ms: f64,
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// A response's status and headers.
#[derive(Debug, Clone, Default)]
pub struct Response {
    pub status: u16,
    headers: Vec<(String, String)>,
}

impl Response {
    /// The header named (any case), the first of them.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Every header named (a server may send `Server-Timing` twice).
    pub fn headers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers.iter().filter(move |(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> u64 {
        self.header("content-length").and_then(|v| v.trim().parse().ok()).unwrap_or(0)
    }
}

/// One connection to the server.
pub struct Connection {
    stream: Stream,
    /// The same socket, to ask the kernel about and to cut from another
    /// thread ([`Connection::socket`]).
    socket: TcpStream,
    host: String,
    pub opening: Opening,
    pub peer: SocketAddr,
    /// What was read past the last response's head.
    kept: Vec<u8>,
}

/// The server's addresses, and how long finding them took.
pub fn resolve(server: &Server) -> io::Result<(Vec<SocketAddr>, f64)> {
    let t0 = Instant::now();
    let addrs: Vec<SocketAddr> = (server.host.as_str(), server.port).to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} has no address", server.host)));
    }
    Ok((addrs, ms(t0)))
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// TLS as every connection of the test uses it: the system's
/// certificates, HTTP/1.1.
pub fn tls_config() -> io::Result<Arc<rustls::ClientConfig>> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_platform_verifier()
        .map_err(io::Error::other)?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

impl Connection {
    /// Open one to `addr` (the server's, already looked up: every
    /// connection of a test goes to the same address), each step timed.
    /// Nothing waits longer than `patience` for the server.
    pub fn open(server: &Server, addr: SocketAddr, tls: Option<&Arc<rustls::ClientConfig>>, patience: Duration) -> io::Result<Connection> {
        let t0 = Instant::now();
        let socket = TcpStream::connect_timeout(&addr, patience)?;
        let connect_ms = ms(t0);
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(patience))?;
        socket.set_write_timeout(Some(patience))?;
        let (stream, tls_ms) = match (server.tls, tls) {
            (true, Some(config)) => {
                let t1 = Instant::now();
                let name = rustls::pki_types::ServerName::try_from(server.host.clone()).map_err(io::Error::other)?;
                let mut session = rustls::ClientConnection::new(config.clone(), name).map_err(io::Error::other)?;
                let mut raw = socket.try_clone()?;
                while session.is_handshaking() {
                    session.complete_io(&mut raw)?;
                }
                (Stream::Tls(Box::new(rustls::StreamOwned::new(session, raw))), ms(t1))
            }
            _ => (Stream::Plain(socket.try_clone()?), 0.0),
        };
        Ok(Connection { stream, socket, host: server.host.clone(), opening: Opening { dns_ms: 0.0, connect_ms, tls_ms }, peer: addr, kept: Vec::new() })
    }

    /// The socket itself: for `TcpInfo::of`, and `shutdown` from another
    /// thread to end whatever this connection is in the middle of.
    pub fn socket(&self) -> io::Result<TcpStream> {
        self.socket.try_clone()
    }

    /// Send a request's head. A `body` length says one follows
    /// ([`Connection::send_body`]).
    pub fn request(&mut self, method: &str, path: &str, body: Option<u64>) -> io::Result<()> {
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: olo-speedtest\r\nAccept: */*\r\nConnection: keep-alive\r\n", self.host);
        if let Some(len) = body {
            head += &format!("Content-Type: application/octet-stream\r\nContent-Length: {len}\r\n");
        }
        head += "\r\n";
        self.stream.write_all(head.as_bytes())?;
        self.stream.flush()
    }

    /// Read a response's head: its status and headers. (What follows it
    /// in the same read is kept for [`Connection::read_body`].)
    pub fn response(&mut self) -> io::Result<Response> {
        let mut head = std::mem::take(&mut self.kept);
        let mut buf = [0u8; 4096];
        let end = loop {
            if let Some(at) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                break at;
            }
            if head.len() > 64 * 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "the response's headers never ended"));
            }
            let n = self.stream.read(&mut buf)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection"));
            }
            head.extend_from_slice(&buf[..n]);
        };
        self.kept = head.split_off(end + 4);
        let text = String::from_utf8_lossy(&head);
        let mut lines = text.split("\r\n");
        let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an HTTP response"))?;
        let headers = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned())).collect();
        Ok(Response { status, headers })
    }

    /// Read `len` bytes of a response's body and drop them, telling
    /// `moved` of each piece as it comes (a false from it stops the
    /// reading there). How many were read.
    pub fn read_body(&mut self, len: u64, buf: &mut [u8], mut moved: impl FnMut(usize) -> bool) -> io::Result<u64> {
        let mut got = (self.kept.len() as u64).min(len);
        self.kept.drain(..got as usize);
        if got > 0 && !moved(got as usize) {
            return Ok(got);
        }
        while got < len {
            let want = buf.len().min((len - got) as usize);
            let n = self.stream.read(&mut buf[..want])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection"));
            }
            got += n as u64;
            if !moved(n) {
                break;
            }
        }
        Ok(got)
    }

    /// Send `len` bytes of a request's body, out of `chunk` over and
    /// over, telling `moved` of each piece (a false stops it there: the
    /// connection is then no use for another request). How many went.
    pub fn send_body(&mut self, len: u64, chunk: &[u8], mut moved: impl FnMut(usize) -> bool) -> io::Result<u64> {
        let mut sent = 0u64;
        while sent < len {
            let want = chunk.len().min((len - sent) as usize);
            let n = self.stream.write(&chunk[..want])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "the server stopped taking data"));
            }
            sent += n as u64;
            if !moved(n) {
                return Ok(sent);
            }
        }
        self.stream.flush()?;
        Ok(sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve as fake;

    /// Requests on one connection, one after another: a status and
    /// headers back, a body read to its length with every piece told, a
    /// body sent, and the reading stopped part way.
    #[test]
    fn requests_and_bodies_on_one_connection() {
        let fake = fake::start(0);
        let (addrs, _) = resolve(&fake.server).unwrap();
        let mut c = Connection::open(&fake.server, addrs[0], None, Duration::from_secs(5)).unwrap();
        assert!(c.opening.connect_ms > 0.0 && c.opening.tls_ms == 0.0);
        let mut buf = vec![0u8; 8192];
        c.request("GET", "/__down?bytes=0", None).unwrap();
        let r = c.response().unwrap();
        assert_eq!((r.status, r.content_length(), r.header("COLO"), r.header("city")), (200, 0, Some("TST"), Some("Testville")));
        assert_eq!(r.headers("server-timing").count(), 2);
        // A body, whole.
        c.request("GET", "/__down?bytes=100000", None).unwrap();
        let r = c.response().unwrap();
        let (mut pieces, mut seen) = (0, 0u64);
        let got = c
            .read_body(r.content_length(), &mut buf, |n| {
                pieces += 1;
                seen += n as u64;
                true
            })
            .unwrap();
        assert_eq!((got, seen), (100_000, 100_000));
        assert!(pieces > 1);
        // A body sent; the connection good for the next request.
        c.request("POST", "/__up", Some(50_000)).unwrap();
        assert_eq!(c.send_body(50_000, &buf, |_| true).unwrap(), 50_000);
        assert_eq!(c.response().unwrap().status, 200);
        assert_eq!(fake.uploaded.load(std::sync::atomic::Ordering::SeqCst), 50_000);
        // Stopped part way.
        c.request("GET", "/__down?bytes=5000000", None).unwrap();
        let r = c.response().unwrap();
        let mut seen = 0u64;
        let got = c
            .read_body(r.content_length(), &mut buf, |n| {
                seen += n as u64;
                seen < 20_000
            })
            .unwrap();
        assert!((20_000..5_000_000).contains(&got), "{got}");
        assert_eq!(fake.requests.load(std::sync::atomic::Ordering::SeqCst), 4);
    }

    #[test]
    fn a_name_with_no_server_is_an_error() {
        let nowhere = Server { host: "127.0.0.1".into(), port: 1, tls: false };
        let (addrs, _) = resolve(&nowhere).unwrap();
        assert!(Connection::open(&nowhere, addrs[0], None, Duration::from_secs(2)).is_err());
    }
}
