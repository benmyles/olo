//! What Preferences ▸ Network keeps in NetworkManager's own configuration
//! (`/etc/NetworkManager/conf.d/90-olo.conf`, written as root by
//! `olo-net-admin`): the order of the services, as the route metrics of
//! each kind of link by default (so a network joined later, by anything,
//! falls in line), and DNS servers and search domains of the person's
//! own, for every link (NetworkManager's global DNS: they take the place
//! of what each network hands out). Anyone can read it; NetworkManager
//! reads it again when told (`Reload`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where it is, under `OLO_NET_ROOT` in tests (a directory standing for
/// `/`).
pub fn path() -> PathBuf {
    root().join("etc/NetworkManager/conf.d/90-olo.conf")
}

/// `/`, or `OLO_NET_ROOT` (tests: files written where they can be
/// looked at, nothing of this computer's touched).
pub fn root() -> PathBuf {
    match std::env::var_os("OLO_NET_ROOT") {
        Some(r) if !r.is_empty() => PathBuf::from(r),
        _ => PathBuf::from("/"),
    }
}

/// A kind of link, as the service order has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Service {
    Ethernet,
    Wifi,
}

impl Service {
    pub const ALL: [Service; 2] = [Service::Ethernet, Service::Wifi];

    pub fn name(self) -> &'static str {
        match self {
            Service::Ethernet => "Ethernet",
            Service::Wifi => "Wi-Fi",
        }
    }

    /// NetworkManager's device type, as `match-device` names it.
    fn device_type(self) -> &'static str {
        match self {
            Service::Ethernet => "ethernet",
            Service::Wifi => "wifi",
        }
    }

    /// NetworkManager's route metric for it when nothing says (its
    /// order unless told: Ethernet, then Wi-Fi).
    pub fn default_metric(self) -> u32 {
        match self {
            Service::Ethernet => 100,
            Service::Wifi => 600,
        }
    }

    pub fn kind(self) -> crate::network::Kind {
        match self {
            Service::Ethernet => crate::network::Kind::Ethernet,
            Service::Wifi => crate::network::Kind::Wifi,
        }
    }
}

/// The comment that keeps search domains given without servers.
const SEARCHES: &str = "olo-search-domains=";

/// DNS of the person's own for every link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dns {
    /// Servers asked instead of each network's ("1.1.1.1", "2606:4700::1111").
    pub servers: Vec<String>,
    /// Domains tried after a short name ("corp.example").
    pub searches: Vec<String>,
}

/// What the file says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetConf {
    /// First to last: traffic goes through the first connected.
    pub order: Vec<Service>,
    pub dns: Dns,
}

impl Default for NetConf {
    fn default() -> Self {
        NetConf { order: Service::ALL.to_vec(), dns: Dns::default() }
    }
}

