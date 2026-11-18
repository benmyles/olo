//! CalDAV (RFC 4791, on WebDAV): the calendars an account has on a
//! server, what is in one, what changed in it since last asked, and an
//! event put there, changed or removed, never over someone else's change
//! (`If-Match`).
//!
//! One protocol for every account: Fastmail's with an app password,
//! Google's with its sign-in's token. Each call is one or two requests
//! and blocks: off the UI thread. The network's changes are Olo's to
//! handle (`olo_net`): the HTTP client is made again on a new network,
//! and what only reads is asked again when the network changed under it
//! (what writes isn't: it may have been done).

use std::time::Duration;

/// How an account signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// An address and its (app) password.
    Basic { user: String, password: String },
    /// An OAuth access token.
    Bearer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The server wouldn't have the sign-in (401, 403).
    SignIn,
    /// It isn't there (404, 410).
    Gone,
    /// It was changed since it was read (412): read it again first.
    Changed,
    /// The token for what changed is no longer good: read it all again.
    StaleToken,
    /// Another answer than expected.
    Status(u16),
    /// No answer: no network, no such server, a timeout.
    Network(String),
    /// An answer that can't be read.
    Answer(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::SignIn => write!(f, "the server didn't accept the sign-in"),
            Error::Gone => write!(f, "it's no longer there"),
            Error::Changed => write!(f, "it was changed somewhere else meanwhile"),
            Error::StaleToken => write!(f, "the server has forgotten what was last read"),
            Error::Status(code) => write!(f, "the server answered {code}"),
            Error::Network(why) => write!(f, "the server couldn't be reached: {why}"),
            Error::Answer(why) => write!(f, "the server's answer couldn't be read: {why}"),
        }
    }
}

/// A calendar on a server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Collection {
    /// Its address, whole.
    pub href: String,
    pub name: String,
    /// "#RRGGBB", where the server says one.
    pub color: Option<String>,
    /// Changes whenever anything in it does (where the server has one).
    pub ctag: Option<String>,
    /// What to say to hear only what changed since (where it has one).
    pub sync_token: Option<String>,
    /// It may be written to.
    pub writable: bool,
}

/// What is in a calendar: one event (and its occurrences changed by
/// themselves) a resource.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resource {
    pub href: String,
    pub etag: String,
    /// Its iCalendar text, where asked for.
    pub data: Option<String>,
}

/// What changed in a calendar since a token.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// What to say next time.
    pub token: Option<String>,
    pub changed: Vec<Resource>,
    pub removed: Vec<String>,
}

/// `href` (a path, or a whole address) as a whole address on `base`'s
/// server.
pub fn resolve(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_owned();
    }
    let origin = match base.find("://").and_then(|i| base[i + 3..].find('/').map(|j| i + 3 + j)) {
        Some(end) => &base[..end],
        None => base,
    };
    if href.starts_with('/') {
        return format!("{origin}{href}");
    }
    // (Beside the base's last part.)
    let dir = base.rfind('/').filter(|i| *i >= origin.len()).map_or(base, |i| &base[..i]);
    format!("{dir}/{href}")
}

