use olo_theme::Palette;
use olo_ui::{Proxy, gpui::RenderImage};
use olo_web::{
    css::{
        color::{self, Color, Scheme, Space, SystemColor},
        math,
        style::{self, Axes, FontMetrics, FontRequest, Host, Keyword},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
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
pub fn max_scroll(key: &Key, extent: f64) -> i32 {
    ((extent * key.scale - f64::from(key.height)).ceil().max(0.) as i32).min(i32::MAX - MAX_VIEWPORT as i32 - 256)
}
pub struct Request {
    pub id: u64,
    pub key: Key,
    pub scroll: i32,
    pub cancel: crate::network::Cancel,
    pub recycle: Vec<Arc<RenderImage>>,
}
pub struct Label {
    pub text: String,
    pub rect: layout::Rect,
}
pub enum Reply {
    Status(u64, String),
    Location(u64, String),
    Ready { id: u64, title: String, tiles: Vec<(paint::DeviceRect, Arc<RenderImage>)> },
    Content { id: u64, labels: Vec<Label>, extent: f64, scroll: i32 },
    Batch(u64, Vec<(paint::DeviceRect, Arc<RenderImage>)>),
    Error(u64, String),
}
pub fn start(proxy: Proxy) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("browser-page".into())
        .spawn(move || {
            let run = || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                let path = olo_xdg::data_dirs().iter().map(|p| p.join("fonts/NotoSans[wght].ttf")).find(|p| p.is_file()).ok_or("Browser font is unavailable")?;
                let mut builder = Builder::new(Default::default());
                builder.load_file(&path, |_| {}, None)?;
                let scope = FontSet::new(builder.finish(None)?, GenericPolicy::new(&["Noto Sans"])?, Default::default())?.scope("en")?;
                let loader = crate::network::Loader::new()?;
                let mut shaper = shaping::Engine::default();
                let mut painter = paint::Engine::default();
                let mut cached: Option<(Key, page::Page, bool)> = None;
                let mut tiles = HashMap::new();
                let mut clock = 0u64;
                while let Ok(mut r) = rx.recv() {
                    while let Ok(new) = rx.try_recv() {
                        r = new
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
    loader: &crate::network::Loader,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    cached: &mut Option<(Key, page::Page, bool)>,
    proxy: &Proxy,
    cache: &mut HashMap<(i32, i32), (Arc<RenderImage>, u64)>,
    clock: &mut u64,
) -> page::Result<()> {
    if r.key.width > MAX_VIEWPORT || r.key.height > MAX_VIEWPORT || !r.key.scale.is_finite() || r.key.scale <= 0. {
        return Err("Viewport exceeds the development renderer limits".into());
    }
    if let Some((key, page, viewport_dependent)) = cached.as_mut()
        && key.source == r.key.source
        && key.revision == r.key.revision
        && key.cwd == r.key.cwd
        && (key.width != r.key.width || key.height != r.key.height || key.scale != r.key.scale || key.palette != r.key.palette)
    {
        proxy.send(Reply::Status(r.id, "Laying out…".into()));
        if *viewport_dependent || key.scale != r.key.scale || key.palette != r.key.palette {
            let host = PageHost::new(scope, &r.key);
            page.restyle(&host, scope, shaper, painter, host.viewport, Some(&r.cancel), |_, _| {})?;
            *viewport_dependent = host.viewport_used.get();
        } else {
            let mut job = layout::Job::new(
                shaper,
                scope,
                page.scene.layout().cache().clone(),
                layout::Viewport { width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale, scale: r.key.scale },
                Default::default(),
                Some(&r.cancel),
            )?;
            while !job.step(64)?.finished {}
            let layout = Arc::new(job.finish()?);
            let mut job = paint::BuildJob::new(&page.document, layout, painter, paint::Options { canvas: r.key.palette.document_background }, Default::default(), Some(&r.cancel))?;
            while !job.step(32)?.finished {}
            page.scene = job.finish()?;
        }
        *key = r.key.clone();
        cache.clear();
    }
    if cached.as_ref().is_none_or(|(key, _, _)| *key != r.key) {
        *cached = None;
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
        let host = PageHost::new(scope, &r.key);
        let mut last = Instant::now();
        let mut phase = None;
        let mut resources = 0usize;
        let page = page::load_resources(
            source,
            &host,
            scope,
            shaper,
            painter,
            layout::Viewport { width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale, scale: r.key.scale },
            Some(&r.cancel),
            |p, n| {
                if phase != Some(p) || last.elapsed() > Duration::from_millis(120) {
                    phase = Some(p);
                    last = Instant::now();
                    proxy.send(Reply::Status(
                        r.id,
                        format!(
                            "{}… {n}",
                            match p {
                                page::Phase::Html => "Parsing",
                                page::Phase::Styles => "Styling",
                                page::Phase::Boxes => "Building boxes",
                                page::Phase::Text => "Preparing text",
                                page::Phase::Layout => "Laying out",
                                page::Phase::Paint => "Painting",
                            }
                        ),
                    ));
                }
            },
            |base, href| {
                let Some(document) = &document else {
                    return Ok(None);
                };
                resources += 1;
                if resources > 128 {
                    return Err("Page exceeds the development stylesheet request limit".into());
                }
                proxy.send(Reply::Status(r.id, "Loading stylesheet…".into()));
                match loader.stylesheet(document, base, href, &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "stylesheet")) {
                    Ok(sheet) => Ok(Some(sheet)),
                    Err(error) if r.cancel.load(Ordering::Relaxed) => Err(error),
                    Err(error) => {
                        eprintln!("Browser stylesheet: {error}");
                        Ok(None)
                    }
                }
            },
        )?;
        *cached = Some((r.key.clone(), page, host.viewport_used.get()));
        cache.clear();
    }
    let page = &cached.as_ref().unwrap().1;
    let scene = &page.scene;
    let extent = scene.content_extent().height;
    let scroll = r.scroll.clamp(0, max_scroll(&r.key, extent));
    let visible = layout::Rect { x: 0., y: f64::from(scroll) / r.key.scale, width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale };
    let mut hits = Vec::new();
    scene.query(visible, &mut hits, Some(&r.cancel))?;
    let mut labels = Vec::new();
    for id in hits {
        let c = &scene.commands()[id as usize];
        if let paint::Primitive::Text { owner, fragment, glyphs, .. } = &c.primitive {
            let t = scene.layout().text_layout(*owner).unwrap();
            let f = &t.fragments()[*fragment as usize];
            let run = t.run(f).unwrap();
            let gs = &run.glyphs[glyphs.start as usize..glyphs.end as usize];
            if let (Some(lo), Some(hi)) = (gs.iter().map(|g| g.cluster).min(), gs.iter().map(|g| g.cluster).max()) {
                let end = run.glyphs.iter().filter_map(|g| (g.cluster > hi).then_some(g.cluster)).min().unwrap_or(f.source.end).min(f.source.end);
                if lo < end {
                    labels.push(Label { text: run.source[lo as usize..end as usize].to_owned(), rect: c.bounds });
                }
            }
        }
    }
    proxy.send(Reply::Content { id: r.id, labels, extent, scroll });
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
                short.push_str(&format!(" · {} inert scripts · {} stylesheet issues", page.scripts, page.stylesheet_issues));
            }
            short
        },
        tiles,
    });
    Ok(())
}
struct PageHost {
    scope: Scope,
    palette: Arc<Palette>,
    scale: f64,
    viewport: layout::Viewport,
    viewport_used: Cell<bool>,
}
impl PageHost {
    fn new(scope: &Scope, key: &Key) -> Self {
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
    fn revision(&self) -> u64 {
        self.scope.revision() ^ self.scale.to_bits() ^ self.viewport.width.to_bits().rotate_left(13) ^ self.viewport.height.to_bits().rotate_left(29)
    }
    fn scheme(&self) -> Scheme {
        if self.palette.is_dark() { Scheme::Dark } else { Scheme::Light }
    }
    fn system_color(&self, c: SystemColor, _: Scheme) -> color::Result<Color> {
        let p = &self.palette;
        let v = match c {
            SystemColor::Canvas => p.document_background,
            SystemColor::CanvasText => p.document_text,
            SystemColor::Field | SystemColor::ButtonFace => p.control_background,
            SystemColor::FieldText | SystemColor::ButtonText => p.control_text,
            SystemColor::ButtonBorder => p.control_border,
            SystemColor::GrayText => olo_theme::mix(p.document_text, p.document_background, 0.55),
            SystemColor::LinkText | SystemColor::ActiveText | SystemColor::AccentColor => p.navigation_base,
            SystemColor::VisitedText => p.navigation_pulse,
            SystemColor::Highlight | SystemColor::SelectedItem => p.list_selected_background,
            SystemColor::HighlightText | SystemColor::SelectedItemText | SystemColor::AccentColorText => p.list_selected_item_text,
            _ => p.document_text,
        };
        Ok(Color::new(Space::Srgb, [f64::from(v[0]) / 255., f64::from(v[1]) / 255., f64::from(v[2]) / 255.], f64::from(v[3]) / 255., 0))
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.scope.metrics(r, None).map(|m| m.style).map_err(|_| style::Error::ContextUnavailable)
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
            _ => return Err(math::Error::ContextUnavailable),
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
        page.restyle(&host, &scope, &mut shaper, &mut painter, host.viewport, None, |_, _| {}).unwrap();
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
