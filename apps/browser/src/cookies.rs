//! Cookies (RFC 6265bis): the browser's one jar, which every page's
//! requests and scripts (`document.cookie`) share. What a response's
//! `Set-Cookie` says is kept and sent back with the requests it matches,
//! as Chrome does: by domain, path and scheme, SameSite by the site the
//! page is on (Lax when unsaid), never HttpOnly to scripts. Cookies with
//! an expiry are kept on disk (`~/.local/state/olo/browser/cookies.json`,
//! or the Agent Screen's own profile), written a second after the last
//! change on a thread of their own; the rest last as long as the browser.
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SameSite {
    Strict,
    Lax,
    None,
    /// Unsaid: Lax, as Chrome takes it.
    Unset,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cookie {
    name: String,
    value: String,
    /// The host it was set by (host-only) or the domain it names, in
    /// lower case without a leading dot.
    domain: String,
    host_only: bool,
    path: String,
    /// Unix seconds; none for a session cookie.
    expires: Option<i64>,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
    /// When it was first set (µs since the epoch), for order and eviction.
    created: i64,
}
/// How a request goes, for which cookies it may carry.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The page the request is made for (its top-level document), or none
    /// for a navigation of the page itself.
    pub site: Option<&'a Url>,
    /// A top-level navigation (Lax cookies go cross-site with a safe one).
    pub navigation: bool,
    /// GET or HEAD.
    pub safe: bool,
}
pub struct Jar {
    cookies: Mutex<Vec<Cookie>>,
    save: Option<mpsc::Sender<()>>,
}
/// Most cookies kept for one site, and in all (Chrome's numbers).
const PER_SITE: usize = 180;
const TOTAL: usize = 3300;
/// A cookie's name and value together, at most (bytes).
const COOKIE_BYTES: usize = 4096;

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}
fn now_micros() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64)
}
/// Where the jar is kept: the Agent Screen's profile when the browser runs
/// on one (its pages' cookies are the screen's, not the person's), else
/// the person's state directory.
fn file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OLO_BROWSER_COOKIES") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute() && h != std::path::Path::new("/"))?;
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/state"));
    Some(match std::env::var("OLO_SCREEN").ok().filter(|s| !s.is_empty() && !s.contains('/') && s != "." && s != "..") {
        Some(screen) => state.join("olo/screens").join(screen).join("olo-browser/cookies.json"),
        None => state.join("olo/browser/cookies.json"),
    })
}
/// The browser's jar, loaded from disk the first time it is asked for.
pub fn jar() -> &'static Jar {
    static JAR: OnceLock<Jar> = OnceLock::new();
    JAR.get_or_init(|| Jar::open(file()))
}
impl Jar {
    fn open(path: Option<PathBuf>) -> Self {
        let now = now_seconds();
        let cookies: Vec<Cookie> = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<Cookie>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.expires.is_some_and(|e| e > now))
            .collect();
        let save = path.map(|path| {
            let (tx, rx) = mpsc::channel::<()>();
            let _ = std::thread::Builder::new().name("browser-cookies".into()).spawn(move || {
                // A change wakes it; it waits a second for more, then writes
                // what the jar keeps (nothing runs while nothing changes).
                while rx.recv().is_ok() {
                    std::thread::sleep(Duration::from_secs(1));
                    while rx.try_recv().is_ok() {}
                    jar().write(&path);
                }
            });
            tx
        });
        Self { cookies: Mutex::new(cookies), save }
    }
    /// A jar of its own, kept nowhere (tests).
    #[cfg(test)]
    fn memory() -> Self {
        Self { cookies: Mutex::new(Vec::new()), save: None }
    }
    fn write(&self, path: &std::path::Path) {
        let now = now_seconds();
        let kept: Vec<Cookie> = self.cookies.lock().unwrap().iter().filter(|c| c.expires.is_some_and(|e| e > now)).cloned().collect();
        let Ok(bytes) = serde_json::to_vec(&kept) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let partial = path.with_extension("json.partial");
        if std::fs::write(&partial, bytes).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&partial, path);
        }
    }
    fn changed(&self, persistent: bool) {
        if persistent && let Some(tx) = &self.save {
            let _ = tx.send(());
        }
    }
    /// A response's `Set-Cookie` header for `url`.
    pub fn set_from_response(&self, url: &Url, line: &str) {
        self.set(url, line, true);
    }
    /// `document.cookie = line` on a page at `url` (HttpOnly cookies are
    /// neither set nor replaced).
    pub fn set_from_script(&self, url: &Url, line: &str) {
        self.set(url, line, false);
    }
    fn set(&self, url: &Url, line: &str, http: bool) {
        if !matches!(url.scheme(), "http" | "https") {
            return;
        }
        let Some(host) = url.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()) else { return };
        let Some(mut cookie) = parse(line, url, &host, now_seconds()) else { return };
        if cookie.http_only && !http {
            return;
        }
        let secure_origin = url.scheme() == "https" || host == "localhost" || host == "127.0.0.1" || host == "[::1]";
        // Secure only from a secure origin; SameSite=None only Secure;
        // the name prefixes as they promise.
        if cookie.secure && !secure_origin || cookie.same_site == SameSite::None && !cookie.secure {
            return;
        }
        if cookie.name.starts_with("__Secure-") && !cookie.secure || cookie.name.starts_with("__Host-") && (!cookie.secure || !cookie.host_only || cookie.path != "/") {
            return;
        }
        let mut cookies = self.cookies.lock().unwrap();
        // An insecure page leaves a secure cookie of the name alone.
        if !secure_origin && cookies.iter().any(|c| c.secure && c.name == cookie.name && (domain_matches(&host, c) || domain_matches(&c.domain, &cookie)) && path_matches(&cookie.path, &c.path)) {
            return;
        }
        let persistent = cookie.expires.is_some();
        if let Some(i) = cookies.iter().position(|c| c.name == cookie.name && c.domain == cookie.domain && c.host_only == cookie.host_only && c.path == cookie.path) {
            if cookies[i].http_only && !http {
                return;
            }
            let old = cookies.remove(i);
            cookie.created = old.created;
            let was_persistent = old.expires.is_some();
            if cookie.expires.is_some_and(|e| e <= now_seconds()) {
                drop(cookies);
                self.changed(was_persistent);
                return;
            }
            cookies.push(cookie);
            Self::evict(&mut cookies);
            drop(cookies);
            self.changed(persistent || was_persistent);
            return;
        }
        if cookie.expires.is_some_and(|e| e <= now_seconds()) {
            return;
        }
        cookies.push(cookie);
        Self::evict(&mut cookies);
        drop(cookies);
        self.changed(persistent);
    }
    /// Expired cookies out, then the oldest of a site past its share and of
    /// the jar past its size.
    fn evict(cookies: &mut Vec<Cookie>) {
        let now = now_seconds();
        cookies.retain(|c| c.expires.is_none_or(|e| e > now));
        if let Some(last) = cookies.last() {
            let site = site_of(&last.domain);
            let mut of_site: Vec<usize> = (0..cookies.len()).filter(|&i| site_of(&cookies[i].domain) == site).collect();
            if of_site.len() > PER_SITE {
                of_site.sort_by_key(|&i| cookies[i].created);
                let drop: std::collections::HashSet<usize> = of_site[..of_site.len() - PER_SITE].iter().copied().collect();
                let mut i = 0;
                cookies.retain(|_| {
                    i += 1;
                    !drop.contains(&(i - 1))
                });
            }
        }
        if cookies.len() > TOTAL {
            cookies.sort_by_key(|c| c.created);
            let extra = cookies.len() - TOTAL;
            cookies.drain(..extra);
        }
    }
    /// The `Cookie` header for a request to `url` made as `context` says,
    /// if any cookie goes with it.
    pub fn header(&self, url: &Url, context: Context<'_>) -> Option<String> {
        let pairs = self.matching(url, Some(context), true);
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }
    /// `document.cookie` for a page at `url`: its cookies but HttpOnly ones.
    pub fn for_script(&self, url: &Url) -> String {
        self.matching(url, None, false).join("; ")
    }
    fn matching(&self, url: &Url, context: Option<Context<'_>>, http: bool) -> Vec<String> {
        if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
            return Vec::new();
        }
        let Some(host) = url.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()) else { return Vec::new() };
        let secure = matches!(url.scheme(), "https" | "wss") || host == "localhost" || host == "127.0.0.1" || host == "[::1]";
        let path = url.path();
        let now = now_seconds();
        // Same-site when the request's site is the page's (schemes too).
        let cross_site = context.and_then(|c| c.site).is_some_and(|site| site_of(site.host_str().unwrap_or("")) != site_of(&host) || (site.scheme() == "https") != secure);
        let cookies = self.cookies.lock().unwrap();
        let mut found: Vec<&Cookie> = cookies
            .iter()
            .filter(|c| {
                domain_matches(&host, c)
                    && path_matches(path, &c.path)
                    && (!c.secure || secure)
                    && c.expires.is_none_or(|e| e > now)
                    && (http || !c.http_only)
                    && (!cross_site
                        || match c.same_site {
                            SameSite::None => true,
                            SameSite::Strict => false,
                            SameSite::Lax | SameSite::Unset => context.is_some_and(|c| c.navigation && c.safe),
                        })
            })
            .collect();
        // Longer paths first, then the older.
        found.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created.cmp(&b.created)));
        found.iter().map(|c| if c.name.is_empty() { c.value.clone() } else { format!("{}={}", c.name, c.value) }).collect()
    }
}
/// `document.cookie` for a page's scripts, through the jar: none for a
/// page with no web origin (a file), whose scripts keep their own.
pub fn script_access() -> olo_js::Cookies {
    Box::new(|url: &str, line: Option<&str>| {
        let url = Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
        Some(match line {
            Some(line) => {
                jar().set_from_script(&url, line);
                String::new()
            }
            None => jar().for_script(&url),
        })
    })
}
/// The registrable domain (eTLD+1) of a host, or the host itself (an IP
/// address, a suffix, a name with no suffix).
fn site_of(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return host;
    }
    psl::domain_str(&host).map_or(host.clone(), str::to_owned)
}
fn is_public_suffix(domain: &str) -> bool {
    psl::suffix_str(domain).is_some_and(|s| s == domain) && psl::domain_str(domain).is_none()
}
/// RFC 6265 §5.1.3: the host is the cookie's (host-only), or within its domain.
fn domain_matches(host: &str, c: &Cookie) -> bool {
    if c.host_only {
        return host == c.domain;
    }
    host == c.domain || host.ends_with(&c.domain) && host.as_bytes().get(host.len() - c.domain.len() - 1) == Some(&b'.') && host.parse::<std::net::IpAddr>().is_err()
}
/// RFC 6265 §5.1.4.
fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie || request.starts_with(cookie) && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/'))
}
/// RFC 6265 §5.1.4: the directory of the request's path.
fn default_path(url: &Url) -> String {
    let path = url.path();
    if !path.starts_with('/') {
        return "/".into();
    }
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => path[..i].to_owned(),
    }
}
/// A `Set-Cookie` line (RFC 6265bis §5.6) from `url` (host `host`), as
/// the cookie to store, or none when it is to be ignored.
fn parse(line: &str, url: &Url, host: &str, now: i64) -> Option<Cookie> {
    let mut parts = line.split(';');
    let pair = parts.next()?;
    if pair.bytes().any(|b| b < 0x20 && b != b'\t' || b == 0x7f) {
        return None;
    }
    let (name, value) = match pair.split_once('=') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => ("", pair.trim()),
    };
    if name.is_empty() && value.is_empty() || name.len() + value.len() > COOKIE_BYTES {
        return None;
    }
    let mut cookie = Cookie {
        name: name.to_owned(),
        value: value.to_owned(),
        domain: host.to_owned(),
        host_only: true,
        path: default_path(url),
        expires: None,
        secure: false,
        http_only: false,
        same_site: SameSite::Unset,
        created: now_micros(),
    };
    let (mut max_age, mut expires) = (None, None);
    for attribute in parts {
        let (key, value) = match attribute.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (attribute.trim(), ""),
        };
        if value.len() > 1024 {
            continue;
        }
        match key.to_ascii_lowercase().as_str() {
            "expires" => {
                if let Some(t) = cookie_date(value) {
                    expires = Some(t);
                }
            }
            "max-age" => {
                let digits = value.strip_prefix('-').unwrap_or(value);
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    let n: i64 = digits.parse().unwrap_or(i64::MAX);
                    max_age = Some(if value.starts_with('-') || n == 0 { i64::MIN } else { now.saturating_add(n.min(400 * 86400)) });
                }
            }
            "domain" => {
                let d = value.strip_prefix('.').unwrap_or(value).trim_end_matches('.').to_ascii_lowercase();
                if !d.is_empty() {
                    cookie.domain = d;
                    cookie.host_only = false;
                }
            }
            "path" => cookie.path = if value.starts_with('/') { value.to_owned() } else { default_path(url) },
            "secure" => cookie.secure = true,
            "httponly" => cookie.http_only = true,
            "samesite" => {
                cookie.same_site = match value.to_ascii_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "lax" => SameSite::Lax,
                    "none" => SameSite::None,
                    _ => SameSite::Unset,
                }
            }
            _ => {}
        }
    }
    // Max-Age over Expires; at most 400 days ahead (RFC 6265bis §5.5).
    cookie.expires = max_age.or(expires.map(|e| e.min(now + 400 * 86400)));
    if !cookie.host_only {
        // A public suffix is only the host's own, host-only; another
        // domain must hold the host.
        if is_public_suffix(&cookie.domain) {
            if cookie.domain != host {
                return None;
            }
            cookie.host_only = true;
        } else if !(host == cookie.domain || host.ends_with(&format!(".{}", cookie.domain)) && host.parse::<std::net::IpAddr>().is_err()) {
            return None;
        }
    }
    Some(cookie)
}
/// RFC 6265 §5.1.1's cookie-date: unix seconds.
fn cookie_date(text: &str) -> Option<i64> {
    let delimiter = |c: char| matches!(c, '\t' | ' '..='/' | ';'..='@' | '['..='`' | '{'..='~');
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in text.split(delimiter).filter(|t| !t.is_empty()) {
        let digits = |t: &str, min: usize, max: usize| -> Option<u32> {
            let n = t.bytes().take_while(u8::is_ascii_digit).count();
            (min..=max).contains(&n).then(|| t[..n].parse().ok()).flatten()
        };
        if time.is_none() {
            let parts: Vec<&str> = token.splitn(3, ':').collect();
            if parts.len() == 3
                && let (Some(h), Some(m), Some(s)) = (digits(parts[0], 1, 2), digits(parts[1], 1, 2), digits(parts[2], 1, 2))
                && parts[0].len() <= 2
                && parts[1].len() <= 2
            {
                time = Some((h, m, s));
                continue;
            }
        }
        if day.is_none()
            && let Some(d) = digits(token, 1, 2)
        {
            day = Some(d);
            continue;
        }
        if month.is_none()
            && let Some(head) = token.get(..3)
        {
            let m = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| head.eq_ignore_ascii_case(m));
            if let Some(m) = m {
                month = Some(m as u32 + 1);
                continue;
            }
        }
        if year.is_none()
            && let Some(y) = digits(token, 2, 4)
        {
            year = Some(y);
        }
    }
    let (h, mi, s) = time?;
    let (d, mo, mut y) = (day?, month?, year?);
    if (70..=99).contains(&y) {
        y += 1900;
    } else if y <= 69 {
        y += 2000;
    }
    if !(1..=31).contains(&d) || y < 1601 || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = (i64::from(if mo <= 2 { y - 1 } else { y }), i64::from(mo));
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }
    fn top() -> Context<'static> {
        Context { site: None, navigation: true, safe: true }
    }
    #[test]
    fn cookies_go_back_by_domain_path_and_scheme() {
        let jar = Jar::memory();
        let page = url("https://www.example.com/a/b");
        jar.set_from_response(&page, "sid=1; Path=/; Secure; HttpOnly");
        jar.set_from_response(&page, "pref=dark; Domain=example.com; Path=/; Max-Age=3600");
        jar.set_from_response(&page, "here=x");
        assert_eq!(jar.header(&url("https://www.example.com/a/c"), top()).as_deref(), Some("here=x; sid=1; pref=dark"));
        assert_eq!(jar.header(&url("https://cdn.example.com/"), top()).as_deref(), Some("pref=dark"));
        assert_eq!(jar.header(&url("http://www.example.com/"), top()).as_deref(), Some("pref=dark"), "a secure cookie stays off http");
        assert_eq!(jar.for_script(&page), "here=x; pref=dark", "HttpOnly stays from scripts");
        jar.set_from_script(&page, "sid=2; Path=/");
        assert!(jar.header(&page, top()).unwrap().contains("sid=1"), "a script can't replace an HttpOnly cookie");
        jar.set_from_response(&page, "pref=; Domain=example.com; Path=/; Max-Age=0");
        assert_eq!(jar.for_script(&page), "here=x");
    }
    #[test]
    fn a_public_suffix_or_another_domain_is_refused() {
        let jar = Jar::memory();
        let page = url("https://www.example.co.uk/");
        jar.set_from_response(&page, "a=1; Domain=co.uk");
        jar.set_from_response(&page, "b=1; Domain=other.co.uk");
        jar.set_from_response(&page, "c=1; Domain=example.co.uk");
        assert_eq!(jar.for_script(&page), "c=1");
    }
    #[test]
    fn samesite_holds_cross_site_requests() {
        let jar = Jar::memory();
        let api = url("https://api.example.com/");
        jar.set_from_response(&api, "lax=1");
        jar.set_from_response(&api, "strict=1; SameSite=Strict");
        jar.set_from_response(&api, "none=1; SameSite=None; Secure");
        jar.set_from_response(&api, "insecure=1; SameSite=None");
        let other = url("https://other.org/");
        let fetch = Context { site: Some(&other), navigation: false, safe: true };
        assert_eq!(jar.header(&api, fetch).as_deref(), Some("none=1"));
        let link = Context { site: Some(&other), navigation: true, safe: true };
        assert_eq!(jar.header(&api, link).as_deref(), Some("lax=1; none=1"));
        let same = url("https://www.example.com/");
        assert_eq!(jar.header(&api, Context { site: Some(&same), navigation: false, safe: false }).as_deref(), Some("lax=1; strict=1; none=1"));
    }
    #[test]
    fn cookie_dates_parse_as_browsers_read_them() {
        assert_eq!(cookie_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1445412480));
        assert_eq!(cookie_date("Wed, 21-Oct-15 07:28:00 GMT"), Some(1445412480));
        assert_eq!(cookie_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(cookie_date("not a date"), None);
    }
}
