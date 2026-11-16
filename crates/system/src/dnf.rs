//! The system's packages through DNF5's own daemon (`dnf5daemon-server`,
//! `org.rpm.dnf.v0` on the system bus): the updates there are, with each
//! one's versions, changelog, advisories (security, bug fix, enhancement;
//! CVEs and bugs) and sizes; the repositories, turned on and off; the
//! history of transactions; a transaction resolved (what it would do,
//! everything it pulls in) before it runs, then run with its downloads'
//! and each package's progress, cancellable while it downloads, now or
//! when the computer next starts; a repository's key confirmed when it
//! asks; and the upgrade to the next Fedora release. Nothing is shelled
//! out to. Everything blocks, so call it from a worker.
//!
//! The daemon keeps a session per client connection, so a [`Session`]
//! has a connection of its own: what it reads it reads once, what it
//! does (a goal: packages marked, then resolved, then run) belongs to it,
//! and it closes with it. The daemon asks polkit before it changes
//! anything; Fedora lets an administrator at the computer run a signed
//! transaction without being asked, so the app shows what a transaction
//! does, and runs it only when told.
//!
//! `OLO_DNF_BUS` (a D-Bus address) talks to that bus instead of the
//! system bus: tests run `dnf-fake` there and never touch the system's
//! packages. [`may_change`] says whether this program may change the
//! system's: only in the person's own session at the computer, never
//! from a test session or an agent's screen.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        mpsc::{Sender, channel},
    },
};

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::Result;

pub const DEST: &str = "org.rpm.dnf.v0";
pub const ROOT: &str = "/org/rpm/dnf/v0";
const SESSIONS: &str = "org.rpm.dnf.v0.SessionManager";
const BASE: &str = "org.rpm.dnf.v0.Base";
const RPM: &str = "org.rpm.dnf.v0.rpm.Rpm";
const REPO: &str = "org.rpm.dnf.v0.rpm.Repo";
const GOAL: &str = "org.rpm.dnf.v0.Goal";
const ADVISORY: &str = "org.rpm.dnf.v0.Advisory";
const HISTORY: &str = "org.rpm.dnf.v0.History";
const OFFLINE: &str = "org.rpm.dnf.v0.Offline";

/// The bus named by `OLO_DNF_BUS`, if any.
fn test_bus() -> Option<String> {
    std::env::var("OLO_DNF_BUS").ok().filter(|a| !a.is_empty())
}

/// Whether this talks to the system's own daemon (not a test's).
pub fn real() -> bool {
    test_bus().is_none()
}

/// Whether this program may change packages: a test's daemon always; the
/// system's only from the person's own session at the computer
/// ([`crate::env::at_the_computer`]), as polkit doesn't ask an
/// administrator there before a signed transaction runs.
pub fn may_change() -> bool {
    !real() || crate::env::at_the_computer()
}

/// Why [`may_change`] says no, in words.
pub fn why_not() -> &'static str {
    "Packages are changed only from the session at the computer, not a test session or an agent's screen."
}

/// A new connection to the daemon's bus: a session is the connection's.
fn connect() -> Result<Connection> {
    match test_bus() {
        Some(address) => zbus::blocking::connection::Builder::address(address.as_str())?.build(),
        None => Connection::system(),
    }
}

/// An error in words: the daemon's message without D-Bus's name for it.
pub fn error_text(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, Some(text), _) if !text.is_empty() => {
            let n = name.as_str();
            if n.ends_with("NotAuthorized") || n.contains("PolicyKit") || n.ends_with("AccessDenied") || text.contains("Not authorized") {
                "Not allowed (authentication was refused or cancelled).".into()
            } else {
                text.trim().to_owned()
            }
        }
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        zbus::Error::InputOutput(e) => format!("Couldn't reach the package service: {e}"),
        other => other.to_string(),
    }
}

/// D-Bus values as JSON, so attribute maps read into plain structs (the
/// daemon's attributes are of many types, and some missing).
pub fn json(v: &Value<'_>) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::U8(n) => J::from(*n),
        Value::Bool(b) => J::Bool(*b),
        Value::I16(n) => J::from(*n),
        Value::U16(n) => J::from(*n),
        Value::I32(n) => J::from(*n),
        Value::U32(n) => J::from(*n),
        Value::I64(n) => J::from(*n),
        Value::U64(n) => J::from(*n),
        Value::F64(n) => J::from(*n),
        Value::Str(s) => J::String(s.as_str().to_owned()),
        Value::Signature(s) => J::String(s.to_string()),
        Value::ObjectPath(p) => J::String(p.as_str().to_owned()),
        Value::Value(inner) => json(inner),
        Value::Array(a) => J::Array(a.inner().iter().map(json).collect()),
        Value::Dict(d) => J::Object(
            d.iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Str(s) => s.as_str().to_owned(),
                        other => json(other).to_string(),
                    };
                    (key, json(v))
                })
                .collect(),
        ),
        Value::Structure(s) => J::Array(s.fields().iter().map(json).collect()),
        _ => J::Null,
    }
}

type Attrs = HashMap<String, OwnedValue>;

/// An attribute map read into `T` (missing ones at their defaults).
fn read<T: serde::de::DeserializeOwned + Default>(attrs: &Attrs) -> T {
    let map: serde_json::Map<String, serde_json::Value> = attrs.iter().map(|(k, v)| (k.clone(), json(v))).collect();
    serde_json::from_value(serde_json::Value::Object(map)).unwrap_or_default()
}

/// Options, as the daemon takes them (`a{sv}`).
#[derive(Default)]
pub struct Opts(HashMap<String, OwnedValue>);

