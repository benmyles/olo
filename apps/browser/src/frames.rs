//! The page's `<iframe>`s: each one's document loaded and rendered on a
//! thread of its own, at the size the iframe shows at, and bound to its
//! element as a picture (as a video's picture is). Its stylesheets and
//! images load as the page's do. A frame with scripts is live: its realm
//! goes on on its thread (timers, its loads, animation frames), its
//! picture drawn again as it changes (at most 30 times a second), the
//! person's input over it passed in, and it and the page post messages to
//! each other's windows (the browser carrying structured clones, origins
//! checked). Captchas, embeds and widgets are such frames.
use crate::{
    network,
    worker::{Key, Loads, Message, PageHost},
};
use olo_web::{dom::NodeId, font::Scope, image::Image, page, paint, text::shaping};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

/// What a frame's thread has made: nothing yet, its picture (until
/// taken), or nothing more to come (taken, or failed).
enum Made {
    Loading,
    Done(Arc<Image>),
    Settled,
}
struct Frame {
    src: String,
    /// The device size it was rendered for.
    size: [u32; 2],
    made: Arc<Mutex<Made>>,
    cancel: network::Cancel,
    image: Option<Arc<Image>>,
    /// Its thread's inbox: input and messages for its page.
    inbox: mpsc::Sender<Message>,
    /// What its page posted to the parent's window: (clone, target origin).
    outbox: Arc<Mutex<Vec<(Vec<u8>, String)>>>,
    /// Its document's origin, once fetched.
    origin: Arc<Mutex<Option<String>>>,
    /// Whether it is in view: out of it, its page runs no animation frames
    /// and draws no pictures (as Chrome throttles off-screen frames).
    shown: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Frame {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Default)]
