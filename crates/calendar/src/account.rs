//! Calendar accounts: any number of Google's and Fastmail's (and any
//! other CalDAV server's, the protocol being one). An account is who
//! keeps it and the address signed in with.
//!
//! Google's are the Google accounts connected to Olo with Calendar
//! ticked (Preferences ▸ Integrations ▸ Google; `olo_google::accounts`:
//! each signs in through an app of its own, "Olo (its address)", which
//! serves Mail and DiskBackup too). The others are listed in
//! `~/.config/olo/calendar-accounts.json` (nothing secret in it; the
//! app's own settings are `calendar.json`), and what signs each in, an
//! app password (Fastmail's: Settings ▸ Privacy & Security ▸ app
//! passwords, with access to calendars), is kept in the login keyring
//! (as Olo's other secrets: `olo_system::secrets`).

use std::{
    io,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::dav::{self, Auth, Server};

/// A person's own OAuth client for Google (`olo_google::Client`).
pub use olo_google::Client as GoogleClient;

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
    std::env::var_os("OLO_CALENDAR_ACCOUNTS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("calendar-accounts.json"))
}

/// The accounts added here (Fastmail's, another CalDAV server's), as
/// `calendar-accounts.json` lists them.
pub fn own_accounts() -> Vec<Account> {
    std::fs::read(accounts_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Whether this computer's Google accounts are looked at: not by a test
/// that names lists of its own without naming Google's.
fn googles() -> bool {
    std::env::var_os("OLO_CALENDAR_ACCOUNTS").is_none() || std::env::var_os("OLO_GOOGLE_ACCOUNTS").is_some()
}

/// Whether this computer's Fastmail accounts are looked at, as
/// [`googles`].
fn fastmails() -> bool {
    std::env::var_os("OLO_CALENDAR_ACCOUNTS").is_none() || std::env::var_os("OLO_FASTMAIL_ACCOUNTS").is_some()
}

/// Every account whose calendars Calendar shows: the Google accounts
/// connected to Olo with Calendar ticked, the Fastmail accounts ticked
/// for it (Integrations ▸ Fastmail, `olo_fastmail`), then those added
/// here (other CalDAV servers: Integrations ▸ CalDAV), in the order they
/// were added.
pub fn accounts() -> Vec<Account> {
    let mut all: Vec<Account> = if googles() {
        olo_google::accounts::serving(olo_google::accounts::Service::Calendar).into_iter().map(|a| Account { provider: Provider::Google, address: a.address, server: String::new() }).collect()
    } else {
        Vec::new()
    };
    if fastmails() {
        adopt_fastmail();
        all.extend(olo_fastmail::serving(olo_fastmail::Service::Calendar).into_iter().map(|a| Account { provider: Provider::Fastmail, address: a.address, server: String::new() }));
    }
    for a in own_accounts() {
        if !all.iter().any(|b| b.id() == a.id()) {
            all.push(a);
        }
    }
    all
}

/// The files that say which accounts there are: followed by whoever
/// shows them (Calendar's window, its reminders, its widgets).
pub fn watched() -> Vec<PathBuf> {
    vec![accounts_path(), olo_google::accounts::path(), olo_fastmail::path()]
}

/// The Fastmail accounts added here before Integrations had a Fastmail
/// tab (in this list, their app passwords under Calendar's name) moved
/// to `olo_fastmail`'s, ticked for Calendar, passwords and all: an
/// upgrade carries them over by itself. Each is let go of here only once
/// it's there. Once a program, until done.
fn adopt_fastmail() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if DONE.load(Ordering::Acquire) {
        return;
    }
    let theirs: Vec<Account> = own_accounts().into_iter().filter(|a| a.provider == Provider::Fastmail).collect();
    let mut moved = Vec::new();
    for a in &theirs {
        let kept = match kept_secret(a) {
            Ok(Some(Secret::Password(p))) => Some(p),
            Ok(_) => None,
            Err(e) => {
                tracing::info!("calendar: the Fastmail account {} is moved later: its password can't be read now ({e})", a.address);
                continue;
            }
        };
        match olo_fastmail::adopt(&a.address, olo_fastmail::Service::Calendar, kept.as_deref()) {
            Ok(()) => moved.push(a.clone()),
            Err(e) => tracing::warn!("calendar: the Fastmail account {} isn't moved: {e}", a.address),
        }
    }
    if !moved.is_empty() {
        let mut listed = own_accounts();
        listed.retain(|a| !moved.contains(a));
        match save_accounts(&listed) {
            Ok(()) => {
                for a in &moved {
                    forget_secret(a);
                    tracing::info!("calendar: the Fastmail account {} moved to Integrations ▸ Fastmail", a.address);
                }
            }
            Err(e) => tracing::warn!("calendar: the Fastmail accounts moved stay listed here too ({e})"),
        }
    }
    if moved.len() == theirs.len() {
        DONE.store(true, Ordering::Release);
    }
}

/// The accounts added here are these (`calendar-accounts.json`: Google's
/// aren't in it).
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

/// A test's own keyring (`OLO_CALENDAR_SECRETS`: a folder, a file an
/// account), so that nothing a test adds or removes touches the
/// person's login keyring, which a test session's programs can reach.
fn test_secret(account: &Account) -> Option<PathBuf> {
    let dir = std::env::var_os("OLO_CALENDAR_SECRETS")?;
    let name: String = account.id().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '@') { c } else { '_' }).collect();
    Some(PathBuf::from(dir).join(format!("{name}.json")))
}