impl Opts {
    pub fn new() -> Opts {
        Opts::default()
    }
    fn put<'a>(mut self, key: &str, v: impl Into<Value<'a>>) -> Opts {
        if let Ok(v) = OwnedValue::try_from(v.into()) {
            self.0.insert(key.to_owned(), v);
        }
        self
    }
    pub fn bool(self, key: &str, v: bool) -> Opts {
        self.put(key, v)
    }
    pub fn str(self, key: &str, v: &str) -> Opts {
        self.put(key, v.to_owned())
    }
    pub fn i32(self, key: &str, v: i32) -> Opts {
        self.put(key, v)
    }
    pub fn i64(self, key: &str, v: i64) -> Opts {
        self.put(key, v)
    }
    pub fn strs(self, key: &str, v: &[&str]) -> Opts {
        self.put(key, v.iter().map(|s| s.to_string()).collect::<Vec<String>>())
    }
    pub fn map(self, key: &str, v: &[(&str, &str)]) -> Opts {
        self.put(key, v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<String, String>>())
    }
}

/// A package: one version of it, installed or in a repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Package {
    pub name: String,
    pub epoch: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    /// The repository it's in ("@System" when installed).
    pub repo_id: String,
    /// Installed: the repository it came from.
    pub from_repo_id: String,
    pub is_installed: bool,
    pub install_size: u64,
    pub download_size: u64,
    pub buildtime: i64,
    pub install_time: i64,
    pub sourcerpm: String,
    pub summary: String,
    pub description: String,
    pub url: String,
    pub license: String,
    pub vendor: String,
    /// "1:2.3-4.fc45": epoch (when not 0), version, release.
    pub evr: String,
    pub nevra: String,
    pub full_nevra: String,
    /// Why it's installed: "User", "Dependency", "Weak Dependency", …
    pub reason: String,
}

impl Package {
    /// "name.arch": what an update and the package it replaces share.
    pub fn key(&self) -> String {
        format!("{}.{}", self.name, self.arch)
    }
}

/// What a package's details add: its changelog and dependencies.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Details {
    pub description: String,
    pub url: String,
    pub license: String,
    /// Newest first.
    pub changelog: Vec<Change>,
    pub requires: Vec<String>,
    pub provides: Vec<String>,
    pub obsoletes: Vec<String>,
    pub conflicts: Vec<String>,
    pub recommends: Vec<String>,
}

/// One changelog entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Change {
    /// Unix time (days, as RPM keeps them).
    pub when: i64,
    /// "Name <mail> - version-release".
    pub author: String,
    pub text: String,
}

/// An advisory: Fedora's (or a repository's) word on an update.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Advisory {
    /// "FEDORA-2026-401703193b".
    pub id: String,
    pub title: String,
    /// "security", "bugfix", "enhancement", "newpackage".
    pub kind: String,
    /// "Critical", "Important", "Moderate", "Low", "None".
    pub severity: String,
    pub status: String,
    pub description: String,
    pub issued: i64,
    pub references: Vec<Reference>,
    /// "name.arch" of each package it updates on this computer.
    pub packages: Vec<String>,
}

/// A bug or CVE an advisory names.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reference {
    /// "cve", "bugzilla", "self", …
    pub kind: String,
    pub id: String,
    pub title: String,
    pub url: String,
}

/// An update: the installed package, the newest there is of it, and what
/// advisories say of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Update {
    pub installed: Package,
    pub new: Package,
    /// Ids of the advisories for it (most severe first), in [`Updates::advisories`].
    pub advisories: Vec<String>,
}

impl Update {
    /// How to ask dnf to upgrade it (Ben, 2026-11-15: one refused
    /// argument left "Update 517 packages" with nothing to do): as
    /// `dnf upgrade` takes what's installed, its `name.arch` when the new
    /// build's is the same, else its name alone. A package moving between
    /// `noarch` and an architecture (python3-wcwidth) is refused by the new
    /// build's: "Packages for argument 'python3-wcwidth.x86_64' available,
    /// but installed for a different architecture."
    pub fn upgrade_spec(&self) -> String {
        if self.installed.arch == self.new.arch { self.new.key() } else { self.new.name.clone() }
    }

    /// The most pressing kind among its advisories: "security", then
    /// "bugfix", "enhancement", "newpackage"; "" with none.
    pub fn kind<'a>(&self, advisories: &'a HashMap<String, Advisory>) -> &'a str {
        let rank = |k: &str| match k {
            "security" => 0,
            "bugfix" => 1,
            "enhancement" => 2,
            _ => 3,
        };
        self.advisories.iter().filter_map(|id| advisories.get(id)).map(|a| a.kind.as_str()).min_by_key(|k| rank(k)).unwrap_or("")
    }
}

/// The updates there are, and the advisories they come with.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Updates {
    pub updates: Vec<Update>,
    pub advisories: HashMap<String, Advisory>,
}

/// A repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i64,
    pub cost: i64,
    pub baseurl: Vec<String>,
    pub metalink: String,
    pub mirrorlist: String,
    pub metadata_expire: i64,
    pub excludepkgs: Vec<String>,
    pub includepkgs: Vec<String>,
    pub skip_if_unavailable: bool,
    pub gpgkey: Vec<String>,
    pub gpgcheck: bool,
    pub repo_gpgcheck: bool,
    pub proxy: String,
    /// The file that says it ("/etc/yum.repos.d/….repo").
    pub repofile: String,
    pub revision: String,
    pub content_tags: Vec<String>,
    pub distro_tags: Vec<String>,
    /// When its metadata was made, and fetched (Unix time).
    pub updated: i64,
    pub cache_updated: i64,
    /// Its packages' size together, and how many (of them, not excluded).
    pub size: u64,
    pub pkgs: u64,
    pub available_pkgs: u64,
    /// "available", "system", "commandline".
    #[serde(rename = "type")]
    pub kind: String,
}

