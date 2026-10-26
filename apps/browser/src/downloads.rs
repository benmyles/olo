//! Downloads. Servo 0.7 has none, so NetPositive fetches files itself on a
//! thread per download (ureq over rustls, with Servo's cookies for the URL,
//! the page as referrer and Servo's user agent) into the Downloads folder.
//!
//! What starts one: Download Linked File / Download Image in the page's
//! context menu; a link with a `download` attribute (the page script
//! catches the click; `blob:` contents are read in the page and sent over
//! in pieces, `data:` URLs are decoded here); and a navigation to
//! something Servo can't show, which it answers with its "Unknown content
//! type" page: the script notices that page and NetPositive fetches the
//! URL again as a download and goes back.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Deserialize;
use tabula_kit::Proxy;

/// What the page script reports.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PageDownload {
    /// The page is Servo's "Unknown content type" page for `url`.
    Unknown { url: String },
    /// A `download` link to `url` was clicked.
    Link { url: String, name: String },
    /// A piece of a `blob:` download's contents (base64).
    Data { id: u64, name: String, seq: u64, last: bool, data: String },
}

/// Where a download's bytes come from.
pub enum Source {
    Http { url: String, cookies: String, referer: Option<String>, user_agent: String },
    /// What a page handed over, gathered in a file (of this length).
    File(fs::File, u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed(String),
    Stopped,
}

pub struct Download {
    pub id: u64,
    pub url: String,
    pub name: String,
    /// Where it is (or will be once finished).
    pub path: Option<PathBuf>,
    pub total: Option<u64>,
    pub received: u64,
    pub state: State,
    cancel: Arc<AtomicBool>,
    /// Bytes a second, smoothed.
    pub rate: f64,
    last: (Instant, u64),
}

impl Download {
    pub fn new(id: u64, url: String, name: String) -> (Download, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let d = Download {
            id,
            url,
            name,
            path: None,
            total: None,
            received: 0,
            state: State::Running,
            cancel: cancel.clone(),
            rate: 0.0,
            last: (Instant::now(), 0),
        };
        (d, cancel)
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn update(&mut self, update: Update) {
        match update.kind {
            Kind::Named { name, path, total } => {
                self.name = name;
                self.path = Some(path);
                self.total = total;
            }
            Kind::Progress(received) => {
                let (then, before) = self.last;
                let secs = then.elapsed().as_secs_f64();
                if secs >= 0.5 {
                    let now = (received - before) as f64 / secs;
                    self.rate = if self.rate == 0.0 { now } else { self.rate * 0.6 + now * 0.4 };
                    self.last = (Instant::now(), received);
                }
                self.received = received;
            }
            Kind::Done => {
                self.state = State::Done;
                self.total = Some(self.received);
            }
            Kind::Failed(e) => self.state = State::Failed(e),
            Kind::Stopped => self.state = State::Stopped,
        }
    }

    /// The line under the name.
    pub fn status(&self) -> String {
        use tabula_kit::files::format_size;
        match &self.state {
            State::Running => {
                let got = format_size(self.received);
                let of = self.total.map(|t| format!(" of {}", format_size(t))).unwrap_or_default();
                let rate = if self.rate > 0.0 { format!(" — {}/s", format_size(self.rate as u64)) } else { String::new() };
                format!("{got}{of}{rate}")
            }
            State::Done => format_size(self.received),
            State::Failed(e) => format!("Failed: {e}"),
            State::Stopped => "Stopped".into(),
        }
    }

    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(t) if t > 0 => (self.received as f64 / t as f64) as f32,
            _ => 0.0,
        }
    }
}

/// A download's news, from its thread.
pub struct Update {
    pub id: u64,
    pub kind: Kind,
}

pub enum Kind {
    Named { name: String, path: PathBuf, total: Option<u64> },
    Progress(u64),
    Done,
    Failed(String),
    Stopped,
}

/// The Downloads folder.
pub fn folder() -> PathBuf {
    tabula_xdg::user_dir("DOWNLOAD")
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join("Downloads"))
}

/// Fetch `source` into `dir` on a thread; `suggested` names it unless the
/// server does.
pub fn start(proxy: Proxy, id: u64, source: Source, suggested: Option<String>, dir: PathBuf, cancel: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("download".into())
        .spawn(move || {
            let send = |kind| proxy.send(Update { id, kind });
            let result = match source {
                Source::Http { url, cookies, referer, user_agent } => {
                    fetch(&url, &cookies, referer.as_deref(), &user_agent, suggested, &dir, &cancel, &send)
                }
                Source::File(mut file, total) => {
                    use std::io::Seek as _;
                    let name = clean_name(suggested.as_deref().unwrap_or("download"));
                    match file.rewind() {
                        Ok(()) => save(&mut file, &name, Some(total), &dir, &cancel, &send),
                        Err(e) => Err(e.to_string()),
                    }
                }
            };
            match result {
                Ok(true) => send(Kind::Done),
                Ok(false) => send(Kind::Stopped),
                Err(e) => send(Kind::Failed(e)),
            }
        })
        .expect("thread");
}

