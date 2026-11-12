//! The pairing's messages to a receiver: HTTP on its AirPlay port, one
//! connection kept, enciphered from pair-verify on ([`Http::encipher`]).

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use super::{Cipher, Error, Post};

pub struct Http {
    stream: TcpStream,
    host: String,
    cipher: Option<Cipher>,
    /// What has come (deciphered) and isn't a whole answer yet.
    came: Vec<u8>,
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

impl Http {
    /// Connect to a receiver's AirPlay port, waiting `wait` at most for
    /// that and for each answer.
    pub fn connect(to: SocketAddr, wait: Duration) -> Result<Http, Error> {
        let stream = TcpStream::connect_timeout(&to, wait).map_err(io)?;
        stream.set_read_timeout(Some(wait)).map_err(io)?;
        stream.set_write_timeout(Some(wait)).map_err(io)?;
        let _ = stream.set_nodelay(true);
        Ok(Http { stream, host: to.to_string(), cipher: None, came: Vec::new() })
    }

    /// From here on what is sent and read is enciphered (after
    /// [`super::verify`]: `Session::control`).
    pub fn encipher(&mut self, cipher: Cipher) {
        self.cipher = Some(cipher);
    }

    /// A request and its answer: the status and the body.
    pub fn request(&mut self, method: &str, path: &str, content_type: &str, body: &[u8]) -> Result<(u16, Vec<u8>), Error> {
        let host = self.host.clone();
        let head = [
            ("Host", host.as_str()),
            ("User-Agent", "AirPlay/320.20"),
            ("Connection", "keep-alive"),
            ("X-Apple-HKP", "3"),
            ("Content-Type", content_type),
            ("Content-Length", &body.len().to_string()),
        ];
        let answer = self.exchange(&format!("{method} {path} HTTP/1.1"), &head, body)?;
        Ok((answer.status, answer.body))
    }

    /// A message as given (its first line, "SETUP rtsp://… RTSP/1.0",
    /// its headers and its body) and the receiver's answer to it.
    pub fn exchange(&mut self, first: &str, head: &[(&str, &str)], body: &[u8]) -> Result<Answer, Error> {
        let mut message = String::from(first);
        for (name, value) in head {
            message.push_str(&format!("\r\n{name}: {value}"));
        }
        message.push_str("\r\n\r\n");
        let mut message = message.into_bytes();
        message.extend(body);
        let message = match &mut self.cipher {
            Some(cipher) => cipher.seal(&message),
            None => message,
        };
        self.stream.write_all(&message).map_err(io)?;
        loop {
            if let Some((answer, used)) = whole(&self.came)? {
                self.came.drain(..used);
                return Ok(answer);
            }
            let mut buf = [0u8; 4096];
            let n = self.stream.read(&mut buf).map_err(io)?;
            if n == 0 {
                return Err(Error::Io("it closed the connection".into()));
            }
            match &mut self.cipher {
                Some(cipher) => self.came.extend(cipher.open(&buf[..n]).map_err(|_| Error::Imposter)?),
                None => self.came.extend(&buf[..n]),
            }
        }
    }

    /// This end's address on the connection: the one the receiver
    /// reaches this computer by.
    pub fn local(&self) -> Option<SocketAddr> {
        self.stream.local_addr().ok()
    }

    /// The receiver's address.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.stream.peer_addr().ok()
    }

    /// How long an answer is waited for from here on.
    pub fn wait(&mut self, wait: Duration) {
        let _ = self.stream.set_read_timeout(Some(wait));
        let _ = self.stream.set_write_timeout(Some(wait));
    }

    /// Something to close the connection with from another thread (a
    /// request waiting on it then ends with [`Error::Io`]).
    pub fn closer(&self) -> Option<Closer> {
        self.stream.try_clone().ok().map(Closer)
    }
}

/// Closes a connection from elsewhere ([`Http::closer`]).
pub struct Closer(TcpStream);

impl Closer {
    pub fn close(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// A receiver's answer to a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub status: u16,
    /// Its headers, names in lower case.
    pub head: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.head.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// A whole answer at the start of `came`, if it's all there, and how
/// many bytes it was.
fn whole(came: &[u8]) -> Result<Option<(Answer, usize)>, Error> {
    let Some(head_end) = came.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if came.len() > 16384 { Err(Error::Unexpected("no end to its answer's head")) } else { Ok(None) };
    };
    let text = String::from_utf8_lossy(&came[..head_end]);
    let mut lines = text.split("\r\n");
    // ("HTTP/1.1 200 OK"; AirPlay also answers as RTSP.)
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse::<u16>().ok()).ok_or(Error::Unexpected("not HTTP"))?;
    let head: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let length = head.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    let (from, to) = (head_end + 4, head_end + 4 + length);
    Ok(came.get(from..to).map(|body| (Answer { status, head, body: body.to_vec() }, to)))
}

impl Post for Http {
    fn post(&mut self, path: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        match self.request("POST", path, "application/octet-stream", body)? {
            (status, body) if status < 400 || !body.is_empty() => Ok(body),
            (status, _) => Err(Error::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_whole_when_its_body_is() {
        let answer = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\ncontent-length: 5\r\n\r\nhelloHTTP/1.1";
        assert_eq!(whole(&answer[..20]).unwrap(), None);
        assert_eq!(whole(&answer[..answer.len() - 10]).unwrap(), None);
        let (got, used) = whole(answer).unwrap().unwrap();
        assert_eq!((got.status, got.body.as_slice(), &answer[used..]), (200, &b"hello"[..], &b"HTTP/1.1"[..]));
        assert_eq!(got.header("content-type"), Some("application/octet-stream"));
        // No body, as RTSP, and what isn't an answer at all.
        let (got, used) = whole(b"RTSP/1.0 470 Connection Authorization Required\r\nCSeq: 3\r\n\r\n").unwrap().unwrap();
        assert_eq!((got.status, got.header("cseq"), got.body.len(), used), (470, Some("3"), 0, 59));
        assert!(whole(b"gibberish\r\n\r\n").is_err());
    }
}
