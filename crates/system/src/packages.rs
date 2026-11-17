//! The system's packages, through PackageKit (`org.freedesktop.PackageKit`
//! on the system bus): what's installed, what's available and what
//! updates there are; installing and updating them, and installing a
//! package file. PackageKit does the work with the system's own package
//! manager (dnf on Fedora) and asks polkit; nothing is shelled out to.
//! Each call is one transaction, waited on (call it off the UI thread),
//! telling how it goes as it goes ([`Progress`]).

use std::collections::HashMap;

use zvariant::{DynamicType, OwnedObjectPath, OwnedValue};

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.PackageKit";
const PATH: &str = "/org/freedesktop/PackageKit";
const IFACE: &str = "org.freedesktop.PackageKit";
const TRANSACTION: &str = "org.freedesktop.PackageKit.Transaction";

/// Filters (a bit for each of PackageKit's `PkFilterEnum`).
const FILTER_NONE: u64 = 1 << 1;
const FILTER_INSTALLED: u64 = 1 << 2;
const FILTER_NOT_INSTALLED: u64 = 1 << 3;
const FILTER_NEWEST: u64 = 1 << 16;
/// Only packages from trusted (signed) sources.
const ONLY_TRUSTED: u64 = 1 << 1;
/// `PkInfoEnum`: installed.
const INFO_INSTALLED: u32 = 1;

/// A package: PackageKit's id ("name;version;arch;repository").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub installed: bool,
    pub summary: String,
}

impl Package {
    pub fn name(&self) -> &str {
        self.id.split(';').next().unwrap_or_default()
    }

    pub fn version(&self) -> &str {
        self.id.split(';').nth(1).unwrap_or_default()
    }
}

/// How a transaction is going.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Done so far, of 100.
    Percent(u32),
    /// What it's doing ("Downloading", "Installing").
    Status(&'static str),
    /// The package it's on.
    Package(String),
}

/// PackageKit's `PkStatusEnum`, said plainly (those worth saying).
fn status(n: u32) -> Option<&'static str> {
    Some(match n {
        2 => "Waiting",
        3 => "Setting up",
        5 => "Removing",
        6 => "Refreshing the package lists",
        8 => "Downloading",
        9 => "Installing",
        10 => "Updating",
        11 => "Cleaning up",
        13 => "Resolving dependencies",
        14 => "Checking signatures",
        16 => "Testing the changes",
        17 => "Committing the changes",
        24 => "Waiting for another package job",
        25 => "Waiting for the package manager",
        30 => "Loading the package lists",
        _ => return None,
    })
}

/// Run `method` on a new transaction and wait for it to finish: the
/// packages it told of, or what went wrong.
fn transaction<B>(method: &str, body: &B, progress: &mut dyn FnMut(Progress)) -> Result<Vec<Package>>
where
    B: serde::Serialize + DynamicType,
{
    let conn = bus::system()?;
    let path: OwnedObjectPath = bus::call(conn, DEST, PATH, IFACE, "CreateTransaction", &())?;
    // (Listening before it starts: its signals are only for its path.)
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(DEST)?.path(path.as_str())?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(1024))?;
    // A person may be asked (polkit) for an untrusted package.
    let _ = bus::call::<_, ()>(conn, DEST, path.as_str(), TRANSACTION, "SetHints", &(vec!["interactive=true".to_string()],));
    bus::call::<_, ()>(conn, DEST, path.as_str(), TRANSACTION, method, body)?;
    let mut packages = Vec::new();
    let mut error = None;
    for m in signals {
        let m = m?;
        let header = m.header();
        let Some(member) = header.member() else { continue };
        match member.as_str() {
            "Package" => {
                if let Ok((info, id, summary)) = m.body().deserialize::<(u32, String, String)>() {
                    progress(Progress::Package(id.split(';').next().unwrap_or_default().to_owned()));
                    packages.push(Package { id, installed: info == INFO_INSTALLED, summary });
                }
            }
            "ErrorCode" => {
                if let Ok((_, details)) = m.body().deserialize::<(u32, String)>() {
                    error = Some(details);
                }
            }
            "PropertiesChanged" => {
                if let Ok((_, changed, _)) = m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() {
                    if let Some(p) = changed.get("Percentage").and_then(|v| u32::try_from(v.clone()).ok()).filter(|p| *p <= 100) {
                        progress(Progress::Percent(p));
                    }
                    if let Some(s) = changed.get("Status").and_then(|v| u32::try_from(v.clone()).ok()).and_then(status) {
                        progress(Progress::Status(s));
                    }
                }
            }
            "Finished" | "Destroy" => break,
            _ => {}
        }
    }
    match error {
        Some(e) => Err(zbus::Error::Failure(e)),
        None => Ok(packages),
    }
}

