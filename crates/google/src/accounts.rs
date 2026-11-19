//! The Google accounts Olo is connected to, and what each is used for
//! (Ben, 2026-11-18: "for each google workspace … we should be able to
//! add as many … as we want in prefs -> integrations and then choose
//! which ones are enabled for mail, calendar, diskbackup in like a
//! grid").
//!
//! An account is connected once (Preferences ▸ Integrations ▸ Google),
//! with an app of its own in its own Google Cloud project, called "Olo
//! (its address)": Google lets only a registered app sign someone in,
//! and an organisation's accounts are let into an app of its own where
//! another's is refused. One sign-in asks for everything Olo's services
//! use; which of them use the account is the person's to tick
//! ([`Service`]). Mail, Calendar and DiskBackup each read this list and
//! take their tokens from here: none keeps a sign-in of its own.
//!
//! The list is `~/.config/olo/google-accounts.json` (addresses,
//! services, what Google allowed: nothing secret); what signs an
//! account in (its app's client and the refresh token) is in the login
//! keyring.

use std::{
    io,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{Client, Endpoints, SignedIn};

/// What Olo does with a Google account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Mail,
    Calendar,
    DiskBackup,
}

impl Service {
    pub const ALL: [Service; 3] = [Service::Mail, Service::Calendar, Service::DiskBackup];

    pub fn name(self) -> &'static str {
        match self {
            Service::Mail => "Mail",
            Service::Calendar => "Calendar",
            Service::DiskBackup => "DiskBackup",
        }
    }

    /// What it asks of Google: its mail (IMAP and SMTP), its calendars
    /// (read and written), or only the files DiskBackup makes in its
    /// Drive.
    pub fn scope(self) -> &'static str {
        match self {
            Service::Mail => "https://mail.google.com/",
            Service::Calendar => "https://www.googleapis.com/auth/calendar",
            Service::DiskBackup => "https://www.googleapis.com/auth/drive.file",
        }
    }

    /// The API its app's project must have turned on: its name in
    /// Google Cloud, and the page that turns it on.
    pub fn api(self) -> (&'static str, &'static str) {
        match self {
            Service::Mail => ("Gmail API", "https://console.cloud.google.com/flows/enableapi?apiid=gmail.googleapis.com"),
            Service::Calendar => ("CalDAV API", "https://console.cloud.google.com/flows/enableapi?apiid=caldav.googleapis.com"),
            Service::DiskBackup => ("Google Drive API", "https://console.cloud.google.com/flows/enableapi?apiid=drive.googleapis.com"),
        }
    }

    /// At most one account serves it (DiskBackup backs up to one place).
    pub fn one_account(self) -> bool {
        self == Service::DiskBackup
    }
}

/// Everything a sign-in asks for: whose account it is, and each
/// service's scope.
pub fn scopes() -> String {
    std::iter::once("openid email").chain(Service::ALL.iter().map(|s| s.scope())).collect::<Vec<_>>().join(" ")
}

/// A Google account connected to Olo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub address: String,
    /// What it's used for (the person's ticks).
    pub services: Vec<Service>,
    /// What its sign-in was allowed (Google's scopes).
    pub scopes: Vec<String>,
}

impl Account {
    /// Its app's name in Google Cloud, and on Google's consent page:
    /// each account's its own, so several people of one organisation
    /// each have theirs.
    pub fn app_name(address: &str) -> String {
        format!("Olo ({})", address.trim())
    }

    /// The person ticked `service` for it.
    pub fn ticked(&self, service: Service) -> bool {
        self.services.contains(&service)
    }

    /// Its sign-in may do what `service` needs.
    pub fn allowed(&self, service: Service) -> bool {
        self.scopes.iter().any(|s| s == service.scope())
    }

    /// `service` uses it: ticked, and allowed.
    pub fn serves(&self, service: Service) -> bool {
        self.ticked(service) && self.allowed(service)
    }
}