impl Repo {
    /// Where it's fetched from, as said: a base URL, a metalink or a
    /// mirror list.
    pub fn source(&self) -> String {
        self.baseurl.first().cloned().filter(|s| !s.is_empty()).or_else(|| Some(self.metalink.clone()).filter(|s| !s.is_empty())).unwrap_or_else(|| self.mirrorlist.clone())
    }
}

/// What a transaction item does.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Item {
    /// "Package", "Group", "Environment", "Module".
    pub object: String,
    /// "Install", "Upgrade", "Downgrade", "Reinstall", "Remove", "Replaced",
    /// "Reason Change", …
    pub action: String,
    /// "User", "Dependency", "Weak Dependency", "Clean", …
    pub reason: String,
    pub package: Package,
    /// For a group: its id and name.
    pub group: Option<(String, String)>,
    /// What it replaces (package ids), when it does.
    pub replaces: Vec<i64>,
}

/// A resolved transaction: what it would do, and what's wrong.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Resolved {
    pub items: Vec<Item>,
    /// 0 fine, 1 fine with warnings, 2 failed.
    pub result: u32,
    pub problems: Vec<String>,
}

impl Resolved {
    pub fn ok(&self) -> bool {
        self.result < 2
    }
    /// Bytes to download, and the change in what's installed.
    pub fn sizes(&self) -> (u64, i64) {
        let mut download = 0u64;
        let mut installed = 0i64;
        for i in &self.items {
            let p = &i.package;
            match i.action.as_str() {
                "Install" | "Upgrade" | "Downgrade" | "Reinstall" => {
                    download += p.download_size;
                    installed += p.install_size as i64;
                }
                "Remove" | "Replaced" => installed -= p.install_size as i64,
                _ => {}
            }
        }
        (download, installed)
    }
}

/// A transaction in the history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Transaction {
    pub id: i64,
    pub start: i64,
    pub end: i64,
    pub user_id: i64,
    /// The command it was, when one said ("dnf5 install foo").
    pub description: String,
    pub comment: String,
    /// "Ok", "Error", "Started", …
    pub status: String,
    pub releasever: String,
    pub installed: Vec<Change2>,
    pub removed: Vec<Change2>,
    pub upgraded: Vec<Change2>,
    pub downgraded: Vec<Change2>,
    pub reinstalled: Vec<Change2>,
}

impl Transaction {
    /// How many packages it touched.
    pub fn count(&self) -> usize {
        self.installed.len() + self.removed.len() + self.upgraded.len() + self.downgraded.len() + self.reinstalled.len()
    }
}

/// A package a past transaction changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Change2 {
    pub name: String,
    pub arch: String,
    pub evr: String,
    /// Before an upgrade or downgrade.
    pub original_evr: String,
    pub repo_id: String,
    pub action: String,
    pub reason: String,
}

/// A transaction waiting for the computer to start again (offline).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Offline {
    pub pending: bool,
    /// What the daemon says of it ("download-complete", "ready", …),
    /// its target release, …
    pub status: HashMap<String, String>,
}

/// A repository's key, asked to be trusted while a transaction runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct KeyRequest {
    pub key_id: String,
    pub user_ids: Vec<String>,
    pub fingerprint: String,
    pub url: String,
    pub created: i64,
}

/// What a package is going through in a transaction
/// (`dnfdaemon::RpmTransactionItem::Actions`).
pub fn action_words(action: u32) -> &'static str {
    match action {
        0 => "Installing",
        1 => "Removing",
        2 => "Upgrading",
        3 => "Downgrading",
        4 => "Reinstalling",
        5 => "Cleaning up",
        _ => "Working on",
    }
}

/// What a session hears while it reads repositories or runs a
/// transaction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Event {
    /// A download starts: its id, what it is, its size (bytes, or 0).
    DownloadStart {
        id: String,
        what: String,
        total: i64,
    },
    Download {
        id: String,
        done: i64,
        total: i64,
    },
    /// It ended: fine (or there already), or why not.
    DownloadEnd {
        id: String,
        ok: bool,
        message: String,
    },
    MirrorFailure {
        id: String,
        message: String,
        url: String,
    },
    /// A repository's key to trust: answer with [`Session::confirm_key`].
    Key(KeyRequest),
    KeyImported {
        key_id: String,
    },
    /// RPM's own steps: verifying, preparing, then each package.
    Begin {
        total: u64,
    },
    Verify {
        done: u64,
        total: u64,
    },
    Prepare {
        done: u64,
        total: u64,
    },
    Action {
        nevra: String,
        action: u32,
        total: u64,
    },
    ActionProgress {
        nevra: String,
        done: u64,
        total: u64,
    },
    ActionDone {
        nevra: String,
    },
    /// How far through the packages (one of how many).
    Element {
        nevra: String,
        done: u64,
        total: u64,
    },
    ScriptError {
        nevra: String,
        kind: u32,
        code: u64,
    },
    UnpackError {
        nevra: String,
    },
    Finished {
        ok: bool,
    },
}

/// One session with the daemon: its own connection, what it read, and
/// its goal.
pub struct Session {
    conn: Connection,
    path: OwnedObjectPath,
    /// Where its signals go while someone listens.
    sink: Sink,
}

/// What a listener hears: an event, or that the call it waits on ended.
enum Heard {
    Event(Event),
    Done,
}

type Sink = Arc<Mutex<Option<Sender<Heard>>>>;

