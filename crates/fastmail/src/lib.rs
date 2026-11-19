//! The Fastmail accounts Olo is connected to, and what each is used for,
//! as `olo_google::accounts` has Google's (Ben, 2026-11-18: "add a
//! Fastmail tab modelled after google where you add fastmail accounts
//! and choose what they integrate with").
//!
//! An account signs in with an app password of its own, made in
//! Fastmail's settings ([`APP_PASSWORDS`]) with access to mail and
//! calendars: one password for both, so ticking Calendar for an account
//! Mail uses asks for nothing more. Mail and Calendar each read this
//! list ([`serving`]) and take the password from here ([`password`]).
//!
//! The list is `~/.config/olo/fastmail-accounts.json` (addresses and
//! ticks: nothing secret), changed under a lock (`.lock` beside it) so
//! two programs changing it at once don't lose a tick; each password is
//! in the login keyring (application olo, service fastmail, account the
//! address). Mail and Calendar each kept their Fastmail accounts in lists
//! of their own before, passwords under their own names: each moves
//! them here, passwords and all, as it next reads its accounts
//! ([`adopt`], from `olo_mail::accounts` and
//! `olo_calendar::account::accounts`), so an upgrade carries them over
//! by itself.

use std::{fs::File, io, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Where its app passwords are made (Settings ▸ Privacy & Security ▸
/// Manage app passwords and access).
pub const APP_PASSWORDS: &str = "https://app.fastmail.com/settings/security";

/// What Olo does with a Fastmail account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Mail,
    Calendar,
}

impl Service {
    pub const ALL: [Service; 2] = [Service::Mail, Service::Calendar];

    pub fn name(self) -> &'static str {
        match self {
            Service::Mail => "Mail",
            Service::Calendar => "Calendar",
        }
    }
}

/// A Fastmail account connected to Olo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub address: String,
    /// What it's used for (the person's ticks).
    pub services: Vec<Service>,
}

impl Account {
    /// The person ticked `service` for it.
    pub fn ticked(&self, service: Service) -> bool {
        self.services.contains(&service)
    }
}

/// Where the accounts are listed (`OLO_FASTMAIL_ACCOUNTS`: another file,
/// a test's).
pub fn path() -> PathBuf {
    std::env::var_os("OLO_FASTMAIL_ACCOUNTS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("fastmail-accounts.json"))
}

/// The accounts connected, in the order they were added.
pub fn accounts() -> Vec<Account> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Those ticked for `service`.
pub fn serving(service: Service) -> Vec<Account> {
    accounts().into_iter().filter(|a| a.ticked(service)).collect()
}

/// The list changed by `change` under its lock, and written whole, then
/// put in place (whoever follows the file never reads half of it).
fn changed<T>(change: impl FnOnce(&mut Vec<Account>) -> T) -> io::Result<T> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut all = accounts();
    let out = change(&mut all);
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&all).unwrap_or_default())?;
    std::fs::rename(tmp, &path)?;
    Ok(out)
}

/// `service` ticked or not for `address`.
pub fn tick(address: &str, service: Service, on: bool) -> io::Result<()> {
    changed(|all| {
        if let Some(a) = all.iter_mut().find(|a| a.address.eq_ignore_ascii_case(address)) {
            if on && !a.ticked(service) {
                a.services.push(service);
                a.services.sort();
            } else if !on {
                a.services.retain(|s| *s != service);
            }
        }
    })
}

/// `address` connected, used for `services` (one there already gets
/// these too, keeping its own), its app password kept. Blocking.
pub fn add(address: &str, password: &str, services: &[Service]) -> Result<(), String> {
    let address = address.trim().to_lowercase();
    keep_password(&address, password.trim())?;
    with(&address, services).map_err(|e| format!("the account couldn't be saved ({e})"))
}

/// `address` listed with `services` among its ticks.
fn with(address: &str, services: &[Service]) -> io::Result<()> {
    changed(|all| {
        let i = match all.iter().position(|a| a.address.eq_ignore_ascii_case(address)) {
            Some(i) => i,
            None => {
                all.push(Account { address: address.to_owned(), services: Vec::new() });
                all.len() - 1
            }
        };
        for s in services {
            if !all[i].ticked(*s) {
                all[i].services.push(*s);
            }
        }
        all[i].services.sort();
    })
}

