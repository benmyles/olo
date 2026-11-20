//! Mail accounts, for one-time codes sent by mail (Ben, 2026-11-17:
//! "support for connecting any number of gmail (free or workspace gmail)
//! accounts as well as fastmail accounts ... the email one-time password
//! monitoring ... check the configured email accounts to see if a new
//! message has arrived with the code and keep looking for up to 60s").
//!
//! An account is its provider and address (`~/.config/olo/mail.json`,
//! nothing secret in it) and an app password kept in the login keyring
//! (as Olo's other secrets: `olo_system::secrets`). Gmail (a free account
//! or a Workspace one) and Fastmail both take IMAP with an app password:
//! Gmail's wants 2-Step Verification on, and a Workspace's admin may have
//! turned app passwords off. Olo reads the mail itself: TLS by rustls with
//! the system's certificates, IMAP here (LOGIN, SELECT, SEARCH, FETCH,
//! IDLE), MIME in [`mime`]; nothing is shelled out to. What's read is only
//! the newest messages' sender, subject and words, to look for a code in;
//! nothing of it is kept or logged.
//!
//! [`Session`]: signed in to an account's inbox. [`Watch`]: an account's
//! mail watched on a thread of its own for a while (what came in the last
//! few minutes, then each message as it comes: the inbox's at once, the
//! server telling, IDLE; where else mail lands, a rule's folder or the
//! junk, every few seconds), stopped when it's dropped. [`check`]: whether an account signs in
//! (Preferences ▸ Integrations, adding one). [`standin`]: a stand-in
//! server, for tests.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub mod mime;
pub mod standin;

pub use mime::{Message, parse};

/// Who keeps the mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gmail,
    Fastmail,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Gmail, Provider::Fastmail];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Gmail => "Gmail",
            Provider::Fastmail => "Fastmail",
        }
    }

    /// Its IMAP server (TLS, port 993).
    pub fn host(self) -> &'static str {
        match self {
            Provider::Gmail => "imap.gmail.com",
            Provider::Fastmail => "imap.fastmail.com",
        }
    }

    /// Where its app passwords are made.
    pub fn app_passwords(self) -> &'static str {
        match self {
            Provider::Gmail => "https://myaccount.google.com/apppasswords",
            // (Privacy & Security: Manage app passwords and access.)
            Provider::Fastmail => "https://app.fastmail.com/settings/security",
        }
    }
}

/// A mail account Olo reads for codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub provider: Provider,
    pub address: String,
}

/// Where the accounts are listed (`OLO_MAIL_ACCOUNTS`: another file, a
/// test's).
pub fn accounts_path() -> PathBuf {
    std::env::var_os("OLO_MAIL_ACCOUNTS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("mail.json"))
}

/// The accounts added here with an app password, in the order they
/// were added (what `mail.json` lists).
pub fn own_accounts() -> Vec<Account> {
    std::fs::read(accounts_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Every account mail is read from: the Google accounts connected to
/// Olo that are ticked for Mail (Preferences ▸ Integrations ▸ Google:
/// they sign in through the account's own app, `olo_google::accounts`),
/// the Fastmail accounts ticked for it (Integrations ▸ Fastmail,
/// `olo_fastmail`), then any added here before those were (Gmail's with
/// an app password).
pub fn accounts() -> Vec<Account> {
    // (A test's own list: this computer's accounts elsewhere aren't in it,
    // unless it names a list of theirs too.)
    let test = std::env::var_os("OLO_MAIL_ACCOUNTS").is_some();
    let mut all: Vec<Account> = if test && std::env::var_os("OLO_GOOGLE_ACCOUNTS").is_none() {
        Vec::new()
    } else {
        olo_google::accounts::serving(olo_google::accounts::Service::Mail).into_iter().map(|a| Account { provider: Provider::Gmail, address: a.address }).collect()
    };
    if !test || std::env::var_os("OLO_GOOGLE_ACCOUNTS").is_some() {
        drop_gmail_under_google();
    }
    if !test || std::env::var_os("OLO_FASTMAIL_ACCOUNTS").is_some() {
        adopt_fastmail();
        all.extend(olo_fastmail::serving(olo_fastmail::Service::Mail).into_iter().map(|a| Account { provider: Provider::Fastmail, address: a.address }));
    }
    merged(all, own_accounts())
}

/// The Gmail accounts added here with an app password before Google
/// accounts were connected to Olo, let go of once a Google account of the
/// address is (its ticks under Google say whether Mail uses it: one left
/// here, which no tab shows, would read its mail with the old app
/// password still, unticked). Their app passwords go too. Once a program.
fn drop_gmail_under_google() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let google: Vec<String> = olo_google::accounts::accounts().into_iter().map(|a| a.address).collect();
    let own = own_accounts();
    let under = |a: &Account| a.provider == Provider::Gmail && google.iter().any(|g| g.eq_ignore_ascii_case(&a.address));
    if !own.iter().any(under) {
        return;
    }
    let kept: Vec<Account> = own.iter().filter(|a| !under(a)).cloned().collect();
    match save_accounts(&kept) {
        Ok(()) => {
            for a in own.iter().filter(|a| under(a)) {
                if !kept.iter().any(|b| b.address.eq_ignore_ascii_case(&a.address)) {
                    forget_password(&a.address);
                }
                tracing::info!("mail: the Gmail account {} is Integrations ▸ Google's now; its app password is let go of", a.address);
            }
        }
        Err(e) => tracing::warn!("mail: the Gmail accounts under Google stay listed here too ({e})"),
    }
}

/// The Fastmail accounts Mail kept itself before Integrations had a
/// Fastmail tab (in its own list, their app passwords under Mail's name)
/// moved to `olo_fastmail`'s, ticked for Mail, passwords and all: an
/// upgrade carries them over by itself (Ben, 2026-11-18: "make sure it
/// auto migrates the existing storage/creds on upgrade"). Each is let go
/// of here only once it's there. Once a program, until done (a keyring
/// not unlocked yet leaves them for the next try).
fn adopt_fastmail() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Acquire) {
        return;
    }
    let theirs: Vec<Account> = own_accounts().into_iter().filter(|a| a.provider == Provider::Fastmail).collect();
    let mut moved = Vec::new();
    for a in &theirs {
        let kept = match kept_password(&a.address) {
            Ok(v) => v,
            Err(e) => {
                tracing::info!("mail: the Fastmail account {} is moved later: its password can't be read now ({e})", a.address);
                continue;
            }
        };
        match olo_fastmail::adopt(&a.address, olo_fastmail::Service::Mail, kept.as_deref()) {
            Ok(()) => moved.push(a.clone()),
            Err(e) => tracing::warn!("mail: the Fastmail account {} isn't moved: {e}", a.address),
        }
    }
    if !moved.is_empty() {
        let mut listed = own_accounts();
        listed.retain(|a| !moved.contains(a));
        match save_accounts(&listed) {
            Ok(()) => {
                for a in &moved {
                    if !listed.iter().any(|b| b.address.eq_ignore_ascii_case(&a.address)) {
                        forget_password(&a.address);
                    }
                    tracing::info!("mail: the Fastmail account {} moved to Integrations ▸ Fastmail", a.address);
                }
            }
            Err(e) => tracing::warn!("mail: the Fastmail accounts moved stay listed here too ({e})"),
        }
    }
    if moved.len() == theirs.len() {
        DONE.store(true, Ordering::Release);
    }
}

