//! `/etc/hosts`, as Preferences ▸ Network ▸ Hosts edits it: each entry
//! (an address, its names, a comment), on or off, kept where it is among
//! the file's comments and blank lines, which stay as they are. An entry
//! turned off is written as a comment Olo knows again (`#off 10.0.0.5
//! nas`). `olo-net-admin` writes it as root, in place (the file keeps its
//! inode, owner, mode and SELinux label), and only if it hasn't changed
//! since it was read.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub fn path() -> PathBuf {
    crate::netconf::root().join("etc/hosts")
}

/// An entry turned off.
const OFF: &str = "#off ";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub on: bool,
    /// "10.0.0.5", "fe80::1".
    pub address: String,
    /// "nas", "nas.home.example".
    pub names: Vec<String>,
    pub comment: String,
}

/// A host name: labels of letters, digits, hyphens and underscores.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

impl Host {
    /// The machine's own (localhost, its name): shown, not to be edited.
    pub fn system(&self) -> bool {
        self.names.iter().any(|n| n == "localhost" || n.starts_with("localhost.") || n.starts_with("localhost4") || n.starts_with("localhost6") || n.starts_with("ip6-"))
    }

    pub fn check(&self) -> Result<(), String> {
        if self.address.parse::<std::net::IpAddr>().is_err() {
            return Err(format!("“{}” isn't an IP address", self.address));
        }
        if self.names.is_empty() {
            return Err(format!("{} has no name", self.address));
        }
        if let Some(bad) = self.names.iter().find(|n| !valid_name(n)) {
            return Err(format!("“{bad}” isn't a host name"));
        }
        if self.comment.contains(['\n', '\r']) {
            return Err("a comment is one line".into());
        }
        Ok(())
    }

    fn line(&self) -> String {
        let comment = if self.comment.trim().is_empty() { String::new() } else { format!("\t# {}", self.comment.trim()) };
        format!("{}{}\t{}{comment}", if self.on { "" } else { OFF }, self.address, self.names.join(" "))
    }

    /// An entry in `line`, if it's one (turned off, if Olo turned it off).
    fn parse(line: &str) -> Option<Host> {
        let (on, rest) = match line.strip_prefix(OFF) {
            Some(r) => (false, r),
            None if line.trim_start().starts_with('#') => return None,
            None => (true, line),
        };
        let (body, comment) = match rest.split_once('#') {
            Some((b, c)) => (b, c.trim().to_owned()),
            None => (rest, String::new()),
        };
        let mut words = body.split_whitespace();
        let address = words.next()?.to_owned();
        address.parse::<std::net::IpAddr>().ok()?;
        let names: Vec<String> = words.map(str::to_owned).collect();
        (!names.is_empty()).then_some(Host { on, address, names, comment })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// An entry, and its line as written.
    Host(Host, String),
    Other(String),
}

/// The file, line by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostsFile {
    lines: Vec<Line>,
}

impl HostsFile {
    pub fn read() -> std::io::Result<(HostsFile, String)> {
        let text = std::fs::read_to_string(path())?;
        Ok((HostsFile::parse(&text), text))
    }

    pub fn parse(text: &str) -> HostsFile {
        HostsFile { lines: text.lines().map(|l| Host::parse(l).map_or_else(|| Line::Other(l.to_owned()), |h| Line::Host(h, l.to_owned()))).collect() }
    }

    /// Its entries, in order.
    pub fn hosts(&self) -> Vec<Host> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                Line::Host(h, _) => Some(h.clone()),
                Line::Other(_) => None,
            })
            .collect()
    }

    /// The file with `hosts` for its entries: each in the place of the
    /// one it was (in order), any more after the last line, the rest of
    /// the lines kept. An entry unchanged keeps its line as written.
    pub fn with(&self, hosts: &[Host]) -> String {
        let mut next = hosts.iter();
        let mut out: Vec<String> = Vec::new();
        for line in &self.lines {
            match line {
                Line::Other(text) => out.push(text.clone()),
                Line::Host(was, written) => {
                    if let Some(h) = next.next() {
                        out.push(if h == was { written.clone() } else { h.line() });
                    }
                }
            }
        }
        out.extend(next.map(Host::line));
        let mut text = out.join("\n");
        text.push('\n');
        text
    }

    /// Checked before it's written as root: every line a comment, blank
    /// or an entry that is what it says.
    pub fn check(text: &str) -> Result<(), String> {
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || (t.starts_with('#') && !t.starts_with(OFF.trim_end())) {
                continue;
            }
            Host::parse(line).ok_or_else(|| format!("“{line}” isn't an entry"))?.check()?;
        }
        if text.contains('\0') {
            return Err("the file has a NUL".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEDORA: &str = "# Loopback entries; do not change.
# For historical reasons, localhost precedes localhost.localdomain:
127.0.0.1   localhost localhost.localdomain localhost4 localhost4.localdomain4
::1         localhost localhost.localdomain localhost6 localhost6.localdomain6
# See hosts(5) for proper format and other examples:
# 192.168.1.10 foo.example.org foo
192.168.1.40 nas nas.home   # the NAS
#off 10.0.0.9 old-printer
";

    #[test]
    fn entries_comments_and_ones_turned_off() {
        let f = HostsFile::parse(FEDORA);
        let hosts = f.hosts();
        assert_eq!(hosts.len(), 4);
        assert!(hosts[0].system() && hosts[1].system());
        assert_eq!(hosts[2], Host { on: true, address: "192.168.1.40".into(), names: vec!["nas".into(), "nas.home".into()], comment: "the NAS".into() });
        assert_eq!(hosts[3], Host { on: false, address: "10.0.0.9".into(), names: vec!["old-printer".into()], comment: String::new() });
        // The example in a comment stays a comment.
        assert!(!hosts.iter().any(|h| h.address == "192.168.1.10"));
        assert!(HostsFile::check(FEDORA).is_ok());
    }

    #[test]
    fn edits_keep_the_rest_of_the_file() {
        let f = HostsFile::parse(FEDORA);
        let mut hosts = f.hosts();
        hosts[2].on = false;
        hosts.remove(3);
        hosts.push(Host { on: true, address: "fd00::7".into(), names: vec!["lab".into()], comment: "IPv6 lab".into() });
        let text = f.with(&hosts);
        assert!(text.starts_with("# Loopback entries; do not change.\n# For historical reasons"), "{text}");
        assert!(text.contains("# 192.168.1.10 foo.example.org foo\n"), "{text}");
        assert!(text.contains("#off 192.168.1.40\tnas nas.home\t# the NAS\n"), "{text}");
        assert!(!text.contains("old-printer"), "{text}");
        assert!(text.ends_with("fd00::7\tlab\t# IPv6 lab\n"), "{text}");
        assert_eq!(HostsFile::parse(&text).hosts(), hosts);
        assert!(HostsFile::check(&text).is_ok());
        // Nothing changed: the same file back, its spacing kept.
        assert_eq!(f.with(&f.hosts()), FEDORA);
    }

    #[test]
    fn checked_before_root_writes_it() {
        assert!(HostsFile::check("10.0.0.5 nas\n").is_ok());
        assert!(HostsFile::check("10.0.0.5\n").is_err());
        assert!(HostsFile::check("nas 10.0.0.5\n").is_err());
        assert!(HostsFile::check("10.0.0.5 bad/name\n").is_err());
        assert!(HostsFile::check("#off nonsense here\n").is_err());
        assert!(Host { on: true, address: "10.0.0.5".into(), names: vec!["nas".into()], comment: "two\nlines".into() }.check().is_err());
    }
}
