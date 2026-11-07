//! Olo's own login screen in another's place (GDM's on Fedora), as root
//! steps: what Olo Setup's "Olo's login screen" installs, and what Olo
//! Doctor offers (Ben, 2026-11-07: "the login screen should come up in
//! olo doctor as something it can do for me"), as tools/install-login.sh
//! --switch does. Its system files (PAM services, the unit, the login
//! screen's user and folders) are this build's own, carried in the
//! program, so an install keeps none of them in the home folder. The
//! programs it runs are copies the login screen's user can run, in
//! /usr/local/libexec/olo: an upgrade replaces the person's
//! (~/.local/bin), not these, so Doctor also says when they're older.

use std::path::{Path, PathBuf};

use crate::fix::{Step, UnitAction};

/// Its PAM services (/etc/pam.d), by name.
pub const PAM: &[(&str, &str)] = &[
    ("olo-login", include_str!("../../../data/pam/olo-login")),
    ("olo-login-face", include_str!("../../../data/pam/olo-login-face")),
    ("olo-login-fingerprint", include_str!("../../../data/pam/olo-login-fingerprint")),
    ("olo-login-greeter", include_str!("../../../data/pam/olo-login-greeter")),
];
pub const UNIT: &str = include_str!("../../../data/systemd/olo-login.service");
pub const SYSUSERS: &str = include_str!("../../../data/sysusers/olo-greeter.conf");
pub const TMPFILES: &str = include_str!("../../../data/tmpfiles/olo-greeter.conf");

/// Where the login screen's programs are copied, and which.
pub const LIBEXEC: &str = "/usr/local/libexec/olo";
pub const PROGRAMS: [&str; 5] = ["olo-login", "olo-greeter", "olo-session", "olo-app-server", "olo-wlproxy"];
/// Olo's data the login screen draws with (icons, its look).
const SHARE: &str = "/usr/local/share/olo";
const SHARED: [&str; 5] = ["icons", "brand", "fonts", "wallpapers", "themes"];
/// SELinux's labels for Olo's login screen, as GDM's are: its program
/// runs as a display manager (`xdm_t`, from which the person's session
/// may start: as `bin_t` it was `unconfined_service_t`, which may not),
/// its folders the display manager's own (tools/install-login.sh: the
/// same).
pub const SELINUX_LABELS: &[(&str, &str)] = &[("/usr/local/libexec/olo/olo-login", "xdm_exec_t"), ("/run/olo-login(/.*)?", "xdm_var_run_t"), ("/var/lib/olo-greeter(/.*)?", "xdm_var_lib_t")];
/// The unit that makes it the login screen (`Alias=display-manager.service`).
pub const SERVICE: &str = "olo-login.service";

/// The steps that make Olo's login screen the login screen (or bring its
/// copies up to date: every step can be done again), from the programs
/// in `bin` (and `pam_olo_keyring.so` there) and Olo's data in `share`;
/// `instead` is the display manager it takes the place of (its unit,
/// disabled: `gdm.service`), if there is one.
pub fn steps(bin: &Path, share: &Path, instead: Option<&str>) -> Vec<Step> {
    let libexec = PathBuf::from(LIBEXEC);
    let mut steps = Vec::new();
    for name in PROGRAMS {
        let from = bin.join(name);
        if from.is_file() {
            steps.push(Step::CopyFile { from, to: libexec.join(name), mode: 0o755 });
        }
    }
    for d in SHARED {
        if share.join(d).is_dir() {
            steps.push(Step::CopyTree { from: share.join(d), to: PathBuf::from(SHARE).join(d), mode: 0o644 });
        }
    }
    for (name, text) in PAM {
        steps.push(Step::WriteFile { path: PathBuf::from("/etc/pam.d").join(name), content: (*text).to_owned(), mode: 0o644 });
    }
    // The login password opens Olo's keyring (olo-login's PAM names it).
    if bin.join("pam_olo_keyring.so").is_file() {
        steps.push(Step::CopyFile { from: bin.join("pam_olo_keyring.so"), to: PathBuf::from("/usr/lib64/security/pam_olo_keyring.so"), mode: 0o755 });
    }
    steps.push(Step::WriteFile { path: "/etc/sysusers.d/olo-greeter.conf".into(), content: SYSUSERS.to_owned(), mode: 0o644 });
    steps.push(Step::WriteFile { path: "/etc/tmpfiles.d/olo-greeter.conf".into(), content: TMPFILES.to_owned(), mode: 0o644 });
    // The login screen's user, made by systemd's own program; its folders
    // made here (systemd won't start its tmpfiles service again by hand,
    // and its sysusers service runs only after an update).
    steps.push(Step::MakeSystemUsers { config: "/etc/sysusers.d/olo-greeter.conf".into() });
    steps.extend(folders(TMPFILES));
    steps.push(Step::SelinuxLabels {
        rules: SELINUX_LABELS.iter().map(|(r, t)| ((*r).to_owned(), (*t).to_owned())).collect(),
        restore: [LIBEXEC, "/run/olo-login", "/var/lib/olo-greeter"].map(PathBuf::from).to_vec(),
    });
    steps.push(Step::WriteFile { path: PathBuf::from("/etc/systemd/system").join(SERVICE), content: UNIT.to_owned(), mode: 0o644 });
    steps.push(Step::Unit { name: SERVICE.into(), action: UnitAction::Reload });
    if let Some(other) = instead.filter(|u| *u != SERVICE) {
        steps.push(Step::Unit { name: other.into(), action: UnitAction::Disable });
    }
    steps.push(Step::Unit { name: SERVICE.into(), action: UnitAction::Enable });
    steps
}

/// The folders a tmpfiles.d file's `d` lines ask for (`d PATH MODE USER
/// GROUP -`), as steps.
pub fn folders(tmpfiles: &str) -> Vec<Step> {
    tmpfiles
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 4 && f[0] == "d").then(|| Step::MakeDir {
                path: PathBuf::from(f[1]),
                mode: u32::from_str_radix(f[2], 8).unwrap_or(0o755),
                owner: (f[3] != "root" && f[3] != "-").then(|| f[3].to_owned()),
            })
        })
        .collect()
}

/// The display manager the system starts, under the root `fs` (its unit:
/// `gdm.service`, `olo-login.service`), if it says.
pub fn display_manager(fs: &Path) -> Option<String> {
    let link = std::fs::read_link(fs.join("etc/systemd/system/display-manager.service")).ok()?;
    link.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// The login screen's copies older than the programs in `bin` (by when
/// each was written: an upgrade writes `bin`'s after the copies), by name.
pub fn stale(fs: &Path, bin: &Path) -> Vec<&'static str> {
    let written = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    PROGRAMS
        .into_iter()
        .filter(|name| {
            let copy = fs.join(LIBEXEC.trim_start_matches('/')).join(name);
            match (written(&bin.join(name)), written(&copy)) {
                (Some(new), Some(old)) => new > old,
                // (Not copied, though the program is there: a part missing.)
                (Some(_), None) => true,
                _ => false,
            }
        })
        .collect()
}
