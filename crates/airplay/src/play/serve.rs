//! The film's file, served to the receiver for the length of the play:
//! the receiver is given an address and fetches the file itself, in
//! pieces (it asks for ranges, and seeks by them), so what it plays is
//! the file's own picture and sound.
//!
//! One file, at one address nobody could guess (128 random bits, told
//! to the receiver over the enciphered connection); anything else asked
//! of the port is "not found". The file itself crosses the network in
//! the clear, as every film sent to a receiver by its address does.

use std::{
    fs::File,
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use super::media::Kind;
use crate::pairing::hap::random;

pub(crate) struct Served {
    /// Where the receiver fetches it.
    pub url: String,
    at: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    stop: AtomicBool,
    /// How many bytes of the file have gone out.
    sent: AtomicU64,
    /// The connections open, each with a number, to close them at the
    /// end.
    open: Mutex<Vec<(u64, TcpStream)>>,
}

impl Served {
    /// Serving `file` from a port of this address until dropped.
    pub fn start(file: &Path, kind: Kind, ip: IpAddr) -> std::io::Result<Served> {
        let listener = TcpListener::bind((ip, 0))?;
        let at = listener.local_addr()?;
        let path = format!("/{}/film.{}", random::<16>().iter().map(|b| format!("{b:02x}")).collect::<String>(), kind.ext);
        let url = match at {
            SocketAddr::V4(a) => format!("http://{a}{path}"),
            SocketAddr::V6(a) => format!("http://[{}]:{}{path}", a.ip(), a.port()),
        };
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), sent: AtomicU64::new(0), open: Mutex::new(Vec::new()) });
        let (theirs, file) = (shared.clone(), file.to_owned());
        let thread = std::thread::Builder::new().name("airplay-file".into()).spawn(move || {
            for (number, stream) in (0u64..).zip(listener.incoming()) {
                if theirs.stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                if let Ok(kept) = stream.try_clone() {
                    theirs.open.lock().unwrap().push((number, kept));
                }
                let (shared, file, path) = (theirs.clone(), file.clone(), path.clone());
                let _ = std::thread::Builder::new().name("airplay-file-conn".into()).spawn(move || {
                    connection(&mut stream, &file, &path, kind, &shared);
                    // (Closed for the receiver too: the copy kept would hold it open.)
                    let _ = stream.shutdown(Shutdown::Both);
                    shared.open.lock().unwrap().retain(|(n, _)| *n != number);
                });
            }
        })?;
        Ok(Served { url, at, shared, thread: Some(thread) })
    }

    /// How many bytes of the file the receiver has taken.
    pub fn sent(&self) -> u64 {
        self.shared.sent.load(Ordering::Relaxed)
    }

    /// The same count, for another thread to read after this is moved.
    pub fn counter(&self) -> impl Fn() -> u64 + Send + 'static {
        let shared = self.shared.clone();
        move || shared.sent.load(Ordering::Relaxed)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for (_, stream) in self.shared.open.lock().unwrap().drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        // (The listener is woken by a caller of its own.)
        let _ = TcpStream::connect_timeout(&self.at, Duration::from_secs(1));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One of the receiver's connections: its requests, one after another.
fn connection(stream: &mut TcpStream, file: &Path, path: &str, kind: Kind, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));
    let (mut came, mut buf) = (Vec::new(), [0u8; 2048]);
    loop {
        let end = loop {
            if let Some(end) = came.windows(4).position(|w| w == b"\r\n\r\n") {
                break end;
            }
            if came.len() > 16384 {
                return;
            }
            let Ok(n @ 1..) = stream.read(&mut buf) else { return };
            came.extend(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&came[..end]).into_owned();
        came.drain(..end + 4);
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split(' ');
        let (method, asked) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
        let range = lines.filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case("range")).map(|(_, v)| v.trim().to_owned());
        if shared.stop.load(Ordering::Relaxed) || answer(stream, method, asked == path, range.as_deref(), file, kind, shared).is_err() {
            return;
        }
    }
}

fn plain(stream: &mut TcpStream, status: &str, more: &str) -> std::io::Result<()> {
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n{more}Connection: keep-alive\r\n\r\n").as_bytes())
}