/// How a session opens.
#[derive(Debug, Clone)]
pub struct Open {
    /// Read the repositories' packages too (not just what's installed).
    pub available: bool,
    /// Only what's cached: no downloading.
    pub cache_only: bool,
    /// Another release's repositories (the next Fedora's, to upgrade).
    pub release: Option<String>,
    /// Changelogs too (Fedora's "other" metadata: larger).
    pub changelogs: bool,
}

impl Default for Open {
    fn default() -> Open {
        Open { available: true, cache_only: false, release: None, changelogs: true }
    }
}

const PACKAGE_ATTRS: &[&str] = &[
    "name",
    "epoch",
    "version",
    "release",
    "arch",
    "repo_id",
    "from_repo_id",
    "is_installed",
    "install_size",
    "download_size",
    "buildtime",
    "install_time",
    "sourcerpm",
    "summary",
    "url",
    "license",
    "vendor",
    "evr",
    "nevra",
    "full_nevra",
    "reason",
];

const REPO_ATTRS: &[&str] = &[
    "id",
    "name",
    "type",
    "enabled",
    "priority",
    "cost",
    "baseurl",
    "metalink",
    "mirrorlist",
    "metadata_expire",
    "excludepkgs",
    "includepkgs",
    "skip_if_unavailable",
    "gpgkey",
    "gpgcheck",
    "repo_gpgcheck",
    "proxy",
    "repofile",
    "revision",
    "content_tags",
    "distro_tags",
    "updated",
    "size",
    "pkgs",
    "available_pkgs",
    "cache_updated",
];

impl Session {
    /// Open a session (the daemon starts if it isn't running).
    pub fn open(how: &Open) -> Result<Session> {
        let conn = connect()?;
        let mut config: Vec<(&str, &str)> = Vec::new();
        if how.cache_only {
            config.push(("cacheonly", "all"));
        }
        if how.changelogs {
            config.push(("optional_metadata_types", "comps,updateinfo,other"));
        }
        let mut opts = Opts::new().bool("load_available_repos", how.available).bool("load_system_repo", true).map("config", &config);
        if let Some(r) = &how.release {
            opts = opts.str("releasever", r);
        }
        let path: OwnedObjectPath = conn.call_method(Some(DEST), ROOT, Some(SESSIONS), "open_session", &(opts.0,))?.body().deserialize()?;
        let sink: Sink = Arc::new(Mutex::new(None));
        listen(&conn, &path, sink.clone())?;
        Ok(Session { conn, path, sink })
    }