#[allow(clippy::too_many_arguments)]
fn fetch(
    url: &str,
    cookies: &str,
    referer: Option<&str>,
    user_agent: &str,
    suggested: Option<String>,
    dir: &Path,
    cancel: &AtomicBool,
    send: &impl Fn(Kind),
) -> Result<bool, String> {
    if url.starts_with("data:") {
        let parsed = data_url::DataUrl::process(url).map_err(|e| format!("{e:?}"))?;
        let (bytes, _) = parsed.decode_to_vec().map_err(|e| format!("{e:?}"))?;
        let name = clean_name(suggested.as_deref().filter(|s| !s.is_empty()).unwrap_or("download"));
        return save(&mut bytes.as_slice(), &name, Some(bytes.len() as u64), dir, cancel, send);
    }
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(20)
        .user_agent(user_agent)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut request = agent.get(url);
    if !cookies.is_empty() {
        request = request.header("Cookie", cookies);
    }
    if let Some(r) = referer {
        request = request.header("Referer", r);
    }
    let response = request.call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("the server answered {code}"),
        e => e.to_string(),
    })?;
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let total = header("content-length").and_then(|l| l.parse().ok());
    let final_url = ureq::ResponseExt::get_uri(&response).to_string();
    let name = header("content-disposition")
        .and_then(|d| disposition_name(&d))
        .or(suggested.filter(|s| !s.is_empty()))
        .or_else(|| url_name(&final_url))
        .unwrap_or_else(|| "download".into());
    let mut body = response.into_body().into_with_config().limit(u64::MAX).reader();
    save(&mut body, &clean_name(&name), total, dir, cancel, send)
}

/// Copy `from` into a new file named `name` in `dir` (via `name.part`);
/// false if stopped.
fn save(
    from: &mut impl Read,
    name: &str,
    total: Option<u64>,
    dir: &Path,
    cancel: &AtomicBool,
    send: &impl Fn(Kind),
) -> Result<bool, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (path, part) = unique(dir, name);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&part).map_err(|e| e.to_string())?;
    let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    send(Kind::Named { name: shown, path: path.clone(), total });
    let mut buf = vec![0u8; 64 * 1024];
    let (mut received, mut told) = (0u64, Instant::now());
    let outcome = loop {
        if cancel.load(Ordering::Relaxed) {
            break Ok(false);
        }
        let n = match from.read(&mut buf) {
            Ok(0) => break Ok(true),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => break Err(e.to_string()),
        };
        if let Err(e) = file.write_all(&buf[..n]) {
            break Err(e.to_string());
        }
        received += n as u64;
        if told.elapsed() >= Duration::from_millis(100) {
            send(Kind::Progress(received));
            told = Instant::now();
        }
    };
    send(Kind::Progress(received));
    match outcome {
        Ok(true) => {
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&part, &path).map_err(|e| e.to_string())?;
            Ok(true)
        }
        other => {
            let _ = fs::remove_file(&part);
            other
        }
    }
}

/// A free name in `dir` (“name (2).ext” …) and its partial file's path.
fn unique(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let (stem, ext) = split_ext(name);
    for n in 1.. {
        let candidate = if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let path = dir.join(&candidate);
        let part = dir.join(format!("{candidate}.part"));
        if !path.exists() && !part.exists() {
            return (path, part);
        }
    }
    unreachable!()
}

/// "archive.tar.gz" → ("archive", ".tar.gz").
fn split_ext(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if lower.ends_with(double) && name.len() > double.len() {
            return name.split_at(name.len() - double.len());
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// A name that is safe as one file name.
pub fn clean_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let mut cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    while cleaned.len() > 200 {
        cleaned.pop();
    }
    if cleaned.is_empty() { "download".into() } else { cleaned }
}

/// The file name in a Content-Disposition header (RFC 6266), preferring
/// the UTF-8 `filename*`.
pub fn disposition_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        let Some((key, value)) = part.split_once('=') else { continue };
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let value = value.trim().trim_matches('"');
                let encoded = value.splitn(3, '\'').nth(2).unwrap_or(value);
                let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy().into_owned();
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
            "filename" => {
                let v = value.trim();
                let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v).replace("\\\"", "\"");
                if !v.is_empty() {
                    plain = Some(v);
                }
            }
            _ => {}
        }
    }
    plain
}

