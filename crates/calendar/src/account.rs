//! Calendar accounts: any number of Google's and Fastmail's (and any
//! other CalDAV server's, the protocol being one). An account is who
//! keeps it and the address signed in with (`~/.config/olo/calendar.json`,
//! nothing secret in it); what signs it in is kept in the login keyring
//! (as Olo's other secrets: `olo_system::secrets`): an app password
//! (Fastmail's: Settings ▸ Privacy & Security ▸ app passwords, with
//! access to calendars), or for Google the person's own OAuth client and
//! the refresh token its sign-in gave (`olo_google`, as DiskBackup signs
//! in to Drive: the same client serves both, with the CalDAV API turned
//! on in its project).

use std::{
    io,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::dav::{self, Auth, Server};

/// Who keeps the calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Google,
    Fastmail,
    /// Any other CalDAV server, by its address.
    Other,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Google, Provider::Fastmail, Provider::Other];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Google => "Google",
            Provider::Fastmail => "Fastmail",
            Provider::Other => "CalDAV",
        }
    }
}

/// What Calendar asks of Google: its calendars, read and written, and
/// whose account it is.
pub const GOOGLE_SCOPES: &str = "openid email https://www.googleapis.com/auth/calendar";

/// Where its app passwords are made.
pub const FASTMAIL_APP_PASSWORDS: &str = "https://app.fastmail.com/settings/security";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub provider: Provider,
    /// The address signed in with.
    pub address: String,
    /// Its server's address (`Other` only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server: String,
}