    fn call<B, R>(&self, iface: &str, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zvariant::Type,
    {
        self.conn.call_method(Some(DEST), self.path.as_str(), Some(iface), method, body)?.body().deserialize()
    }

    fn call0<B: serde::Serialize + zvariant::DynamicType>(&self, iface: &str, method: &str, body: &B) -> Result<()> {
        self.conn.call_method(Some(DEST), self.path.as_str(), Some(iface), method, body)?;
        Ok(())
    }

    /// Calls `(success, error)` methods, the error as an error.
    fn call_ok<B: serde::Serialize + zvariant::DynamicType>(&self, iface: &str, method: &str, body: &B) -> Result<()> {
        let (ok, why): (bool, String) = self.call(iface, method, body)?;
        if ok { Ok(()) } else { Err(zbus::Error::Failure(if why.is_empty() { format!("{method} failed") } else { why })) }
    }

    /// Have `events` hear what happens while `f` runs (on this thread).
    pub fn hearing<T>(&self, events: &mut dyn FnMut(Event), f: impl FnOnce(&Session) -> T + Send) -> T
    where
        T: Send,
    {
        let (tx, rx) = channel::<Heard>();
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        let out = std::thread::scope(|s| {
            let (done_tx, done_rx) = channel::<T>();
            s.spawn(move || {
                let r = f(self);
                let _ = done_tx.send(r);
                let _ = tx.send(Heard::Done);
            });
            loop {
                match rx.recv() {
                    Ok(Heard::Event(e)) => events(e),
                    // What came before the end was heard first (one queue).
                    Ok(Heard::Done) | Err(_) => break done_rx.recv().expect("the call ends"),
                }
            }
        });
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
        out
    }

    /// Load the repositories' metadata now (downloading what's out of
    /// date): whether all of them loaded.
    pub fn read_repos(&self) -> Result<bool> {
        self.call(BASE, "read_all_repos", &())
    }

    /// Mark the cached metadata out of date, so the next read fetches it.
    pub fn expire_cache(&self) -> Result<()> {
        self.call_ok(BASE, "clean_with_options", &("expire-cache", Opts::new().bool("interactive", true).0))
    }

    /// Packages that match `opts` (`package_attrs` added).
    pub fn packages(&self, opts: Opts, attrs: &[&str]) -> Result<Vec<Attrs>> {
        let opts = opts.strs("package_attrs", attrs);
        self.call(RPM, "list", &(opts.0,))
    }

    /// The updates there are, with their advisories.
    pub fn updates(&self) -> Result<Updates> {
        let new: Vec<Package> = self.packages(Opts::new().str("scope", "upgrades").i32("latest-limit", 1), PACKAGE_ATTRS)?.iter().map(read).collect();
        let old: Vec<Package> = self.packages(Opts::new().str("scope", "upgradable"), PACKAGE_ATTRS)?.iter().map(read).collect();
        let mut installed: HashMap<String, Package> = HashMap::new();
        for p in old {
            // Of several installed (kernels), the newest is what's updated.
            let k = p.key();
            if installed.get(&k).is_none_or(|q| q.buildtime < p.buildtime) {
                installed.insert(k, p);
            }
        }
        let advisories = self.advisories("updates")?;
        let mut by_package: HashMap<String, Vec<String>> = HashMap::new();
        for a in advisories.values() {
            for k in &a.packages {
                by_package.entry(k.clone()).or_default().push(a.id.clone());
            }
        }
        let severity = |s: &str| match s.to_ascii_lowercase().as_str() {
            "critical" => 0,
            "important" => 1,
            "moderate" => 2,
            "low" => 3,
            _ => 4,
        };
        let mut updates: Vec<Update> = new
            .into_iter()
            .map(|n| {
                let k = n.key();
                let mut ids = by_package.remove(&k).unwrap_or_default();
                ids.sort_by_key(|id| (advisories.get(id).map(|a| (a.kind != "security", severity(&a.severity))).unwrap_or((true, 5)), id.clone()));
                ids.dedup();
                // What it updates: the installed build of its name.arch, else
                // of its name in another architecture (one moving between
                // noarch and an architecture, as python3-wcwidth did).
                let was = installed.get(&k).or_else(|| installed.values().filter(|q| q.name == n.name).max_by_key(|q| q.buildtime)).cloned().unwrap_or_default();
                Update { installed: was, new: n, advisories: ids }
            })
            .collect();
        updates.sort_by(|a, b| a.new.name.cmp(&b.new.name).then(a.new.arch.cmp(&b.new.arch)));
        Ok(Updates { updates, advisories })
    }

    /// Advisories (`availability`: "updates", "available", "installed",
    /// "all"), by id, each with the packages here it's for.
    pub fn advisories(&self, availability: &str) -> Result<HashMap<String, Advisory>> {
        let attrs = ["name", "title", "type", "severity", "status", "description", "buildtime", "references", "collections"];
        let opts = Opts::new().strs("advisory_attrs", &attrs).str("availability", availability);
        let list: Vec<Attrs> = self.call(ADVISORY, "list", &(opts.0,))?;
        let mut out = HashMap::new();
        for a in &list {
            let j: serde_json::Map<String, serde_json::Value> = a.iter().map(|(k, v)| (k.clone(), json(v))).collect();
            let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            let mut packages = Vec::new();
            for c in j.get("collections").and_then(|v| v.as_array()).into_iter().flatten() {
                for p in c.get("packages").and_then(|v| v.as_array()).into_iter().flatten() {
                    let applies = p.get("applicability").and_then(|v| v.as_str()).unwrap_or("available");
                    if applies == "unrelated" {
                        continue;
                    }
                    let (n, arch) = (p.get("n").and_then(|v| v.as_str()).unwrap_or_default(), p.get("a").and_then(|v| v.as_str()).unwrap_or_default());
                    let k = format!("{n}.{arch}");
                    if !n.is_empty() && !packages.contains(&k) {
                        packages.push(k);
                    }
                }
            }
            let references = j
                .get("references")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let f = |i: usize, k: &str| r.get(i).or_else(|| r.get(k)).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    let reference = Reference { id: f(0, "id"), kind: f(1, "type"), title: f(2, "title"), url: f(3, "url") };
                    (!reference.id.is_empty() || !reference.url.is_empty()).then_some(reference)
                })
                .collect();
            let advisory = Advisory {
                id: s("name"),
                title: s("title"),
                kind: s("type"),
                severity: s("severity"),
                status: s("status"),
                description: s("description"),
                issued: j.get("buildtime").and_then(|v| v.as_i64()).unwrap_or_default(),
                references,
                packages,
            };
            if !advisory.id.is_empty() {
                out.insert(advisory.id.clone(), advisory);
            }
        }
        Ok(out)
    }

    /// A package's details: its changelog (newest first) and
    /// dependencies. `nevra` names it exactly; `scope` as in `list`.
    pub fn details(&self, nevra: &str, scope: &str) -> Result<Details> {
        let attrs = ["description", "url", "license", "changelogs", "requires", "provides", "obsoletes", "conflicts", "recommends"];
        let opts = Opts::new().strs("patterns", &[nevra]).str("scope", scope).bool("with_provides", false).bool("with_filenames", false).bool("with_binaries", false);
        let list = self.packages(opts, &attrs)?;
        let Some(a) = list.first() else { return Ok(Details::default()) };
        let j: serde_json::Map<String, serde_json::Value> = a.iter().map(|(k, v)| (k.clone(), json(v))).collect();
        let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let strs = |k: &str| j.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let mut changelog: Vec<Change> = j
            .get("changelogs")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let c = c.as_array()?;
                Some(Change { when: c.first()?.as_i64().unwrap_or_default(), author: c.get(1)?.as_str()?.to_owned(), text: c.get(2)?.as_str()?.to_owned() })
            })
            .collect();
        changelog.sort_by_key(|c| std::cmp::Reverse(c.when));
        Ok(Details {
            description: s("description"),
            url: s("url"),
            license: s("license"),
            changelog,
            requires: strs("requires"),
            provides: strs("provides"),
            obsoletes: strs("obsoletes"),
            conflicts: strs("conflicts"),
            recommends: strs("recommends"),
        })
    }

    /// Every repository, on or off.
    pub fn repos(&self) -> Result<Vec<Repo>> {
        let opts = Opts::new().strs("repo_attrs", REPO_ATTRS).str("enable_disable", "all");
        let list: Vec<Attrs> = self.call(REPO, "list", &(opts.0,))?;
        let mut repos: Vec<Repo> = list.iter().map(read).collect();
        repos.sort_by(|a, b| b.enabled.cmp(&a.enabled).then(a.id.cmp(&b.id)));
        Ok(repos)
    }

    /// The mirrors a repository was found at (long: asked for one).
    pub fn mirrors(&self, id: &str) -> Result<Vec<String>> {
        let opts = Opts::new().strs("repo_attrs", &["id", "mirrors"]).strs("patterns", &[id]).str("enable_disable", "all");
        let list: Vec<Attrs> = self.call(REPO, "list", &(opts.0,))?;
        Ok(list.first().and_then(|a| a.get("mirrors")).map(|v| json(v)).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())).unwrap_or_default())
    }

    /// Turn repositories on or off (polkit asks: the configuration is
    /// written).
    pub fn enable(&self, ids: &[&str], on: bool) -> Result<()> {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        self.call0(REPO, if on { "enable_with_options" } else { "disable_with_options" }, &(ids, Opts::new().bool("interactive", true).0))
    }

    /// The history, newest first (`limit`: how many).
    pub fn history(&self, limit: i64) -> Result<Vec<Transaction>> {
        let opts = Opts::new()
            .i64("limit", limit)
            .strs("transaction_attrs", &["id", "start", "end", "user_id", "description", "status", "releasever", "comment"])
            .strs("package_attrs", &["name", "arch", "evr", "repo_id", "action", "reason"]);
        let list: Vec<Attrs> = self.call(HISTORY, "list", &(opts.0,))?;
        Ok(list
            .iter()
            .map(|a| {
                let j: serde_json::Map<String, serde_json::Value> = a.iter().map(|(k, v)| (k.clone(), json(v))).collect();
                let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                let n = |k: &str| j.get(k).and_then(|v| v.as_i64()).unwrap_or_default();
                let changes = |k: &str| -> Vec<Change2> { j.get(k).cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default() };
                Transaction {
                    id: n("id"),
                    start: n("start"),
                    end: n("end"),
                    user_id: n("user_id"),
                    description: s("description"),
                    comment: s("comment"),
                    status: s("status"),
                    releasever: s("releasever"),
                    installed: changes("installed"),
                    removed: changes("removed"),
                    upgraded: changes("upgraded"),
                    downgraded: changes("downgraded"),
                    reinstalled: changes("reinstalled"),
                }
            })
            .collect())
    }

    /// Whether a transaction waits for the next start.
    pub fn offline(&self) -> Result<Offline> {
        let (pending, status): (bool, Attrs) = self.call(OFFLINE, "get_status", &())?;
        Ok(Offline {
            pending,
            status: status
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match json(v) {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        },
                    )
                })
                .collect(),
        })
    }

    /// Cancel the transaction waiting for the next start.
    pub fn cancel_offline(&self) -> Result<()> {
        self.call_ok(OFFLINE, "cancel_with_options", &(Opts::new().bool("interactive", true).0,))
    }

    /// What the computer does once the waiting transaction is done:
    /// "reboot" or "poweroff".
    pub fn offline_then(&self, action: &str) -> Result<()> {
        self.call_ok(OFFLINE, "set_finish_action_with_options", &(action, Opts::new().bool("interactive", true).0))
    }

    // The goal: packages marked, resolved, run.

    /// Forget what's marked.
    pub fn reset(&self) -> Result<()> {
        self.call0(GOAL, "reset", &())
    }

    fn mark(&self, method: &str, specs: &[String]) -> Result<()> {
        self.call0(RPM, method, &(specs.to_vec(), Opts::new().0))
    }
    pub fn upgrade(&self, specs: &[String]) -> Result<()> {
        self.mark("upgrade", specs)
    }
    pub fn install(&self, specs: &[String]) -> Result<()> {
        self.mark("install", specs)
    }
    pub fn remove(&self, specs: &[String]) -> Result<()> {
        self.mark("remove", specs)
    }
    pub fn downgrade(&self, specs: &[String]) -> Result<()> {
        self.mark("downgrade", specs)
    }
    pub fn reinstall(&self, specs: &[String]) -> Result<()> {
        self.mark("reinstall", specs)
    }
    pub fn distro_sync(&self, specs: &[String]) -> Result<()> {
        self.mark("distro_sync", specs)
    }
    /// Everything to the release the session was opened for.
    pub fn system_upgrade(&self) -> Result<()> {
        self.call0(RPM, "system_upgrade", &(Opts::new().str("mode", "distrosync").bool("interactive", true).0,))
    }

    /// What the marked packages come to.
    pub fn resolve(&self, allow_erasing: bool) -> Result<Resolved> {
        type Raw = (String, String, String, Attrs, Attrs);
        let (raw, result): (Vec<Raw>, u32) = self.call(GOAL, "resolve", &(Opts::new().bool("allow_erasing", allow_erasing).bool("interactive", true).0,))?;
        let items = raw
            .into_iter()
            .map(|(object, action, reason, attrs, obj)| {
                let replaces = attrs.get("replaces").map(|v| json(v)).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|n| n.as_i64()).collect())).unwrap_or_default();
                let group = (object != "Package").then(|| {
                    let j: serde_json::Map<String, serde_json::Value> = obj.iter().map(|(k, v)| (k.clone(), json(v))).collect();
                    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    (s("groupid"), s("name"))
                });
                let mut package: Package = read(&obj);
                if package.download_size == 0
                    && let Some(n) = obj.get("package_size").map(|v| json(v)).and_then(|v| v.as_u64())
                {
                    package.download_size = n;
                }
                if package.nevra.is_empty() && !package.name.is_empty() {
                    package.nevra = format!("{}-{}.{}", package.name, package.evr, package.arch);
                }
                Item { object, action, reason, package, group, replaces }
            })
            .collect();
        let problems = if result >= 1 { self.call::<_, Vec<String>>(GOAL, "get_transaction_problems_string", &()).unwrap_or_default() } else { Vec::new() };
        Ok(Resolved { items, result, problems })
    }

    /// Run the resolved transaction (now, or `offline`: downloaded now,
    /// done as the computer next starts). Blocks until it's done; hear
    /// how it goes with [`Session::hearing`]. `comment` goes in the
    /// history.
    pub fn run(&self, offline: bool, comment: &str) -> Result<()> {
        if !may_change() {
            return Err(zbus::Error::Failure(why_not().into()));
        }
        self.call0(GOAL, "do_transaction", &(Opts::new().bool("offline", offline).bool("interactive", true).str("comment", comment).0,))
    }

    /// Stop the transaction running (only while it downloads).
    pub fn cancel(&self) -> Result<()> {
        self.call_ok(GOAL, "cancel", &())
    }

    /// Trust (or not) the key a transaction asked about.
    pub fn confirm_key(&self, key_id: &str, yes: bool) -> Result<()> {
        self.call0(REPO, "confirm_key_with_options", &(key_id, yes, Opts::new().bool("interactive", true).0))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.conn.call_method(Some(DEST), ROOT, Some(SESSIONS), "close_session", &(self.path.clone(),));
    }
}

