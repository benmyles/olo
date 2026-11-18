//! A stand-in CalDAV server, for tests and for trying the app with no
//! account: one person's calendars kept in memory, on a loopback port,
//! answering what [`crate::dav::Server`] asks (who is signed in, where
//! their calendars are, what is in one, what changed since a token, an
//! event's text, put and delete with their conditions) as a real one
//! does, and nothing more.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Default)]
pub struct Calendar {
    pub name: String,
    pub color: Option<String>,
    pub read_only: bool,
    /// By file name ("abc.ics"): its tag and its text.
    pub items: BTreeMap<String, (String, String)>,
    /// What changed, in order: the change's number, the file, gone or not.
    log: Vec<(u64, String, bool)>,
    /// Tokens from before this change are no longer known.
    floor: u64,
}

#[derive(Debug, Default)]
struct State {
    /// By folder name ("work").
    calendars: BTreeMap<String, Calendar>,
    changes: u64,
    /// Requests answered, as "METHOD path".
    asked: Vec<String>,
    /// As Google signs in: the refresh token it knows, the access tokens
    /// it has given that are still good, and how many it has given.
    refresh: Option<String>,
    tokens: Vec<String>,
    given: u64,
}

/// The server: gone when dropped.
pub struct Standin {
    pub port: u16,
    user: String,
    password: String,
    state: Arc<Mutex<State>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Standin {
    /// Started, with one account.
    pub fn start(user: &str, password: &str) -> Standin {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().unwrap().port();
        let state: Arc<Mutex<State>> = Default::default();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s, st, auth) = (state.clone(), stop.clone(), basic(user, password));
        std::thread::Builder::new()
            .name("caldav-standin".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if st.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        let (s, auth) = (s.clone(), auth.clone());
                        let _ = std::thread::Builder::new().name("caldav-standin-conn".into()).spawn(move || serve(stream, &s, &auth));
                    }
                }
            })
            .expect("thread");
        Standin { port, user: user.to_owned(), password: password.to_owned(), state, stop }
    }

    /// Where it is.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    pub fn auth(&self) -> crate::dav::Auth {
        crate::dav::Auth::Basic { user: self.user.clone(), password: self.password.clone() }
    }

    /// A calendar there (its folder's name, what it's called, its colour).
    pub fn add_calendar(&self, folder: &str, name: &str, color: Option<&str>) {
        self.state.lock().unwrap().calendars.insert(folder.to_owned(), Calendar { name: name.to_owned(), color: color.map(str::to_owned), ..Default::default() });
    }

    pub fn set_read_only(&self, folder: &str, on: bool) {
        if let Some(c) = self.state.lock().unwrap().calendars.get_mut(folder) {
            c.read_only = on;
        }
    }

    /// An event put there by someone else (another device): its new tag.
    pub fn put(&self, folder: &str, file: &str, ics: &str) -> String {
        let mut s = self.state.lock().unwrap();
        put(&mut s, folder, file, ics)
    }

    /// Removed by someone else.
    pub fn remove(&self, folder: &str, file: &str) {
        let mut s = self.state.lock().unwrap();
        remove(&mut s, folder, file);
    }

    /// An event's text there now.
    pub fn get(&self, folder: &str, file: &str) -> Option<String> {
        self.state.lock().unwrap().calendars.get(folder)?.items.get(file).map(|(_, ics)| ics.clone())
    }

    pub fn files(&self, folder: &str) -> Vec<String> {
        self.state.lock().unwrap().calendars.get(folder).map(|c| c.items.keys().cloned().collect()).unwrap_or_default()
    }

    /// Signs in as Google does too: `refresh` is a refresh token it
    /// knows, for which `POST /token` gives access tokens.
    pub fn knows_refresh_token(&self, refresh: &str) {
        self.state.lock().unwrap().refresh = Some(refresh.to_owned());
    }

    /// Every access token given is no longer good (an hour went by).
    pub fn expire_tokens(&self) {
        self.state.lock().unwrap().tokens.clear();
    }

    /// How many access tokens it has given.
    pub fn tokens_given(&self) -> u64 {
        self.state.lock().unwrap().given
    }

    /// What was asked since this was last asked, as "METHOD path".
    pub fn asked(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock().unwrap().asked)
    }

    /// Forget what changed when: tokens from before are no longer known.
    pub fn forget_changes(&self) {
        let mut s = self.state.lock().unwrap();
        s.changes += 1;
        let floor = s.changes;
        for c in s.calendars.values_mut() {
            c.log.clear();
            c.floor = floor;
        }
    }
}

