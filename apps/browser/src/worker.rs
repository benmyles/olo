pub use olo_js::{Input, InputKind};
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
#[derive(Clone)]
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
    /// What the live page's scripts asked for over the network, fetched
    /// (from the worker's own loads).
    Loaded(u64, Result<olo_js::Loaded, String>),
    /// A worker of the live page posted a message: advance it now.
    Wake,
    /// The window drew a frame and will draw another: the page's next frame
    /// is due now (frames paced by the display, see `Live::paced_at`).
    Frame,
    /// Input from the person, for the live page's scripts (document
    /// coordinates in CSS px).
    Input(olo_js::Input),
    Recycle(Vec<Arc<RenderImage>>),
    /// The window's shared textures for WebGL canvases (see
    /// `Reply::SharedWanted`): per canvas, (key, memory, its size, width,
    /// height), none when the window can't share with the canvas's GPU.
    SharedTextures(Vec<(olo_web::dom::NodeId, Vec<SharedHandle>)>),
    RecycleTargets(Arc<crate::navigation::FocusTargets>),
    /// The wheel over a scroll container (see `ScrollArea`): scroll it by
    /// (dx, dy) CSS px.
    ScrollBox {
        node: olo_web::dom::NodeId,
        dx: f64,
        dy: f64,
    },
}
/// A scroll container on the page, for the window to send the wheel over
/// it there (see `Message::ScrollBox`): its padding box (CSS px, document
/// coordinates, or the viewport's when `fixed`), its offset, the farthest
/// it scrolls and which axes a person may scroll.
#[derive(Clone, Copy, Debug)]
pub struct ScrollArea {
    pub node: olo_web::dom::NodeId,
    pub rect: layout::Rect,
    pub fixed: bool,
    pub offset: [f64; 2],
    pub max: [f64; 2],
    pub user: [bool; 2],
}
impl ScrollArea {
    /// Whether it can take a scroll by (dx, dy): an axis it scrolls on with
    /// room left that way.
    pub fn takes(&self, dx: f64, dy: f64) -> bool {
        let room = |i: usize, d: f64| self.user[i] && self.max[i] > 0. && (d > 0. && self.offset[i] < self.max[i] - 0.5 || d < 0. && self.offset[i] > 0.5);
        room(0, dx) || room(1, dy)
    }
}
#[derive(Clone)]
pub struct Label {
    pub image: bool,
    pub text: olo_ui::gpui::SharedString,
    pub rect: layout::Rect,
    pub node: Option<olo_web::dom::NodeId>,
    pub link: Option<Arc<crate::links::Link>>,
    /// Fixed to the viewport: `rect` is viewport coordinates.
    pub fixed: bool,
    /// Under a sticky box: that box, whose shift at the current scroll
    /// applies to `rect`.
    pub sticky: Option<layout::Sticky>,
}
/// Where a layer's tiles are shown: scrolled with the page, fixed to the
/// viewport, or shifted as a sticky box's scroll has it.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    Page,
    Fixed,
    Sticky(layout::Sticky),
    /// A moved layer: its tiles in the layer's own space, shown moved by
    /// (dx, dy) device pixels with the page and within its ancestors' clip
    /// (document device pixels).
    Moved {
        dx: i32,
        dy: i32,
        clip: Option<paint::DeviceRect>,
    },
}
/// One of the page's layers above its base: tiles (device coordinates in
/// the layer's own space) and how they are placed.
/// A shared texture handed to the page: key, memory, memory size, width, height.
pub type SharedHandle = (u64, std::os::fd::OwnedFd, u64, u32, u32);
pub struct LayerTiles {
    /// A canvas frame in a shared texture (where, its key, whether its
    /// colours are premultiplied) instead of tiles.
    pub shared: Option<(paint::DeviceRect, u64, bool)>,
    pub placement: Placement,
    pub tiles: Vec<(paint::DeviceRect, Arc<RenderImage>)>,
    /// The images are drawn at their rectangles' sizes, not texel for
    /// pixel (a canvas's backing store at another density): sampled
    /// linearly, on the GPU.
    pub scaled: bool,
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
        /// The page wants more frames (animations, a canvas drawing): the
        /// window asks for them as it draws its own.
        animating: bool,
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
        scroll_areas: Arc<[ScrollArea]>,
    },
    Batch(u64, Vec<(paint::DeviceRect, Arc<RenderImage>)>),
    /// The page's layers above its base tiles, in paint order (later page
    /// runs over fixed and sticky layers before them); empty when the
    /// base is all there is.
    Layers(u64, Vec<LayerTiles>),
    /// Animating WebGL canvases that would draw into textures of the
    /// window's renderer: each with its size and its GPU's UUID.
    SharedWanted(u64, Vec<(olo_web::dom::NodeId, u32, u32, [u8; 16])>),
    /// Shared textures the page let go of, for the window to free.
    DropShared(Vec<u64>),
    /// The page's scripts (or a form it submitted) sent it to `url`.
    Navigate {
        id: u64,
        url: String,
        replace: bool,
    },
    Error(u64, String),
}
/// Which pass of a page: the static page first, shown at once, then the
/// page as its scripts leave it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Static,
    Scripted,
    /// The page's clock advanced, and the page rendered again if its
    /// scripts changed it.
    Live,
}
/// A page whose scripts keep running: its realm, when its next timer is
/// due, and what has been rendered.
struct Live {
    realm: olo_js::Realm,
    /// A render is wanted although the document is unchanged (a canvas's
    /// readback was in flight at the last one).
    render_again: bool,
    /// Cancelled when the page goes (its fetches and measurements stop).
    cancel: crate::network::Cancel,
    /// Fetches what its scripts ask for, off this thread.
    loads: Loads,
    started: Instant,
    next_due_ms: Option<f64>,
    rendered: u64,
    last_render: Instant,
    last_eval: Option<String>,
    /// Since the last summary: advances, script time, renders, render time.
    stats: (usize, f64, usize, f64, Instant),
    /// Since the last summary: the page's ticks, and the most in one advance.
    ticks: usize,
    max_ticks: usize,
    /// The next display frame (the page's clock, ms).
    next_frame_ms: f64,
    /// The page waits for a frame, or changed since the last render: the
    /// worker wakes at the next frame.
    wants_frame: bool,
    render_due: bool,
    /// Canvases whose last frame is in a shared texture, with its key.
    shared_frames: Vec<(olo_web::dom::NodeId, u64)>,
    /// Display frames seen, and what the page's frames cost.
    frame_count: u64,
    usage: Usage,
    /// When the window last asked for a frame, and whether it asks now.
    paced_at: Option<Instant>,
    frame_requested: bool,
    /// The window's frame period (ms), measured between its frames: the
    /// page's own clock when a frame changed nothing (no render, so no
    /// window frame to ask for the next).
    display_period_ms: f64,
    /// The page was told its window is hidden.
    hidden: bool,
    /// Since when the window was told the page wants frames and sent none
    /// (cleared by a frame): past a quarter of a second, it is hidden.
    waiting_since: Option<Instant>,
}
/// What a live page's frames cost (its scripts and renders on the CPU, its
/// WebGL on the GPU), reviewed each second: over budget, it draws every
/// second, then fourth, then eighth display frame; well under again, back
/// to every frame. A page's animation slows rather than the machine.
struct Usage {
    cpu_ms: f64,
    gpu_ms: f64,
    frames: u32,
    since: Instant,
    divisor: u32,
    /// Seconds in a row over budget (a page's first, loading, second is
    /// not held against it).
    over: u32,
}
impl Usage {
    /// CPU a drawn frame may take (60% of a 60 Hz frame), and GPU (half).
    const CPU_MS: f64 = 10.;
    const GPU_MS: f64 = 8.;
    fn new() -> Self {
        Self { cpu_ms: 0., gpu_ms: 0., frames: 0, since: Instant::now(), divisor: 1, over: 0 }
    }
    /// After a second: the divisor changed, if it did.
    fn review(&mut self) -> Option<u32> {
        if self.since.elapsed() < Duration::from_secs(1) || self.frames == 0 {
            return None;
        }
        let (cpu, gpu) = (self.cpu_ms / f64::from(self.frames), self.gpu_ms / f64::from(self.frames));
        let before = self.divisor;
        let mut over = 0;
        if cpu > Self::CPU_MS || gpu > Self::GPU_MS {
            over = self.over + 1;
            if over >= 2 {
                self.divisor = (self.divisor * 2).min(8);
            }
        } else if cpu < Self::CPU_MS / 2. && gpu < Self::GPU_MS / 2. {
            self.divisor = (self.divisor / 2).max(1);
        }
        *self = Self { divisor: self.divisor, over, ..Self::new() };
        (self.divisor != before).then_some(self.divisor)
    }
}
/// RGBA (the engine's tiles and images, premultiplied) as BGRA, for the
/// kit's premultiplied pictures (`from_premultiplied_bgra`: the GPU makes
/// them straight; composited as straight, a translucent white was grey).
/// Four pixels a step: the red and blue bytes of each swapped in place.
fn to_bgra(bytes: &mut [u8]) {
    let (quads, rest) = bytes.as_chunks_mut::<16>();
    for q in quads {
        let mut v = u128::from_le_bytes(*q);
        let rb = v & 0x00ff00ff_00ff00ff_00ff00ff_00ff00ff;
        let ga = v & 0xff00ff00_ff00ff00_ff00ff00_ff00ff00;
        v = ga | ((rb << 16) & 0x00ff0000_00ff0000_00ff0000_00ff0000) | ((rb >> 16) & 0x000000ff_000000ff_000000ff_000000ff);
        *q = v.to_le_bytes();
    }
    for p in rest.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
}
/// The overlap of two rectangles, if any.
fn intersect(a: layout::Rect, b: layout::Rect) -> Option<layout::Rect> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.width).min(b.x + b.width), (a.y + a.height).min(b.y + b.height));
    (x1 > x0 && y1 > y0).then_some(layout::Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}
