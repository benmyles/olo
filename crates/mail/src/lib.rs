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
//! [`Session`]: signed in to an account's inbox. [`Watch`]: an inbox
//! watched on a thread of its own for a while (what came in the last few
//! minutes, then each message as it comes, the server telling: IDLE),
//! stopped when it's dropped. [`check`]: whether an account signs in
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

/// The keyring attributes of an account's app password.
fn attributes(address: &str) -> [(&str, &str); 3] {
    [("application", "olo"), ("service", "mail"), ("account", address)]
}

/// Keep `password` for `address` in the login keyring. Blocking (a D-Bus
/// call): off the UI thread.
pub fn keep_password(address: &str, password: &str) -> Result<(), String> {
    let label = format!("Mail: {address} (Olo)");
    olo_system::secrets::store(&label, &attributes(address), password.as_bytes()).map_err(|e| e.to_string())
}

/// `address`'s app password, if kept. Blocking. (Against a test's
/// stand-in server, `OLO_MAIL_IMAP`, its own.)
pub fn password(address: &str) -> Option<String> {
    if std::env::var_os("OLO_MAIL_IMAP").is_some() {
        return Some(standin::PASSWORD.into());
    }
    let v = olo_system::secrets::lookup(&attributes(address)).ok()??;
    String::from_utf8(v).ok().filter(|p| !p.is_empty())
}

/// Forget `address`'s app password. Blocking.
pub fn forget_password(address: &str) {
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
    /// What's come of a line not ended yet (a wait that ran out in it).
    partial: Vec<u8>,
}

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
    /// Sign in to `account` with `password` (its app password: spaces, as
    /// Google shows it in fours, left out) and open its inbox.
    pub fn open(account: &Account, password: &str) -> Result<Session, String> {
        Session::open_at(&Server::of(account), account, password)
    }

    /// [`Session::open`] at `server`.
    pub fn open_at(server: &Server, account: &Account, password: &str) -> Result<Session, String> {
        let mut s = Session::greeted(server)?;
        let pass: String = password.chars().filter(|c| !c.is_whitespace()).collect();
        s.command(&format!("LOGIN {} {}", quoted(&account.address), quoted(&pass)))
            .map_err(|e| if e.starts_with("NO") || e.starts_with("BAD") { format!("{} didn’t take the address and app password ({})", account.provider.name(), e.trim()) } else { e })?;
        let caps = s.command("CAPABILITY")?;
        s.idle = caps.iter().any(|c| c.text.split_whitespace().any(|w| w.eq_ignore_ascii_case("IDLE")));
        let answers = s.command("SELECT INBOX")?;
        s.exists = answers.iter().find_map(|a| a.text.strip_prefix("* ").and_then(|r| r.strip_suffix(" EXISTS")).and_then(|n| n.trim().parse().ok())).unwrap_or(0);
        s.uid_next = answers.iter().find_map(|a| bracket_number(&a.text, "UIDNEXT")).unwrap_or(0);
        if s.uid_next == 0 {
            // (A server that doesn't say: the last one there, plus one.)
            let found = s.command("UID SEARCH ALL")?;
            s.uid_next = found.iter().filter_map(|a| a.text.strip_prefix("* SEARCH")).flat_map(|r| r.split_whitespace().filter_map(|n| n.parse::<u32>().ok())).max().map_or(1, |m| m + 1);
        }
        Ok(s)
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
        let mut s = Session { stream: BufReader::new(raw), socket, tag: 0, uid_next: 0, exists: 0, idle: false, partial: Vec::new() };
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

    /// The messages that came within `within` of now (of the inbox's
    /// newest five), oldest first.
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

    /// The messages that came since the last read (or since signing in),
    /// oldest first, each read whole up to 64 KB (a code is in its first
    /// lines).
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

/// Whether `account` signs in with `password` (Preferences, adding it):
/// signed in and its inbox opened, then signed out. Blocking.
pub fn check(account: &Account, password: &str) -> Result<(), String> {
    Session::open(account, password).map(Session::close)
}

/// An inbox watched on a thread of its own (named "mail"): the messages
/// of the last `recent`, then each as it comes, until its time is up or
/// it's stopped (dropped). Each message read goes to `found`, which says
/// whether to go on.
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
    pub fn start(account: Account, recent: Duration, until: Instant, mut found: impl FnMut(u32, Message) -> bool + Send + 'static) -> Watch {
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

fn watch(account: &Account, recent: Duration, sh: &WatchShared, found: &mut dyn FnMut(u32, Message) -> bool) -> Result<(), String> {
    let stopped = || sh.stopped.load(Ordering::SeqCst);
    let until = || *sh.until.lock().unwrap_or_else(|e| e.into_inner());
    let password = password(&account.address).ok_or("no app password in the keyring")?;
    if stopped() {
        return Ok(());
    }
    let mut s = Session::open(account, &password)?;
    *sh.socket.lock().unwrap_or_else(|e| e.into_inner()) = s.stopper();
    // (Stopped while it signed in: the stop had no connection to shut.)
    if stopped() {
        return Ok(());
    }
    for (uid, m) in s.recent(recent)? {
        if !found(uid, m) {
            s.close();
            return Ok(());
        }
    }
    while !stopped() && Instant::now() < until() {
        if !s.wait(until())? || stopped() {
            continue;
        }
        for (uid, m) in s.new_messages()? {
            if !found(uid, m) {
                s.close();
                return Ok(());
            }
        }
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
}