/// Hear the session's signals for as long as its connection lasts,
/// handing them to whoever listens.
fn listen(conn: &Connection, path: &OwnedObjectPath, sink: Sink) -> Result<()> {
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).path(path.as_str())?.build();
    let iter = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(256))?;
    std::thread::Builder::new()
        .name("dnf-signals".into())
        .spawn(move || {
            for msg in iter {
                let Ok(msg) = msg else { break };
                let Some(e) = event(&msg) else { continue };
                if let Some(tx) = &*sink.lock().unwrap_or_else(|e| e.into_inner()) {
                    let _ = tx.send(Heard::Event(e));
                }
            }
        })
        .map_err(|e| zbus::Error::InputOutput(Arc::new(e)))?;
    Ok(())
}

/// A signal as an [`Event`].
fn event(msg: &zbus::Message) -> Option<Event> {
    let header = msg.header();
    let member = header.member()?.as_str().to_owned();
    let body = msg.body();
    Some(match member.as_str() {
        "download_add_new" => {
            let (_, id, what, total): (OwnedObjectPath, String, String, i64) = body.deserialize().ok()?;
            Event::DownloadStart { id, what, total }
        }
        "download_progress" => {
            let (_, id, total, done): (OwnedObjectPath, String, i64, i64) = body.deserialize().ok()?;
            Event::Download { id, done, total }
        }
        "download_end" => {
            let (_, id, status, message): (OwnedObjectPath, String, u32, String) = body.deserialize().ok()?;
            Event::DownloadEnd { id, ok: status != 2, message }
        }
        "download_mirror_failure" => {
            let (_, id, message, url, _): (OwnedObjectPath, String, String, String, String) = body.deserialize().ok()?;
            Event::MirrorFailure { id, message, url }
        }
        "repo_key_import_request" => {
            let (_, key_id, user_ids, fingerprint, url, created): (OwnedObjectPath, String, Vec<String>, String, String, i64) = body.deserialize().ok()?;
            Event::Key(KeyRequest { key_id, user_ids, fingerprint, url, created })
        }
        "repo_key_imported" => {
            let (_, key_id, _, _, _, _): (OwnedObjectPath, String, Vec<String>, String, String, i64) = body.deserialize().ok()?;
            Event::KeyImported { key_id }
        }
        "transaction_before_begin" => {
            let (_, total): (OwnedObjectPath, u64) = body.deserialize().ok()?;
            Event::Begin { total }
        }
        "transaction_verify_start" | "transaction_verify_progress" | "transaction_verify_stop" => {
            if member.ends_with("progress") {
                let (_, done, total): (OwnedObjectPath, u64, u64) = body.deserialize().ok()?;
                Event::Verify { done, total }
            } else {
                let (_, total): (OwnedObjectPath, u64) = body.deserialize().ok()?;
                Event::Verify { done: if member.ends_with("stop") { total } else { 0 }, total }
            }
        }
        "transaction_transaction_start" | "transaction_transaction_progress" | "transaction_transaction_stop" => {
            if member.ends_with("progress") {
                let (_, done, total): (OwnedObjectPath, u64, u64) = body.deserialize().ok()?;
                Event::Prepare { done, total }
            } else {
                let (_, total): (OwnedObjectPath, u64) = body.deserialize().ok()?;
                Event::Prepare { done: if member.ends_with("stop") { total } else { 0 }, total }
            }
        }
        "transaction_action_start" => {
            let (_, nevra, action, total): (OwnedObjectPath, String, u32, u64) = body.deserialize().ok()?;
            Event::Action { nevra, action, total }
        }
        "transaction_action_progress" => {
            let (_, nevra, done, total): (OwnedObjectPath, String, u64, u64) = body.deserialize().ok()?;
            Event::ActionProgress { nevra, done, total }
        }
        "transaction_action_stop" => {
            let (_, nevra, _): (OwnedObjectPath, String, u64) = body.deserialize().ok()?;
            Event::ActionDone { nevra }
        }
        "transaction_elem_progress" => {
            let (_, nevra, done, total): (OwnedObjectPath, String, u64, u64) = body.deserialize().ok()?;
            Event::Element { nevra, done, total }
        }
        "transaction_script_error" => {
            let (_, nevra, kind, code): (OwnedObjectPath, String, u32, u64) = body.deserialize().ok()?;
            Event::ScriptError { nevra, kind, code }
        }
        "transaction_unpack_error" => {
            let (_, nevra): (OwnedObjectPath, String) = body.deserialize().ok()?;
            Event::UnpackError { nevra }
        }
        "transaction_after_complete" => {
            let (_, ok): (OwnedObjectPath, bool) = body.deserialize().ok()?;
            Event::Finished { ok }
        }
        _ => return None,
    })
}