fn answer(stream: &mut TcpStream, method: &str, ours: bool, range: Option<&str>, file: &Path, kind: Kind, shared: &Shared) -> std::io::Result<()> {
    if !ours {
        return plain(stream, "404 Not Found", "");
    }
    if method != "GET" && method != "HEAD" {
        return plain(stream, "405 Method Not Allowed", "Allow: GET, HEAD\r\n");
    }
    let Ok(mut file) = File::open(file) else { return plain(stream, "404 Not Found", "") };
    let len = file.metadata()?.len();
    let (status, from, to) = match part(range, len) {
        Ok(Some((from, to))) => ("206 Partial Content", from, to),
        Ok(None) => ("200 OK", 0, len.saturating_sub(1)),
        Err(()) => return plain(stream, "416 Range Not Satisfiable", &format!("Content-Range: bytes */{len}\r\n")),
    };
    let count = if len == 0 { 0 } else { to - from + 1 };
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\nConnection: keep-alive\r\n", kind.mime);
    if status.starts_with("206") {
        head.push_str(&format!("Content-Range: bytes {from}-{to}/{len}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if method == "HEAD" || count == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(from))?;
    // (File to socket: the kernel copies it, nothing passes through here.)
    let sent = std::io::copy(&mut file.take(count), stream)?;
    shared.sent.fetch_add(sent, Ordering::Relaxed);
    if sent == count { Ok(()) } else { Err(std::io::ErrorKind::UnexpectedEof.into()) }
}

/// The part of a file of `len` bytes a Range header asks for (first and
/// last byte); None: all of it (no header, or more than one range, which
/// may be answered with the whole); Err: none of it is there.
pub(crate) fn part(range: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((from, to)) = spec.split_once('-') else { return Ok(None) };
    let (from, to) = (from.trim(), to.trim());
    let last = len.checked_sub(1).ok_or(())?;
    match (from.parse::<u64>(), to.parse::<u64>()) {
        // "-500": the last 500.
        (Err(_), Ok(n)) if from.is_empty() => {
            if n == 0 {
                Err(())
            } else {
                Ok(Some((len.saturating_sub(n), last)))
            }
        }
        (Ok(a), _) if a > last => Err(()),
        (Ok(a), Ok(b)) if b >= a => Ok(Some((a, b.min(last)))),
        (Ok(a), Err(_)) if to.is_empty() => Ok(Some((a, last))),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_asked_for() {
        assert_eq!(part(None, 100), Ok(None));
        assert_eq!(part(Some("bytes=0-1"), 100), Ok(Some((0, 1))));
        assert_eq!(part(Some("bytes=10-"), 100), Ok(Some((10, 99))));
        assert_eq!(part(Some("bytes=90-500"), 100), Ok(Some((90, 99))));
        assert_eq!(part(Some("bytes=-30"), 100), Ok(Some((70, 99))));
        assert_eq!(part(Some("bytes=-300"), 100), Ok(Some((0, 99))));
        assert_eq!(part(Some("bytes=100-"), 100), Err(()));
        assert_eq!(part(Some("bytes=-0"), 100), Err(()));
        assert_eq!(part(Some("bytes=0-0"), 0), Err(()));
        // Not understood, or several: the whole.
        assert_eq!(part(Some("bytes=0-1,5-6"), 100), Ok(None));
        assert_eq!(part(Some("bytes=5-2"), 100), Ok(None));
        assert_eq!(part(Some("lines=1-2"), 100), Ok(None));
    }

    fn ask(at: &str, request: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect(at).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut all = Vec::new();
        stream.read_to_end(&mut all).unwrap();
        let end = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&all[..end]).into_owned(), all[end + 4..].to_vec())
    }

    /// The whole, a part, its head alone, what isn't there, and
    /// anything that isn't the film; and nothing once it's dropped.
    #[test]
    fn served_whole_and_in_parts_to_who_has_the_address() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-airplay-serve-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let film: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("a film.mp4"), &film).unwrap();
        let served = Served::start(&dir.join("a film.mp4"), Kind { mime: "video/mp4", ext: "mp4" }, "127.0.0.1".parse().unwrap()).unwrap();
        let (at, path) = served.url.strip_prefix("http://").unwrap().split_once('/').map(|(a, p)| (a.to_owned(), format!("/{p}"))).unwrap();
        assert!(path.ends_with("/film.mp4") && path.len() == "/film.mp4".len() + 33, "{path}");

        let (head, body) = ask(&at, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 50000\r\nAccept-Ranges: bytes"), "{head}");
        assert_eq!(body, film);
        // Two on one connection: a part, then the end.
        let (head, body) = ask(&at, &format!("GET {path} HTTP/1.1\r\nRange: bytes=100-199\r\n\r\nGET {path} HTTP/1.1\r\nrange: bytes=-10\r\n\r\n"));
        assert!(head.starts_with("HTTP/1.1 206 Partial Content") && head.contains("Content-Range: bytes 100-199/50000") && head.contains("Content-Length: 100"), "{head}");
        assert_eq!(&body[..100], &film[100..200]);
        let rest = String::from_utf8_lossy(&body[100..]).into_owned();
        assert!(rest.contains("Content-Range: bytes 49990-49999/50000"), "{rest}");
        assert_eq!(&body[body.len() - 10..], &film[49990..]);
        assert_eq!(served.sent(), 50_000 + 100 + 10);

        let (head, body) = ask(&at, &format!("HEAD {path} HTTP/1.1\r\n\r\n"));
        assert!(head.contains("Content-Length: 50000") && body.is_empty(), "{head}");
        assert!(ask(&at, &format!("GET {path} HTTP/1.1\r\nRange: bytes=50000-\r\n\r\n")).0.starts_with("HTTP/1.1 416"));
        assert!(ask(&at, &format!("POST {path} HTTP/1.1\r\n\r\n")).0.starts_with("HTTP/1.1 405"));
        // Not the film: the folder, the file by its name, another's guess.
        for other in ["/", "/a%20film.mp4", "/00000000000000000000000000000000/film.mp4", "/../../etc/passwd"] {
            assert!(ask(&at, &format!("GET {other} HTTP/1.1\r\n\r\n")).0.starts_with("HTTP/1.1 404"), "{other}");
        }
        drop(served);
        assert!(TcpStream::connect(&at).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