impl Drop for Standin {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // (Wake the listener so it sees.)
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn basic(user: &str, password: &str) -> String {
    use base64::Engine as _;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}")))
}

fn put(s: &mut State, folder: &str, file: &str, ics: &str) -> String {
    s.changes += 1;
    let n = s.changes;
    let etag = format!("\"e{n}\"");
    if let Some(c) = s.calendars.get_mut(folder) {
        c.items.insert(file.to_owned(), (etag.clone(), ics.to_owned()));
        c.log.push((n, file.to_owned(), false));
    }
    etag
}

fn remove(s: &mut State, folder: &str, file: &str) {
    s.changes += 1;
    let n = s.changes;
    if let Some(c) = s.calendars.get_mut(folder)
        && c.items.remove(file).is_some()
    {
        c.log.push((n, file.to_owned(), true));
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What's between `<…name>` and `</…name>` in `xml`, each time.
fn between<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find(&format!("{name}>")) {
        let from = open + name.len() + 1;
        // (Its opening tag, not a closing one.)
        let opening = rest[..open].rfind('<').is_some_and(|lt| !rest[lt..open].starts_with("</"));
        let Some(close) = rest[from..].find("</") else { break };
        if opening {
            out.push(rest[from..from + close].trim());
        }
        rest = &rest[from..];
    }
    out
}

fn serve(stream: TcpStream, state: &Mutex<State>, auth: &str) {
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { return };
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let mut body = vec![0u8; headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0)];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let (status, extra, answer) = {
        let mut s = state.lock().unwrap();
        let given = headers.get("authorization").map(String::as_str).unwrap_or_default();
        if method == "POST" && path == "/token" {
            // Google's token endpoint: an access token for the refresh token it knows.
            let refresh = body.split('&').find_map(|kv| kv.strip_prefix("refresh_token=")).unwrap_or_default();
            if s.refresh.as_deref() == Some(refresh) {
                s.given += 1;
                let token = format!("access-{}", s.given);
                s.tokens.push(token.clone());
                (200, String::new(), format!(r#"{{"access_token":"{token}","expires_in":3600}}"#))
            } else {
                (400, String::new(), r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#.to_owned())
            }
        } else if given != auth && !given.strip_prefix("Bearer ").is_some_and(|t| s.tokens.iter().any(|k| k == t)) {
            (401, String::new(), String::new())
        } else {
            s.asked.push(format!("{method} {path}"));
            answer(&mut s, &method, &path, &headers, &body)
        }
    };
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        412 => "Precondition Failed",
        _ => "Other",
    };
    let mut stream = reader.into_inner();
    let _ = write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{answer}", answer.len());
}

fn multistatus(responses: &str, token: Option<u64>) -> (u16, String, String) {
    let token = token.map(|t| format!("<d:sync-token>t{t}</d:sync-token>")).unwrap_or_default();
    (
        207,
        String::new(),
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/">{responses}{token}</d:multistatus>"#
        ),
    )
}

fn answer(s: &mut State, method: &str, path: &str, headers: &BTreeMap<String, String>, body: &str) -> (u16, String, String) {
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    let ok = |href: &str, props: &str| format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
    match (method, parts.as_slice()) {
        // Who is signed in; where their calendars are.
        ("PROPFIND", []) => multistatus(&ok("/", "<d:current-user-principal><d:href>/principal/</d:href></d:current-user-principal>"), None),
        ("PROPFIND", ["principal"]) => multistatus(&ok("/principal/", "<c:calendar-home-set><d:href>/cal/</d:href></c:calendar-home-set>"), None),
        // The calendars.
        ("PROPFIND", ["cal"]) => {
            let mut out = ok("/cal/", "<d:resourcetype><d:collection/></d:resourcetype>");
            for (folder, c) in &s.calendars {
                let color = c.color.as_ref().map(|c| format!("<a:calendar-color>{c}FF</a:calendar-color>")).unwrap_or_default();
                let rights = if c.read_only { "<d:privilege><d:read/></d:privilege>" } else { "<d:privilege><d:read/></d:privilege><d:privilege><d:write/></d:privilege>" };
                out += &ok(
                    &format!("/cal/{folder}/"),
                    &format!(
                        "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>{}</d:displayname>{color}<cs:getctag>c{}</cs:getctag><d:sync-token>t{}</d:sync-token><c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set><d:current-user-privilege-set>{rights}</d:current-user-privilege-set>",
                        esc(&c.name),
                        c.log.last().map_or(0, |l| l.0),
                        s.changes
                    ),
                );
            }
            // (And a list of tasks, which isn't a calendar of events.)
            out += &ok(
                "/cal/tasks/",
                "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Tasks</d:displayname><c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>",
            );
            multistatus(&out, None)
        }
        // What is in one.
        ("PROPFIND", ["cal", folder]) => match s.calendars.get(*folder) {
            Some(c) => {
                let mut out = ok(&format!("/cal/{folder}/"), "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>");
                for (file, (etag, _)) in &c.items {
                    out += &ok(&format!("/cal/{folder}/{file}"), &format!("<d:getetag>{}</d:getetag><d:resourcetype/>", esc(etag)));
                }
                multistatus(&out, None)
            }
            None => (404, String::new(), String::new()),
        },
        ("REPORT", ["cal", folder]) => {
            let changes = s.changes;
            let Some(c) = s.calendars.get(*folder) else { return (404, String::new(), String::new()) };
            if body.contains("sync-collection") {
                // What changed since the token (none: everything there).
                let token = between(body, "sync-token").first().copied().unwrap_or_default().to_owned();
                let since = match token.strip_prefix('t').map(str::parse::<u64>) {
                    None if token.is_empty() => None,
                    Some(Ok(n)) if n < c.floor => return (403, String::new(), "<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>".into()),
                    Some(Ok(n)) => Some(n),
                    _ => return (403, String::new(), "<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>".into()),
                };
                let mut out = String::new();
                match since {
                    None => {
                        for (file, (etag, _)) in &c.items {
                            out += &ok(&format!("/cal/{folder}/{file}"), &format!("<d:getetag>{}</d:getetag>", esc(etag)));
                        }
                    }
                    Some(n) => {
                        let mut seen = std::collections::BTreeSet::new();
                        for (_, file, _) in c.log.iter().rev().filter(|l| l.0 > n) {
                            if !seen.insert(file.clone()) {
                                continue;
                            }
                            match c.items.get(file) {
                                Some((etag, _)) => out += &ok(&format!("/cal/{folder}/{file}"), &format!("<d:getetag>{}</d:getetag>", esc(etag))),
                                None => out += &format!("<d:response><d:href>/cal/{folder}/{file}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"),
                            }
                        }
                    }
                }
                multistatus(&out, Some(changes))
            } else {
                // The text of those named.
                let mut out = String::new();
                for href in between(body, "href") {
                    let file = href.rsplit('/').next().unwrap_or_default();
                    match c.items.get(file) {
                        Some((etag, ics)) => out += &ok(&format!("/cal/{folder}/{file}"), &format!("<d:getetag>{}</d:getetag><c:calendar-data>{}</c:calendar-data>", esc(etag), esc(ics))),
                        None => out += &format!("<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"),
                    }
                }
                multistatus(&out, None)
            }
        }
        ("PUT", ["cal", folder, file]) => {
            let Some(c) = s.calendars.get(*folder) else { return (404, String::new(), String::new()) };
            if c.read_only {
                return (403, String::new(), String::new());
            }
            let now = c.items.get(*file).map(|(etag, _)| etag.clone());
            let fits = match (headers.get("if-match"), headers.get("if-none-match")) {
                (Some(tag), _) => now.as_deref() == Some(tag.as_str()),
                (None, Some(_)) => now.is_none(),
                (None, None) => true,
            };
            if !fits {
                return (412, String::new(), String::new());
            }
            let etag = put(s, folder, file, body);
            (if now.is_some() { 204 } else { 201 }, format!("ETag: {etag}\r\n"), String::new())
        }
        ("DELETE", ["cal", folder, file]) => {
            let Some(c) = s.calendars.get(*folder) else { return (404, String::new(), String::new()) };
            match (c.items.get(*file), headers.get("if-match")) {
                (None, _) => (404, String::new(), String::new()),
                (Some((etag, _)), Some(tag)) if etag != tag => (412, String::new(), String::new()),
                _ => {
                    remove(s, folder, file);
                    (204, String::new(), String::new())
                }
            }
        }
        _ => (404, String::new(), String::new()),
    }
}