/// A byte count as people read it: "1.2 MB".
pub fn size_words(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1000.0 {
        format!("{bytes} bytes")
    } else if b < 1e6 {
        format!("{:.0} KB", b / 1e3)
    } else if b < 1e9 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.2} GB", b / 1e9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_json() {
        let v = Value::from(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(json(&v), serde_json::json!(["a", "b"]));
        let mut attrs: Attrs = HashMap::new();
        attrs.insert("name".into(), OwnedValue::try_from(Value::from("bash")).unwrap());
        attrs.insert("install_size".into(), OwnedValue::from(1234u64));
        attrs.insert("is_installed".into(), OwnedValue::from(true));
        attrs.insert("id".into(), OwnedValue::from(7i32));
        let p: Package = read(&attrs);
        assert_eq!((p.name.as_str(), p.install_size, p.is_installed), ("bash", 1234, true));
    }

    #[test]
    fn sizes_add_up() {
        let pkg = |d, i| Package { download_size: d, install_size: i, ..Default::default() };
        let r = Resolved {
            items: vec![
                Item { action: "Upgrade".into(), package: pkg(100, 1000), ..Default::default() },
                Item { action: "Replaced".into(), package: pkg(0, 900), ..Default::default() },
                Item { action: "Install".into(), package: pkg(50, 300), ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(r.sizes(), (150, 400));
        assert_eq!(size_words(1_234_567), "1.2 MB");
    }
}

#[cfg(test)]
mod live {
    /// What this computer's daemon says, read-only and from the cache:
    /// `cargo test -p olo-system --lib dnf::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_the_system() {
        let s = super::Session::open(&super::Open { cache_only: true, ..Default::default() }).expect("session");
        let u = s.updates().expect("updates");
        let kinds = |k: &str| u.updates.iter().filter(|x| x.kind(&u.advisories) == k).count();
        println!("{} updates ({} security, {} bug fixes, {} enhancements), {} advisories", u.updates.len(), kinds("security"), kinds("bugfix"), kinds("enhancement"), u.advisories.len());
        if let Some(x) = u.updates.iter().find(|x| !x.advisories.is_empty()) {
            println!("{}: {} -> {} ({}), {:?}", x.new.name, x.installed.evr, x.new.evr, x.new.repo_id, x.advisories);
            let d = s.details(&x.new.nevra, "upgrades").expect("details");
            println!("  {} changelog entries; newest: {:?}", d.changelog.len(), d.changelog.first().map(|c| (&c.author, c.text.lines().next())));
        }
        let repos = s.repos().expect("repos");
        println!("{} repositories, {} on", repos.len(), repos.iter().filter(|r| r.enabled).count());
        let h = s.history(3).expect("history");
        println!("history: {:?}", h.iter().map(|t| (t.id, t.count(), t.status.as_str())).collect::<Vec<_>>());
        println!("offline: {:?}", s.offline().expect("offline"));
        s.reset().expect("reset");
        if let Some(x) = u.updates.first() {
            s.upgrade(std::slice::from_ref(&x.new.name)).expect("mark");
            let r = s.resolve(false).expect("resolve");
            println!("resolving {}: result {}, {} items, sizes {:?}, problems {:?}", x.new.name, r.result, r.items.len(), r.sizes(), r.problems);
            for i in r.items.iter().take(3) {
                println!("  {} {} {} {}", i.action, i.reason, i.package.nevra, i.package.download_size);
            }
        }
    }
}

/// The arguments dnf refused, in its problems, each with the problem:
/// "Packages for argument 'X' available, but …", "No match for argument:
/// X", "Argument 'X' matches only …".
pub fn refused_arguments(problems: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for problem in problems {
        for line in problem.lines() {
            let quoted = ["argument '", "Argument '"].iter().find_map(|m| line.find(m).map(|at| &line[at + m.len()..])).and_then(|rest| rest.split_once('\'').map(|(a, _)| a));
            let after = line.find("argument: ").map(|at| line[at + "argument: ".len()..].split_whitespace().next().unwrap_or("").trim_end_matches(['.', ','])).filter(|a| !a.is_empty());
            if let Some(arg) = quoted.or(after)
                && !out.iter().any(|(a, _)| a == arg)
            {
                out.push((arg.to_owned(), line.trim().to_owned()));
            }
        }
    }
    out
}

#[cfg(test)]
mod refused_tests {
    use super::*;

    #[test]
    fn refused_arguments_are_read_from_problems() {
        let problems = vec![
            "Packages for argument 'python3-wcwidth.x86_64' available, but installed for a different architecture.".to_owned(),
            "No match for argument: no-such-package".to_owned(),
            "Problem 1: something else entirely".to_owned(),
        ];
        let r = refused_arguments(&problems);
        assert_eq!(r.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), ["python3-wcwidth.x86_64", "no-such-package"]);
        assert!(r[0].1.contains("different architecture"));
    }

    #[test]
    fn an_update_is_asked_for_as_installed() {
        let p = |arch: &str| Package { name: "python3-wcwidth".into(), arch: arch.into(), ..Package::default() };
        let u = |a: &str, b: &str| Update { installed: p(a), new: p(b), advisories: Vec::new() };
        assert_eq!(u("x86_64", "x86_64").upgrade_spec(), "python3-wcwidth.x86_64");
        assert_eq!(u("noarch", "x86_64").upgrade_spec(), "python3-wcwidth", "moving architecture: by name");
        assert_eq!(u("x86_64", "noarch").upgrade_spec(), "python3-wcwidth");
    }
}