/// Where the accounts are listed (`OLO_GOOGLE_ACCOUNTS`: another file, a
/// test's).
pub fn path() -> PathBuf {
    std::env::var_os("OLO_GOOGLE_ACCOUNTS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("google-accounts.json"))
}

/// The accounts connected, in the order they were added.
pub fn accounts() -> Vec<Account> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The accounts connected are these (written whole, then put in place:
/// whoever follows the file never reads half of it).
pub fn save(accounts: &[Account]) -> io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(accounts).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

/// The accounts `service` uses.
pub fn serving(service: Service) -> Vec<Account> {
    accounts().into_iter().filter(|a| a.serves(service)).collect()
}

/// `service` ticked or not for `address` among `accounts`: a service of
/// one account ([`Service::one_account`]) leaves the others.
pub fn tick(accounts: &mut [Account], address: &str, service: Service, on: bool) {
    for a in accounts.iter_mut() {
        let this = a.address.eq_ignore_ascii_case(address);
        if this && on && !a.ticked(service) {
            a.services.push(service);
            a.services.sort();
        } else if (this && !on) || (!this && on && service.one_account()) {
            a.services.retain(|s| *s != service);
        }
    }
}

/// What signs an account in: its own app's client, and the refresh
/// token Google gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Secret {
    pub client: Client,
    pub refresh_token: String,
}

fn attributes(address: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("service", "google"), ("account", address)]
}

/// A test's own keyring (`OLO_GOOGLE_SECRETS`: a folder, a file an
/// account), so nothing a test adds or removes touches the person's
/// login keyring, which a test session's programs can reach.
fn test_secret(address: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OLO_GOOGLE_SECRETS")?;
    let name: String = address.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '@') { c } else { '_' }).collect();
    Some(PathBuf::from(dir).join(format!("{name}.json")))
}

/// Keep what signs `address` in, in the login keyring. Blocking (a
/// D-Bus call): off the UI thread.
pub fn keep_secret(address: &str, secret: &Secret) -> Result<(), String> {
    let json = serde_json::to_vec(secret).unwrap_or_default();
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        return file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, json)).map_err(|e| e.to_string());
    }
    olo_system::secrets::store(&format!("Google: {address} (Olo)"), &attributes(&address), &json).map_err(|e| format!("the keyring can't keep it: {e}"))
}

/// What signs `address` in, if kept. Blocking.
pub fn secret(address: &str) -> Option<Secret> {
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        return serde_json::from_slice(&std::fs::read(file).ok()?).ok();
    }
    serde_json::from_slice(&olo_system::secrets::lookup(&attributes(&address)).ok()??).ok()
}

/// Forget it. Blocking.
pub fn forget_secret(address: &str) {
    let address = address.to_lowercase();
    if let Some(file) = test_secret(&address) {
        let _ = std::fs::remove_file(file);
        return;
    }
    let _ = olo_system::secrets::delete(&attributes(&address));
}

/// An account signed in with `client`: what signs it in kept, and the
/// account listed with what Google allowed. One there already keeps its
/// ticks (less what's no longer allowed); a new one is used for Mail
/// and Calendar where allowed (DiskBackup is the person's to move).
/// Blocking.
pub fn connected(client: &Client, signed: &SignedIn) -> Result<Account, String> {
    let address = signed.email.trim().to_lowercase();
    if address.is_empty() {
        return Err("Google didn't say whose account it is".into());
    }
    keep_secret(&address, &Secret { client: client.clone(), refresh_token: signed.refresh_token.clone() })?;
    let mut all = accounts();
    let now = match all.iter_mut().find(|a| a.address.eq_ignore_ascii_case(&address)) {
        Some(a) => {
            a.scopes = signed.scopes.clone();
            a.clone()
        }
        None => {
            let mut a = Account { address: address.clone(), services: Vec::new(), scopes: signed.scopes.clone() };
            a.services = [Service::Mail, Service::Calendar].into_iter().filter(|s| a.allowed(*s)).collect();
            all.push(a.clone());
            a
        }
    };
    save(&all).map_err(|e| format!("{}: {e}", path().display()))?;
    forget_token(&address);
    Ok(now)
}

