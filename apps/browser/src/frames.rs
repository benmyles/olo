//! The page's `<iframe>`s: each one's document loaded and rendered on a
//! thread of its own, at the size the iframe shows at, and bound to its
//! element as a picture (as a video's picture is). Its stylesheets and
//! images load as the page's do; its scripts don't run yet, and it is
//! not live (it doesn't take the person's input).
use crate::{
    network,
    worker::{Key, PageHost},
};
use olo_web::{dom::NodeId, font::Scope, image::Image, page, paint, text::shaping};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
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
        wake: &olo_js::Waker,
    ) -> (bool, bool) {
        if self.scanned != Some(revision)
            && let Some(doc) = doc()
        {
            self.scanned = Some(revision);
            self.found = iframes(&doc, base);
            if std::env::var_os("OLO_BROWSER_TIMING").is_some() && !self.found.is_empty() {
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
                start(src.clone(), size, made.clone(), cancel.clone(), loader.clone(), scope.clone(), key.clone(), wake.clone());
                self.frames.insert(node, Frame { src, size, made, cancel, image });
            }
            let f = self.frames.get_mut(&node).expect("inserted above");
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
/// The frame's document fetched, laid out and painted at `size` (device
/// pixels, at the page's scale and palette), on a thread of its own.
#[allow(clippy::too_many_arguments)]
fn start(src: String, size: [u32; 2], made: Arc<Mutex<Made>>, cancel: network::Cancel, loader: Arc<network::Loader>, scope: Scope, key: Key, wake: olo_js::Waker) {
    let slot = made.clone();
    let spawned = std::thread::Builder::new().name("browser-frame".into()).spawn(move || {
        let result = (|| -> page::Result<Arc<Image>> {
            let mut document = loader.document(&src, None, &cancel, |_| {})?;
            if document.status.is_some_and(|s| !(200..300).contains(&s)) {
                return Err("The frame's document returned an unsuccessful HTTP status".into());
            }
            let html = document.take_html(&cancel)?;
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
                })
            };
            // Its scripts, run once before it renders (a realm of its own,
            // its timers run for half a second of its own time): what they
            // build (an embed's player, a widget) is what shows.
            let dark = key.palette.is_dark();
            let mut run = |doc: olo_web::dom::Document, quirks: bool, _: page::Measurer| -> page::Result<olo_web::dom::Document> {
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
                    budget_ms: 500.,
                    wall_seconds: 3.,
                    ..Default::default()
                };
                let fetcher: olo_js::Fetcher = {
                    let (loader, cancel, from) = (loader.clone(), cancel.clone(), document.url.clone());
                    Box::new(move |url: &str, method: &str, body: &str, headers: &str| {
                        let url = url::Url::parse(url).map_err(|e| e.to_string())?;
                        if method == "GET" && headers.is_empty() {
                            let (status, content_type, body, final_url) = loader.text(&from, url, &cancel).map_err(|e| e.to_string())?;
                            return Ok(olo_js::Fetched { status, content_type, body: body.into_bytes(), url: final_url });
                        }
                        let (status, content_type, body, final_url) = loader.request(&from, url, method, headers, body.as_bytes(), &cancel).map_err(|e| e.to_string())?;
                        Ok(olo_js::Fetched { status, content_type, body, url: final_url })
                    })
                };
                let mut realm = olo_js::Realm::new(doc, config, Some(fetcher), None).map_err(|e| page::Error::from(e.as_str()))?;
                realm.run_page(&scripts);
                Ok(realm.snapshot())
            };
            let media = olo_web::css::media::Media::screen(host.viewport.width, host.viewport.height, key.scale, dark);
            let prepared = page::prepare_scripted(html, &scope, media, false, Some(&cancel), |_, _| {}, resource, |_, _| {}, Some(&mut run))?;
            let frame = page::finish(prepared, &host, &mut shaper, &mut painter, host.viewport, Some(&cancel), |_, _| {})?;
            let tile = painter.raster(&frame.scene, paint::DeviceRect { x: 0, y: 0, width: size[0], height: size[1] }, Some(&cancel))?;
            Image::from_premultiplied(size[0], size[1], tile.into_rgba(), key.scale, Default::default(), Some(&cancel)).map_err(|e| format!("frame picture: {e}").into())
        })();
        *made.lock().unwrap() = match result {
            Ok(image) => {
                if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                    eprintln!("Browser frame {src}: rendered at {}x{}", size[0], size[1]);
                }
                Made::Done(image)
            }
            Err(e) => {
                if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                    eprintln!("Browser frame {src}: {e}");
                }
                Made::Settled
            }
        };
        // The page looks at it now.
        wake();
    });
    if spawned.is_err() {
        *slot.lock().unwrap() = Made::Settled;
    }
}