/// A DNS server's address, checked: an IPv4 or IPv6 address.
pub fn valid_server(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// A search domain, checked: labels of letters, digits and hyphens.
pub fn valid_domain(s: &str) -> bool {
    let s = s.strip_suffix('.').unwrap_or(s);
    !s.is_empty() && s.len() <= 253 && s.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

impl NetConf {
    /// The file read (the default where there's none).
    pub fn read() -> NetConf {
        std::fs::read_to_string(path()).map(|t| NetConf::parse(&t)).unwrap_or_default()
    }

    pub fn is_default(&self) -> bool {
        *self == NetConf::default()
    }

    /// Each service's route metric: as NetworkManager's own when the
    /// order is its own, else 100, 200, … in order.
    pub fn metrics(&self) -> Vec<(Service, u32)> {
        if self.order == Service::ALL {
            return self.order.iter().map(|s| (*s, s.default_metric())).collect();
        }
        self.order.iter().enumerate().map(|(i, s)| (*s, 100 * (i as u32 + 1))).collect()
    }

    /// Checked before it's written as root: a known order, each service
    /// once, addresses and domains that are what they say.
    pub fn check(&self) -> Result<(), String> {
        let mut seen = Vec::new();
        for s in &self.order {
            if seen.contains(s) {
                return Err(format!("{} is in the order twice", s.name()));
            }
            seen.push(*s);
        }
        if let Some(bad) = self.dns.servers.iter().find(|s| !valid_server(s)) {
            return Err(format!("“{bad}” isn't an IP address"));
        }
        if let Some(bad) = self.dns.searches.iter().find(|s| !valid_domain(s)) {
            return Err(format!("“{bad}” isn't a domain"));
        }
        Ok(())
    }

    /// Search domains without servers of one's own: NetworkManager's
    /// global DNS can't have them (it would leave every link with no
    /// servers), so each connection gets them (`olo-net-admin`), and the
    /// file only remembers them.
    pub fn searches_alone(&self) -> bool {
        self.dns.servers.is_empty() && !self.dns.searches.is_empty()
    }

    /// The file's text (None: nothing of the person's own, no file).
    pub fn render(&self) -> Option<String> {
        let order_own = self.order != Service::ALL;
        let dns_own = !self.dns.servers.is_empty();
        if !order_own && !dns_own && !self.searches_alone() {
            return None;
        }
        let mut t = String::from("# Olo: the order of the network services, and DNS servers of your own\n# (Preferences ▸ Network). Olo writes this file: changes here are replaced.\n");
        if order_own {
            for (s, metric) in self.metrics() {
                t += &format!("\n[connection-olo-{}]\nmatch-device=type:{}\nipv4.route-metric={metric}\nipv6.route-metric={metric}\n", s.device_type(), s.device_type());
            }
        }
        if dns_own {
            if !self.dns.searches.is_empty() {
                t += &format!("\n[global-dns]\nsearches={}\n", self.dns.searches.join(","));
            }
            t += &format!("\n[global-dns-domain-*]\nservers={}\n", self.dns.servers.join(","));
        }
        if self.searches_alone() {
            t += &format!("\n# The search domains each connection has from Olo:\n# {SEARCHES}{}\n", self.dns.searches.join(","));
        }
        Some(t)
    }

    /// What `text` (the file) says: the order by the metrics in it,
    /// services it doesn't name where NetworkManager would put them.
    pub fn parse(text: &str) -> NetConf {
        let mut section = String::new();
        let mut kind: Option<Service> = None;
        let mut metrics: Vec<(Service, u32)> = Vec::new();
        let mut dns = Dns::default();
        let list = |v: &str| v.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>();
        for line in text.lines().map(str::trim) {
            if let Some(v) = line.strip_prefix('#').map(str::trim).and_then(|l| l.strip_prefix(SEARCHES)) {
                dns.searches = list(v);
                continue;
            }
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
                kind = None;
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match (section.as_str(), k) {
                (s, "match-device") if s.starts_with("connection") => kind = Service::ALL.into_iter().find(|x| v == format!("type:{}", x.device_type())),
                (s, "ipv4.route-metric") if s.starts_with("connection") => {
                    if let (Some(x), Ok(m)) = (kind, v.parse::<u32>()) {
                        metrics.retain(|(y, _)| *y != x);
                        metrics.push((x, m));
                    }
                }
                ("global-dns", "searches") => dns.searches = list(v),
                ("global-dns-domain-*", "servers") => dns.servers = list(v),
                _ => {}
            }
        }
        for s in Service::ALL {
            if !metrics.iter().any(|(x, _)| *x == s) {
                metrics.push((s, s.default_metric()));
            }
        }
        // (Ties as NetworkManager's own order.)
        metrics.sort_by_key(|(s, m)| (*m, Service::ALL.iter().position(|x| x == s)));
        NetConf { order: metrics.into_iter().map(|(s, _)| s).collect(), dns }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_of_ones_own_is_no_file() {
        assert_eq!(NetConf::default().render(), None);
        assert_eq!(NetConf::parse(""), NetConf::default());
        // Search domains alone don't make global DNS: each connection has
        // them, and the file remembers them.
        let c = NetConf { dns: Dns { servers: Vec::new(), searches: vec!["corp.example".into(), "lab.example".into()] }, ..NetConf::default() };
        let text = c.render().expect("a file");
        assert!(!text.contains("[global-dns"), "{text}");
        assert!(c.searches_alone());
        assert_eq!(NetConf::parse(&text), c);
    }

    #[test]
    fn wifi_first_and_dns_round_trip() {
        let c = NetConf { order: vec![Service::Wifi, Service::Ethernet], dns: Dns { servers: vec!["1.1.1.1".into(), "2606:4700::1111".into()], searches: vec!["corp.example".into()] } };
        assert!(c.check().is_ok());
        let text = c.render().expect("a file");
        assert!(text.contains("[connection-olo-wifi]\nmatch-device=type:wifi\nipv4.route-metric=100\nipv6.route-metric=100\n"), "{text}");
        assert!(text.contains("[connection-olo-ethernet]\nmatch-device=type:ethernet\nipv4.route-metric=200\n"), "{text}");
        assert!(text.contains("[global-dns]\nsearches=corp.example\n"), "{text}");
        assert!(text.contains("[global-dns-domain-*]\nservers=1.1.1.1,2606:4700::1111\n"), "{text}");
        assert_eq!(NetConf::parse(&text), c);
        // The order as it is with DNS alone: no metrics written.
        let dns_only = NetConf { order: Service::ALL.to_vec(), ..c };
        let text = dns_only.render().unwrap();
        assert!(!text.contains("route-metric"), "{text}");
        assert_eq!(NetConf::parse(&text), dns_only);
    }

    #[test]
    fn checked_before_root_writes_it() {
        let bad = |servers: &[&str], searches: &[&str]| {
            NetConf { order: Service::ALL.to_vec(), dns: Dns { servers: servers.iter().map(|s| s.to_string()).collect(), searches: searches.iter().map(|s| s.to_string()).collect() } }.check().is_err()
        };
        assert!(bad(&["1.1.1.1\n[connection]"], &[]));
        assert!(bad(&["dns.example"], &[]));
        assert!(bad(&["1.1.1.1"], &["bad domain"]));
        assert!(bad(&["1.1.1.1"], &["-x.example"]));
        assert!(!bad(&["9.9.9.9"], &["corp.example", "lab.corp.example."]));
        assert!(NetConf { order: vec![Service::Wifi, Service::Wifi], dns: Dns::default() }.check().is_err());
    }
}
