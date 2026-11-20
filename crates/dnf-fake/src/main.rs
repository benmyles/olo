//! `dnf-fake [--address ADDRESS]`: a stand-in for DNF5's daemon
//! (`dnf5daemon-server`, `org.rpm.dnf.v0`) for headless sessions and
//! tests, so nothing touches the system's packages. It takes the name on
//! the bus at ADDRESS (default: the session bus) and plays a Fedora 45
//! computer with updates waiting: security fixes with CVEs and
//! severities, bug fixes and enhancements, each with a changelog; a
//! kernel that installs beside the old ones (the oldest removed); a
//! third-party repository whose key is asked to be trusted; RPM Fusion
//! added by its release package's URL; a history whose upgrades can be
//! undone (the old versions are still in Fedora's repository); updates
//! put off until the computer next starts; and Fedora 46 to upgrade to.
//! Checking again (the cache expired) finds one more update. Downloads
//! and each package's steps are signalled as the real daemon does, and
//! a run can be cancelled while it downloads. `--repos DIR` adds the
//! repositories the .repo files in DIR say (and drops them when their
//! file goes), as /etc/yum.repos.d is read: OS Updates' Add and Remove
//! Repository write there in tests (`OLO_UPDATES_REPOS_DIR`). Point apps
//! at it with `OLO_DNF_BUS=ADDRESS`.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const ROOT: &str = "/org/rpm/dnf/v0";
const BASE: &str = "org.rpm.dnf.v0.Base";
const RPM: &str = "org.rpm.dnf.v0.rpm.Rpm";
const NOW: i64 = 1_794_700_000;
const DAY: i64 = 86_400;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.rpm.dnf.v0")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Error(String),
}

type Attrs = HashMap<String, OwnedValue>;

fn val<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("a value without fds")
}

#[derive(Debug, Clone)]
struct Pkg {
    name: String,
    epoch: String,
    version: String,
    release: String,
    arch: String,
    repo: String,
    summary: String,
    vendor: String,
    url: String,
    license: String,
    download: u64,
    size: u64,
    built: i64,
    installed_at: i64,
    reason: String,
    changelog: Vec<(i64, String, String)>,
    requires: Vec<String>,
}

impl Pkg {
    fn evr(&self) -> String {
        if self.epoch.is_empty() || self.epoch == "0" { format!("{}-{}", self.version, self.release) } else { format!("{}:{}-{}", self.epoch, self.version, self.release) }
    }
    fn nevra(&self) -> String {
        format!("{}-{}.{}", self.name, self.evr(), self.arch)
    }
    fn key(&self) -> String {
        format!("{}.{}", self.name, self.arch)
    }
    fn matches(&self, spec: &str) -> bool {
        let nvr = format!("{}-{}-{}", self.name, self.version, self.release);
        spec == self.name || spec == self.key() || spec == self.nevra() || spec == nvr || spec == format!("{}-{}", self.name, self.evr()) || spec == "*"
    }
    fn attrs(&self, wanted: &[String], installed: bool) -> Attrs {
        let mut a = Attrs::new();
        for w in wanted {
            let v = match w.as_str() {
                "name" => val(self.name.clone()),
                "epoch" => val(if self.epoch.is_empty() { "0".to_owned() } else { self.epoch.clone() }),
                "version" => val(self.version.clone()),
                "release" => val(self.release.clone()),
                "arch" => val(self.arch.clone()),
                "repo_id" => val(if installed { "@System".to_owned() } else { self.repo.clone() }),
                "from_repo_id" => val(if installed { self.repo.clone() } else { String::new() }),
                "is_installed" => val(installed),
                "install_size" => val(self.size),
                "download_size" | "package_size" => val(self.download),
                "buildtime" => val(self.built as u64),
                "install_time" => val(if installed { self.installed_at as u64 } else { 0 }),
                "sourcerpm" => val(format!("{}-{}-{}.src.rpm", self.name, self.version, self.release)),
                "summary" => val(self.summary.clone()),
                "description" => val(format!("{}.\n\nThe {} package, as Fedora builds it.", self.summary, self.name)),
                "url" => val(self.url.clone()),
                "license" => val(self.license.clone()),
                "vendor" => val(self.vendor.clone()),
                "evr" => val(self.evr()),
                "nevra" | "full_nevra" => val(self.nevra()),
                "reason" => val(self.reason.clone()),
                "group" => val("Unspecified".to_owned()),
                "id" => val(1i32),
                "changelogs" => val(self.changelog.clone()),
                "requires" => val(self.requires.clone()),
                "provides" => val(vec![format!("{} = {}", self.name, self.evr()), format!("{}({}) = {}", self.name, if self.arch == "noarch" { "noarch" } else { "x86-64" }, self.evr())]),
                "obsoletes" | "conflicts" | "recommends" | "suggests" | "enhances" | "supplements" | "requires_pre" | "files" => val(Vec::<String>::new()),
                _ => continue,
            };
            a.insert(w.clone(), v);
        }
        a
    }
}

/// A package, quickly: name, version-release, arch, repository, summary,
/// download and installed sizes (KB).
fn pkg(name: &str, vr: &str, arch: &str, repo: &str, summary: &str, kb: u64, inst_kb: u64) -> Pkg {
    let (ver, rel) = vr.rsplit_once('-').unwrap_or((vr, "1.fc45"));
    let (epoch, ver) = match ver.split_once(':') {
        Some((e, v)) => (e.to_owned(), v.to_owned()),
        None => (String::new(), ver.to_owned()),
    };
    let fedora = repo == "fedora" || repo == "updates" || repo == "updates-testing";
    Pkg {
        name: name.into(),
        epoch,
        version: ver,
        release: rel.into(),
        arch: arch.into(),
        repo: repo.into(),
        summary: summary.into(),
        vendor: if fedora {
            "Fedora Project".into()
        } else if repo.starts_with("rpmfusion") {
            "RPM Fusion".into()
        } else {
            String::new()
        },
        url: format!("https://example.org/{name}"),
        license: "MIT".into(),
        download: kb * 1024,
        size: inst_kb * 1024,
        built: NOW - 3 * DAY,
        installed_at: NOW - 40 * DAY,
        reason: "User".into(),
        changelog: Vec::new(),
        requires: vec!["glibc".into()],
    }
}

#[derive(Debug, Clone)]
struct Adv {
    id: String,
    kind: String,
    severity: String,
    title: String,
    description: String,
    issued: i64,
    /// (id, type, title, url)
    refs: Vec<(String, String, String, String)>,
    /// nevras it brings
    packages: Vec<String>,
}

#[derive(Debug, Clone)]
struct RepoM {
    id: String,
    name: String,
    enabled: bool,
    baseurl: String,
    metalink: String,
    gpgkey: Vec<String>,
    file: String,
    pkgs: u64,
    size: u64,
    priority: i64,
}

#[derive(Debug, Clone)]
struct Tx {
    id: i64,
    start: i64,
    description: String,
    comment: String,
    /// (action, name, arch, evr, original evr, repo, reason)
    changes: Vec<(String, String, String, String, String, String, String)>,
}

struct Model {
    installed: Vec<Pkg>,
    available: Vec<Pkg>,
    advisories: Vec<Adv>,
    repos: Vec<RepoM>,
    history: Vec<Tx>,
    offline: Option<HashMap<String, String>>,
    trusted: HashSet<String>,
    /// Found by checking again.
    later: Vec<Pkg>,
    checked_again: bool,
    /// Where .repo files are read from (`--repos`).
    repos_dir: Option<std::path::PathBuf>,
}

type Shared = Arc<Mutex<Model>>;

fn changelog(p: &mut Pkg, entries: &[(&str, &str)]) {
    let n = entries.len() as i64;
    p.changelog = entries.iter().enumerate().map(|(i, (who, text))| (NOW - (n - i as i64) * 7 * DAY, format!("{who} - {}-{}", p.version, p.release), text.to_string())).collect();
}