/// The same resource, however each side wrote its address (a path or a
/// whole one, `%40` or `@`).
pub fn same_href(a: &str, b: &str) -> bool {
    fn path(h: &str) -> String {
        let p = match h.find("://").and_then(|i| h[i + 3..].find('/').map(|j| i + 3 + j)) {
            Some(start) => &h[start..],
            None => h,
        };
        let mut out = String::new();
        let bytes = p.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && p.is_char_boundary(i + 1)
                && p.is_char_boundary(i + 3)
                && let Ok(v) = u8::from_str_radix(&p[i + 1..i + 3], 16)
            {
                out.push(v as char);
                i += 3;
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        out.trim_end_matches('/').to_owned()
    }
    path(a) == path(b)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const DAV: &str = "DAV:";
const CAL: &str = "urn:ietf:params:xml:ns:caldav";
const CS: &str = "http://calendarserver.org/ns/";
const APPLE: &str = "http://apple.com/ns/ical/";

/// One `response` of a multistatus: its address, whether it's gone, and
/// its properties that the server had (status 200).
struct Response<'a, 'input> {
    href: String,
    gone: bool,
    props: Vec<roxmltree::Node<'a, 'input>>,
}

fn responses<'a, 'input>(doc: &'a roxmltree::Document<'input>) -> Vec<Response<'a, 'input>> {
    let is = |n: &roxmltree::Node, ns: &str, name: &str| n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns);
    doc.root_element()
        .children()
        .filter(|n| is(n, DAV, "response"))
        .map(|r| {
            let href = r.children().find(|n| is(n, DAV, "href")).and_then(|n| n.text()).unwrap_or_default().trim().to_owned();
            let status = |n: &roxmltree::Node| n.children().find(|c| is(c, DAV, "status")).and_then(|s| s.text()).map(|t| t.to_owned()).unwrap_or_default();
            let gone = status(&r).contains(" 404") || status(&r).contains(" 410");
            let props = r
                .children()
                .filter(|n| is(n, DAV, "propstat") && (status(n).is_empty() || status(n).contains(" 200")))
                .flat_map(|ps| ps.children().filter(|n| is(n, DAV, "prop")).flat_map(|p| p.children().filter(|c| c.is_element())))
                .collect();
            Response { href, gone, props }
        })
        .collect()
}

impl Response<'_, '_> {
    fn prop(&self, ns: &str, name: &str) -> Option<roxmltree::Node<'_, '_>> {
        self.props.iter().copied().find(|n| n.tag_name().name() == name && n.tag_name().namespace() == Some(ns))
    }

    fn text(&self, ns: &str, name: &str) -> Option<String> {
        self.prop(ns, name).map(|n| n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>().trim().to_owned()).filter(|t| !t.is_empty())
    }

    /// The address in a property that holds one (`<href>` inside it).
    fn href_in(&self, ns: &str, name: &str) -> Option<String> {
        self.prop(ns, name)?.descendants().find(|n| n.is_element() && n.tag_name().name() == "href").and_then(|n| n.text()).map(|t| t.trim().to_owned())
    }
}

/// Where a token comes from, each time one is wanted (a sign-in whose
/// tokens last an hour: Google's). Asked with `true` for one that's new
/// (the one it gave was refused).
pub type Tokens = Box<dyn Fn(bool) -> Result<String, Error> + Send + Sync>;

/// How long the network may be away before something that reads gives up.
const PATIENCE: Duration = Duration::from_secs(20);

/// An account on a server.
pub struct Server {
    agent: olo_net::PerNetwork<ureq::Agent>,
    auth: Auth,
    tokens: Option<Tokens>,
}

fn agent() -> ureq::Agent {
    // TLS by rustls with the system's certificates, as Olo's other
    // clients. No status turned into an error by the agent (each is read
    // here), and WebDAV's own methods let through.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).timeout_global(Some(Duration::from_secs(60))).http_status_as_error(false).allow_non_standard_methods(true).max_redirects(4).build().into()
}

impl Server {
    pub fn new(auth: Auth) -> Server {
        Server { agent: olo_net::PerNetwork::new(agent), auth, tokens: None }
    }

    /// Signed in with a token asked of `tokens` for each request (which
    /// keeps the one it has while it's good). A request refused with the
    /// token it was given is made once more with a new one: a token's
    /// hour can end without the clock here saying so (the computer
    /// asleep meanwhile).
    pub fn with_tokens(tokens: impl Fn(bool) -> Result<String, Error> + Send + Sync + 'static) -> Server {
        Server { agent: olo_net::PerNetwork::new(agent), auth: Auth::Bearer(String::new()), tokens: Some(Box::new(tokens)) }
    }

    fn authorization(&self, fresh: bool) -> Result<String, Error> {
        use base64::Engine as _;
        if let Some(tokens) = &self.tokens {
            return Ok(format!("Bearer {}", tokens(fresh)?));
        }
        Ok(match &self.auth {
            Auth::Basic { user, password } => format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))),
            Auth::Bearer(token) => format!("Bearer {token}"),
        })
    }

    /// One request: its status, its `ETag` and its body. What only reads
    /// is asked again when the network changed under it.
    fn ask(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, Option<String>, String), Error> {
        if matches!(method, "PROPFIND" | "REPORT" | "GET") { olo_net::across_changes(PATIENCE, || self.ask_once(method, url, headers, body)) } else { self.ask_once(method, url, headers, body) }
    }

    fn ask_once(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, Option<String>, String), Error> {
        match self.send(method, url, headers, body, false)? {
            // (Refused with a token: once more, with a new one.)
            (401, ..) if self.tokens.is_some() => self.read(self.send(method, url, headers, body, true)?),
            answer => self.read(answer),
        }
    }

    /// An answer as what it means.
    fn read(&self, (status, etag, text): (u16, Option<String>, String)) -> Result<(u16, Option<String>, String), Error> {
        match status {
            401 | 403 => Err(Error::SignIn),
            404 | 410 => Err(Error::Gone),
            412 => Err(Error::Changed),
            _ => Ok((status, etag, text)),
        }
    }

    /// A request sent: whatever the server answered.
    fn send(&self, method: &str, url: &str, headers: &[(&str, &str)], body: &str, fresh: bool) -> Result<(u16, Option<String>, String), Error> {
        let mut request = ureq::http::Request::builder().method(method).uri(url).header("Authorization", self.authorization(fresh)?).header("User-Agent", "Olo-Calendar");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(body.to_owned()).map_err(|e| Error::Network(e.to_string()))?;
        let mut answer = self.agent.get().run(request).map_err(|e| Error::Network(e.to_string()))?;
        let status = answer.status().as_u16();
        let etag = answer.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let text = answer.body_mut().read_to_string().map_err(|e| Error::Network(e.to_string()))?;
        Ok((status, etag, text))
    }

    /// A PROPFIND or a REPORT: its multistatus, read by `read`.
    fn multistatus<T>(&self, method: &str, url: &str, depth: &str, body: &str, read: impl FnOnce(Vec<Response>) -> T) -> Result<T, Error> {
        let (status, _, text) = self.ask(method, url, &[("Depth", depth), ("Content-Type", "application/xml; charset=utf-8")], body)?;
        if status != 207 {
            return Err(Error::Status(status));
        }
        let doc = roxmltree::Document::parse(&text).map_err(|e| Error::Answer(e.to_string()))?;
        Ok(read(responses(&doc)))
    }

    /// The calendars the account has, found from `start` (the server's
    /// address, or anywhere under it): who is signed in there, where
    /// their calendars are kept, and each calendar that holds events.
    pub fn calendars(&self, start: &str) -> Result<Vec<Collection>, Error> {
        let ask_one = |url: &str, prop: &str, ns: &str, name: &str| -> Result<Option<String>, Error> {
            let body = format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="{CAL}"><d:prop>{prop}</d:prop></d:propfind>"#);
            self.multistatus("PROPFIND", url, "0", &body, |rs| rs.first().and_then(|r| r.href_in(ns, name)))
        };
        let principal = ask_one(start, "<d:current-user-principal/>", DAV, "current-user-principal")?.map(|h| resolve(start, &h)).unwrap_or_else(|| start.to_owned());
        let home = ask_one(&principal, "<c:calendar-home-set/>", CAL, "calendar-home-set")?.map(|h| resolve(&principal, &h)).unwrap_or_else(|| principal.clone());
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="{CAL}" xmlns:cs="{CS}" xmlns:a="{APPLE}"><d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><cs:getctag/><d:sync-token/><c:supported-calendar-component-set/><d:current-user-privilege-set/></d:prop></d:propfind>"#
        );
        self.multistatus("PROPFIND", &home, "1", &body, |rs| {
            rs.iter()
                .filter(|r| r.prop(DAV, "resourcetype").is_some_and(|t| t.children().any(|c| c.is_element() && c.tag_name().name() == "calendar")))
                // (One for events: not a list of tasks only.)
                .filter(|r| r.prop(CAL, "supported-calendar-component-set").is_none_or(|s| s.children().any(|c| c.attribute("name").is_some_and(|n| n.eq_ignore_ascii_case("VEVENT")))))
                .map(|r| {
                    let href = resolve(&home, &r.href);
                    let name = r.text(DAV, "displayname").unwrap_or_else(|| href.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned());
                    // ("#RRGGBBAA" as Apple's clients write it: the colour alone.)
                    let color = r.text(APPLE, "calendar-color").filter(|c| c.starts_with('#') && c.len() >= 7).map(|c| c[..7].to_ascii_uppercase());
                    let writable =
                        r.prop(DAV, "current-user-privilege-set").is_none_or(|p| p.descendants().any(|n| n.is_element() && matches!(n.tag_name().name(), "write" | "write-content" | "all")));
                    Collection { href, name, color, ctag: r.text(CS, "getctag"), sync_token: r.text(DAV, "sync-token"), writable }
                })
                .collect()
        })
    }

    /// Every resource of a calendar with its tag (not its text).
    pub fn list(&self, calendar: &str) -> Result<Vec<Resource>, Error> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/><d:resourcetype/></d:prop></d:propfind>"#;
        self.multistatus("PROPFIND", calendar, "1", body, |rs| {
            rs.iter()
                .filter(|r| !same_href(&r.href, calendar) && !r.prop(DAV, "resourcetype").is_some_and(|t| t.children().any(|c| c.is_element())))
                .filter_map(|r| Some(Resource { href: resolve(calendar, &r.href), etag: r.text(DAV, "getetag")?, data: None }))
                .collect()
        })
    }

    /// What changed in a calendar since `token` (None: everything in it,
    /// and a token to ask with next time).
    pub fn changes(&self, calendar: &str, token: Option<&str>) -> Result<Changes, Error> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            xml_escape(token.unwrap_or_default())
        );
        let (status, _, text) = self.ask("REPORT", calendar, &[("Depth", "1"), ("Content-Type", "application/xml; charset=utf-8")], &body).map_err(|e| match e {
            // (A token the server no longer knows: 403 with valid-sync-token, or 410.)
            Error::SignIn | Error::Gone | Error::Changed if token.is_some() => Error::StaleToken,
            e => e,
        })?;
        if status != 207 {
            return Err(if token.is_some() && matches!(status, 400 | 409) { Error::StaleToken } else { Error::Status(status) });
        }
        let doc = roxmltree::Document::parse(&text).map_err(|e| Error::Answer(e.to_string()))?;
        let token = doc.root_element().children().find(|n| n.is_element() && n.tag_name().name() == "sync-token").and_then(|n| n.text()).map(|t| t.trim().to_owned());
        let mut changes = Changes { token, ..Default::default() };
        for r in responses(&doc) {
            if same_href(&r.href, calendar) {
                continue;
            }
            let href = resolve(calendar, &r.href);
            match (r.gone, r.text(DAV, "getetag")) {
                (true, _) => changes.removed.push(href),
                (false, Some(etag)) => changes.changed.push(Resource { href, etag, data: None }),
                (false, None) => {}
            }
        }
        Ok(changes)
    }

    /// These resources of a calendar with their text (asked for fifty at
    /// a time).
    pub fn fetch(&self, calendar: &str, hrefs: &[String]) -> Result<Vec<Resource>, Error> {
        let mut out = Vec::new();
        for some in hrefs.chunks(50) {
            let list: String = some
                .iter()
                .map(|h| {
                    // (As a path, which every server takes.)
                    let path = h.find("://").and_then(|i| h[i + 3..].find('/').map(|j| &h[i + 3 + j..])).unwrap_or(h);
                    format!("<d:href>{}</d:href>", xml_escape(path))
                })
                .collect();
            let body =
                format!(r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-multiget xmlns:d="DAV:" xmlns:c="{CAL}"><d:prop><d:getetag/><c:calendar-data/></d:prop>{list}</c:calendar-multiget>"#);
            out.extend(self.multistatus("REPORT", calendar, "1", &body, |rs| {
                rs.iter().filter_map(|r| Some(Resource { href: resolve(calendar, &r.href), etag: r.text(DAV, "getetag")?, data: Some(r.text(CAL, "calendar-data")?) })).collect::<Vec<_>>()
            })?);
        }
        Ok(out)
    }

    /// Put `ics` at `href`: a new resource (`etag` None: only if nothing
    /// is there), or in place of the one read with `etag` (only if it's
    /// still that one). Its new tag, where the server says (else read it).
    pub fn put(&self, href: &str, ics: &str, etag: Option<&str>) -> Result<Option<String>, Error> {
        let condition = match etag {
            Some(tag) => ("If-Match", tag),
            None => ("If-None-Match", "*"),
        };
        let (status, new, _) = self.ask("PUT", href, &[("Content-Type", "text/calendar; charset=utf-8"), condition], ics)?;
        if matches!(status, 200 | 201 | 204) { Ok(new) } else { Err(Error::Status(status)) }
    }

    /// Remove the resource read with `etag`, if it's still that one.
    pub fn delete(&self, href: &str, etag: &str) -> Result<(), Error> {
        match self.ask("DELETE", href, &[("If-Match", etag)], "") {
            Ok((200 | 204, ..)) | Err(Error::Gone) => Ok(()),
            Ok((status, ..)) => Err(Error::Status(status)),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standin::Standin;

    fn ics(uid: &str, summary: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261120T140000Z\r\nDTEND:20261120T150000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
    }

    /// An account's calendars are found from the server's address alone
    /// (who is signed in, where their calendars are, each one of events),
    /// with their names, colours and whether they may be written to.
    #[test]
    fn the_calendars_an_account_has() {
        let there = Standin::start("ben@example.org", "app-password");
        there.add_calendar("home", "Home & family", Some("#ff8800"));
        there.add_calendar("work", "Work", None);
        there.set_read_only("work", true);
        let server = Server::new(there.auth());
        let found = server.calendars(&there.url()).unwrap();
        assert_eq!(found.iter().map(|c| (c.name.as_str(), c.color.as_deref(), c.writable)).collect::<Vec<_>>(), [("Home & family", Some("#FF8800"), true), ("Work", None, false)]);
        assert_eq!(found[0].href, format!("http://127.0.0.1:{}/cal/home/", there.port));
        assert!(found[0].sync_token.is_some() && found[0].ctag.is_some());
        assert_eq!(there.asked(), ["PROPFIND /", "PROPFIND /principal/", "PROPFIND /cal/"]);
        // A wrong password: said as that.
        let wrong = Server::new(Auth::Basic { user: "ben@example.org".into(), password: "wrong".into() });
        assert_eq!(wrong.calendars(&there.url()), Err(Error::SignIn));
        // No server there at all.
        let nowhere = Server::new(there.auth());
        assert!(matches!(nowhere.calendars("http://127.0.0.1:9/"), Err(Error::Network(_))));
    }

    /// What is in a calendar, what changed in it since a token (changed,
    /// added, removed, by another device), the text of those wanted; and
    /// a token the server no longer knows is said as that.
    #[test]
    fn what_is_in_a_calendar_and_what_changed() {
        let there = Standin::start("ben@example.org", "pw");
        there.add_calendar("home", "Home", None);
        there.put("home", "a.ics", &ics("a", "Lunch"));
        there.put("home", "b.ics", &ics("b", "Dentist <at 3> & after"));
        let server = Server::new(there.auth());
        let cal = format!("{}cal/home/", there.url());
        let listed = server.list(&cal).unwrap();
        assert_eq!(listed.iter().map(|r| r.href.rsplit('/').next().unwrap()).collect::<Vec<_>>(), ["a.ics", "b.ics"]);
        // Everything, the first time, and a token.
        let first = server.changes(&cal, None).unwrap();
        assert_eq!((first.changed.len(), first.removed.len()), (2, 0));
        let token = first.token.clone().expect("a token");
        let got = server.fetch(&cal, &first.changed.iter().map(|r| r.href.clone()).collect::<Vec<_>>()).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got[1].data.as_deref().unwrap().contains("SUMMARY:Dentist <at 3> & after"), "{:?}", got[1].data);
        assert_eq!(got[0].etag, listed[0].etag);
        // Nothing since.
        let none = server.changes(&cal, Some(&token)).unwrap();
        assert!(none.changed.is_empty() && none.removed.is_empty());
        // Another device changes one, adds one and removes one.
        there.put("home", "a.ics", &ics("a", "Lunch, later"));
        there.put("home", "c.ics", &ics("c", "Run"));
        there.remove("home", "b.ics");
        let since = server.changes(&cal, Some(&token)).unwrap();
        let mut changed: Vec<&str> = since.changed.iter().map(|r| r.href.rsplit('/').next().unwrap()).collect();
        changed.sort_unstable();
        assert_eq!((changed, since.removed.iter().map(|h| h.rsplit('/').next().unwrap()).collect::<Vec<_>>()), (vec!["a.ics", "c.ics"], vec!["b.ics"]));
        assert_ne!(since.token, Some(token.clone()));
        // The server forgot: said, so everything is read again.
        there.forget_changes();
        assert_eq!(server.changes(&cal, Some(&token)), Err(Error::StaleToken));
        assert_eq!(server.changes(&cal, None).unwrap().changed.len(), 2);
    }

    /// An event is put there only if nothing is; changed only if it's
    /// still the one that was read; removed the same way: never over a
    /// change made somewhere else meanwhile.
    #[test]
    fn written_never_over_someone_elses_change() {
        let there = Standin::start("ben@example.org", "pw");
        there.add_calendar("home", "Home", None);
        there.add_calendar("shared", "Shared", None);
        there.set_read_only("shared", true);
        let server = Server::new(there.auth());
        let href = format!("{}cal/home/new.ics", there.url());
        let etag = server.put(&href, &ics("new", "Lunch"), None).unwrap().expect("its tag");
        assert_eq!(there.get("home", "new.ics").as_deref(), Some(ics("new", "Lunch").as_str()));
        // Already there: not put over it as if it were new.
        assert_eq!(server.put(&href, &ics("new", "Other"), None), Err(Error::Changed));
        // Changed, with the tag it was read with.
        let etag2 = server.put(&href, &ics("new", "Lunch at one"), Some(&etag)).unwrap().unwrap();
        assert_ne!(etag, etag2);
        // Changed by another device since: ours is refused, theirs stays.
        there.put("home", "new.ics", &ics("new", "Theirs"));
        assert_eq!(server.put(&href, &ics("new", "Mine"), Some(&etag2)), Err(Error::Changed));
        assert_eq!(server.delete(&href, &etag2), Err(Error::Changed));
        assert!(there.get("home", "new.ics").unwrap().contains("Theirs"));
        // Removed, with the tag it has now; removed again (gone already): fine.
        let now = server.list(&format!("{}cal/home/", there.url())).unwrap().remove(0).etag;
        assert_eq!(server.delete(&href, &now), Ok(()));
        assert_eq!(server.delete(&href, &now), Ok(()));
        assert!(there.files("home").is_empty());
        // A calendar that may only be read: said.
        assert_eq!(server.put(&format!("{}cal/shared/x.ics", there.url()), &ics("x", "X"), None), Err(Error::SignIn));
    }

    #[test]
    fn addresses() {
        assert_eq!(resolve("https://caldav.example.org/dav/", "/dav/calendars/user/"), "https://caldav.example.org/dav/calendars/user/");
        assert_eq!(resolve("https://caldav.example.org", "/x/"), "https://caldav.example.org/x/");
        assert_eq!(resolve("https://a.example/dav/cal/", "https://b.example/other/"), "https://b.example/other/");
        assert_eq!(resolve("https://a.example/dav/cal/", "one.ics"), "https://a.example/dav/cal/one.ics");
        assert!(same_href("https://a.example/cal/ben%40example.org/x.ics", "/cal/ben@example.org/x.ics"));
        assert!(same_href("/cal/home/", "https://a.example/cal/home") && !same_href("/cal/home/", "/cal/work/"));
    }
}
