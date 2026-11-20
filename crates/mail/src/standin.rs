//! A stand-in IMAP server on loopback, for tests (this crate's, and the
//! phone service's, which reads mail for codes): an inbox of messages,
//! other mailboxes beside it (a folder, the junk, Gmail's All Mail, each
//! with its special-use flags), a message delivered to one while a client
//! waits (told at once, as IDLE tells, of the mailbox it has open), and
//! what was asked of it. Any password but "wrong" signs in. Plain
//! TCP: a session reaches it through `OLO_MAIL_IMAP` (`HOST:PORT`).

use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The password a session gives the stand-in (`crate::password` with
/// `OLO_MAIL_IMAP` set).
pub const PASSWORD: &str = "standin";
/// The Google token it takes (`AUTHENTICATE XOAUTH2`); any other is
/// refused as Google refuses one: a line of why, then NO.
pub const TOKEN: &str = "ya29.standin";

/// When it was, `secs` seconds ago (Unix seconds).
pub fn ago(secs: u64) -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(secs)
}

/// A mailbox: its name, its flags as LIST says them (`\\Junk`), and each
/// message's arrival (Unix seconds) and bytes, its UID its place from 1.
struct Mailbox {
    name: String,
    flags: String,
    messages: Vec<(u64, String)>,
}

struct State {
    /// The inbox first.
    boxes: Vec<Mailbox>,
    logins: Vec<String>,
    said: Vec<String>,
    /// It lists with what each mailbox has (LIST-STATUS).
    list_status: bool,
}

impl State {
    fn find(&self, name: &str) -> Option<usize> {
        self.boxes.iter().position(|b| b.name == name || b.name == "INBOX" && name.eq_ignore_ascii_case("INBOX"))
    }
}

/// The server, running for as long as the test does.
#[derive(Clone)]
pub struct Server {
    address: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub fn start(messages: Vec<(u64, String)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let address = listener.local_addr().expect("its address").to_string();
        let inbox = Mailbox { name: "INBOX".into(), flags: String::new(), messages };
        let state = Arc::new(Mutex::new(State { boxes: vec![inbox], logins: Vec::new(), said: Vec::new(), list_status: true }));
        let st = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let st = st.clone();
                std::thread::spawn(move || serve(stream, &st));
            }
        });
        Server { address, state }
    }

    /// Where it listens (`HOST:PORT`).
    pub fn address(&self) -> String {
        self.address.clone()
    }

    /// A message comes to the inbox, now.
    pub fn deliver(&self, raw: &str) {
        self.deliver_to("INBOX", raw);
    }

    /// A message comes to mailbox `name`, now.
    pub fn deliver_to(&self, name: &str, raw: &str) {
        self.put(name, ago(0), raw);
    }

    /// A message in mailbox `name` that came at `at` (Unix seconds).
    pub fn put(&self, name: &str, at: u64, raw: &str) {
        let mut st = self.lock();
        let i = st.find(name).expect("a mailbox of the stand-in's");
        st.boxes[i].messages.push((at, raw.to_owned()));
    }

    /// A mailbox beside the inbox, its special-use flags as LIST says
    /// them (`"\\Junk"`, `"\\All"`, `""` for a folder of the person's).
    pub fn mailbox(&self, name: &str, flags: &str) {
        self.lock().boxes.push(Mailbox { name: name.into(), flags: flags.into(), messages: Vec::new() });
    }

    /// It asks each mailbox's STATUS, not listing them with it.
    pub fn without_list_status(&self) {
        self.lock().list_status = false;
    }

    /// Each sign-in: "ADDRESS PASSWORD".
    pub fn logins(&self) -> Vec<String> {
        self.lock().logins.clone()
    }

    /// Each command asked, without its tag (a LOGIN's address and
    /// password left out).
    pub fn said(&self) -> Vec<String> {
        self.lock().said.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An IMAP quoted string's text, or an atom.
fn unquote(s: &str) -> (String, &str) {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for (i, c) in rest.char_indices() {
            if escaped {
                out.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                return (out, &rest[i + 1..]);
            } else {
                out.push(c);
            }
        }
        (out, "")
    } else {
        let end = s.find(' ').unwrap_or(s.len());
        (s[..end].to_owned(), &s[end..])
    }
}

