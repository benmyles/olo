//! For ureq's users: a download that goes on where it broke off. Read as
//! any body is; when a read fails (its connection cut by a network
//! change, the server gone quiet past the agent's read timeout, a reset),
//! the rest is asked for (`Range`, from where it got to; `If-Range`, so
//! it's still the same file) on whatever network there is now, once there
//! is one, up to [`TRIES`] times. A server that takes no ranges sends it
//! whole again, and what's had already is passed over.

use std::{
    io::{self, Read},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Times a download goes on after breaking off.
pub const TRIES: u32 = 8;
/// How long it waits for a network to come back.
const PATIENCE: Duration = Duration::from_secs(10 * 60);

pub struct Download<'a> {
    agent: ureq::Agent,
    url: String,
    accept: Option<String>,
    reader: Option<ureq::BodyReader<'static>>,
    /// Bytes read so far, and how many there are (if said).
    got: u64,
    total: Option<u64>,
    /// Bytes to pass over (a server that sent it whole again).
    skip: u64,
    /// Which file it is (its `ETag`, else `Last-Modified`).
    version: Option<String>,
    stop: Option<&'a AtomicBool>,
    tries: u32,
}

fn header(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

impl<'a> Download<'a> {
    /// `url`'s body, from `agent` (asking for `accept`, if given); `stop`,
    /// if given, ends a wait for the network (with [`crate::nudge`]). The
    /// first answer's failure is the agent's own error (a status, …),
    /// after trying again across a network change.
    pub fn start(agent: &ureq::Agent, url: &str, accept: Option<&str>, stop: Option<&'a AtomicBool>) -> Result<Download<'a>, ureq::Error> {
        let resp = crate::across_changes(PATIENCE, || {
            let mut req = agent.get(url);
            if let Some(a) = accept {
                req = req.header("Accept", a);
            }
            req.call()
        })?;
        let total = header(&resp, "content-length").and_then(|v| v.parse().ok());
        let version = header(&resp, "etag").filter(|e| !e.starts_with("W/")).or_else(|| header(&resp, "last-modified"));
        Ok(Download { agent: agent.clone(), url: url.to_owned(), accept: accept.map(str::to_owned), reader: Some(resp.into_body().into_reader()), got: 0, total, skip: 0, version, stop, tries: 0 })
    }

    /// Its size, if the server said.
    pub fn total(&self) -> Option<u64> {
        self.total
    }

    fn stopped(&self) -> bool {
        self.stop.is_some_and(|s| s.load(Ordering::Relaxed))
    }

    /// The rest, asked for again.
    fn resume(&mut self) -> io::Result<()> {
        let mut req = self.agent.get(&self.url).header("Range", format!("bytes={}-", self.got));
        if let Some(a) = &self.accept {
            req = req.header("Accept", a);
        }
        if let Some(v) = &self.version {
            req = req.header("If-Range", v);
        }
        let resp = req.call().map_err(io::Error::other)?;
        match resp.status().as_u16() {
            206 => {
                let from = header(&resp, "content-range").and_then(|r| r.strip_prefix("bytes ")?.split('-').next()?.parse::<u64>().ok());
                if from != Some(self.got) {
                    return Err(io::Error::other(format!("the server sent the rest from {from:?}, not {}", self.got)));
                }
                self.skip = 0;
            }
            200 => {
                // Whole again: the same file (no ranges taken), passed over
                // to where it got to; another (changed meanwhile), not.
                let version = header(&resp, "etag").filter(|e| !e.starts_with("W/")).or_else(|| header(&resp, "last-modified"));
                if self.version.is_some() && version != self.version {
                    return Err(io::Error::other("the file changed on the server while it was downloaded"));
                }
                self.skip = self.got;
            }
            s => return Err(io::Error::other(format!("the server answered {s} going on with the download"))),
        }
        self.reader = Some(resp.into_body().into_reader());
        Ok(())
    }

    /// Read on, from the reader there is.
    fn read_on(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(r) = self.reader.as_mut() else { return Err(io::Error::other("no answer to read")) };
        while self.skip > 0 {
            let mut scratch = [0u8; 64 << 10];
            let want = (self.skip as usize).min(scratch.len());
            let n = r.read(&mut scratch[..want])?;
            if n == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            self.skip -= n as u64;
        }
        let n = r.read(buf)?;
        if n == 0 && self.total.is_some_and(|t| self.got < t) {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        Ok(n)
    }
}

impl Read for Download<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let e = match self.read_on(buf) {
                Ok(n) => {
                    self.got += n as u64;
                    return Ok(n);
                }
                Err(e) => e,
            };
            self.reader = None;
            self.tries += 1;
            if self.tries > TRIES || self.stopped() {
                return Err(e);
            }
            tracing::info!("download: broke off at {} of {:?} bytes ({e}): going on", self.got, self.total);
            // No network: once there is one. Else a moment first (less,
            // if the network just changed), more each time.
            if !crate::online() {
                if !crate::wait_online(Some(Instant::now() + PATIENCE), &|| self.stopped()) {
                    return Err(e);
                }
            } else {
                crate::rest(Duration::from_millis(250) * (1 << self.tries.min(5)));
            }
            if self.stopped() {
                return Err(e);
            }
            if let Err(e) = self.resume() {
                tracing::info!("download: couldn't go on ({e})");
                self.reader = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    /// A server of `data` that breaks off the first answer after `cut`
    /// bytes, then answers ranges (or, `ranges` false, sends it whole).
    fn server(data: Vec<u8>, cut: usize, ranges: bool) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for (i, c) in l.incoming().take(2).enumerate() {
                let mut c = c.unwrap();
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                    if line.to_ascii_lowercase().starts_with("if-range") {
                        asked.push(line.trim().to_owned());
                    }
                }
                let from = if ranges { range.unwrap_or(0) } else { 0 };
                let head = if from > 0 {
                    format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{}\r\nETag: \"v1\"\r\n\r\n", data.len() - from, data.len() - 1, data.len())
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\n\r\n", data.len())
                };
                c.write_all(head.as_bytes()).unwrap();
                let end = if i == 0 { cut } else { data.len() };
                c.write_all(&data[from..end]).unwrap();
                c.flush().unwrap();
                // (Gone: the connection closes short of its length.)
            }
            asked
        });
        (url, h)
    }

    fn data() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn a_download_goes_on_where_it_broke_off() {
        let (url, h) = server(data(), 100_000, true);
        let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
        let mut d = Download::start(&agent, &url, None, None).unwrap();
        assert_eq!(d.total(), Some(300_000));
        let mut got = Vec::new();
        d.read_to_end(&mut got).unwrap();
        assert!(got == data(), "{} bytes", got.len());
        assert_eq!(h.join().unwrap(), ["if-range: \"v1\""]);
    }

    #[test]
    fn whole_again_is_passed_over() {
        let (url, h) = server(data(), 123_457, false);
        let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
        let mut d = Download::start(&agent, &url, None, None).unwrap();
        let mut got = Vec::new();
        d.read_to_end(&mut got).unwrap();
        assert!(got == data(), "{} bytes", got.len());
        h.join().unwrap();
    }
}