/// Whether a package is for this computer (its architecture, or none).
fn native(p: &Package) -> bool {
    let arch = p.id.split(';').nth(2).unwrap_or("");
    arch == std::env::consts::ARCH || arch == "noarch"
}

/// Each package called one of `names`, for this computer's
/// architecture: the one installed if there is one, else the newest the
/// repositories have. (Asked for the newest alone, PackageKit gave an
/// update waiting as not installed, and other architectures' builds:
/// installing what was missing became updating, which asked for a
/// password, Olo Setup 2026-11-07.)
pub fn resolve(names: &[&str]) -> Result<Vec<Package>> {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let mut out: Vec<Package> = transaction("Resolve", &(FILTER_INSTALLED, names.clone()), &mut |_| {})?.into_iter().filter(native).collect();
    let rest: Vec<String> = names.into_iter().filter(|n| !out.iter().any(|p| p.name() == n)).collect();
    if !rest.is_empty() {
        for p in transaction("Resolve", &(FILTER_NEWEST | FILTER_NOT_INSTALLED, rest), &mut |_| {})?.into_iter().filter(native) {
            if !out.iter().any(|q| q.name() == p.name()) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Read the package lists again (a repository just added).
pub fn refresh(progress: &mut dyn FnMut(Progress)) -> Result<()> {
    transaction("RefreshCache", &(false,), progress).map(|_| ())
}

/// Install the packages called `names` (and what they need), from
/// trusted sources. Those already installed are left; none there: an
/// error naming them.
pub fn install(names: &[&str], progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let found = resolve(names)?;
    let missing: Vec<&str> = names.iter().copied().filter(|n| !found.iter().any(|p| p.name() == *n)).collect();
    if !missing.is_empty() {
        return Err(zbus::Error::Failure(format!("not in any repository: {}", missing.join(", "))));
    }
    let ids: Vec<String> = found.into_iter().filter(|p| !p.installed).map(|p| p.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    transaction("InstallPackages", &(ONLY_TRUSTED, ids), progress).map(|_| ())
}

/// Remove the installed packages called `names`, and what was installed
/// only for them (those not installed are passed over). Nothing else
/// that needs them is taken with them: PackageKit refuses then, and the
/// error says what would go.
pub fn remove(names: &[&str], progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let wanted: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let ids: Vec<String> = transaction("Resolve", &(FILTER_INSTALLED, wanted), &mut |_| {})?.into_iter().filter(native).filter(|p| p.installed).map(|p| p.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    // (No transaction flags; not what depends on them; what only they
    // needed goes too.)
    transaction("RemovePackages", &(0u64, ids, false, true), progress).map(|_| ())
}

/// Install a package file (`path`, readable by PackageKit): a
/// repository's release package, which isn't signed by a key the system
/// trusts yet.
pub fn install_file(path: &std::path::Path, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    transaction("InstallFiles", &(0u64, vec![path.to_string_lossy().into_owned()]), progress).map(|_| ())
}

/// The updates waiting.
pub fn updates() -> Result<Vec<Package>> {
    transaction("GetUpdates", &(FILTER_NONE,), &mut |_| {})
}

/// Update the packages with these ids (from [`updates`]).
pub fn update(ids: Vec<String>, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    transaction("UpdatePackages", &(ONLY_TRUSTED, ids), progress).map(|_| ())
}

/// The services Olo installs and updates software through: PackageKit
/// (Olo Setup, Doctor) and dnf's own (OS Updates, `crate::dnf`), each by
/// its package and its name on the system bus.
pub const SERVICES: &[(&str, &str)] = &[("PackageKit", DEST), ("dnf5daemon-server", "org.rpm.dnf.v0")];

/// Whether `name` can be asked on the system bus: running, or started
/// when asked (its activation file there).
fn on_the_bus(name: &str) -> bool {
    let Ok(conn) = bus::system() else { return false };
    let has = |method: &str| -> Vec<String> { bus::call(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", method, &()).unwrap_or_default() };
    has("ListActivatableNames").iter().chain(has("ListNames").iter()).any(|n| n == name)
}

/// The services of [`SERVICES`] that aren't here, by package.
pub fn missing_services() -> Vec<&'static str> {
    SERVICES.iter().filter(|(_, name)| !on_the_bus(name)).map(|(package, _)| *package).collect()
}

/// PackageKit and dnf's service put in place where they aren't (root): a
/// minimal Fedora has neither, and nothing else on the bus installs
/// software, so this one step runs Fedora's own dnf (Ben, 2026-11-15:
/// "just have olo installer and olo doctor install the needed
/// packagekit/dnf stuff", choosing one dnf call over a package manager
/// of Olo's own). Everything after goes through the services. Then the
/// bus is told to read its activation files again, and the services
/// waited for (ten seconds at most). What dnf says goes to `progress`
/// line by line.
pub fn bootstrap(progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let missing = missing_services();
    if missing.is_empty() {
        return Ok(());
    }
    let dnf = ["/usr/bin/dnf5", "/usr/bin/dnf"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| zbus::Error::Failure("neither PackageKit nor dnf is installed: nothing here can install software".into()))?;
    progress(Progress::Status("Installing the software service"));
    let mut child = std::process::Command::new(dnf)
        .args(["install", "-y", "--setopt=install_weak_deps=False"])
        .args(&missing)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| zbus::Error::Failure(format!("dnf couldn't start: {e}")))?;
    // (Its errors kept to say; what it does said as it goes.)
    let err = child.stderr.take().map(|e| std::thread::spawn(move || std::io::read_to_string(e).unwrap_or_default()));
    if let Some(out) = child.stdout.take() {
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(out).lines().map_while(std::result::Result::ok) {
            let line = line.trim();
            if !line.is_empty() {
                progress(Progress::Package(line.to_owned()));
            }
        }
    }
    let status = child.wait().map_err(|e| zbus::Error::Failure(e.to_string()))?;
    let said = err.and_then(|t| t.join().ok()).unwrap_or_default();
    if !status.success() {
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped").trim().to_owned();
        return Err(zbus::Error::Failure(format!("installing {} didn't work: {last}", missing.join(" and "))));
    }
    if let Ok(conn) = bus::system() {
        let _ = bus::call::<_, ()>(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig", &());
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !missing_services().is_empty() {
        if std::time::Instant::now() >= until {
            return Err(zbus::Error::Failure(format!("{} installed, but not yet on the system bus", missing.join(" and "))));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(())
}

/// Whether a package called `name` is installed (PackageKit's word; None
/// without PackageKit).
pub fn installed(name: &str) -> Option<bool> {
    resolve(&[name]).ok().map(|ps| ps.iter().any(|p| p.name() == name && p.installed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let p = Package { id: "akmod-nvidia;3:580.95-1.fc45;x86_64;rpmfusion-nonfree".into(), installed: false, summary: String::new() };
        assert_eq!((p.name(), p.version()), ("akmod-nvidia", "3:580.95-1.fc45"));
        assert_eq!(status(8), Some("Downloading"));
    }

    /// This machine's PackageKit (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn live() {
        eprintln!("{:?}", resolve(&["kernel", "linux-firmware", "akmod-nvidia"]));
        eprintln!("{:?}", updates().map(|u| u.into_iter().map(|p| p.id).collect::<Vec<_>>()));
    }
}