impl Account {
    /// What names it, in files and the keyring: who keeps it and the
    /// address (and, for another server, which).
    pub fn id(&self) -> String {
        match self.provider {
            Provider::Google => format!("google:{}", self.address),
            Provider::Fastmail => format!("fastmail:{}", self.address),
            Provider::Other => format!("caldav:{}@{}", self.address, self.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/')),
        }
    }

    /// Where its calendars are looked for from (`OLO_CALENDAR_SERVER`: a
    /// test's stand-in, for every account).
    pub fn start(&self) -> String {
        if let Ok(test) = std::env::var("OLO_CALENDAR_SERVER") {
            return test;
        }
        match self.provider {
            Provider::Google => format!("https://apidata.googleusercontent.com/caldav/v2/{}/user", olo_google::escape(&self.address)),
            Provider::Fastmail => format!("https://caldav.fastmail.com/dav/principals/user/{}/", olo_google::escape(&self.address)),
            Provider::Other => self.server.clone(),
        }
    }
}

/// Where the accounts are listed (`OLO_CALENDAR_ACCOUNTS`: another file,
/// a test's).
pub fn accounts_path() -> PathBuf {
    std::env::var_os("OLO_CALENDAR_ACCOUNTS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("calendar.json"))
}

/// The accounts set up, in the order they were added.
pub fn accounts() -> Vec<Account> {
    std::fs::read(accounts_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The accounts set up are these.
pub fn save_accounts(accounts: &[Account]) -> io::Result<()> {
    let path = accounts_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(accounts).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

/// What signs an account in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Secret {
    /// An app password.
    Password(String),
    /// Google's: the person's own OAuth client, and the refresh token.
    Google { client: olo_google::Client, refresh_token: String },
}

fn attributes(id: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("service", "calendar"), ("account", id)]
}

/// Keep what signs `account` in, in the login keyring. Blocking (a D-Bus
/// call): off the UI thread.
pub fn keep_secret(account: &Account, secret: &Secret) -> Result<(), String> {
    let (id, label) = (account.id(), format!("Calendar: {} (Olo)", account.address));
    olo_system::secrets::store(&label, &attributes(&id), &serde_json::to_vec(secret).unwrap_or_default()).map_err(|e| e.to_string())
}

/// What signs `account` in, if kept. Blocking. (`OLO_CALENDAR_SECRET`:
/// a test's, as JSON, for every account.)
pub fn secret(account: &Account) -> Option<Secret> {
    if let Ok(test) = std::env::var("OLO_CALENDAR_SECRET") {
        return serde_json::from_str(&test).ok();
    }
    serde_json::from_slice(&olo_system::secrets::lookup(&attributes(&account.id())).ok()??).ok()
}

/// Forget it. Blocking.
pub fn forget_secret(account: &Account) {
    let _ = olo_system::secrets::delete(&attributes(&account.id()));
}

/// A Google client already on this computer to sign in with, so the
/// person makes one once: another calendar account's, else DiskBackup's.
/// Blocking.
pub fn known_google_client() -> Option<olo_google::Client> {
    for a in accounts() {
        if let Some(Secret::Google { client, .. }) = secret(&a) {
            return Some(client);
        }
    }
    // (DiskBackup's sign-in: {"client": …, "refresh_token": …}.)
    let kept = olo_system::secrets::lookup(olo_system::secrets::DISKBACKUP_GOOGLE.attributes).ok()??;
    serde_json::from_slice::<serde_json::Value>(&kept).ok().and_then(|v| serde_json::from_value(v.get("client")?.clone()).ok())
}

/// Google's addresses (`OLO_CALENDAR_GOOGLE`: a test's own Google, its
/// `/auth` and `/token`).
pub fn google_endpoints() -> olo_google::Endpoints {
    match std::env::var("OLO_CALENDAR_GOOGLE") {
        Ok(base) => olo_google::Endpoints { auth: format!("{base}/auth"), token: format!("{base}/token"), api: base },
        Err(_) => Default::default(),
    }
}

/// `account`'s server, signed in with `secret`: nothing is asked of it
/// yet. Google's tokens are fetched as they're wanted and kept while
/// they're good (an hour, less a minute).
pub fn connect(account: &Account, secret: &Secret) -> Server {
    connect_at(account, secret, google_endpoints())
}

/// [`connect`], with Google at `endpoints`.
pub fn connect_at(account: &Account, secret: &Secret, endpoints: olo_google::Endpoints) -> Server {
    match secret {
        Secret::Password(password) => Server::new(Auth::Basic { user: account.address.clone(), password: password.clone() }),
        Secret::Google { client, refresh_token } => {
            let (client, refresh) = (client.clone(), refresh_token.clone());
            let http = olo_net::PerNetwork::new(|| olo_google::agent("Olo Calendar"));
            let held: Mutex<Option<(String, Instant)>> = Mutex::new(None);
            Server::with_tokens(move |fresh| {
                let mut held = held.lock().unwrap();
                if !fresh
                    && let Some((token, until)) = held.as_ref()
                    && Instant::now() < *until
                {
                    return Ok(token.clone());
                }
                let (token, lasts) =
                    olo_google::access_token(&http.get(), &endpoints, &client, &refresh).map_err(|e| if olo_google::unreachable(&e) { dav::Error::Network(e) } else { dav::Error::SignIn })?;
                *held = Some((token.clone(), Instant::now() + lasts.saturating_sub(Duration::from_secs(60))));
                Ok(token)
            })
        }
    }
}

/// Whether `account` signs in with `secret`: its calendars, as adding it
/// shows them (Preferences ▸ Integrations). Blocking.
pub fn check(account: &Account, secret: &Secret) -> Result<Vec<dav::Collection>, dav::Error> {
    connect(account, secret).calendars(&account.start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_named_and_found() {
        let google = Account { provider: Provider::Google, address: "ben@example.org".into(), server: String::new() };
        let fastmail = Account { provider: Provider::Fastmail, address: "ben@fastmail.example".into(), server: String::new() };
        let other = Account { provider: Provider::Other, address: "ben".into(), server: "https://cloud.example.org/remote.php/dav/".into() };
        assert_eq!((google.id(), fastmail.id(), other.id()), ("google:ben@example.org".into(), "fastmail:ben@fastmail.example".into(), "caldav:ben@cloud.example.org/remote.php/dav".into()));
        if std::env::var_os("OLO_CALENDAR_SERVER").is_none() {
            assert_eq!(google.start(), "https://apidata.googleusercontent.com/caldav/v2/ben%40example.org/user");
            assert_eq!(fastmail.start(), "https://caldav.fastmail.com/dav/principals/user/ben%40fastmail.example/");
            assert_eq!(other.start(), "https://cloud.example.org/remote.php/dav/");
        }
        // As they're listed on disk: nothing secret, the server only where there is one.
        let text = serde_json::to_string(&[google.clone(), other.clone()]).unwrap();
        assert_eq!(text, r#"[{"provider":"google","address":"ben@example.org"},{"provider":"other","address":"ben","server":"https://cloud.example.org/remote.php/dav/"}]"#);
        assert_eq!(serde_json::from_str::<Vec<Account>>(&text).unwrap(), [google, other]);
        // What signs one in, as it's kept.
        let s = Secret::Google { client: olo_google::Client { id: "1.apps.googleusercontent.com".into(), secret: "x".into() }, refresh_token: "r".into() };
        assert_eq!(serde_json::from_str::<Secret>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
        assert_eq!(serde_json::to_string(&Secret::Password("p".into())).unwrap(), r#"{"password":"p"}"#);
    }

    /// Google's way in: an access token is asked for with the refresh
    /// token once and used while it's good; one that stopped being good
    /// without the clock here saying so (the computer asleep) is replaced
    /// the moment the server refuses it, and the request made again; a
    /// sign-in Google ended is said as that, not as the network's fault.
    #[test]
    fn googles_tokens_fetched_kept_and_replaced() {
        use crate::standin::Standin;
        let there = Standin::start("nobody", "no password signs in here");
        there.add_calendar("primary", "Ben", Some("#4285F4"));
        there.knows_refresh_token("refresh-1");
        let endpoints = olo_google::Endpoints { auth: format!("{}auth", there.url()), token: format!("{}token", there.url()), api: there.url() };
        let account = Account { provider: Provider::Google, address: "ben@example.org".into(), server: String::new() };
        let client = olo_google::Client { id: "1.apps.googleusercontent.com".into(), secret: "x".into() };
        let server = connect_at(&account, &Secret::Google { client: client.clone(), refresh_token: "refresh-1".into() }, endpoints.clone());
        assert_eq!(server.calendars(&there.url()).unwrap()[0].name, "Ben");
        let cal = format!("{}cal/primary/", there.url());
        server.put(&format!("{cal}a.ics"), "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", None).unwrap();
        assert_eq!(server.list(&cal).unwrap().len(), 1);
        assert_eq!(there.tokens_given(), 1, "one token for all of that");
        // Its hour ended while the computer slept: refused, replaced, asked again.
        there.expire_tokens();
        assert_eq!(server.list(&cal).unwrap().len(), 1);
        assert_eq!(there.tokens_given(), 2);
        // A write too (it wasn't done the first time: it was refused).
        there.expire_tokens();
        server.put(&format!("{cal}b.ics"), "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", None).unwrap();
        assert_eq!((there.files("primary").len(), there.tokens_given()), (2, 3));
        // Google ended the sign-in: said as that.
        let ended = connect_at(&account, &Secret::Google { client, refresh_token: "revoked".into() }, endpoints);
        assert_eq!(ended.calendars(&there.url()), Err(dav::Error::SignIn));
        // Google not there: the network's.
        let away = olo_google::Endpoints { auth: "http://127.0.0.1:9/auth".into(), token: "http://127.0.0.1:9/token".into(), api: "http://127.0.0.1:9".into() };
        let nowhere = connect_at(&account, &Secret::Google { client: olo_google::Client { id: "i".into(), secret: "s".into() }, refresh_token: "r".into() }, away);
        assert!(matches!(nowhere.calendars(&there.url()), Err(dav::Error::Network(_))));
    }
}