/// Whether an error only says the work was cancelled (a newer request
/// took over), as every phase reports it.
fn is_cancellation(error: &page::Error) -> bool {
    error.to_string().ends_with("Cancelled")
}
/// What fetching for the live page's scripts takes: the loads run on
/// their own threads and answer through the worker's channel.
#[derive(Clone)]
struct Loads {
    loader: Arc<crate::network::Loader>,
    document: crate::network::Resource,
    scope: Scope,
    key: Key,
    cancel: crate::network::Cancel,
    tx: mpsc::Sender<Message>,
}
impl Loads {
    /// Start every load the scripts asked for since the last time.
    fn dispatch(&self, realm: &mut olo_js::Realm) {
        for request in realm.take_requests() {
            let timing = std::env::var_os("OLO_BROWSER_TIMING").is_some();
            if timing {
                eprintln!("  requested: {}", request.url);
            }
            let this = self.clone();
            let spawned = std::thread::Builder::new().name("browser-load".into()).spawn(move || {
                let started = Instant::now();
                let result = url::Url::parse(&request.url).map_err(|e| e.to_string()).and_then(|url| match &request.kind {
                    olo_js::RequestKind::Image => {
                        let host = PageHost::new(&this.scope, &this.key);
                        let svg = |bytes: &[u8]| olo_web::svg_image::rasterize(bytes, &host, &this.scope, this.key.scale, Some(&this.cancel)).map_err(|e| format!("SVG image: {e}").into());
                        this.loader.image(&this.document, url, 64 << 20, &this.cancel, |_| {}, &svg).map(olo_js::Loaded::Image).map_err(|e| e.to_string())
                    }
                    olo_js::RequestKind::Text { method, .. } => {
                        if method != "GET" {
                            return Err(format!("{method} requests are not supported yet"));
                        }
                        this.loader
                            .bytes(&this.document.url, url, &this.cancel)
                            .map(|(status, content_type, body, url)| olo_js::Loaded::Text(olo_js::Fetched { status, content_type, body, url }))
                            .map_err(|e| e.to_string())
                    }
                });
                if timing {
                    let ms = started.elapsed().as_secs_f64() * 1000.;
                    match &result {
                        Ok(olo_js::Loaded::Image(i)) => eprintln!("  loaded: {} ({}x{}, {ms:.0} ms)", request.url, i.width(), i.height()),
                        Ok(olo_js::Loaded::Text(f)) => eprintln!("  loaded: {} ({} bytes, HTTP {}, {ms:.0} ms)", request.url, f.body.len(), f.status),
                        Err(e) => eprintln!("  load failed: {} ({e})", request.url),
                    }
                }
                let _ = this.tx.send(Message::Loaded(request.id, result));
            });
            if let Err(e) = spawned {
                let _ = self.tx.send(Message::Loaded(request.id, Err(format!("load thread: {e}"))));
            }
        }
    }
    /// What workers fetch with, on their threads.
    fn worker_fetcher(&self) -> olo_js::SharedFetcher {
        let this = self.clone();
        Arc::new(move |url: &str, method: &str, _body: &str| {
            if method != "GET" {
                return Err(format!("{method} requests are not supported yet"));
            }
            let url = url::Url::parse(url).map_err(|e| e.to_string())?;
            this.loader.bytes(&this.document.url, url, &this.cancel).map(|(status, content_type, body, url)| olo_js::Fetched { status, content_type, body, url }).map_err(|e| e.to_string())
        })
    }
    fn waker(&self) -> olo_js::Waker {
        let tx = self.tx.clone();
        Arc::new(move || {
            let _ = tx.send(Message::Wake);
        })
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.cancel.cancel();
        if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
            eprintln!("  live: the page's realm let go");
        }
    }
}
/// A page whose window stopped drawing (hidden, covered, minimised: it
/// asked for frames and none came for a quarter of a second) draws once a
/// second, as browsers pause a hidden tab's animation frames; the window
/// drawing again asks for frames, and the page is paced by them at once.
fn hidden(live: &Live) -> bool {
    live.waiting_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(250))
}
/// The page's own frame period (ms): a display frame, or a second while
/// its window is hidden.
fn frame_period_ms(live: &Live) -> f64 {
    if hidden(live) { 1000. } else { live.display_period_ms }
}
/// A live page's frame: its animation frames run and it renders once a
/// display frame (60 Hz), so animations are as smooth as the page draws
/// them (at 100 ms they showed at 10 frames a second and stuttered).
const FRAME: Duration = Duration::from_micros(16_667);
struct Cached {
    key: Key,
    page: page::Page,
    /// The fetched document and its HTML, for the scripted pass.
    document: Option<crate::network::Resource>,
    source: Arc<str>,
    /// Decoded images by URL, shared by the passes.
    images: HashMap<url::Url, Option<Arc<olo_web::image::Image>>>,
    scripted: bool,
    loaded_at: Instant,
    live: Option<Live>,
    viewport_dependent: bool,
    origin: url::Url,
    base: url::Url,
    links: HashMap<olo_web::dom::NodeId, Option<Arc<crate::links::Link>>>,
    /// The scene and document revision the focus targets were found in.
    targets_scene: Option<Arc<olo_web::paint::Scene>>,
    targets_revision: u64,
    /// The last labels: the scene they were made from, when, at which
    /// scroll and size.
    label_cache: Option<(Arc<olo_web::paint::Scene>, Instant, i32, (u32, u32), Vec<Label>)>,
    /// The last scene's paint identity (and size) and its tiles'
    /// signatures, by stratum and place.
    paint_identity: Option<(((usize, u64), u32, u32, u64))>,
    tile_signatures: HashMap<(usize, i32, i32), Option<u64>>,
    navigation: u64,
    focus_targets: Option<Arc<crate::navigation::FocusTargets>>,
    anchor_scroll: Option<f64>,
}
pub fn start(proxy: Proxy) -> mpsc::Sender<Message> {
    let (tx, rx) = mpsc::channel::<Message>();
    let own_tx = tx.clone();
    std::thread::Builder::new()
        .name("browser-page".into())
        .spawn(move || {
            let tx = own_tx;
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
                let mut pending: std::collections::VecDeque<Message> = Default::default();
                let mut last: Option<Request> = None;
                loop {
                    // The static page is on screen and its scripts have not run:
                    // run them now, unless something else is waiting.
                    if let Some(r) = &last
                        && JAVASCRIPT
                        && cached.as_ref().is_some_and(|c| c.page.scripts > 0 && !c.scripted)
                        && !r.cancel.load(Ordering::Relaxed)
                        && pending.is_empty()
                    {
                        match rx.try_recv() {
                            Ok(message) => pending.push_back(message),
                            Err(mpsc::TryRecvError::Disconnected) => break,
                            Err(mpsc::TryRecvError::Empty) => {
                                let result = render(r, &loader, &tx, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock, Pass::Scripted);
                                if let Err(error) = result
                                    && !r.cancel.load(Ordering::Relaxed)
                                {
                                    eprintln!("Browser scripts: {error}");
                                    if let Some(c) = cached.as_mut() {
                                        c.scripted = true;
                                    }
                                }
                            }
                        }
                    }
                    // A live page's next timer sets how long to wait for a
                    // message; nothing due, nothing to wait for.
                    // Until its next timer, or its next frame when it waits for
                    // one (animation frames, a change or a canvas to show).
                    let wait = cached.as_ref().and_then(|c| c.live.as_ref().map(|l| (c, l))).and_then(|(c, l)| {
                        let paced = l.paced_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
                        let frame = (!paced && (l.wants_frame || l.render_due || l.render_again || l.realm.canvases_dirty() || c.page.animating())).then_some(l.next_frame_ms).filter(|_| !hidden(l));
                        let due = match (l.next_due_ms, frame) {
                            (Some(a), Some(b)) => Some(a.min(b)),
                            (a, b) => a.or(b),
                        };
                        due.map(|due| Duration::from_secs_f64((due / 1000. - l.started.elapsed().as_secs_f64()).max(0.)))
                    });
                    let received = if let Some(m) = pending.pop_front() {
                        Some(m)
                    } else {
                        match wait {
                            Some(d) => match rx.recv_timeout(d) {
                                Ok(m) => Some(m),
                                Err(mpsc::RecvTimeoutError::Timeout) => None,
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            },
                            None => match rx.recv() {
                                Ok(m) => Some(m),
                                Err(_) => break,
                            },
                        }
                    };
                    let Some(message) = received else {
                        // The live page's clock: run what is due, render if changed.
                        if let Some(r) = &last
                            && !r.cancel.load(Ordering::Relaxed)
                            && let Err(error) = render(r, &loader, &tx, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock, Pass::Live)
                        {
                            // A newer request cancelled this render: the live
                            // page goes on under it. Anything else ends it.
                            if !r.cancel.load(Ordering::Relaxed) && !is_cancellation(&error) {
                                eprintln!("Browser live: {error}");
                                if let Some(c) = cached.as_mut() {
                                    c.live = None;
                                }
                            }
                        }
                        continue;
                    };
                    let mut r = match message {
                        Message::Render(r) => r,
                        Message::Loaded(id, result) => {
                            // The live page's scripts hear of it now; what
                            // they draw renders at once.
                            if let Some(c) = cached.as_mut()
                                && let Some(live) = c.live.as_mut()
                            {
                                live.realm.deliver(id, result);
                                live.loads.dispatch(&mut live.realm);
                                live.next_due_ms = Some(0.);
                            }
                            continue;
                        }
                        Message::Wake => {
                            if let Some(c) = cached.as_mut()
                                && let Some(live) = c.live.as_mut()
                            {
                                live.next_due_ms = Some(0.);
                            }
                            continue;
                        }
                        Message::Frame => {
                            if let Some(c) = cached.as_mut()
                                && let Some(live) = c.live.as_mut()
                            {
                                let now = Instant::now();
                                if let Some(last) = live.paced_at {
                                    let ms = (now - last).as_secs_f64() * 1000.;
                                    // Back-to-back frames (not a pause): the display's rate,
                                    // smoothed over a few frames.
                                    if (4.0..34.).contains(&ms) {
                                        live.display_period_ms = live.display_period_ms * 0.75 + ms * 0.25;
                                    }
                                }
                                live.paced_at = Some(now);
                                live.frame_requested = true;
                                live.waiting_since = None;
                                live.next_due_ms = Some(0.);
                            }
                            continue;
                        }
                        Message::Input(input) => {
                            // Pointer and keys reach the live page's scripts; a
                            // change they make renders at once.
                            if let Some(c) = cached.as_mut()
                                && let Some(live) = c.live.as_mut()
                            {
                                let target = match input.kind {
                                    olo_js::InputKind::Key { .. } | olo_js::InputKind::Scroll => None,
                                    _ => {
                                        let (scroll, height) = last.as_ref().map_or((0., f64::INFINITY), |r| (f64::from(r.scroll) / r.key.scale, f64::from(r.key.height) / r.key.scale));
                                        let stickies = c.page.scene.layout().sticky();
                                        c.page.scene.hit_test(input.x, input.y, scroll, height, &stickies)
                                    }
                                };
                                live.realm.input(&input, target);
                                live.loads.dispatch(&mut live.realm);
                                // A link, form or script sent the page elsewhere.
                                if let Some((url, replace)) = live.realm.take_navigation()
                                    && let Some(r) = &last
                                {
                                    proxy.send(Reply::Navigate { id: r.id, url, replace });
                                }
                                if live.realm.revision() != live.rendered {
                                    // What the person did shows at once, not at the next frame.
                                    live.next_due_ms = Some(0.);
                                    live.next_frame_ms = 0.;
                                }
                                if let Some(r) = &last
                                    && !r.cancel.load(Ordering::Relaxed)
                                    && let Err(error) = render(r, &loader, &tx, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock, Pass::Live)
                                    && !is_cancellation(&error)
                                {
                                    eprintln!("Browser live: {error}");
                                }
                            }
                            continue;
                        }
                        Message::Recycle(images) => {
                            drop(images);
                            continue;
                        }
                        // A person scrolled a box: its offset kept (in the live
                        // page's document too, with its `scroll` event), the
                        // scene built again over the same layout, and the
                        // tiles it changed painted.
                        Message::ScrollBox { node, mut dx, mut dy } => {
                            // Wheel steps already queued for the same box: one scroll.
                            while let Ok(next) = rx.try_recv() {
                                match next {
                                    Message::ScrollBox { node: n, dx: x, dy: y } if n == node => {
                                        dx += x;
                                        dy += y;
                                    }
                                    other => {
                                        pending.push_back(other);
                                        break;
                                    }
                                }
                            }
                            if let Some(c) = cached.as_mut()
                                && let Some(r) = &last
                                && let Some(to) = c.page.scroll_box(node, [dx, dy])
                            {
                                if let Some(live) = c.live.as_mut() {
                                    live.realm.scroll_box(node, to);
                                    live.loads.dispatch(&mut live.realm);
                                }
                                let started = Instant::now();
                                let host = PageHost::new(c.page.scope(), &r.key);
                                let repainted = c.page.repaint(&host, &mut painter, Some(&r.cancel));
                                if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                                    eprintln!("  box scroll: scene in {:.2} ms", started.elapsed().as_secs_f64() * 1000.);
                                }
                                match repainted {
                                    Ok(()) => {
                                        if let Err(error) = render(r, &loader, &tx, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock, Pass::Static)
                                            && !is_cancellation(&error)
                                        {
                                            eprintln!("Browser box scroll: {error}");
                                        }
                                    }
                                    Err(error) => eprintln!("Browser box scroll: {error}"),
                                }
                            }
                            continue;
                        }
                        Message::SharedTextures(list) => {
                            if let Some(live) = cached.as_mut().and_then(|c| c.live.as_mut()) {
                                let mut released = Vec::new();
                                for (node, textures) in list {
                                    if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                                        eprintln!("  shared: {} textures given for a canvas", textures.len());
                                    }
                                    released.extend(live.realm.provide_shared(node, textures));
                                }
                                if !released.is_empty() {
                                    proxy.send(Reply::DropShared(released));
                                }
                                live.render_due = true;
                            } else {
                                // No page to draw into them: the window frees them.
                                proxy.send(Reply::DropShared(list.into_iter().flat_map(|(_, t)| t.into_iter().map(|t| t.0)).collect()));
                            }
                            continue;
                        }
                        Message::RecycleTargets(targets) => {
                            drop(targets);
                            continue;
                        }
                    };
                    let mut kept = std::collections::VecDeque::new();
                    for message in pending.drain(..).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
                        match message {
                            Message::Render(new) => r = new,
                            Message::Input(input) => kept.push_back(Message::Input(input)),
                            Message::Loaded(id, result) => kept.push_back(Message::Loaded(id, result)),
                            Message::Wake => kept.push_back(Message::Wake),
                            Message::Frame => kept.push_back(Message::Frame),
                            Message::Recycle(images) => drop(images),
                            Message::SharedTextures(list) => kept.push_back(Message::SharedTextures(list)),
                            Message::RecycleTargets(targets) => drop(targets),
                            m @ Message::ScrollBox { .. } => kept.push_back(m),
                        }
                    }
                    pending = kept;
                    drop(std::mem::take(&mut r.recycle));
                    if r.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let result = render(&r, &loader, &tx, &scope, &mut shaper, &mut painter, &mut cached, &proxy, &mut tiles, &mut clock, Pass::Static);
                    last = Some((*r).clone());
                    if let Err(error) = result
                        && !r.cancel.load(Ordering::Relaxed)
                    {
                        eprintln!("Browser: {error}");
                        proxy.send(Reply::Error(r.id, error.to_string()));
                        continue;
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
    tx: &mpsc::Sender<Message>,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    cached: &mut Option<Cached>,
    proxy: &Proxy,
    cache: &mut HashMap<(i32, i32), (Arc<RenderImage>, u64, u64)>,
    clock: &mut u64,
    pass: Pass,
) -> page::Result<()> {
    if r.key.width > MAX_VIEWPORT || r.key.height > MAX_VIEWPORT || !r.key.scale.is_finite() || r.key.scale <= 0. {
        return Err("Viewport exceeds the development renderer limits".into());
    }
    let newly_loaded = cached.as_ref().is_none_or(|c| c.key.source != r.key.source || c.key.cwd != r.key.cwd || c.key.revision != r.key.revision);
    let newly_loaded = newly_loaded || pass == Pass::Scripted && cached.as_ref().is_some_and(|c| !c.scripted);
    let mut rerendered = false;
    if pass == Pass::Live {
        let Some(c) = cached.as_mut() else { return Ok(()) };
        let Some(live) = c.live.as_mut() else { return Ok(()) };
        let now_ms = live.started.elapsed().as_secs_f64() * 1000.;
        // One frame clock, 60 Hz from the page's start: animation frames run
        // on its ticks and the page renders right after them, so what a frame
        // draws is what shows (two clocks, the page's 16 ms and the render's
        // 16.7, beat against each other and dropped frames).
        // Paced by the window's frames while it sends them (each of its frames
        // asks for the page's next: locked to the display, none dropped by a
        // timer drifting against it); by the page's own 60 Hz clock before.
        let paced = live.paced_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
        // Its window stopped drawing: the page is hidden (no animation
        // frames, visibilitychange) until the window asks for frames again.
        let is_hidden = hidden(live);
        if is_hidden != live.hidden {
            live.hidden = is_hidden;
            live.realm.set_hidden(is_hidden);
        }
        let mut frame = if paced { std::mem::take(&mut live.frame_requested) } else { !is_hidden && now_ms + 0.5 >= live.next_frame_ms };
        if frame && !paced {
            let period = frame_period_ms(live);
            live.next_frame_ms += period;
            if live.next_frame_ms <= now_ms {
                // Behind (a long task): the next frame from now, on the grid.
                live.next_frame_ms = ((now_ms / period).floor() + 1.) * period;
            }
        }
        // A page over its budget draws every `frame_divisor`th frame (see
        // `Usage`): its animations slower, the machine left its share.
        if frame {
            live.frame_count = live.frame_count.wrapping_add(1);
            frame = live.frame_count % u64::from(live.usage.divisor) == 0;
        }
        let advanced = live.realm.advance_frame(now_ms, Duration::from_millis(30), Some(frame));
        live.usage.cpu_ms += advanced.spent_ms;
        if let Some((url, replace)) = live.realm.take_navigation() {
            proxy.send(Reply::Navigate { id: r.id, url, replace });
        }
        // Its scripts stopped (memory, a hang): the page stays as it was
        // last shown, its realm (and the memory it held) let go.
        if let Some(why) = live.realm.stopped() {
            proxy.send(Reply::Status(r.id, format!("This page's scripts were stopped: {why}.")));
            c.live = None;
            return Ok(());
        }
        live.loads.dispatch(&mut live.realm);
        if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
            for line in live.realm.take_console() {
                eprintln!("  live console: {line}");
            }
        }
        live.next_due_ms = advanced.next_due_ms;
        live.wants_frame = advanced.wants_frame;
        live.stats.0 += 1;
        live.stats.1 += advanced.spent_ms;
        live.ticks += advanced.ticks;
        live.max_ticks = live.max_ticks.max(advanced.ticks);
        if live.stats.4.elapsed() >= Duration::from_secs(2) && std::env::var_os("OLO_BROWSER_TIMING").is_some() {
            let (advances, js, renders, render_ms, since) = live.stats;
            let report = live.realm.report();
            let (gl_calls, gl_seconds) = live.realm.take_gl_stats();
            eprintln!(
                "  live: in {:.1} s, {advances} advances ({} ticks, at most {} in one; {js:.0} ms of scripts, {gl_calls} WebGL calls in {:.0} ms), {renders} renders ({render_ms:.0} ms); {} layouts for measurements so far",
                since.elapsed().as_secs_f64(),
                live.ticks,
                live.max_ticks,
                gl_seconds * 1000.,
                report.layouts
            );
            live.ticks = 0;
            live.max_ticks = 0;
            live.stats = (0, 0., 0, 0., Instant::now());
            // OLO_JS_PROFILE: where the natives' time went, the top ten.
            if std::env::var_os("OLO_JS_PROFILE").is_some() {
                let profile = olo_js::take_native_profile();
                let total: f64 = profile.iter().map(|p| p.2).sum();
                let calls: u64 = profile.iter().map(|p| p.1).sum();
                let top: Vec<String> = profile.iter().take(10).map(|(name, n, t)| format!("{name} {n}×{:.1} ms", t * 1000.)).collect();
                eprintln!("  natives: {calls} calls in {:.0} ms: {}", total * 1000., top.join(", "));
            }
        }
        let revision = live.realm.revision();
        // The document changed, a readback is in flight, or a canvas was
        // drawn on (its drawing shows only through a render).
        if revision != live.rendered || live.render_again || live.realm.canvases_dirty() || c.page.animating() {
            if !frame {
                // Changes between frames show at the next frame.
                live.render_due = true;
                return Ok(());
            }
            live.render_due = false;
            let started = Instant::now();
            let doc = live.realm.snapshot();
            let snapshot_ms = started.elapsed().as_secs_f64() * 1000.;
            let viewport = layout::Viewport { width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale, scale: r.key.scale };
            // The live page's own cancellation, not the request's: a newer
            // request from the window supersedes what this render shows,
            // but the page goes on under it.
            let cancel = live.cancel.clone();
            if let Some(document) = &c.document {
                let light = document.light();
                let mut resource = |kind: page::ResourceKind, base: Option<&str>, href: &str| -> page::Result<Option<page::Resource>> {
                    Ok(match kind {
                        page::ResourceKind::Stylesheet => {
                            let absolute = base.and_then(|b| light.url.join(b).ok()).unwrap_or_else(|| light.url.clone()).join(href).ok().map(|u| Arc::<str>::from(u.as_str()));
                            loader.stylesheet(&light, base, href, &cancel, |_| {}).ok().map(|s| page::Resource::Stylesheet(s, absolute))
                        }
                        page::ResourceKind::Font => {
                            let url = base.and_then(|b| light.url.join(b).ok()).unwrap_or_else(|| light.url.clone()).join(href).ok();
                            url.and_then(|u| loader.font(&light, u, &cancel, |_| {}).ok()).map(page::Resource::Font)
                        }
                        page::ResourceKind::Image => None,
                    })
                };
                if c.page.refresh(&doc, Some(&cancel), &mut resource)? && std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                    eprintln!("  live render: stylesheets changed, rules remade");
                }
            }
            let host = PageHost::new(c.page.scope(), &r.key);
            let before_canvases = Instant::now();
            if std::env::var_os("OLO_CANVAS_DEBUG").is_some() {
                eprintln!("  canvases before render: {}", live.realm.canvas_states());
            }
            let canvases = live.realm.canvases();
            // Canvases whose frame is in a shared texture: refused below if
            // the scene can't show them as a surface.
            live.shared_frames = canvases.iter().filter_map(|(n, i)| i.shared_texture().map(|(k, _)| (*n, k))).collect();
            let wanted = live.realm.shared_wanted();
            if !wanted.is_empty() && std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                eprintln!("  shared: wanted for {} canvases ({:?})", wanted.len(), wanted.iter().map(|w| (w.1, w.2)).collect::<Vec<_>>());
            }
            if !wanted.is_empty() {
                proxy.send(Reply::SharedWanted(r.id, wanted));
            }
            let released = live.realm.released_shared();
            if !released.is_empty() {
                proxy.send(Reply::DropShared(released));
            }
            let canvases_ms = before_canvases.elapsed().as_secs_f64() * 1000.;
            let before_rerender = Instant::now();
            // A readback still in flight shows at the next render: soon.
            // A readback in flight shows at the next frame.
            live.render_again = live.realm.canvases_pending();
            c.page.set_time(now_ms);
            c.page.rerender(doc, canvases, &host, shaper, painter, viewport, Some(&cancel), |_, _| {})?;
            // CSS animations still running: the next frame renders again.
            live.render_again |= c.page.animating();
            // Their events reach the page's scripts.
            let events = c.page.take_animation_events();
            if !events.is_empty() {
                let events: Vec<(olo_web::dom::NodeId, &str, &str, f64)> = events.iter().map(|(n, k, name, t)| (*n, *k, &**name, *t)).collect();
                live.realm.animation_events(&events);
            }
            if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                eprintln!(
                    "  live render: revision {} -> {} in {:.0} ms (snapshot {snapshot_ms:.1}, canvases {canvases_ms:.1}, rerender {:.1})",
                    live.rendered,
                    revision,
                    started.elapsed().as_secs_f64() * 1000.,
                    before_rerender.elapsed().as_secs_f64() * 1000.
                );
            }
            // OLO_BROWSER_EVAL_LIVE=FILE: its expression evaluated after each
            // live render, logged when its value changes (inspecting a state
            // reached by interaction).
            if let Ok(file) = std::env::var("OLO_BROWSER_EVAL_LIVE")
                && let Ok(js) = std::fs::read_to_string(file)
            {
                match live.realm.eval(&js) {
                    Ok(value) => {
                        if live.last_eval.as_deref() != Some(value.as_str()) {
                            eprintln!("Browser live eval: {value}");
                            live.last_eval = Some(value);
                        }
                    }
                    Err(e) => eprintln!("Browser live eval failed: {e}"),
                }
            }
            live.rendered = revision;
            // The log of changes before this render (and the last
            // measurement) has served: dropped, so snapshots stay cheap.
            live.realm.forget_changes_before(revision);
            live.last_render = Instant::now();
            live.stats.2 += 1;
            live.stats.3 += started.elapsed().as_secs_f64() * 1000.;
            live.usage.cpu_ms += started.elapsed().as_secs_f64() * 1000.;
            live.usage.gpu_ms += live.realm.take_gpu_ms();
            live.usage.frames += 1;
            if let Some(divisor) = live.usage.review() {
                if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                    eprintln!("  usage: the page draws every {divisor} frames");
                }
            }
            rerendered = true;
        } else {
            // A frame that changed nothing renders nothing, so the window
            // draws no frame to ask for the next: a page still animating
            // (a timeline in its delay, say) runs on its own clock at the
            // display's rate until a change shows and the window paces it
            // again (it waited for a window frame that never came, and ran
            // at a few frames a second).
            if paced && frame && live.wants_frame {
                live.paced_at = None;
                live.next_frame_ms = now_ms + live.display_period_ms;
            }
            return Ok(());
        }
    }
    if newly_loaded {
        // The scripted pass starts from what the static pass fetched.
        let previous = if pass == Pass::Scripted { cached.take() } else { None };
        let loaded_at = previous.as_ref().map_or_else(Instant::now, |c| c.loaded_at);
        *cached = None;
        painter.clear_image_cache();
        cache.clear();
        proxy.send(Reply::Status(r.id, if pass == Pass::Scripted { "Running scripts…" } else { "Loading…" }.into()));
        let (document, source, mut images) = match previous {
            Some(c) => (c.document, c.source, c.images),
            None => {
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
                (document, source, HashMap::new())
            }
        };
        let mut last = Instant::now();
        let mut phase = None;
        let mut resources = 0usize;
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
        let mut live_slot: Option<Live> = None;
        let mut layout_slot: Option<Arc<layout::Layout>> = None;
        let mut canvas_slot: Vec<(olo_web::dom::NodeId, Arc<olo_web::image::Image>)> = Vec::new();
        // The live page's own cancellation: later requests from the window
        // cancel their predecessors, not the page living on.
        let live_cancel = crate::network::Cancel::default();
        // The page's decoded images so far, for scripts asking about them.
        let known_images = Arc::new(images.clone());
        let mut run_scripts = |doc: olo_web::dom::Document, quirks: bool, measurer: page::Measurer| -> page::Result<olo_web::dom::Document> {
            let Some(document) = &document else {
                return Ok(doc);
            };
            let page_url = document.url.to_string();
            let scripts: Vec<olo_js::Script> = olo_js::page_scripts(&doc, &page_url)
                .into_iter()
                .filter_map(|(source, name, module, node)| match source {
                    olo_js::Source::Inline(text) => Some(olo_js::Script { source: text, name, module, node: Some(node) }),
                    olo_js::Source::External(u) => {
                        proxy.send(Reply::Status(r.id, "Loading script…".into()));
                        let url = url::Url::parse(&u).ok()?;
                        match loader.script(document, url, &r.cancel, crate::network::progress(|s| proxy.send(Reply::Status(r.id, s)), "script")) {
                            Ok(text) => Some(olo_js::Script { source: Arc::from(text), name: u, module, node: Some(node) }),
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
            // The first scripted render waits only for what is due at once;
            // the page then lives on its own clock.
            let config = olo_js::Config {
                url: page_url,
                viewport: (f64::from(r.key.width) / r.key.scale, f64::from(r.key.height) / r.key.scale),
                scale: r.key.scale,
                quirks,
                dark: r.key.palette.is_dark(),
                budget_ms: 50.,
                ..Default::default()
            };
            let fetch_document = document.url.clone();
            let fetcher: olo_js::Fetcher = {
                let cancel = live_cancel.clone();
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
            // Measurements lay the document out with the styles and fonts
            // gathered so far, through a host for them.
            let measure: olo_js::Measure = {
                let mut host = PageHost::new(measurer.scope(), &r.key);
                let mut shaper = shaping::Engine::default();
                let cancel = live_cancel.clone();
                let key = r.key.clone();
                let loader = loader.clone();
                let light = document.light();
                let mut measurer = measurer;
                let timing = std::env::var_os("OLO_BROWSER_TIMING").is_some();
                Box::new(move |doc: &olo_web::dom::Document, previous: Option<page::Measured>| {
                    let call = Instant::now();
                    // Stylesheets the scripts added since: the rules follow.
                    let mut resource = |kind: page::ResourceKind, base: Option<&str>, href: &str| -> page::Result<Option<page::Resource>> {
                        Ok(match kind {
                            page::ResourceKind::Stylesheet => {
                                let absolute = base.and_then(|b| light.url.join(b).ok()).unwrap_or_else(|| light.url.clone()).join(href).ok().map(|u| Arc::<str>::from(u.as_str()));
                                loader.stylesheet(&light, base, href, &cancel, |_| {}).ok().map(|s| page::Resource::Stylesheet(s, absolute))
                            }
                            page::ResourceKind::Font => {
                                let url = base.and_then(|b| light.url.join(b).ok()).unwrap_or_else(|| light.url.clone()).join(href).ok();
                                url.and_then(|u| loader.font(&light, u, &cancel, |_| {}).ok()).map(page::Resource::Font)
                            }
                            page::ResourceKind::Image => None,
                        })
                    };
                    let refresh_started = Instant::now();
                    match measurer.refresh(doc, Some(&cancel), &mut resource) {
                        Ok(true) => {
                            if timing {
                                eprintln!("  measuring: stylesheets changed, rules remade in {:.0} ms", refresh_started.elapsed().as_secs_f64() * 1000.);
                            }
                            host = PageHost::new(measurer.scope(), &key);
                        }
                        Ok(false) => {}
                        Err(e) => eprintln!("Browser scripts: stylesheets: {e}"),
                    }
                    match measurer.measure(doc, &host, &mut shaper, viewport, previous, Some(&cancel)) {
                        Ok(m) => {
                            if timing {
                                let [a, b, c, d, e, f] = m.phases_ms();
                                eprintln!(
                                    "  layout for scripts: style {a:.1} ms, boxes {b:.1} ms, text {c:.1} ms, layout {d:.1} ms, index {e:.1} ms, all {f:.1} ms (call {:.1} ms): {}",
                                    call.elapsed().as_secs_f64() * 1000.,
                                    m.path()
                                );
                            }
                            Some(m)
                        }
                        Err(e) => {
                            eprintln!("Browser scripts: measuring failed: {e}");
                            None
                        }
                    }
                })
            };
            let started = Instant::now();
            // Images the scripts load (textures, canvas sources, `img`
            // elements they make): the page's own at once, others fetched
            // on their own threads and delivered to the live page.
            let image_lookup: olo_js::ImageLookup = {
                let known = known_images.clone();
                Box::new(move |url: &str| url::Url::parse(url).ok().and_then(|u| known.get(&u).cloned().flatten()))
            };
            let loads = Loads { loader: loader.clone(), document: document.light(), scope: scope.clone(), key: r.key.clone(), cancel: live_cancel.clone(), tx: tx.clone() };
            let mut realm = olo_js::Realm::new(doc, config, Some(fetcher), Some(measure))
                .map_err(|e| page::Error::from(e.as_str()))?
                .with_image_lookup(Some(image_lookup))
                .with_workers(Some(loads.worker_fetcher()), Some(loads.waker()));
            // OLO_BROWSER_EVAL_FIRST=FILE: its expression run before the
            // page's scripts (to patch what they will call, for debugging).
            if let Some(js) = std::env::var("OLO_BROWSER_EVAL_FIRST").ok().and_then(|f| std::fs::read_to_string(f).ok()) {
                match realm.eval(&js) {
                    Ok(value) => eprintln!("Browser eval (first): {value}"),
                    Err(e) => eprintln!("Browser eval (first) failed: {e}"),
                }
            }
            let report = realm.run_page(&scripts);
            loads.dispatch(&mut realm);
            let scripted_at = Instant::now();
            // OLO_BROWSER_EVAL=JS: an expression run after the page's scripts,
            // its value logged, for comparing with Chrome.
            let eval = std::env::var("OLO_BROWSER_EVAL").ok().or_else(|| std::env::var("OLO_BROWSER_EVAL_FILE").ok().and_then(|f| std::fs::read_to_string(f).ok()));
            if let Some(js) = eval {
                match realm.eval(&js) {
                    Ok(value) => eprintln!("Browser eval: {value}"),
                    Err(e) => eprintln!("Browser eval failed: {e}"),
                }
            }
            // The realm lives on: its timers keep running on the page's clock.
            let doc = realm.snapshot();
            layout_slot = realm.last_measured();
            canvas_slot = realm.canvases();
            let render_again = realm.canvases_pending();
            live_slot = Some(Live {
                realm,
                render_again,
                loads,
                cancel: live_cancel.clone(),
                started: scripted_at,
                next_due_ms: Some(0.),
                rendered: doc.revision(),
                last_render: Instant::now(),
                last_eval: None,
                stats: (0, 0., 0, 0., Instant::now()),
                ticks: 0,
                max_ticks: 0,
                next_frame_ms: 0.,
                wants_frame: false,
                render_due: false,
                shared_frames: Vec::new(),
                frame_count: 0,
                usage: Usage::new(),
                paced_at: None,
                frame_requested: false,
                display_period_ms: FRAME.as_secs_f64() * 1000.,
                hidden: false,
                waiting_since: None,
            });
            eprintln!(
                "Browser scripts: {} run, {} errors, {} console lines, {} timers, {} listeners, {} layouts ({:.0} ms), revision {} -> {}, {:.0} ms",
                report.scripts,
                report.errors.len(),
                report.console.len(),
                report.timers,
                report.listeners,
                report.layouts,
                report.layout_ms,
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
        let mut no_scripts = |doc: olo_web::dom::Document, _: bool, _: page::Measurer| -> page::Result<olo_web::dom::Document> { Ok(doc) };
        let prepared = page::prepare_scripted(
            source.clone(),
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
                        page::PAGE_IMAGE_LIMITS.bytes.saturating_sub(image_bytes),
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
                            eprintln!("Browser image {url}: {error}");
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
            |kind, items| {
                if let Some(document) = &document {
                    loader.prefetch(document, kind, items, &r.cancel);
                }
            },
            match pass {
                Pass::Scripted if JAVASCRIPT => Some(&mut run_scripts),
                // Parsed as with scripting on (noscript content hidden), nothing run.
                Pass::Static if JAVASCRIPT => Some(&mut no_scripts),
                _ => None,
            },
        )?;
        // The host is made for the page's own fonts (its web faces included).
        let host = PageHost::new(prepared.scope(), &r.key);
        let mut prepared = prepared;
        prepared.bind_images(std::mem::take(&mut canvas_slot), Some(&r.cancel))?;
        let page = page::finish_with(prepared, &host, shaper, painter, viewport, Some(&r.cancel), layout_slot.as_deref(), &mut report)?;
        let origin = document.as_ref().map(|d| d.url.clone()).unwrap_or(url::Url::parse("about:engine")?);
        let base = page.base_href.as_deref().and_then(|href| origin.join(href).ok()).unwrap_or_else(|| origin.clone());
        eprintln!("Browser: {} page ready {:.0} ms after the request", if pass == Pass::Scripted { "scripted" } else { "static" }, loaded_at.elapsed().as_secs_f64() * 1000.);
        *cached = Some(Cached {
            key: r.key.clone(),
            page,
            document,
            source,
            images,
            scripted: pass == Pass::Scripted,
            loaded_at,
            live: live_slot,
            viewport_dependent: host.viewport_used.get(),
            origin,
            base,
            links: HashMap::new(),
            navigation: 0,
            focus_targets: None,
            targets_scene: None,
            targets_revision: 0,
            label_cache: None,
            paint_identity: None,
            tile_signatures: HashMap::new(),
            anchor_scroll: None,
        });
        cache.clear();
    }
    let c = cached.as_mut().unwrap();
    let navigation_changed = c.navigation != r.navigation;
    let mut input = r.interaction;
    input.target = c.page.interaction.target;
    let mut scene_changed = newly_loaded || rerendered;
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
                return Err(page::Error::from(format!("restyle for the viewport: {error}").as_str()));
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
    let before_targets = Instant::now();
    // The targets come from the scene's commands and the links' and
    // focusable elements' attributes: kept while the commands are the same
    // and the document's changes since touched no attributes but styles (a
    // frame of an animation).
    let targets_stand = cached.focus_targets.is_some()
        && cached.targets_scene.as_ref().is_some_and(|s| s.same_commands(&cached.page.scene))
        && cached.page.document.changes_since(cached.targets_revision).is_some_and(|changes| changes.iter().all(|(_, c)| matches!(c, olo_web::dom::Change::Style(_))));
    if (scene_changed || cached.focus_targets.is_none()) && !targets_stand {
        cached.focus_targets = Some(crate::navigation::focus_targets(&cached.page, &cached.base, &cached.origin, &mut cached.links, &r.cancel)?);
        cached.targets_scene = Some(cached.page.scene.clone());
    }
    // Checked up to now (the change log before this render is let go).
    cached.targets_revision = cached.page.document.revision();
    let targets_ms = before_targets.elapsed().as_secs_f64() * 1000.;
    let before_labels = Instant::now();
    let page = &cached.page;
    let scene = &page.scene;
    let extent = scene.content_extent().height;
    let scroll = sought.map_or(r.scroll, |y| (y * r.key.scale).round().max(0.) as i32).clamp(0, max_scroll(&r.key, extent));
    let visible = layout::Rect { x: 0., y: f64::from(scroll) / r.key.scale, width: f64::from(r.key.width) / r.key.scale, height: f64::from(r.key.height) / r.key.scale };
    // The labels (what agents read of the page) are kept for a tenth of a
    // second while frames only move things (the same commands, scroll and
    // size): an animation does not make them again sixty times a second.
    let labels_stand = cached
        .label_cache
        .as_ref()
        .is_some_and(|(scene, at, s, k, _)| scene.same_commands(&cached.page.scene) && at.elapsed() < Duration::from_millis(100) && *s == scroll && *k == (r.key.width, r.key.height));
    let labels = if labels_stand {
        cached.label_cache.as_ref().map(|c| c.4.clone()).unwrap_or_default()
    } else {
        let mut hits = Vec::new();
        scene.query(visible, &mut hits, Some(&r.cancel))?;
        // The fixed layer's content lies in the viewport at the initial scroll.
        let viewport_rect = layout::Rect { x: 0., y: 0., width: visible.width, height: visible.height };
        let mut fixed_hits = Vec::new();
        for (i, stratum) in scene.strata().iter().enumerate() {
            if stratum.kind == paint::LayerKind::Fixed {
                let mut ids = Vec::new();
                scene.query_stratum(i, viewport_rect, &mut ids, Some(&r.cancel))?;
                fixed_hits.extend(ids);
            }
        }
        // Sticky boxes' content, each with its box (shifted by the scroll).
        let stickies = scene.layout().sticky();
        let mut sticky_hits: Vec<(u32, layout::Sticky)> = Vec::new();
        for (i, stratum) in scene.strata().iter().enumerate() {
            if let paint::LayerKind::Sticky(root) = stratum.kind
                && let Some(st) = stickies.iter().find(|s| s.root == root)
                && let Some(extent) = scene.stratum_extent(i)
            {
                let mut ids = Vec::new();
                scene.query_stratum(i, extent, &mut ids, Some(&r.cancel))?;
                sticky_hits.extend(ids.into_iter().map(|id| (id, *st)));
            }
        }
        // Moved layers' content where it shows now, with the layer's translation.
        let mut moved_hits: Vec<(u32, [f64; 2])> = Vec::new();
        for i in 0..scene.strata().len() {
            if let Some(m) = scene.moved(i) {
                let shown = match m.clip {
                    Some(c) => intersect(visible, c),
                    None => Some(visible),
                };
                if let Some(shown) = shown {
                    let region = layout::Rect { x: shown.x - m.translate[0], y: shown.y - m.translate[1], width: shown.width, height: shown.height };
                    let mut ids = Vec::new();
                    scene.query_stratum(i, region, &mut ids, Some(&r.cancel))?;
                    moved_hits.extend(ids.into_iter().map(|id| (id, m.translate)));
                }
            }
        }
        let mut labels = Vec::new();
        for (id, fixed, sticky, offset) in hits
            .into_iter()
            .map(|id| (id, false, None, [0., 0.]))
            .chain(fixed_hits.into_iter().map(|id| (id, true, None, [0., 0.])))
            .chain(sticky_hits.into_iter().map(|(id, st)| (id, false, Some(st), [0., 0.])))
            .chain(moved_hits.into_iter().map(|(id, t)| (id, false, None, t)))
        {
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
            let Some(mut rect) = scene.visible_bounds(id) else {
                continue;
            };
            rect.x += offset[0];
            rect.y += offset[1];
            let link = match node {
                Some(node) => crate::links::resolve(&page.document, node, &cached.base, &cached.origin, &mut cached.links)?,
                None => None,
            };
            labels.push(Label { image, text: text.into_owned().into(), rect, node, link, fixed, sticky });
        }
        cached.label_cache = Some((cached.page.scene.clone(), Instant::now(), scroll, (r.key.width, r.key.height), labels.clone()));
        labels
    };
    if pass == Pass::Live && std::env::var_os("OLO_BROWSER_TIMING").is_some() {
        eprintln!("  content: focus targets {targets_ms:.2} ms, labels {:.2} ms ({} labels)", before_labels.elapsed().as_secs_f64() * 1000., labels.len());
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
        scroll_areas: page
            .scene
            .scrollers()
            .iter()
            .filter(|s| (0..2).any(|i| s.user[i] && s.max()[i] > 0.))
            .map(|s| ScrollArea { node: s.node, rect: s.rect, fixed: s.fixed, offset: s.offset, max: s.max(), user: s.user })
            .collect(),
    });
    let stickies = scene.layout().sticky();
    let mut tiles = Vec::new();
    let mut last = Instant::now();
    let mut repainted = 0usize;
    let mut total = 0usize;
    thread_local! {
        static TILE_PARTS: std::cell::RefCell<HashMap<(i32, i32), Vec<(u64, &'static str, [u64; 5], u32)>>> = std::cell::RefCell::new(HashMap::new());
    }
    let mut tile_parts_store = TILE_PARTS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let tile_parts = &mut tile_parts_store;
    let painting = Instant::now();
    let scale = scene.device_scale();
    let top = scroll.div_euclid(256) * 256;
    // The strata in paint order: the base page run (opaque, sent as it
    // paints), then each layer above it (transparent): page runs over the
    // visible rows, the fixed layer over the viewport, a sticky layer over
    // its own extent; every tile kept by the stratum's signature there,
    // tiles with nothing of the stratum skipped.
    let mut layers: Vec<LayerTiles> = Vec::new();
    let mut shown_shared: Vec<u64> = Vec::new();
    // The tiles' signatures, kept for a next scene that paints the same.
    let identity = (scene.paint_identity(), r.key.width, r.key.height, scene.device_scale().to_bits());
    let same_paint = cached.paint_identity == Some(identity);
    let previous_signatures = std::mem::take(&mut cached.tile_signatures);
    let mut signatures: HashMap<(usize, i32, i32), Option<u64>> = HashMap::with_capacity(previous_signatures.len());
    for (si, stratum) in scene.strata().iter().enumerate() {
        if let paint::LayerKind::Surface(_) = stratum.kind {
            // A live surface: its image shown as it is, scrolled with the
            // page, kept by its identity (no tiles to paint: what is around
            // it stays as it was while it changes).
            let Some((bounds, image)) = scene.surface(si) else {
                layers.push(LayerTiles { shared: None, placement: Placement::Page, tiles: Vec::new(), scaled: false });
                continue;
            };
            let x = (bounds.x * scale).round() as i32;
            let y = (bounds.y * scale).round() as i32;
            let width = ((bounds.x + bounds.width) * scale).round() as i32 - x;
            let height = ((bounds.y + bounds.height) * scale).round() as i32 - y;
            if width <= 0 || height <= 0 {
                layers.push(LayerTiles { shared: None, placement: Placement::Page, tiles: Vec::new(), scaled: false });
                continue;
            }
            let region = paint::DeviceRect { x, y, width: width as u32, height: height as u32 };
            // A frame in a shared texture: the window shows the texture.
            if let Some((key, premultiplied)) = image.shared_texture() {
                shown_shared.push(key);
                total += 1;
                let scaled = image.width() != region.width || image.height() != region.height;
                layers.push(LayerTiles { shared: Some((region, key, premultiplied)), placement: Placement::Page, tiles: Vec::new(), scaled });
                continue;
            }
            *clock = clock.wrapping_add(1);
            // In the tile cache under the stratum's own key, by the image's identity.
            let key = (i32::MAX - si as i32, i32::MAX);
            let signature = image.serial();
            let shown = if let Some((shown, stamp, kept)) = cache.get_mut(&key)
                && *kept == signature
            {
                *stamp = *clock;
                shown.clone()
            } else {
                let mut bytes = image.rgba().to_vec();
                to_bgra(&mut bytes);
                let shown = olo_ui::widgets::image_view::from_premultiplied_bgra(image.width(), image.height(), bytes).ok_or("Surface upload failed")?;
                cache.insert(key, (shown.clone(), *clock, signature));
                repainted += 1;
                shown
            };
            total += 1;
            let scaled = image.width() != region.width || image.height() != region.height;
            layers.push(LayerTiles { shared: None, placement: Placement::Page, tiles: vec![(region, shown)], scaled });
            continue;
        }
        let (x0, y0, x1, y1) = match stratum.kind {
            paint::LayerKind::Page | paint::LayerKind::Surface(_) => (0, top, r.key.width as i32, scroll + r.key.height as i32),
            // A moved layer: the tiles of its own space that show now (the
            // visible rows within its clip, less its translation); a frame
            // that only moves it finds them all kept.
            paint::LayerKind::Moved(_) => {
                let Some(m) = scene.moved(si) else { continue };
                let (dx, dy) = ((m.translate[0] * scale).round() as i32, (m.translate[1] * scale).round() as i32);
                let (mut sx0, mut sy0, mut sx1, mut sy1) = (0, scroll, r.key.width as i32, scroll + r.key.height as i32);
                if let Some(c) = m.clip {
                    sx0 = sx0.max((c.x * scale).floor() as i32);
                    sy0 = sy0.max((c.y * scale).floor() as i32);
                    sx1 = sx1.min(((c.x + c.width) * scale).ceil() as i32);
                    sy1 = sy1.min(((c.y + c.height) * scale).ceil() as i32);
                }
                if sx1 <= sx0 || sy1 <= sy0 {
                    layers.push(LayerTiles { shared: None, placement: Placement::Page, tiles: Vec::new(), scaled: false });
                    continue;
                }
                ((sx0 - dx).div_euclid(256) * 256, (sy0 - dy).div_euclid(256) * 256, sx1 - dx, sy1 - dy)
            }
            paint::LayerKind::Fixed | paint::LayerKind::Sticky(_) => {
                let Some(extent) = scene.stratum_extent(si) else {
                    layers.push(LayerTiles { shared: None, placement: Placement::Fixed, tiles: Vec::new(), scaled: false });
                    continue;
                };
                let (limit_x, limit_y) = if stratum.kind == paint::LayerKind::Fixed { (f64::from(r.key.width), f64::from(r.key.height)) } else { (f64::INFINITY, f64::INFINITY) };
                let x0 = (extent.x * scale).floor().max(0.).min(limit_x) as i32;
                let y0 = (extent.y * scale).floor().max(0.).min(limit_y) as i32;
                let x1 = ((extent.x + extent.width) * scale).ceil().min(limit_x).min(f64::from(x0) + 2048.) as i32;
                let y1 = ((extent.y + extent.height) * scale).ceil().min(limit_y).min(f64::from(y0) + 2048.) as i32;
                (x0.div_euclid(256) * 256, y0.div_euclid(256) * 256, x1, y1)
            }
        };
        let placement = match stratum.kind {
            paint::LayerKind::Moved(_) => match scene.moved(si) {
                Some(m) => Placement::Moved {
                    dx: (m.translate[0] * scale).round() as i32,
                    dy: (m.translate[1] * scale).round() as i32,
                    clip: m.clip.map(|c| {
                        let (x0, y0) = ((c.x * scale).floor() as i32, (c.y * scale).floor() as i32);
                        let (x1, y1) = (((c.x + c.width) * scale).ceil() as i32, ((c.y + c.height) * scale).ceil() as i32);
                        paint::DeviceRect { x: x0, y: y0, width: (x1 - x0).max(0) as u32, height: (y1 - y0).max(0) as u32 }
                    }),
                },
                None => Placement::Page,
            },
            paint::LayerKind::Page | paint::LayerKind::Surface(_) => Placement::Page,
            paint::LayerKind::Fixed => Placement::Fixed,
            paint::LayerKind::Sticky(root) => match stickies.iter().find(|s| s.root == root) {
                Some(st) => Placement::Sticky(*st),
                None => Placement::Page,
            },
        };
        let mut layer_tiles = Vec::new();
        for y in (y0..y1).step_by(256) {
            for x in (x0..x1).step_by(256) {
                if r.cancel.load(Ordering::Relaxed) {
                    return Err(paint::Error::Cancelled.into());
                }
                *clock = clock.wrapping_add(1);
                let region = paint::DeviceRect { x, y, width: 256, height: 256 };
                let tile_rect = layout::Rect { x: f64::from(x) / scale, y: f64::from(y) / scale, width: 256. / scale, height: 256. / scale };
                // A tile painted from the same commands over the same images,
                // styles and texts is the same tile: kept across re-renders.
                let signature = if si == 0 && std::env::var_os("OLO_TILE_DEBUG").is_some() {
                    // Name the first command that changed since the last paint of this tile.
                    let (signature, parts) = scene.tile_signature_parts(tile_rect, Some(&r.cancel))?;
                    let previous = tile_parts.insert((x, y), parts.clone());
                    if let Some(previous) = previous
                        && let Some((image, _, kept)) = cache.get(&(x, y))
                        && *kept != signature
                    {
                        let _ = image;
                        let first = parts.iter().zip(previous.iter()).position(|(a, b)| a.0 != b.0);
                        let which = first.map(|i| {
                            let names = ["bounds", "transform", "clip", "groups", "primitive"];
                            (0..5).find(|&k| parts[i].2[k] != previous[i].2[k]).map_or("?", |k| names[k])
                        });
                        let owner = first.and_then(|i| scene.node(parts[i].3)).map(|n| {
                            let doc = &page.document;
                            format!(
                                "<{} id={:?} class={:?}>",
                                doc.name(n).ok().flatten().unwrap_or("?"),
                                doc.attribute(n, "id").ok().flatten().unwrap_or(""),
                                doc.attribute(n, "class").ok().flatten().unwrap_or("").chars().take(60).collect::<String>()
                            )
                        });
                        eprintln!(
                            "  tile {x},{y} changed: {} commands (was {}), first difference at {:?} in {:?} of {:?}",
                            parts.len(),
                            previous.len(),
                            first.map(|i| (i, parts[i].1, previous[i].1)),
                            which,
                            owner
                        );
                    }
                    signature
                } else {
                    // The scene paints what the last did (only moved layers
                    // placed anew): the tile's signature then is the last one's.
                    let known = if same_paint { previous_signatures.get(&(si, x, y)).copied() } else { None };
                    let signature = match known {
                        Some(known) => known,
                        None => {
                            let (signature, any) = scene.stratum_tile_signature(si, tile_rect, Some(&r.cancel))?;
                            use std::hash::{Hash, Hasher};
                            let mut h = std::hash::DefaultHasher::new();
                            (signature, si).hash(&mut h);
                            (si == 0 || any).then(|| h.finish())
                        }
                    };
                    signatures.insert((si, x, y), signature);
                    // Nothing of this layer here: no tile.
                    let Some(signature) = signature else { continue };
                    signature
                };
                // Each stratum's tiles apart in the cache (x stays below 2^16).
                // Each stratum's tiles apart (a moved layer's x may be negative).
                let key = if matches!(stratum.kind, paint::LayerKind::Moved(_)) { (x ^ ((si as i32 + 1) << 24), y) } else { (x | ((si as i32) << 16), y) };
                let image = if let Some((image, stamp, kept)) = cache.get_mut(&key)
                    && *kept == signature
                {
                    *stamp = *clock;
                    image.clone()
                } else {
                    let tile = painter.raster_stratum(scene, si, region, si == 0, Some(&r.cancel))?;
                    let mut bytes = tile.into_rgba();
                    to_bgra(&mut bytes);
                    let image = olo_ui::widgets::image_view::from_premultiplied_bgra(256, 256, bytes).ok_or("Tile upload failed")?;
                    if cache.len() >= 512 {
                        let key = *cache.iter().min_by_key(|(_, (_, stamp, _))| *stamp).unwrap().0;
                        cache.remove(&key);
                    }
                    cache.insert(key, (image.clone(), *clock, signature));
                    repainted += 1;
                    image
                };
                total += 1;
                if si == 0 {
                    tiles.push((region, image));
                    if last.elapsed() > Duration::from_millis(120) {
                        proxy.send(Reply::Batch(r.id, std::mem::take(&mut tiles)));
                        last = Instant::now();
                    }
                } else {
                    layer_tiles.push((region, image));
                }
            }
        }
        if si > 0 {
            layers.push(LayerTiles { shared: None, placement, tiles: layer_tiles, scaled: false });
        }
    }
    TILE_PARTS.with(|t| *t.borrow_mut() = tile_parts_store);
    cached.paint_identity = Some(identity);
    cached.tile_signatures = signatures;
    // A canvas whose frame is in a shared texture but which the scene shows
    // otherwise (under a transform, in a group): read back from now on.
    if let Some(live) = cached.live.as_mut() {
        for (node, key) in std::mem::take(&mut live.shared_frames) {
            if !shown_shared.contains(&key) {
                if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                    let surfaces: Vec<String> = (0..scene.strata().len()).filter_map(|i| scene.surface(i).map(|(_, im)| format!("{:?}", im.shared_texture()))).collect();
                    eprintln!("  shared: refusing key {key}: the scene's surfaces show {surfaces:?}, frames {:?}", shown_shared);
                }
                live.realm.refuse_shared(node);
                live.render_due = true;
            }
        }
    }
    if pass == Pass::Live && std::env::var_os("OLO_BROWSER_TIMING").is_some() && !layers.is_empty() {
        eprintln!("  layers: {} ({} tiles)", layers.len(), layers.iter().map(|l| l.tiles.len()).sum::<usize>());
    }
    proxy.send(Reply::Layers(r.id, layers));
    if pass == Pass::Live && std::env::var_os("OLO_BROWSER_TIMING").is_some() {
        eprintln!("  tiles: {repainted} of {total} repainted in {:.1} ms", painting.elapsed().as_secs_f64() * 1000.);
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
        animating: {
            let animating = cached.page.animating() || cached.live.as_ref().is_some_and(|l| l.wants_frame || l.render_due || l.render_again || l.realm.canvases_dirty());
            if animating && let Some(live) = cached.live.as_mut() {
                live.waiting_since.get_or_insert_with(Instant::now);
            }
            animating
        },
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
    use olo_web::font::GenericPolicy;
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
        let red = |page: &page::Page| page.scene.commands().iter().find(|c| matches!(c.primitive, paint::Primitive::Fill { color: [255, 0, 0, 255], .. })).unwrap().bounds;
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
