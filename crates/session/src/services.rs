//! The session's services as others see them, and what they may ask of
//! it (`olo_protocol::services`: Preferences ▸ Services, `olo-session
//! services`): the list, and Restart, Stop and Start of one.
//!
//! No thread: the socket and each connection to it raise SIGIO (O_ASYNC,
//! owned by this process), which the session waits for with its other
//! signals, and what's waiting is taken without blocking. Nothing is
//! asked of it while nobody asks; a watcher is written to only when the
//! list changes.

use std::{
    io::{ErrorKind, Read, Write},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use olo_protocol::services::{Ask, Said, Service};

/// What one connection has sent and is.
struct Client {
    stream: UnixStream,
    /// What has come of a line not yet ended.
    partial: Vec<u8>,
    /// Sent the list each time it changes.
    watching: bool,
    /// Answered at least once: one that only asks is let go then.
    answered: bool,
    /// It has gone (its read ended, a write failed, a line ran too long).
    gone: bool,
    /// The list as it was last sent this watcher.
    seen: Option<Vec<Service>>,
}

/// The socket and who is connected to it.
pub struct Control {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<Client>,
}

/// SIGIO for this fd, to this process, and nothing it does blocks.
fn signalled(fd: i32) -> bool {
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETOWN, libc::getpid()) == 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK | libc::O_ASYNC) == 0
    }
}

