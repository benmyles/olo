//! The accounts signed in to the 1Password app on this computer, so an app
//! can offer them by name instead of asking for one. The app keeps a
//! summary of each (`accounts` in its `1password.sqlite`: JSON with the
//! team's name, the user's name and email and the sign-in address, in the
//! clear beside the encrypted keys); this reads only those, read-only.
//! The SDK integration takes an account's UUID as its name.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// What the SDK integration takes as `account_name`.
    pub uuid: String,
    /// As 1Password's sidebar shows it.
    pub name: String,
    pub email: String,
    /// The sign-in address, e.g. `my.1password.com`.
    pub address: String,
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    team_name: String,
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    user_email: String,
    #[serde(default)]
    sign_in_url: String,
    #[serde(default)]
    account_type: String,
}

/// Where the 1Password app keeps its data: `OLO_1PASSWORD_DATA`, else
/// the native app's, else the Flatpak's.
pub fn data_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("OLO_1PASSWORD_DATA") {
        return vec![dir.into()];
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    vec![config.join("1Password"), home.join(".var/app/com.onepassword.OnePassword/config/1Password")]
}

/// Every account the app knows, by name (none when the app isn't
/// installed or its data can't be read).
pub fn local() -> Vec<Account> {
    let mut out: Vec<Account> = Vec::new();
    for dir in data_dirs() {
        match read(&dir.join("1password.sqlite")) {
            Ok(accounts) => {
                for a in accounts {
                    if !out.iter().any(|o| o.uuid == a.uuid) {
                        out.push(a);
                    }
                }
            }
            Err(e) => tracing::debug!("1Password accounts in {}: {e}", dir.display()),
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn read(db: &Path) -> rusqlite::Result<Vec<Account>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    let mut q = conn.prepare("SELECT account_uuid, data FROM accounts")?;
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (uuid, data) = row?;
        let Ok(s) = serde_json::from_slice::<Summary>(&data) else { continue };
        out.push(account(uuid, s));
    }
    Ok(out)
}

fn account(uuid: String, s: Summary) -> Account {
    // An individual or family account's "team" is the person; the sidebar
    // shows the account's own name where there is one.
    let name = if !s.team_name.trim().is_empty() {
        s.team_name.trim().to_owned()
    } else if !s.user_name.trim().is_empty() {
        s.user_name.trim().to_owned()
    } else if s.account_type == "I" {
        "Personal".to_owned()
    } else {
        uuid.clone()
    };
    let address = s.sign_in_url.trim_start_matches("https://").trim_end_matches('/').to_owned();
    Account { uuid, name, email: s.user_email, address }
}

/// Write a database shaped like the app's with these accounts (for the
/// stand-in app and tests).
pub fn write_fixture(db: &Path, accounts: &[Account]) -> rusqlite::Result<()> {
    let _ = std::fs::remove_file(db);
    let conn = rusqlite::Connection::open(db)?;
    conn.execute_batch("CREATE TABLE accounts (account_uuid TEXT PRIMARY KEY NOT NULL, data BLOB NOT NULL)")?;
    for a in accounts {
        let data = serde_json::json!({
            "team_name": a.name,
            "user_name": a.name,
            "user_email": a.email,
            "sign_in_url": format!("https://{}", a.address),
            "account_type": "B",
        });
        conn.execute("INSERT INTO accounts VALUES (?1, ?2)", rusqlite::params![a.uuid, serde_json::to_vec(&data).unwrap()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_apps_accounts() {
        let dir = std::env::temp_dir().join(format!("olo-1p-accounts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("1password.sqlite");
        let a = |uuid: &str, name: &str| Account { uuid: uuid.into(), name: name.into(), email: "user@example.com".into(), address: "example.1password.com".into() };
        write_fixture(&db, &[a("BBBBBBBBBBBBBBBBBBBBBBBBBB", "work"), a("AAAAAAAAAAAAAAAAAAAAAAAAAA", "Family")]).unwrap();
        let got = read(&db).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&a("BBBBBBBBBBBBBBBBBBBBBBBBBB", "work")));
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("OLO_1PASSWORD_DATA", &dir) };
        let names: Vec<String> = local().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["Family", "work"]);
        // An individual account with no team name.
        let s: Summary = serde_json::from_str(r#"{"user_name":"","team_name":"","account_type":"I","sign_in_url":"https://my.1password.com/"}"#).unwrap();
        let p = account("C".into(), s);
        assert_eq!((p.name.as_str(), p.address.as_str()), ("Personal", "my.1password.com"));
        assert!(read(&dir.join("missing.sqlite")).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