pub struct Frames {
    frames: HashMap<NodeId, Frame>,
    /// The document revision the iframes were last looked for at.
    scanned: Option<u64>,
    found: Vec<(NodeId, String)>,
}
/// Each iframe of the document with the URL it loads (against `base`).
fn iframes(doc: &olo_web::dom::Document, base: &url::Url) -> Vec<(NodeId, String)> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.name(n).ok().flatten() == Some("iframe")
            && let Some(src) = doc.attribute(n, "src").ok().flatten().map(str::trim).filter(|s| !s.is_empty())
            && let Ok(url) = base.join(src)
            && matches!(url.scheme(), "http" | "https" | "file")
        {
            out.push((n, url.to_string()));
        }
        if let Ok(children) = doc.children(n) {
            stack.extend(children);
        }
    }
    out
}
impl Frames {
    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }
    /// The document revision the iframes were last looked for at.
    pub fn scanned_at(&self) -> Option<u64> {
        self.scanned
    }
    /// Input over an iframe (CSS px in the frame's own document) for its
    /// page: whether it has one to take it.
    pub fn input(&self, node: NodeId, input: olo_js::Input) -> bool {
        self.frames.get(&node).is_some_and(|f| f.inbox.send(Message::Input { navigation: 0, wheel: None, input }).is_ok())
    }
    /// Whether an element is a frame being shown.
    pub fn has(&self, node: NodeId) -> bool {
        self.frames.contains_key(&node)
    }
    /// A message the page (of `origin`) posted to an iframe's window, for
    /// the frame's page if its origin is the one asked for ("*" any, "/"
    /// the page's own).
    pub fn post(&self, node: NodeId, data: Vec<u8>, target: &str, origin: &str) {
        let Some(f) = self.frames.get(&node) else { return };
        let theirs = f.origin.lock().unwrap().clone();
        if target_matches(target, theirs.as_deref(), origin) {
            let _ = f.inbox.send(Message::FrameMessage(data, origin.to_owned()));
        }
    }
    /// Messages frames' pages posted to the page's window (of `origin`),
    /// each with the frame's element and origin: those for this origin.
    pub fn take_messages(&self, origin: &str) -> Vec<(NodeId, Vec<u8>, String)> {
        let mut out = Vec::new();
        for (node, f) in &self.frames {
            let posted = std::mem::take(&mut *f.outbox.lock().unwrap());
            let theirs = f.origin.lock().unwrap().clone().unwrap_or_default();
            for (data, target) in posted {
                if target_matches(&target, Some(origin), &theirs) {
                    out.push((*node, data, theirs.clone()));
                }
            }
        }
        out
    }
    /// The pictures to bind, by element.
    pub fn images(&self) -> Vec<(NodeId, Arc<Image>)> {
        self.frames.iter().filter_map(|(n, f)| f.image.clone().map(|i| (*n, i))).collect()
    }
    /// Follow the document's iframes (`doc` when the page's revision
    /// changed since the last look), start each one's render at the size it
    /// shows at (`shown`, device pixels; again when that changes), and take
    /// in the pictures made. Whether a picture came, and whether one is
    /// still being made (look again soon).
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        revision: u64,
        doc: impl FnOnce() -> Option<olo_web::dom::Document>,
        loader: &Arc<network::Loader>,
        base: &url::Url,
        scope: &Scope,
        key: &Key,
        shown: &dyn Fn(NodeId) -> Option<[u32; 2]>,
        in_view: &dyn Fn(NodeId) -> bool,
        wake: &olo_js::Waker,
    ) -> (bool, bool) {
        if self.scanned != Some(revision)
            && let Some(doc) = doc()
        {
            self.scanned = Some(revision);
            self.found = iframes(&doc, base);
            if olo_web::env_on!("OLO_BROWSER_TIMING") && !self.found.is_empty() {
                eprintln!("  frames: {} iframes, shown at {:?}", self.found.len(), self.found.iter().map(|(n, _)| shown(*n)).collect::<Vec<_>>());
            }
            self.frames.retain(|n, f| self.found.iter().any(|(m, src)| m == n && *src == f.src));
        }
        let mut changed = false;
        let mut pending = false;
        for (node, src) in self.found.clone() {
            let Some(size) = shown(node).filter(|[w, h]| *w > 0 && *h > 0 && *w <= 8192 && *h <= 8192) else { continue };
            let restart = self.frames.get(&node).is_none_or(|f| f.size != size);
            if restart {
                let made = Arc::new(Mutex::new(Made::Loading));
                let cancel = network::Cancel::default();
                let image = self.frames.remove(&node).and_then(|mut f| f.image.take());
                let (inbox, rx) = mpsc::channel();
                let outbox = Arc::new(Mutex::new(Vec::new()));
                let origin = Arc::new(Mutex::new(None));
                let in_sight = Arc::new(std::sync::atomic::AtomicBool::new(in_view(node)));
                let ends = Ends { inbox: inbox.clone(), rx, outbox: outbox.clone(), origin: origin.clone(), shown: in_sight.clone() };
                start(src.clone(), base.clone(), size, made.clone(), cancel.clone(), loader.clone(), scope.clone(), key.clone(), wake.clone(), ends);
                self.frames.insert(node, Frame { src, size, made, cancel, image, inbox, outbox, origin, shown: in_sight });
            }
            let f = self.frames.get_mut(&node).expect("inserted above");
            // Come into view: its page goes on drawing.
            let visible = in_view(node);
            if f.shown.swap(visible, std::sync::atomic::Ordering::Relaxed) != visible && visible {
                let _ = f.inbox.send(Message::Wake);
            }
            // (Taken out under the lock, which is let go before the match.)
            let made = std::mem::replace(&mut *f.made.lock().unwrap(), Made::Settled);
            match made {
                Made::Done(image) => {
                    f.image = Some(image);
                    changed = true;
                }
                Made::Loading => {
                    *f.made.lock().unwrap() = Made::Loading;
                    pending = true;
                }
                Made::Settled => {}
            }
        }
        (changed, pending)
    }
}
/// Whether a message posted for `target` ("*", "/" for the sender's own
/// origin, or an origin) may go to a window of origin `to`.
fn target_matches(target: &str, to: Option<&str>, from: &str) -> bool {
    match target {
        "*" => true,
        "/" => to == Some(from),
        t => url::Url::parse(t).is_ok_and(|u| Some(u.origin().ascii_serialization().as_str()) == to),
    }
}
/// The frame's thread's ends of its channels with the page's.
struct Ends {
    inbox: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    outbox: Arc<Mutex<Vec<(Vec<u8>, String)>>>,
    origin: Arc<Mutex<Option<String>>>,
    shown: Arc<std::sync::atomic::AtomicBool>,
}
/// How often a live frame's picture may be drawn again.
const FRAME_REDRAW: Duration = Duration::from_millis(33);
/// The frame's document fetched, laid out and painted at `size` (device
/// pixels, at the page's scale and palette), on a thread of its own.
#[allow(clippy::too_many_arguments)]
fn start(src: String, parent: url::Url, size: [u32; 2], made: Arc<Mutex<Made>>, cancel: network::Cancel, loader: Arc<network::Loader>, scope: Scope, key: Key, wake: olo_js::Waker, ends: Ends) {
    let slot = made.clone();
    let spawned = std::thread::Builder::new().name("browser-frame".into()).spawn(move || {
        let publish = |image: Arc<Image>| {
            *made.lock().unwrap() = Made::Done(image);
            wake();
        };
        let result = (|| -> page::Result<()> {
            let mut document = loader.frame_document(url::Url::parse(&src)?, &parent, &cancel)?;
            if document.status.is_some_and(|s| !(200..300).contains(&s)) {
                return Err("The frame's document returned an unsuccessful HTTP status".into());
            }
            let html = document.take_html(&cancel)?;
            *ends.origin.lock().unwrap() = Some(document.url.origin().ascii_serialization());
            let key = Key { source: src.clone(), cwd: None, revision: 0, width: size[0], height: size[1], scale: key.scale, palette: key.palette.clone() };
            let host = PageHost::new(&scope, &key);
            let mut shaper = shaping::Engine::default();
            let mut painter = paint::Engine::default();
            let svg = |bytes: &[u8]| olo_web::svg_image::rasterize(bytes, &host, &scope, key.scale, Some(&cancel)).map_err(|e| format!("SVG image: {e}").into());
            let mut image_bytes = 0usize;
            let resource = |kind: page::ResourceKind, base: Option<&str>, href: &str| -> page::Result<Option<page::Resource>> {
                Ok(match kind {
                    page::ResourceKind::Stylesheet => {
                        let absolute = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href).ok().map(|u| Arc::<str>::from(u.as_str()));
                        loader.stylesheet(&document, base, href, &cancel, |_| {}).ok().map(|s| page::Resource::Stylesheet(s, absolute))
                    }
                    page::ResourceKind::Image => {
                        let Ok(mut url) = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href) else { return Ok(None) };
                        url.set_fragment(None);
                        match loader.image(&document, url, page::PAGE_IMAGE_LIMITS.bytes.saturating_sub(image_bytes), &cancel, |_| {}, &svg) {
                            Ok(image) => {
                                image_bytes += image.payload_bytes();
                                Some(page::Resource::Image(image))
                            }
                            Err(_) => None,
                        }
                    }
                    page::ResourceKind::Font => None,
                    page::ResourceKind::Script => None,
                })
            };
            // Its scripts, run before it first renders (a realm of its own,
            // laid out for their measurements), then kept going: what they
            // build (an embed's player, a widget, a captcha) is what shows.
            let dark = key.palette.is_dark();
            let mut kept: Option<olo_js::Realm> = None;
            let mut run = |doc: olo_web::dom::Document, quirks: bool, measurer: page::Measurer| -> page::Result<olo_web::dom::Document> {
                let url = document.url.to_string();
                let scripts: Vec<olo_js::Script> = olo_js::page_scripts(&doc, &url)
                    .into_iter()
                    .filter_map(|(source, name, module, node)| match source {
                        olo_js::Source::Inline(text) => Some(olo_js::Script { source: text, name, module, node: Some(node) }),
                        olo_js::Source::External(u) => {
                            let at = url::Url::parse(&u).ok()?;
                            loader.script(&document, at, &cancel, |_| {}).ok().map(|text| olo_js::Script { source: Arc::from(text), name: u, module, node: Some(node) })
                        }
                    })
                    .collect();
                if scripts.is_empty() {
                    return Ok(doc);
                }
                let config = olo_js::Config {
                    url,
                    viewport: (f64::from(size[0]) / key.scale, f64::from(size[1]) / key.scale),
                    scale: key.scale,
                    quirks,
                    dark,
                    budget_ms: 100.,
                    wall_seconds: 3.,
                    framed: true,
                    ..Default::default()
                };
                let fetcher: olo_js::Fetcher = {
                    let (loader, cancel, from) = (loader.clone(), cancel.clone(), document.url.clone());
                    Box::new(move |url: &str, method: &str, body: &[u8], headers: &str| {
                        let url = url::Url::parse(url).map_err(|e| e.to_string())?;
                        if method == "GET" && headers == olo_js::SCRIPT_DESTINATION {
                            let (status, content_type, body, final_url, headers) = loader.script_response(&from, url, &cancel).map_err(|e| e.to_string())?;
                            return Ok(olo_js::Fetched { status, content_type, body, url: final_url, headers });
                        }
                        if method == "GET" && headers.is_empty() {
                            let (status, content_type, body, final_url, headers) = loader.text(&from, url, &cancel).map_err(|e| e.to_string())?;
                            return Ok(olo_js::Fetched { status, content_type, body: body.into_bytes(), url: final_url, headers });
                        }
                        let (status, content_type, body, final_url, headers) = loader.request(&from, url, method, headers, body, &cancel).map_err(|e| e.to_string())?;
                        Ok(olo_js::Fetched { status, content_type, body, url: final_url, headers })
                    })
                };
                let measure: olo_js::Measure = {
                    let host = PageHost::new(&scope, &key);
                    let mut shaper = shaping::Engine::default();
                    let mut measurer = measurer;
                    let cancel = cancel.clone();
                    let viewport = host.viewport;
                    Box::new(move |doc: &olo_web::dom::Document, pictures: &[(NodeId, Arc<Image>)], previous: Option<page::Measured>| {
                        measurer.bind_pictures(doc, pictures, Some(&cancel));
                        measurer.measure(doc, &host, &mut shaper, viewport, previous, Some(&cancel)).ok()
                    })
                };
                // (Pictures its scripts load come as the live page's do.)
                let lookup: olo_js::ImageLookup = Box::new(|_: &str| None);
                let mut realm = olo_js::Realm::new(doc, config, Some(fetcher), Some(measure))
                    .map_err(|e| page::Error::from(e.as_str()))?
                    .with_cookies(Some(crate::cookies::script_access()))
                    .with_image_lookup(Some(lookup));
                realm.run_page(&scripts);
                if olo_web::env_on!("OLO_BROWSER_TIMING") {
                    for e in &realm.report().errors {
                        eprintln!("  frame {}: script error: {}", document.url, e.lines().next().unwrap_or(""));
                    }
                }
                let snapshot = realm.snapshot();
                kept = Some(realm);
                Ok(snapshot)
            };
            let media = olo_web::css::media::Media::screen(host.viewport.width, host.viewport.height, key.scale, dark);
            let prepared = page::prepare_scripted(html, &scope, media, false, Some(&cancel), |_, _| {}, resource, |_, _| {}, Some(&mut run))?;
            drop(run);
            let mut frame = page::finish(prepared, &host, &mut shaper, &mut painter, host.viewport, Some(&cancel), |_, _| {})?;
            // Painted in tiles of at most 512 × 512 (the painter's limit),
            // put together.
            let picture = |painter: &mut paint::Engine, frame: &page::Page| -> page::Result<Arc<Image>> {
                const TILE: u32 = 512;
                let (w, h) = (size[0], size[1]);
                let mut rgba = vec![0u8; w as usize * h as usize * 4];
                for ty in (0..h).step_by(TILE as usize) {
                    for tx in (0..w).step_by(TILE as usize) {
                        let (tw, th) = (TILE.min(w - tx), TILE.min(h - ty));
                        let tile = painter.raster(&frame.scene, paint::DeviceRect { x: tx as i32, y: ty as i32, width: tw, height: th }, Some(&cancel))?.into_rgba();
                        for row in 0..th as usize {
                            let from = row * tw as usize * 4;
                            let to = ((ty as usize + row) * w as usize + tx as usize) * 4;
                            rgba[to..to + tw as usize * 4].copy_from_slice(&tile[from..from + tw as usize * 4]);
                        }
                    }
                }
                Image::from_premultiplied(w, h, rgba, key.scale, Default::default(), Some(&cancel)).map_err(|e| format!("frame picture: {e}").into())
            };
            publish(picture(&mut painter, &frame)?);
            if olo_web::env_on!("OLO_BROWSER_TIMING") {
                eprintln!("Browser frame {src}: rendered at {}x{}{}", size[0], size[1], if kept.is_some() { ", live" } else { "" });
            }
            let Some(mut realm) = kept else { return Ok(()) };
            // Live: its realm goes on, on this thread.
            let loads = Loads { loader: loader.clone(), document: document.light(), scope: scope.clone(), key: key.clone(), cancel: cancel.clone(), tx: ends.inbox.clone() };
            let started = Instant::now();
            let mut rendered = realm.revision();
            let mut drawn = Instant::now();
            let mut dirty = false;
            let (mut next_due, mut wants_frame) = (Some(0.), false);
            while !cancel.load(std::sync::atomic::Ordering::Relaxed) && realm.stopped().is_none() {
                loads.dispatch(&mut realm);
                let now = started.elapsed().as_secs_f64() * 1000.;
                // Out of view: no animation frames, no pictures; its timers
                // and messages go on.
                let in_sight = ends.shown.load(std::sync::atomic::Ordering::Relaxed);
                let framing = in_sight && (wants_frame || dirty);
                let until = if framing { 16. - now % 16. } else { next_due.map_or(1000., |d: f64| (d - now).clamp(0., 1000.)) };
                let mut take = |m: Message, realm: &mut olo_js::Realm| match m {
                    Message::Loaded(id, result) => realm.deliver(id, result),
                    Message::FrameMessage(data, origin) => realm.receive_message(None, &data, &origin),
                    Message::Input { input, .. } => {
                        let h = f64::from(size[1]) / key.scale;
                        let target = match input.kind {
                            olo_js::InputKind::Key { .. } | olo_js::InputKind::Scroll => None,
                            _ => frame.scene.hit_test(input.x, input.y, 0., h, &frame.scene.layout().sticky()),
                        };
                        realm.input(&input, target);
                    }
                    _ => {}
                };
                match ends.rx.recv_timeout(Duration::from_secs_f64(until / 1000.)) {
                    Ok(m) => take(m, &mut realm),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                while let Ok(m) = ends.rx.try_recv() {
                    take(m, &mut realm);
                }
                let now = started.elapsed().as_secs_f64() * 1000.;
                let in_sight = ends.shown.load(std::sync::atomic::Ordering::Relaxed);
                let advanced = realm.advance_frame(now, Duration::from_millis(30), Some(wants_frame && in_sight));
                (next_due, wants_frame) = (advanced.next_due_ms, advanced.wants_frame);
                if olo_web::env_on!("OLO_BROWSER_TIMING") {
                    for line in realm.take_console() {
                        eprintln!("  frame {}: {line}", document.url.host_str().unwrap_or(""));
                    }
                }
                let posts: Vec<_> = realm.take_posts().into_iter().filter(|(to, _, _)| to.is_none()).map(|(_, data, target)| (data, target)).collect();
                if !posts.is_empty() {
                    ends.outbox.lock().unwrap().extend(posts);
                    wake();
                }
                dirty |= advanced.changed || realm.revision() != rendered;
                if dirty && in_sight && drawn.elapsed() >= FRAME_REDRAW {
                    frame.rerender(realm.snapshot(), realm.canvases(), &host, &mut shaper, &mut painter, host.viewport, Some(&cancel), |_, _| {})?;
                    publish(picture(&mut painter, &frame)?);
                    rendered = realm.revision();
                    drawn = Instant::now();
                    dirty = false;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {}
            Err(e) => {
                if olo_web::env_on!("OLO_BROWSER_TIMING") {
                    eprintln!("Browser frame {src}: {e}");
                }
                let mut m = made.lock().unwrap();
                if matches!(*m, Made::Loading) {
                    *m = Made::Settled;
                }
                drop(m);
                wake();
            }
        }
    });
    if spawned.is_err() {
        *slot.lock().unwrap() = Made::Settled;
    }
}