/// `address` disconnected: off the list, what signs it in forgotten.
/// (Google isn't asked to revoke it: its app is the person's own, and
/// another computer may be signed in with it.) Blocking.
pub fn remove(address: &str) -> io::Result<()> {
    let mut all = accounts();
    all.retain(|a| !a.address.eq_ignore_ascii_case(address));
    save(&all)?;
    forget_secret(address);
    forget_token(address);
    Ok(())
}

/// Google's addresses (`OLO_GOOGLE_API`: a test's own Google, its
/// `/auth`, `/token` and APIs).
pub fn endpoints() -> Endpoints {
    match std::env::var("OLO_GOOGLE_API") {
        Ok(base) => Endpoints { auth: format!("{base}/auth"), token: format!("{base}/token"), api: base },
        Err(_) => Endpoints::default(),
    }
}

/// Access tokens in hand, by address: asked for with the refresh token
/// once and used while good (an hour, less a minute).
static TOKENS: Mutex<Vec<(String, String, Instant)>> = Mutex::new(Vec::new());

fn forget_token(address: &str) {
    TOKENS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(a, ..)| !a.eq_ignore_ascii_case(address));
}

/// Why no token could be had.
#[derive(Debug, Clone, PartialEq)]
pub enum NoToken {
    /// The account isn't connected here (or its sign-in isn't kept).
    NotConnected,
    /// Google couldn't be reached.
    Network(String),
    /// Google no longer takes its sign-in (revoked, the app gone): the
    /// account must be connected again.
    Refused(String),
}

impl std::fmt::Display for NoToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoToken::NotConnected => f.write_str("its sign-in isn't on this computer: sign in again in Preferences ▸ Integrations ▸ Google"),
            NoToken::Network(e) | NoToken::Refused(e) => f.write_str(e),
        }
    }
}

/// An access token for `address`: the one in hand while it's good,
/// else (or with `fresh`: a server just refused the one in hand) a new
/// one. `agent` names who asks ("Olo Calendar"). Blocking.
pub fn token(address: &str, fresh: bool, agent: &str) -> Result<String, NoToken> {
    token_at(address, fresh, agent, &endpoints())
}

/// [`token`], with Google at `endpoints`.
pub fn token_at(address: &str, fresh: bool, agent: &str, endpoints: &Endpoints) -> Result<String, NoToken> {
    if !fresh && let Some((_, token, _)) = TOKENS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(a, _, until)| a.eq_ignore_ascii_case(address) && Instant::now() < *until) {
        return Ok(token.clone());
    }
    let secret = secret(address).filter(|s| !s.refresh_token.is_empty()).ok_or(NoToken::NotConnected)?;
    let (token, lasts) =
        crate::access_token(&crate::agent(agent), endpoints, &secret.client, &secret.refresh_token).map_err(|e| if crate::unreachable(&e) { NoToken::Network(e) } else { NoToken::Refused(e) })?;
    let mut held = TOKENS.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|(a, ..)| !a.eq_ignore_ascii_case(address));
    held.push((address.to_lowercase(), token.clone(), Instant::now() + lasts.saturating_sub(Duration::from_secs(60))));
    Ok(token)
}

/// Whether `address`'s sign-in sees the Drive file `id` (the folder
/// DiskBackup backs up into): a file made with `drive.file` is seen only
/// by the app that made it, its Google Cloud project's (another
/// project's client, however it's named, sees nothing of it). Blocking.
pub fn sees_drive_file(address: &str, id: &str) -> Result<bool, String> {
    let token = token(address, false, "Olo").map_err(|e| e.to_string())?;
    let at = format!("{}/drive/v3/files/{}?fields=id&supportsAllDrives=true", endpoints().api, crate::escape(id));
    let answer = crate::agent("Olo").get(&at).header("Authorization", &format!("Bearer {token}")).call().map_err(|e| format!("{}: {e}", crate::UNREACHABLE))?;
    match answer.status().as_u16() {
        200 => Ok(true),
        404 => Ok(false),
        s => Err(format!("Google Drive answered {s}")),
    }
}