/// The last part of a URL's path, decoded.
pub fn url_name(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.rev().find(|s| !s.is_empty())?;
    let name = percent_encoding::percent_decode_str(last).decode_utf8_lossy().into_owned();
    (!name.is_empty()).then_some(name)
}

pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-download-{token}:")
}

/// The page side: Servo's unknown-content page, `download` links (also
/// clicked from script while detached), and reading `blob:` contents.
pub fn script(token: &str) -> String {
    let marker = serde_json::to_string(&marker(token)).unwrap_or_default();
    format!(
        r#"(function() {{
  var log = console.log.bind(console), K = {marker}, next = 1;
  function report(o) {{ log(K + JSON.stringify(o)); }}
  if (window.top === window) document.addEventListener('DOMContentLoaded', function() {{
    var b = document.body;
    if (!b || b.children.length !== 1 || b.firstElementChild.tagName !== 'P') return;
    if (/^Unknown content type \(.+\)\.$/.test(b.textContent.trim())) report({{op: 'unknown', url: location.href}});
  }});
  function bytes(name, blob) {{
    var id = next++, CH = 393216;
    blob.arrayBuffer().then(function(buf) {{
      var u = new Uint8Array(buf), n = Math.max(1, Math.ceil(u.length / CH));
      for (var i = 0; i < n; i++) {{
        var part = u.subarray(i * CH, (i + 1) * CH), s = '';
        for (var j = 0; j < part.length; j += 8192) s += String.fromCharCode.apply(null, part.subarray(j, j + 8192));
        report({{op: 'data', id: id, name: name, seq: i, last: i === n - 1, data: btoa(s)}});
      }}
    }});
  }}
  function take(a) {{
    var name = a.getAttribute('download') || '';
    if (a.protocol === 'blob:') {{
      fetch(a.href).then(function(r) {{ return r.blob(); }}).then(function(b) {{ bytes(name, b); }});
    }} else {{
      report({{op: 'link', url: a.href, name: name}});
    }}
  }}
  window.addEventListener('click', function(e) {{
    if (e.defaultPrevented || e.button !== 0) return;
    var a = e.target && e.target.closest && e.target.closest('a[download]');
    if (!a || !a.href) return;
    e.preventDefault();
    take(a);
  }});
  // Servo doesn't reflect `download`, so `a.download = 'x'` would set no attribute.
  if (!('download' in HTMLAnchorElement.prototype)) Object.defineProperty(HTMLAnchorElement.prototype, 'download', {{
    configurable: true, enumerable: true,
    get: function() {{ return this.getAttribute('download') || ''; }},
    set: function(v) {{ this.setAttribute('download', v); }}
  }});
  var click = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function() {{
    if (!this.isConnected && this.hasAttribute('download') && this.href) return take(this);
    return click.call(this);
  }};
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(disposition_name(r#"attachment; filename="report.pdf""#).unwrap(), "report.pdf");
        assert_eq!(disposition_name("attachment; filename=a.zip").unwrap(), "a.zip");
        assert_eq!(
            disposition_name(r#"attachment; filename="x.txt"; filename*=UTF-8''%E2%82%AC%20rates.txt"#).unwrap(),
            "€ rates.txt"
        );
        assert!(disposition_name("inline").is_none());
        assert_eq!(url_name("https://e.com/files/My%20File.tar.gz?x=1").unwrap(), "My File.tar.gz");
        assert!(url_name("https://e.com/").is_none());
        assert_eq!(clean_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(clean_name("  "), "download");
        assert_eq!(split_ext("a.tar.gz"), ("a", ".tar.gz"));
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext("README"), ("README", ""));
    }

    #[test]
    fn saving() {
        let dir = std::env::temp_dir().join(format!("np-dl-{}", std::process::id()));
        let cancel = AtomicBool::new(false);
        let named = std::sync::Mutex::new(Vec::new());
        let send = |k: Kind| {
            if let Kind::Named { name, .. } = k {
                named.lock().unwrap().push(name);
            }
        };
        for _ in 0..2 {
            assert!(save(&mut &b"hello"[..], "a.txt", Some(5), &dir, &cancel, &send).unwrap());
        }
        assert_eq!(*named.lock().unwrap(), ["a.txt", "a (2).txt"]);
        assert_eq!(fs::read(dir.join("a (2).txt")).unwrap(), b"hello");
        cancel.store(true, Ordering::Relaxed);
        assert!(!save(&mut &b"x"[..], "b.txt", None, &dir, &cancel, &send).unwrap());
        assert!(!dir.join("b.txt").exists() && !dir.join("b.txt.part").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