/// The Google accounts' and those added here, as one list. One added here
/// is left out only where it's the same mailbox as a Google account's: a
/// Gmail account now connected under Google. Another provider's at the
/// same address is a mailbox of its own, and listed: a domain's mail at
/// Fastmail, its Google account kept for Drive and Calendar. (Left out by
/// its address alone, Ben's Fastmail account went from every list, and
/// its mail was looked for at Google, once his Google account of that
/// address was ticked for Mail, 2026-11-18.)
fn merged(mut all: Vec<Account>, own: Vec<Account>) -> Vec<Account> {
    for a in own {
        let under_google = a.provider == Provider::Gmail && all.iter().any(|g| g.address.eq_ignore_ascii_case(&a.address));
        if !under_google && !all.contains(&a) {
            all.push(a);
        }
    }
    all
}

/// The files that say which accounts there are: followed by whoever
/// shows or watches them.
pub fn watched() -> Vec<PathBuf> {
    vec![accounts_path(), olo_google::accounts::path(), olo_fastmail::path()]
}

/// What signs an account in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// An app password.
    Password(String),
    /// A Google access token (`AUTHENTICATE XOAUTH2`).
    Token(String),
}

impl From<&str> for Auth {
    fn from(password: &str) -> Auth {
        Auth::Password(password.to_owned())
    }
}

impl From<&String> for Auth {
    fn from(password: &String) -> Auth {
        Auth::Password(password.clone())
    }
}

impl From<&Auth> for Auth {
    fn from(auth: &Auth) -> Auth {
        auth.clone()
    }
}

/// What signs `account` in now: a Google account connected to Olo and
/// ticked for Mail, a token from its sign-in; else its app password
/// from the keyring. Err: neither, in words. Blocking.
pub fn auth_of(account: &Account) -> Result<Auth, String> {
    use olo_google::accounts::{self, Service};
    if account.provider == Provider::Gmail
        && std::env::var_os("OLO_MAIL_IMAP").is_none()
        && accounts::accounts().iter().any(|a| a.address.eq_ignore_ascii_case(&account.address) && a.serves(Service::Mail))
    {
        return accounts::token(&account.address, false, "Olo Mail").map(Auth::Token).map_err(|e| e.to_string());
    }
    // (A Fastmail account's, as Integrations ▸ Fastmail keeps it.)
    if account.provider == Provider::Fastmail
        && std::env::var_os("OLO_MAIL_IMAP").is_none()
        && let Some(p) = olo_fastmail::password(&account.address)
    {
        return Ok(Auth::Password(p));
    }
    password(&account.address).map(Auth::Password).ok_or_else(|| "no app password in the keyring".to_owned())
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

/// The keyring attributes of an account's app password.
fn attributes(address: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("service", "mail"), ("account", address)]
}

/// A test's own keyring (`OLO_MAIL_SECRETS`: a folder, a file an
/// address), so nothing a test adds or removes touches the person's
/// login keyring, which a test session's programs can reach.
fn test_password(address: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OLO_MAIL_SECRETS")?;
    let name: String = address.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '@') { c } else { '_' }).collect();
    Some(PathBuf::from(dir).join(name))
}

/// `address`'s app password as kept (Err: the keyring couldn't be read).
fn kept_password(address: &str) -> Result<Option<String>, String> {
    if let Some(file) = test_password(address) {
        return Ok(std::fs::read_to_string(file).ok());
    }
    Ok(olo_system::secrets::lookup(&attributes(address)).map_err(|e| e.to_string())?.and_then(|b| String::from_utf8(b).ok()))
}

/// Keep `password` for `address` in the login keyring. Blocking (a D-Bus
/// call): off the UI thread.
pub fn keep_password(address: &str, password: &str) -> Result<(), String> {
    if let Some(file) = test_password(address) {
        return file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, password)).map_err(|e| e.to_string());
    }
    let label = format!("Mail: {address} (Olo)");
    olo_system::secrets::store(&label, &attributes(address), password.as_bytes()).map_err(|e| e.to_string())
}

/// `address`'s app password, if kept. Blocking. (Against a test's
/// stand-in server, `OLO_MAIL_IMAP`, its own.)
pub fn password(address: &str) -> Option<String> {
    if std::env::var_os("OLO_MAIL_IMAP").is_some() {
        return Some(standin::PASSWORD.into());
    }
    kept_password(address).ok().flatten().filter(|p| !p.is_empty())
}

/// Forget `address`'s app password. Blocking.
pub fn forget_password(address: &str) {
    if let Some(file) = test_password(address) {
        let _ = std::fs::remove_file(file);
        return;
    }
    let _ = olo_system::secrets::delete(&attributes(address));
}

/// How long the server has to answer anything (an idle wait aside).
const PATIENCE: Duration = Duration::from_secs(20);

