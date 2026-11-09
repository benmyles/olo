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
    /// The network's: two threads that only wait on sockets (what is
    /// received is decoded, parsed and laid out on threads of its own). A
    /// fetch started ahead goes on while the page's thread does its work.
    runtime: tokio::runtime::Runtime,
    /// Fetched subresources by URL, for the page's second pass (its
    /// scripts), the next page sharing them, and a visit back: bytes as
    /// received, with the media type, status and the URL they came from.
    cache: Arc<std::sync::Mutex<SessionCache>>,
    /// Fetches started ahead (the preload scanner's), by URL: a load that
    /// wants one joins it rather than fetching it again.
    inflight: Arc<std::sync::Mutex<std::collections::HashMap<String, Inflight>>>,
    /// How many fetches started ahead go at once.
    permits: Arc<tokio::sync::Semaphore>,
    /// Pictures and style sheets fetched ahead, decoded or parsed on the
    /// CPU pool as they arrive (side by side, off the page's thread), by URL.
    decoding: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<Decoding>>>>,
    /// What stops the fetches started ahead: the document's, not one render
    /// request's (the page asks to render again while they are under way;
    /// they stop when another document loads).
    ahead: std::sync::Mutex<(Cancel, Option<Url>)>,
    /// Who to tell when a fetch started ahead lands, by URL (a live page
    /// rendering without the sheet it waits for).
    waiters: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<Box<dyn FnOnce() + Send>>>>>,
    /// Style sheets parsed, by URL and the document encoding they were
    /// parsed in, while a page holds them: the page's next refresh takes
    /// them as they are (no copy, no parse).
    sheets: std::sync::Mutex<std::collections::HashMap<(String, &'static encoding_rs::Encoding), std::sync::Weak<css::Syntax>>>,
    /// Web fonts unpacked, by URL, while a page holds them.
    fonts: std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<[u8]>>>,
}
/// A picture being decoded ahead: its result once there.
#[derive(Default)]
struct Decoding {
    result: std::sync::Mutex<Option<Decoded>>,
    ready: std::sync::Condvar,
}
enum Decoded {
    Image(Arc<olo_web::image::Image>),
    /// A style sheet, parsed in its document's encoding.
    Sheet(Arc<css::Syntax>, &'static encoding_rs::Encoding),
    /// A web font, its WOFF container unpacked.
    Font(Arc<[u8]>),
    /// Not decoded ahead (an SVG, a failure, a cancellation): the load
    /// does it itself, as before.
    Itself,
}
impl Decoding {
    fn set(&self, decoded: Decoded) {
        *self.result.lock().unwrap() = Some(decoded);
        self.ready.notify_all();
    }
}
/// A raster picture's decoding (on the CPU pool's threads): within the
/// default limits, the page's budget applied when it takes it.
fn decode_raster(bytes: &[u8], cancel: Option<&AtomicBool>) -> page::Result<Arc<olo_web::image::Image>> {
    let raster = olo_images::raster::decode(bytes, olo_images::raster::Limits::default(), cancel)?;
    let (w, h) = raster.pixels.dimensions();
    Ok(olo_web::image::Image::from_rgba(w, h, raster.pixels.into_raw(), Default::default(), cancel)?)
}
/// A web font out of its WOFF container (as it was when it isn't one, or
/// can't be unpacked: the engine says so).
fn unpack_font(bytes: Arc<[u8]>, cancel: Option<&AtomicBool>) -> Arc<[u8]> {
    if olo_web::font::woff::is_woff(&bytes)
        && let Ok(sfnt) = olo_web::font::woff::decode(&bytes, cancel)
    {
        return Arc::from(sfnt);
    }
    bytes
}
/// A fetched style sheet parsed, in its document's encoding unless it
/// says otherwise.
fn parse_sheet(status: Option<u16>, mime: Option<&mime::Mime>, bytes: &[u8], environment: &'static encoding_rs::Encoding, cancel: Option<&AtomicBool>) -> page::Result<css::Syntax> {
    if status.is_some_and(|s| !(200..300).contains(&s)) {
        return Err("Stylesheet returned an unsuccessful HTTP status".into());
    }
    if mime.is_none_or(|m| m.essence_str() != "text/css") {
        return Err("Stylesheet MIME type must be text/css".into());
    }
    let encoding = mime.and_then(|m| m.get_param("charset")).map(|p| p.as_str());
    Ok(css::Syntax::parse_bytes(bytes, css::ByteOptions { protocol_encoding: encoding, environment_encoding: Some(environment) }, Default::default(), cancel)?.syntax)
}
/// A fetch under way or done: its response (any status), or None when it
/// failed or was cancelled.
type Inflight = Arc<tokio::sync::OnceCell<Option<Arc<CachedResponse>>>>;
/// Fetches started ahead going at once (the server's HTTP/2 connection
/// takes them together).
const PREFETCHES: usize = 24;
/// One fetch, kept whatever its status (an image served with a 404 still
/// shows).
async fn fetch_once(client: Client, url: Url, initiator: Url, limit: usize, flag: Arc<AtomicBool>, kind: Kind) -> Option<Arc<CachedResponse>> {
    let started = Instant::now();
    if olo_web::env_on!("OLO_FETCH_DEBUG") {
        eprintln!("  ahead {url} started on {:?}", std::thread::current().name());
    }
    let fetched = get(&client, url.clone(), Some(initiator), limit, flag, &mut |_| {}, kind, false, None).await;
    if olo_web::env_on!("OLO_FETCH_DEBUG") {
        eprintln!("  ahead {url} done in {:.0} ms ({})", started.elapsed().as_secs_f64() * 1000., if fetched.is_ok() { "ok" } else { "failed" });
    }
    let fetched = fetched.ok()?;
    Some(Arc::new(CachedResponse { url: fetched.url, bytes: fetched.bytes, mime: fetched.mime, status: fetched.status.unwrap_or(200) }))
}
/// A successful response into the session cache.
fn remember(cache: &std::sync::Mutex<SessionCache>, requested: &str, response: &Arc<CachedResponse>) {
    if !(200..300).contains(&response.status) {
        return;
    }
    let mut cache = cache.lock().unwrap();
    if cache.bytes + response.bytes.len() > SESSION_CACHE_BYTES {
        cache.entries.clear();
        cache.bytes = 0;
    }
    cache.bytes += response.bytes.len();
    cache.entries.insert(requested.to_owned(), response.clone());
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
#[derive(Clone)]
pub struct Resource {
    pub url: Url,
    pub bytes: Vec<u8>,
    pub mime: Option<mime::Mime>,
    pub status: Option<u16>,
    /// The response's headers as `name: value` lines (names lower case,
    /// `Set-Cookie` left out, as Fetch never shows it to scripts), when it
    /// came from the network for a script (else none: its media type says
    /// what there is).
    pub headers: String,
    document_encoding: Option<&'static encoding_rs::Encoding>,
}
impl Loader {
    pub fn new() -> page::Result<Self> {
        Ok(Self {
            // Responses' headers up to 256 KB as Chrome takes them (hyper's
            // 16 KB default refused YouTube's: an HTTP/2 stream error).
            client: Client::builder()
                .http2_max_header_list_size(256 * 1024)
                // HTTP/2 as Chrome opens it (its SETTINGS and first
                // WINDOW_UPDATE are part of what bot checks fingerprint):
                // 6 MiB a stream, 15 MiB the connection, no max frame size.
                .http2_initial_stream_window_size(6 * 1024 * 1024)
                .http2_initial_connection_window_size(15 * 1024 * 1024)
                .http2_max_frame_size(None)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(60))
                .build()?,
            runtime: tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("browser-net").enable_all().build()?,
            cache: Default::default(),
            inflight: Default::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(PREFETCHES)),
            decoding: Default::default(),
            ahead: Default::default(),
            waiters: Default::default(),
            sheets: Default::default(),
            fonts: Default::default(),
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
        let url = Self::navigation(source, cwd)?;
        self.forget_ahead(&url);
        self.fetch(url, None, HTML_LIMIT, cancel, progress, Kind::Document)
    }
    /// A document a form posts to: its body (of `kind`), what the server
    /// answers (a redirect after it goes on as a GET).
    pub fn document_post(&self, source: &str, from: &str, kind: &str, body: &[u8], cancel: &Cancel) -> page::Result<Resource> {
        let url = Url::parse(source)?;
        // (A post's answer is another document, even at the same URL.)
        self.ahead.lock().unwrap().1 = None;
        self.forget_ahead(&url);
        // From the page that posted it (its origin and referrer go with it,
        // as Chrome sends them; servers check them against forgery).
        let initiator = Url::parse(from).ok().filter(|u| matches!(u.scheme(), "http" | "https"));
        let outgoing = Outgoing::new("POST", &format!("content-type: {kind}"), body)?;
        let mut progress = |_| {};
        self.runtime.block_on(async {
            let work = get(&self.client, url, initiator, HTML_LIMIT, cancel.flag.clone(), &mut progress, Kind::Document, false, Some(&outgoing));
            tokio::select! { biased; _ = cancel.signal.cancelled() => Err("Cancelled".into()), result = work => result }
        })
    }
    /// An iframe's document, for the page at `parent` (its site, for
    /// cookies, and its referrer).
    pub fn frame_document(&self, url: Url, parent: &Url, cancel: &Cancel) -> page::Result<Resource> {
        self.fetch(url, Some(parent), HTML_LIMIT, cancel, |_| {}, Kind::Document)
    }
    /// A style sheet, parsed: the one a page already holds, else the one
    /// parsed ahead as it arrived (waited for), else fetched and parsed.
    pub fn stylesheet(&self, document: &Resource, base: Option<&str>, href: &str, cancel: &Cancel, progress: impl FnMut(usize)) -> page::Result<Arc<css::Syntax>> {
        let base = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone());
        let url = base.join(href)?;
        let key = (url.to_string(), document.encoding());
        if let Some(sheet) = self.sheets.lock().unwrap().get(&key).and_then(std::sync::Weak::upgrade) {
            return Ok(sheet);
        }
        if let Some(Decoded::Sheet(sheet, encoding)) = self.decoded(&url, cancel)
            && encoding == key.1
        {
            self.sheets.lock().unwrap().insert(key, Arc::downgrade(&sheet));
            return Ok(sheet);
        }
        let resource = self.fetch(url, Some(&document.url), CSS_LIMIT, cancel, progress, Kind::Stylesheet)?;
        let sheet = Arc::new(parse_sheet(resource.status, resource.mime.as_ref(), &resource.bytes, key.1, Some(cancel))?);
        let mut sheets = self.sheets.lock().unwrap();
        if sheets.len() >= 1024 {
            sheets.retain(|_, w| w.strong_count() > 0);
        }
        sheets.insert(key, Arc::downgrade(&sheet));
        Ok(sheet)
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
    pub fn text(&self, initiator: &Url, url: Url, cancel: &Cancel) -> page::Result<(u16, String, String, String, String)> {
        let resource = self.fetch(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Fetch)?;
        let charset = resource.mime.as_ref().and_then(|m| m.get_param("charset")).and_then(|p| encoding_rs::Encoding::for_label(p.as_str().as_bytes()));
        let encoding = encoding_rs::Encoding::for_bom(&resource.bytes).map(|b| b.0).or(charset).unwrap_or(encoding_rs::UTF_8);
        let (text, _, _) = encoding.decode(&resource.bytes);
        Ok((resource.status.unwrap_or(200), resource.mime.as_ref().map(ToString::to_string).unwrap_or_default(), text.into_owned(), resource.url.to_string(), resource.headers))
    }
    /// A module script's GET (`olo_js::SCRIPT_DESTINATION`): as a script, so
    /// cached and joined with the fetch started for it with the page.
    pub fn script_response(&self, initiator: &Url, url: Url, cancel: &Cancel) -> page::Result<(u16, String, Vec<u8>, String, String)> {
        let resource = self.fetch(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Script)?;
        Ok((resource.status.unwrap_or(200), resource.mime.as_ref().map(ToString::to_string).unwrap_or_default(), resource.bytes, resource.url.to_string(), resource.headers))
    }
    /// A script's request with its method, headers (`name: value` lines)
    /// and body (fetch(), XMLHttpRequest): status, content type, bytes,
    /// final URL. Not cached.
    pub fn request(&self, initiator: &Url, url: Url, method: &str, headers: &str, body: &[u8], cancel: &Cancel) -> page::Result<(u16, String, Vec<u8>, String, String)> {
        self.runtime.block_on(self.request_async(initiator, url, method, headers, body, cancel))
    }
    async fn request_async(&self, initiator: &Url, url: Url, method: &str, headers: &str, body: &[u8], cancel: &Cancel) -> page::Result<(u16, String, Vec<u8>, String, String)> {
        let outgoing = Outgoing::new(method, headers, body)?;
        if olo_web::env_on!("OLO_FETCH_DEBUG") {
            eprintln!(
                "  request {method} {url}: {:?}; body {} bytes: {}",
                outgoing.headers.iter().map(|(n, v)| format!("{n}: {}", v.to_str().unwrap_or("?"))).collect::<Vec<_>>(),
                body.len(),
                String::from_utf8_lossy(&body[..body.len().min(160)])
            );
        }
        let mut progress = |_| {};
        let resource = {
            let work = get(&self.client, url, Some(initiator.clone()), SCRIPT_LIMIT, cancel.flag.clone(), &mut progress, Kind::Fetch, false, Some(&outgoing));
            tokio::select! { biased; _ = cancel.signal.cancelled() => Err("Cancelled".into()), result = work => result }
        }?;
        if olo_web::env_on!("OLO_FETCH_DEBUG") && resource.status.is_some_and(|s| s >= 400) {
            eprintln!("  answered {:?}: {}", resource.status, String::from_utf8_lossy(&resource.bytes[..resource.bytes.len().min(600)]));
        }
        Ok((resource.status.unwrap_or(200), resource.mime.as_ref().map(ToString::to_string).unwrap_or_default(), resource.bytes, resource.url.to_string(), resource.headers))
    }
    /// A live page's script's load (fetch(), XMLHttpRequest, a module):
    /// status, content type, bytes, final URL, without a thread waiting on
    /// it (run it with `spawn`).
    pub async fn script_load(&self, initiator: &Url, url: Url, method: &str, headers: &str, body: &[u8], cancel: &Cancel) -> page::Result<(u16, String, Vec<u8>, String, String)> {
        let respond = |r: Resource| (r.status.unwrap_or(200), r.mime.as_ref().map(ToString::to_string).unwrap_or_default(), r.bytes, r.url.to_string(), r.headers);
        if method == "GET" && headers == olo_js::SCRIPT_DESTINATION {
            self.fetch_async(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Script).await.map(respond)
        } else if method == "GET" && headers.is_empty() {
            self.fetch_async(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Fetch).await.map(respond)
        } else {
            self.request_async(initiator, url, method, headers, body, cancel).await
        }
    }
    /// A picture a live page's script asked for: decoded already (fetched
    /// ahead), else its bytes, for `decode_image` on the CPU pool.
    pub async fn image_source(&self, document: &Url, url: Url, cancel: &Cancel) -> page::Result<ImageSource> {
        if url.scheme() != "data"
            && let Some(Decoded::Image(image)) = self.decoded_now(&url)
        {
            return Ok(ImageSource::Decoded(image));
        }
        Ok(ImageSource::Bytes(self.fetch_async(url, Some(document), IMAGE_LIMIT, cancel, |_| {}, Kind::Image).await?.bytes))
    }
    /// Run this on the network runtime (a load's I/O; its processing goes
    /// to the CPU pool).
    pub fn spawn(&self, work: impl std::future::Future<Output = ()> + Send + 'static) {
        self.runtime.spawn(work);
    }
    /// A fetch() for scripts: status, content type, bytes, final URL.
    pub fn bytes(&self, initiator: &Url, url: Url, cancel: &Cancel) -> page::Result<(u16, String, Vec<u8>, String, String)> {
        let resource = self.fetch(url, Some(initiator), SCRIPT_LIMIT, cancel, |_| {}, Kind::Fetch)?;
        Ok((resource.status.unwrap_or(200), resource.mime.as_ref().map(ToString::to_string).unwrap_or_default(), resource.bytes, resource.url.to_string(), resource.headers))
    }
    /// A media file (a `<video>`'s), whole, up to `limit` bytes: its bytes
    /// and its type. Not kept in the cache (it is held by its player).
    pub fn media(&self, initiator: &Url, url: Url, limit: usize, cancel: &Cancel) -> page::Result<(Vec<u8>, String)> {
        let resource = self.fetch(url, Some(initiator), limit, cancel, |_| {}, Kind::Fetch)?;
        if resource.status.is_some_and(|s| !(200..300).contains(&s)) {
            return Err(format!("HTTP {}", resource.status.unwrap_or(0)).into());
        }
        Ok((resource.bytes, resource.mime.as_ref().map(ToString::to_string).unwrap_or_default()))
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
            // Decoded ahead, within the page's budget: taken as it is.
            if let Some(Decoded::Image(image)) = self.decoded(&url, cancel)
                && image.payload_bytes() <= remaining
            {
                return Ok(image);
            }
            // Whatever its status: a picture served with a 404 (a search
            // engine's stand-in icon) shows, as in Chrome; one that isn't a
            // picture fails to decode below.
            let resource = self.fetch(url, Some(&document.url), IMAGE_LIMIT, cancel, &mut progress, Kind::Image)?;
            resource.bytes
        };
        decode_image(&bytes, remaining, cancel, svg)
    }
}
/// A picture a script asked for: decoded ahead, or its bytes to decode.
pub enum ImageSource {
    Decoded(Arc<olo_web::image::Image>),
    Bytes(Vec<u8>),
}
/// A picture's bytes decoded within `remaining` bytes of pixels (an SVG
/// handed to `svg`).
pub fn decode_image(bytes: &[u8], remaining: usize, cancel: &AtomicBool, svg: &dyn Fn(&[u8]) -> page::Result<Arc<olo_web::image::Image>>) -> page::Result<Arc<olo_web::image::Image>> {
    if looks_like_svg(bytes) {
        return svg(bytes);
    }
    let mut limits = olo_images::raster::Limits::default();
    limits.surface_bytes = limits.surface_bytes.min(remaining as u64);
    limits.pixels = limits.pixels.min((remaining / 4) as u64);
    let raster = olo_images::raster::decode(bytes, limits, Some(cancel))?;
    let (w, h) = raster.pixels.dimensions();
    Ok(olo_web::image::Image::from_rgba(w, h, raster.pixels.into_raw(), Default::default(), Some(cancel))?)
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
    /// A web font's bytes: unpacked from WOFF (as a page holds it already,
    /// or as it was unpacked when it arrived, or now), else as they came
    /// (the engine takes what else it can).
    pub fn font(&self, document: &Resource, url: Url, cancel: &Cancel, progress: impl FnMut(usize)) -> page::Result<Arc<[u8]>> {
        if let Some(font) = self.fonts.lock().unwrap().get(url.as_str()).and_then(std::sync::Weak::upgrade) {
            return Ok(font);
        }
        let key = url.to_string();
        let font = match self.decoded(&url, cancel) {
            Some(Decoded::Font(font)) => font,
            _ => unpack_font(self.font_bytes(document, url, cancel, progress)?, Some(cancel)),
        };
        let mut fonts = self.fonts.lock().unwrap();
        if fonts.len() >= 256 {
            fonts.retain(|_, w| w.strong_count() > 0);
        }
        fonts.insert(key, Arc::downgrade(&font));
        Ok(font)
    }
    fn font_bytes(&self, document: &Resource, url: Url, cancel: &Cancel, mut progress: impl FnMut(usize)) -> page::Result<Arc<[u8]>> {
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
    /// A resource, waited for (on a thread that must have it: a page's
    /// preparation, a script's own synchronous request).
    fn fetch(&self, url: Url, initiator: Option<&Url>, limit: usize, cancel: &Cancel, progress: impl FnMut(usize), kind: Kind) -> page::Result<Resource> {
        self.runtime.block_on(self.fetch_async(url, initiator, limit, cancel, progress, kind))
    }
    /// A resource: as the session cache has it, or joining its fetch
    /// started ahead, or fetched; kept in the cache when it can be.
    async fn fetch_async(&self, url: Url, initiator: Option<&Url>, limit: usize, cancel: &Cancel, mut progress: impl FnMut(usize), kind: Kind) -> page::Result<Resource> {
        // A data: URL (a script's, a module's, fetch()'s, a style sheet's):
        // its own bytes and media type, decoded where it is.
        if url.scheme() == "data" {
            let data = data_url::DataUrl::process(url.as_str()).map_err(|_| "Malformed data URL")?;
            let mut bytes = Vec::new();
            data.decode(|chunk| {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled");
                }
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err("data: URL exceeds its size limit");
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .map_err(|_| "data: URL rejected or cancelled")?;
            progress(bytes.len());
            let mime = data.mime_type().to_string().parse::<mime::Mime>().ok();
            return Ok(Resource { url, bytes, mime, status: Some(200), document_encoding: None, headers: String::new() });
        }
        let allow_file = url.scheme() == "file";
        let cacheable = !matches!(kind, Kind::Document | Kind::Fetch) && url.scheme() != "file";
        if cacheable && let Some(c) = self.cache.lock().unwrap().entries.get(url.as_str()).cloned() {
            check_url(&url, initiator)?;
            return Ok(Resource { url: c.url.clone(), bytes: c.bytes.clone(), mime: c.mime.clone(), status: Some(c.status), document_encoding: None, headers: String::new() });
        }
        // Started ahead: joined (waited for, or done here first if it is
        // still queued).
        let started = if cacheable { self.inflight.lock().unwrap().get(url.as_str()).cloned() } else { None };
        if let Some(cell) = started
            && let Some(initiator) = initiator
        {
            check_url(&url, Some(initiator))?;
            let got = {
                let work = cell.get_or_init(|| fetch_once(self.client.clone(), url.clone(), initiator.clone(), limit, cancel.flag.clone(), kind));
                tokio::select! { biased; _ = cancel.signal.cancelled() => None, response = work => Some(response.clone()) }
            };
            self.inflight.lock().unwrap().remove(url.as_str());
            match got {
                None => return Err("Cancelled".into()),
                Some(Some(c)) => {
                    remember(&self.cache, url.as_str(), &c);
                    progress(c.bytes.len());
                    return Ok(Resource { url: c.url.clone(), bytes: c.bytes.clone(), mime: c.mime.clone(), status: Some(c.status), document_encoding: None, headers: String::new() });
                }
                // It failed: fetched again below, its error said there.
                Some(None) => {}
            }
        }
        let requested = url.clone();
        let fetched = {
            let work = get(&self.client, url, initiator.cloned(), limit, cancel.flag.clone(), &mut progress, kind, allow_file, None);
            tokio::select! { biased; _ = cancel.signal.cancelled() => Err("Cancelled".into()), result = work => result }
        }?;
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
    /// Start fetching these subresources together (12 at a time) into the
    /// session cache, so the loads that follow find them there: the
    /// preload scanner's work, after the parse. Failures are left to the
    /// load itself to report.
    pub fn prefetch(&self, document: &Resource, kind: page::ResourceKind, items: &[(Option<String>, String)], cancel: &Cancel) {
        let kind = match kind {
            page::ResourceKind::Stylesheet => Kind::Stylesheet,
            page::ResourceKind::Image => Kind::Image,
            page::ResourceKind::Font => Kind::Font,
            page::ResourceKind::Script => Kind::Script,
        };
        let limit = match kind {
            Kind::Stylesheet => CSS_LIMIT,
            Kind::Image => IMAGE_LIMIT,
            Kind::Script => SCRIPT_LIMIT,
            _ => FONT_LIMIT,
        };
        let urls: Vec<Url> = items.iter().filter_map(|(base, href)| base.as_deref().and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href).ok()).collect();
        self.start(document, kind, limit, urls, cancel);
    }
    /// Start fetching these in the background, at most `PREFETCHES` at
    /// once, without waiting: the loads that want them join them.
    fn start(&self, document: &Resource, kind: Kind, limit: usize, urls: Vec<Url>, cancel: &Cancel) {
        let initiator = document.url.clone();
        for url in urls {
            if !matches!(url.scheme(), "http" | "https") || check_url(&url, Some(&initiator)).is_err() || self.cache.lock().unwrap().entries.contains_key(url.as_str()) {
                continue;
            }
            let cell: Inflight = {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.contains_key(url.as_str()) {
                    continue;
                }
                let cell = Inflight::default();
                inflight.insert(url.to_string(), cell.clone());
                cell
            };
            let (client, cache, inflight, permits, waiters) = (self.client.clone(), self.cache.clone(), self.inflight.clone(), self.permits.clone(), self.waiters.clone());
            let _ = cancel;
            let ahead = self.ahead.lock().unwrap().0.clone();
            let (initiator, flag, signal) = (initiator.clone(), ahead.flag.clone(), ahead.signal.clone());
            // A picture or style sheet: decoded or parsed on the CPU pool
            // once it arrives.
            let environment = document.encoding();
            let decoding = matches!(kind, Kind::Image | Kind::Stylesheet | Kind::Font).then(|| {
                let d = Arc::new(Decoding::default());
                self.decoding.lock().unwrap().insert(url.to_string(), d.clone());
                d
            });
            let undecoded = decoding.clone();
            self.runtime.spawn(async move {
                let work = async {
                    let Ok(_permit) = permits.acquire_owned().await else { return };
                    let response = cell.get_or_init(|| fetch_once(client, url.clone(), initiator, limit, flag.clone(), kind)).await.clone();
                    // Kept: in the cache when it succeeded (no longer under
                    // way), else for the load that wants it (a 404's
                    // picture); then whoever waits for it told.
                    let finish = {
                        let response = response.clone();
                        move || {
                            if let Some(r) = &response
                                && (200..300).contains(&r.status)
                            {
                                remember(&cache, url.as_str(), r);
                                inflight.lock().unwrap().remove(url.as_str());
                            }
                            // (Taken under the lock `ready_or_start` registers under.)
                            let woken = waiters.lock().unwrap().remove(url.as_str()).unwrap_or_default();
                            for wake in woken {
                                wake();
                            }
                        }
                    };
                    match (&decoding, response) {
                        // A picture: its page told once it is decoded (taken
                        // then without waiting).
                        (Some(d), Some(r)) if matches!(kind, Kind::Image) && !looks_like_svg(&r.bytes) => {
                            let (d, flag) = (d.clone(), flag.clone());
                            rayon::spawn(move || {
                                d.set(decode_raster(&r.bytes, Some(&flag)).map_or(Decoded::Itself, Decoded::Image));
                                finish();
                            });
                        }
                        // A sheet: its page told once it is parsed (taken
                        // then without waiting).
                        (Some(d), Some(r)) if matches!(kind, Kind::Stylesheet) => {
                            let (d, flag) = (d.clone(), flag.clone());
                            rayon::spawn(move || {
                                // (Its declarations compiled too, ready for the
                                // page's compile.)
                                let parsed = parse_sheet(Some(r.status), r.mime.as_ref(), &r.bytes, environment, Some(&flag)).inspect(|s| drop(css::cascade::SheetBuilder::precompile(s, Some(&flag))));
                                d.set(parsed.map_or(Decoded::Itself, |s| Decoded::Sheet(Arc::new(s), environment)));
                                finish();
                            });
                        }
                        // A font: unpacked, its page told then.
                        (Some(d), Some(r)) if matches!(kind, Kind::Font) && (200..300).contains(&r.status) => {
                            let (d, flag) = (d.clone(), flag.clone());
                            rayon::spawn(move || {
                                d.set(Decoded::Font(unpack_font(Arc::from(&r.bytes[..]), Some(&flag))));
                                finish();
                            });
                        }
                        (d, _) => {
                            if let Some(d) = d {
                                d.set(Decoded::Itself);
                            }
                            finish();
                        }
                    }
                };
                tokio::select! { biased; _ = signal.cancelled() => {
                    if let Some(d) = undecoded {
                        d.set(Decoded::Itself);
                    }
                }, _ = work => {} }
            });
        }
    }
    /// What the last page started ahead and never took (failed fetches,
    /// pictures decoded for it): let go when another document loads.
    fn forget_ahead(&self, url: &Url) {
        {
            let mut ahead = self.ahead.lock().unwrap();
            // The same document again (a render asked for anew): its
            // fetches go on.
            if ahead.1.as_ref() == Some(url) {
                return;
            }
            std::mem::take(&mut ahead.0).cancel();
            ahead.1 = Some(url.clone());
        }
        self.decoding.lock().unwrap().retain(|_, d| d.result.lock().unwrap().is_none());
        self.inflight.lock().unwrap().retain(|_, cell| !cell.initialized());
    }
    /// A picture decoded or sheet parsed ahead, waited for (its fetch and
    /// decoding go on elsewhere), or None when there is none to take.
    fn decoded(&self, url: &Url, cancel: &Cancel) -> Option<Decoded> {
        let d = self.decoding.lock().unwrap().remove(url.as_str())?;
        let mut result = d.result.lock().unwrap();
        while result.is_none() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            result = d.ready.wait_timeout(result, Duration::from_millis(50)).unwrap().0;
        }
        result.take()
    }
    /// A picture decoded ahead when its decoding is done (taken), without
    /// waiting for one under way.
    fn decoded_now(&self, url: &Url) -> Option<Decoded> {
        let mut decoding = self.decoding.lock().unwrap();
        let taken = decoding.get(url.as_str())?.result.lock().unwrap().take()?;
        decoding.remove(url.as_str());
        Some(taken)
    }
    /// Whether a style sheet or font is here to load without waiting (in the
    /// cache, or its fetch done, whatever came of it). If not, its fetch is
    /// started (or already going) and `wake` is called when it lands.
    pub fn ready_or_start(&self, document: &Resource, kind: page::ResourceKind, url: &Url, cancel: &Cancel, wake: Box<dyn FnOnce() + Send>) -> bool {
        if !matches!(url.scheme(), "http" | "https") {
            return true;
        }
        let key = url.as_str();
        // Held by a page already (parsed, unpacked).
        if self.sheets.lock().unwrap().get(&(key.to_owned(), document.encoding())).is_some_and(|w| w.strong_count() > 0) || self.fonts.lock().unwrap().get(key).is_some_and(|w| w.strong_count() > 0) {
            return true;
        }
        let mut waiters = self.waiters.lock().unwrap();
        // Arrived but still being parsed: told when it is.
        if self.decoding.lock().unwrap().get(key).is_some_and(|d| d.result.lock().unwrap().is_none()) {
            waiters.entry(key.to_owned()).or_default().push(wake);
            return false;
        }
        if self.cache.lock().unwrap().entries.contains_key(key) {
            return true;
        }
        let started = self.inflight.lock().unwrap().get(key).cloned();
        match started {
            Some(cell) if cell.initialized() => true,
            Some(_) => {
                waiters.entry(key.to_owned()).or_default().push(wake);
                false
            }
            None => {
                waiters.entry(key.to_owned()).or_default().push(wake);
                drop(waiters);
                self.prefetch(document, kind, &[(None, url.to_string())], cancel);
                // Refused to start (a scheme or mixed content): loaded (and
                // refused) as before.
                if !self.inflight.lock().unwrap().contains_key(key) {
                    self.waiters.lock().unwrap().remove(key);
                    return true;
                }
                false
            }
        }
    }
    /// Start fetching a page's scripts ahead (its external classic and
    /// module scripts, its preloads), as the preload scanner does.
    pub fn prefetch_scripts(&self, document: &Resource, urls: Vec<Url>, cancel: &Cancel) {
        self.start(document, Kind::Script, SCRIPT_LIMIT, urls, cancel);
    }
}
/// One GET (redirects followed, 20 at most), or a local file read.
#[allow(clippy::too_many_arguments)]
/// A script's request: its method, the headers it may set, its body.
struct Outgoing {
    method: reqwest::Method,
    headers: Vec<(header::HeaderName, header::HeaderValue)>,
    body: Vec<u8>,
    /// Its credentials mode when not same-origin (fetch's `credentials`,
    /// XMLHttpRequest's `withCredentials`): cookies with every request
    /// (include) or none (omit).
    credentials: Option<Credentials>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Credentials {
    Omit,
    SameOrigin,
    Include,
}
impl Outgoing {
    fn new(method: &str, headers: &str, body: &[u8]) -> page::Result<Self> {
        let method = reqwest::Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes()).map_err(|_| "Invalid request method")?;
        if matches!(method.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err("Forbidden request method".into());
        }
        // Fetch §2.2.2: headers a page may not set.
        const FORBIDDEN: &[&str] = &[
            "accept-charset",
            "accept-encoding",
            "access-control-request-headers",
            "access-control-request-method",
            "connection",
            "content-length",
            "cookie",
            "cookie2",
            "date",
            "dnt",
            "expect",
            "host",
            "keep-alive",
            "origin",
            "referer",
            "set-cookie",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "user-agent",
            "via",
        ];
        let mut out = Vec::new();
        let mut credentials = None;
        for line in headers.lines() {
            let Some((name, value)) = line.split_once(':') else { continue };
            let name = name.trim().to_ascii_lowercase();
            // The page's scripts say their credentials mode so (never sent).
            if name == "x-olo-credentials" {
                credentials = match value.trim() {
                    "omit" => Some(Credentials::Omit),
                    "include" => Some(Credentials::Include),
                    _ => None,
                };
                continue;
            }
            // A CORS preflight's questions, from the page's CORS code (an
            // OPTIONS request only): the headers a page may not set itself.
            if let Some(asked) = name.strip_prefix("x-olo-preflight-")
                && method == reqwest::Method::OPTIONS
                && matches!(asked, "method" | "headers")
                && let Ok(v) = header::HeaderValue::from_str(value.trim())
            {
                let n = if asked == "method" { "access-control-request-method" } else { "access-control-request-headers" };
                out.push((header::HeaderName::from_static(n), v));
                continue;
            }
            if FORBIDDEN.contains(&name.as_str()) || name.starts_with("proxy-") || name.starts_with("sec-") || name.starts_with("x-olo-") {
                continue;
            }
            if let (Ok(n), Ok(v)) = (header::HeaderName::from_bytes(name.as_bytes()), header::HeaderValue::from_str(value.trim())) {
                out.push((n, v));
            }
        }
        Ok(Self { method, headers: out, body: body.to_vec(), credentials })
    }
}
#[allow(clippy::too_many_arguments)]
async fn get(
    client: &Client,
    mut url: Url,
    initiator: Option<Url>,
    limit: usize,
    cancel: Arc<AtomicBool>,
    progress: &mut impl FnMut(usize),
    kind: Kind,
    allow_file: bool,
    outgoing: Option<&Outgoing>,
) -> page::Result<Resource> {
    let initiator = initiator.as_ref();
    // A redirect may turn the request into a GET (below).
    let mut method = outgoing.map_or(reqwest::Method::GET, |o| o.method.clone());
    {
        {
            {
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
                        return Ok(Resource { url, bytes, mime, status: None, document_encoding: None, headers: String::new() });
                    }
                    let mut request = client.request(method.clone(), url.clone());
                    // Cookies, as the request's credentials mode allows:
                    // a script's request and a font (CORS) only to their own
                    // origin unless they include them; the rest always.
                    let credentials = outgoing.and_then(|o| o.credentials).unwrap_or(match kind {
                        Kind::Fetch | Kind::Font => Credentials::SameOrigin,
                        _ => Credentials::Include,
                    });
                    let with_cookies = match credentials {
                        Credentials::Omit => false,
                        Credentials::Include => true,
                        Credentials::SameOrigin => initiator.is_none_or(|i| i.origin() == url.origin()),
                    };
                    let context = crate::cookies::Context {
                        site: initiator,
                        navigation: matches!(kind, Kind::Document) && initiator.is_none(),
                        safe: matches!(method, reqwest::Method::GET | reqwest::Method::HEAD),
                    };
                    let cookie = if with_cookies { crate::cookies::jar().header(&url, context) } else { None };
                    for (name, value) in chrome_headers(&url, initiator, kind, &method, outgoing, cookie) {
                        request = request.header(name, value);
                    }
                    if let Some(o) = outgoing
                        && !matches!(method, reqwest::Method::GET | reqwest::Method::HEAD)
                    {
                        request = request.body(o.body.clone());
                    }
                    // (With what it came from: "error sending request" alone
                    // says nothing of DNS, TLS or the connection.)
                    let mut response = request.send().await.map_err(|e| {
                        let mut text = e.to_string();
                        let mut source = std::error::Error::source(&e);
                        while let Some(cause) = source {
                            text.push_str(": ");
                            text.push_str(&cause.to_string());
                            source = cause.source();
                        }
                        page::Error::from(text.as_str())
                    })?;
                    let status = response.status();
                    // The client hints a document's server asks for, sent to
                    // its origin from now on (as Chrome keeps them).
                    if matches!(kind, Kind::Document)
                        && let Some(asked) = response.headers().get("accept-ch").and_then(|v| v.to_str().ok())
                    {
                        remember_hints(&url, asked);
                    }
                    if with_cookies {
                        for line in response.headers().get_all(header::SET_COOKIE) {
                            if let Ok(line) = std::str::from_utf8(line.as_bytes()) {
                                crate::cookies::jar().set_from_response(&url, line);
                            }
                        }
                    }
                    if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
                        && let Some(location) = response.headers().get(header::LOCATION)
                    {
                        // Fetch §4.4: a 303 (not after a HEAD), or a 301 or 302
                        // after a POST, goes on as a GET without its body.
                        if status.as_u16() == 303 && method != reqwest::Method::HEAD || matches!(status.as_u16(), 301 | 302) && method == reqwest::Method::POST {
                            method = reqwest::Method::GET;
                        }
                        let location = location.to_str()?;
                        let mut target = url.join(location)?;
                        if !location.contains('#') {
                            target.set_fragment(url.fragment());
                        }
                        url = target;
                        continue;
                    }
                    let mime = response.headers().get(header::CONTENT_TYPE).map(|h| h.to_str()?.parse::<mime::Mime>().map_err(page::Error::from)).transpose()?;
                    // (Only a script's own request shows its headers.)
                    let headers = if matches!(kind, Kind::Fetch) { header_lines(response.headers()) } else { String::new() };
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        append(&mut bytes, &chunk, limit)?;
                        progress(bytes.len());
                    }
                    return Ok(Resource { url, bytes, mime, status: Some(status.as_u16()), document_encoding: None, headers });
                }
                Err("Too many HTTP redirects".into())
            }
        }
    }
}
/// A response's headers as scripts may read them: `name: value` lines, a
/// name repeated as the response repeats it, without `Set-Cookie`.
fn header_lines(headers: &reqwest::header::HeaderMap) -> String {
    let mut out = String::new();
    for (name, value) in headers {
        if matches!(name.as_str(), "set-cookie" | "set-cookie2") {
            continue;
        }
        let Ok(value) = value.to_str() else { continue };
        out.push_str(name.as_str());
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
    out
}
/// The Referer a request from `from` (the page, or a stylesheet) to `to`
/// carries, by the default policy, strict-origin-when-cross-origin (as
/// Chrome sends): the whole URL, less its fragment and credentials, on
/// the same origin; the origin alone across origins; nothing from https to
/// http, or from a page with no web origin (a file). Some servers answer
/// without one with 404 (apple.com's fonts stylesheet).
/// The client hints each origin's server asked for (`Accept-CH`), by
/// origin.
static ASKED_HINTS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Vec<&'static str>>>> = std::sync::LazyLock::new(Default::default);
/// The high-entropy client hints Olo answers (Chrome on Linux's values),
/// in the order Chrome sends them.
const HIGH_ENTROPY: [(&str, &str); 7] = [
    ("sec-ch-ua-arch", "\"x86\""),
    ("sec-ch-ua-platform-version", "\"\""),
    ("sec-ch-ua-model", "\"\""),
    ("sec-ch-ua-bitness", "\"64\""),
    ("sec-ch-ua-full-version-list", olo_js::SEC_CH_UA_FULL),
    ("sec-ch-ua-full-version", concat!("\"", "157.0.8103.38", "\"")),
    ("sec-ch-ua-wow64", "?0"),
];
fn remember_hints(url: &Url, asked: &str) {
    let wanted: Vec<&'static str> = asked.split(',').map(|h| h.trim().to_ascii_lowercase()).filter_map(|h| HIGH_ENTROPY.iter().find(|(n, _)| *n == h).map(|(n, _)| *n)).collect();
    let mut hints = ASKED_HINTS.lock().unwrap();
    if hints.len() > 4096 {
        hints.clear();
    }
    hints.insert(url.origin().ascii_serialization(), wanted);
}
/// How a request's target stands to what asked for it (`Sec-Fetch-Site`):
/// none (the person asked: typed, a bookmark), the same origin, the same
/// site (scheme and registrable domain), or another site.
fn fetch_site(initiator: Option<&Url>, url: &Url) -> &'static str {
    let Some(from) = initiator else { return "none" };
    if from.origin() == url.origin() {
        return "same-origin";
    }
    let site = |u: &Url| (u.scheme().to_owned(), u.host_str().map(crate::cookies::site_of));
    if site(from) == site(url) { "same-site" } else { "cross-site" }
}
/// A request's headers as Chrome 157 sends them, in its order (captured
/// from it: what servers and their bot checks compare a browser against):
/// the page's own headers (a CORS request's origin before them), the
/// client hints (the high-entropy ones its origin asked for), the user
/// agent, what it accepts for its destination, its `Sec-Fetch-*`, the
/// referrer, encodings, languages and cookies.
fn chrome_headers(url: &Url, initiator: Option<&Url>, kind: Kind, method: &reqwest::Method, outgoing: Option<&Outgoing>, cookie: Option<String>) -> Vec<(header::HeaderName, header::HeaderValue)> {
    let mut out: Vec<(header::HeaderName, header::HeaderValue)> = Vec::with_capacity(24);
    let mut add = |name: &str, value: &str| {
        if let (Ok(n), Ok(v)) = (header::HeaderName::from_bytes(name.as_bytes()), header::HeaderValue::from_str(value)) {
            out.push((n, v));
        }
    };
    let navigation = matches!(kind, Kind::Document);
    let safe = matches!(*method, reqwest::Method::GET | reqwest::Method::HEAD);
    let same_origin = initiator.is_some_and(|i| i.origin() == url.origin());
    let origin = initiator.filter(|i| matches!(i.scheme(), "http" | "https")).map(|i| i.origin().ascii_serialization());
    let mode = match kind {
        Kind::Document => "navigate",
        Kind::Font | Kind::Fetch => "cors",
        Kind::Stylesheet | Kind::Image | Kind::Script => "no-cors",
    };
    // A font (always CORS) names its origin first; a script's request to
    // another origin, or one with a body, after what it accepts.
    if matches!(kind, Kind::Font)
        && let Some(o) = &origin
    {
        add("origin", o);
    }
    let page_headers: Vec<&(header::HeaderName, header::HeaderValue)> =
        outgoing.map(|o| o.headers.iter().filter(|(n, _)| !(*method == reqwest::Method::GET && o.method != reqwest::Method::GET && n == header::CONTENT_TYPE)).collect()).unwrap_or_default();
    let page_accept = page_headers.iter().find(|(n, _)| n == header::ACCEPT).map(|(_, v)| v.clone());
    let content_type = page_headers.iter().find(|(n, _)| n == header::CONTENT_TYPE).map(|(_, v)| v.clone());
    for (n, v) in &page_headers {
        if n != header::ACCEPT && n != header::CONTENT_TYPE {
            out.push(((*n).clone(), (*v).clone()));
        }
    }
    let secure = url.scheme() == "https" || url.host_str().is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]" || h.ends_with(".localhost"));
    let asked: Vec<&'static str> = if secure { ASKED_HINTS.lock().unwrap().get(&url.origin().ascii_serialization()).cloned().unwrap_or_default() } else { Vec::new() };
    let mut add = |name: &str, value: &str| {
        if let (Ok(n), Ok(v)) = (header::HeaderName::from_bytes(name.as_bytes()), header::HeaderValue::from_str(value)) {
            out.push((n, v));
        }
    };
    if secure {
        let high = |add: &mut dyn FnMut(&str, &str)| {
            for (name, value) in HIGH_ENTROPY {
                if asked.contains(&name) {
                    add(name, value);
                }
            }
        };
        if navigation {
            add("sec-ch-ua", olo_js::SEC_CH_UA);
            add("sec-ch-ua-mobile", "?0");
            add("sec-ch-ua-platform", "\"Linux\"");
            high(&mut add);
        } else {
            add("sec-ch-ua-platform", "\"Linux\"");
            add("sec-ch-ua", olo_js::SEC_CH_UA);
            add("sec-ch-ua-mobile", "?0");
            high(&mut add);
        }
    }
    if navigation {
        add("upgrade-insecure-requests", "1");
    }
    add("user-agent", olo_js::USER_AGENT);
    if let Some(ct) = &content_type {
        out_push_raw(&mut add, "content-type", ct);
    }
    let accept = match kind {
        Kind::Document => "text/html,application/xhtml+xml,application/xml;q=0.9,image/jxl,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        Kind::Stylesheet => "text/css,*/*;q=0.1",
        Kind::Image => "image/jxl,image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        Kind::Font | Kind::Script | Kind::Fetch => "*/*",
    };
    match &page_accept {
        Some(v) => out_push_raw(&mut add, "accept", v),
        None => add("accept", accept),
    }
    if !matches!(kind, Kind::Font)
        && (!safe || matches!(kind, Kind::Fetch) && !same_origin)
        && let Some(o) = &origin
    {
        add("origin", o);
    }
    if secure {
        add("sec-fetch-site", fetch_site(initiator, url));
        add("sec-fetch-mode", mode);
        if navigation {
            add("sec-fetch-user", "?1");
        }
        add(
            "sec-fetch-dest",
            match kind {
                Kind::Document => "document",
                Kind::Stylesheet => "style",
                Kind::Image => "image",
                Kind::Font => "font",
                Kind::Script => "script",
                Kind::Fetch => "empty",
            },
        );
    }
    if let Some(r) = initiator.and_then(|from| referrer(from, url)) {
        add("referer", &r);
    }
    add("accept-encoding", "gzip, deflate, br, zstd");
    add("accept-language", "en-US,en;q=0.9");
    if let Some(c) = &cookie {
        add("cookie", c);
    }
    out
}
/// A header value given as it is (a page's own, already checked).
fn out_push_raw(add: &mut impl FnMut(&str, &str), name: &str, value: &header::HeaderValue) {
    if let Ok(v) = value.to_str() {
        add(name, v);
    }
}
fn referrer(from: &Url, to: &Url) -> Option<String> {
    if !matches!(from.scheme(), "http" | "https") || from.scheme() == "https" && to.scheme() == "http" {
        return None;
    }
    if from.origin() == to.origin() {
        let mut url = from.clone();
        url.set_fragment(None);
        let _ = url.set_username("");
        let _ = url.set_password(None);
        return Some(url.to_string());
    }
    Some(format!("{}/", from.origin().ascii_serialization()))
}
/// A document of this media type is shown as its text (plain text, JSON,
/// scripts, style sheets, CSV, Markdown), as Chrome shows them.
fn shown_as_text(kind: &str) -> bool {
    matches!(kind, "text/plain" | "text/css" | "text/csv" | "text/markdown" | "text/javascript" | "application/javascript" | "application/json" | "text/json" | "application/x-javascript")
        || kind.starts_with("application/") && kind.ends_with("+json")
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
    /// This resource's identity (URL, media type, status, encoding) without
    /// its bytes, for loading relative to it later.
    pub fn light(&self) -> Resource {
        Resource { url: self.url.clone(), bytes: Vec::new(), mime: self.mime.clone(), status: self.status, document_encoding: Some(self.encoding()), headers: String::new() }
    }
    fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.document_encoding.unwrap_or_else(|| {
            let protocol = self.mime.as_ref().and_then(|m| m.get_param("charset")).map(|p| p.as_str());
            if let Some(kind) = self.mime.as_ref().map(|m| m.essence_str()).filter(|k| shown_as_text(k)) {
                // JSON is UTF-8 by its standard; other text, Windows-1252
                // unless it says otherwise (as Chrome reads it).
                let default = if kind == "text/plain" { encoding_rs::WINDOWS_1252 } else { encoding_rs::UTF_8 };
                encoding_rs::Encoding::for_bom(&self.bytes).map(|p| p.0).or_else(|| protocol.and_then(|p| encoding_rs::Encoding::for_label(p.as_bytes()))).unwrap_or(default)
            } else {
                olo_web::html::bytes::encoding(&self.bytes, protocol).0
            }
        })
    }
    /// Decode once, retaining only URL/transport/encoding metadata for sheets.
    /// Release the encoded document buffer before parsing/layout allocations.
    pub fn take_html(&mut self, cancel: &Cancel) -> page::Result<Arc<str>> {
        let kind = self.mime.as_ref().map(|m| m.essence_str());
        if !matches!(kind, None | Some("text/html")) && !kind.is_some_and(shown_as_text) {
            return Err("This document MIME type is not yet supported".into());
        }
        let skip = olo_web::html::bytes::encoding(&self.bytes, None).1;
        self.document_encoding = Some(self.encoding());
        let plain = kind.is_some_and(shown_as_text);
        // Text is shown as Chrome shows it: preformatted, wrapping.
        let mut output = if plain {
            "<!doctype html><meta charset=utf-8><meta name=color-scheme content=\"light dark\"><pre style=\"word-wrap: break-word; white-space: pre-wrap\">".to_owned()
        } else {
            String::new()
        };
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
    fn resources_fetched_ahead_go_together_once_and_a_load_joins_them() {
        // Each answer 200 ms late, on threads of its own: six scripts and
        // a sheet started ahead take one wait, not seven; each asked once.
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", socket.local_addr().unwrap());
        let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = asked.clone();
        thread::spawn(move || {
            for stream in socket.incoming() {
                let Ok(mut stream) = stream else { break };
                let seen = seen.clone();
                thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut b = [0u8];
                    while !request.ends_with(b"\r\n\r\n") && stream.read(&mut b).unwrap_or(0) == 1 {
                        request.push(b[0]);
                    }
                    let path = String::from_utf8_lossy(&request).split_ascii_whitespace().nth(1).unwrap_or("").to_owned();
                    seen.lock().unwrap().push(path.clone());
                    if path != "/page" {
                        thread::sleep(Duration::from_millis(200));
                    }
                    let (kind, body) = if path.ends_with(".css") {
                        ("text/css", "a{}")
                    } else if path.ends_with(".js") {
                        ("text/javascript", "1")
                    } else {
                        ("text/html", "<p>")
                    };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                });
            }
        });
        let loader = Loader::new().unwrap();
        let cancel = Cancel::default();
        let doc = loader.document(&format!("{base}/page"), None, &cancel, |_| {}).unwrap();
        let started = Instant::now();
        let scripts: Vec<Url> = (0..6).map(|i| Url::parse(&format!("{base}/s{i}.js")).unwrap()).collect();
        loader.prefetch_scripts(&doc, scripts.clone(), &cancel);
        loader.prefetch(&doc, page::ResourceKind::Stylesheet, &[(None, "/t.css".into())], &cancel);
        // Started ahead: nothing waited for yet.
        assert!(started.elapsed() < Duration::from_millis(100), "{:?}", started.elapsed());
        // A render asked for anew (the same document) lets them go on.
        let _ = loader.document(&format!("{base}/page"), None, &Cancel::default(), |_| {}).unwrap();
        // The loads join them: the sheet and every script in about one wait.
        assert_eq!(loader.stylesheet(&doc, None, "/t.css", &cancel, |_| {}).unwrap().source(), "a{}");
        for url in scripts {
            assert_eq!(loader.script(&doc, url, &cancel, |_| {}).unwrap(), "1");
        }
        assert!(started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
        let asked = asked.lock().unwrap().clone();
        for path in ["/t.css", "/s0.js", "/s5.js"] {
            assert_eq!(asked.iter().filter(|p| *p == path).count(), 1, "{asked:?}");
        }
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
    fn referrers_follow_strict_origin_when_cross_origin() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert_eq!(referrer(&u("https://a.test/p/q?x=1#frag"), &u("https://a.test/style.css")).as_deref(), Some("https://a.test/p/q?x=1"));
        assert_eq!(referrer(&u("https://user:pw@a.test/p"), &u("https://a.test/s")).as_deref(), Some("https://a.test/p"));
        assert_eq!(referrer(&u("https://a.test/p/q"), &u("https://cdn.test/s.css")).as_deref(), Some("https://a.test/"));
        assert_eq!(referrer(&u("https://a.test/p"), &u("http://a.test/s")), None);
        assert_eq!(referrer(&u("file:///tmp/p.html"), &u("https://a.test/s")), None);
    }
    #[test]
    fn wrong_css_mime_is_rejected() {
        let (base, server) = server(&[("/theme", "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\nbody{color:red}")], 1);
        let loader = Loader::new().unwrap();
        let doc = Resource { url: Url::parse(&base).unwrap(), bytes: vec![], mime: None, status: None, document_encoding: None, headers: String::new() };
        assert!(loader.stylesheet(&doc, None, "/theme", &Cancel::default(), |_| {}).is_err());
        server.join().unwrap();
    }
    #[test]
    fn image_data_and_http_use_bounded_worker_decode() {
        let data = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
        let loader = Loader::new().unwrap();
        let document = Resource { url: Url::parse("https://example.test/page").unwrap(), bytes: Vec::new(), mime: None, status: None, document_encoding: None, headers: String::new() };
        let no_svg = |_: &[u8]| -> page::Result<Arc<olo_web::image::Image>> { Err("no SVG here".into()) };
        let image = loader.image(&document, Url::parse(data).unwrap(), 4, &Cancel::default(), |_| {}, &no_svg).unwrap();
        assert_eq!(image.rgba(), [255, 0, 0, 255]);
        assert!(loader.image(&document, Url::parse(data).unwrap(), 3, &Cancel::default(), |_| {}, &no_svg).is_err());
        let cancelled = Cancel::default();
        cancelled.cancel();
        assert!(loader.image(&document, Url::parse(data).unwrap(), 4, &cancelled, |_| {}, &no_svg).is_err());
        let png = data_url::DataUrl::process(data).unwrap().decode_to_vec().unwrap().0;
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/red.png", socket.local_addr().unwrap())).unwrap();
        let document = Resource { url: url.clone(), bytes: Vec::new(), mime: None, status: None, document_encoding: None, headers: String::new() };
        let server = thread::spawn(move || {
            let (mut stream, _) = socket.accept().unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            // What Chrome accepts for an image, and its destination.
            let request = std::str::from_utf8(&request[..n]).unwrap();
            assert!(request.contains("accept: image/jxl,image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8"), "{request}");
            assert!(request.contains("user-agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36\r\n"), "{request}");
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", png.len()).unwrap();
            stream.write_all(&png).unwrap();
        });
        let image = loader.image(&document, url, 4, &Cancel::default(), |_| {}, &no_svg).unwrap();
        assert_eq!(image.rgba(), [255, 0, 0, 255]);
        server.join().unwrap();
    }
}
