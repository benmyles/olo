//! Backup: every Olo setting and piece of saved state, in one file and
//! back, on this computer or another. A backup is a zstd-compressed tar
//! of `config/` (the settings directory, `~/.config/olo`) and `state/`
//! (`~/.local/state/olo`), with `manifest.json` first saying what it
//! holds and where it was made (`Origin`). Left out: the automatic
//! backups, and Agent Screens' own homes (`LEFT_OUT`).
//!
//! Sealed with a passphrase (`Seal::Passphrase`: Argon2id, then
//! XChaCha20-Poly1305 a megabyte at a time), a backup also holds the
//! secrets Olo keeps in the login keyring (`olo_system::secrets::KEPT`:
//! the keys to Passwords' and the Token Pool's vaults, the GitHub token,
//! Telegram's API ID and sign-in), so what it holds opens on another
//! computer; open, it holds no secret of the keyring's, and vaults whose
//! keys it lacks are left alone on another computer.
//!
//! Importing shows what a backup holds and asks before replacing
//! anything; what is there is saved first (`state/backups`, the last few,
//! sealed with a key kept in the keyring, secrets and all), so an import
//! can be undone by importing that. Files the backup doesn't have are
//! left alone. On another computer (`Origin::computer`), paths under the
//! old home folder are moved to this one's (`rehome`), Telegram's
//! sign-in stays behind (each computer signs in as itself:
//! `Carry::SameComputer`), and the Token Pool's accounts come in paused
//! (`ARRIVED`); what doesn't come along is said (`Imported::notes`).
//! Export and import run on a worker thread with progress.
//!
//! What goes in, and what comes back, is chosen a kind at a time
//! (`Choice`, of the `parts`): each app's files (`app_of`) with the
//! keyring's secrets that go with them (`part_of_secret`), and the saved
//! Wi-Fi networks; all of them unless some are left out. A backup of
//! everything can bring back only the Token Pool, say.
//!
//! Agents: `olo-preferences --export FILE [--sealed]`, `--inspect FILE`
//! and `--import FILE` do the same without a window (the summary on
//! stdout; a passphrase read from stdin), `--only "Token Pool,Passwords"`
//! for some of it.

use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, channel},
    },
    time::Duration,
};

use base64::Engine as _;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use olo_protocol::Settings;
use olo_system::{
    network::{self, SavedWifi},
    secrets::{self, Carry},
};
use olo_ui::{
    file_panel::{self, FilePanel, PanelResult},
    gpui::{Context, Entity, FontWeight, IntoElement as _, ParentElement as _, ScrollHandle, Styled as _, canvas, div, px},
    metrics as m,
    style::{self, c},
    theme,
    widgets::{Button, CheckBox, ScrollView, StatusBar, StringView, TextControl, TextField},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{Msg, PanelView, form::Ui, panel};

pub const EXTENSION: &str = "olo-backup";
/// Backups made before the rename (Tabula), which still import.
pub const OLD_EXTENSION: &str = "tabula-backup";
const FORMAT: &str = "olo-backup";
const OLD_FORMAT: &str = "tabula-backup";
const VERSION: u32 = 2;
const MANIFEST: &str = "manifest.json";
/// A sealed backup's payload, after its manifest: a zstd tar of its files
/// and `SECRETS`, sealed.
const SEALED: &str = "sealed";
/// In a sealed payload: the keyring's secrets (`Secret`s), and the
/// saved Wi-Fi networks (`Wifi`).
const SECRETS: &str = "secrets.json";
const WIFI: &str = "wifi.json";
/// Automatic backups made before an import that are kept.
const KEEP: usize = 5;
/// Never in a backup, nor put back from one: the automatic backups, and
/// Agent Screens' own homes (an agent's desktop on this computer: their
/// apps' settings and files, gigabytes of them).
const LEFT_OUT: &[&str] = &["state/backups", "state/screens"];
/// Beside the Token Pool's store, from a backup imported on another
/// computer: the pool pauses its accounts as it next opens
/// (`olo_token_pool::store::ARRIVED`).
const ARRIVED: &str = "config/pool-arrived.json";
/// The vaults whose keys are in the keyring: on another computer, one
/// comes only with its key.
const VAULTS: &[(&str, secrets::Kept)] = &[("config/passwords.vault", secrets::PASSWORDS_KEY), ("config/pool.vault", secrets::POOL_KEY)];

/// What a backup holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    /// When it was made (RFC 3339, local time).
    pub created: String,
    pub files: Vec<FileEntry>,
    /// Where it was made (none before version 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Origin>,
    /// How its files are sealed (none: open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealing>,
    /// The keyring's secrets it holds, by what they are ("Passwords").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// `config/…` or `state/…`.
    pub path: String,
    pub size: u64,
}

/// Where a backup was made.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Origin {
    /// The home folder its paths are under.
    pub home: String,
    /// Which computer and person: a digest of the machine's id and the
    /// user's (the same both ways is the same computer; it says nothing
    /// more about either).
    pub computer: String,
    /// The processor ("aarch64", "x86_64").
    pub arch: String,
}

impl Origin {
    pub fn here() -> Origin {
        use sha2::Digest as _;
        let id = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
        // SAFETY: getuid can't fail.
        let uid = unsafe { libc::getuid() };
        let digest = sha2::Sha256::digest(format!("olo-backup\n{}\n{uid}", id.trim()).as_bytes());
        Origin { home: std::env::var("HOME").unwrap_or_default(), computer: digest[..12].iter().map(|b| format!("{b:02x}")).collect(), arch: std::env::consts::ARCH.into() }
    }
}

/// How a backup's files are sealed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sealing {
    /// "passphrase" (Argon2id of what the person typed, with `salt` and
    /// these costs), or "keyring" (a key kept in the login keyring, the
    /// item named by `salt`: the backups made before an import).
    pub with: String,
    pub salt: String,
    #[serde(default)]
    pub m_kib: u32,
    #[serde(default)]
    pub passes: u32,
}

/// How to seal a backup being made.
pub enum Seal {
    /// Not: no secret of the keyring's in it.
    Open,
    /// With what the person typed.
    Passphrase(Zeroizing<String>),
    /// With a new key kept in the login keyring (the backups made before
    /// an import, which only this computer and person can open).
    Keyring,
}

/// Argon2id's costs for a passphrase: 64 MiB, three passes (a third of a
/// second or so, on the worker).
const ARGON_M_KIB: u32 = if cfg!(test) { 256 } else { 64 * 1024 };
const ARGON_PASSES: u32 = if cfg!(test) { 1 } else { 3 };
/// A sealed payload goes in pieces this big, each sealed on its own.
const CHUNK: usize = 1 << 20;
/// The sealed payload's header: magic and format, then a 16-byte nonce
/// prefix (each piece's nonce is it and the piece's number).
const SEAL_MAGIC: &[u8; 4] = b"OLOB";
const SEAL_FORMAT: u8 = 1;
/// The keyring item a "keyring" sealed backup's key is in.
fn backup_key_attrs(id: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("purpose", "backup-key"), ("backup", id)]
}

/// This computer, as Backup sees it: its keyring, its saved networks,
/// and where it is (tests make their own).
pub struct Here<'a> {
    pub keyring: &'a dyn Keyring,
    pub networks: &'a dyn Networks,
    pub origin: Origin,
}

impl Here<'static> {
    /// The computer this runs on.
    pub fn this() -> Here<'static> {
        Here { keyring: &Login, networks: &NetworkManager, origin: Origin::here() }
    }
}

/// The saved Wi-Fi networks, as Backup uses them (NetworkManager's; tests
/// use their own).
pub trait Networks: Sync {
    /// Those that can go to another computer, with their passwords, and
    /// the names of those that can't (enterprise ones). An error: there's
    /// no NetworkManager.
    fn saved(&self) -> Result<(Vec<SavedWifi>, Vec<String>), String>;
    fn has(&self, ssid: &[u8]) -> Result<bool, String>;
    fn add(&self, network: &SavedWifi) -> Result<(), String>;
}

/// NetworkManager's (`olo_system::network`).
pub struct NetworkManager;

impl Networks for NetworkManager {
    fn saved(&self) -> Result<(Vec<SavedWifi>, Vec<String>), String> {
        network::saved_wifi().map_err(|e| e.to_string())
    }
    fn has(&self, ssid: &[u8]) -> Result<bool, String> {
        network::has_wifi(ssid).map_err(|e| e.to_string())
    }
    fn add(&self, network: &SavedWifi) -> Result<(), String> {
        network::add_wifi(network).map_err(|e| e.to_string())
    }
}

/// A sealed backup's saved Wi-Fi networks, and those left behind.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Wifi {
    networks: Vec<SavedWifi>,
    left: Vec<String>,
}

/// What a sealed backup holds besides its files.
#[derive(Default)]
struct Sealed {
    secrets: Vec<Secret>,
    wifi: Option<Wifi>,
}