/// Sign in to Google with `client`, asking for everything Olo's
/// services use: `open` is given the consent page's address (for a
/// browser), Google's answer is waited for until `cancel` or 15
/// minutes. `address`: the account meant, where known. Blocking.
pub fn sign_in(client: &Client, address: Option<&str>, cancel: &std::sync::atomic::AtomicBool, open: impl FnOnce(&str)) -> Result<SignedIn, String> {
    sign_in_at(client, address, &endpoints(), cancel, open)
}

/// [`sign_in`], with Google at `endpoints`.
pub fn sign_in_at(client: &Client, address: Option<&str>, endpoints: &Endpoints, cancel: &std::sync::atomic::AtomicBool, open: impl FnOnce(&str)) -> Result<SignedIn, String> {
    client.check()?;
    let client = Client { id: client.id.trim().to_owned(), secret: client.secret.trim().to_owned() };
    let sign_in = crate::SignIn::start_as(&client, endpoints, &scopes(), "Olo", address).map_err(|e| e.to_string())?;
    open(sign_in.url());
    sign_in.wait(&crate::agent("Olo"), cancel)
}

/// Google's consent page at `url` opened in the person's browser.
/// (`OLO_GOOGLE_CONSENT=self`: fetched here instead, for a test whose
/// stand-in Google agrees at once; nothing opens.)
pub fn open_consent(url: &str) {
    if std::env::var("OLO_GOOGLE_CONSENT").is_ok_and(|v| v == "self") {
        let url = url.to_owned();
        std::thread::spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(4).timeout_global(Some(Duration::from_secs(20))).build().into();
            let _ = agent.get(&url).call();
        });
        return;
    }
    let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(address: &str, services: &[Service], scopes: &[&str]) -> Account {
        Account { address: address.into(), services: services.to_vec(), scopes: scopes.iter().map(|s| (*s).to_owned()).collect() }
    }

    /// What an account serves is what's ticked and allowed; DiskBackup
    /// is one account's; the list as it's kept says nothing secret.
    #[test]
    fn ticks_and_what_is_allowed() {
        let all: Vec<&str> = Service::ALL.iter().map(|s| s.scope()).collect();
        let mut list = vec![account("ada@example.com", &[Service::Mail], &all), account("ada@example.org", &[Service::DiskBackup], &[Service::DiskBackup.scope()])];
        assert!(list[0].serves(Service::Mail) && !list[0].serves(Service::Calendar));
        tick(&mut list, "ADA@example.com", Service::Calendar, true);
        tick(&mut list, "ada@example.com", Service::DiskBackup, true);
        assert_eq!(list[0].services, [Service::Mail, Service::Calendar, Service::DiskBackup]);
        assert_eq!(list[1].services, [], "DiskBackup moved: one account's");
        // Ticked but not allowed: not served, until signed in again.
        tick(&mut list, "ada@example.org", Service::Calendar, true);
        assert!(list[1].ticked(Service::Calendar) && !list[1].serves(Service::Calendar));
        tick(&mut list, "ada@example.com", Service::Mail, false);
        assert_eq!(list[0].services, [Service::Calendar, Service::DiskBackup]);
        assert_eq!(serde_json::to_string(&list[1]).unwrap(), r#"{"address":"ada@example.org","services":["calendar"],"scopes":["https://www.googleapis.com/auth/drive.file"]}"#);
        assert_eq!(Account::app_name(" ada@example.com "), "Olo (ada@example.com)");
        assert_eq!(scopes(), "openid email https://mail.google.com/ https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/drive.file");
    }
}
