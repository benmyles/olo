//! The pool's clients running now, for the Clients window and `olo-pool
//! --clients`: each Claude Code and Codex process (its folder, when it
//! started, and where its connections go: through the pool's proxy, or
//! straight to a provider), and Codex's daemon, in which Codex's windows
//! run their conversations (its version, whether it started before the
//! pool's settings, its connections and its conversations). Read from
//! /proc, and the daemon over its control socket, so off the UI thread.

use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::SystemTime,
};

use serde::Serialize;

use crate::{
    codex_daemon::{self, Thread},
    model::Provider,
    peer::Program,
};

/// Where a process's connections go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Connections {
    /// To the pool's proxy.
    pub pool: usize,
    /// Straight to a server elsewhere, over HTTPS.
    pub direct: usize,
}

impl Connections {
    /// "through the pool", "straight to OpenAI", "both: …", "none open".
    pub fn describe(&self, provider: Provider) -> String {
        let company = match provider {
            Provider::Claude => "Anthropic",
            Provider::Codex => "OpenAI",
        };
        match (self.pool, self.direct) {
            (0, 0) => "no connection open".into(),
            (_, 0) => "through the pool".into(),
            (0, _) => format!("straight to {company}"),
            (p, d) => format!("{p} through the pool, {d} straight out"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    /// Seconds since the epoch.
    pub started: i64,
    pub cwd: Option<PathBuf>,
    pub connections: Connections,
}

#[derive(Debug, Clone, Serialize)]
pub struct Daemon {
    pub pid: i32,
    pub started: i64,
    /// Codex's version ("0.166.0"), from where its program is kept.
    pub version: Option<String>,
    /// It started before Codex's settings for the pool last changed, so
    /// runs on the ones before (`codex_daemon::stale`).
    pub stale: bool,
    pub connections: Connections,
    /// Its conversations (Err: it didn't say).
    pub threads: Result<Vec<Thread>, String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Running {
    pub claude: Vec<Process>,
    /// Codex's windows (and `codex exec`s).
    pub codex: Vec<Process>,
    pub daemon: Option<Daemon>,
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Established TCP connections by socket inode: (remote address, port),
/// the address as /proc/net/tcp writes it (IPv4 mapped into IPv6 too).
fn established() -> HashMap<u64, (String, u16)> {
    let mut out = HashMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else { continue };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            // State 01: established.
            if f.len() <= 9 || f[3] != "01" {
                continue;
            }
            let Some((addr, port)) = f[2].split_once(':') else { continue };
            let (Ok(port), Ok(inode)) = (u16::from_str_radix(port, 16), f[9].parse::<u64>()) else { continue };
            out.insert(inode, (addr.to_owned(), port));
        }
    }
    out
}

/// Whether /proc/net/tcp's address `a` is loopback (127.x, ::1, or 127.x
/// mapped into IPv6).
fn loopback(a: &str) -> bool {
    match a.len() {
        // IPv4 in the kernel's byte order: the first octet last.
        8 => a.ends_with("7F"),
        32 => a == "00000000000000000000000001000000" || (a.starts_with("0000000000000000FFFF0000") && a.ends_with("7F")),
        _ => false,
    }
}

/// Where `pid`'s connections go, with the pool's proxy on `proxy_port`.
fn connections(pid: u32, table: &HashMap<u64, (String, u16)>, proxy_port: u16) -> Connections {
    let mut c = Connections::default();
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return c };
    for fd in fds.flatten() {
        let Ok(link) = std::fs::read_link(fd.path()) else { continue };
        let Some(inode) = link.to_str().and_then(|l| l.strip_prefix("socket:[")?.strip_suffix(']')?.parse::<u64>().ok()) else { continue };
        let Some((addr, port)) = table.get(&inode) else { continue };
        if loopback(addr) {
            if *port == proxy_port {
                c.pool += 1;
            }
        } else if *port == 443 {
            c.direct += 1;
        }
    }
    c
}

/// What's running now, with the pool's proxy on `proxy_port`.
pub fn scan(proxy_port: u16) -> Running {
    let table = established();
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX);
    let daemon = codex_daemon::running();
    let mut out = Running::default();
    let Ok(entries) = std::fs::read_dir("/proc") else { return out };
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if !e.metadata().is_ok_and(|m| m.uid() == uid) {
            continue;
        }
        let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else { continue };
        let exe = exe.to_str().and_then(|x| x.strip_suffix(" (deleted)")).map(PathBuf::from).unwrap_or(exe);
        let args: Vec<String> = std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| b.split(|&c| c == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
            .unwrap_or_default();
        let program = Program { pid, exe, args };
        let provider = [Provider::Claude, Provider::Codex].into_iter().find(|&p| program.is_client(p));
        // Codex's daemon and its keeper are Codex's, not windows.
        if provider.is_none() || program.args.iter().any(|a| a == "app-server") {
            continue;
        }
        let Some(started) = codex_daemon::started(pid as i32) else { continue };
        let process = Process {
            pid,
            started: secs(started),
            cwd: std::fs::read_link(format!("/proc/{pid}/cwd")).ok(),
            connections: connections(pid, &table, proxy_port),
        };
        match provider {
            Some(Provider::Claude) => out.claude.push(process),
            _ => out.codex.push(process),
        }
    }
    out.claude.sort_by_key(|p| p.started);
    out.codex.sort_by_key(|p| p.started);
    out.daemon = daemon.map(|d| Daemon {
        pid: d.pid,
        started: secs(d.started),
        version: codex_daemon::version(d.pid),
        stale: codex_daemon::stale(&d),
        connections: connections(d.pid as u32, &table, proxy_port),
        threads: codex_daemon::threads().map_err(|e| e.to_string()),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_where_connections_go() {
        assert!(loopback("0100007F") && loopback("00000000000000000000000001000000") && loopback("0000000000000000FFFF00000100007F"));
        assert!(!loopback("D19B40AC") && !loopback("0000000000000000FFFF0000D19B40AC"));
        // This process's own connection to a stand-in for the proxy.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _server = listener.accept().unwrap();
        let c = connections(std::process::id(), &established(), port);
        assert_eq!(c, Connections { pool: 1, direct: 0 });
        assert_eq!(c.describe(Provider::Codex), "through the pool");
        assert_eq!(Connections { pool: 0, direct: 2 }.describe(Provider::Codex), "straight to OpenAI");
        assert_eq!(Connections::default().describe(Provider::Claude), "no connection open");
    }
}