/// Where a session connects: the provider's server (TLS), or a test's
/// stand-in (plain, on loopback).
#[derive(Debug, Clone)]
pub enum Server {
    Tls(&'static str),
    Plain(String),
}

impl Server {
    /// The account's provider's server, or the stand-in a test names
    /// (`OLO_MAIL_IMAP`, `HOST:PORT`).
    pub fn of(account: &Account) -> Server {
        match std::env::var("OLO_MAIL_IMAP").ok().filter(|v| !v.is_empty()) {
            Some(at) => Server::Plain(at),
            None => Server::Tls(account.provider.host()),
        }
    }
}

/// The way to the server: TLS, or plain (a stand-in).
enum Stream {
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    Plain(TcpStream),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tls(s) => s.read(buf),
            Stream::Plain(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tls(s) => s.write(buf),
            Stream::Plain(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tls(s) => s.flush(),
            Stream::Plain(s) => s.flush(),
        }
    }
}

/// One untagged response: its line's words, and the literals in it (a
/// message's bytes) in order.
#[derive(Debug, Default, Clone)]
struct Untagged {
    text: String,
    literals: Vec<Vec<u8>>,
}

/// Signed in to an account's inbox.
pub struct Session {
    stream: BufReader<Stream>,
    /// The connection itself: its waits, and stopping it from elsewhere.
    socket: TcpStream,
    tag: u32,
    /// The UID the next message to come will have: what's new is from
    /// here (moved on as messages are read).
    uid_next: u32,
    /// How many messages the inbox has.
    exists: u32,
    /// The server waits with us for what comes (IDLE).
    idle: bool,
    /// It lists its mailboxes with what each has, in one asking
    /// (LIST-STATUS, RFC 5819).
    list_status: bool,
    /// What's come of a line not ended yet (a wait that ran out in it).
    partial: Vec<u8>,
}

/// The most mailboxes besides the inbox a watch looks in.
pub const ELSEWHERE_MOST: usize = 30;
/// How often a watch looks in them, between what the inbox's server
/// tells at once.
pub const ELSEWHERE_EVERY: Duration = Duration::from_secs(5);

/// TLS as the system's certificates have it.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| e.to_string())?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A string as IMAP takes it: quoted, `"` and `\` escaped.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Connect to `host:port` within `PATIENCE`, each of its addresses tried.
fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| format!("{host} couldn’t be found ({e})"))?;
    let mut last = format!("{host} couldn’t be found");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, PATIENCE) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("{host} couldn’t be reached ({e})"),
        }
    }
    Err(last)
}