/// An account Mail or Calendar kept itself before, taken over: listed,
/// ticked for `service`, its password (theirs, if they had one) kept
/// unless one is already. Blocking. Done when this says so: only then
/// may they let go of theirs.
pub fn adopt(address: &str, service: Service, password: Option<&str>) -> Result<(), String> {
    let address = address.trim().to_lowercase();
    if let Some(p) = password.filter(|p| !p.trim().is_empty())
        && self::password(&address).is_none()
    {
        keep_password(&address, p.trim())?;
    }
    with(&address, &[service]).map_err(|e| format!("the account couldn't be saved ({e})"))
}

/// `address` disconnected: off the list, its password forgotten.
/// Blocking.
pub fn remove(address: &str) -> io::Result<()> {
    changed(|all| all.retain(|a| !a.address.eq_ignore_ascii_case(address)))?;
    forget_password(address);
    Ok(())
}

fn attributes(address: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("service", "fastmail"), ("account", address)]
}

/// A test's own keyring (`OLO_FASTMAIL_SECRETS`: a folder, a file an
/// account), so nothing a test adds or removes touches the person's
/// login keyring, which a test session's programs can reach.
fn test_secret(address: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OLO_FASTMAIL_SECRETS")?;
    let name: String = address.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '@') { c } else { '_' }).collect();
    Some(PathBuf::from(dir).join(name))
}

/// Keep `address`'s app password, in the login keyring. Blocking (a
/// D-Bus call): off the UI thread.
pub fn keep_password(address: &str, password: &str) -> Result<(), String> {
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        return file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, password)).map_err(|e| e.to_string());
    }
    olo_system::secrets::store(&format!("Fastmail: {address} (Olo)"), &attributes(&address), password.as_bytes()).map_err(|e| format!("the keyring can't keep it: {e}"))
}

/// `address`'s app password, if kept. Blocking.
pub fn password(address: &str) -> Option<String> {
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        return std::fs::read_to_string(file).ok();
    }
    String::from_utf8(olo_system::secrets::lookup(&attributes(&address)).ok()??).ok()
}

/// Forget it. Blocking.
pub fn forget_password(address: &str) {
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        let _ = std::fs::remove_file(file);
        return;
    }
    let _ = olo_system::secrets::delete(&attributes(&address));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, as the list and the keyring are a process's (its
    /// environment): added with a password and ticks, ticked and
    /// unticked, adopted from another list (its password kept only where
    /// none is, its ticks added to), removed with its password.
    #[test]
    fn accounts_added_ticked_adopted_and_removed() {
        let dir = PathBuf::from(format!("/var/tmp/olo-fastmail-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: the one test in this binary reading them.
        unsafe {
            std::env::set_var("OLO_FASTMAIL_ACCOUNTS", dir.join("fastmail-accounts.json"));
            std::env::set_var("OLO_FASTMAIL_SECRETS", dir.join("secrets"));
        }
        assert!(accounts().is_empty());
        add(" Ann@Example.org ", "pw-1 ", &[Service::Mail]).unwrap();
        assert_eq!(accounts(), [Account { address: "ann@example.org".into(), services: vec![Service::Mail] }]);
        assert_eq!(password("ann@example.org").as_deref(), Some("pw-1"));
        tick("ANN@example.org", Service::Calendar, true).unwrap();
        assert_eq!(serving(Service::Calendar).len(), 1);
        tick("ann@example.org", Service::Mail, false).unwrap();
        assert_eq!(accounts()[0].services, [Service::Calendar]);
        // Taken over from Mail's own list: ticked for Mail too; the
        // password it had kept, as one is already.
        adopt("ann@example.org", Service::Mail, Some("old")).unwrap();
        assert_eq!(accounts()[0].services, [Service::Mail, Service::Calendar]);
        assert_eq!(password("ann@example.org").as_deref(), Some("pw-1"));
        // A new one, with its password.
        adopt("bob@example.net", Service::Calendar, Some("pw-2")).unwrap();
        assert_eq!(accounts().len(), 2);
        assert_eq!(password("bob@example.net").as_deref(), Some("pw-2"));
        remove("ann@example.org").unwrap();
        assert_eq!(accounts().iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["bob@example.net"]);
        assert_eq!(password("ann@example.org"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
