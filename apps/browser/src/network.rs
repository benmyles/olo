//! Worker-owned resource loading. No network runtime or polling on the UI.
use olo_web::{css, page, text};
use reqwest::{Client, header};
use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;
const HTML_LIMIT: usize = 16 * 1024 * 1024;
#[derive(Clone, Copy)]
enum Kind {
    Document,
    Stylesheet,
    Image,
    Font,
    Script,
    /// A script's own request (`fetch`, `XMLHttpRequest`).
    Fetch,
}
const SCRIPT_LIMIT: usize = 16 * 1024 * 1024;
const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
const FONT_LIMIT: usize = 32 * 1024 * 1024;
const CSS_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    signal: CancellationToken,
}
impl Cancel {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.signal.cancel();
    }
}
impl Deref for Cancel {
    type Target = AtomicBool;
    fn deref(&self) -> &AtomicBool {
        &self.flag
    }
}
pub struct Loader {
    client: Client,
    runtime: tokio::runtime::Runtime,
    /// Fetched subresources by URL, for the page's second pass (its
    /// scripts), the next page sharing them, and a visit back: bytes as
    /// received, with the media type, status and the URL they came from.
    cache: std::sync::Mutex<SessionCache>,
}
#[derive(Default)]
struct SessionCache {
    entries: std::collections::HashMap<String, Arc<CachedResponse>>,
    bytes: usize,
}
struct CachedResponse {
    url: Url,
    bytes: Vec<u8>,
    mime: Option<mime::Mime>,
    status: u16,
}
/// What the session cache holds before starting over.
const SESSION_CACHE_BYTES: usize = 256 << 20;
pub struct Resource {
    pub url: Url,
    pub bytes: Vec<u8>,
    pub mime: Option<mime::Mime>,
    pub status: Option<u16>,
    document_encoding: Option<&'static encoding_rs::Encoding>,
}
impl Loader {
    pub fn new() -> page::Result<Self> {
        Ok(Self {
            client: Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(60)).user_agent("OloBrowser/0.1").build()?,
            runtime: tokio::runtime::Builder::new_current_thread().enable_all().build()?,
            cache: Default::default(),
        })
    }
    fn navigation(source: &str, cwd: Option<&std::path::Path>) -> page::Result<Url> {
        let source = source.trim();
        if source.contains("://") || source.starts_with("file:") {
            return Ok(Url::parse(source)?);
        }
        let path = cwd.map_or_else(|| std::path::PathBuf::from(source), |cwd| cwd.join(source));
        if source.starts_with('/') || source.starts_with('.') || path.is_file() || !source.contains(':') && !source.contains('.') && source != "localhost" {
            return Url::from_file_path(std::path::absolute(path)?).map_err(|_| "Invalid file path".into());
        }
        Ok(Url::parse(&format!("https://{source}"))?)
    }
    pub fn document(&self, source: &str, cwd: Option<&std::path::Path>, cancel: &Cancel, progress: impl FnMut(usize)) -> page::Result<Resource> {
        self.fetch(Self::navigation(source, cwd)?, None, HTML_LIMIT, cancel, progress, Kind::Document)
    }
    pub fn stylesheet(&self, document: &Resource, base: Option<&str>, href: &str, cancel: &Cancel, progress: impl FnMut(usize)) -> page::Result<css::Syntax> {
        let base = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone());
        let resource = self.fetch(base.join(href)?, Some(&document.url), CSS_LIMIT, cancel, progress, Kind::Stylesheet)?;
        if resource.status.is_some_and(|s| !(200..300).contains(&s)) {
            return Err("Stylesheet returned an unsuccessful HTTP status".into());
        }
        if resource.mime.as_ref().is_none_or(|m| m.essence_str() != "text/css") {
            return Err("Stylesheet MIME type must be text/css".into());
        }
        let encoding = resource.mime.as_ref().and_then(|m| m.get_param("charset")).map(|p| p.as_str());
        Ok(css::Syntax::parse_bytes(&resource.bytes, css::ByteOptions { protocol_encoding: encoding, environment_encoding: Some(document.encoding()) }, Default::default(), Some(cancel))?.syntax)
    }
    /// A classic script's source text, decoded by its charset, else the
    /// document's encoding, as browsers do.
    pub fn script(&self, document: &Resource, url: Url, cancel: &Cancel, progress: impl FnMut(usize)) -> page::Result<String> {
        let resource = self.fetch(url, Some(&document.url), SCRIPT_LIMIT, cancel, progress, Kind::Script)?;
        if resource.status.is_some_and(|s| !(200..300).contains(&s)) {
            return Err("Script returned an unsuccessful HTTP status".into());
        }
        let charset = resource.mime.as_ref().and_then(|m| m.get_param("charset")).and_then(|p| encoding_rs::Encoding::for_label(p.as_str().as_bytes()));
        let encoding = encoding_rs::Encoding::for_bom(&resource.bytes).map(|b| b.0).or(charset).unwrap_or_else(|| document.encoding());
        let (text, _, _) = encoding.decode(&resource.bytes);
        Ok(text.into_owned())
    }
    /// A script's own GET: status, media type and the body as text.
    pub fn text(&self, initiator: &Url, url: Url, cancel: &Cancel) -> page::Result<(u16, String, String, String)> {
        let resource = self.fetch(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Fetch)?;
        let charset = resource.mime.as_ref().and_then(|m| m.get_param("charset")).and_then(|p| encoding_rs::Encoding::for_label(p.as_str().as_bytes()));
        let encoding = encoding_rs::Encoding::for_bom(&resource.bytes).map(|b| b.0).or(charset).unwrap_or(encoding_rs::UTF_8);
        let (text, _, _) = encoding.decode(&resource.bytes);
        Ok((resource.status.unwrap_or(200), resource.mime.as_ref().map(ToString::to_string).unwrap_or_default(), text.into_owned(), resource.url.to_string()))
    }
    /// An image: a raster format decoded here, or an SVG handed to `svg`
    /// (the engine rasterises it).
    pub fn image(
        &self,
        document: &Resource,
        url: Url,
        remaining: usize,
        cancel: &Cancel,
        mut progress: impl FnMut(usize),
        svg: &dyn Fn(&[u8]) -> page::Result<Arc<olo_web::image::Image>>,
    ) -> page::Result<Arc<olo_web::image::Image>> {
        let bytes = if url.scheme() == "data" {
            let data = data_url::DataUrl::process(url.as_str()).map_err(|_| "Malformed image data URL")?;
            let mut bytes = Vec::new();
            let mut reported = 0usize;
            data.decode(|chunk| {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled");
                }
                if chunk.len() > IMAGE_LIMIT.saturating_sub(bytes.len()) {
                    return Err("Image exceeds its encoded limit");
                }
                bytes.extend_from_slice(chunk);
                if bytes.len() - reported >= 32 << 10 {
                    reported = bytes.len();
                    progress(reported);
                }
                Ok(())
            })
            .map_err(|_| "Image data URL rejected or cancelled")?;
            progress(bytes.len());
            bytes
        } else {
            let resource = self.fetch(url, Some(&document.url), IMAGE_LIMIT, cancel, &mut progress, Kind::Image)?;
            if resource.status.is_some_and(|s| !(200..300).contains(&s)) {
                return Err("Image returned an unsuccessful HTTP status".into());
            }
            resource.bytes
        };
        if looks_like_svg(&bytes) {
            return svg(&bytes);
        }
        let mut limits = olo_images::raster::Limits::default();
        limits.surface_bytes = limits.surface_bytes.min(remaining as u64);
        limits.pixels = limits.pixels.min((remaining / 4) as u64);
        let raster = olo_images::raster::decode(&bytes, limits, Some(cancel))?;
        let (w, h) = raster.pixels.dimensions();
        Ok(olo_web::image::Image::from_rgba(w, h, raster.pixels.into_raw(), Default::default(), Some(cancel))?)
    }
}
/// An SVG document by its first bytes: XML or an `svg` root, possibly after
/// a byte-order mark, comments or a doctype.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<?xml") || text.starts_with("<svg") || text.starts_with("<!DOCTYPE svg") || (text.starts_with("<!--") && text.contains("<svg"))
}
impl Loader {
    /// A web font's bytes (any container; the engine decodes WOFF itself).
    pub fn font(&self, document: &Resource, url: Url, cancel: &Cancel, mut progress: impl FnMut(usize)) -> page::Result<Arc<[u8]>> {
        if url.scheme() == "data" {
            let data = data_url::DataUrl::process(url.as_str()).map_err(|_| "Malformed font data URL")?;
            let mut bytes = Vec::new();
            data.decode(|chunk| {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled");
                }
                if chunk.len() > FONT_LIMIT.saturating_sub(bytes.len()) {
                    return Err("Font exceeds its encoded limit");
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .map_err(|_| "Font data URL rejected or cancelled")?;
            progress(bytes.len());
            return Ok(Arc::from(bytes));
        }
        let resource = self.fetch(url, Some(&document.url), FONT_LIMIT, cancel, &mut progress, Kind::Font)?;
        if resource.status.is_some_and(|s| !(200..300).contains(&s)) {
            return Err("Font returned an unsuccessful HTTP status".into());
        }
        Ok(Arc::from(resource.bytes))
    }
    fn fetch(&self, url: Url, initiator: Option<&Url>, limit: usize, cancel: &Cancel, mut progress: impl FnMut(usize), kind: Kind) -> page::Result<Resource> {
        let allow_file = url.scheme() == "file";
        let cacheable = !matches!(kind, Kind::Document | Kind::Fetch) && url.scheme() != "file";
        if cacheable && let Some(c) = self.cache.lock().unwrap().entries.get(url.as_str()).cloned() {
            check_url(&url, initiator)?;
            return Ok(Resource { url: c.url.clone(), bytes: c.bytes.clone(), mime: c.mime.clone(), status: Some(c.status), document_encoding: None });
        }
        let requested = url.clone();
        let fetched = self.fetch_uncached(url, initiator, limit, cancel, &mut progress, kind, allow_file)?;
        if cacheable && fetched.status.is_some_and(|s| (200..300).contains(&s)) {
            let mut cache = self.cache.lock().unwrap();
            if cache.bytes + fetched.bytes.len() > SESSION_CACHE_BYTES {
                cache.entries.clear();
                cache.bytes = 0;
            }
            cache.bytes += fetched.bytes.len();
            cache
                .entries
                .insert(requested.to_string(), Arc::new(CachedResponse { url: fetched.url.clone(), bytes: fetched.bytes.clone(), mime: fetched.mime.clone(), status: fetched.status.unwrap_or(200) }));
        }
        Ok(fetched)
    }
    #[allow(clippy::too_many_arguments)]
    fn fetch_uncached(&self, mut url: Url, initiator: Option<&Url>, limit: usize, cancel: &Cancel, progress: &mut impl FnMut(usize), kind: Kind, allow_file: bool) -> page::Result<Resource> {
        self.runtime.block_on(async {
            let work = async {
                for _ in 0..=20 {
                    check_url(&url, initiator)?;
                    if url.scheme() == "file" && !allow_file {
                        return Err("Web responses cannot redirect to local files".into());
                    }
                    if url.scheme() == "file" {
                        let path = url.to_file_path().map_err(|_| "Invalid local file URL")?;
                        // Small bounded reads, with cancellation between them. This is
                        // already the dedicated worker, never the UI thread.
                        use std::io::Read;
                        let mut file = std::fs::File::open(path)?;
                        let mut bytes = Vec::new();
                        let mut chunk = [0u8; 65536];
                        loop {
                            if cancel.load(Ordering::Relaxed) {
                                return Err("Cancelled".into());
                            }
                            let n = file.read(&mut chunk)?;
                            if n == 0 {
                                break;
                            }
                            append(&mut bytes, &chunk[..n], limit)?;
                            progress(bytes.len());
                        }
                        let mime = Some(
                            match kind {
                                Kind::Document => "text/html",
                                Kind::Stylesheet => "text/css",
                                Kind::Image | Kind::Font => "application/octet-stream",
                                Kind::Script => "text/javascript",
                                Kind::Fetch => "text/plain",
                            }
                            .parse()?,
                        );
                        return Ok(Resource { url, bytes, mime, status: None, document_encoding: None });
                    }
                    let mut response = self
                        .client
                        .get(url.clone())
                        .header(
                            header::ACCEPT,
                            match kind {
                                Kind::Document => "text/html,text/plain;q=0.9,*/*;q=0.1",
                                Kind::Stylesheet => "text/css,*/*;q=0.1",
                                Kind::Image => "image/png,image/jpeg,image/webp,image/gif,image/svg+xml,*/*;q=0.1",
                                Kind::Font => "font/woff2,font/woff,font/ttf,font/otf,*/*;q=0.1",
                                Kind::Script => "*/*",
                                Kind::Fetch => "*/*",
                            },
                        )
                        .send()
                        .await?;
                    let status = response.status();
                    if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
                        && let Some(location) = response.headers().get(header::LOCATION)
                    {
                        let location = location.to_str()?;
                        let mut target = url.join(location)?;
                        if !location.contains('#') {
                            target.set_fragment(url.fragment());
                        }
                        url = target;
                        continue;
                    }
                    let mime = response.headers().get(header::CONTENT_TYPE).map(|h| h.to_str()?.parse::<mime::Mime>().map_err(page::Error::from)).transpose()?;
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        append(&mut bytes, &chunk, limit)?;
                        progress(bytes.len());
                    }
                    return Ok(Resource { url, bytes, mime, status: Some(status.as_u16()), document_encoding: None });
                }
                Err("Too many HTTP redirects".into())
            };
            tokio::select! { biased; _ = cancel.signal.cancelled() => Err("Cancelled".into()), result = work => result }
        })
    }
}
fn append(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) -> page::Result<()> {
    if chunk.len() > limit.saturating_sub(bytes.len()) {
        return Err("Decoded resource exceeds its size limit".into());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}
fn check_url(url: &Url, initiator: Option<&Url>) -> page::Result<()> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Embedded URL credentials are not supported".into());
    }
    if !matches!(url.scheme(), "http" | "https" | "file") {
        return Err("Unsupported resource URL scheme".into());
    }
    if let Some(parent) = initiator {
        if url.scheme() == "file" && parent.scheme() != "file" {
            return Err("Web pages cannot load local files".into());
        }
        if parent.scheme() == "https" && url.scheme() == "http" {
            return Err("Blocked insecure stylesheet on an HTTPS page".into());
        }
    }
    Ok(())
}
impl Resource {
    fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.document_encoding.unwrap_or_else(|| {
            let protocol = self.mime.as_ref().and_then(|m| m.get_param("charset")).map(|p| p.as_str());
            if self.mime.as_ref().is_some_and(|m| m.essence_str() == "text/plain") {
                encoding_rs::Encoding::for_bom(&self.bytes).map(|p| p.0).or_else(|| protocol.and_then(|p| encoding_rs::Encoding::for_label(p.as_bytes()))).unwrap_or(encoding_rs::WINDOWS_1252)
            } else {
                olo_web::html::bytes::encoding(&self.bytes, protocol).0
            }
        })
    }
    /// Decode once, retaining only URL/transport/encoding metadata for sheets.
    /// Release the encoded document buffer before parsing/layout allocations.
    pub fn take_html(&mut self, cancel: &Cancel) -> page::Result<Arc<str>> {
        let kind = self.mime.as_ref().map(|m| m.essence_str());
        if !matches!(kind, None | Some("text/html" | "text/plain")) {
            return Err("This document MIME type is not yet supported".into());
        }
        let skip = olo_web::html::bytes::encoding(&self.bytes, None).1;
        self.document_encoding = Some(self.encoding());
        let plain = kind == Some("text/plain");
        let mut output = if plain { "<!doctype html><meta charset=utf-8><pre>".to_owned() } else { String::new() };
        let mut decoder = text::Decoder::new(self.encoding(), text::DecodeLimits { input_bytes: HTML_LIMIT, output_bytes: HTML_LIMIT }, Some(cancel));
        decoder.push(&self.bytes[skip..], true, |s| {
            if !plain {
                return text::collect(&mut output, s, HTML_LIMIT);
            }
            for piece in s.split_inclusive(['&', '<']) {
                let (literal, escape) = if let Some(prefix) = piece.strip_suffix('&') {
                    (prefix, "&amp;")
                } else if let Some(prefix) = piece.strip_suffix('<') {
                    (prefix, "&lt;")
                } else {
                    (piece, "")
                };
                text::collect(&mut output, literal, HTML_LIMIT)?;
                text::collect(&mut output, escape, HTML_LIMIT)?;
            }
            Ok(())
        })?;
        self.bytes = Vec::new();
        Ok(Arc::from(output))
    }
}
/// Throttle status traffic while bytes stream, without a timer when idle.
pub fn progress(mut send: impl FnMut(String), name: &str) -> impl FnMut(usize) {
    let mut last = Instant::now();
    move |bytes| {
        if last.elapsed() >= Duration::from_millis(120) {
            send(format!("Loading {name}… {bytes} bytes"));
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };
    fn server(routes: &'static [(&'static str, &'static str)], count: usize) -> (String, thread::JoinHandle<()>) {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", socket.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for _ in 0..count {
                let (mut stream, _) = socket.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                loop {
                    let mut b = [0u8];
                    if stream.read(&mut b).unwrap() == 0 {
                        break;
                    }
                    request.push(b[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(request.len() < 16384);
                }
                let path = std::str::from_utf8(&request).unwrap().split_ascii_whitespace().nth(1).unwrap();
                let response = routes.iter().find(|r| r.0 == path).unwrap_or_else(|| panic!("Unexpected request {path}")).1;
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (address, handle)
    }
    #[test]
    fn redirect_final_url_charset_css_and_plain_text() {
        let (base, server) = server(
            &[
                ("/start", "HTTP/1.1 302 Found\r\nLocation: /dir/page\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                ("/dir/page", "HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<title>Résumé</title><link rel=stylesheet href=theme.css>"),
                ("/assets/theme.css", "HTTP/1.1 200 OK\r\nContent-Type: text/css; charset=utf-8\r\nConnection: close\r\n\r\nbody{color:red}"),
                ("/plain", "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n<script>&literal"),
            ],
            4,
        );
        let loader = Loader::new().unwrap();
        let cancel = Cancel::default();
        let mut doc = loader.document(&format!("{base}/start#anchor"), None, &cancel, |_| {}).unwrap();
        assert_eq!(doc.url.path(), "/dir/page");
        assert_eq!(doc.url.fragment(), Some("anchor"));
        assert_eq!(doc.status, Some(404));
        assert!(doc.take_html(&cancel).unwrap().contains("Résumé"));
        let css = loader.stylesheet(&doc, Some("/assets/"), "theme.css", &cancel, |_| {}).unwrap();
        assert_eq!(css.source(), "body{color:red}");
        let plain = loader.document(&format!("{base}/plain"), None, &cancel, |_| {}).unwrap().take_html(&cancel).unwrap();
        assert!(plain.contains("&lt;script>&amp;literal"));
        server.join().unwrap();
    }
    #[test]
    fn cancellation_wakes_a_pending_response() {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", socket.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (_stream, _) = socket.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        let cancel = Cancel::default();
        let copy = cancel.clone();
        let worker = thread::spawn(move || Loader::new().unwrap().document(&address, None, &copy, |_| {}));
        accepted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        cancel.cancel();
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        release_tx.send(()).unwrap();
        server.join().unwrap();
    }
    #[test]
    fn schemes_mixed_content_credentials_and_decoded_limits() {
        let https = Url::parse("https://example.test/").unwrap();
        for target in ["file:///etc/passwd", "http://example.test/a.css", "data:text/css,a{}", "https://user:secret@example.test/"] {
            assert!(check_url(&Url::parse(target).unwrap(), Some(&https)).is_err());
        }
        assert!(check_url(&Url::parse("https://cdn.example.test/a.css").unwrap(), Some(&https)).is_ok());
        let mut bytes = vec![1; 3];
        assert!(append(&mut bytes, &[1; 2], 4).is_err());
        assert_eq!(bytes.len(), 3);
        assert_eq!(Loader::navigation("example.com/path", None).unwrap().as_str(), "https://example.com/path");
        assert_eq!(Loader::navigation("127.0.0.1:8080/x", None).unwrap().as_str(), "https://127.0.0.1:8080/x");
    }
    #[test]
    fn compressed_bodies_are_decoded_and_the_decoded_size_is_bounded() {
        let gzip: &[u8] = &[
            31, 139, 8, 0, 0, 0, 0, 0, 0, 255, 179, 41, 201, 44, 201, 73, 181, 115, 206, 207, 45, 40, 74, 45, 46, 78, 77, 177, 209, 135, 136, 84, 140, 130, 81, 48, 10, 70, 44, 0, 0, 175, 97, 62, 69,
            25, 4, 0, 0,
        ];
        for limit in [2048, 64] {
            let socket = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = Url::parse(&format!("http://{}/", socket.local_addr().unwrap())).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = socket.accept().unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", gzip.len()).unwrap();
                stream.write_all(gzip).unwrap();
            });
            let result = Loader::new().unwrap().fetch(url, None, limit, &Cancel::default(), |_| {}, Kind::Document);
            if limit == 2048 {
                assert!(result.unwrap().bytes.starts_with(b"<title>Compressed</title>"));
            } else {
                assert!(result.err().unwrap().to_string().contains("size limit"));
            }
            server.join().unwrap();
        }
    }
    #[test]
    fn document_redirect_cannot_enter_the_local_filesystem() {
        let (base, server) = server(&[("/file", "HTTP/1.1 302 Found\r\nLocation: file:///dev/null\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")], 1);
        let result = Loader::new().unwrap().document(&format!("{base}/file"), None, &Cancel::default(), |_| {});
        assert!(result.err().unwrap().to_string().contains("local files"));
        server.join().unwrap();
    }
    #[test]
    fn wrong_css_mime_is_rejected() {
        let (base, server) = server(&[("/theme", "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\nbody{color:red}")], 1);
        let loader = Loader::new().unwrap();
        let doc = Resource { url: Url::parse(&base).unwrap(), bytes: vec![], mime: None, status: None, document_encoding: None };
        assert!(loader.stylesheet(&doc, None, "/theme", &Cancel::default(), |_| {}).is_err());
        server.join().unwrap();
    }
    #[test]
    fn image_data_and_http_use_bounded_worker_decode() {
        let data = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
        let loader = Loader::new().unwrap();
        let document = Resource { url: Url::parse("https://example.test/page").unwrap(), bytes: Vec::new(), mime: None, status: None, document_encoding: None };
        let image = loader.image(&document, Url::parse(data).unwrap(), 4, &Cancel::default(), |_| {}).unwrap();
        assert_eq!(image.rgba(), [255, 0, 0, 255]);
        assert!(loader.image(&document, Url::parse(data).unwrap(), 3, &Cancel::default(), |_| {}).is_err());
        let cancelled = Cancel::default();
        cancelled.cancel();
        assert!(loader.image(&document, Url::parse(data).unwrap(), 4, &cancelled, |_| {}).is_err());
        let png = data_url::DataUrl::process(data).unwrap().decode_to_vec().unwrap().0;
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/red.png", socket.local_addr().unwrap())).unwrap();
        let document = Resource { url: url.clone(), bytes: Vec::new(), mime: None, status: None, document_encoding: None };
        let server = thread::spawn(move || {
            let (mut stream, _) = socket.accept().unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            assert!(std::str::from_utf8(&request[..n]).unwrap().contains("image/png"));
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", png.len()).unwrap();
            stream.write_all(&png).unwrap();
        });
        let image = loader.image(&document, url, 4, &Cancel::default(), |_| {}).unwrap();
        assert_eq!(image.rgba(), [255, 0, 0, 255]);
        server.join().unwrap();
    }
}