fn timed_out(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

impl Session {
    /// Sign in to `account` with `auth` (its app password: spaces, as
    /// Google shows it in fours, left out; or a Google token) and open
    /// its inbox.
    pub fn open(account: &Account, auth: impl Into<Auth>) -> Result<Session, String> {
        Session::open_at(&Server::of(account), account, auth)
    }

    /// [`Session::open`] at `server`.
    pub fn open_at(server: &Server, account: &Account, auth: impl Into<Auth>) -> Result<Session, String> {
        let mut s = Session::greeted(server)?;
        match auth.into() {
            Auth::Password(password) => {
                let pass: String = password.chars().filter(|c| !c.is_whitespace()).collect();
                s.command(&format!("LOGIN {} {}", quoted(&account.address), quoted(&pass)))
                    .map_err(|e| if e.starts_with("NO") || e.starts_with("BAD") { format!("{} didn’t take the address and app password ({})", account.provider.name(), e.trim()) } else { e })?;
            }
            // (RFC 7628's forerunner, as Google has it: the address and
            // the token in one go. Refused, the server says why in a
            // line of its own first, answered with an empty one:
            // `command` does.)
            Auth::Token(token) => {
                use base64::Engine as _;
                let said = base64::engine::general_purpose::STANDARD.encode(format!("user={}\x01auth=Bearer {token}\x01\x01", account.address));
                s.command(&format!("AUTHENTICATE XOAUTH2 {said}")).map_err(|e| {
                    if e.starts_with("NO") || e.starts_with("BAD") {
                        format!("{} didn’t take the account’s sign-in ({}): is Mail allowed for it, and IMAP turned on in its Gmail settings?", account.provider.name(), e.trim())
                    } else {
                        e
                    }
                })?;
            }
        }
        let caps = s.command("CAPABILITY")?;
        let can = |what: &str| caps.iter().any(|c| c.text.split_whitespace().any(|w| w.eq_ignore_ascii_case(what)));
        (s.idle, s.list_status) = (can("IDLE"), can("LIST-STATUS"));
        s.select_inbox()?;
        Ok(s)
    }

    /// The inbox opened (again), to watch: how many it has, and the UID
    /// the next will have.
    pub fn select_inbox(&mut self) -> Result<(), String> {
        let answers = self.command("SELECT INBOX")?;
        self.exists = answers.iter().find_map(|a| a.text.strip_prefix("* ").and_then(|r| r.strip_suffix(" EXISTS")).and_then(|n| n.trim().parse().ok())).unwrap_or(0);
        self.uid_next = answers.iter().find_map(|a| bracket_number(&a.text, "UIDNEXT")).unwrap_or(0);
        if self.uid_next == 0 {
            // (A server that doesn't say: the last one there, plus one.)
            let found = self.command("UID SEARCH ALL")?;
            self.uid_next = found.iter().filter_map(|a| a.text.strip_prefix("* SEARCH")).flat_map(|r| r.split_whitespace().filter_map(|n| n.parse::<u32>().ok())).max().map_or(1, |m| m + 1);
        }
        Ok(())
    }

    /// Where else than the inbox a message may land, to watch beside it
    /// (Ben's mailed code that a rule filed away, 2026-11-18): Gmail's All
    /// Mail (every label's, an archived one's) and its spam; another's
    /// every mailbox but the inbox, sent mail, drafts and the trash, its
    /// junk and archive first. [`ELSEWHERE_MOST`] at most.
    pub fn elsewhere(&mut self, provider: Provider) -> Result<Vec<String>, String> {
        let has = |flags: &[String], f: &str| flags.iter().any(|x| x.eq_ignore_ascii_case(f));
        let mut boxes: Vec<(u8, String)> = self
            .mailboxes()?
            .into_iter()
            .filter(|(name, flags)| {
                !name.eq_ignore_ascii_case("INBOX")
                    && match provider {
                        Provider::Gmail => has(flags, "\\All") || has(flags, "\\Junk"),
                        Provider::Fastmail => !["\\Sent", "\\Drafts", "\\Trash", "\\All", "\\Flagged", "\\Important"].iter().any(|f| has(flags, f)),
                    }
            })
            .map(|(name, flags)| {
                (
                    if has(&flags, "\\Junk") {
                        0
                    } else if has(&flags, "\\Archive") || has(&flags, "\\All") {
                        1
                    } else {
                        2
                    },
                    name,
                )
            })
            .collect();
        boxes.sort_by_key(|(first, _)| *first);
        Ok(boxes.into_iter().take(ELSEWHERE_MOST).map(|(_, name)| name).collect())
    }

    /// The UID the next message of each of `boxes` will have, as each is
    /// now: in one asking where the server lists with what each has
    /// (LIST-STATUS), else each asked (STATUS). A mailbox gone is left
    /// out.
    pub fn uid_nexts(&mut self, boxes: &[String]) -> Result<Vec<(String, u32)>, String> {
        let status = |text: &str| -> Option<(String, u32)> {
            let rest = text.strip_prefix("* STATUS ")?;
            let (name, rest) = if let Some(q) = rest.strip_prefix('"') {
                let (n, r) = q.split_once("\" (")?;
                (n.replace("\\\"", "\"").replace("\\\\", "\\"), r)
            } else {
                let (n, r) = rest.split_once(" (")?;
                (n.to_owned(), r)
            };
            Some((name, bracket_free_number(rest, "UIDNEXT")?))
        };
        let answers = if self.list_status {
            self.command("LIST \"\" \"*\" RETURN (STATUS (UIDNEXT))")?
        } else {
            let mut all = Vec::new();
            for b in boxes {
                // (One gone since it was listed: the rest still asked.)
                if let Ok(a) = self.command(&format!("STATUS {} (UIDNEXT)", quoted(b))) {
                    all.extend(a);
                }
            }
            all
        };
        Ok(answers.iter().filter_map(|a| status(&a.text)).filter(|(name, _)| boxes.contains(name)).collect())
    }

    /// Connected to `server`, and greeted (not signed in).
    fn greeted(server: &Server) -> Result<Session, String> {
        let (socket, raw) = match server {
            Server::Plain(at) => {
                let (host, port) = at.rsplit_once(':').ok_or("no port")?;
                let socket = connect(host, port.parse().map_err(|_| "no port")?)?;
                (socket.try_clone().map_err(|e| e.to_string())?, Stream::Plain(socket))
            }
            Server::Tls(host) => {
                let socket = connect(host, 993)?;
                socket.set_read_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
                let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
                let mut session = rustls::ClientConnection::new(tls_config()?, name).map_err(|e| e.to_string())?;
                let mut sock = socket.try_clone().map_err(|e| e.to_string())?;
                while session.is_handshaking() {
                    session.complete_io(&mut sock).map_err(|e| format!("{host}: {e}"))?;
                }
                (sock, Stream::Tls(Box::new(rustls::StreamOwned::new(session, socket))))
            }
        };
        socket.set_read_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
        socket.set_write_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
        let mut s = Session { stream: BufReader::new(raw), socket, tag: 0, uid_next: 0, exists: 0, idle: false, list_status: false, partial: Vec::new() };
        let greeting = s.line().map_err(|e| format!("the server didn’t greet ({e})"))?;
        if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
            return Err(format!("the server didn’t greet: {}", greeting.trim()));
        }
        Ok(s)
    }

    /// The UID the next message will have.
    pub fn uid_next(&self) -> u32 {
        self.uid_next
    }

    /// A handle that ends this session from another thread (what it's
    /// waiting on returns at once).
    pub fn stopper(&self) -> Option<TcpStream> {
        self.socket.try_clone().ok()
    }

    /// The messages that came within `within` of now (of the newest five
    /// of the mailbox open), oldest first.
    pub fn recent(&mut self, within: Duration) -> Result<Vec<(u32, Message)>, String> {
        if self.exists == 0 {
            return Ok(Vec::new());
        }
        let first = self.exists.saturating_sub(4).max(1);
        let dates = self.command(&format!("FETCH {first}:{} (UID INTERNALDATE)", self.exists))?;
        let since = now().saturating_sub(within.as_secs());
        let uids: Vec<u32> = dates
            .iter()
            .filter(|f| f.text.contains("FETCH"))
            .filter_map(|f| Some((word_after(&f.text, "UID")?.parse::<u32>().ok()?, internal_date(&f.text)?)))
            .filter(|(_, at)| *at >= since)
            .map(|(uid, _)| uid)
            .collect();
        self.fetch(&uids)
    }

    /// The messages that came since the last read (or since signing in,
    /// or since [`Session::read_from`]), oldest first, each read whole up
    /// to 64 KB (a code is in its first lines).
    pub fn new_messages(&mut self) -> Result<Vec<(u32, Message)>, String> {
        let from = self.uid_next.max(1);
        let found = self.command(&format!("UID SEARCH UID {from}:*"))?;
        // (`N:*` always has the last message, new or not.)
        let uids: Vec<u32> = found.iter().filter_map(|a| a.text.strip_prefix("* SEARCH")).flat_map(|r| r.split_whitespace().filter_map(|n| n.parse::<u32>().ok())).filter(|&u| u >= from).collect();
        let got = self.fetch(&uids)?;
        if let Some(last) = uids.iter().max() {
            self.uid_next = self.uid_next.max(last + 1);
        }
        Ok(got)
    }

    /// What [`Session::new_messages`] reads next: from UID `uid` on (the
    /// inbox again after looking elsewhere, from where it had got to).
    pub fn read_from(&mut self, uid: u32) {
        self.uid_next = uid;
    }

    /// The account's mailboxes that can be opened: each one's name as
    /// the server writes it (for [`Session::look_in`]) and what it says
    /// of it (`\\All`, `\\Archive`, `\\Sent`, `\\Trash`, `\\Junk`,
    /// `\\Drafts`, where the server marks them).
    pub fn mailboxes(&mut self) -> Result<Vec<(String, Vec<String>)>, String> {
        let listed = self.command("LIST \"\" \"*\"")?;
        let mut out = Vec::new();
        for l in listed {
            let Some(rest) = l.text.strip_prefix("* LIST (") else { continue };
            let Some((flags, rest)) = rest.split_once(')') else { continue };
            let flags: Vec<String> = flags.split_whitespace().map(str::to_owned).collect();
            if flags.iter().any(|f| f.eq_ignore_ascii_case("\\Noselect") || f.eq_ignore_ascii_case("\\NonExistent")) {
                continue;
            }
            // (`"/" "Name"`, `"/" Name`, or the name as a literal.)
            let rest = rest.trim_start();
            let after_delimiter = match rest.strip_prefix('"') {
                Some(r) => r.split_once('"').map_or("", |(_, r)| r),
                None => rest.split_once(' ').map_or("", |(_, r)| r),
            }
            .trim();
            let name = if let Some(raw) = l.literals.first() {
                String::from_utf8_lossy(raw).into_owned()
            } else if let Some(q) = after_delimiter.strip_prefix('"') {
                q.strip_suffix('"').unwrap_or(q).replace("\\\"", "\"").replace("\\\\", "\\")
            } else {
                after_delimiter.to_owned()
            };
            if !name.is_empty() {
                out.push((name, flags));
            }
        }
        Ok(out)
    }

    /// Look in `mailbox` from here on, read only (nothing is marked as
    /// read, nothing can be changed): how many messages it has.
    pub fn look_in(&mut self, mailbox: &str) -> Result<u32, String> {
        let answers = self.command(&format!("EXAMINE {}", quoted(mailbox)))?;
        self.exists = answers.iter().find_map(|a| a.text.strip_prefix("* ").and_then(|r| r.strip_suffix(" EXISTS")).and_then(|n| n.trim().parse().ok())).unwrap_or(0);
        self.uid_next = answers.iter().find_map(|a| bracket_number(&a.text, "UIDNEXT")).unwrap_or(0);
        Ok(self.exists)
    }

    /// The messages of the mailbox looked in that match `criteria`
    /// (IMAP's own search keys: `SUBJECT "code" SINCE 1-Jan-2026`), by
    /// UID, oldest first.
    pub fn search(&mut self, criteria: &str) -> Result<Vec<u32>, String> {
        let found = self.command(&format!("UID SEARCH {criteria}"))?;
        let mut uids: Vec<u32> = found.iter().filter_map(|a| a.text.strip_prefix("* SEARCH")).flat_map(|r| r.split_whitespace().filter_map(|n| n.parse::<u32>().ok())).collect();
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    /// Messages `uids` of the mailbox looked in, read (each up to 64 KB),
    /// oldest first.
    pub fn read(&mut self, uids: &[u32]) -> Result<Vec<(u32, Message)>, String> {
        self.fetch(uids)
    }

    /// Messages `uids`, read.
    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<(u32, Message)>, String> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let fetched = self.command(&format!("UID FETCH {set} (UID BODY.PEEK[]<0.65536>)"))?;
        let mut out = Vec::new();
        for f in fetched.into_iter().filter(|f| f.text.contains("FETCH")) {
            let Some(uid) = word_after(&f.text, "UID").and_then(|u| u.parse().ok()) else { continue };
            if let Some(raw) = f.literals.first() {
                out.push((uid, parse(raw)));
            }
        }
        out.sort_by_key(|(u, _)| *u);
        Ok(out)
    }

    /// Wait for something to come, until `until`: true when a message
    /// may have (the server said so, or one without IDLE is to be asked
    /// again), false when the time is up.
    pub fn wait(&mut self, until: Instant) -> Result<bool, String> {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        if !self.idle {
            // Asked again every five seconds: a wait on the connection,
            // which a stop ends.
            let pause = left.min(Duration::from_secs(5));
            self.socket.set_read_timeout(Some(pause)).map_err(|e| e.to_string())?;
            let r = self.line();
            self.socket.set_read_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
            return match r {
                Err(e) if timed_out(&e) => {
                    self.command("NOOP")?;
                    Ok(true)
                }
                Err(e) => Err(e.to_string()),
                Ok(_) => Ok(true),
            };
        }
        self.tag += 1;
        let tag = format!("o{}", self.tag);
        self.send(&format!("{tag} IDLE"))?;
        let go_on = self.line().map_err(|e| e.to_string())?;
        if !go_on.starts_with('+') {
            // (It said it doesn't, after all: asked again from now on.)
            self.idle = false;
            return Ok(true);
        }
        let mut came = false;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.socket.set_read_timeout(Some(left.min(Duration::from_secs(25 * 60)))).map_err(|e| e.to_string())?;
            match self.line() {
                Ok(l) if l.trim_end().ends_with("EXISTS") => {
                    came = true;
                    break;
                }
                Ok(l) if l.starts_with("* BYE") => return Err("the server ended the connection".into()),
                Ok(_) => {}
                Err(e) if timed_out(&e) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        self.socket.set_read_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
        self.send("DONE")?;
        loop {
            let l = self.line().map_err(|e| e.to_string())?;
            if l.starts_with(&format!("{tag} ")) {
                break;
            }
            if l.trim_end().ends_with("EXISTS") {
                came = true;
            }
        }
        Ok(came)
    }

    /// Signed out.
    pub fn close(mut self) {
        let _ = self.command("LOGOUT");
    }

    fn send(&mut self, line: &str) -> Result<(), String> {
        let stream = self.stream.get_mut();
        stream.write_all(format!("{line}\r\n").as_bytes()).map_err(|e| format!("couldn’t write to the server ({e})"))?;
        stream.flush().map_err(|e| e.to_string())
    }

    /// One line from the server (up to its CRLF); a wait that runs out
    /// keeps what had come of it.
    fn line(&mut self) -> io::Result<String> {
        match self.stream.read_until(b'\n', &mut self.partial) {
            Ok(_) if self.partial.ends_with(b"\n") => {
                let line = std::mem::take(&mut self.partial);
                Ok(String::from_utf8_lossy(&line).into_owned())
            }
            Ok(_) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection")),
            Err(e) => Err(e),
        }
    }

    /// Send `cmd`, and read until its tagged answer: the untagged ones
    /// before it, or what was wrong.
    fn command(&mut self, cmd: &str) -> Result<Vec<Untagged>, String> {
        self.tag += 1;
        let tag = format!("o{}", self.tag);
        self.send(&format!("{tag} {cmd}"))?;
        let mut answers = Vec::new();
        let lost = |e: io::Error| format!("the server stopped answering ({e})");
        loop {
            let mut line = self.line().map_err(lost)?;
            // (The server has more to say before it answers, and waits
            // for a line: a sign-in refused, saying why. An empty one.)
            if line.starts_with("+ ") || line.trim_end() == "+" {
                self.send("")?;
                continue;
            }
            if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
                let rest = rest.trim_end().to_owned();
                if rest.starts_with("OK") {
                    return Ok(answers);
                }
                return Err(rest);
            }
            let mut one = Untagged::default();
            // A line ending "{N}": N bytes follow, then the line goes on.
            loop {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                match literal_size(trimmed) {
                    Some(n) => {
                        one.text.push_str(trimmed);
                        let mut bytes = vec![0u8; n];
                        self.stream.read_exact(&mut bytes).map_err(lost)?;
                        one.literals.push(bytes);
                        line = self.line().map_err(lost)?;
                    }
                    None => {
                        one.text.push_str(trimmed);
                        break;
                    }
                }
            }
            if one.text.starts_with("* ") && one.text.ends_with(" EXISTS") {
                self.exists = one.text[2..one.text.len() - 7].trim().parse().unwrap_or(self.exists);
            }
            answers.push(one);
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The size of a literal a line ends with ("… {1234}"), if it does.
fn literal_size(line: &str) -> Option<usize> {
    let open = line.rfind('{')?;
    line.ends_with('}').then(|| line[open + 1..line.len() - 1].trim_end_matches('+').parse().ok()).flatten()
}

/// The number in "[KEY n]" of a line.
fn bracket_number(line: &str, key: &str) -> Option<u32> {
    let at = line.find(&format!("[{key} "))? + key.len() + 2;
    line[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// The number after `key` in a STATUS's list ("MESSAGES 3 UIDNEXT 42)").
fn bracket_free_number(text: &str, key: &str) -> Option<u32> {
    word_after(text, key)?.parse().ok()
}

/// The word after `key` in a line ("UID 42").
fn word_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut words = line.split(|c: char| c.is_whitespace() || c == '(' || c == ')');
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case(key) {
            return words.find(|w| !w.is_empty());
        }
    }
    None
}

/// A FETCH's INTERNALDATE ("17-Nov-2026 13:45:02 +0000"), in Unix
/// seconds.
fn internal_date(line: &str) -> Option<u64> {
    let at = line.find("INTERNALDATE \"")? + 14;
    let text = &line[at..at + line[at..].find('"')?];
    let (date, rest) = text.trim().split_once(' ')?;
    let (time, zone) = rest.trim().split_once(' ')?;
    let mut d = date.split('-');
    let day: i64 = d.next()?.trim().parse().ok()?;
    let month_name = d.next()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| month_name.eq_ignore_ascii_case(m))? as i64 + 1;
    let year: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (t.next()??, t.next()??, t.next()??);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z: i64 = zone.trim_start_matches(['+', '-']).parse().ok()?;
    let offset = sign * ((z / 100) * 3600 + (z % 100) * 60);
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + s - offset).ok()
}

/// Whether `account` signs in with `auth` (Preferences, adding it):
/// signed in and its inbox opened, then signed out. Blocking.
pub fn check(account: &Account, auth: impl Into<Auth>) -> Result<(), String> {
    Session::open(account, auth).map(Session::close)
}

/// An account's mail watched on a thread of its own (named "mail"): the
/// messages of the last `recent`, then each as it comes, until its time
/// is up or it's stopped (dropped): the inbox's, the server telling at
/// once (IDLE), and those of where else mail lands ([`Session::elsewhere`],
/// a rule's folder, the junk), looked at every [`ELSEWHERE_EVERY`]. Each
/// message read goes to `found` with a key of its own (its Message-ID,
/// else its mailbox and UID: found in two mailboxes, the same key), which
/// says whether to go on.
pub struct Watch {
    shared: Arc<WatchShared>,
}

struct WatchShared {
    until: Mutex<Instant>,
    stopped: AtomicBool,
    done: AtomicBool,
    /// The connection, while there is one: shut to stop it.
    socket: Mutex<Option<TcpStream>>,
}

impl WatchShared {
    fn new(until: Instant) -> Arc<WatchShared> {
        Arc::new(WatchShared { until: Mutex::new(until), stopped: AtomicBool::new(false), done: AtomicBool::new(false), socket: Mutex::new(None) })
    }
}

impl Watch {
    pub fn start(account: Account, recent: Duration, until: Instant, mut found: impl FnMut(String, Message) -> bool + Send + 'static) -> Watch {
        let shared = WatchShared::new(until);
        let sh = shared.clone();
        let spawned = std::thread::Builder::new().name("mail".into()).spawn(move || {
            if let Err(e) = watch(&account, recent, &sh, &mut found) {
                // (What went wrong, never what the mail said.)
                if !sh.stopped.load(Ordering::SeqCst) {
                    tracing::info!("mail: {}: {e}", account.provider.name());
                }
            }
            sh.done.store(true, Ordering::SeqCst);
        });
        if spawned.is_err() {
            shared.done.store(true, Ordering::SeqCst);
        }
        Watch { shared }
    }

    /// Watch until `until` now, if that's later.
    pub fn extend(&self, until: Instant) {
        let mut u = self.shared.until.lock().unwrap_or_else(|e| e.into_inner());
        *u = (*u).max(until);
    }

    /// It's still watching.
    pub fn running(&self) -> bool {
        !self.shared.done.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(s) = self.shared.socket.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

fn watch(account: &Account, recent: Duration, sh: &WatchShared, found: &mut dyn FnMut(String, Message) -> bool) -> Result<(), String> {
    let stopped = || sh.stopped.load(Ordering::SeqCst);
    let password = auth_of(account)?;
    if stopped() {
        return Ok(());
    }
    let s = Session::open(account, &password)?;
    *sh.socket.lock().unwrap_or_else(|e| e.into_inner()) = s.stopper();
    // (Stopped while it signed in: the stop had no connection to shut.)
    if stopped() {
        return Ok(());
    }
    watch_in(s, account.provider, recent, sh, found)
}

/// A message's key: its Message-ID, else where it is.
fn key_of(mailbox: &str, uid: u32, m: &Message) -> String {
    if m.id.is_empty() { format!("{mailbox}/{uid}") } else { m.id.clone() }
}

/// [`watch`], signed in.
fn watch_in(mut s: Session, provider: Provider, recent: Duration, sh: &WatchShared, found: &mut dyn FnMut(String, Message) -> bool) -> Result<(), String> {
    let stopped = || sh.stopped.load(Ordering::SeqCst);
    let until = || *sh.until.lock().unwrap_or_else(|e| e.into_inner());
    let mut give = |s: Session, mailbox: &str, got: Vec<(u32, Message)>| -> Result<Session, ()> {
        for (uid, m) in got {
            if !found(key_of(mailbox, uid, &m), m) {
                s.close();
                return Err(());
            }
        }
        Ok(s)
    };
    let got = s.recent(recent)?;
    let Ok(s2) = give(s, "INBOX", got) else { return Ok(()) };
    s = s2;
    // Where else mail lands, and what came there of late. (A server that
    // can't say: the inbox alone.)
    let elsewhere = s.elsewhere(provider).unwrap_or_default();
    let mut next: Vec<(String, u32)> = Vec::new();
    if !elsewhere.is_empty() {
        let inbox_next = s.uid_next();
        for b in &elsewhere {
            if s.look_in(b).is_err() {
                continue;
            }
            next.push((b.clone(), s.uid_next()));
            let got = s.recent(recent)?;
            let Ok(s2) = give(s, b, got) else { return Ok(()) };
            s = s2;
        }
        s.select_inbox()?;
        s.read_from(inbox_next);
        let got = s.new_messages()?;
        let Ok(s2) = give(s, "INBOX", got) else { return Ok(()) };
        s = s2;
    }
    let mut looked = Instant::now();
    while !stopped() && Instant::now() < until() {
        let wake = if next.is_empty() { until() } else { until().min(looked + ELSEWHERE_EVERY) };
        if s.wait(wake)? && !stopped() {
            let got = s.new_messages()?;
            let Ok(s2) = give(s, "INBOX", got) else { return Ok(()) };
            s = s2;
        }
        if next.is_empty() || stopped() || Instant::now() < looked + ELSEWHERE_EVERY {
            continue;
        }
        looked = Instant::now();
        // What's grown since it was last looked in: what came, read.
        let names: Vec<String> = next.iter().map(|(b, _)| b.clone()).collect();
        let now: Vec<(String, u32)> = s.uid_nexts(&names)?;
        let grown: Vec<(String, u32)> = next.iter().filter(|(b, was)| now.iter().any(|(n, is)| n == b && is > was)).cloned().collect();
        if grown.is_empty() {
            continue;
        }
        let inbox_next = s.uid_next();
        for (b, was) in grown {
            if s.look_in(&b).is_err() {
                continue;
            }
            let uids: Vec<u32> = s.search(&format!("UID {was}:*"))?.into_iter().filter(|&u| u >= was).collect();
            if let Some(n) = next.iter_mut().find(|(n, _)| *n == b) {
                n.1 = uids.iter().max().map_or(n.1, |m| m + 1).max(s.uid_next());
            }
            let got = s.read(&uids)?;
            let Ok(s2) = give(s, &b, got) else { return Ok(()) };
            s = s2;
        }
        s.select_inbox()?;
        s.read_from(inbox_next);
        let got = s.new_messages()?;
        let Ok(s2) = give(s, "INBOX", got) else { return Ok(()) };
        s = s2;
    }
    if !stopped() {
        s.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imap_bits() {
        assert_eq!(quoted(r#"pa"ss\"#), r#""pa\"ss\\""#);
        assert_eq!(literal_size("* 1 FETCH (UID 7 BODY[]<0> {123}"), Some(123));
        assert_eq!(literal_size("* OK done"), None);
        assert_eq!(bracket_number("* OK [UIDNEXT 4392] Predicted next UID", "UIDNEXT"), Some(4392));
        assert_eq!(word_after("* 3 FETCH (UID 4391 BODY[]<0> {12}", "UID"), Some("4391"));
        assert_eq!(internal_date("* 1 FETCH (UID 9 INTERNALDATE \"17-Nov-2026 13:45:02 +0000\")"), Some(1_794_923_102));
        assert_eq!(internal_date("* 1 FETCH (INTERNALDATE \" 7-Nov-2026 05:45:02 -0800\" UID 9)"), Some(1_794_923_102 - 10 * 86_400));
    }

    fn account() -> Account {
        Account { provider: Provider::Gmail, address: "me@example.com".into() }
    }

    const CODE: &str = "From: Acme <no-reply@acme.example>\r\nSubject: Your Acme sign-in code\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nYour Acme verification code is 482913.\r\n";

    /// Signed in at the stand-in (the app password's spaces left out),
    /// the inbox opened, what came in the last minutes read and nothing
    /// older, then a message as it comes (the server telling, IDLE).
    #[test]
    fn signs_in_and_reads_what_comes() {
        let server = standin::Server::start(vec![(standin::ago(3 * 3600), "Subject: old\r\n\r\nYour code is 111111".into()), (standin::ago(30), CODE.into())]);
        let mut s = Session::open_at(&Server::Plain(server.address()), &account(), "test pass word").unwrap();
        assert_eq!(s.uid_next(), 3);
        let recent = s.recent(Duration::from_secs(300)).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!((recent[0].0, recent[0].1.subject.as_str(), recent[0].1.from.as_str()), (2, "Your Acme sign-in code", "Acme"));
        assert!(recent[0].1.text.contains("482913"));
        // Nothing new: the wait runs out.
        let t = Instant::now();
        assert!(!s.wait(Instant::now() + Duration::from_millis(300)).unwrap());
        assert!(t.elapsed() >= Duration::from_millis(280));
        assert!(s.new_messages().unwrap().is_empty());
        // One comes while it waits: told at once.
        let later = server.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.deliver("Subject: Sign in\r\n\r\nYour code: 771204");
        });
        let t = Instant::now();
        assert!(s.wait(Instant::now() + Duration::from_secs(10)).unwrap());
        assert!(t.elapsed() < Duration::from_secs(5));
        let new = s.new_messages().unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!((new[0].0, new[0].1.text.as_str()), (3, "Your code: 771204"));
        assert_eq!(s.uid_next(), 4);
        s.close();
        assert_eq!(server.logins(), ["me@example.com testpassword"]);
    }

    /// Gmail's and Fastmail's own servers, over TLS with the system's
    /// certificates: greeted, IDLE among what they can do, signed out
    /// (no account: nothing signs in). Run by hand: it goes out to them.
    #[test]
    #[ignore]
    fn the_real_servers_greet() {
        for p in Provider::ALL {
            let mut s = Session::greeted(&Server::Tls(p.host())).unwrap_or_else(|e| panic!("{}: {e}", p.name()));
            let caps = s.command("CAPABILITY").unwrap();
            assert!(caps.iter().any(|c| c.text.contains("IDLE")), "{}: {caps:?}", p.name());
            s.close();
            eprintln!("{}: greeted over TLS, has IDLE", p.name());
        }
    }

    /// A Google account's way in: its address and token in one line
    /// (XOAUTH2), the inbox opened; a token the server refuses is said
    /// plainly, after its line of why is answered.
    #[test]
    fn a_google_token() {
        let server = standin::Server::start(Vec::new());
        let s = Session::open_at(&Server::Plain(server.address()), &account(), &Auth::Token(standin::TOKEN.into())).unwrap();
        assert_eq!(s.uid_next(), 1, "its inbox opened");
        s.close();
        assert_eq!(server.logins(), ["me@example.com token"]);
        let e = Session::open_at(&Server::Plain(server.address()), &account(), &Auth::Token("stale".into())).err().unwrap();
        assert!(e.starts_with("Gmail didn’t take the account’s sign-in"), "{e}");
    }

    /// A wrong password, said plainly.
    #[test]
    fn a_wrong_password() {
        let server = standin::Server::start(Vec::new());
        let e = Session::open_at(&Server::Plain(server.address()), &account(), "wrong").err().unwrap();
        assert!(e.starts_with("Gmail didn’t take the address and app password"), "{e}");
    }

    /// A watch's loop: the recent message, then one that comes, each
    /// given to what's watching; then its time runs out, and it signs
    /// out. And a session stopped while it waits ends at once.
    #[test]
    fn a_watch() {
        let server = standin::Server::start(vec![(standin::ago(20), CODE.into())]);
        let (tx, rx) = std::sync::mpsc::channel();
        let at = server.address();
        let t = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(1500);
            let mut s = Session::open_at(&Server::Plain(at), &account(), "x").unwrap();
            for (uid, m) in s.recent(Duration::from_secs(300)).unwrap() {
                tx.send((uid, m.text)).unwrap();
            }
            while Instant::now() < until {
                if s.wait(until).unwrap() {
                    for (uid, m) in s.new_messages().unwrap() {
                        tx.send((uid, m.text)).unwrap();
                    }
                }
            }
            s.close();
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().0, 1);
        server.deliver("Subject: x\r\n\r\nYour code is 990011");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), (2, "Your code is 990011".into()));
        t.join().unwrap();
        assert!(server.said().iter().any(|c| c == "LOGOUT"), "signed out when its time was up");

        // Stopped while it waits: over at once.
        let server = standin::Server::start(Vec::new());
        let mut s = Session::open_at(&Server::Plain(server.address()), &account(), "x").unwrap();
        let shared = WatchShared::new(Instant::now() + Duration::from_secs(60));
        *shared.socket.lock().unwrap() = s.stopper();
        let stopper = Watch { shared };
        let t = Instant::now();
        let waiting = std::thread::spawn(move || s.wait(Instant::now() + Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(200));
        stopper.stop();
        assert!(waiting.join().unwrap().is_err());
        assert!(t.elapsed() < Duration::from_secs(3), "a stop ends the wait at once");
    }

    /// Where else than the inbox a watch looks: Gmail's All Mail and spam
    /// (a label's mail is in All Mail); another's every mailbox but sent
    /// mail, drafts and the trash, the junk and the archive first. What
    /// each will number next, in one asking or one a mailbox.
    #[test]
    fn where_else_mail_lands() {
        let server = standin::Server::start(Vec::new());
        for (name, flags) in [("Receipts", ""), ("Sent", "\\Sent"), ("Junk", "\\Junk"), ("Trash", "\\Trash"), ("Archive", "\\Archive"), ("Drafts", "\\Drafts")] {
            server.mailbox(name, flags);
        }
        server.put("Receipts", standin::ago(60), CODE);
        let mut s = Session::open_at(&Server::Plain(server.address()), &account(), "x").unwrap();
        let boxes = s.elsewhere(Provider::Fastmail).unwrap();
        assert_eq!(boxes, ["Junk", "Archive", "Receipts"]);
        assert_eq!(s.uid_nexts(&boxes).unwrap(), [("Receipts".to_owned(), 2), ("Junk".to_owned(), 1), ("Archive".to_owned(), 1)]);
        assert!(server.said().iter().any(|c| c.contains("RETURN (STATUS (UIDNEXT))")), "LIST-STATUS used where the server has it");
        s.close();

        let server = standin::Server::start(Vec::new());
        for (name, flags) in [("[Gmail]/All Mail", "\\HasNoChildren \\All"), ("[Gmail]/Spam", "\\HasNoChildren \\Junk"), ("Work", "\\HasNoChildren"), ("[Gmail]/Sent Mail", "\\Sent")] {
            server.mailbox(name, flags);
        }
        server.without_list_status();
        let mut s = Session::open_at(&Server::Plain(server.address()), &account(), "x").unwrap();
        let boxes = s.elsewhere(Provider::Gmail).unwrap();
        assert_eq!(boxes, ["[Gmail]/Spam", "[Gmail]/All Mail"]);
        assert_eq!(s.uid_nexts(&boxes).unwrap(), [("[Gmail]/Spam".to_owned(), 1), ("[Gmail]/All Mail".to_owned(), 1)]);
        assert!(server.said().iter().any(|c| c.starts_with("STATUS ")), "STATUS asked where it can't list with it");
        s.close();
    }

    /// A watch finds a code wherever the account's mail lands: one that
    /// came to the junk a minute ago, one a rule files in a folder while it
    /// watches (Ben's, 2026-11-18, filed away and never found), and the
    /// inbox's still at once after looking elsewhere; never sent mail's or
    /// the trash's. The same message in two mailboxes has one key.
    #[test]
    fn a_watch_looks_where_mail_lands() {
        let server = standin::Server::start(Vec::new());
        for (name, flags) in [("Receipts", ""), ("Junk", "\\Junk"), ("Sent", "\\Sent"), ("Trash", "\\Trash")] {
            server.mailbox(name, flags);
        }
        let mail = |id: &str, code: &str| format!("From: Acme <no-reply@acme.example>\r\nMessage-ID: <{id}@acme.example>\r\nSubject: Your code\r\n\r\nYour Acme verification code is {code}.\r\n");
        server.put("Junk", standin::ago(60), &mail("junk", "300001"));
        server.put("Sent", standin::ago(30), &mail("sent", "300002"));
        server.put("Trash", standin::ago(30), &mail("trash", "300003"));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = Session::open_at(&Server::Plain(server.address()), &account(), "x").unwrap();
        let shared = WatchShared::new(Instant::now() + Duration::from_secs(8));
        let sh = shared.clone();
        let t = std::thread::spawn(move || {
            let mut found = |key: String, m: Message| tx.send((key, m.text)).is_ok();
            watch_in(s, Provider::Fastmail, Duration::from_secs(300), &sh, &mut found).unwrap();
        });
        let next = || rx.recv_timeout(Duration::from_secs(8)).unwrap();
        assert_eq!(next(), ("junk@acme.example".to_owned(), "Your Acme verification code is 300001.".to_owned()));
        let filed = Instant::now();
        server.deliver_to("Receipts", &mail("filed", "300004"));
        assert_eq!(next().0, "filed@acme.example");
        assert!(filed.elapsed() < ELSEWHERE_EVERY + Duration::from_secs(2), "a filed message waited {:?}", filed.elapsed());
        let came = Instant::now();
        server.deliver(&mail("inbox", "300005"));
        assert_eq!(next().0, "inbox@acme.example");
        assert!(came.elapsed() < Duration::from_secs(2), "the inbox's waited {:?}", came.elapsed());
        t.join().unwrap();
        assert!(rx.try_recv().is_err(), "sent mail's or the trash's read");
        assert_eq!(key_of("INBOX", 4, &parse(b"Subject: x\r\n\r\ny")), "INBOX/4");
        assert!(server.said().iter().all(|c| !c.contains("\"Sent\"") && !c.contains("\"Trash\"")), "{:?}", server.said());
    }

    /// An account added here is left out only where a Google account is
    /// the same mailbox (Gmail, of its address): a Fastmail account of the
    /// address a Google account has stays.
    #[test]
    fn a_fastmail_account_stays_beside_a_google_one_of_its_address() {
        let gmail = |a: &str| Account { provider: Provider::Gmail, address: a.into() };
        let fastmail = |a: &str| Account { provider: Provider::Fastmail, address: a.into() };
        let google = vec![gmail("ben@example.org")];
        let own = vec![fastmail("ben@example.org"), gmail("Ben@Example.org"), gmail("ann@example.com"), fastmail("ann@example.net")];
        assert_eq!(merged(google, own), vec![gmail("ben@example.org"), fastmail("ben@example.org"), gmail("ann@example.com"), fastmail("ann@example.net")]);
    }
}