fn sample() -> Model {
    // What's installed, then the newer versions in the repositories.
    let pairs: Vec<(Pkg, Pkg)> = vec![
        (pkg("kernel", "6.19.4-200.fc45", "x86_64", "updates", "The Linux kernel", 180, 0), pkg("kernel", "6.19.8-200.fc45", "x86_64", "updates", "The Linux kernel", 182, 0)),
        (
            pkg("kernel-core", "6.19.4-200.fc45", "x86_64", "updates", "The Linux kernel (the core)", 19_000, 72_000),
            pkg("kernel-core", "6.19.8-200.fc45", "x86_64", "updates", "The Linux kernel (the core)", 19_100, 72_400),
        ),
        (
            pkg("firefox", "146.0-1.fc45", "x86_64", "updates", "Mozilla Firefox Web browser", 88_000, 270_000),
            pkg("firefox", "146.0.2-1.fc45", "x86_64", "updates", "Mozilla Firefox Web browser", 88_400, 271_000),
        ),
        (pkg("nss", "3.118-1.fc45", "x86_64", "updates", "Network Security Services", 900, 2_400), pkg("nss", "3.119-1.fc45", "x86_64", "updates", "Network Security Services", 910, 2_430)),
        (
            pkg("openssl-libs", "1:3.5.4-1.fc45", "x86_64", "updates", "A general purpose cryptography library with TLS implementation", 2_300, 7_800),
            pkg("openssl-libs", "1:3.5.5-1.fc45", "x86_64", "updates", "A general purpose cryptography library with TLS implementation", 2_310, 7_810),
        ),
        (
            pkg("sudo", "1.9.17-3.fc45", "x86_64", "fedora", "Allows restricted root access for specified users", 1_100, 4_300),
            pkg("sudo", "1.9.17p2-1.fc45", "x86_64", "updates", "Allows restricted root access for specified users", 1_120, 4_310),
        ),
        (
            pkg("curl", "8.17.0-1.fc45", "x86_64", "fedora", "A utility for getting files from remote servers", 320, 800),
            pkg("curl", "8.17.0-3.fc45", "x86_64", "updates", "A utility for getting files from remote servers", 322, 801),
        ),
        (pkg("git", "2.52.0-1.fc45", "x86_64", "updates", "Fast Version Control System", 60, 90), pkg("git", "2.52.1-1.fc45", "x86_64", "updates", "Fast Version Control System", 61, 91)),
        (
            pkg("vim-enhanced", "2:9.1.1800-1.fc45", "x86_64", "updates", "A version of the VIM editor which includes recent enhancements", 1_900, 4_200),
            pkg("vim-enhanced", "2:9.1.1912-1.fc45", "x86_64", "updates", "A version of the VIM editor which includes recent enhancements", 1_920, 4_230),
        ),
        (pkg("glibc", "2.42-9.fc45", "x86_64", "updates", "The GNU libc libraries", 2_200, 6_700), pkg("glibc", "2.42-11.fc45", "x86_64", "updates", "The GNU libc libraries", 2_210, 6_710)),
        (
            pkg("systemd", "258.2-1.fc45", "x86_64", "updates", "System and Service Manager", 5_100, 15_000),
            pkg("systemd", "258.3-1.fc45", "x86_64", "updates", "System and Service Manager", 5_120, 15_040),
        ),
        (
            pkg("NetworkManager", "1:1.54.2-1.fc45", "x86_64", "updates", "Network connection manager and user applications", 2_400, 6_000),
            pkg("NetworkManager", "1:1.54.3-1.fc45", "x86_64", "updates", "Network connection manager and user applications", 2_410, 6_010),
        ),
        (
            pkg("python3", "3.14.1-2.fc45", "x86_64", "updates", "Version 3.14 of the Python interpreter", 30, 40),
            pkg("python3", "3.14.2-1.fc45", "x86_64", "updates", "Version 3.14 of the Python interpreter", 30, 40),
        ),
        (
            pkg("mesa-dri-drivers", "26.2.6-1.fc45", "x86_64", "updates", "Mesa-based DRI drivers", 24_000, 98_000),
            pkg("mesa-dri-drivers", "26.3.0-1.fc45", "x86_64", "updates", "Mesa-based DRI drivers", 24_600, 99_500),
        ),
        (pkg("pipewire", "1.6.0-1.fc45", "x86_64", "updates", "Media Sharing Server", 300, 600), pkg("pipewire", "1.6.1-1.fc45", "x86_64", "updates", "Media Sharing Server", 302, 604)),
        (
            pkg("rust", "1.92.0-1.fc45", "x86_64", "updates", "The Rust Programming Language", 30_000, 98_000),
            pkg("rust", "1.93.0-1.fc45", "x86_64", "updates", "The Rust Programming Language", 30_500, 99_800),
        ),
        (pkg("tzdata", "2026a-1.fc45", "noarch", "fedora", "Timezone data", 430, 1_700), pkg("tzdata", "2026b-1.fc45", "noarch", "updates", "Timezone data", 431, 1_701)),
        (
            pkg("google-chrome-stable", "146.0.7680.71-1", "x86_64", "google-chrome", "Google Chrome", 118_000, 380_000),
            pkg("google-chrome-stable", "146.0.7680.153-1", "x86_64", "google-chrome", "Google Chrome", 118_400, 381_000),
        ),
        (
            pkg("akmod-nvidia", "3:620.60.03-1.fc45", "x86_64", "rpmfusion-nonfree-updates", "Akmod package for nvidia kernel module(s)", 45, 100),
            pkg("akmod-nvidia", "3:620.70.06-1.fc45", "x86_64", "rpmfusion-nonfree-updates", "Akmod package for nvidia kernel module(s)", 46, 101),
        ),
        (pkg("olo-tools", "0.9.0-1.fc45", "noarch", "copr:olo", "Olo's command-line tools", 120, 400), pkg("olo-tools", "0.9.1-1.fc45", "noarch", "copr:olo", "Olo's command-line tools", 121, 402)),
        // Moving between noarch and an architecture (Ben's, 2026-11-15:
        // asked for by the new build's name.arch, dnf refused it, and with
        // it all 517 updates).
        (
            pkg("python3-wcwidth", "0.2.13-5.fc45", "noarch", "fedora", "Measures the width of unicode strings in a terminal", 60, 200),
            pkg("python3-wcwidth", "0.2.14-1.fc45", "x86_64", "updates", "Measures the width of unicode strings in a terminal", 62, 210),
        ),
    ];
    let mut installed = Vec::new();
    let mut available = Vec::new();
    for (mut old, mut new) in pairs {
        if old.name == "nss" || old.name == "kernel-core" {
            old.reason = "Dependency".into();
            new.reason = "Dependency".into();
        }
        new.built = NOW - 2 * DAY;
        let update = format!("- Update to {}", new.version);
        let who = if new.repo.starts_with("google") { "Google Inc." } else { "Fedora Packager <packager@fedoraproject.org>" };
        changelog(&mut new, &[(who, "- Rebuilt for the new toolchain"), (who, "- Fix a crash on start-up\n- Update translations"), (who, &update)]);
        // The installed version stays available too (to reinstall or go
        // back to), as Fedora's own repository keeps the release's.
        let mut back = old.clone();
        back.reason = String::new();
        available.push(back);
        installed.push(old);
        available.push(new);
    }
    // The older kernels installed beside the newest (install-only).
    for (age, v) in [(60, "6.18.9-200.fc45"), (30, "6.19.1-200.fc45")] {
        for (n, s) in [("kernel", 0), ("kernel-core", 72_000)] {
            let mut k = pkg(n, v, "x86_64", "updates", "The Linux kernel", 0, s);
            k.installed_at = NOW - age * DAY;
            k.built = NOW - (age + 2) * DAY;
            installed.push(k);
        }
    }
    // A package history upgraded, whose old version is still about.
    let expat_old = pkg("expat", "2.8.3-1.fc45", "x86_64", "fedora", "An XML parser library", 120, 300);
    let mut expat = pkg("expat", "2.9.0-1.fc45", "x86_64", "updates", "An XML parser library", 121, 302);
    expat.installed_at = NOW - 5 * DAY;
    installed.push(expat.clone());
    available.push(expat_old);
    available.push(expat);
    // New packages updates pull in.
    available.push(pkg("firefox-langpacks", "146.0.2-1.fc45", "x86_64", "updates", "Firefox langpacks", 41_000, 60_000));
    available.push(pkg("mesa-vulkan-drivers", "26.3.0-1.fc45", "x86_64", "updates", "Mesa Vulkan drivers", 12_000, 41_000));
    let later = vec![{
        let mut p = pkg("bash", "5.3.6-1.fc45", "x86_64", "updates", "The GNU Bourne Again shell", 1_900, 8_600);
        changelog(&mut p, &[("Fedora Packager <packager@fedoraproject.org>", "- Update to 5.3.6")]);
        p.built = NOW - DAY;
        p
    }];
    installed.push(pkg("bash", "5.3.3-1.fc45", "x86_64", "fedora", "The GNU Bourne Again shell", 1_900, 8_500));

    let adv = |id: &str, kind: &str, sev: &str, title: &str, desc: &str, refs: Vec<(&str, &str)>, pkgs: &[&str], available: &[Pkg]| Adv {
        id: id.into(),
        kind: kind.into(),
        severity: sev.into(),
        title: title.into(),
        description: desc.into(),
        issued: NOW - 2 * DAY,
        refs: refs
            .into_iter()
            .map(|(kind, id)| {
                let url = if kind == "cve" { format!("https://www.cve.org/CVERecord?id={id}") } else { format!("https://bugzilla.redhat.com/show_bug.cgi?id={id}") };
                (id.to_owned(), kind.to_owned(), format!("{id}: details in the record"), url)
            })
            .collect(),
        packages: pkgs.iter().filter_map(|n| available.iter().filter(|p| p.name == *n).max_by_key(|p| p.built).map(|p| p.nevra())).collect(),
    };
    let advisories = vec![
        adv(
            "FEDORA-2026-1a2b3c4d5e",
            "security",
            "Important",
            "kernel-6.19.8-200.fc45",
            "The 6.19.8 stable kernel update contains a number of important fixes across the tree, among them a use-after-free in the network stack.",
            vec![("cve", "CVE-2026-40112"), ("cve", "CVE-2026-40177"), ("bugzilla", "2401187")],
            &["kernel", "kernel-core"],
            &available,
        ),
        adv(
            "FEDORA-2026-2b3c4d5e6f",
            "security",
            "Critical",
            "firefox-146.0.2-1.fc45 nss-3.119-1.fc45",
            "Firefox 146.0.2: fixes for memory safety bugs, one of them exploited in the wild.",
            vec![("cve", "CVE-2026-41020"), ("cve", "CVE-2026-41021")],
            &["firefox", "nss"],
            &available,
        ),
        adv(
            "FEDORA-2026-3c4d5e6f70",
            "security",
            "Moderate",
            "openssl-3.5.5-1.fc45",
            "Update to 3.5.5: a timing side channel in RSA decryption.",
            vec![("cve", "CVE-2026-38811")],
            &["openssl-libs"],
            &available,
        ),
        adv(
            "FEDORA-2026-4d5e6f7081",
            "security",
            "Important",
            "sudo-1.9.17p2-1.fc45",
            "A local privilege escalation through the chroot option.",
            vec![("cve", "CVE-2026-39001"), ("bugzilla", "2399120")],
            &["sudo"],
            &available,
        ),
        adv("FEDORA-2026-5e6f708192", "security", "Moderate", "curl-8.17.0-3.fc45", "Fixes a heap overflow in the SOCKS5 proxy handshake.", vec![("cve", "CVE-2026-37744")], &["curl"], &available),
        adv(
            "FEDORA-2026-6f708192a3",
            "security",
            "Low",
            "git-2.52.1-1.fc45 vim-9.1.1912-1.fc45",
            "Security fixes for crafted repositories and modelines.",
            vec![("cve", "CVE-2026-36002"), ("cve", "CVE-2026-36410")],
            &["git", "vim-enhanced"],
            &available,
        ),
        adv(
            "FEDORA-2026-708192a3b4",
            "bugfix",
            "None",
            "glibc-2.42-11.fc45",
            "Fixes a regression in getaddrinfo with mixed IPv4 and IPv6 resolvers.",
            vec![("bugzilla", "2403301")],
            &["glibc"],
            &available,
        ),
        adv("FEDORA-2026-8192a3b4c5", "bugfix", "None", "systemd-258.3-1.fc45", "systemd 258.3: stable fixes.", vec![("bugzilla", "2402200"), ("bugzilla", "2402211")], &["systemd"], &available),
        adv("FEDORA-2026-92a3b4c5d6", "bugfix", "None", "NetworkManager-1.54.3-1.fc45", "Wi-Fi reconnects after suspend again.", vec![], &["NetworkManager"], &available),
        adv("FEDORA-2026-a3b4c5d6e7", "bugfix", "None", "python3.14-3.14.2-1.fc45", "Python 3.14.2.", vec![], &["python3"], &available),
        adv("FEDORA-2026-b4c5d6e7f8", "enhancement", "None", "mesa-26.3.0-1.fc45", "Mesa 26.3: new Vulkan extensions and faster shader compiles.", vec![], &["mesa-dri-drivers"], &available),
        adv("FEDORA-2026-c5d6e7f809", "enhancement", "None", "pipewire-1.6.1-1.fc45", "PipeWire 1.6.1.", vec![], &["pipewire"], &available),
        adv("FEDORA-2026-d6e7f8091a", "enhancement", "None", "rust-1.93.0-1.fc45", "Rust 1.93.0.", vec![], &["rust"], &available),
        adv("FEDORA-2026-e7f8091a2b", "bugfix", "None", "tzdata-2026b-1.fc45", "Time zone changes for 2027.", vec![], &["tzdata"], &available),
    ];
    let repo = |id: &str, name: &str, enabled: bool, file: &str, pkgs: u64| RepoM {
        id: id.into(),
        name: name.into(),
        enabled,
        baseurl: if id.starts_with("fedora") || id.starts_with("updates") { String::new() } else { format!("https://example.org/{id}/$basearch") },
        metalink: if id.starts_with("fedora") || id.starts_with("updates") { format!("https://mirrors.fedoraproject.org/metalink?repo={id}-45&arch=x86_64") } else { String::new() },
        gpgkey: vec![if id.starts_with("fedora") || id.starts_with("updates") {
            "file:///usr/share/pki/rpm-gpg/RPM-GPG-KEY-fedora-45-x86_64".into()
        } else {
            format!("https://example.org/{id}/key.asc")
        }],
        file: file.into(),
        pkgs,
        size: pkgs * 2_400_000,
        priority: 99,
    };
    let repos = vec![
        repo("fedora", "Fedora 45 - x86_64", true, "/usr/share/dnf5/repos.d/fedora.repo", 75_855),
        repo("updates", "Fedora 45 - x86_64 - Updates", true, "/usr/share/dnf5/repos.d/fedora-updates.repo", 21_312),
        repo("updates-testing", "Fedora 45 - x86_64 - Test Updates", false, "/usr/share/dnf5/repos.d/fedora-updates-testing.repo", 0),
        repo("fedora-cisco-openh264", "Fedora 45 openh264 (From Cisco) - x86_64", true, "/usr/share/dnf5/repos.d/fedora-cisco-openh264.repo", 4),
        repo("google-chrome", "google-chrome", true, "/etc/yum.repos.d/google-chrome.repo", 3),
        repo("rpmfusion-nonfree", "RPM Fusion for Fedora 45 - Nonfree", true, "/etc/yum.repos.d/rpmfusion-nonfree.repo", 120),
        repo("rpmfusion-nonfree-updates", "RPM Fusion for Fedora 45 - Nonfree - Updates", true, "/etc/yum.repos.d/rpmfusion-nonfree-updates.repo", 40),
        repo("copr:olo", "Copr repo for olo owned by ben", true, "/etc/yum.repos.d/_copr:copr.fedorainfracloud.org:ben:olo.repo", 6),
    ];
    let mut trusted = HashSet::new();
    for r in &repos {
        if r.id != "copr:olo" {
            trusted.insert(r.id.clone());
        }
    }
    let ch = |a: &str, n: &str, evr: &str, orig: &str, repo: &str, reason: &str| (a.to_owned(), n.to_owned(), "x86_64".to_owned(), evr.to_owned(), orig.to_owned(), repo.to_owned(), reason.to_owned());
    let history = vec![
        Tx {
            id: 1,
            start: NOW - 60 * DAY,
            description: "anaconda".into(),
            comment: String::new(),
            changes: vec![ch("Install", "bash", "5.3.3-1.fc45", "", "fedora", "User"), ch("Install", "kernel", "6.18.9-200.fc45", "", "updates", "User")],
        },
        Tx {
            id: 2,
            start: NOW - 40 * DAY,
            description: "dnf5 install firefox git vim-enhanced".into(),
            comment: String::new(),
            changes: vec![
                ch("Install", "firefox", "146.0-1.fc45", "", "updates", "User"),
                ch("Install", "git", "2.52.0-1.fc45", "", "updates", "User"),
                ch("Install", "vim-enhanced", "2:9.1.1800-1.fc45", "", "updates", "User"),
            ],
        },
        Tx {
            id: 3,
            start: NOW - 20 * DAY,
            description: "dnf5 upgrade".into(),
            comment: String::new(),
            changes: vec![ch("Install", "kernel", "6.19.1-200.fc45", "", "updates", "User"), ch("Upgrade", "glibc", "2.42-9.fc45", "2.42-7.fc45", "updates", "Dependency")],
        },
        Tx {
            id: 4,
            start: NOW - 5 * DAY,
            description: "OS Updates".into(),
            comment: "Updated 1 package".into(),
            changes: vec![ch("Upgrade", "expat", "2.9.0-1.fc45", "2.8.3-1.fc45", "updates", "Dependency")],
        },
    ];
    Model { installed, available, advisories, repos, history, offline: None, trusted, later, checked_again: false, repos_dir: None }
}

