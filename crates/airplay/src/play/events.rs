//! The event channel: a second connection, which the sender makes (to
//! the port SETUP's answer names) and the receiver then speaks on: what
//! it is, and what its player is doing. Each thing it says is a request
//! that wants its "200 OK", or the receiver closes the session within
//! half a minute.

use std::{
    io::{Read as _, Write as _},
    net::{Shutdown, SocketAddr, TcpStream},
    thread::JoinHandle,
    time::Duration,
};

use plist::Value;

use super::lists as pl;
use crate::pairing::Cipher;

pub(crate) enum Event {
    /// Something it said (what was inside `params.data`, where it was
    /// put there).
    Said(Value),
    /// It closed the channel.
    Closed,
}

pub(crate) struct Events {
    stream: TcpStream,
    thread: Option<JoinHandle<()>>,
}

impl Events {
    /// Connected and listening until dropped; `heard` is called on the
    /// channel's own thread.
    pub fn connect(to: SocketAddr, mut cipher: Cipher, wait: Duration, mut heard: impl FnMut(Event) + Send + 'static) -> std::io::Result<Events> {
        let stream = TcpStream::connect_timeout(&to, wait)?;
        let _ = stream.set_nodelay(true);
        let mut ours = stream.try_clone()?;
        let thread = std::thread::Builder::new().name("airplay-events".into()).spawn(move || {
            let (mut came, mut buf) = (Vec::new(), [0u8; 4096]);
            'reading: while let Ok(n @ 1..) = ours.read(&mut buf) {
                let Ok(plain) = cipher.open(&buf[..n]) else { break };
                came.extend(plain);
                while let Some((first, head, body, used)) = request(&came) {
                    came.drain(..used);
                    if let Some(said) = pl::read(&body) {
                        // (Its player's news comes wrapped: a list inside a list.)
                        let inner = pl::at(&said, &["params", "data"]).and_then(Value::as_data).and_then(pl::read);
                        heard(Event::Said(inner.unwrap_or(said)));
                    }
                    let protocol = first.split(' ').nth(2).unwrap_or("RTSP/1.0");
                    let mut reply = format!("{protocol} 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0");
                    for name in ["server", "cseq"] {
                        if let Some((_, value)) = head.iter().find(|(n, _)| n == name) {
                            reply.push_str(&format!("\r\n{}: {value}", if name == "cseq" { "CSeq" } else { "Server" }));
                        }
                    }
                    reply.push_str("\r\n\r\n");
                    if ours.write_all(&cipher.seal(reply.as_bytes())).is_err() {
                        break 'reading;
                    }
                }
            }
            heard(Event::Closed);
        })?;
        Ok(Events { stream, thread: Some(thread) })
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A whole request at the start of `came`: its first line, its headers
/// (names in lower case), its body, and how many bytes it was.
#[allow(clippy::type_complexity)]
pub(crate) fn request(came: &[u8]) -> Option<(String, Vec<(String, String)>, Vec<u8>, usize)> {
    let end = came.windows(4).position(|w| w == b"\r\n\r\n")?;
    let text = String::from_utf8_lossy(&came[..end]);
    let mut lines = text.split("\r\n");
    let first = lines.next()?.to_owned();
    let head: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let length = head.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    let body = came.get(end + 4..end + 4 + length)?.to_vec();
    Some((first, head, body, end + 4 + length))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn a_request_is_whole_when_its_body_is() {
        let came = b"POST /command RTSP/1.0\r\nCSeq: 4\r\nContent-Length: 3\r\n\r\nabcPOST";
        assert!(request(&came[..30]).is_none());
        assert!(request(&came[..came.len() - 5]).is_none());
        let (first, head, body, used) = request(came).unwrap();
        assert_eq!((first.as_str(), body.as_slice(), &came[used..]), ("POST /command RTSP/1.0", &b"abc"[..], &b"POST"[..]));
        assert_eq!(head, [("cseq".to_owned(), "4".to_owned()), ("content-length".to_owned(), "3".to_owned())]);
    }

    /// What a receiver says is heard (unwrapped where it wraps it) and
    /// answered with its own number; its closing is heard too.
    #[test]
    fn heard_and_answered() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (ours, theirs) = ([3u8; 32], [4u8; 32]);
        let (tx, rx) = mpsc::channel();
        let events = Events::connect(listener.local_addr().unwrap(), Cipher::new(&ours, &theirs), Duration::from_secs(5), move |e| {
            let _ = tx.send(match e {
                Event::Said(v) => pl::text(&v, &["type"]).unwrap_or("?").to_owned(),
                Event::Closed => "closed".to_owned(),
            });
        })
        .unwrap();
        let (mut tv, _) = listener.accept().unwrap();
        tv.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut cipher = Cipher::new(&theirs, &ours);
        let plain = pl::write(&pl::dict([("type", Value::from("updateInfo"))]));
        let wrapped = pl::write(&pl::dict([("params", pl::dict([("data", Value::Data(pl::write(&pl::dict([("type", Value::from("playbackState"))]))))]))]));
        let mut both = Vec::new();
        for (n, body) in [plain, wrapped].iter().enumerate() {
            both.extend(format!("POST /command RTSP/1.0\r\nCSeq: {n}\r\nServer: AirTunes/1\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes());
            both.extend(body);
        }
        tv.write_all(&cipher.seal(&both)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "updateInfo");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "playbackState");
        let (mut answers, mut buf) = (Vec::new(), [0u8; 1024]);
        while answers.windows(4).filter(|w| w == b"\r\n\r\n").count() < 2 {
            let n = tv.read(&mut buf).unwrap();
            answers.extend(cipher.open(&buf[..n]).unwrap());
        }
        let answers = String::from_utf8(answers).unwrap();
        assert!(answers.starts_with("RTSP/1.0 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\nServer: AirTunes/1\r\nCSeq: 0\r\n\r\n"), "{answers}");
        assert!(answers.ends_with("CSeq: 1\r\n\r\n"));
        drop(tv);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "closed");
        drop(events);
    }
}