/// The numbers of a set ("2", "1,3", "2:4", "5:*") there are, as IMAP
/// has them: "N:*" is N to the last, or the last alone if N is past it.
fn set(s: &str, last: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let (a, b) = part.split_once(':').unwrap_or((part, part));
        let n = |x: &str| if x == "*" { Some(last) } else { x.parse().ok() };
        if let (Some(a), Some(b)) = (n(a), n(b)) {
            out.extend((a.min(b)..=a.max(b)).filter(|n| (1..=last).contains(n)));
        }
    }
    out
}

/// "17-Nov-2026 13:45:02 +0000".
fn internal_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // (Howard Hinnant's civil_from_days.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{day:02}-{}-{year} {:02}:{:02}:{:02} +0000", MONTHS[(month - 1) as usize], rest / 3600, rest % 3600 / 60, rest % 60)
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let lock = || state.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut w = stream;
    let say = |w: &mut TcpStream, s: &str| w.write_all(s.as_bytes()).is_ok();
    if !say(&mut w, "* OK [CAPABILITY IMAP4rev1 IDLE] Stand-in ready\r\n") {
        return;
    }
    // The mailbox this client has open, and how many of its messages it
    // has been told of.
    let mut current = 0usize;
    let mut told = 0u32;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(n) if n > 0 => {}
            _ => return,
        }
        let Some((tag, cmd)) = line.trim_end().split_once(' ') else { continue };
        let (tag, cmd) = (tag.to_owned(), cmd.to_owned());
        let upper = cmd.to_ascii_uppercase();
        let count = lock().boxes[current].messages.len() as u32;
        let reply = if let Some(args) = cmd.strip_prefix("LOGIN ") {
            lock().said.push("LOGIN".into());
            let (address, rest) = unquote(args);
            let (password, _) = unquote(rest);
            if password == "wrong" {
                format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
            } else {
                lock().logins.push(format!("{address} {password}"));
                format!("{tag} OK signed in\r\n")
            }
        } else if let Some(said) = cmd.strip_prefix("AUTHENTICATE XOAUTH2 ") {
            use base64::Engine as _;
            lock().said.push("AUTHENTICATE XOAUTH2".into());
            let text = base64::engine::general_purpose::STANDARD.decode(said.trim()).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
            let mut parts = text.split('\x01');
            let (user, auth) = (parts.next().unwrap_or_default().trim_start_matches("user=").to_owned(), parts.next().unwrap_or_default().to_owned());
            if auth == format!("auth=Bearer {TOKEN}") {
                lock().logins.push(format!("{user} token"));
                format!("{tag} OK signed in\r\n")
            } else {
                // (Why, base64, then it waits for a line before saying NO.)
                let _ = w.write_all(b"+ eyJzdGF0dXMiOiI0MDAifQ==\r\n");
                let _ = w.flush();
                let mut empty = String::new();
                let _ = reader.read_line(&mut empty);
                format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
            }
        } else {
            lock().said.push(cmd.clone());
            if upper == "CAPABILITY" {
                let more = if lock().list_status { " LIST-STATUS" } else { "" };
                format!("* CAPABILITY IMAP4rev1 IDLE{more}\r\n{tag} OK\r\n")
            } else if let Some((verb, name)) = cmd.split_once(' ').filter(|(v, _)| v.eq_ignore_ascii_case("SELECT") || v.eq_ignore_ascii_case("EXAMINE")) {
                let (name, _) = unquote(name);
                let st = lock();
                match st.find(&name) {
                    Some(i) => {
                        current = i;
                        let n = st.boxes[i].messages.len() as u32;
                        told = n;
                        let how = if verb.eq_ignore_ascii_case("EXAMINE") { "READ-ONLY" } else { "READ-WRITE" };
                        format!("* {n} EXISTS\r\n* OK [UIDNEXT {}] next\r\n{tag} OK [{how}] selected\r\n", n + 1)
                    }
                    None => format!("{tag} NO no such mailbox\r\n"),
                }
            } else if upper.starts_with("LIST \"\" \"*\"") {
                let st = lock();
                let with_status = upper.contains("RETURN (STATUS");
                let mut out = String::new();
                for b in &st.boxes {
                    out.push_str(&format!("* LIST ({}) \"/\" \"{}\"\r\n", b.flags, b.name));
                    if with_status && st.list_status {
                        out.push_str(&format!("* STATUS \"{}\" (UIDNEXT {})\r\n", b.name, b.messages.len() + 1));
                    }
                }
                format!("{out}{tag} OK\r\n")
            } else if let Some(rest) = cmd.strip_prefix("STATUS ") {
                let (name, _) = unquote(rest);
                let st = lock();
                match st.find(&name) {
                    Some(i) => format!("* STATUS \"{}\" (UIDNEXT {})\r\n{tag} OK\r\n", st.boxes[i].name, st.boxes[i].messages.len() + 1),
                    None => format!("{tag} NO no such mailbox\r\n"),
                }
            } else if upper == "NOOP" {
                let news = if count > told { format!("* {count} EXISTS\r\n") } else { String::new() };
                told = count;
                format!("{news}{tag} OK\r\n")
            } else if let Some(s) = upper.strip_prefix("FETCH ").and_then(|r| r.strip_suffix(" (UID INTERNALDATE)")) {
                let st = lock();
                let mut out = String::new();
                for n in set(s, count) {
                    if let Some((at, _)) = st.boxes[current].messages.get(n as usize - 1) {
                        out.push_str(&format!("* {n} FETCH (UID {n} INTERNALDATE \"{}\")\r\n", internal_date(*at)));
                    }
                }
                format!("{out}{tag} OK\r\n")
            } else if let Some(s) = upper.strip_prefix("UID SEARCH UID ") {
                let uids = set(s, count);
                format!("* SEARCH{}\r\n{tag} OK\r\n", uids.iter().map(|u| format!(" {u}")).collect::<String>())
            } else if upper == "UID SEARCH ALL" {
                format!("* SEARCH{}\r\n{tag} OK\r\n", (1..=count).map(|u| format!(" {u}")).collect::<String>())
            } else if let Some(rest) = upper.strip_prefix("UID FETCH ") {
                let s = rest.split(' ').next().unwrap_or_default();
                let st = lock();
                let mut out = String::new();
                for n in set(s, count) {
                    if let Some((_, raw)) = st.boxes[current].messages.get(n as usize - 1) {
                        out.push_str(&format!("* {n} FETCH (UID {n} BODY[]<0> {{{}}}\r\n{raw})\r\n", raw.len()));
                    }
                }
                format!("{out}{tag} OK\r\n")
            } else if upper == "IDLE" {
                if !say(&mut w, "+ idling\r\n") {
                    return;
                }
                // Told of each message as it comes, until DONE.
                let _ = reader.get_ref().set_read_timeout(Some(Duration::from_millis(40)));
                let mut done = String::new();
                loop {
                    let now = lock().boxes[current].messages.len() as u32;
                    if now > told {
                        told = now;
                        if !say(&mut w, &format!("* {now} EXISTS\r\n")) {
                            return;
                        }
                    }
                    match reader.read_line(&mut done) {
                        Ok(0) => return,
                        Ok(_) if done.ends_with('\n') => break,
                        Ok(_) => {}
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(_) => return,
                    }
                }
                let _ = reader.get_ref().set_read_timeout(None);
                lock().said.push(done.trim().to_owned());
                format!("{tag} OK IDLE terminated\r\n")
            } else if upper == "LOGOUT" {
                let _ = say(&mut w, &format!("* BYE\r\n{tag} OK\r\n"));
                return;
            } else {
                format!("{tag} BAD not known here\r\n")
            }
        };
        if !say(&mut w, &reply) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_and_sets() {
        assert_eq!(super::internal_date(1_794_923_102), "17-Nov-2026 13:45:02 +0000");
        assert_eq!(super::set("2:4", 9), [2, 3, 4]);
        assert_eq!(super::set("5:*", 7), [5, 6, 7]);
        assert_eq!(super::set("1,3", 3), [1, 3]);
        assert_eq!(super::set("8:*", 7), [7]);
        assert_eq!(super::set("1:*", 0), [] as [u32; 0]);
    }
}