impl Model {
    /// The .repo files in `repos_dir` read again: their repositories
    /// added, those whose file went dropped.
    fn read_dir(&mut self) {
        let Some(dir) = self.repos_dir.clone() else { return };
        let mut seen = HashSet::new();
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "repo") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let file = path.display().to_string();
            let mut cur: Option<RepoM> = None;
            let mut found = Vec::new();
            for line in text.lines().map(str::trim) {
                if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                    found.extend(cur.take());
                    cur = Some(RepoM {
                        id: id.trim().into(),
                        name: id.trim().into(),
                        enabled: true,
                        baseurl: String::new(),
                        metalink: String::new(),
                        gpgkey: Vec::new(),
                        file: file.clone(),
                        pkgs: 5,
                        size: 12_000_000,
                        priority: 99,
                    });
                } else if let (Some(r), Some((k, v))) = (cur.as_mut(), line.split_once('=')) {
                    let v = v.trim().to_owned();
                    match k.trim() {
                        "name" => r.name = v,
                        "baseurl" => r.baseurl = v,
                        "metalink" | "mirrorlist" => r.metalink = v,
                        "enabled" => r.enabled = v != "0",
                        "gpgkey" => r.gpgkey = vec![v],
                        _ => {}
                    }
                }
            }
            found.extend(cur);
            for r in found {
                seen.insert(r.id.clone());
                if !self.repos.iter().any(|x| x.id == r.id) {
                    self.trusted.insert(r.id.clone());
                    self.repos.push(r);
                }
            }
        }
        let dir = dir.display().to_string();
        self.repos.retain(|r| !r.file.starts_with(&dir) || seen.contains(&r.id));
    }

    fn enabled(&self, repo: &str) -> bool {
        repo == "@commandline" || self.repos.iter().any(|r| r.id == repo && r.enabled)
    }
    fn installed_of(&self, key: &str) -> Vec<&Pkg> {
        self.installed.iter().filter(|p| p.key() == key).collect()
    }
    /// The newest available version of `key` in an enabled repository.
    fn newest(&self, key: &str) -> Option<&Pkg> {
        self.available.iter().filter(|p| p.key() == key && self.enabled(&p.repo)).max_by(|a, b| a.built.cmp(&b.built).then(a.evr().cmp(&b.evr())))
    }
    /// The newest build of `p` in another architecture, one of the two
    /// noarch: dnf's update of it.
    fn moved(&self, p: &Pkg) -> Option<&Pkg> {
        self.available.iter().filter(|a| a.name == p.name && a.arch != p.arch && (a.arch == "noarch" || p.arch == "noarch") && self.enabled(&a.repo)).max_by_key(|a| a.built)
    }
    /// Installed packages with something newer.
    fn upgradable(&self) -> Vec<(&Pkg, &Pkg)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for p in &self.installed {
            if !seen.insert(p.key()) {
                continue;
            }
            let newest_installed = self.installed_of(&p.key()).into_iter().max_by_key(|q| q.built).cloned().unwrap_or_else(|| p.clone());
            if let Some(n) = self.newest(&p.key())
                && n.built > newest_installed.built
            {
                let old = self.installed.iter().find(|q| q.key() == p.key() && q.evr() == newest_installed.evr()).unwrap_or(p);
                out.push((old, n));
            } else if let Some(n) = self.moved(p)
                && n.built > newest_installed.built
            {
                // (As dnf: noarch to an architecture, or back, is an update.)
                out.push((p, n));
            }
        }
        out
    }
}