/// The login keyring, as Backup uses it (tests use their own).
pub trait Keyring: Sync {
    /// There is one to keep secrets in.
    fn available(&self) -> bool;
    fn lookup(&self, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, String>;
    fn store(&self, label: &str, attributes: &[(&str, &str)], secret: &[u8]) -> Result<(), String>;
    fn delete(&self, attributes: &[(&str, &str)]) -> Result<(), String>;
}

/// The person's login keyring (`olo_system::secrets`).
pub struct Login;

impl Keyring for Login {
    fn available(&self) -> bool {
        secrets::present()
    }
    fn lookup(&self, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, String> {
        secrets::lookup(attributes).map_err(|e| e.to_string())
    }
    fn store(&self, label: &str, attributes: &[(&str, &str)], secret: &[u8]) -> Result<(), String> {
        secrets::store(label, attributes, secret).map_err(|e| e.to_string())
    }
    fn delete(&self, attributes: &[(&str, &str)]) -> Result<(), String> {
        secrets::delete(attributes).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// A secret of the keyring's, in a sealed backup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Secret {
    label: String,
    attributes: Vec<(String, String)>,
    /// Base64.
    secret: String,
}

impl Secret {
    fn attrs(&self) -> Vec<(&str, &str)> {
        self.attributes.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    /// Which of Olo's it is, if one this Olo knows.
    fn kept(&self) -> Option<secrets::Kept> {
        secrets::KEPT.iter().copied().find(|k| k.attributes.len() == self.attributes.len() && k.attributes.iter().all(|(a, b)| self.attributes.iter().any(|(x, y)| x == a && y == b)))
    }
}

impl Manifest {
    /// The files by app: (app, files, bytes), apps by name.
    pub fn groups(&self) -> Vec<(String, usize, u64)> {
        let mut groups: Vec<(String, usize, u64)> = Vec::new();
        for f in &self.files {
            let app = app_of(&f.path);
            match groups.iter_mut().find(|g| g.0 == app) {
                Some(g) => {
                    g.1 += 1;
                    g.2 += f.size;
                }
                None => groups.push((app, 1, f.size)),
            }
        }
        groups.sort_by_key(|g| g.0.to_lowercase());
        groups
    }

    /// What it holds, part by part (`Choice`): (part, files, bytes), by
    /// name; a sealed backup's secrets and Wi-Fi networks with the part
    /// they go with (one of their own: no files).
    pub fn parts(&self) -> Vec<(String, usize, u64)> {
        let mut parts = self.groups();
        for what in &self.secrets {
            let part = part_of_secret(what);
            if !parts.iter().any(|p| p.0 == part) {
                parts.push((part, 0, 0));
            }
        }
        parts.sort_by_key(|p| p.0.to_lowercase());
        parts
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// "Oct 27, 2026, 2:20 AM" (or as written, if it can't be read).
    pub fn created_text(&self) -> String {
        self.created.parse::<jiff::Timestamp>().map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%b %-d, %Y, %-I:%M\u{a0}%p").to_string()).unwrap_or_else(|_| self.created.clone())
    }

    /// Sealed with a passphrase (asked for before importing).
    pub fn needs_passphrase(&self) -> bool {
        self.sealed.as_ref().is_some_and(|s| s.with == "passphrase")
    }

    /// Made on another computer (or by another person) than `here`.
    pub fn from_elsewhere(&self, here: &Origin) -> bool {
        self.from.as_ref().is_some_and(|f| f.computer != here.computer)
    }
}

/// The saved Wi-Fi networks, as a part of a backup (`Choice`).
pub const WIFI_PART: &str = "Wi-Fi Networks";

/// The part of a backup a secret of the keyring's goes with (by what
/// it is, `Kept::what`): Telegram's sign-in with Telegram, the rest
/// with what they open (the Token Pool's key with its accounts), or one
/// of their own (GitHub).
fn part_of_secret(what: &str) -> String {
    match what {
        "Telegram sign-in" => "Telegram".into(),
        "Wi-Fi networks" => WIFI_PART.into(),
        w => w.into(),
    }
}

/// Which parts of a backup go in, or come back: every one but those
/// left out (so a part new since is in).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choice {
    pub left_out: std::collections::BTreeSet<String>,
}

impl Choice {
    /// Everything.
    pub fn all() -> Choice {
        Choice::default()
    }

    pub fn has(&self, part: &str) -> bool {
        !self.left_out.contains(part)
    }

    /// Only `wanted` (as the parts are named, in any case) of `parts`;
    /// an error names those that aren't parts, and the parts.
    pub fn only(wanted: &[String], parts: &[String]) -> Result<Choice, String> {
        let found = |w: &String| parts.iter().find(|p| p.eq_ignore_ascii_case(w.trim()));
        let unknown: Vec<&String> = wanted.iter().filter(|w| found(w).is_none()).collect();
        if !unknown.is_empty() {
            return Err(format!("not parts of it: {}; its parts are {}", unknown.iter().map(|u| format!("“{u}”")).collect::<Vec<_>>().join(", "), parts.join(", ")));
        }
        let keep: Vec<&String> = wanted.iter().filter_map(found).collect();
        Ok(Choice { left_out: parts.iter().filter(|p| !keep.contains(p)).cloned().collect() })
    }

    pub fn set(&mut self, part: &str, on: bool) {
        if on {
            self.left_out.remove(part);
        } else {
            self.left_out.insert(part.to_owned());
        }
    }
}

/// This computer's parts, as an export holds them: (part, files, bytes),
/// by name; the secrets' parts without files of their own (GitHub) and
/// the Wi-Fi networks go only in a sealed backup. Walks the settings'
/// folders: on a worker.
pub fn parts_here(roots: &[(&'static str, PathBuf)]) -> Vec<(String, usize, u64)> {
    let mut parts: Vec<(String, usize, u64)> = Vec::new();
    for (name, path) in collect(roots) {
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        add_to(&mut parts, app_of(&name), size);
    }
    for k in secrets::KEPT {
        let part = part_of_secret(k.what);
        if !parts.iter().any(|p| p.0 == part) {
            parts.push((part, 0, 0));
        }
    }
    parts.push((WIFI_PART.into(), 0, 0));
    parts.sort_by_key(|p| p.0.to_lowercase());
    parts
}

fn add_to(parts: &mut Vec<(String, usize, u64)>, part: String, bytes: u64) {
    match parts.iter_mut().find(|g| g.0 == part) {
        Some(g) => {
            g.1 += 1;
            g.2 += bytes;
        }
        None => parts.push((part, 1, bytes)),
    }
}

/// Which app a backed-up file belongs to, for the summary.
fn app_of(path: &str) -> String {
    let rel = path.split_once('/').map_or(path, |(_, r)| r);
    for part in rel.split('/') {
        let stem = part.split('.').next().unwrap_or(part);
        match stem {
            "olo" | "tabula" | "" => continue,
            "app_server" => return "Desktop (Preferences)".into(),
            "signatures" => return "Viewer".into(),
            "desktop" => return "Tracker".into(),
            "palette" => return "Palette".into(),
            _ => {}
        }
        if let Some((name, _)) = olo_xdg::apps::our_app(stem) {
            return name.into();
        }
        let mut name = stem.replace(['-', '_'], " ");
        if let Some(first) = name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        return name;
    }
    "Other".into()
}

/// The directories a backup holds, by their name in it.
pub fn roots() -> Vec<(&'static str, PathBuf)> {
    vec![("config", olo_ui::config_dir()), ("state", olo_ui::state_dir())]
}

/// Where automatic backups go (left out of backups themselves).
fn backups_dir(roots: &[(&'static str, PathBuf)]) -> Option<PathBuf> {
    roots.iter().find(|(n, _)| *n == "state").map(|(_, d)| d.join("backups"))
}

/// A name in a backup that is never backed up nor put back (`LEFT_OUT`).
fn left_out(name: &str) -> bool {
    LEFT_OUT.iter().any(|l| name == *l || name.strip_prefix(l).is_some_and(|rest| rest.starts_with('/')))
}

/// Every regular file under the roots: (name in the backup, path).
fn collect(roots: &[(&'static str, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for (name, dir) in roots {
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let Ok(t) = e.file_type() else { continue };
                let Ok(rel) = path.strip_prefix(dir) else { continue };
                let named = format!("{name}/{}", rel.to_string_lossy());
                if left_out(&named) {
                    continue;
                }
                if t.is_dir() {
                    stack.push(path);
                    continue;
                }
                let file = e.file_name().to_string_lossy().into_owned();
                // Half-written files (atomic saves in progress).
                if !t.is_file() || file.ends_with(".saving") || file.ends_with(".tmp") || file.ends_with(".partial") {
                    continue;
                }
                out.push((named, path));
            }
        }
    }
    out.sort();
    out
}

fn now() -> String {
    jiff::Zoned::now().timestamp().to_string()
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("random bytes");
    b
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// The key a passphrase makes with `salt`.
fn passphrase_key(passphrase: &str, salt: &[u8], m_kib: u32, passes: u32) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = argon2::Params::new(m_kib, passes, 1, Some(32)).map_err(|e| e.to_string())?;
    let mut key = Zeroizing::new([0u8; 32]);
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params).hash_password_into(passphrase.as_bytes(), salt, key.as_mut()).map_err(|e| e.to_string())?;
    Ok(key)
}

/// `plain` sealed: the header, then each `CHUNK` sealed with its number
/// and whether it's the last (so pieces can't be dropped, moved or cut
/// off the end unnoticed).
fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let prefix = random::<16>();
    let mut out = SEAL_MAGIC.to_vec();
    out.push(SEAL_FORMAT);
    out.extend_from_slice(&prefix);
    let cipher = XChaCha20Poly1305::new(key.into());
    let pieces: Vec<&[u8]> = if plain.is_empty() { vec![&[]] } else { plain.chunks(CHUNK).collect() };
    let n = pieces.len();
    for (i, piece) in pieces.into_iter().enumerate() {
        let (nonce, aad) = piece_nonce(&prefix, i as u64, i + 1 == n);
        out.extend(cipher.encrypt(&XNonce::from(nonce), Payload { msg: piece, aad: &aad }).expect("sealing"));
    }
    out
}

fn piece_nonce(prefix: &[u8; 16], i: u64, last: bool) -> ([u8; 24], [u8; 6]) {
    let mut nonce = [0u8; 24];
    nonce[..16].copy_from_slice(prefix);
    nonce[16..].copy_from_slice(&i.to_be_bytes());
    let mut aad = [0u8; 6];
    aad[..4].copy_from_slice(SEAL_MAGIC);
    aad[4] = SEAL_FORMAT;
    aad[5] = last as u8;
    (nonce, aad)
}

/// `seal` undone; an error if any of it was changed, or the key is
/// another's.
fn unseal(key: &[u8; 32], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    let damaged = || "The backup is damaged.".to_string();
    if sealed.len() < 5 + 16 + 16 || &sealed[..4] != SEAL_MAGIC || sealed[4] != SEAL_FORMAT {
        return Err(damaged());
    }
    let prefix: [u8; 16] = sealed[5..21].try_into().map_err(|_| damaged())?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut out = Zeroizing::new(Vec::with_capacity(sealed.len()));
    let pieces: Vec<&[u8]> = sealed[21..].chunks(CHUNK + 16).collect();
    let n = pieces.len();
    for (i, piece) in pieces.into_iter().enumerate() {
        let (nonce, aad) = piece_nonce(&prefix, i as u64, i + 1 == n);
        let plain = cipher.decrypt(&XNonce::from(nonce), Payload { msg: piece, aad: &aad }).map_err(|_| if i == 0 { "That isn't the backup's password.".to_string() } else { damaged() })?;
        out.extend_from_slice(&plain);
    }
    Ok(out)
}

/// Progress: (bytes done, bytes in all).
pub type Progress<'a> = &'a dyn Fn(u64, u64);

/// Write a backup of `roots` to `dest` (atomically), sealed as `how`
/// says; a sealed one takes the keyring's secrets along. `here` is this
/// computer (`Origin::here`).
pub fn export(roots: &[(&'static str, PathBuf)], dest: &Path, how: &Seal, here: &Here, progress: Progress, cancel: &AtomicBool) -> Result<Manifest, String> {
    export_only(roots, dest, how, here, &Choice::all(), progress, cancel)
}

/// [`export`], of the parts `choice` has.
pub fn export_only(roots: &[(&'static str, PathBuf)], dest: &Path, how: &Seal, here: &Here, choice: &Choice, progress: Progress, cancel: &AtomicBool) -> Result<Manifest, String> {
    let keyring = here.keyring;
    let mut files = Vec::new();
    let mut data = Vec::new();
    for (name, path) in collect(roots).into_iter().filter(|(name, _)| choice.has(&app_of(name))) {
        // Read now, so the manifest and the archive agree.
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mode = std::fs::metadata(&path).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap_or(0o644);
                files.push(FileEntry { path: name.clone(), size: bytes.len() as u64 });
                data.push((name, Zeroizing::new(bytes), mode));
            }
            Err(e) => tracing::info!("backup: skipping {}: {e}", path.display()),
        }
    }
    // The key, and the keyring's secrets, if it's sealed.
    let (key, sealing) = match how {
        Seal::Open => (None, None),
        Seal::Passphrase(words) => {
            let salt = random::<16>();
            let key = passphrase_key(words, &salt, ARGON_M_KIB, ARGON_PASSES)?;
            (Some(key), Some(Sealing { with: "passphrase".into(), salt: b64().encode(salt), m_kib: ARGON_M_KIB, passes: ARGON_PASSES }))
        }
        Seal::Keyring => {
            let id: String = random::<8>().iter().map(|b| format!("{b:02x}")).collect();
            let key = Zeroizing::new(random::<32>());
            keyring.store(&format!("Olo settings before an import ({})", jiff::Zoned::now().strftime("%b %-d, %Y, %-I:%M %p")), &backup_key_attrs(&id), key.as_ref())?;
            (Some(key), Some(Sealing { with: "keyring".into(), salt: id, m_kib: 0, passes: 0 }))
        }
    };
    let mut kept = Vec::new();
    // (No keyring: sealed even so, with no secrets of its in it.)
    if key.is_some() && keyring.available() {
        for k in secrets::KEPT.iter().filter(|k| choice.has(&part_of_secret(k.what))) {
            match keyring.lookup(k.attributes) {
                Ok(Some(value)) => {
                    kept.push((k, Secret { label: k.label.into(), attributes: k.attributes.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), secret: b64().encode(&value) }))
                }
                Ok(None) => {}
                Err(e) => return Err(format!("The login keyring couldn't be read: {e}")),
            }
        }
    }
    let mut whats: Vec<String> = Vec::new();
    for (k, _) in &kept {
        if !whats.iter().any(|w| w == k.what) {
            whats.push(k.what.into());
        }
    }
    // The saved Wi-Fi networks, with their passwords: in a backup sealed
    // with a password (one made before an import leaves them: an import
    // only adds networks, so undoing it needs none). No NetworkManager:
    // none.
    let wifi = match how {
        Seal::Passphrase(_) if choice.has(WIFI_PART) => here.networks.saved().ok().map(|(networks, left)| Wifi { networks, left }).filter(|w| !w.networks.is_empty() || !w.left.is_empty()),
        _ => None,
    };
    if wifi.as_ref().is_some_and(|w| !w.networks.is_empty()) {
        whats.push("Wi-Fi networks".into());
    }
    let manifest = Manifest { format: FORMAT.into(), version: VERSION, created: now(), files, from: Some(here.origin.clone()), sealed: sealing, secrets: whats };
    let total = manifest.bytes();
    let partial = dest.with_file_name(format!(".{}.partial", dest.file_name().unwrap_or_default().to_string_lossy()));
    let write = || -> Result<(), String> {
        let file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let enc = zstd::Encoder::new(file, 9).map_err(|e| e.to_string())?;
        let mut tar = tar::Builder::new(enc);
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        append(&mut tar, MANIFEST, &json, 0o644)?;
        let mut done = 0;
        let mut each = |tar: &mut dyn FnMut(&str, &[u8], u32) -> Result<(), String>| -> Result<(), String> {
            for (name, bytes, mode) in &data {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled.".into());
                }
                tar(name, bytes, *mode)?;
                done += bytes.len() as u64;
                progress(done, total);
            }
            Ok(())
        };
        match &key {
            None => each(&mut |name, bytes, mode| append(&mut tar, name, bytes, mode))?,
            Some(key) => {
                // The files and the secrets, in a tar of their own, sealed.
                let mut inner = tar::Builder::new(zstd::Encoder::new(Vec::new(), 9).map_err(|e| e.to_string())?);
                each(&mut |name, bytes, mode| append(&mut inner, name, bytes, mode))?;
                let secrets = Zeroizing::new(serde_json::to_vec(&kept.iter().map(|(_, s)| s).collect::<Vec<_>>()).map_err(|e| e.to_string())?);
                append(&mut inner, SECRETS, &secrets, 0o600)?;
                if let Some(w) = &wifi {
                    append(&mut inner, WIFI, &Zeroizing::new(serde_json::to_vec(w).map_err(|e| e.to_string())?), 0o600)?;
                }
                let plain = Zeroizing::new(inner.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?);
                append(&mut tar, SEALED, &seal(key, &plain), 0o600)?;
            }
        }
        let enc = tar.into_inner().map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())
    };
    match write() {
        Ok(()) => std::fs::rename(&partial, dest).map_err(|e| e.to_string()).map(|()| manifest),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            if let Some(s) = &manifest.sealed
                && s.with == "keyring"
            {
                let _ = keyring.delete(&backup_key_attrs(&s.salt));
            }
            Err(e)
        }
    }
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    h.set_mode(mode);
    h.set_mtime(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    h.set_entry_type(tar::EntryType::Regular);
    tar.append_data(&mut h, name, bytes).map_err(|e| e.to_string())
}

fn open(path: &Path) -> Result<tar::Archive<zstd::Decoder<'static, std::io::BufReader<std::fs::File>>>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = zstd::Decoder::new(file).map_err(|_| "This isn't an Olo backup.".to_string())?;
    Ok(tar::Archive::new(dec))
}

fn not_backup() -> String {
    "This isn't an Olo backup.".into()
}

/// What the backup at `path` holds (its manifest, checked).
pub fn inspect(path: &Path) -> Result<Manifest, String> {
    let mut archive = open(path)?;
    let mut entries = archive.entries().map_err(|_| not_backup())?;
    let mut first = entries.next().ok_or_else(not_backup)?.map_err(|_| not_backup())?;
    if first.path().ok().as_deref() != Some(Path::new(MANIFEST)) {
        return Err(not_backup());
    }
    let mut json = String::new();
    first.read_to_string(&mut json).map_err(|_| not_backup())?;
    let manifest: Manifest = serde_json::from_str(&json).map_err(|_| not_backup())?;
    if manifest.format != FORMAT && manifest.format != OLD_FORMAT {
        return Err(not_backup());
    }
    if manifest.version > VERSION || manifest.sealed.as_ref().is_some_and(|s| s.with != "passphrase" && s.with != "keyring") {
        return Err("This backup was made by a newer Olo.".into());
    }
    Ok(manifest)
}

/// Where a file named `name` in a backup goes, if it is one of ours.
fn target(roots: &[(&'static str, PathBuf)], name: &Path) -> Option<PathBuf> {
    let mut parts = name.components();
    let Some(Component::Normal(root)) = parts.next() else { return None };
    let (_, dir) = roots.iter().find(|(n, _)| std::ffi::OsStr::new(n) == root)?;
    let rest: PathBuf = parts.clone().collect();
    if rest.as_os_str().is_empty() || !parts.all(|p| matches!(p, Component::Normal(_))) {
        return None;
    }
    Some(dir.join(rest))
}

/// `text` with the paths under `old` (a home folder) under `new`
/// instead: where `old` is a path's start ("/home/ben/x", `file://`
/// ones too) and the whole of a name (not "/home/benjamin", nor
/// "/srv/home/ben"). None if there are none.
fn rehome(text: &str, old: &str, new: &str) -> Option<String> {
    if old.is_empty() || old == new || !text.contains(old) {
        return None;
    }
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '-');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(i) = rest.find(old) {
        let before = rest[..i].chars().next_back().or(out.chars().next_back());
        let after = rest[i + old.len()..].chars().next();
        let starts = before.is_none_or(|c| !name_char(c));
        let ends = after.is_none_or(|c| !name_char(c));
        out.push_str(&rest[..i]);
        if starts && ends {
            out.push_str(new);
            changed = true;
        } else {
            out.push_str(old);
        }
        rest = &rest[i + old.len()..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// app_server's settings (`app_server.json`) from another computer,
/// without what was its own: displays known by their connector, not
/// their monitor (`is_connector_key`: on another computer the port means
/// another display, its built-in panel say), as the main display, with
/// their own settings or windowing modes. Edited as JSON, so what a newer
/// Olo saved comes along untouched. None if it changes nothing.
fn without_connectors(json: &[u8]) -> Option<Vec<u8>> {
    use olo_protocol::is_connector_key;
    let mut v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let mut changed = false;
    if let Some(display) = v.get_mut("display").and_then(|d| d.as_object_mut()) {
        if display.get("main").and_then(|m| m.as_str()).is_some_and(is_connector_key) {
            display.insert("main".into(), serde_json::Value::Null);
            changed = true;
        }
        if let Some(outputs) = display.get_mut("outputs").and_then(|o| o.as_object_mut()) {
            let before = outputs.len();
            outputs.retain(|k, _| !is_connector_key(k));
            changed |= outputs.len() != before;
        }
    }
    if let Some(modes) = v.pointer_mut("/tiling/modes").and_then(|m| m.as_array_mut()) {
        let before = modes.len();
        modes.retain(|m| !m.get("display").and_then(|d| d.as_str()).is_some_and(is_connector_key));
        changed |= modes.len() != before;
    }
    changed.then(|| serde_json::to_vec_pretty(&v).ok()).flatten()
}

/// What an import did.
#[derive(Debug)]
pub struct Imported {
    pub manifest: Manifest,
    pub files: usize,
    /// The secrets put in the keyring.
    pub secrets: usize,
    /// The settings as they were, saved first.
    pub before: Option<PathBuf>,
    /// app_server's settings from the backup, to hand to it (it keeps its
    /// own copy and would write the old one back).
    pub settings: Option<Settings>,
    /// What didn't come along, or came differently, and why.
    pub notes: Vec<String>,
}

/// The backup's files (name, mode, bytes) and secrets, opened: sealed
/// ones with `passphrase` (or the keyring's key, for those sealed so).
fn contents(src: &Path, manifest: &Manifest, passphrase: Option<&str>, keyring: &dyn Keyring, mut each: impl FnMut(&Path, u32, Zeroizing<Vec<u8>>) -> Result<(), String>) -> Result<Sealed, String> {
    let mut archive = open(src)?;
    let mut sealed = None;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let name = entry.path().map_err(|e| e.to_string())?.into_owned();
        if name == Path::new(MANIFEST) || entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        let mut bytes = Zeroizing::new(Vec::new());
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        match &manifest.sealed {
            Some(_) if name == Path::new(SEALED) => sealed = Some(bytes),
            Some(_) => {}
            None => each(&name, mode, bytes)?,
        }
    }
    let Some(how) = &manifest.sealed else { return Ok(Sealed::default()) };
    let sealed = sealed.ok_or_else(|| "The backup is damaged.".to_string())?;
    let key = match how.with.as_str() {
        "passphrase" => {
            let words = passphrase.ok_or_else(|| "The backup's password is needed to open it.".to_string())?;
            let salt = b64().decode(&how.salt).map_err(|_| "The backup is damaged.".to_string())?;
            passphrase_key(words, &salt, how.m_kib, how.passes)?
        }
        _ => {
            let bytes = keyring.lookup(&backup_key_attrs(&how.salt))?.ok_or_else(|| "Its key isn't in this login keyring: it opens only on the computer that made it.".to_string())?;
            Zeroizing::new(bytes.as_slice().try_into().map_err(|_| "Its key is damaged.".to_string())?)
        }
    };
    let plain = unseal(&key, &sealed)?;
    let mut inner = tar::Archive::new(zstd::Decoder::new(plain.as_slice()).map_err(|_| "The backup is damaged.".to_string())?);
    let mut kept = Sealed::default();
    for entry in inner.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let name = entry.path().map_err(|e| e.to_string())?.into_owned();
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        let mut bytes = Zeroizing::new(Vec::new());
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if name == Path::new(SECRETS) {
            kept.secrets = serde_json::from_slice(&bytes).map_err(|_| "The backup is damaged.".to_string())?;
        } else if name == Path::new(WIFI) {
            kept.wifi = Some(serde_json::from_slice(&bytes).map_err(|_| "The backup is damaged.".to_string())?);
        } else if entry.header().entry_type() == tar::EntryType::Regular {
            each(&name, mode, bytes)?;
        }
    }
    Ok(kept)
}

/// [`import_only`] of everything (the tests'; the panel and `--import`
/// choose).
#[cfg(test)]
pub fn import(roots: &[(&'static str, PathBuf)], src: &Path, passphrase: Option<&str>, here: &Here, progress: Progress, cancel: &AtomicBool) -> Result<Imported, String> {
    import_only(roots, src, passphrase, here, &Choice::all(), progress, cancel)
}

/// Replace the files under `roots` with the backup's parts that
/// `choice` has, and the keyring's secrets with theirs, saving what is
/// there first (all of it); the other parts stay as they are here.
/// `here` is this computer: one made elsewhere is moved in (`rehome`,
/// `Carry`).
pub fn import_only(roots: &[(&'static str, PathBuf)], src: &Path, passphrase: Option<&str>, here: &Here, choice: &Choice, progress: Progress, cancel: &AtomicBool) -> Result<Imported, String> {
    let keyring = here.keyring;
    let manifest = inspect(src)?;
    let elsewhere = manifest.from_elsewhere(&here.origin);
    let old_home = manifest.from.as_ref().map(|f| f.home.clone()).filter(|h| !h.is_empty() && *h != here.origin.home);
    // Opened first, so a wrong password replaces nothing.
    let mut files: Vec<(PathBuf, u32, Zeroizing<Vec<u8>>)> = Vec::new();
    let Sealed { secrets: mut kept, wifi } = contents(src, &manifest, passphrase, keyring, |name, mode, bytes| {
        if choice.has(&app_of(&name.to_string_lossy())) {
            files.push((name.to_path_buf(), mode, bytes));
        }
        Ok(())
    })?;
    kept.retain(|s| choice.has(&part_of_secret(s.kept().map_or(s.label.as_str(), |k| k.what))));
    let wifi = wifi.filter(|_| choice.has(WIFI_PART));
    let mut notes = Vec::new();
    // No keyring here to keep them in: as if the backup had none (and so
    // the vaults they open stay as they are, on another computer).
    if !kept.is_empty() && !keyring.available() {
        kept.clear();
        notes.push("Passwords and sign-ins: there's no login keyring here to keep them in, so they didn't come along.".to_string());
    }
    // What is there, so this can be undone: sealed with a key in the
    // keyring, secrets and all; open, without them, if there is no
    // keyring (and then no secret is replaced).
    let before = match backups_dir(roots) {
        Some(dir) if !collect(roots).is_empty() || !kept.is_empty() => {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let stamp = jiff::Zoned::now().strftime("%Y-%m-%d %H.%M.%S").to_string();
            let path = dir.join(format!("Before import {stamp}.{EXTENSION}"));
            let sealed = export(roots, &path, &Seal::Keyring, here, &|_, _| {}, cancel).map(|_| true);
            match sealed {
                Ok(s) => Some((path, s)),
                Err(e) if !cancel.load(Ordering::Relaxed) => {
                    tracing::info!("backup: the settings before couldn't be sealed ({e}); saved open, without the keyring's secrets");
                    export(roots, &path, &Seal::Open, here, &|_, _| {}, cancel)?;
                    Some((path, false))
                }
                Err(e) => return Err(e),
            }
        }
        _ => None,
    };
    if let Some(dir) = backups_dir(roots) {
        prune(&dir, keyring);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled.".into());
    }
    // The vaults that come only with their keys: on another computer, a
    // backup without one leaves this computer's vault as it is.
    let has_secret = |k: &secrets::Kept| kept.iter().any(|s| s.kept() == Some(*k));
    let skip: Vec<&str> = VAULTS.iter().filter(|(_, k)| elsewhere && !has_secret(k)).map(|(f, _)| *f).collect();
    for (_, k) in VAULTS.iter().filter(|(f, _)| skip.contains(f) && files.iter().any(|(n, ..)| n == Path::new(f))) {
        notes.push(format!("{}: left as it is here. The backup holds no key to open what it has of it (one exported without passwords and sign-ins holds none).", k.what));
    }
    // From here on it runs to the end: each file is replaced whole.
    let total: u64 = files.iter().map(|(_, _, b)| b.len() as u64).sum();
    let (mut done, mut written, mut settings): (u64, usize, Option<Settings>) = (0, 0, None);
    for (name, mode, bytes) in files {
        let shown = name.to_string_lossy();
        if left_out(&shown) || skip.contains(&shown.as_ref()) {
            continue;
        }
        let Some(dest) = target(roots, &name) else {
            tracing::info!("backup: ignoring {}", name.display());
            continue;
        };
        // Paths under the old home folder, under this one's.
        let moved = old_home.as_deref().and_then(|old| rehome(std::str::from_utf8(&bytes).ok()?, old, &here.origin.home)).map(|t| Zeroizing::new(t.into_bytes()));
        let mut bytes = moved.unwrap_or(bytes);
        if name == Path::new("config/app_server.json") {
            if elsewhere && let Some(own) = without_connectors(&bytes) {
                bytes = Zeroizing::new(own);
            }
            settings = serde_json::from_slice(&bytes).ok();
        }
        write_atomic(&dest, &bytes, mode).map_err(|e| format!("“{}” couldn't be written: {e}", dest.display()))?;
        written += 1;
        done += bytes.len() as u64;
        progress(done, total);
    }
    // The keyring's secrets: each replaced, unless it is a sign-in of the
    // other computer's own, or the one here couldn't be saved first.
    let saved_before = before.as_ref().is_some_and(|(_, sealed)| *sealed);
    let mut stored = 0;
    for s in &kept {
        let kept_as = s.kept();
        let what = kept_as.map_or("A secret", |k| k.what);
        if elsewhere && kept_as.is_none_or(|k| k.carry == Carry::SameComputer) {
            notes.push(format!("{what}: sign in again on this computer (each computer signs in as itself)."));
            continue;
        }
        let value = Zeroizing::new(b64().decode(&s.secret).map_err(|_| "The backup is damaged.".to_string())?);
        let attrs = s.attrs();
        let now = keyring.lookup(&attrs)?;
        if now.as_deref() == Some(value.as_slice()) {
            stored += 1;
            continue;
        }
        if now.is_some() && !saved_before {
            notes.push(format!("{what}: left as it is here, as this computer's couldn't be saved first."));
            continue;
        }
        match keyring.store(&s.label, &attrs, &value) {
            Ok(()) => stored += 1,
            Err(e) => notes.push(format!("{what}: the login keyring didn't take it ({e}). Import the backup again once it does.")),
        }
    }
    // The saved Wi-Fi networks this computer hasn't saved (one it has
    // keeps its own settings).
    if let Some(w) = &wifi {
        let mut added = Vec::new();
        for n in &w.networks {
            match here.networks.has(&n.ssid) {
                Ok(true) => {}
                Ok(false) => match here.networks.add(n) {
                    Ok(()) => added.push(n.name()),
                    Err(e) => notes.push(format!("Wi-Fi: “{}” couldn't be saved here ({e}).", n.name())),
                },
                Err(_) => {
                    notes.push("Wi-Fi: there's no NetworkManager here to save the networks in.".into());
                    break;
                }
            }
        }
        if !added.is_empty() {
            notes.push(format!("Wi-Fi: {} saved here, {}.", if added.len() == 1 { "a network".to_string() } else { format!("{} networks", added.len()) }, list(&added)));
        }
        if !w.left.is_empty() {
            let (what, them) = if w.left.len() == 1 { ("an enterprise network", "it") } else { ("enterprise networks", "them") };
            notes.push(format!("Wi-Fi: {} didn't come along ({what}, set up with the other computer's certificates): join {them} again here.", list(&w.left)));
        }
    }
    if elsewhere {
        if written > 0 && !skip.contains(&"config/pool.vault") && target(roots, Path::new("config/pool.vault")).is_some_and(|p| p.exists()) && has_secret(&secrets::POOL_KEY) {
            if let Some(marker) = target(roots, Path::new(ARRIVED)) {
                write_atomic(&marker, b"{}\n", 0o600).map_err(|e| e.to_string())?;
            }
            notes.push("Token Pool: its accounts came in paused. Turn them on here once the other computer's pool no longer uses them: each sign-in renews itself, and two computers renewing one sign each other out.".into());
        }
        notes.push(if wifi.is_some() {
            "Bluetooth devices and Touch ID fingerprints belong to each computer: pair and enrol them again here.".into()
        } else {
            "Bluetooth devices, Wi-Fi networks and Touch ID fingerprints belong to each computer: pair, join and enrol them again here.".into()
        });
        if settings.as_ref().is_some_and(|s| s.keyboard.profile.platform == olo_protocol::shortcuts::Platform::Mac) {
            notes.push("Keyboard: the Mac profile came along, ⌘ on the Super key (the Windows key on most PC keyboards). Preferences ▸ Keyboard picks another for this keyboard.".into());
        }
    }
    Ok(Imported { manifest, files: written, secrets: stored, before: before.map(|(p, _)| p), settings, notes })
}

/// A, B and C (unquoted).
fn list_plain(names: &[String]) -> String {
    match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// "A, B and C".
fn list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("“{n}”")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn write_atomic(dest: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = dest.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.partial", dest.file_name().unwrap_or_default().to_string_lossy()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
    std::fs::rename(&tmp, dest)
}

/// Keep only the newest automatic backups (and the keys of those gone).
fn prune(dir: &Path, keyring: &dyn Keyring) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(EXTENSION) || e.file_name().to_string_lossy().ends_with(OLD_EXTENSION))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    all.sort();
    let extra = all.len().saturating_sub(KEEP);
    for (_, p) in all.into_iter().take(extra) {
        if let Ok(Some(s)) = inspect(&p).map(|m| m.sealed)
            && s.with == "keyring"
        {
            let _ = keyring.delete(&backup_key_attrs(&s.salt));
        }
        let _ = std::fs::remove_file(p);
    }
}

/// A size as people read it.
pub fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1000 => format!("{b} bytes"),
        b if b < 1_000_000 => format!("{:.1} KB", b as f64 / 1e3),
        b => format!("{:.1} MB", b as f64 / 1e6),
    }
}

// ---- The panel ----

/// What the worker reports.
enum Report {
    Progress(u64, u64),
    Exported(PathBuf, Result<Manifest, String>),
    Inspected(PathBuf, Result<Manifest, String>),
    Imported(Result<Box<Imported>, String>),
}

enum Phase {
    Idle,
    Working {
        /// An export (its box shows the progress), else an import.
        export: bool,
        done: u64,
        total: u64,
        cancel: Arc<AtomicBool>,
        cancellable: bool,
    },
}

/// How many rows the Import box's list shows.
const WELL_ROWS: f32 = 7.0;
/// How many rows the Export box's list of what goes in shows.
const PARTS_ROWS: f32 = 5.0;
/// A part's row: a check box and room to the next (`TIGHT`; a list
/// row apart, check boxes crowd).
const PART_ROW: f32 = m::MARK_ROW + m::TIGHT;

/// This computer's parts, as an export holds them (`parts_here`).
type Parts = Vec<(String, usize, u64)>;

pub struct State {
    phase: Phase,
    rx: Option<Receiver<Report>>,
    /// The backup chosen to import, and what it holds.
    chosen: Option<(PathBuf, Manifest)>,
    /// How the last export went.
    exported: Option<(bool, String)>,
    /// How the last import (or reading a backup) went, and what it left
    /// behind or did differently.
    imported: Option<(bool, String)>,
    notes: Vec<String>,
    /// The settings as they were before the last import.
    before: Option<PathBuf>,
    /// Passwords and sign-ins go in the export, sealed with the password
    /// typed twice.
    sealed: bool,
    password: Entity<TextField>,
    again: Entity<TextField>,
    /// A sealed backup's password, to import it.
    unlock: Entity<TextField>,
    proxy: olo_ui::Proxy,
    scroll: ScrollHandle,
    /// What an export holds, part by part (read on a thread as the panel
    /// opens, and after each export and import), and those left out.
    parts: Option<Parts>,
    parts_rx: Option<Receiver<Parts>>,
    export_choice: Choice,
    export_scroll: ScrollHandle,
    /// The parts of the backup chosen that come back.
    import_choice: Choice,
}

/// A backup opened from Tracker, for the panel to show (`open_later`):
/// its window may be made after it's asked.
static OPENED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Show what `path` holds, ready to import, in the Backup panel: the one
/// open now (`take_opened`), or the next made.
pub fn open_later(path: PathBuf) {
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

impl State {
    /// The backup opened from Tracker, if one waits: read it now.
    pub fn take_opened(&mut self) {
        if let Some(path) = OPENED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.inspect(path);
        }
    }

    pub fn new(cx: &mut Context<PanelView>) -> State {
        let field = |cx: &mut Context<PanelView>| {
            let f = olo_ui::widgets::text_field(cx, |f| f.password(true));
            // (The buttons it enables, redrawn as it's typed in.)
            cx.subscribe(&f, |_, _, _: &olo_ui::widgets::TextFieldEvent, cx| cx.notify()).detach();
            f
        };
        let mut st = State {
            phase: Phase::Idle,
            rx: None,
            chosen: None,
            exported: None,
            imported: None,
            notes: Vec::new(),
            before: None,
            sealed: true,
            password: field(cx),
            again: field(cx),
            unlock: field(cx),
            proxy: olo_ui::proxy(cx),
            scroll: ScrollHandle::new(),
            parts: None,
            parts_rx: None,
            export_choice: Choice::all(),
            export_scroll: ScrollHandle::new(),
            import_choice: Choice::all(),
        };
        st.read_parts();
        st.take_opened();
        st
    }

    /// This computer's parts, read again on a thread (it walks the
    /// settings' folders).
    fn read_parts(&mut self) {
        let (tx, rx) = channel();
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("backup parts".into()).spawn(move || {
            let _ = tx.send(parts_here(&roots()));
            proxy.send(Msg::Redraw);
        });
        if spawned.is_ok() {
            self.parts_rx = Some(rx);
        }
    }

    /// An export of what's chosen holds something.
    fn export_has_some(&self) -> bool {
        self.parts.as_ref().is_some_and(|parts| parts.iter().any(|(p, files, _)| self.export_choice.has(p) && (*files > 0 || self.sealed)))
    }

    fn work(&mut self, export: bool, cancellable: bool, job: impl FnOnce(&dyn Fn(Report), &AtomicBool) + Send + 'static) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("backup".into())
            .spawn(move || {
                // Progress drawn at most twenty times a second.
                let last = std::cell::Cell::new(None::<std::time::Instant>);
                let send = move |r: Report| {
                    let progress = matches!(r, Report::Progress(..));
                    let _ = tx.send(r);
                    if !progress || last.get().is_none_or(|t| t.elapsed() > Duration::from_millis(50)) {
                        last.set(Some(std::time::Instant::now()));
                        proxy.send(Msg::Redraw);
                    }
                };
                job(&send, &flag);
            })
            .expect("thread");
        self.rx = Some(rx);
        if export {
            self.exported = None;
        } else {
            self.imported = None;
            self.notes.clear();
            self.before = None;
        }
        self.phase = Phase::Working { export, done: 0, total: 0, cancel, cancellable };
    }

    /// The file panel chose where to export to.
    pub fn export_to(&mut self, dest: PathBuf, cx: &mut Context<PanelView>) {
        let how = if self.sealed { Seal::Passphrase(Zeroizing::new(self.password.read(cx).text().to_owned())) } else { Seal::Open };
        for f in [&self.password, &self.again] {
            f.update(cx, |f, cx| f.set_text("", cx));
        }
        let choice = self.export_choice.clone();
        self.work(true, true, move |send, cancel| {
            let r = export_only(&roots(), &dest, &how, &Here::this(), &choice, &|d, t| send(Report::Progress(d, t)), cancel);
            send(Report::Exported(dest, r));
        });
    }

    /// A backup chosen to import (the file panel, or opened from
    /// Tracker): read what it holds.
    pub fn inspect(&mut self, src: PathBuf) {
        if matches!(self.phase, Phase::Working { .. }) {
            return;
        }
        self.chosen = None;
        self.work(false, false, move |send, _| {
            let r = inspect(&src);
            send(Report::Inspected(src, r));
        });
    }

    fn import(&mut self, cx: &mut Context<PanelView>) {
        let Some((src, _)) = self.chosen.clone() else { return };
        let words = Zeroizing::new(self.unlock.read(cx).text().to_owned());
        self.unlock.update(cx, |f, cx| f.set_text("", cx));
        let choice = self.import_choice.clone();
        self.work(false, true, move |send, cancel| {
            let words = Some(words.as_str()).filter(|w| !w.is_empty());
            let r = import_only(&roots(), &src, words, &Here::this(), &choice, &|d, t| send(Report::Progress(d, t)), cancel);
            send(Report::Imported(r.map(Box::new)));
        });
    }

    /// Take in what the worker reported; app_server's settings from an
    /// import, when there were some.
    fn poll(&mut self) -> Option<Settings> {
        let mut settings = None;
        if let Some(parts) = self.parts_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.parts = Some(parts);
            self.parts_rx = None;
        }
        while let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match r {
                Report::Progress(d, t) => {
                    if let Phase::Working { done, total, .. } = &mut self.phase {
                        (*done, *total) = (d, t);
                    }
                }
                Report::Exported(dest, r) => {
                    self.phase = Phase::Idle;
                    self.read_parts();
                    self.exported = Some(match r {
                        Ok(m) if m.sealed.is_some() => (true, format!("Saved “{}”, sealed.", name_of(&dest))),
                        Ok(_) => (true, format!("Saved “{}”.", name_of(&dest))),
                        Err(e) => (false, format!("Not saved: {e}")),
                    });
                }
                Report::Inspected(src, r) => {
                    self.phase = Phase::Idle;
                    match r {
                        Ok(m) => {
                            // (Everything it holds, until some is left out.)
                            self.import_choice = Choice::all();
                            self.chosen = Some((src, m));
                        }
                        Err(e) => self.imported = Some((false, format!("“{}” can't be imported. {e}", name_of(&src)))),
                    }
                }
                Report::Imported(r) => {
                    self.phase = Phase::Idle;
                    self.read_parts();
                    match r {
                        Ok(i) => {
                            settings = i.settings;
                            self.chosen = None;
                            self.before = i.before.clone();
                            self.notes = i.notes.clone();
                            let undo = match &i.before {
                                Some(p) => format!(" Your settings as they were are in “{}”.", name_of(p)),
                                None => String::new(),
                            };
                            let secrets = if i.secrets > 0 { format!(" and {} {}", i.secrets, if i.secrets == 1 { "password or sign-in" } else { "passwords and sign-ins" }) } else { String::new() };
                            // (Which parts, when some were left out.)
                            let parts: Vec<String> = i.manifest.parts().into_iter().map(|p| p.0).collect();
                            let chosen: Vec<String> = parts.iter().filter(|p| self.import_choice.has(p)).cloned().collect();
                            let of = if chosen.len() < parts.len() { format!(" of {}", list_plain(&chosen)) } else { String::new() };
                            let files = if i.files == 1 { "1 file".to_string() } else { format!("{} files", i.files) };
                            self.imported = Some((true, format!("Imported {files}{of}{secrets}. Apps that are open take some settings when they next start.{undo}")));
                        }
                        // (Still chosen: a wrong password can be typed again.)
                        Err(e) => self.imported = Some((false, format!("Not imported: {e}"))),
                    }
                }
            }
        }
        settings
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Backup(st) => Some(st),
        _ => None,
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The name an export is offered under.
pub fn suggested_name() -> String {
    format!("Olo Settings {}.{EXTENSION}", jiff::Zoned::now().strftime("%Y-%m-%d"))
}

/// A file panel for the backup (to export to, or to import), its answer
/// to this panel's window.
fn ask(ui: &Ui, export: bool) -> impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + 'static {
    let view = ui.cx.entity().downgrade();
    move |_, cx| {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let (title, panel) =
            if export { ("Export Settings", FilePanel::save(home, suggested_name()).types(&[EXTENSION])) } else { ("Import Settings", FilePanel::open(home).types(&[EXTENSION, OLD_EXTENSION])) };
        let view = view.clone();
        file_panel::show(cx, title, panel, move |a, cx| {
            let PanelResult::Chosen(path) = a.result else { return };
            let _ = view.update(cx, |v, cx| {
                if let Some(st) = state_of(v) {
                    if export {
                        st.export_to(path, cx);
                    } else {
                        st.inspect(path);
                    }
                }
                cx.notify();
            });
        });
    }
}

/// A line of text cut short to fit (an ellipsis), in `color`.
fn line(text: String, color: olo_ui::gpui::Hsla) -> impl olo_ui::gpui::IntoElement {
    canvas(
        |_, _, _| {},
        move |r, _, window, cx| {
            let font = style::ui_font(FontWeight::NORMAL);
            let shown = style::fit(window, text.clone().into(), style::PLAIN_SIZE, &font, r.size.width);
            style::label(window, cx, r.left(), r.center().y, style::Align::Left, shown, style::PLAIN_SIZE, font, color);
        },
    )
    .w_full()
    .h(px(m::CONTROL_HEIGHT))
}

/// Progress, and Cancel if it can be.
fn progress(done: u64, total: u64, cancel: &Arc<AtomicBool>, cancellable: bool) -> olo_ui::gpui::Div {
    let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
    let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StatusBar::new(fraction)));
    if cancellable {
        let cancel = cancel.clone();
        let stopping = cancel.load(Ordering::Relaxed);
        row = row.child(Button::new("Cancel").enabled(!stopping).on_click(move |_, _| cancel.store(true, Ordering::Relaxed)));
    }
    row
}

/// A part of a backup in a list: its check box, and what it holds on the
/// right (small, dim).
fn part_row(id: String, part: String, detail: String, on: bool, enabled: bool, toggle: impl Fn(bool, &mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + 'static) -> olo_ui::gpui::AnyElement {
    let detail = canvas(
        |_, _, _| {},
        move |r, _, window, cx| {
            let p = theme::palette(cx);
            style::label(window, cx, r.right(), r.center().y, style::Align::Right, detail.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
        },
    )
    .w(px(140.0))
    .h_full()
    .flex_shrink_0();
    // An item of a list to agents (and so, being what's in it on this
    // computer, not one of the panel's settings to the palette's index).
    let name = part.clone();
    let item = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            // (As much of it as shows: scrolled out of the list, none.)
            let shown = b.intersect(&window.content_mask().bounds);
            if shown.size.height > px(0.0) {
                olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "ListItem", name: Some(name.clone().into()), bounds: shown, enabled: true, ..Default::default() });
            }
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .child(item)
        .flex()
        .flex_row()
        .items_center()
        .h(px(PART_ROW))
        .pl(px(m::TEXT_INSET))
        .pr(px(m::SPACING))
        .child(div().flex_1().min_w_0().child(CheckBox::new(part, on).id(id).enabled(enabled).on_toggle(toggle)))
        .child(detail)
        .into_any_element()
}

/// The rows of `PART_ROW` a list scrolled by `scroll` shows (its height,
/// or `rows` of them before it's laid out): only those are built, the
/// others are room (a control scrolled out of sight would still say
/// it's there, where it isn't, to agents).
fn shown_rows(scroll: &ScrollHandle, rows: f32) -> std::ops::Range<usize> {
    let top = (-f32::from(scroll.offset().y)).max(0.0);
    let height = match f32::from(scroll.bounds().size.height) {
        h if h > 0.0 => h,
        _ => rows * PART_ROW + 4.0,
    };
    // (Rows from 2 points down; those reaching inside the 2 points of
    // padding at each end.)
    (top / PART_ROW).floor() as usize..((top + height - 4.0) / PART_ROW).ceil().max(0.0) as usize
}

/// Select All and Select None over a list of parts (`id` tells the two
/// lists' apart): all of `parts` in, or none; each enabled while it would
/// change something.
fn all_or_none(ui: &Ui, id: &str, parts: Vec<String>, choice: &Choice, enabled: bool, set: fn(&mut State) -> &mut Choice) -> olo_ui::gpui::AnyElement {
    let some_out = parts.iter().any(|p| !choice.has(p));
    let some_in = parts.iter().any(|p| choice.has(p));
    let all = ui.on_click_view(move |v, _, cx| {
        if let Some(st) = state_of(v) {
            *set(st) = Choice::all();
        }
        cx.notify();
    });
    let none = ui.on_click_view(move |v, _, cx| {
        if let Some(st) = state_of(v) {
            set(st).left_out = parts.iter().cloned().collect();
        }
        cx.notify();
    });
    div()
        .flex()
        .flex_row()
        .justify_end()
        .gap(px(m::SPACING))
        .child(olo_ui::widgets::LinkButton::new("Select All").id(format!("{id}-all")).enabled(enabled && some_out).on_click(all))
        .child(olo_ui::widgets::LinkButton::new("Select None").id(format!("{id}-none")).enabled(enabled && some_in).on_click(none))
        .into_any_element()
}

/// "3 files, 12 KB", or what a part with none is.
fn part_detail(files: usize, bytes: u64, none: &str) -> String {
    if files == 0 { none.to_owned() } else { format!("{} {}, {}", files, if files == 1 { "file" } else { "files" }, size_text(bytes)) }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    if let Some(s) = st.poll() {
        // app_server keeps its own copy: hand it the backup's.
        let prefs = ui.prefs();
        ui.cx.defer(move |cx| {
            let _ = prefs.update(cx, |p, cx| p.edit(cx, |now| *now = s));
        });
    }
    let p = theme::palette(ui.cx);
    let idle = matches!(st.phase, Phase::Idle);
    let (export_it, choose_it) = (ask(ui, true), ask(ui, false));
    let divider = crate::form::divider_for(ui.window, &["Password:", "Again:"]);
    let (password, again) = (st.password.read(ui.cx).text().to_owned(), st.again.read(ui.cx).text().to_owned());
    let sealed = st.sealed;
    let ready = !sealed || (!password.is_empty() && password == again);
    let ink = |ok: bool| if ok { c(p.panel_text) } else { c(p.failure) };

    // Side by side: one over the other, the panel was taller than a
    // 1280×800 screen once the lists had Select All and None.
    ui.columns(
        |ui| {
            ui.bbox("Export", |ui| {
                ui.note("Saves Olo's settings and what apps keep between runs in one file, to bring back here or to another computer. What goes in:");
                // A check box for each part (passwords, sign-ins and Wi-Fi
                // networks only in a sealed one).
                let mut rows: Vec<olo_ui::gpui::AnyElement> = Vec::new();
                let shown = shown_rows(&st.export_scroll, PARTS_ROWS);
                for (i, (part, files, bytes)) in st.parts.clone().unwrap_or_default().into_iter().enumerate() {
                    if !shown.contains(&i) {
                        rows.push(div().h(px(PART_ROW)).into_any_element());
                        continue;
                    }
                    let toggle = ui.on_view({
                        let part = part.clone();
                        move |v, on: bool, _, cx| {
                            if let Some(st) = state_of(v) {
                                st.export_choice.set(&part, on);
                            }
                            cx.notify();
                        }
                    });
                    let usable = files > 0 || sealed;
                    let detail = part_detail(files, bytes, if sealed { "sealed" } else { "sealed only" });
                    rows.push(part_row(format!("export-part-{part}"), part.clone(), detail, usable && st.export_choice.has(&part), idle && usable, toggle));
                }
                if st.parts.is_none() {
                    rows.push(div().pl(px(m::TEXT_INSET)).py(px(m::STACK / 2.0)).child(StringView::new("Looking…").color(style::disabled_text(&p))).into_any_element());
                }
                // (Those a backup that isn't sealed can't hold aren't counted.)
                let usable: Vec<String> = st.parts.iter().flatten().filter(|(_, files, _)| *files > 0 || sealed).map(|(p, ..)| p.clone()).collect();
                let links = all_or_none(ui, "export-parts", usable, &st.export_choice, idle && st.parts.is_some(), |st| &mut st.export_choice);
                ui.add(links);
                ui.add(
                    div()
                        .h(px(PARTS_ROWS * PART_ROW + 4.0))
                        .child(ScrollView::new("backup-parts", &st.export_scroll).background(p.document_background).auto_hide(true).child(div().flex().flex_col().py(px(2.0)).children(rows))),
                );
                let toggle = ui.on_view(|v, on: bool, _, cx| {
                    if let Some(st) = state_of(v) {
                        st.sealed = on;
                    }
                    cx.notify();
                });
                ui.add(CheckBox::new("Include passwords and sign-ins, sealed with a password", sealed).enabled(idle).on_toggle(toggle));
                ui.enabled_if(idle && sealed, |ui| {
                    ui.add(TextControl::new(st.password.clone()).label("Password:").divider(divider));
                    ui.add(TextControl::new(st.again.clone()).label("Again:").divider(divider));
                });
                // How it's going, or went, beside the button (the box keeps its
                // height).
                let status = match &st.phase {
                    Phase::Working { export: true, done, total, cancel, cancellable } => progress(*done, *total, cancel, *cancellable).into_any_element(),
                    _ if sealed && !again.is_empty() && password != again => line("The two passwords aren't the same.".into(), c(p.failure)).into_any_element(),
                    _ => match &st.exported {
                        Some((ok, text)) => line(text.clone(), ink(*ok)).into_any_element(),
                        None => div().into_any_element(),
                    },
                };
                let export = Button::new("Export…").enabled(idle && ready && st.export_has_some()).on_click(export_it);
                ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(status)).child(export));
            });
        },
        |ui| {
            ui.bbox("Import", |ui| {
                ui.note("Puts what you choose from a backup in place of what's here; the rest stays as it is. What you have now is saved first.");
                // What the backup chosen holds; else what the last import did.
                let mut rows: Vec<olo_ui::gpui::AnyElement> = Vec::new();
                // (Paragraphs a `STACK` apart, as notes are from what follows, and
                // clear of the scroll bar.)
                let text = |t: String, color| div().pl(px(m::TEXT_INSET)).pr(px(m::SPACING)).py(px(m::STACK / 2.0)).child(StringView::new(t).wrap(true).color(color)).into_any_element();
                if let Some((src, manifest)) = &st.chosen {
                    let from = if manifest.from_elsewhere(&Origin::here()) { ", on another computer" } else { "" };
                    ui.add(StringView::new(format!("“{}”, made {}{from}. What's checked comes back:", name_of(src), manifest.created_text())).wrap(true));
                    let parts: Vec<String> = manifest.parts().into_iter().map(|p| p.0).collect();
                    let links = all_or_none(ui, "import-parts", parts, &st.import_choice, idle, |st| &mut st.import_choice);
                    ui.add(links);
                    let shown = shown_rows(&st.scroll, WELL_ROWS * m::LIST_ROW / PART_ROW);
                    for (i, (part, files, bytes)) in manifest.parts().into_iter().enumerate() {
                        if !shown.contains(&i) {
                            rows.push(div().h(px(PART_ROW)).into_any_element());
                            continue;
                        }
                        let toggle = ui.on_view({
                            let part = part.clone();
                            move |v, on: bool, _, cx| {
                                if let Some(st) = state_of(v) {
                                    st.import_choice.set(&part, on);
                                }
                                cx.notify();
                            }
                        });
                        rows.push(part_row(format!("import-part-{part}"), part.clone(), part_detail(files, bytes, "sealed"), st.import_choice.has(&part), idle, toggle));
                    }
                }
                if let Some((ok, said)) = &st.imported {
                    rows.push(text(said.clone(), if *ok { c(p.document_text) } else { c(p.failure) }));
                    for note in &st.notes {
                        rows.push(text(format!("• {note}"), c(p.document_text)));
                    }
                }
                if rows.is_empty() {
                    rows.push(text("Choose a backup to see what it holds before importing it.".into(), style::disabled_text(&p)));
                }
                ui.add(
                    div()
                        .h(px(WELL_ROWS * m::LIST_ROW + 4.0))
                        .child(ScrollView::new("backup-well", &st.scroll).background(p.document_background).auto_hide(true).child(div().flex().flex_col().py(px(2.0)).children(rows))),
                );
                let locked = st.chosen.as_ref().is_some_and(|(_, m)| m.needs_passphrase());
                ui.enabled_if(idle && locked, |ui| ui.add(TextControl::new(st.unlock.clone()).label("Password:").divider(divider)));
                let some = st.chosen.as_ref().is_some_and(|(_, m)| m.parts().iter().any(|(part, ..)| st.import_choice.has(part)));
                let can = idle && some && (!locked || !st.unlock.read(ui.cx).text().is_empty());
                let status = match &st.phase {
                    Phase::Working { export: false, done, total, cancel, cancellable } => progress(*done, *total, cancel, *cancellable).into_any_element(),
                    _ => match st.before.clone() {
                        Some(before) if st.chosen.is_none() => Button::new(olo_ui::tracker::LABEL)
                            .on_click(move |_, _| {
                                if let Err(e) = olo_ui::tracker::open(&before) {
                                    tracing::warn!("couldn't open Tracker: {e}");
                                }
                            })
                            .into_any_element(),
                        _ => div().into_any_element(),
                    },
                };
                let go = ui.on_click_view(move |v, _, cx| {
                    if let Some(st) = state_of(v) {
                        st.import(cx);
                    }
                    cx.notify();
                });
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().child(status))
                        .child(Button::new("Choose…").enabled(idle).on_click(choose_it))
                        .child(Button::new("Import").default(st.chosen.is_some()).enabled(can).on_click(go)),
                );
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    /// A keyring of the test's own.
    #[derive(Default)]
    struct Memory(Mutex<HashMap<Vec<(String, String)>, (String, Vec<u8>)>>);

    fn key_of(attrs: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut k: Vec<(String, String)> = attrs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        k.sort();
        k
    }

    impl Keyring for Memory {
        fn available(&self) -> bool {
            true
        }
        fn lookup(&self, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().unwrap().get(&key_of(attributes)).map(|(_, v)| v.clone()))
        }
        fn store(&self, label: &str, attributes: &[(&str, &str)], secret: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().insert(key_of(attributes), (label.into(), secret.to_vec()));
            Ok(())
        }
        fn delete(&self, attributes: &[(&str, &str)]) -> Result<(), String> {
            self.0.lock().unwrap().remove(&key_of(attributes));
            Ok(())
        }
    }

    impl Memory {
        fn get(&self, k: secrets::Kept) -> Option<Vec<u8>> {
            self.lookup(k.attributes).unwrap()
        }
        fn put(&self, k: secrets::Kept, v: &[u8]) {
            self.store(k.label, k.attributes, v).unwrap();
        }
    }

    /// No keyring at all (as with none on the bus).
    struct None_;

    impl Keyring for None_ {
        fn available(&self) -> bool {
            false
        }
        fn lookup(&self, _: &[(&str, &str)]) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn store(&self, _: &str, _: &[(&str, &str)], _: &[u8]) -> Result<(), String> {
            Err("there is no keyring".into())
        }
        fn delete(&self, _: &[(&str, &str)]) -> Result<(), String> {
            Ok(())
        }
    }

    fn dirs(tag: &str) -> (PathBuf, Vec<(&'static str, PathBuf)>) {
        // (On disk, not in /tmp, which is memory.)
        let base = PathBuf::from("/var/tmp").join(format!("olo-backup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        (base.clone(), vec![("config", base.join("config")), ("state", base.join("state"))])
    }

    /// No NetworkManager.
    struct NoNets;

    impl Networks for NoNets {
        fn saved(&self) -> Result<(Vec<SavedWifi>, Vec<String>), String> {
            Err("no NetworkManager".into())
        }
        fn has(&self, _: &[u8]) -> Result<bool, String> {
            Err("no NetworkManager".into())
        }
        fn add(&self, _: &SavedWifi) -> Result<(), String> {
            Err("no NetworkManager".into())
        }
    }

    /// Saved networks of the test's own.
    #[derive(Default)]
    struct Nets(Mutex<Vec<SavedWifi>>, Vec<String>);

    impl Networks for Nets {
        fn saved(&self) -> Result<(Vec<SavedWifi>, Vec<String>), String> {
            Ok((self.0.lock().unwrap().clone(), self.1.clone()))
        }
        fn has(&self, ssid: &[u8]) -> Result<bool, String> {
            Ok(self.0.lock().unwrap().iter().any(|n| n.ssid == ssid))
        }
        fn add(&self, network: &SavedWifi) -> Result<(), String> {
            self.0.lock().unwrap().push(network.clone());
            Ok(())
        }
    }

    fn wifi(name: &str, psk: Option<&str>) -> SavedWifi {
        SavedWifi {
            id: name.into(),
            ssid: name.as_bytes().to_vec(),
            hidden: false,
            autoconnect: true,
            key_mgmt: if psk.is_some() { "wpa-psk".into() } else { String::new() },
            psk: psk.map(Into::into),
        }
    }

    /// This computer for a test: its keyring, no networks, `origin`.
    fn on<'a>(keyring: &'a dyn Keyring, origin: &Origin) -> Here<'a> {
        Here { keyring, networks: &NoNets, origin: origin.clone() }
    }

    fn here(home: &str, computer: &str) -> Origin {
        Origin { home: home.into(), computer: computer.into(), arch: "aarch64".into() }
    }

    fn words(w: &str) -> Seal {
        Seal::Passphrase(Zeroizing::new(w.into()))
    }

    #[test]
    fn export_and_import_round_trip() {
        let (base, roots) = dirs("trip");
        let (cfg, state) = (roots[0].1.clone(), roots[1].1.clone());
        std::fs::create_dir_all(cfg.join("olo")).unwrap();
        std::fs::create_dir_all(state.join("notifications")).unwrap();
        std::fs::create_dir_all(state.join("screens/agent/home")).unwrap();
        std::fs::write(cfg.join("terminal.json"), b"{\"font\":12}").unwrap();
        std::fs::write(cfg.join("olo/palette.json"), b"[]").unwrap();
        write_atomic(&cfg.join("passwords.vault"), b"sealed", 0o600).unwrap();
        std::fs::write(cfg.join(".x.json.saving"), b"half").unwrap();
        std::fs::write(state.join("notifications/history.json"), b"[1,2]").unwrap();
        std::fs::write(state.join("screens/agent/home/big"), b"an agent's desktop").unwrap();
        let file = base.join("b.olo-backup");
        let no = AtomicBool::new(false);
        let keyring = Memory::default();
        let me = here("/home/ben", "mac");
        let m = export(&roots, &file, &Seal::Open, &on(&keyring, &me), &|_, _| {}, &no).unwrap();
        let names: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["config/olo/palette.json", "config/passwords.vault", "config/terminal.json", "state/notifications/history.json"], "Agent Screens' homes are left out");
        assert_eq!(inspect(&file).unwrap(), m);
        assert!(m.sealed.is_none() && m.secrets.is_empty());
        let groups: Vec<String> = m.groups().into_iter().map(|g| g.0).collect();
        assert_eq!(groups, ["Notifications", "Palette", "Passwords", "Terminal"]);

        // Change things, then bring the backup back (on the same computer).
        std::fs::write(cfg.join("terminal.json"), b"{\"font\":20}").unwrap();
        std::fs::remove_file(cfg.join("passwords.vault")).unwrap();
        std::fs::write(cfg.join("new.json"), b"{}").unwrap();
        let i = import(&roots, &file, None, &on(&keyring, &me), &|_, _| {}, &no).unwrap();
        assert_eq!(i.files, 4);
        assert!(i.notes.is_empty(), "{:?}", i.notes);
        assert_eq!(std::fs::read(cfg.join("terminal.json")).unwrap(), b"{\"font\":12}");
        assert_eq!(std::fs::read(cfg.join("passwords.vault")).unwrap(), b"sealed");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(cfg.join("passwords.vault")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(cfg.join("new.json").exists(), "files the backup doesn't have are left alone");
        // What was there before was saved, sealed with a key in the
        // keyring, and isn't itself backed up.
        let before = inspect(i.before.as_ref().unwrap()).unwrap();
        assert!(before.files.iter().any(|f| f.path == "config/new.json"));
        assert_eq!(before.sealed.as_ref().unwrap().with, "keyring");
        assert!(collect(&roots).iter().all(|(n, _)| !n.starts_with("state/backups")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A sealed backup brings Passwords and the rest to another computer
    /// and person: the keyring's secrets with it, paths moved to the new
    /// home, Telegram's sign-in left behind, the pool's accounts paused.
    #[test]
    fn sealed_onto_another_computer() {
        let (base, mac) = dirs("mac");
        let (_, thinkpad) = dirs("thinkpad");
        let no = AtomicBool::new(false);
        let (old, new) = ("/home/ben", "/home/bfrank");
        std::fs::create_dir_all(&mac[0].1).unwrap();
        std::fs::write(mac[0].1.join("places.json"), format!("[\"{old}/src\",\"file://{old}/Pictures\",\"{old}\",\"{old}benjamin/x\",\"/srv{old}/y\"]")).unwrap();
        write_atomic(&mac[0].1.join("passwords.vault"), b"vault sealed with key A", 0o600).unwrap();
        write_atomic(&mac[0].1.join("pool.vault"), b"pool sealed with key P", 0o600).unwrap();
        let a = Memory::default();
        a.put(secrets::PASSWORDS_KEY, b"key A");
        a.put(secrets::POOL_KEY, b"key P");
        a.put(secrets::GITHUB, b"ghp_token");
        a.put(secrets::TELEGRAM_API, b"{\"id\":1}");
        a.put(secrets::TELEGRAM_SESSION, b"auth keys");
        let file = base.join("mac.olo-backup");
        let m = export(&mac, &file, &words("correct horse"), &on(&a, &here(old, "mac")), &|_, _| {}, &no).unwrap();
        assert_eq!(m.secrets, ["Passwords", "Token Pool", "GitHub", "Telegram", "Telegram sign-in"]);
        let raw = std::fs::read(&file).unwrap();
        for secret in [&b"key A"[..], b"ghp_token", b"auth keys", b"vault sealed"] {
            assert!(!raw.windows(secret.len()).any(|w| w == secret), "sealed: {}", String::from_utf8_lossy(secret));
        }
        assert!(m.needs_passphrase());

        // The ThinkPad has a vault of its own, and its own keyring.
        std::fs::create_dir_all(&thinkpad[0].1).unwrap();
        write_atomic(&thinkpad[0].1.join("passwords.vault"), b"empty vault, key B", 0o600).unwrap();
        let b = Memory::default();
        b.put(secrets::PASSWORDS_KEY, b"key B");
        let there = here(new, "thinkpad");
        assert!(m.from_elsewhere(&there));
        // A wrong password replaces nothing.
        let e = import(&thinkpad, &file, Some("wrong"), &on(&b, &there), &|_, _| {}, &no).unwrap_err();
        assert!(e.contains("password"), "{e}");
        assert_eq!(std::fs::read(thinkpad[0].1.join("passwords.vault")).unwrap(), b"empty vault, key B");
        assert_eq!(b.get(secrets::PASSWORDS_KEY).unwrap(), b"key B");
        assert!(import(&thinkpad, &file, None, &on(&b, &there), &|_, _| {}, &no).is_err(), "sealed: a password is needed");

        let i = import(&thinkpad, &file, Some("correct horse"), &on(&b, &there), &|_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(thinkpad[0].1.join("passwords.vault")).unwrap(), b"vault sealed with key A");
        assert_eq!(b.get(secrets::PASSWORDS_KEY).unwrap(), b"key A");
        assert_eq!(b.get(secrets::GITHUB).unwrap(), b"ghp_token");
        assert_eq!(b.get(secrets::TELEGRAM_API).unwrap(), b"{\"id\":1}");
        assert_eq!(b.get(secrets::TELEGRAM_SESSION), None, "each computer signs in to Telegram as itself");
        assert_eq!(i.secrets, 4);
        assert!(thinkpad[0].1.join("pool-arrived.json").exists(), "the pool's accounts come in paused");
        assert!(i.notes.iter().any(|n| n.starts_with("Telegram sign-in")), "{:?}", i.notes);
        assert!(i.notes.iter().any(|n| n.starts_with("Token Pool")), "{:?}", i.notes);
        let places = std::fs::read_to_string(thinkpad[0].1.join("places.json")).unwrap();
        assert_eq!(places, format!("[\"{new}/src\",\"file://{new}/Pictures\",\"{new}\",\"{old}benjamin/x\",\"/srv{old}/y\"]"));

        // Undone: the ThinkPad's own vault, and its key, back.
        let undo = i.before.unwrap();
        let back = import(&thinkpad, &undo, None, &on(&b, &there), &|_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(thinkpad[0].1.join("passwords.vault")).unwrap(), b"empty vault, key B");
        assert_eq!(b.get(secrets::PASSWORDS_KEY).unwrap(), b"key B");
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        // Its key opens it on this computer alone.
        assert!(import(&mac, &undo, None, &on(&a, &here(old, "mac")), &|_, _| {}, &no).is_err());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(thinkpad[0].1.parent().unwrap());
    }

    /// Back on the computer it came from, a sealed backup brings back
    /// Telegram's sign-in too, and the pool as it was.
    #[test]
    fn sealed_back_onto_the_same_computer() {
        let (base, roots) = dirs("same");
        let no = AtomicBool::new(false);
        std::fs::create_dir_all(&roots[0].1).unwrap();
        write_atomic(&roots[0].1.join("pool.vault"), b"pool", 0o600).unwrap();
        let k = Memory::default();
        k.put(secrets::TELEGRAM_SESSION, b"auth keys");
        k.put(secrets::POOL_KEY, b"key P");
        let me = here("/home/ben", "mac");
        let file = base.join("b.olo-backup");
        export(&roots, &file, &words("pw"), &on(&k, &me), &|_, _| {}, &no).unwrap();
        // The keyring lost them (a fresh login keyring, say).
        let fresh = Memory::default();
        let i = import(&roots, &file, Some("pw"), &on(&fresh, &me), &|_, _| {}, &no).unwrap();
        assert!(i.notes.is_empty(), "{:?}", i.notes);
        assert_eq!(fresh.get(secrets::TELEGRAM_SESSION).unwrap(), b"auth keys");
        assert_eq!(fresh.get(secrets::POOL_KEY).unwrap(), b"key P");
        assert!(!roots[0].1.join("pool-arrived.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An open backup has no keys: on another computer it leaves the
    /// vaults there alone, and says so.
    #[test]
    fn open_onto_another_computer_leaves_the_vaults() {
        let (base, mac) = dirs("open-mac");
        let (_, other) = dirs("open-other");
        let no = AtomicBool::new(false);
        std::fs::create_dir_all(&mac[0].1).unwrap();
        write_atomic(&mac[0].1.join("passwords.vault"), b"vault A", 0o600).unwrap();
        std::fs::write(mac[0].1.join("terminal.json"), b"{}").unwrap();
        let file = base.join("b.olo-backup");
        export(&mac, &file, &Seal::Open, &on(&Memory::default(), &here("/home/ben", "mac")), &|_, _| {}, &no).unwrap();
        std::fs::create_dir_all(&other[0].1).unwrap();
        write_atomic(&other[0].1.join("passwords.vault"), b"vault B", 0o600).unwrap();
        let i = import(&other, &file, None, &on(&Memory::default(), &here("/home/x", "other")), &|_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(other[0].1.join("passwords.vault")).unwrap(), b"vault B");
        assert!(other[0].1.join("terminal.json").exists());
        assert!(i.notes.iter().any(|n| n.starts_with("Passwords: left as it is")), "{:?}", i.notes);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(other[0].1.parent().unwrap());
    }

    /// With no keyring, what was there is saved open, and the keyring's
    /// secrets (none to read) aren't replaced.
    #[test]
    fn without_a_keyring() {
        let (base, roots) = dirs("nokeyring");
        let no = AtomicBool::new(false);
        std::fs::create_dir_all(&roots[0].1).unwrap();
        std::fs::write(roots[0].1.join("terminal.json"), b"{}").unwrap();
        let file = base.join("b.olo-backup");
        let me = here("/home/ben", "mac");
        export(&roots, &file, &Seal::Open, &on(&None_, &me), &|_, _| {}, &no).unwrap();
        let i = import(&roots, &file, None, &on(&None_, &me), &|_, _| {}, &no).unwrap();
        assert!(inspect(i.before.as_ref().unwrap()).unwrap().sealed.is_none());
        // A sealed one from another computer: its files, not its
        // secrets, and so not the vault they'd open.
        let k = Memory::default();
        k.put(secrets::PASSWORDS_KEY, b"key A");
        write_atomic(&roots[0].1.join("passwords.vault"), b"vault A", 0o600).unwrap();
        let sealed = base.join("s.olo-backup");
        export(&roots, &sealed, &words("pw"), &on(&k, &me), &|_, _| {}, &no).unwrap();
        write_atomic(&roots[0].1.join("passwords.vault"), b"vault here", 0o600).unwrap();
        let i = import(&roots, &sealed, Some("pw"), &on(&None_, &here("/home/x", "other")), &|_, _| {}, &no).unwrap();
        assert_eq!(std::fs::read(roots[0].1.join("passwords.vault")).unwrap(), b"vault here");
        assert!(i.notes.iter().any(|n| n.contains("no login keyring")), "{:?}", i.notes);
        assert!(export(&roots, &base.join("k.olo-backup"), &Seal::Keyring, &on(&None_, &me), &|_, _| {}, &no).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A sealed backup takes the saved Wi-Fi networks along, passwords and
    /// all: another computer saves those it hasn't, keeps its own, and
    /// hears which (enterprise ones) stayed behind. An open one, and the
    /// backup made before an import, read none (no password asked of
    /// NetworkManager for them).
    #[test]
    fn wifi_networks_come_along() {
        let (base, mac) = dirs("wifi-mac");
        let (_, other) = dirs("wifi-other");
        let no = AtomicBool::new(false);
        std::fs::create_dir_all(&mac[0].1).unwrap();
        std::fs::write(mac[0].1.join("terminal.json"), b"{}").unwrap();
        let theirs = Nets(Mutex::new(vec![wifi("Home", Some("hunter22")), wifi("Cafe", None), wifi("Office", Some("old"))]), vec!["eduroam".into()]);
        let k = Memory::default();
        let from = here("/home/ben", "mac");
        let file = base.join("b.olo-backup");
        let m = export(&mac, &file, &words("pw"), &Here { keyring: &k, networks: &theirs, origin: from.clone() }, &|_, _| {}, &no).unwrap();
        assert!(m.secrets.iter().any(|s| s == "Wi-Fi networks"));
        let raw = std::fs::read(&file).unwrap();
        for secret in [&b"hunter22"[..], b"eduroam"] {
            assert!(!raw.windows(secret.len()).any(|w| w == secret), "sealed: {}", String::from_utf8_lossy(secret));
        }
        let open = export(&mac, &base.join("o.olo-backup"), &Seal::Open, &Here { keyring: &k, networks: &theirs, origin: from.clone() }, &|_, _| {}, &no).unwrap();
        assert!(open.secrets.is_empty());

        std::fs::create_dir_all(&other[0].1).unwrap();
        std::fs::write(other[0].1.join("deskcalc.json"), b"{}").unwrap();
        let ours = Nets(Mutex::new(vec![wifi("Office", Some("new"))]), Vec::new());
        let k2 = Memory::default();
        let i = import(&other, &file, Some("pw"), &Here { keyring: &k2, networks: &ours, origin: here("/home/b", "thinkpad") }, &|_, _| {}, &no).unwrap();
        let now = ours.0.lock().unwrap().clone();
        assert_eq!(now.iter().map(|n| n.name()).collect::<Vec<_>>(), ["Office", "Home", "Cafe"]);
        assert_eq!(now[0].psk.as_deref(), Some("new"), "its own kept");
        assert_eq!(now[1].psk.as_deref(), Some("hunter22"));
        assert!(i.notes.iter().any(|n| n == "Wi-Fi: 2 networks saved here, “Home” and “Cafe”."), "{:?}", i.notes);
        assert!(i.notes.iter().any(|n| n.contains("“eduroam” didn't come along (an enterprise network")), "{:?}", i.notes);
        assert!(i.notes.iter().any(|n| n.starts_with("Bluetooth devices and Touch ID")), "{:?}", i.notes);
        // (The backup made first asked for no network's password.)
        assert!(inspect(i.before.as_ref().unwrap()).unwrap().secrets.iter().all(|s| s != "Wi-Fi networks"));
        // No NetworkManager there: said so.
        let i = import(&other, &file, Some("pw"), &on(&k2, &here("/home/b", "thinkpad")), &|_, _| {}, &no).unwrap();
        assert!(i.notes.iter().any(|n| n.contains("no NetworkManager")), "{:?}", i.notes);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(other[0].1.parent().unwrap());
    }

    /// A backup of everything brings back only what's chosen (Ben: "maybe
    /// i have an export with everything but i just want to include token
    /// pooler settings/accounts for my import"), the rest left as it is;
    /// and an export holds only what's chosen.
    #[test]
    fn only_the_parts_chosen() {
        let (base, mac) = dirs("parts-mac");
        let (_, other) = dirs("parts-other");
        let no = AtomicBool::new(false);
        std::fs::create_dir_all(&mac[0].1).unwrap();
        std::fs::write(mac[0].1.join("terminal.json"), b"{\"font\":14}").unwrap();
        std::fs::write(mac[0].1.join("pool.json"), b"{\"order\":1}").unwrap();
        write_atomic(&mac[0].1.join("pool.vault"), b"pool sealed with key P", 0o600).unwrap();
        write_atomic(&mac[0].1.join("passwords.vault"), b"vault A", 0o600).unwrap();
        let a = Memory::default();
        a.put(secrets::POOL_KEY, b"key P");
        a.put(secrets::PASSWORDS_KEY, b"key A");
        a.put(secrets::GITHUB, b"ghp_token");
        let theirs = Nets(Mutex::new(vec![wifi("Home", Some("hunter22"))]), Vec::new());
        let from = here("/home/ben", "mac");
        let file = base.join("all.olo-backup");
        let m = export(&mac, &file, &words("pw"), &Here { keyring: &a, networks: &theirs, origin: from.clone() }, &|_, _| {}, &no).unwrap();
        let parts: Vec<String> = m.parts().into_iter().map(|p| p.0).collect();
        assert_eq!(parts, ["GitHub", "Passwords", "Terminal", "Token Pool", WIFI_PART]);
        assert!(Choice::only(&["Nope".into()], &parts).unwrap_err().contains("“Nope”"));

        // Onto another computer: the Token Pool alone.
        std::fs::create_dir_all(&other[0].1).unwrap();
        std::fs::write(other[0].1.join("terminal.json"), b"{\"font\":11}").unwrap();
        write_atomic(&other[0].1.join("passwords.vault"), b"vault B", 0o600).unwrap();
        let b = Memory::default();
        b.put(secrets::PASSWORDS_KEY, b"key B");
        let ours = Nets::default();
        let only = Choice::only(&["token pool".into()], &parts).unwrap();
        let there = Here { keyring: &b, networks: &ours, origin: here("/home/b", "thinkpad") };
        let i = import_only(&other, &file, Some("pw"), &there, &only, &|_, _| {}, &no).unwrap();
        assert_eq!((i.files, i.secrets), (2, 1));
        assert_eq!(std::fs::read(other[0].1.join("pool.vault")).unwrap(), b"pool sealed with key P");
        assert_eq!(std::fs::read(other[0].1.join("pool.json")).unwrap(), b"{\"order\":1}");
        assert_eq!(b.get(secrets::POOL_KEY).unwrap(), b"key P");
        assert!(other[0].1.join("pool-arrived.json").exists(), "its accounts come in paused");
        // The rest as it was here.
        assert_eq!(std::fs::read(other[0].1.join("terminal.json")).unwrap(), b"{\"font\":11}");
        assert_eq!(std::fs::read(other[0].1.join("passwords.vault")).unwrap(), b"vault B");
        assert_eq!(b.get(secrets::PASSWORDS_KEY).unwrap(), b"key B");
        assert_eq!(b.get(secrets::GITHUB), None);
        assert!(ours.0.lock().unwrap().is_empty(), "no Wi-Fi network");
        // What was here, all of it, saved first.
        let before: Vec<String> = inspect(i.before.as_ref().unwrap()).unwrap().parts().into_iter().map(|p| p.0).collect();
        assert!(before.contains(&"Terminal".to_string()) && before.contains(&"Passwords".to_string()), "{before:?}");

        // An export of Terminal alone: nothing else, no secret, no network.
        let t =
            export_only(&mac, &base.join("t.olo-backup"), &words("pw"), &Here { keyring: &a, networks: &theirs, origin: from }, &Choice::only(&["Terminal".into()], &parts).unwrap(), &|_, _| {}, &no)
                .unwrap();
        assert_eq!(t.parts().into_iter().map(|p| p.0).collect::<Vec<_>>(), ["Terminal"]);
        let raw = std::fs::read(base.join("t.olo-backup")).unwrap();
        assert!(t.secrets.is_empty() && !raw.is_empty());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(other[0].1.parent().unwrap());
    }

    #[test]
    fn this_computers_parts() {
        let (base, roots) = dirs("parts-here");
        std::fs::create_dir_all(&roots[0].1).unwrap();
        std::fs::write(roots[0].1.join("terminal.json"), b"{}").unwrap();
        std::fs::write(roots[0].1.join("pool.json"), b"{}").unwrap();
        let parts = parts_here(&roots);
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        // Its apps' files, and what goes only in a sealed backup.
        assert_eq!(names, ["GitHub", "Passwords", "Telegram", "Terminal", "Token Pool", WIFI_PART]);
        assert_eq!(parts.iter().find(|p| p.0 == "Terminal").map(|p| (p.1, p.2)), Some((1, 2)));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_changed_seal_is_found() {
        let key = [7u8; 32];
        let plain: Vec<u8> = (0..(CHUNK * 2 + 100)).map(|i| i as u8).collect();
        let sealed = seal(&key, &plain);
        assert_eq!(unseal(&key, &sealed).unwrap().as_slice(), &plain[..]);
        assert!(unseal(&[8u8; 32], &sealed).is_err(), "another key");
        let mut changed = sealed.clone();
        changed[CHUNK + 100] ^= 1;
        assert!(unseal(&key, &changed).is_err(), "a byte changed");
        assert!(unseal(&key, &sealed[..21 + CHUNK + 16]).is_err(), "cut short at a piece's end");
        assert_eq!(unseal(&key, &seal(&key, b"")).unwrap().as_slice(), b"");
    }

    /// On another computer, displays known only by their port are
    /// another's: their settings stay behind; monitors' come along.
    #[test]
    fn displays_by_port_stay_behind() {
        let json = br#"{"display":{"scale":2.0,"main":"DP-1","outputs":{"eDP-1":{"scale":2.0},"Apple Computer Inc StudioDisplay 0x1D4440B7":{"scale":2.0}}},"tiling":{"modes":[{"workspace":3,"display":"eDP-1","mode":"auto_tiling"},{"workspace":2,"display":"Apple Computer Inc StudioDisplay 0x1D4440B7","mode":"auto_tiling"}]},"newer":{"kept":true}}"#;
        let v: serde_json::Value = serde_json::from_slice(&without_connectors(json).unwrap()).unwrap();
        assert!(v["display"]["main"].is_null());
        assert_eq!(v["display"]["outputs"].as_object().unwrap().keys().collect::<Vec<_>>(), ["Apple Computer Inc StudioDisplay 0x1D4440B7"]);
        assert_eq!(v["tiling"]["modes"].as_array().unwrap().len(), 1);
        assert_eq!(v["newer"]["kept"], true, "what a newer Olo saved comes along");
        assert!(serde_json::from_value::<Settings>(v).is_ok());
        assert_eq!(without_connectors(br#"{"display":{"main":"Dell Inc. DELL S2425HS 8K08534","outputs":{}}}"#), None);
    }

    #[test]
    fn moves_paths_to_the_new_home() {
        assert_eq!(rehome("\"/home/ben\"", "/home/ben", "/home/b").as_deref(), Some("\"/home/b\""));
        assert_eq!(rehome("cd /home/ben/src && ls", "/home/ben", "/home/b").as_deref(), Some("cd /home/b/src && ls"));
        assert_eq!(rehome("/home/benjamin /x/home/ben", "/home/ben", "/home/b"), None);
        assert_eq!(rehome("nothing here", "/home/ben", "/home/b"), None);
        assert_eq!(rehome("/home/ben", "/home/ben", "/home/ben"), None);
    }

    #[test]
    fn refuses_what_isnt_a_backup() {
        let (base, roots) = dirs("bad");
        std::fs::create_dir_all(&base).unwrap();
        let junk = base.join("junk.olo-backup");
        std::fs::write(&junk, b"not a backup").unwrap();
        assert!(inspect(&junk).is_err());
        // Names that would land outside the roots are not ours.
        assert_eq!(target(&roots, Path::new("config/../../etc/passwd")), None);
        assert_eq!(target(&roots, Path::new("/etc/passwd")), None);
        assert_eq!(target(&roots, Path::new("other/x.json")), None);
        assert_eq!(target(&roots, Path::new("config")), None);
        assert_eq!(target(&roots, Path::new("config/a/b.json")), Some(roots[0].1.join("a/b.json")));
        assert!(left_out("state/screens/agent/x") && left_out("state/backups") && !left_out("state/screensaver.json"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn names_apps() {
        assert_eq!(app_of("config/app_server.json"), "Desktop (Preferences)");
        assert_eq!(app_of("config/terminal.json"), "Terminal");
        assert_eq!(app_of("state/notifications/history.json"), "Notifications");
        assert_eq!(app_of("config/deskcalc.json"), "DeskCalc");
        assert_eq!(app_of("config/some-thing.json"), "Some thing");
    }
}