impl Control {
    /// Listening at `path` (one left from a session before is replaced).
    pub fn bind(path: &Path) -> Option<Control> {
        let _ = std::fs::remove_file(path);
        let listener = match UnixListener::bind(path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("olo-session: can't listen for its services' requests at {}: {e}", path.display());
                return None;
            }
        };
        if !signalled(listener.as_raw_fd()) {
            eprintln!("olo-session: can't be signalled for its services' requests");
            return None;
        }
        Some(Control { listener, path: path.to_owned(), clients: Vec::new() })
    }

    /// What's been asked since last time, without waiting: each request
    /// with the connection to answer it on.
    pub fn take(&mut self) -> Vec<(usize, Result<Ask, String>)> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if signalled(stream.as_raw_fd()) {
                        self.clients.push(Client { stream, partial: Vec::new(), watching: false, answered: false, gone: false, seen: None });
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let mut asked = Vec::new();
        let mut buf = [0u8; 4096];
        for (i, c) in self.clients.iter_mut().enumerate().filter(|(_, c)| !c.gone) {
            loop {
                match c.stream.read(&mut buf) {
                    Ok(0) => {
                        c.gone = true;
                        break;
                    }
                    Ok(n) => c.partial.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                // (No request is anywhere near this long.)
                if c.partial.len() > 64 * 1024 {
                    c.gone = true;
                    break;
                }
            }
            while let Some(end) = c.partial.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = c.partial.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line[..end]).trim().to_owned();
                if !line.is_empty() {
                    asked.push((i, serde_json::from_str::<Ask>(&line).map_err(|e| format!("not a request: {e}"))));
                }
            }
        }
        asked
    }

    /// `said` to connection `i`; a watcher keeps being sent the list.
    pub fn answer(&mut self, i: usize, said: &Said, watch: bool) {
        if let Some(c) = self.clients.get_mut(i) {
            c.watching |= watch;
            c.answered = true;
            if let Said::Services { services } = said {
                c.seen = Some(services.clone());
            }
            if !send(&mut c.stream, said) {
                c.gone = true;
            }
        }
    }

    /// Someone is watching the list (only then is it made, each turn).
    pub fn watched(&self) -> bool {
        self.clients.iter().any(|c| c.watching && !c.gone)
    }

    /// The connections that only asked, answered, let go (what was
    /// written is still read at the other end before it ends).
    pub fn done_with_askers(&mut self) {
        for c in self.clients.iter_mut().filter(|c| c.answered && !c.watching && !c.gone) {
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
            c.gone = true;
        }
    }

    /// The list as it is now, to each watcher it isn't what it was last
    /// sent (`now`, when someone watches: [`Self::watched`]); then the
    /// connections that have gone are dropped.
    pub fn changed(&mut self, now: Option<&[Service]>) {
        if let Some(now) = now {
            let said = Said::Services { services: now.to_vec() };
            for c in self.clients.iter_mut().filter(|c| c.watching && !c.gone && c.seen.as_deref() != Some(now)) {
                c.seen = Some(now.to_vec());
                if !send(&mut c.stream, &said) {
                    c.gone = true;
                }
            }
        }
        self.clients.retain(|c| !c.gone);
    }

    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One line, all of it or nothing (a watcher that doesn't read is let
/// go rather than waited on: the session never blocks on it).
fn send(stream: &mut UnixStream, said: &Said) -> bool {
    let Ok(mut line) = serde_json::to_vec(said) else { return false };
    line.push(b'\n');
    let mut at = 0;
    let mut tries = 0;
    while at < line.len() {
        match stream.write(&line[at..]) {
            Ok(0) => return false,
            Ok(n) => at += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::WouldBlock && tries < 20 => {
                // (A list is a few kilobytes: a socket's buffer takes it
                // at once unless the other end has stopped reading.)
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
    true
}

/// `olo-session services|restart|stop|start`: asks the session of this
/// display, prints its answer (`--json`: as it came), and whether it
/// went (the exit status).
pub fn command(args: &[String]) -> i32 {
    let verb = args.first().map(String::as_str).unwrap_or("services");
    let json = args.iter().any(|a| a == "--json");
    let named = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned();
    let ask = match (verb, named) {
        ("services", _) => Ask::List,
        ("restart", Some(program)) => Ask::Restart { program },
        ("stop", Some(program)) => Ask::Stop { program },
        ("start", Some(program)) => Ask::Start { program },
        _ => {
            eprintln!("usage: olo-session services [--json] | restart|stop|start PROGRAM");
            return 2;
        }
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = olo_protocol::services::socket_path(&runtime, &olo_xdg::display_name());
    let said = (|| -> std::io::Result<Said> {
        let mut stream = UnixStream::connect(&path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut line = serde_json::to_vec(&ask).map_err(std::io::Error::other)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        let mut reply = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(stream), &mut reply)?;
        serde_json::from_str(&reply).map_err(std::io::Error::other)
    })();
    match said {
        Err(e) => {
            eprintln!("olo-session: no answer from the session at {}: {e}", path.display());
            1
        }
        Ok(Said::Error { message }) => {
            eprintln!("olo-session: {message}");
            1
        }
        Ok(said @ Said::Services { .. }) => {
            if json {
                println!("{}", serde_json::to_string(&said).unwrap_or_default());
            } else if let Said::Services { services } = said {
                print!("{}", table(&services, crate::now_secs()));
            }
            0
        }
    }
}

/// The list as a person reads it in a terminal.
pub fn table(services: &[Service], now: u64) -> String {
    use olo_protocol::services::State;
    let mut out = String::new();
    for s in services {
        let (name, _) = olo_protocol::services::about(&s.program);
        let state = match s.state {
            State::Running if s.since > 0 => format!("running for {}", span(now.saturating_sub(s.since))),
            State::Running => "running".to_owned(),
            State::Stopped => "stopped".to_owned(),
            State::NotWanted => "turned off".to_owned(),
            State::GaveUp => "left stopped after ending too often".to_owned(),
        };
        let restarts = if s.restarts > 0 { format!(", started again {}×", s.restarts) } else { String::new() };
        let pid = if s.pid > 0 { format!(" (pid {})", s.pid) } else { String::new() };
        out.push_str(&format!("{name:<20} {:<16} {state}{restarts}{pid}\n", s.program));
    }
    out
}

/// How long, in the largest unit it has whole ("1 minute", "3 hours").
fn span(secs: u64) -> String {
    let (n, unit) = match secs {
        0..60 => (secs, "second"),
        60..3600 => (secs / 60, "minute"),
        3600..86400 => (secs / 3600, "hour"),
        _ => (secs / 86400, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_protocol::services::State;

    fn service(program: &str, state: State, since: u64) -> Service {
        Service { program: program.into(), args: Vec::new(), pid: if state == State::Running { 7 } else { 0 }, since, restarts: 0, state, window_server: false }
    }

    #[test]
    fn asked_and_answered_over_the_socket() {
        // (The socket raises SIGIO, which the session blocks and waits for;
        // a test process would end of it.)
        // SAFETY: setting a signal's disposition.
        unsafe { libc::signal(libc::SIGIO, libc::SIG_IGN) };
        let dir = std::env::temp_dir().join(format!("olo-session-services-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("s.sock");
        let mut control = Control::bind(&path).expect("bound");
        let mut asker = UnixStream::connect(&path).unwrap();
        asker.write_all(b"{\"do\":\"list\"}\n{\"do\":\"stop\",\"program\":\"olo-pool\"}\nnonsense\n").unwrap();
        let mut asked = Vec::new();
        for _ in 0..50 {
            asked.extend(control.take());
            if asked.len() >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert_eq!(asked[0].1, Ok(Ask::List));
        assert_eq!(asked[1].1, Ok(Ask::Stop { program: "olo-pool".into() }));
        assert!(asked[2].1.is_err());
        let list = vec![service("olo-pool", State::Running, 1)];
        control.answer(asked[0].0, &Said::Services { services: list.clone() }, true);
        let mut reader = std::io::BufReader::new(asker.try_clone().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        assert_eq!(serde_json::from_str::<Said>(&line).unwrap(), Said::Services { services: list.clone() });
        // A watcher is sent the list when it changes, and only then.
        // (Not again: it has this list.)
        control.changed(Some(&list));
        let changed = vec![service("olo-pool", State::Stopped, 0)];
        assert!(control.watched());
        control.changed(Some(&changed));
        control.changed(Some(&changed));
        line.clear();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        assert_eq!(serde_json::from_str::<Said>(&line).unwrap(), Said::Services { services: changed });
        asker.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
        line.clear();
        assert!(std::io::BufRead::read_line(&mut reader, &mut line).is_err() || line.is_empty(), "sent again unchanged: {line}");
        control.remove();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_table_for_a_terminal() {
        let mut s = service("olo-pool", State::Running, 940);
        s.restarts = 2;
        let t = table(&[s, service("olo-passwords", State::NotWanted, 0)], 1000);
        assert!(t.starts_with("Token Pool           olo-pool         running for 1 minute, started again 2× (pid 7)\n"), "{t}");
        assert!(t.contains("SSH Agent            olo-passwords    turned off\n"), "{t}");
    }
}