/// One session: what it was opened for and its goal.
#[derive(Default)]
struct SessionState {
    release: String,
    goal: Vec<(String, Vec<String>)>,
    resolved: Vec<Item>,
    cancel: Arc<AtomicBool>,
    /// A key's answer, by key id.
    answers: HashMap<String, bool>,
}

#[derive(Debug, Clone)]
struct Item {
    action: String,
    reason: String,
    pkg: Pkg,
}

type State = Arc<Mutex<SessionState>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn opt_str(o: &Attrs, k: &str) -> Option<String> {
    o.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}
fn opt_strs(o: &Attrs, k: &str) -> Vec<String> {
    o.get(k).and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}
fn opt_bool(o: &Attrs, k: &str) -> Option<bool> {
    o.get(k).and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
}
fn opt_i64(o: &Attrs, k: &str) -> Option<i64> {
    let v = o.get(k)?;
    i64::try_from(v.try_clone().ok()?).ok().or_else(|| i32::try_from(v.try_clone().ok()?).ok().map(i64::from))
}

struct Sessions {
    model: Shared,
    next: AtomicU32,
}

#[zbus::interface(name = "org.rpm.dnf.v0.SessionManager")]
impl Sessions {
    #[zbus(name = "open_session")]
    async fn open_session(&self, options: Attrs, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<OwnedObjectPath, Error> {
        let n = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let path = format!("{ROOT}/s{n}");
        let release = opt_str(&options, "releasever").unwrap_or_else(|| "45".into());
        let state: State = Arc::new(Mutex::new(SessionState { release, ..Default::default() }));
        let server = conn.object_server();
        let s = || Session { model: self.model.clone(), state: state.clone(), path: path.clone() };
        server.at(path.as_str(), BaseI(s())).await?;
        server.at(path.as_str(), RpmI(s())).await?;
        server.at(path.as_str(), RepoI(s())).await?;
        server.at(path.as_str(), GoalI(s())).await?;
        server.at(path.as_str(), AdvisoryI(s())).await?;
        server.at(path.as_str(), HistoryI(s())).await?;
        server.at(path.as_str(), OfflineI(s())).await?;
        Ok(OwnedObjectPath::try_from(path).map_err(zbus::Error::from)?)
    }

    #[zbus(name = "close_session")]
    async fn close_session(&self, path: OwnedObjectPath, #[zbus(connection)] conn: &zbus::Connection) -> bool {
        let server = conn.object_server();
        let p = path.as_str();
        let _ = server.remove::<BaseI, _>(p).await;
        let _ = server.remove::<RpmI, _>(p).await;
        let _ = server.remove::<RepoI, _>(p).await;
        let _ = server.remove::<GoalI, _>(p).await;
        let _ = server.remove::<AdvisoryI, _>(p).await;
        let _ = server.remove::<HistoryI, _>(p).await;
        server.remove::<OfflineI, _>(p).await.unwrap_or(false)
    }
}

#[derive(Clone)]
struct Session {
    model: Shared,
    state: State,
    path: String,
}

impl Session {
    async fn emit<B: serde::Serialize + zvariant::DynamicType>(&self, conn: &zbus::Connection, iface: &str, member: &str, body: &B) {
        let _ = conn.emit_signal(None::<()>, self.path.as_str(), iface, member, body).await;
    }
    fn me(&self) -> ObjectPath<'_> {
        ObjectPath::try_from(self.path.as_str()).expect("a session's path")
    }
    fn release_ok(&self) -> std::result::Result<(), Error> {
        let r = lock(&self.state).release.clone();
        if r == "45" || r == "46" {
            Ok(())
        } else {
            Err(Error::Error(format!(
                "Failed to download metadata for repo 'fedora': Cannot download repomd.xml: Status code: 404 for https://mirrors.fedoraproject.org/metalink?repo=fedora-{r}&arch=x86_64"
            )))
        }
    }
}

