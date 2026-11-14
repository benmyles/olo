//! The pairings kept: for each receiver paired with, who it is and the
//! keys that prove us to each other ([`crate::pairing::Credentials`]), in
//! a file of the person's own that only they can read
//! (`airplay/pairings.json` in Olo's state folder). A receiver is known
//! by its id, which stays the same when it's renamed.
//!
//! (A pairing's key lets its holder send to that receiver, nothing
//! more; it's kept as others keep theirs, and could move into
//! Passwords.)

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pairing::Credentials;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    /// The receiver's id as it announces it ("AA:BB:CC:DD:EE:FF").
    pub device_id: String,
    /// Its name when it was paired with (it may have been renamed).
    pub name: String,
    /// [`Credentials::to_line`].
    pub credentials: String,
}

impl Kept {
    pub fn credentials(&self) -> Option<Credentials> {
        Credentials::from_line(&self.credentials)
    }
}

pub fn path() -> PathBuf {
    olo_protocol::airplay::pairings_path()
}

pub fn all() -> Vec<Kept> {
    std::fs::read_to_string(path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The pairing with the receiver of this id, or (it announced none)
/// this name.
pub fn of(device_id: &str, name: &str) -> Option<Kept> {
    let all = all();
    all.iter().find(|k| !device_id.is_empty() && k.device_id.eq_ignore_ascii_case(device_id)).or_else(|| all.iter().find(|k| k.name == name)).cloned()
}

/// Keep a pairing, in place of any with the same receiver: the one of
/// its id, or (either without an id) of its name. Two receivers that
/// share a name and differ in id are two.
pub fn keep(new: Kept) -> std::io::Result<()> {
    let mut all = all();
    all.retain(|k| !same(k, &new));
    all.push(new);
    write(&all)
}

fn same(a: &Kept, b: &Kept) -> bool {
    if a.device_id.is_empty() || b.device_id.is_empty() { a.name == b.name } else { a.device_id.eq_ignore_ascii_case(&b.device_id) }
}

/// Forget the pairings with this name or id; how many there were.
pub fn forget(which: &str) -> std::io::Result<usize> {
    let mut all = all();
    let before = all.len();
    all.retain(|k| !k.name.eq_ignore_ascii_case(which) && !k.device_id.eq_ignore_ascii_case(which));
    if all.len() != before {
        write(&all)?;
    }
    Ok(before - all.len())
}

fn write(all: &[Kept]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let path = path();
    let dir = path.parent().expect("a folder");
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    // (Whole or not at all, and never readable by another on its way.)
    let tmp = path.with_extension("json.new");
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(all).unwrap_or_else(|_| "[]".into()).as_bytes())?;
    }
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(id: &str, name: &str) -> Kept {
        Kept { device_id: id.into(), name: name.into(), credentials: String::new() }
    }

    #[test]
    fn one_receiver_is_one_by_its_id_whatever_its_called() {
        assert!(same(&kept("AA:BB", "Front Room"), &kept("aa:bb", "Den")));
        assert!(!same(&kept("AA:BB", "Front Room"), &kept("CC:DD", "Front Room")));
        assert!(same(&kept("", "Front Room"), &kept("CC:DD", "Front Room")));
        assert!(!same(&kept("", "Front Room"), &kept("", "Den")));
    }
}
