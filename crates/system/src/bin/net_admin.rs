//! olo-net-admin: what Preferences ▸ Network changes as root.
//!
//!   olo-net-admin serve     requests on stdin, one JSON object a line,
//!                           each answered on stdout ({"ok":true} or
//!                           {"error":"…"}), until stdin ends
//!
//! Preferences starts it through polkit (pkexec, which asks for the
//! password with Olo's agent) the first time the panel needs root, and
//! keeps it while the panel is open, so the password is asked once. What
//! it's asked:
//!
//! - `{"conf": NetConf}`: the services' order and DNS of one's own,
//!   written to NetworkManager's conf.d (`olo_system::netconf`; search
//!   domains given alone onto each connection), NetworkManager told to
//!   read it again (`Reload`), and every link connected given it
//!   (`Reapply`), without disconnecting.
//! - `{"hosts": {"text": "…", "was": "…"}}`: /etc/hosts as `text`
//!   (checked: comments and entries only), in place, if it's still `was`.
//! - `{"shape": {"interface": "enp3s0", "shaping": Shaping}}`: the link
//!   shaped (`olo_system::shaping`), or Olo's shaping taken away.
//! - `{"profile": {"profile": PATH, "ipv4": Ipv4, "device": PATH}}`: a
//!   connection's IPv4 settings, and its link given them (for an account
//!   NetworkManager doesn't let change system connections itself).
//!
//! `OLO_NET_ROOT` (tests): files under that directory instead of `/`;
//! `OLO_NM_BUS`: NetworkManager there (`nm-fake`).

use std::io::{BufRead, Write};

use olo_system::{
    hosts::HostsFile,
    netconf::{self, NetConf},
    network::{self, Ipv4},
    shaping::{self, Shaping},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Request {
    Conf(NetConf),
    Hosts { text: String, was: String },
    Shape { interface: String, shaping: Shaping },
    Profile { profile: String, ipv4: Ipv4, device: Option<String> },
}

fn conf(c: &NetConf) -> Result<(), String> {
    c.check()?;
    let was = NetConf::read();
    let path = netconf::path();
    match c.render() {
        Some(text) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", path.display())),
            _ => {}
        },
    }
    let alone = |c: &NetConf| if c.searches_alone() { c.dns.searches.clone() } else { Vec::new() };
    let (old, new) = (alone(&was), alone(c));
    if old != new {
        network::set_profile_searches(&old, &new).map_err(|e| format!("the connections' search domains: {e}"))?;
    }
    network::reload().map_err(|e| format!("NetworkManager didn't read it again: {e}"))?;
    network::reapply_all().map_err(|e| format!("the links didn't take it: {e}"))
}

fn hosts(text: &str, was: &str) -> Result<(), String> {
    HostsFile::check(text)?;
    let path = olo_system::hosts::path();
    let now = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if now != was {
        return Err("/etc/hosts changed since it was read: look again".into());
    }
    // In place: the file keeps its owner, mode and SELinux label.
    let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", path.display()))
}

fn shape(interface: &str, s: &Shaping) -> Result<(), String> {
    let r = if s.is_on() { shaping::apply(interface, s) } else { shaping::clear(interface) };
    r.map_err(|e| format!("{interface}: {e}"))
}

fn profile(path: &str, ipv4: &Ipv4, device: Option<&str>) -> Result<(), String> {
    network::update_profile(path, |s| network::set_ipv4_in(s, ipv4)).map_err(|e| e.to_string())?;
    if let Some(d) = device {
        network::reapply(d).map_err(|e| format!("the link didn't take it: {e}"))?;
    }
    Ok(())
}

fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let r = match serde_json::from_str::<Request>(&line) {
            Err(e) => Err(format!("not a request: {e}")),
            Ok(Request::Conf(c)) => conf(&c),
            Ok(Request::Hosts { text, was }) => hosts(&text, &was),
            Ok(Request::Shape { interface, shaping }) => shape(&interface, &shaping),
            Ok(Request::Profile { profile: p, ipv4, device }) => profile(&p, &ipv4, device.as_deref()),
        };
        let answer = match r {
            Ok(()) => serde_json::json!({ "ok": true }),
            Err(e) => serde_json::json!({ "error": e }),
        };
        if writeln!(out, "{answer}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    0
}

fn main() {
    olo_crash::install();
    let code = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve(),
        _ => {
            eprintln!("usage: olo-net-admin serve (root; requests as JSON lines on stdin)");
            2
        }
    };
    std::process::exit(code);
}