struct BaseI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.Base")]
impl BaseI {
    /// Fetches each enabled repository's metadata, signalled as it goes.
    #[zbus(name = "read_all_repos")]
    async fn read_all_repos(&self, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<bool, Error> {
        self.0.release_ok()?;
        let repos: Vec<(String, String)> = lock(&self.0.model).repos.iter().filter(|r| r.enabled).map(|r| (r.id.clone(), r.name.clone())).collect();
        for (id, name) in repos {
            let total = 2_000_000i64;
            self.0.emit(conn, BASE, "download_add_new", &(self.0.me(), id.as_str(), name.as_str(), total)).await;
            for i in 1..=4 {
                async_io::Timer::after(Duration::from_millis(25)).await;
                self.0.emit(conn, BASE, "download_progress", &(self.0.me(), id.as_str(), total, total * i / 4)).await;
            }
            self.0.emit(conn, BASE, "download_end", &(self.0.me(), id.as_str(), 0u32, "")).await;
        }
        // Checking again finds what came out since.
        let mut m = lock(&self.0.model);
        if m.checked_again && !m.later.is_empty() {
            let later = std::mem::take(&mut m.later);
            m.available.extend(later);
        }
        Ok(true)
    }

    #[zbus(name = "clean_with_options")]
    fn clean_with_options(&self, cache_type: String, _options: Attrs) -> (bool, String) {
        if cache_type == "expire-cache" || cache_type == "metadata" || cache_type == "all" {
            lock(&self.0.model).checked_again = true;
        }
        (true, String::new())
    }

    #[zbus(name = "clean")]
    fn clean(&self, cache_type: String) -> (bool, String) {
        self.clean_with_options(cache_type, Attrs::new())
    }

    #[zbus(name = "reset")]
    fn reset(&self) -> (bool, String) {
        (true, String::new())
    }
}

struct RpmI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.rpm.Rpm")]
impl RpmI {
    #[zbus(name = "list")]
    fn list(&self, options: Attrs) -> std::result::Result<Vec<Attrs>, Error> {
        self.0.release_ok()?;
        let attrs = opt_strs(&options, "package_attrs");
        let scope = opt_str(&options, "scope").unwrap_or_else(|| "all".into());
        let patterns = opt_strs(&options, "patterns");
        let repos = opt_strs(&options, "repo");
        let m = lock(&self.0.model);
        let wanted = |p: &Pkg| (patterns.is_empty() || patterns.iter().any(|s| p.matches(s))) && (repos.is_empty() || repos.contains(&p.repo));
        let mut out: Vec<(Pkg, bool)> = Vec::new();
        match scope.as_str() {
            "installed" => out.extend(m.installed.iter().filter(|p| wanted(p)).map(|p| (p.clone(), true))),
            "upgradable" => out.extend(m.upgradable().into_iter().filter(|(o, _)| wanted(o)).map(|(o, _)| (o.clone(), true))),
            "upgrades" => {
                for (_, n) in m.upgradable() {
                    if wanted(n) {
                        out.push((n.clone(), false));
                    }
                }
            }
            "available" => out.extend(m.available.iter().filter(|p| m.enabled(&p.repo) && wanted(p)).map(|p| (p.clone(), false))),
            _ => {
                out.extend(m.installed.iter().filter(|p| wanted(p)).map(|p| (p.clone(), true)));
                out.extend(m.available.iter().filter(|p| m.enabled(&p.repo) && wanted(p)).map(|p| (p.clone(), false)));
            }
        }
        Ok(out.iter().map(|(p, i)| p.attrs(&attrs, *i)).collect())
    }

    #[zbus(name = "install")]
    fn install(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("install".into(), pkg_specs));
    }
    #[zbus(name = "upgrade")]
    fn upgrade(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("upgrade".into(), pkg_specs));
    }
    #[zbus(name = "remove")]
    fn remove(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("remove".into(), pkg_specs));
    }
    #[zbus(name = "downgrade")]
    fn downgrade(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("downgrade".into(), pkg_specs));
    }
    #[zbus(name = "reinstall")]
    fn reinstall(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("reinstall".into(), pkg_specs));
    }
    #[zbus(name = "distro_sync")]
    fn distro_sync(&self, pkg_specs: Vec<String>, _options: Attrs) {
        lock(&self.0.state).goal.push(("distro_sync".into(), pkg_specs));
    }
    #[zbus(name = "system_upgrade")]
    fn system_upgrade(&self, _options: Attrs) {
        lock(&self.0.state).goal.push(("system_upgrade".into(), Vec::new()));
    }
}

struct RepoI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.rpm.Repo")]
impl RepoI {
    #[zbus(name = "list")]
    fn list(&self, options: Attrs) -> std::result::Result<Vec<Attrs>, Error> {
        let attrs = opt_strs(&options, "repo_attrs");
        let which = opt_str(&options, "enable_disable").unwrap_or_else(|| "enabled".into());
        let patterns = opt_strs(&options, "patterns");
        let mut m = lock(&self.0.model);
        m.read_dir();
        let mut out = Vec::new();
        for r in &m.repos {
            if (which == "enabled" && !r.enabled) || (which == "disabled" && r.enabled) || (!patterns.is_empty() && !patterns.contains(&r.id)) {
                continue;
            }
            let mut a = Attrs::new();
            for w in &attrs {
                let v = match w.as_str() {
                    "id" => val(r.id.clone()),
                    "name" => val(r.name.clone()),
                    "type" => val("available".to_owned()),
                    "enabled" => val(r.enabled),
                    "priority" => val(r.priority as i32),
                    "cost" => val(1000i32),
                    "baseurl" => val(if r.baseurl.is_empty() { Vec::<String>::new() } else { vec![r.baseurl.clone()] }),
                    "metalink" => val(r.metalink.clone()),
                    "mirrorlist" => val(String::new()),
                    "metadata_expire" => val(172_800i32),
                    "excludepkgs" | "includepkgs" | "content_tags" | "distro_tags" => val(Vec::<String>::new()),
                    "skip_if_unavailable" => val(r.id != "fedora" && r.id != "updates"),
                    "gpgkey" => val(r.gpgkey.clone()),
                    "gpgcheck" => val(true),
                    "repo_gpgcheck" => val(false),
                    "proxy" => val(String::new()),
                    "repofile" => val(r.file.clone()),
                    "revision" => val("1794600000".to_owned()),
                    "updated" => val(NOW - DAY),
                    "cache_updated" => val(NOW - 3600),
                    "size" => val(r.size),
                    "pkgs" | "available_pkgs" => val(if r.enabled { r.pkgs } else { 0 }),
                    "mirrors" => val(if r.metalink.is_empty() { Vec::<String>::new() } else { vec![format!("https://mirror.example.org/fedora/{}/", r.id)] }),
                    other => return Err(Error::Error(format!("Repo attribute '{other}' not supported"))),
                };
                a.insert(w.clone(), v);
            }
            out.push(a);
        }
        Ok(out)
    }