/// Keep what signs `account` in, in the login keyring. Blocking (a D-Bus
/// call): off the UI thread.
pub fn keep_secret(account: &Account, secret: &Secret) -> Result<(), String> {
    let json = serde_json::to_vec(secret).unwrap_or_default();
    if let Some(file) = test_secret(account) {
        return file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, json)).map_err(|e| e.to_string());
    }
    let (id, label) = (account.id(), format!("Calendar: {} (Olo)", account.address));
    olo_system::secrets::store(&label, &attributes(&id), &json).map_err(|e| e.to_string())
}

/// What signs `account` in, if kept: a Google account connected to Olo
/// signs in through its own app (`olo_google::accounts`), a Fastmail
/// one with the app password Integrations ▸ Fastmail keeps for it.
/// Blocking.
pub fn secret(account: &Account) -> Option<Secret> {
    if account.provider == Provider::Google
        && (std::env::var_os("OLO_CALENDAR_SECRETS").is_none() || std::env::var_os("OLO_GOOGLE_SECRETS").is_some())
        && let Some(s) = olo_google::accounts::secret(&account.address)
    {
        return Some(Secret::Google { client: s.client, refresh_token: s.refresh_token });
    }
    if account.provider == Provider::Fastmail
        && fastmails()
        && let Some(p) = olo_fastmail::password(&account.address)
    {
        return Some(Secret::Password(p));
    }
    kept_secret(account).ok().flatten()
}

/// What this list kept for `account` itself (Err: the keyring couldn't
/// be read). Blocking.
fn kept_secret(account: &Account) -> Result<Option<Secret>, String> {
    if let Some(file) = test_secret(account) {
        return Ok(std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()));
    }
    Ok(olo_system::secrets::lookup(&attributes(&account.id())).map_err(|e| e.to_string())?.and_then(|b| serde_json::from_slice(&b).ok()))
}

/// Forget it. Blocking.
pub fn forget_secret(account: &Account) {
    if let Some(file) = test_secret(account) {
        let _ = std::fs::remove_file(file);
        return;
    }
    let _ = olo_system::secrets::delete(&attributes(&account.id()));
}

/// Google's addresses (`OLO_CALENDAR_GOOGLE`: a test's own Google, its
/// `/auth` and `/token`; else Olo's, `olo_google::accounts::endpoints`).
pub fn google_endpoints() -> olo_google::Endpoints {
    match std::env::var("OLO_CALENDAR_GOOGLE") {
        Ok(base) => olo_google::Endpoints { auth: format!("{base}/auth"), token: format!("{base}/token"), api: base },
        Err(_) => olo_google::accounts::endpoints(),
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

    /// Signing in to Google (Olo's one sign-in, `olo_google::accounts`):
    /// the consent page opened (here fetched: the stand-in agrees at once
    /// and sends the browser back), asking for the account's calendars
    /// among the rest, the code made a refresh token, the account named
    /// by the address Google says; and what that gives signs in to its
    /// calendars.
    #[test]
    fn signing_in_to_google() {
        use crate::standin::Standin;
        let there = Standin::start("Ada@Example.org", "no password signs in here");
        there.add_calendar("primary", "Ada", None);
        there.knows_refresh_token("refresh-7");
        let endpoints = olo_google::Endpoints { auth: format!("{}auth", there.url()), token: format!("{}token", there.url()), api: there.url() };
        let client = olo_google::Client { id: "1.apps.googleusercontent.com".into(), secret: "x".into() };
        let opened = std::sync::Mutex::new(String::new());
        let signed = olo_google::accounts::sign_in_at(&client, None, &endpoints, &std::sync::atomic::AtomicBool::new(false), |url| {
            *opened.lock().unwrap() = url.to_owned();
            let url = url.to_owned();
            std::thread::spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(4).build().into();
                let _ = agent.get(&url).call();
            });
        })
        .unwrap();
        let url = opened.into_inner().unwrap();
        assert!(url.contains("auth%2Fcalendar") && url.contains("access_type=offline") && url.contains("code_challenge_method=S256") && url.contains("prompt=select_account"), "{url}");
        assert!(url.contains("mail.google.com") && url.contains("drive.file"), "everything Olo's services use: {url}");
        assert_eq!((signed.email.to_lowercase().as_str(), signed.refresh_token.as_str()), ("ada@example.org", "refresh-7"));
        assert!(signed.scopes.iter().any(|s| s == "https://www.googleapis.com/auth/calendar"), "{:?}", signed.scopes);
        let account = Account { provider: Provider::Google, address: signed.email.to_lowercase(), server: String::new() };
        let secret = Secret::Google { client: client.clone(), refresh_token: signed.refresh_token };
        assert_eq!(connect_at(&account, &secret, endpoints.clone()).calendars(&there.url()).unwrap()[0].name, "Ada");
        // A client that isn't one: said before anything opens.
        let bad = olo_google::Client { id: "nope".into(), secret: "x".into() };
        assert!(olo_google::accounts::sign_in_at(&bad, None, &endpoints, &std::sync::atomic::AtomicBool::new(false), |_| panic!("nothing to open")).is_err());
        // Stopped while waiting; and one for an account known goes to it.
        let stop = std::sync::atomic::AtomicBool::new(true);
        let hinted = std::sync::Mutex::new(String::new());
        assert_eq!(olo_google::accounts::sign_in_at(&client, Some("ada@example.org"), &endpoints, &stop, |url| *hinted.lock().unwrap() = url.to_owned()).unwrap_err(), "stopped");
        assert!(hinted.into_inner().unwrap().contains("login_hint=ada%40example.org"));
    }
}
