use olo_theme::Palette;
use olo_ui::{Proxy, gpui::RenderImage};
use olo_web::{
    css::{
        color::{self, Color, Scheme, Space, SystemColor},
        math,
        style::{self, Axes, FontMetrics, FontRequest, Host, Keyword},
    },
    font::{Builder, FontSet, Scope},
    layout, page, paint,
    text::shaping,
};
use std::{
    cell::Cell,
    collections::HashMap,
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};
const ABOUT: &str = "<!doctype html><title>Olo Browser</title><style>body{max-width:680px;margin:24px auto;padding:0 20px}h1{font-size:32px;line-height:40px}p{line-height:24px}.note{padding:16px;border-width:1px;border-style:solid;border-color:GrayText}</style><h1>Olo Browser</h1><p>A native window over Olo's original Rust page engine.</p><div class=note><p>Enter a website URL or a local HTML file path above.</p><p>This development build renders static documents. HTTP/HTTPS navigation is available. JavaScript, complete web layout and browser security isolation are still being built.</p></div>";
#[derive(Clone, PartialEq)]
pub struct Key {
    pub source: String,
    pub cwd: Option<std::path::PathBuf>,
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub palette: Arc<Palette>,
}
// Keep aligned tile ends and viewport additions inside signed device coordinates.
const MAX_VIEWPORT: u32 = 32768;
/// Whether pages' scripts run (`OLO_BROWSER_NO_JS=1` turns them off, for
/// comparing a page with and without).
const JAVASCRIPT: bool = true;
pub fn max_scroll(key: &Key, extent: f64) -> i32 {
    ((extent * key.scale - f64::from(key.height)).ceil().max(0.) as i32).min(i32::MAX - MAX_VIEWPORT as i32 - 256)
}
pub struct Request {
    pub id: u64,
    pub key: Key,
    pub scroll: i32,
    pub cancel: crate::network::Cancel,
    pub recycle: Vec<Arc<RenderImage>>,
    pub interaction: page::Interaction,
    pub navigation: u64,
    pub fragment: Option<olo_ui::gpui::SharedString>,
    pub seek_anchor: bool,
    pub focus_scroll: Option<olo_web::dom::NodeId>,
}
pub enum Message {
    Render(Box<Request>),
    Recycle(Vec<Arc<RenderImage>>),
    RecycleTargets(Arc<crate::navigation::FocusTargets>),
}
pub struct Label {
    pub image: bool,
    pub text: olo_ui::gpui::SharedString,
    pub rect: layout::Rect,
    pub node: Option<olo_web::dom::NodeId>,
    pub link: Option<Arc<crate::links::Link>>,
}
pub enum Reply {
    Status(u64, String),
    Location(u64, String),
    /// `fallbacks` lists what the engine substituted on this page (empty when
    /// it rendered everything it was asked to), for the status line and agents.
    Ready {
        id: u64,
        title: String,
        fallbacks: String,
        tiles: Vec<(paint::DeviceRect, Arc<RenderImage>)>,
    },
    Content {
        id: u64,
        labels: Vec<Label>,
        extent: f64,
        scroll: i32,
        interaction_mask: u64,
        document: u64,
        target: Option<olo_web::dom::NodeId>,
        focus_targets: Arc<crate::navigation::FocusTargets>,
    },
    Batch(u64, Vec<(paint::DeviceRect, Arc<RenderImage>)>),
    Error(u64, String),
}
struct Cached {
    key: Key,
    page: page::Page,
    viewport_dependent: bool,
    origin: url::Url,
    base: url::Url,
    links: HashMap<olo_web::dom::NodeId, Option<Arc<crate::links::Link>>>,
    navigation: u64,
    focus_targets: Option<Arc<crate::navigation::FocusTargets>>,
    anchor_scroll: Option<f64>,
}
pub fn start(proxy: Proxy) -> mpsc::Sender<Message> {
    let (tx, rx) = mpsc::channel::<Message>();
    std::thread::Builder::new()
        .name("browser-page".into())
        .spawn(move || {
            let run = || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                // Olo's own fonts first (the sans family with its italic, the
                // mono family, the symbol and maths faces), then the system's,
                // mapped, with Chrome's family policy so pages take the fonts
                // Chrome would on this machine.
                let fonts = olo_xdg::data_dirs().iter().map(|p| p.join("fonts")).find(|p| p.join("NotoSans[wght].ttf").is_file()).ok_or("Browser font is unavailable")?;
                let mut builder = Builder::new(Default::default());
                for file in ["NotoSans[wght].ttf", "NotoSans-Italic[wght].ttf", "NotoSansMono[wght].ttf", "NotoSansSymbols[wght].ttf", "NotoSansSymbols2-Regular.ttf", "NotoSansMath-Regular.ttf"] {
                    let path = fonts.join(file);
                    if path.is_file() {
                        builder.load_file(&path, |_| {}, None)?;
                    }
                }
                let system = olo_web::font::system::directories().into_iter().filter(|d| *d != fonts).collect::<Vec<_>>();
                let mapped = olo_web::font::system::map_directories(&mut builder, &system, |path| {
                    let file = std::fs::File::open(path).ok()?;
                    if file.metadata().ok()?.len() > 64 << 20 {
                        return None;
                    }
                    // SAFETY: a read-only mapping of an installed font file; the
                    // engine treats the bytes as untrusted and bounds-checks every
                    // table read, so a file rewritten underneath fails closed.
                    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
                    Some(std::sync::Arc::new(map) as std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>)
                });
                eprintln!("Browser fonts: {mapped} system font files mapped");
                let policy = olo_web::font::system::chrome_policy()?;
                let scope = FontSet::new(builder.finish(None)?, policy, Default::default())?.scope("en")?;
                let loader = Arc::new(crate::network::Loader::new()?);
                let mut shaper = shaping::Engine::default();
                let mut painter = paint::Engine::default();
                let mut cached: Option<Cached> = None;
                let mut tiles = HashMap::new();
                let mut clock = 0u64;
                while let Ok(message) = rx.recv() {
                    let mut r = match message {
                        Message::Render(r) => r,
                        Message::Recycle(images) => {
                            drop(images);
                            continue;
                        }
                        Message::RecycleTargets(targets) => {
                            drop(targets);
                            continue;
                        }
                    };
                    while let Ok(message) = rx.try_recv() {
                        match message {
                            Message::Render(new) => r = new,
                            Message::Recycle(images) => drop(images),
                            Message::RecycleTargets(targets) => drop(targets),
                        }
                    }
                    drop(std::mem::take(&mut r.recycle));
                    if r.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let result = render(&r, &loader, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock);
                    if let Err(error) = result
                        && !r.cancel.load(Ordering::Relaxed)
                    {
                        eprintln!("Browser: {error}");
                        proxy.send(Reply::Error(r.id, error.to_string()));
                    }
                }
                Ok(())
            };
            if let Err(e) = run() {
                proxy.send(Reply::Error(0, e.to_string()));
            }
        })
        .expect("browser worker");
    tx
}
#[allow(clippy::too_many_arguments)] // Persistent worker resources and explicit request/output channel.
fn render(
    r: &Request,
    loader: &Arc<crate::network::Loader>,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    cached: &mut Option<Cached>,
    proxy: &Proxy,
    cache: &mut HashMap<(i32, i32), (Arc<RenderImage>, u64)>,
    clock: &mut u64,
) -> page::Result<()> {
    if r.key.width > MAX_VIEWPORT || r.key.height > MAX_VIEWPORT || !r.key.scale.is_finite() || r.key.scale <= 0. {
        return Err("Viewport exceeds the development renderer limits".into());
    }
    let newly_loaded = cached.as_ref().is_none_or(|c| c.key.source != r.key.source || c.key.cwd != r.key.cwd || c.key.revision != r.key.revision);
    if newly_loaded {
        *cached = None;
        painter.clear_image_cache();
        cache.clear();
        proxy.send(Reply::Status(r.id, "Loading…".into()));
        let mut document = if r.key.source == "about:engine" {
            None
        } else {
            Some(loader.document(&r.key.source, r.key.cwd.as_deref(), &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "document"))?)
        };
        if let Some(document) = &document {
            proxy.send(Reply::Location(r.id, document.url.to_string()));
        }
        let source = match &mut document {
            Some(document) => document.take_html(&r.cancel)?,
            None => Arc::from(ABOUT),
        };
        let mut last = Instant::now();
        let mut phase = None;
        let mut resources = 0usize;
        let mut images: HashMap<url::Url, Option<Arc<olo_web::image::Image>>> = HashMap::new();
        let mut image_bytes = 0usize;
        let viewport = layout::Viewport { width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale, scale: r.key.scale };
        let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, r.key.palette.is_dark());
        let mut report = |p: page::Phase, n: usize| {
            if phase != Some(p) || last.elapsed() > Duration::from_millis(120) {
                phase = Some(p);
                last = Instant::now();
                proxy.send(Reply::Status(
                    r.id,
                    format!(
                        "{}… {n}",
                        match p {
                            page::Phase::Html => "Parsing",
                            page::Phase::Scripts => "Running scripts",
                            page::Phase::Styles => "Styling",
                            page::Phase::Boxes => "Building boxes",
                            page::Phase::Text => "Preparing text",
                            page::Phase::Layout => "Laying out",
                            page::Phase::Paint => "Painting",
                        }
                    ),
                ));
            }
        };
        // Scripts run against the parsed document before its resources load;
        // what they add is then fetched and styled like the rest.
        let mut run_scripts = |doc: olo_web::dom::Document, quirks: bool| -> page::Result<olo_web::dom::Document> {
            let Some(document) = &document else {
                return Ok(doc);
            };
            let page_url = document.url.to_string();
            let scripts: Vec<olo_js::Script> = olo_js::page_scripts(&doc, &page_url)
                .into_iter()
                .filter_map(|(source, name)| match source {
                    olo_js::Source::Inline(text) => Some(olo_js::Script { source: text, name }),
                    olo_js::Source::External(u) => {
                        proxy.send(Reply::Status(r.id, "Loading script…".into()));
                        let url = url::Url::parse(&u).ok()?;
                        match loader.script(document, url, &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "script")) {
                            Ok(text) => Some(olo_js::Script { source: Arc::from(text), name: u }),
                            Err(error) => {
                                eprintln!("Browser script {u}: {error}");
                                None
                            }
                        }
                    }
                })
                .collect();
            if scripts.is_empty() {
                return Ok(doc);
            }
            proxy.send(Reply::Status(r.id, format!("Running {} scripts…", scripts.len())));
            let config = olo_js::Config { url: page_url, viewport: (f64::from(r.key.width) / r.key.scale, f64::from(r.key.height) / r.key.scale), scale: r.key.scale, quirks, ..Default::default() };
            let fetch_document = document.url.clone();
            let fetcher: olo_js::Fetcher = {
                let cancel = r.cancel.clone();
                let loader = loader.clone();
                Box::new(move |url: &str, method: &str, _body: &str| {
                    if method != "GET" {
                        return Err(format!("{method} requests are not supported yet"));
                    }
                    let url = url::Url::parse(url).map_err(|e| e.to_string())?;
                    let (status, content_type, body, final_url) = loader.text(&fetch_document, url, &cancel).map_err(|e| e.to_string())?;
                    Ok(olo_js::Fetched { status, content_type, body: body.into_bytes(), url: final_url })
                })
            };
            let started = Instant::now();
            let mut realm = olo_js::Realm::new(doc, config, Some(fetcher)).map_err(|e| page::Error::from(e.as_str()))?;
            let report = realm.run_page(&scripts);
            let (doc, _) = realm.finish();
            eprintln!(
                "Browser scripts: {} run, {} errors, {} console lines, {} timers, {} listeners, revision {} -> {}, {:.0} ms",
                report.scripts,
                report.errors.len(),
                report.console.len(),
                report.timers,
                report.listeners,
                report.revision_before,
                report.revision_after,
                started.elapsed().as_secs_f64() * 1000.
            );
            for line in report.console.iter().take(50) {
                eprintln!("  console: {line}");
            }
            for error in report.errors.iter().take(50) {
                eprintln!("  error: {error}");
            }
            // The document as the scripts left it, for comparing with Chrome's.
            if let Ok(path) = std::env::var("OLO_BROWSER_DOM") {
                let mut text = String::new();
                let mut budget = 64 << 20;
                if olo_js::html::serialize(&doc, doc.root(), &mut text, &mut budget).is_ok() {
                    let _ = std::fs::write(path, text);
                }
            }
            Ok(doc)
        };
        let prepared = page::prepare_scripted(
            source,
            scope,
            media,
            true,
            Some(&r.cancel),
            &mut report,
            |kind, base, href| {
                let Some(document) = &document else {
                    return Ok(None);
                };
                if kind == page::ResourceKind::Font {
                    let mut url = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href)?;
                    url.set_fragment(None);
                    resources += 1;
                    if resources > 1024 {
                        return Err("Page exceeds the development resource request limit".into());
                    }
                    proxy.send(Reply::Status(r.id, "Loading font…".into()));
                    return match loader.font(document, url, &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "font")) {
                        Ok(bytes) => Ok(Some(page::Resource::Font(bytes))),
                        Err(error) if r.cancel.load(Ordering::Relaxed) => Err(error),
                        Err(error) => {
                            eprintln!("Browser font: {error}");
                            Ok(None)
                        }
                    };
                }
                if kind == page::ResourceKind::Image {
                    let mut url = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href)?;
                    url.set_fragment(None);
                    if let Some(image) = images.get(&url) {
                        return Ok(image.as_ref().map(|image| page::Resource::Image(image.clone())));
                    }
                    resources += 1;
                    if resources > 1024 {
                        return Err("Page exceeds the development resource request limit".into());
                    }
                    proxy.send(Reply::Status(r.id, "Loading image…".into()));
                    let host = PageHost::new(scope, &r.key);
                    let svg = |bytes: &[u8]| olo_web::svg_image::rasterize(bytes, &host, scope, r.key.scale, Some(&r.cancel)).map_err(|e| format!("SVG image: {e}").into());
                    return match loader.image(
                        document,
                        url.clone(),
                        (64usize << 20).saturating_sub(image_bytes),
                        &r.cancel,
                        crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "image"),
                        &svg,
                    ) {
                        Ok(image) => {
                            image_bytes += image.payload_bytes();
                            images.insert(url, Some(image.clone()));
                            Ok(Some(page::Resource::Image(image)))
                        }
                        Err(error) if r.cancel.load(Ordering::Relaxed) => Err(error),
                        Err(error) => {
                            eprintln!("Browser image: {error}");
                            images.insert(url, None);
                            Ok(None)
                        }
                    };
                }
                resources += 1;
                if resources > 1024 {
                    return Err("Page exceeds the development resource request limit".into());
                }
                proxy.send(Reply::Status(r.id, "Loading stylesheet…".into()));
                let absolute = base.and_then(|b| document.url.join(b).ok()).unwrap_or_else(|| document.url.clone()).join(href).ok().map(|u| Arc::<str>::from(u.as_str()));
                match loader.stylesheet(document, base, href, &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "stylesheet")) {
                    Ok(sheet) => Ok(Some(page::Resource::Stylesheet(sheet, absolute))),
                    Err(error) if r.cancel.load(Ordering::Relaxed) => Err(error),
                    Err(error) => {
                        eprintln!("Browser stylesheet: {error}");
                        Ok(None)
                    }
                }
            },
            if JAVASCRIPT { Some(&mut run_scripts) } else { None },
        )?;
        // The host is made for the page's own fonts (its web faces included).
        let host = PageHost::new(prepared.scope(), &r.key);
        let page = page::finish(prepared, &host, shaper, painter, viewport, Some(&r.cancel), &mut report)?;
        let origin = document.as_ref().map(|d| d.url.clone()).unwrap_or(url::Url::parse("about:engine")?);
        let base = page.base_href.as_deref().and_then(|href| origin.join(href).ok()).unwrap_or_else(|| origin.clone());
        *cached = Some(Cached { key: r.key.clone(), page, viewport_dependent: host.viewport_used.get(), origin, base, links: HashMap::new(), navigation: 0, focus_targets: None, anchor_scroll: None });
        cache.clear();
    }
    let c = cached.as_mut().unwrap();
    let navigation_changed = c.navigation != r.navigation;
    let mut input = r.interaction;
    input.target = c.page.interaction.target;
    let mut scene_changed = newly_loaded;
    let mut sought = if navigation_changed { None } else { c.anchor_scroll };
    if navigation_changed {
        let fragment = if newly_loaded { c.origin.fragment().map(str::to_owned) } else { r.fragment.as_ref().map(ToString::to_string) };
        input.target = crate::navigation::fragment(&c.page.document, fragment.as_deref(), &r.cancel)?;
        c.origin.set_fragment(fragment.as_deref());
        c.base = c.page.base_href.as_deref().and_then(|base| c.origin.join(base).ok()).unwrap_or_else(|| c.origin.clone());
        c.links.clear();
        c.focus_targets = None;
        if r.seek_anchor
            && (fragment.as_deref() == Some("")
                || input.target.is_none() && fragment.as_deref().is_some_and(|f| percent_encoding::percent_decode_str(f).decode_utf8_lossy().eq_ignore_ascii_case("top")))
        {
            sought = Some(0.);
        }
    }
    if let Some(Cached { key, page, viewport_dependent, .. }) = cached.as_mut()
        && key.source == r.key.source
        && key.revision == r.key.revision
        && key.cwd == r.key.cwd
        && (key.width != r.key.width || key.height != r.key.height || key.scale != r.key.scale || key.palette != r.key.palette || page.interaction.changes_style(input, page.interaction_mask))
    {
        proxy.send(Reply::Status(r.id, "Laying out…".into()));
        if *viewport_dependent || page.media_dependent || key.scale != r.key.scale || key.palette != r.key.palette || page.interaction.changes_style(input, page.interaction_mask) {
            let previous = page.interaction;
            page.interaction = input;
            let host = PageHost::new(page.scope(), &r.key);
            if let Err(error) = page.restyle(&host, shaper, painter, host.viewport, Some(&r.cancel), |_, _| {}) {
                page.interaction = previous;
                return Err(error);
            }
            *viewport_dependent = host.viewport_used.get();
        } else {
            let page_scope = page.scope().clone();
            let mut job = layout::Job::new(
                shaper,
                &page_scope,
                page.scene.layout().cache().clone(),
                layout::Viewport { width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale, scale: r.key.scale },
                Default::default(),
                Some(&r.cancel),
            )?;
            while !job.step(64)?.finished {}
            let layout = Arc::new(job.finish()?);
            let mut job = paint::BuildJob::new(&page.document, layout, painter, paint::Options { canvas: [255, 255, 255, 255] }, Default::default(), Some(&r.cancel))?;
            while !job.step(32)?.finished {}
            page.scene = job.finish()?;
        }
        *key = r.key.clone();
        cache.clear();
        scene_changed = true;
    }
    let cached = cached.as_mut().unwrap();
    cached.page.interaction = input;
    if navigation_changed || scene_changed && r.seek_anchor {
        if r.seek_anchor
            && let Some(target) = input.target
            && let Some(rect) = crate::navigation::target_rect(&cached.page, target, &r.cancel)?
        {
            sought = Some(rect.y);
        }
        cached.anchor_scroll = sought;
        cached.navigation = r.navigation;
    }
    // Keep an unacknowledged anchor move across cancellation/replacement. The
    // UI clears seek_anchor only after it accepts Content for this request.
    sought = r.seek_anchor.then_some(cached.anchor_scroll).flatten();
    if let Some(target) = r.focus_scroll
        && let Some(rect) = crate::navigation::target_rect(&cached.page, target, &r.cancel)?
    {
        let top = f64::from(r.scroll) / r.key.scale;
        let height = f64::from(r.key.height) / r.key.scale;
        if rect.y < top || rect.y + rect.height > top + height {
            sought = Some(rect.y + rect.height / 2. - height / 2.);
        }
    }
    if scene_changed || cached.focus_targets.is_none() {
        cached.focus_targets = Some(crate::navigation::focus_targets(&cached.page, &cached.base, &cached.origin, &mut cached.links, &r.cancel)?);
    }
    let page = &cached.page;
    let scene = &page.scene;
    let extent = scene.content_extent().height;
    let scroll = sought.map_or(r.scroll, |y| (y * r.key.scale).round().max(0.) as i32).clamp(0, max_scroll(&r.key, extent));
    let visible = layout::Rect { x: 0., y: f64::from(scroll) / r.key.scale, width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale };
    let mut hits = Vec::new();
    scene.query(visible, &mut hits, Some(&r.cancel))?;
    let mut labels = Vec::new();
    for id in hits {
        let vector = matches!(scene.commands()[id as usize].primitive, paint::Primitive::Vector { .. });
        let image = vector || matches!(scene.commands()[id as usize].primitive, paint::Primitive::Image { .. });
        let node = scene.node(id);
        let text = if image {
            std::borrow::Cow::Borrowed(node.map(|n| page.document.attribute(n, if vector { "aria-label" } else { "alt" })).transpose()?.flatten().filter(|s| !s.is_empty()).unwrap_or(if vector {
                "Graphic"
            } else {
                "Image"
            }))
        } else {
            let Some(text) = scene.text_content(id).filter(|s| !s.is_empty()) else { continue };
            text
        };
        let Some(rect) = scene.visible_bounds(id) else {
            continue;
        };
        let link = match node {
            Some(node) => crate::links::resolve(&page.document, node, &cached.base, &cached.origin, &mut cached.links)?,
            None => None,
        };
        labels.push(Label { image, text: text.into_owned().into(), rect, node, link });
    }
    proxy.send(Reply::Content {
        id: r.id,
        labels,
        extent,
        scroll,
        interaction_mask: page.interaction_mask,
        document: page.document.id(),
        target: page.interaction.target,
        focus_targets: cached.focus_targets.as_ref().unwrap().clone(),
    });
    let mut tiles = Vec::new();
    let mut last = Instant::now();
    let top = scroll.div_euclid(256) * 256;
    for y in (top..scroll + r.key.height as i32).step_by(256) {
        for x in (0..r.key.width as i32).step_by(256) {
            if r.cancel.load(Ordering::Relaxed) {
                return Err(paint::Error::Cancelled.into());
            }
            *clock = clock.wrapping_add(1);
            let region = paint::DeviceRect { x, y, width: 256, height: 256 };
            let image = if let Some((image, stamp)) = cache.get_mut(&(x, y)) {
                *stamp = *clock;
                image.clone()
            } else {
                let tile = painter.raster(scene, region, Some(&r.cancel))?;
                let mut bytes = tile.into_rgba();
                for p in bytes.as_chunks_mut::<4>().0 {
                    p.swap(0, 2)
                }
                let image = olo_ui::widgets::image_view::from_bgra(256, 256, bytes).ok_or("Tile upload failed")?;
                if cache.len() >= 256 {
                    let key = *cache.iter().min_by_key(|(_, (_, stamp))| *stamp).unwrap().0;
                    cache.remove(&key);
                }
                cache.insert((x, y), (image.clone(), *clock));
                image
            };
            tiles.push((region, image));
            if last.elapsed() > Duration::from_millis(120) {
                proxy.send(Reply::Batch(r.id, std::mem::take(&mut tiles)));
                last = Instant::now();
            }
        }
    }
    proxy.send(Reply::Ready {
        id: r.id,
        title: {
            let title = if page.title.is_empty() { &r.key.source } else { &page.title };
            let mut short: String = title.chars().take(256).collect();
            if title.chars().nth(256).is_some() {
                short.push('…');
            }
            if page.scripts > 0 || page.stylesheet_issues > 0 {
                short.push_str(&format!(" · {} scripts · {} stylesheet issues", page.scripts, page.stylesheet_issues));
            }
            if !page.fallbacks.is_empty() {
                short.push_str(&format!(" · {} fallbacks", page.fallbacks.total()));
            }
            short
        },
        fallbacks: page.fallbacks.to_string(),
        tiles,
    });
    Ok(())
}
pub(crate) struct PageHost {
    scope: Scope,
    palette: Arc<Palette>,
    scale: f64,
    viewport: layout::Viewport,
    viewport_used: Cell<bool>,
}
impl PageHost {
    pub(crate) fn new(scope: &Scope, key: &Key) -> Self {
        Self {
            scope: scope.clone(),
            palette: key.palette.clone(),
            scale: key.scale,
            viewport: layout::Viewport { width: f64::from(key.width) / key.scale, height: f64::from(key.height) / key.scale, scale: key.scale },
            viewport_used: Cell::new(false),
        }
    }
}
impl Host for PageHost {
    fn environment(&self) -> style::Result<Option<Arc<olo_web::css::variables::Environment>>> {
        olo_web::css::variables::Environment::rectangular().map(Some).map_err(|error| style::Error::Property(olo_web::css::properties::Error::Variables(error)))
    }
    fn revision(&self) -> u64 {
        self.scope.revision() ^ self.scale.to_bits() ^ self.viewport.width.to_bits().rotate_left(13) ^ self.viewport.height.to_bits().rotate_left(29)
    }
    fn scheme(&self) -> Scheme {
        if self.palette.is_dark() { Scheme::Dark } else { Scheme::Light }
    }
    fn system_color(&self, c: SystemColor, _: Scheme) -> color::Result<Color> {
        // Pages render as other browsers render them: a white canvas with
        // the standard light system colours, whatever Olo's own palette is.
        // Olo's dark modes for pages (a page's own color-scheme, then an
        // automatic dark rendering) are separate work; the selection colours
        // are Olo's.
        let p = &self.palette;
        let v: [u8; 4] = match c {
            SystemColor::Canvas | SystemColor::Field => [255, 255, 255, 255],
            SystemColor::CanvasText | SystemColor::FieldText | SystemColor::ButtonText => [0, 0, 0, 255],
            SystemColor::ButtonFace => [239, 239, 239, 255],
            SystemColor::ButtonBorder => [118, 118, 118, 255],
            SystemColor::GrayText => [128, 128, 128, 255],
            SystemColor::LinkText => [0, 0, 238, 255],
            SystemColor::ActiveText => [255, 0, 0, 255],
            SystemColor::VisitedText => [85, 26, 139, 255],
            SystemColor::AccentColor => p.navigation_base,
            SystemColor::Highlight | SystemColor::SelectedItem => p.list_selected_background,
            SystemColor::HighlightText | SystemColor::SelectedItemText | SystemColor::AccentColorText => p.list_selected_item_text,
            _ => [0, 0, 0, 255],
        };
        Ok(Color::new(Space::Srgb, [f64::from(v[0]) / 255., f64::from(v[1]) / 255., f64::from(v[2]) / 255.], f64::from(v[3]) / 255., 0))
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.scope.metrics(r, None).map(|m| m.style_at(self.scale)).map_err(style::Error::Font)
    }
    fn font_context(&self) -> u64 {
        self.scope.context()
    }
    fn font_size(&self, k: Keyword, parent: f64) -> style::Result<f64> {
        Ok(match k {
            Keyword::XxSmall => 9.,
            Keyword::XSmall => 10.,
            Keyword::Small => 13.,
            Keyword::Medium => 16.,
            Keyword::Large => 18.,
            Keyword::XLarge => 24.,
            Keyword::XxLarge => 32.,
            Keyword::XxxLarge => 48.,
            Keyword::Smaller => parent / 1.2,
            Keyword::Larger => parent * 1.2,
            _ => return Err(style::Error::Invalid),
        })
    }
    fn device_scale(&self) -> f64 {
        self.scale
    }
    fn factor(&self, unit: math::RelativeUnit, axes: Option<Axes>) -> math::Result<f64> {
        use math::RelativeUnit::*;
        let (w, h) = (self.viewport.width, self.viewport.height);
        let vertical = axes.unwrap_or_default().writing_mode != style::WritingMode::HorizontalTb;
        let value = match unit {
            Vw | Svw | Lvw | Dvw => w,
            Vh | Svh | Lvh | Dvh => h,
            Vi | Svi | Lvi | Dvi => {
                if vertical {
                    h
                } else {
                    w
                }
            }
            Vb | Svb | Lvb | Dvb => {
                if vertical {
                    w
                } else {
                    h
                }
            }
            Vmin | Svmin | Lvmin | Dvmin => w.min(h),
            Vmax | Svmax | Lvmax | Dvmax => w.max(h),
            _ => return Err(math::Error::UnitContextUnavailable(unit)),
        };
        self.viewport_used.set(true);
        Ok(value / 100.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_restyle_reuses_dom_and_compiled_external_sheets() {
        let mut builder = Builder::new(Default::default());
        builder.load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/web/tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
        let scope = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
        let mut key = Key { source: "about:engine".into(), cwd: None, revision: 0, width: 1000, height: 600, scale: 2., palette: Arc::new(Palette::light()) };
        let host = PageHost::new(&scope, &key);
        let mut shaper = shaping::Engine::default();
        let mut painter = paint::Engine::default();
        let mut fetched = 0;
        let mut page = page::load_resources(
            Arc::from("<!doctype html><link rel=stylesheet href=sheet.css><div></div>"),
            &host,
            &scope,
            &mut shaper,
            &mut painter,
            host.viewport,
            None,
            |_, _| {},
            |_, _| {
                fetched += 1;
                Ok(Some(olo_web::css::Syntax::parse(Arc::from("body{margin:0}div{height:10vh;width:50vw;background-color:red}"), Default::default(), None)?))
            },
        )
        .unwrap();
        let red = |page: &page::Page| page.scene.commands().iter().find(|c| matches!(c.primitive, paint::Primitive::Fill { color: [255, 0, 0, 255] })).unwrap().bounds;
        assert_eq!((red(&page).width, red(&page).height), (250., 30.));
        assert!(host.viewport_used.get());
        let identity = page.document.id();
        key.width = 600;
        key.height = 400;
        let host = PageHost::new(&scope, &key);
        page.restyle(&host, &mut shaper, &mut painter, host.viewport, None, |_, _| {}).unwrap();
        assert_eq!((red(&page).width, red(&page).height), (150., 20.));
        assert_eq!(page.document.id(), identity);
        assert_eq!(fetched, 1);
        use math::RelativeUnit::*;
        let axes = Axes { writing_mode: style::WritingMode::VerticalRl, ..Default::default() };
        assert_eq!(host.factor(Vi, Some(axes)).unwrap(), 2.);
        assert_eq!(host.factor(Vb, Some(axes)).unwrap(), 3.);
        for unit in [Vw, Svw, Lvw, Dvw] {
            assert_eq!(host.factor(unit, None).unwrap(), 3.);
        }
        assert_eq!(host.factor(Vmin, None).unwrap(), 2.);
    }
    #[test]
    fn extreme_document_scroll_still_leaves_room_for_viewport_and_tile() {
        for height in [1, 256, MAX_VIEWPORT] {
            let key = Key { source: "about:engine".into(), cwd: None, revision: 0, width: 100, height, scale: 2., palette: Arc::new(Palette::light()) };
            let scroll = max_scroll(&key, 1e300);
            assert!(scroll.checked_add(height as i32).and_then(|end| end.checked_add(256)).is_some());
            assert_eq!(max_scroll(&key, 0.), 0);
        }
    }
}