    #[zbus(name = "enable_with_options")]
    fn enable_with_options(&self, repo_ids: Vec<String>, _options: Attrs) {
        for r in lock(&self.0.model).repos.iter_mut().filter(|r| repo_ids.contains(&r.id)) {
            r.enabled = true;
        }
    }
    #[zbus(name = "disable_with_options")]
    fn disable_with_options(&self, repo_ids: Vec<String>, _options: Attrs) {
        for r in lock(&self.0.model).repos.iter_mut().filter(|r| repo_ids.contains(&r.id)) {
            r.enabled = false;
        }
    }
    #[zbus(name = "enable")]
    fn enable(&self, repo_ids: Vec<String>) {
        self.enable_with_options(repo_ids, Attrs::new())
    }
    #[zbus(name = "disable")]
    fn disable(&self, repo_ids: Vec<String>) {
        self.disable_with_options(repo_ids, Attrs::new())
    }
    #[zbus(name = "confirm_key_with_options")]
    fn confirm_key_with_options(&self, key_id: String, confirmed: bool, _options: Attrs) {
        lock(&self.0.state).answers.insert(key_id, confirmed);
    }
    #[zbus(name = "confirm_key")]
    fn confirm_key(&self, key_id: String, confirmed: bool) {
        self.confirm_key_with_options(key_id, confirmed, Attrs::new())
    }
}

struct GoalI(Session);

