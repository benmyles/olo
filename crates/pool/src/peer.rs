//! Which program is on the other end of a loopback connection, read from
//! /proc: the socket's inode from `/proc/net/tcp`, then the process
//! holding it. The proxy (`intercept`) opens a provider's host only for
//! the pool's own clients, Claude Code and Codex; anything else that
//! inherited their proxy setting (a script Claude Code runs, which needn't
//! trust the pool's certificate) goes straight through, as without the
//! pool.

use olo_sync::Mutex;
use std::{
    net::{SocketAddr, SocketAddrV4},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use crate::model::Provider;

/// A program: its process, executable and arguments.
#[derive(Debug, Clone)]
pub struct Program {
    pub pid: u32,
    pub exe: PathBuf,
    pub args: Vec<String>,
}

impl Program {
    /// Whether it is `provider`'s own client: Claude Code (its own build,
    /// kept in `…/claude/versions/`, or the npm package) or Codex (its
    /// binary, which the npm package also runs).
    pub fn is_client(&self, provider: Provider) -> bool {
        let name = |p: &str| Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
        let exe = self.exe.to_string_lossy();
        let (exe_name, arg0) = (name(&exe), self.args.first().map(|a| name(a)).unwrap_or_default());
        match provider {
            Provider::Claude => exe.contains("/claude/versions/") || exe_name == "claude" || arg0 == "claude" || self.args.iter().any(|a| a.contains("@anthropic-ai/claude-code")),
            Provider::Codex => exe_name == "codex" || arg0 == "codex",
        }
    }
}

/// The program that made the connection from `peer` to `local` (both
/// loopback), if this user's and /proc says.
pub fn program(peer: SocketAddr, local: SocketAddr) -> Option<Program> {
    let inode = socket_inode(peer, local)?;
    let pid = holder(inode)?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    // (A binary replaced while it runs, as Claude Code updates itself.)
    let exe = exe.to_str().and_then(|e| e.strip_suffix(" (deleted)")).map(PathBuf::from).unwrap_or(exe);
    let args =
        std::fs::read(format!("/proc/{pid}/cmdline")).ok().map(|b| b.split(|&c| c == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()).unwrap_or_default();
    Some(Program { pid, exe, args })
}

/// /proc/net/tcp's form of an address: the IPv4 address as the kernel's
/// own word, then the port, in hex.
fn hex(a: SocketAddrV4) -> String {
    format!("{:08X}:{:04X}", u32::from_ne_bytes(a.ip().octets()), a.port())
}

/// The inode of the client's end of the connection.
fn socket_inode(peer: SocketAddr, local: SocketAddr) -> Option<u64> {
    let (SocketAddr::V4(peer), SocketAddr::V4(local)) = (peer, local) else { return None };
    let (from, to) = (hex(peer), hex(local));
    let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
    table.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 9 && f[1] == from && f[2] == to).then(|| f[9].parse().ok()).flatten()
    })
}

/// The processes found lately (most recent first): a client makes its
/// connections over and over, so they're looked at first.
static RECENT: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// The process holding socket `inode` among this user's.
fn holder(inode: u64) -> Option<u32> {
    let link = format!("socket:[{inode}]");
    let holds = |pid: u32| std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.as_os_str() == link.as_str())));
    let recent = RECENT.lock().unwrap().clone();
    let found = recent.iter().copied().find(|&p| holds(p)).or_else(|| {
        let uid = std::fs::metadata("/proc/self").ok()?.uid();
        std::fs::read_dir("/proc")
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok().filter(|_| e.metadata().is_ok_and(|m| m.uid() == uid)))
            .find(|&p| !recent.contains(&p) && holds(p))
    })?;
    let mut recent = RECENT.lock().unwrap();
    recent.retain(|&p| p != found);
    recent.insert(0, found);
    recent.truncate(8);
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(exe: &str, args: &[&str]) -> Program {
        Program { pid: 1, exe: exe.into(), args: args.iter().map(|a| (*a).to_owned()).collect() }
    }

    #[test]
    fn knows_the_clients() {
        let claude = program("/home/u/.local/share/claude/versions/2.1.314", &["claude", "-p", "hi"]);
        assert!(claude.is_client(Provider::Claude) && !claude.is_client(Provider::Codex));
        assert!(program("/usr/bin/node", &["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"]).is_client(Provider::Claude));
        let codex = program("/home/u/.codex/packages/standalone/releases/0.166.0-aarch64-unknown-linux-musl/bin/codex", &["codex", "exec"]);
        assert!(codex.is_client(Provider::Codex) && !codex.is_client(Provider::Claude));
        // What they run is not them.
        for other in [
            program("/usr/bin/python3.14", &["python3", "call_claude.py"]),
            program("/usr/bin/curl", &["curl", "https://api.anthropic.com/v1/messages"]),
            program("/usr/bin/node", &["node", "script.js"]),
        ] {
            assert!(!other.is_client(Provider::Claude) && !other.is_client(Provider::Codex), "{other:?}");
        }
    }

    #[test]
    fn finds_who_connected() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, peer) = listener.accept().unwrap();
        let found = super::program(peer, server.local_addr().unwrap()).expect("found");
        assert_eq!(found.pid, std::process::id());
        assert_eq!(found.exe, std::env::current_exe().unwrap());
        drop(client);
    }
}