/// What resolving a goal comes to.
fn resolve(m: &Model, release: &str, goal: &[(String, Vec<String>)]) -> (Vec<Item>, Vec<String>) {
    let mut items: Vec<Item> = Vec::new();
    let mut problems = Vec::new();
    let add = |items: &mut Vec<Item>, action: &str, reason: &str, p: &Pkg| {
        if !items.iter().any(|i| i.pkg.nevra() == p.nevra() && i.action == action) {
            items.push(Item { action: action.into(), reason: reason.into(), pkg: p.clone() });
        }
    };
    for (op, specs) in goal {
        if op == "system_upgrade" {
            if release != "46" {
                problems.push(format!("No release {release} to upgrade to."));
                continue;
            }
            for p in &m.installed {
                let mut n = p.clone();
                n.release = n.release.replace("fc45", "fc46");
                n.repo = "fedora".into();
                n.built = NOW + DAY;
                add(&mut items, "Upgrade", &p.reason, &n);
                add(&mut items, "Replaced", &p.reason, p);
            }
            continue;
        }
        for spec in specs {
            if spec.starts_with("https://") && spec.ends_with(".rpm") {
                // "…/rpmfusion-free-release-45.noarch.rpm": its name
                // before the release number.
                let file = spec.rsplit('/').next().unwrap_or_default();
                let name = file.split("-45").next().unwrap_or(file);
                let p = pkg(name, "45-1", "noarch", "@commandline", "A repository's release package", 12, 30);
                add(&mut items, "Install", "User", &p);
                continue;
            }
            // An update moving architecture asked for by its new build's
            // name.arch: refused, as dnf5 does.
            if op == "upgrade" && m.upgradable().iter().any(|(o, n)| o.arch != n.arch && *spec == n.key()) {
                problems.push(format!("Packages for argument '{spec}' available, but installed for a different architecture."));
                continue;
            }
            let found: Vec<&Pkg> = match op.as_str() {
                "remove" => m.installed.iter().filter(|p| p.matches(spec)).collect(),
                "downgrade" | "install" | "reinstall" => m.available.iter().filter(|p| m.enabled(&p.repo) && p.matches(spec)).collect(),
                _ => m.upgradable().into_iter().filter(|(o, n)| o.matches(spec) || n.matches(spec) || spec == "*").map(|(_, n)| n).collect(),
            };
            if found.is_empty() {
                if op == "upgrade" && m.installed.iter().any(|p| p.matches(spec)) {
                    continue; // Nothing newer: nothing to do.
                }
                problems.push(format!("No match for argument: {spec}"));
                continue;
            }
            for p in found {
                let mut installed: Vec<&Pkg> = m.installed_of(&p.key());
                // (Its build in the other architecture, for one moving.)
                if installed.is_empty() && op == "upgrade" {
                    installed = m.installed.iter().filter(|q| q.name == p.name).collect();
                }
                match op.as_str() {
                    "remove" => add(&mut items, "Remove", "User", p),
                    "reinstall" => add(&mut items, "Reinstall", "User", p),
                    "downgrade" => {
                        add(&mut items, "Downgrade", "User", p);
                        if let Some(o) = installed.first() {
                            add(&mut items, "Replaced", &o.reason, o);
                        }
                    }
                    _ if p.name == "kernel" || p.name == "kernel-core" => {
                        // Install-only: beside the others, the oldest of
                        // three going.
                        add(&mut items, "Install", "Dependency", p);
                        if installed.len() >= 3
                            && let Some(oldest) = installed.iter().min_by_key(|q| q.installed_at)
                        {
                            add(&mut items, "Remove", "Clean", oldest);
                        }
                    }
                    _ if installed.is_empty() => add(&mut items, "Install", "User", p),
                    _ => {
                        let reason = installed.first().map(|o| o.reason.clone()).unwrap_or_else(|| "User".into());
                        add(&mut items, "Upgrade", &reason, p);
                        if let Some(o) = installed.iter().max_by_key(|q| q.built) {
                            add(&mut items, "Replaced", &reason, o);
                        }
                        // What it pulls in.
                        let pulls: &[&str] = match p.name.as_str() {
                            "firefox" => &["nss", "firefox-langpacks"],
                            "mesa-dri-drivers" => &["mesa-vulkan-drivers"],
                            "kernel" => &["kernel-core"],
                            _ => &[],
                        };
                        for dep in pulls {
                            let key = format!("{dep}.{}", p.arch);
                            if let Some(d) = m.newest(&key) {
                                let have = m.installed_of(&key);
                                if have.is_empty() {
                                    add(&mut items, "Install", "Dependency", d);
                                } else if have.iter().all(|h| h.built < d.built) {
                                    add(&mut items, "Upgrade", "Dependency", d);
                                    if let Some(h) = have.first() {
                                        add(&mut items, "Replaced", "Dependency", h);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // An argument it can't take leaves nothing to do, as dnf5's (strict:
    // skip_unavailable off) does.
    if !problems.is_empty() {
        items.clear();
    }
    (items, problems)
}

#[zbus::interface(name = "org.rpm.dnf.v0.Goal")]
impl GoalI {
    #[allow(clippy::type_complexity)]
    #[zbus(name = "resolve")]
    fn resolve(&self, _options: Attrs) -> std::result::Result<(Vec<(String, String, String, Attrs, Attrs)>, u32), Error> {
        self.0.release_ok()?;
        let (release, goal) = {
            let s = lock(&self.0.state);
            (s.release.clone(), s.goal.clone())
        };
        let (items, problems) = resolve(&lock(&self.0.model), &release, &goal);
        let attrs: Vec<String> = ["name", "epoch", "version", "release", "arch", "repo_id", "from_repo_id", "package_size", "install_size", "evr", "reason"].iter().map(|s| s.to_string()).collect();
        let out = items.iter().map(|i| ("Package".to_owned(), i.action.clone(), i.reason.clone(), Attrs::new(), i.pkg.attrs(&attrs, i.action == "Replaced" || i.action == "Remove"))).collect();
        let result = if !problems.is_empty() && items.is_empty() {
            2
        } else if !problems.is_empty() {
            1
        } else {
            0
        };
        let mut s = lock(&self.0.state);
        s.resolved = items;
        drop(s);
        PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()).insert(self.0.path.clone(), problems);
        Ok((out, result))
    }

    #[zbus(name = "get_transaction_problems_string")]
    fn get_transaction_problems_string(&self) -> Vec<String> {
        PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()).get(&self.0.path).cloned().unwrap_or_default()
    }

    #[zbus(name = "get_transaction_problems")]
    fn get_transaction_problems(&self) -> Vec<Attrs> {
        Vec::new()
    }

    #[zbus(name = "do_transaction")]
    async fn do_transaction(&self, options: Attrs, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let offline = opt_bool(&options, "offline").unwrap_or(false);
        let comment = opt_str(&options, "comment").unwrap_or_default();
        let (items, cancel, release) = {
            let s = lock(&self.0.state);
            s.cancel.store(false, Ordering::Relaxed);
            (s.resolved.clone(), s.cancel.clone(), s.release.clone())
        };
        if items.is_empty() {
            return Err(Error::Error("The transaction is empty: resolve it first.".into()));
        }
        let me = self.0.me();
        // A repository's key not yet trusted is asked about first.
        let untrusted: Vec<String> = {
            let m = lock(&self.0.model);
            let mut v: Vec<String> =
                items.iter().filter(|i| i.action != "Remove" && i.action != "Replaced" && !m.trusted.contains(&i.pkg.repo) && i.pkg.repo != "@commandline").map(|i| i.pkg.repo.clone()).collect();
            v.dedup();
            v
        };
        for repo in untrusted {
            let key_id = "5F7E2A8B9C1D3E4F".to_owned();
            self.0
                .emit(
                    conn,
                    BASE,
                    "repo_key_import_request",
                    &(
                        me.clone(),
                        key_id.as_str(),
                        vec![format!("{repo} (Copr) <copr-devel@lists.fedorahosted.org>")],
                        "9C1D 3E4F 5F7E 2A8B 0A1B 2C3D 4E5F 6071 8293 A4B5",
                        "https://copr-be.cloud.fedoraproject.org/results/ben/olo/pubkey.gpg",
                        NOW - 400 * DAY,
                    ),
                )
                .await;
            let mut answer = None;
            for _ in 0..600 {
                async_io::Timer::after(Duration::from_millis(50)).await;
                if let Some(a) = lock(&self.0.state).answers.remove(&key_id) {
                    answer = Some(a);
                    break;
                }
            }
            if answer != Some(true) {
                return Err(Error::Error(format!("Public key for packages from \"{repo}\" is not trusted.")));
            }
            lock(&self.0.model).trusted.insert(repo.clone());
            self.0
                .emit(
                    conn,
                    BASE,
                    "repo_key_imported",
                    &(me.clone(), key_id.as_str(), vec![format!("{repo} (Copr)")], "9C1D 3E4F", "https://copr-be.cloud.fedoraproject.org/results/ben/olo/pubkey.gpg", NOW - 400 * DAY),
                )
                .await;
        }
        // Downloads, each in steps; cancellable.
        for i in items.iter().filter(|i| matches!(i.action.as_str(), "Install" | "Upgrade" | "Downgrade" | "Reinstall")) {
            let id = i.pkg.nevra();
            let total = i.pkg.download.max(1) as i64;
            self.0.emit(conn, BASE, "download_add_new", &(me.clone(), id.as_str(), id.as_str(), total)).await;
            for step in 1..=6 {
                async_io::Timer::after(Duration::from_millis(30)).await;
                if cancel.load(Ordering::Relaxed) {
                    self.0.emit(conn, BASE, "download_end", &(me.clone(), id.as_str(), 2u32, "Interrupted by user")).await;
                    return Err(Error::Error("The transaction was cancelled.".into()));
                }
                self.0.emit(conn, BASE, "download_progress", &(me.clone(), id.as_str(), total, total * step / 6)).await;
            }
            self.0.emit(conn, BASE, "download_end", &(me.clone(), id.as_str(), 0u32, "")).await;
        }
        if offline {
            let mut status = HashMap::new();
            status.insert("status".to_owned(), "ready".to_owned());
            status.insert("target_releasever".to_owned(), release.clone());
            status.insert("cmd_line".to_owned(), if comment.is_empty() { "OS Updates".to_owned() } else { comment.clone() });
            status.insert("poweroff".to_owned(), "false".to_owned());
            lock(&self.0.model).offline = Some(status);
            return Ok(());
        }
        let n = items.len() as u64;
        self.0.emit(conn, RPM, "transaction_before_begin", &(me.clone(), n)).await;
        self.0.emit(conn, RPM, "transaction_verify_start", &(me.clone(), n)).await;
        for k in 1..=n {
            self.0.emit(conn, RPM, "transaction_verify_progress", &(me.clone(), k, n)).await;
        }
        self.0.emit(conn, RPM, "transaction_verify_stop", &(me.clone(), n)).await;
        self.0.emit(conn, RPM, "transaction_transaction_start", &(me.clone(), n)).await;
        self.0.emit(conn, RPM, "transaction_transaction_stop", &(me.clone(), n)).await;
        for (k, i) in items.iter().enumerate() {
            let nevra = i.pkg.nevra();
            let action: u32 = match i.action.as_str() {
                "Install" => 0,
                "Remove" | "Replaced" => 1,
                "Upgrade" => 2,
                "Downgrade" => 3,
                "Reinstall" => 4,
                _ => 5,
            };
            self.0.emit(conn, RPM, "transaction_action_start", &(me.clone(), nevra.as_str(), action, i.pkg.size)).await;
            for step in 1..=3u64 {
                async_io::Timer::after(Duration::from_millis(15)).await;
                self.0.emit(conn, RPM, "transaction_action_progress", &(me.clone(), nevra.as_str(), i.pkg.size * step / 3, i.pkg.size)).await;
            }
            self.0.emit(conn, RPM, "transaction_action_stop", &(me.clone(), nevra.as_str(), i.pkg.size)).await;
            self.0.emit(conn, RPM, "transaction_elem_progress", &(me.clone(), nevra.as_str(), k as u64 + 1, n)).await;
        }
        // What it did, done.
        {
            let mut m = lock(&self.0.model);
            let mut changes = Vec::new();
            for i in &items {
                let p = &i.pkg;
                match i.action.as_str() {
                    "Install" | "Upgrade" | "Downgrade" | "Reinstall" => {
                        let orig = items.iter().find(|r| r.action == "Replaced" && r.pkg.key() == p.key()).map(|r| r.pkg.evr()).unwrap_or_default();
                        let mut q = p.clone();
                        q.installed_at = NOW;
                        if q.reason.is_empty() {
                            q.reason = i.reason.clone();
                        }
                        if i.action == "Reinstall" {
                            m.installed.retain(|x| x.nevra() != p.nevra());
                        }
                        m.installed.push(q);
                        changes.push((i.action.clone(), p.name.clone(), p.arch.clone(), p.evr(), orig, p.repo.clone(), i.reason.clone()));
                        if p.repo == "@commandline" && p.name.starts_with("rpmfusion-") {
                            let free = p.name.contains("-free-");
                            let base = if free { "rpmfusion-free" } else { "rpmfusion-nonfree" };
                            for (id, name) in [
                                (base.to_owned(), format!("RPM Fusion for Fedora 45 - {}", if free { "Free" } else { "Nonfree" })),
                                (format!("{base}-updates"), format!("RPM Fusion for Fedora 45 - {} - Updates", if free { "Free" } else { "Nonfree" })),
                            ] {
                                if !m.repos.iter().any(|r| r.id == id) {
                                    m.repos.push(RepoM {
                                        id: id.clone(),
                                        name,
                                        enabled: true,
                                        baseurl: String::new(),
                                        metalink: format!("https://mirrors.rpmfusion.org/metalink?repo={id}-45&arch=x86_64"),
                                        gpgkey: vec![format!("file:///usr/share/pki/rpm-gpg/RPM-GPG-KEY-{base}-fedora-2020")],
                                        file: format!("/etc/yum.repos.d/{id}.repo"),
                                        pkgs: 400,
                                        size: 1 << 31,
                                        priority: 99,
                                    });
                                    m.trusted.insert(id);
                                }
                            }
                        }
                    }
                    "Remove" | "Replaced" => {
                        let nevra = p.nevra();
                        m.installed.retain(|x| x.nevra() != nevra);
                        if i.action == "Remove" {
                            changes.push(("Removed".into(), p.name.clone(), p.arch.clone(), p.evr(), String::new(), p.repo.clone(), i.reason.clone()));
                        }
                    }
                    _ => {}
                }
            }
            let id = m.history.last().map(|t| t.id).unwrap_or(0) + 1;
            m.history.push(Tx { id, start: NOW + 60, description: "OS Updates".into(), comment, changes });
        }
        lock(&self.0.state).resolved.clear();
        self.0.emit(conn, RPM, "transaction_after_complete", &(me.clone(), true)).await;
        Ok(())
    }

    #[zbus(name = "cancel")]
    fn cancel(&self) -> (bool, String) {
        lock(&self.0.state).cancel.store(true, Ordering::Relaxed);
        (true, String::new())
    }

    #[zbus(name = "reset")]
    fn reset(&self) {
        let mut s = lock(&self.0.state);
        s.goal.clear();
        s.resolved.clear();
    }
}

static PROBLEMS: std::sync::LazyLock<Mutex<HashMap<String, Vec<String>>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

struct AdvisoryI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.Advisory")]
impl AdvisoryI {
    #[zbus(name = "list")]
    fn list(&self, options: Attrs) -> std::result::Result<Vec<Attrs>, Error> {
        let attrs = opt_strs(&options, "advisory_attrs");
        let m = lock(&self.0.model);
        let upgradable: HashSet<String> = m.upgradable().iter().map(|(_, n)| n.nevra()).collect();
        let mut out = Vec::new();
        for a in &m.advisories {
            if !a.packages.iter().any(|p| upgradable.contains(p)) {
                continue;
            }
            let mut v = Attrs::new();
            for w in &attrs {
                let x = match w.as_str() {
                    "advisoryid" => val(1i32),
                    "name" => val(a.id.clone()),
                    "title" => val(a.title.clone()),
                    "type" => val(a.kind.clone()),
                    "severity" => val(a.severity.clone()),
                    "status" => val("stable".to_owned()),
                    "vendor" => val("updates@fedoraproject.org".to_owned()),
                    "description" => val(a.description.clone()),
                    "buildtime" => val(a.issued as u64),
                    "message" => val(String::new()),
                    "rights" => val("Copyright (C) 2026 Red Hat, Inc. and others.".to_owned()),
                    "references" => val(a.refs.clone()),
                    "collections" => {
                        let packages: Vec<HashMap<String, OwnedValue>> = a
                            .packages
                            .iter()
                            .filter_map(|nevra| m.available.iter().find(|p| p.nevra() == *nevra))
                            .map(|p| {
                                let mut h = HashMap::new();
                                h.insert("n".to_owned(), val(p.name.clone()));
                                h.insert("e".to_owned(), val(if p.epoch.is_empty() { "0".to_owned() } else { p.epoch.clone() }));
                                h.insert("v".to_owned(), val(p.version.clone()));
                                h.insert("r".to_owned(), val(p.release.clone()));
                                h.insert("a".to_owned(), val(p.arch.clone()));
                                h.insert("nevra".to_owned(), val(p.nevra()));
                                h.insert("applicability".to_owned(), val("available".to_owned()));
                                h
                            })
                            .collect();
                        let mut c: HashMap<String, OwnedValue> = HashMap::new();
                        c.insert("packages".to_owned(), val(packages));
                        c.insert("modules".to_owned(), val(Vec::<String>::new()));
                        val(vec![c])
                    }
                    _ => continue,
                };
                v.insert(w.clone(), x);
            }
            out.push(v);
        }
        Ok(out)
    }
}

struct HistoryI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.History")]
impl HistoryI {
    #[zbus(name = "list")]
    fn list(&self, options: Attrs) -> Vec<Attrs> {
        let limit = opt_i64(&options, "limit").unwrap_or(i64::MAX).max(0) as usize;
        let m = lock(&self.0.model);
        m.history
            .iter()
            .rev()
            .take(limit)
            .map(|t| {
                let mut a = Attrs::new();
                a.insert("id".into(), val(t.id));
                a.insert("start".into(), val(t.start));
                a.insert("end".into(), val(t.start + 40));
                a.insert("user_id".into(), val(1000i64));
                a.insert("description".into(), val(t.description.clone()));
                a.insert("comment".into(), val(t.comment.clone()));
                a.insert("status".into(), val("Ok".to_owned()));
                a.insert("releasever".into(), val("45".to_owned()));
                for (key, actions) in
                    [("installed", &["Install"][..]), ("removed", &["Removed"][..]), ("upgraded", &["Upgrade"][..]), ("downgraded", &["Downgrade"][..]), ("reinstalled", &["Reinstall"][..])]
                {
                    let list: Vec<HashMap<String, OwnedValue>> = t
                        .changes
                        .iter()
                        .filter(|c| actions.contains(&c.0.as_str()))
                        .map(|(action, name, arch, evr, orig, repo, reason)| {
                            let mut h = HashMap::new();
                            h.insert("name".to_owned(), val(name.clone()));
                            h.insert("arch".to_owned(), val(arch.clone()));
                            h.insert("evr".to_owned(), val(evr.clone()));
                            h.insert("repo_id".to_owned(), val(repo.clone()));
                            h.insert("action".to_owned(), val(if action == "Removed" { "Remove".to_owned() } else { action.clone() }));
                            h.insert("reason".to_owned(), val(reason.clone()));
                            if !orig.is_empty() {
                                h.insert("original_evr".to_owned(), val(orig.clone()));
                            }
                            h
                        })
                        .collect();
                    a.insert(key.into(), val(list));
                }
                a
            })
            .collect()
    }

    #[zbus(name = "recent_changes")]
    fn recent_changes(&self, _options: Attrs) -> HashMap<String, Vec<Attrs>> {
        HashMap::new()
    }
}

struct OfflineI(Session);

#[zbus::interface(name = "org.rpm.dnf.v0.Offline")]
impl OfflineI {
    #[zbus(name = "get_status")]
    fn get_status(&self) -> (bool, Attrs) {
        match &lock(&self.0.model).offline {
            Some(s) => (true, s.iter().map(|(k, v)| (k.clone(), val(v.clone()))).collect()),
            None => (false, Attrs::new()),
        }
    }
    #[zbus(name = "cancel_with_options")]
    fn cancel_with_options(&self, _options: Attrs) -> (bool, String) {
        let was = lock(&self.0.model).offline.take().is_some();
        (was, if was { String::new() } else { "No offline transaction is configured.".into() })
    }
    #[zbus(name = "cancel")]
    fn cancel(&self) -> (bool, String) {
        self.cancel_with_options(Attrs::new())
    }
    #[zbus(name = "set_finish_action_with_options")]
    fn set_finish_action_with_options(&self, action: String, _options: Attrs) -> (bool, String) {
        match lock(&self.0.model).offline.as_mut() {
            Some(s) => {
                s.insert("poweroff".into(), (action == "poweroff").to_string());
                (true, String::new())
            }
            None => (false, "No offline transaction is configured.".into()),
        }
    }
    #[zbus(name = "set_finish_action")]
    fn set_finish_action(&self, action: String) -> (bool, String) {
        self.set_finish_action_with_options(action, Attrs::new())
    }
    #[zbus(name = "schedule_for_next_boot")]
    fn schedule_for_next_boot(&self, _options: Attrs) -> (bool, String) {
        (lock(&self.0.model).offline.is_some(), String::new())
    }
    #[zbus(name = "clean")]
    fn clean(&self, _options: Attrs) -> (bool, String) {
        lock(&self.0.model).offline = None;
        (true, String::new())
    }
}

fn main() {
    olo_crash::install();
    let mut address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let mut repos_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().expect("--address ADDRESS"),
            "--repos" => repos_dir = Some(std::path::PathBuf::from(args.next().expect("--repos DIR"))),
            _ => {
                eprintln!("usage: dnf-fake [--address ADDRESS] [--repos DIR]");
                std::process::exit(2);
            }
        }
    }
    let mut sample = sample();
    sample.repos_dir = repos_dir;
    sample.read_dir();
    let model: Shared = Arc::new(Mutex::new(sample));
    let conn = match zbus::blocking::connection::Builder::address(address.as_str()).and_then(|b| b.name("org.rpm.dnf.v0")).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dnf-fake: can't take org.rpm.dnf.v0 on {address}: {e}");
            std::process::exit(1);
        }
    };
    conn.object_server().at(ROOT, Sessions { model, next: AtomicU32::new(0) }).expect("the session manager");
    loop {
        std::thread::park();
    }
}
