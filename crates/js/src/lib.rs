//! JavaScript for Olo's browser: V8 runs a page's scripts against the
//! engine's document. A realm holds one isolate, one context and the
//! document; a few native functions (`__olo.*`) read and change the
//! document, run selectors, serialise and parse HTML, log and fetch; the
//! prelude (`prelude.js`) builds the DOM and Web API surface on them:
//! Node, Element, Document, events, timers, storage, location, fetch.
//!
//! Scripts run after the document is parsed (in order, as if deferred);
//! `document.write` is not supported. Time is virtual: timers run in due
//! order up to a budget, without waiting. The document comes back changed
//! and the page styles, lays out and paints from it.
#![forbid(unsafe_op_in_unsafe_fn)]
pub mod canvas;
pub mod clone;
pub mod html;
mod modules;
pub mod webgl;
pub mod worker;
use olo_web::{
    css::{
        self,
        selectors::{CompileOptions, DomTree, MatchLimits, MatchOptions, Selectors},
    },
    dom::{Document, Namespace, NodeId, NodeType},
    html::QuirksMode,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    rc::Rc,
    sync::{Arc, OnceLock},
    time::Instant,
};
const PRELUDE: &str = concat!(include_str!("shared.js"), "\n", include_str!("prelude.js"), "\n", include_str!("webgl.js"));
/// What the page is, for `location`, `navigator` and the viewport.
#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub viewport: (f64, f64),
    pub scale: f64,
    pub user_agent: String,
    pub language: String,
    pub quirks: bool,
    /// Whether the page is shown dark (`prefers-color-scheme`).
    pub dark: bool,
    /// Virtual milliseconds of timers to run after the scripts and load.
    pub budget_ms: f64,
    /// Wall-clock seconds the whole run may take.
    pub wall_seconds: f64,
}
/// What the browser calls itself, to servers and to scripts alike: a
/// Chrome of the same engine generation, so sites serve the pages they
/// serve Chrome (an unknown agent gets fallback or bot pages), with Olo
/// named after it.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Olo/0.1";
impl Default for Config {
    fn default() -> Self {
        Self { url: "about:blank".into(), viewport: (1000., 700.), scale: 1., user_agent: USER_AGENT.into(), language: "en-US".into(), quirks: false, dark: false, budget_ms: 3000., wall_seconds: 8. }
    }
}
/// A fetched resource for `fetch` and `XMLHttpRequest`.
pub struct Fetched {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub url: String,
}
/// Fetches a URL (method, body) for scripts; the browser supplies one.
pub type Fetcher = Box<dyn FnMut(&str, &str, &str) -> Result<Fetched, String>>;
/// An image already decoded for the page, by absolute URL (no I/O); the
/// browser supplies one. Images it doesn't have are requested
/// (`Realm::take_image_requests`) and delivered (`Realm::image_loaded`)
/// when fetched, off this thread.
pub type ImageLookup = Box<dyn FnMut(&str) -> Option<Arc<olo_web::image::Image>>>;
/// Something the scripts asked for over the network: the browser fetches
/// it (off this thread) and answers with `Realm::deliver`.
pub struct Request {
    pub id: u64,
    pub url: String,
    pub kind: RequestKind,
}
pub enum RequestKind {
    /// An `img` element's image, decoded.
    Image,
    /// A fetch() or XMLHttpRequest: the bytes.
    Text { method: String, body: String },
}
/// What a request got.
pub enum Loaded {
    Image(Arc<olo_web::image::Image>),
    Text(Fetched),
}
pub use worker::{SharedFetcher, Waker};
/// Lays out the document as it is now, for what scripts measure; the
/// browser supplies one (olo_web::page::Measurer behind a host).
pub type Measure = Box<dyn FnMut(&Document, Option<olo_web::page::Measured>) -> Option<olo_web::page::Measured>>;
/// How often scripts may have the page laid out, and for how long, within
/// one run (the page's scripts, or one advance of its clock): past these,
/// measurements come from the last layout until the next run.
const LAYOUTS: usize = 400;
const LAYOUT_SECONDS: f64 = 4.;
const LAYOUTS_PER_ADVANCE: usize = 60;
const LAYOUT_SECONDS_PER_ADVANCE: f64 = 0.25;
/// A script to run: its source and a name for errors.
pub struct Script {
    pub source: Arc<str>,
    pub name: String,
    /// A module script (`type="module"`): its imports loaded, run as one.
    pub module: bool,
    /// Its `<script>` element: `document.currentScript` while a classic
    /// script runs (Turbopack's chunks find their URL by it).
    pub node: Option<NodeId>,
}
/// Input from the person, for the page's scripts: where (document
/// coordinates, CSS px) and what.
#[derive(Clone, Debug)]
pub struct Input {
    pub kind: InputKind,
    pub x: f64,
    pub y: f64,
    /// The window's scroll position, so events report client coordinates.
    pub scroll_y: f64,
    pub buttons: u32,
    /// Bits: shift 1, control 2, alt 4, meta 8.
    pub modifiers: u8,
}
#[derive(Clone, Debug)]
pub enum InputKind {
    Move,
    Down,
    Up,
    Wheel {
        dx: f64,
        dy: f64,
    },
    Key {
        down: bool,
        key: String,
        code: String,
        text: Option<String>,
    },
    /// The window scrolled (to `scroll_y`).
    Scroll,
}
/// What an advance of the page's clock did.
#[derive(Debug, Clone, Copy)]
pub struct Advanced {
    pub next_due_ms: Option<f64>,
    pub changed: bool,
    /// Wall time spent running scripts (and their measurements).
    pub spent_ms: f64,
    /// How many times the page's tick ran (more than one: work due at the
    /// same moment, scheduled by what ran).
    pub ticks: usize,
    /// The page waits for a display frame (animation frame callbacks,
    /// intersection checks): driven by frames, see `advance_frame`.
    pub wants_frame: bool,
}
/// What happened.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub scripts: usize,
    pub errors: Vec<String>,
    pub console: Vec<String>,
    pub timers: usize,
    pub listeners: usize,
    /// Layouts run for measurements, and their time.
    pub layouts: usize,
    pub layout_ms: f64,
    pub revision_before: u64,
    pub revision_after: u64,
    pub wall_ms: f64,
}
struct Templates {
    event_target: v8::Global<v8::FunctionTemplate>,
    node: v8::Global<v8::FunctionTemplate>,
    element: v8::Global<v8::FunctionTemplate>,
    html_element: v8::Global<v8::FunctionTemplate>,
    svg_element: v8::Global<v8::FunctionTemplate>,
    text: v8::Global<v8::FunctionTemplate>,
    comment: v8::Global<v8::FunctionTemplate>,
    document: v8::Global<v8::FunctionTemplate>,
    fragment: v8::Global<v8::FunctionTemplate>,
}
struct State {
    document: Document,
    config: Config,
    quirks: QuirksMode,
    wrappers: HashMap<NodeId, v8::Global<v8::Object>>,
    /// The boxed ids wrappers point at; freed with the realm.
    ids: Vec<*mut NodeId>,
    templates: Option<Templates>,
    fetcher: Option<Fetcher>,
    /// The page's canvases, by element, drawn by its scripts.
    canvases: HashMap<NodeId, canvas::Canvas>,
    /// The page's WebGL canvases, by element, drawn by the GPU.
    gl: HashMap<NodeId, webgl::Context>,
    /// WebGL calls made and the time in them (for the live page's stats).
    gl_calls: u64,
    /// Selectors compiled for querySelector and matches, by their text
    /// (a page asks the same ones over and over).
    selectors: HashMap<(String, bool), Arc<Selectors>>,
    gl_seconds: f64,
    image_lookup: Option<ImageLookup>,
    /// Images the scripts loaded, by `img` element.
    images: HashMap<NodeId, Arc<olo_web::image::Image>>,
    /// Image loads asked for and not yet taken by the browser, then in
    /// flight until delivered.
    requests: Vec<Request>,
    /// The element each image request in flight is for.
    in_flight: HashMap<u64, Option<NodeId>>,
    /// The modules loaded so far, by URL.
    modules: modules::Modules,
    /// A navigation the page asked for (location, a form), not yet taken.
    navigation: Option<(String, bool)>,
    next_request: u64,
    /// Workers by id, with what they need to be made.
    workers: HashMap<u32, worker::Handle>,
    next_worker: u32,
    worker_fetcher: Option<SharedFetcher>,
    waker: Option<Waker>,
    measure: Option<Measure>,
    /// The last layout, good while the document's revision is its own.
    measured: Option<olo_web::page::Measured>,
    layout_seconds: f64,
    /// Layouts and their time within the current run, against the run's
    /// budget.
    run_layouts: usize,
    run_layout_seconds: f64,
    run_limits: (usize, f64),
    report: Report,
    started: Instant,
}
type Shared = Rc<RefCell<State>>;
pub(crate) fn runtime() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        // A page's heap goes back to the system when its isolate goes (a
        // page stopped at its memory limit, a page left): V8 kept the pages
        // of a dead isolate in a process-wide pool for the next one (1.8 GB
        // held after a page was stopped at 2 GB), and pooled pages are let
        // go after five seconds unused.
        v8::V8::set_flags_from_string("--no-memory-pool-share-memory-on-teardown --memory-pool-timeout=5");
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}
/// One page's scripting: its isolate, context and document.
pub struct Realm {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    state: Shared,
    /// What stops the page's scripts: its heap at the limit, a task that
    /// never ends (see `Guard`).
    guard: Arc<Guard>,
}
/// A page's scripts may use this much of V8's heap: past it they are
/// stopped (the page, not the browser or the machine, out of memory).
const HEAP_MAX: usize = 2 << 30;
/// A task (a script, a timer, an event) may run this long before the
/// page's scripts are stopped (a page that hangs).
const TASK_MAX: std::time::Duration = std::time::Duration::from_secs(10);
/// Why a page's scripts were stopped.
const STOPPED_MEMORY: u8 = 1;
const STOPPED_HUNG: u8 = 2;
/// The realm's stop switch, shared with V8's heap callback and the
/// watchdog thread: when the running task began (ms since the watchdog
/// started; 0: none runs), and why its scripts were stopped (0: they
/// weren't).
pub struct Guard {
    handle: v8::IsolateHandle,
    busy_since: std::sync::atomic::AtomicU64,
    stopped: std::sync::atomic::AtomicU8,
}
/// The realms the watchdog watches.
static GUARDS: std::sync::Mutex<Vec<std::sync::Weak<Guard>>> = std::sync::Mutex::new(Vec::new());
static WATCH_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
fn watch_now_ms() -> u64 {
    WATCH_EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}
/// One thread for every realm of the process: twice a second, a task
/// running longer than `TASK_MAX` stops its realm's scripts. Asleep, it
/// costs nothing but those wakeups, and only while a realm lives.
fn watch(guard: &Arc<Guard>) {
    let mut guards = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    guards.retain(|g| g.strong_count() > 0);
    let first = guards.is_empty();
    guards.push(Arc::downgrade(guard));
    drop(guards);
    if first {
        let _ = std::thread::Builder::new().name("script-watchdog".into()).spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let mut guards = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
                guards.retain(|g| g.strong_count() > 0);
                if guards.is_empty() {
                    break;
                }
                let now = watch_now_ms();
                for g in guards.iter().filter_map(|g| g.upgrade()) {
                    let since = g.busy_since.load(std::sync::atomic::Ordering::Relaxed);
                    if since != 0 && now.saturating_sub(since) > TASK_MAX.as_millis() as u64 && g.stopped.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                        g.stopped.store(STOPPED_HUNG, std::sync::atomic::Ordering::Relaxed);
                        g.handle.terminate_execution();
                    }
                }
            }
        });
    }
}
/// V8 near its heap limit: the page's scripts are stopped, with room to
/// unwind (else V8 aborts the process).
unsafe extern "C" fn near_heap_limit(data: *mut std::ffi::c_void, current: usize, _initial: usize) -> usize {
    // SAFETY: `data` is the realm's guard, alive while its isolate is.
    let guard = unsafe { &*(data as *const Guard) };
    if guard.stopped.swap(STOPPED_MEMORY, std::sync::atomic::Ordering::Relaxed) == 0 {
        guard.handle.terminate_execution();
    }
    current + (64 << 20)
}
impl Drop for Realm {
    fn drop(&mut self) {
        // What the heap held goes back to the system with the isolate: told
        // to let go of what it keeps for reuse first.
        self.isolate.low_memory_notification();
        if std::env::var_os("OLO_WEB_TIMING").is_some() {
            eprintln!("realm: dropped");
        }
    }
}
/// A task running in the realm (see `Guard`), until dropped.
struct Busy<'a>(&'a Guard);
impl<'a> Busy<'a> {
    fn new(guard: &'a Guard) -> Self {
        guard.busy_since.store(watch_now_ms(), std::sync::atomic::Ordering::Relaxed);
        Self(guard)
    }
}
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy_since.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}
impl Realm {
    pub fn new(document: Document, config: Config, fetcher: Option<Fetcher>, measure: Option<Measure>) -> Result<Self, String> {
        runtime();
        let quirks = if config.quirks { QuirksMode::Quirks } else { QuirksMode::NoQuirks };
        let revision = document.revision();
        // Import maps apply to every module the page loads, so they are read
        // before any runs.
        let import_map = modules::ImportMap::of_document(&document, &config.url);
        let state: Shared = Rc::new(RefCell::new(State {
            document,
            config,
            quirks,
            wrappers: HashMap::new(),
            ids: Vec::new(),
            templates: None,
            fetcher,
            canvases: HashMap::new(),
            gl: HashMap::new(),
            gl_calls: 0,
            gl_seconds: 0.,
            selectors: HashMap::new(),
            image_lookup: None,
            images: HashMap::new(),
            requests: Vec::new(),
            in_flight: HashMap::new(),
            modules: modules::Modules::with_map(import_map),
            navigation: None,
            next_request: 1,
            workers: HashMap::new(),
            next_worker: 1,
            worker_fetcher: None,
            waker: None,
            measure,
            measured: None,
            layout_seconds: 0.,
            run_layouts: 0,
            run_layout_seconds: 0.,
            run_limits: (LAYOUTS, LAYOUT_SECONDS),
            report: Report { revision_before: revision, ..Default::default() },
            started: Instant::now(),
        }));
        let mut isolate = v8::Isolate::new(v8::CreateParams::default().heap_limits(0, HEAP_MAX));
        let guard = Arc::new(Guard { handle: isolate.thread_safe_handle(), busy_since: Default::default(), stopped: Default::default() });
        isolate.add_near_heap_limit_callback(near_heap_limit, Arc::as_ptr(&guard) as *mut std::ffi::c_void);
        watch(&guard);
        isolate.set_slot(state.clone());
        modules::install(&mut isolate);
        modules::enter(&state);
        let context = {
            v8::scope!(let scope, &mut isolate);
            let global = v8::ObjectTemplate::new(scope);
            // Named access on the window: an unknown global name is the
            // element with that id (or name, for a few elements), as in
            // browsers. Non-masking: only names nothing else defines.
            global.set_named_property_handler(
                v8::NamedPropertyHandlerConfiguration::new().getter(named_global).flags(v8::PropertyHandlerFlags::NON_MASKING | v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS),
            );
            let context = v8::Context::new(scope, v8::ContextOptions { global_template: Some(global), ..Default::default() });
            let scope = &mut v8::ContextScope::new(scope, context);
            install(scope, &state)?;
            v8::Global::new(scope, context)
        };
        let mut realm = Self { isolate, context, state, guard };
        realm.run(PRELUDE, "prelude.js").map_err(|e| format!("prelude: {e}"))?;
        Ok(realm)
    }
    /// WebGL calls made and seconds spent in them since last taken.
    pub fn take_gl_stats(&mut self) -> (u64, f64) {
        let mut s = self.state.borrow_mut();
        (std::mem::take(&mut s.gl_calls), std::mem::take(&mut s.gl_seconds))
    }
    /// Console lines logged since last taken (the live page's).
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().report.console)
    }
    /// Supply the lookup of images the page already has decoded.
    pub fn with_image_lookup(self, lookup: Option<ImageLookup>) -> Self {
        self.state.borrow_mut().image_lookup = lookup;
        self
    }
    /// Workers fetch their scripts (and fetch()) through this, on their
    /// own threads; `waker` is called when one posts a message, so the
    /// browser advances the page soon.
    pub fn with_workers(self, fetcher: Option<SharedFetcher>, waker: Option<Waker>) -> Self {
        let mut s = self.state.borrow_mut();
        s.worker_fetcher = fetcher;
        s.waker = waker;
        drop(s);
        self
    }
    /// The requests scripts made since last taken; the browser fetches
    /// each and answers with `deliver`.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.state.borrow_mut().requests)
    }
    /// A request's outcome: an image element gets its image and its scripts
    /// hear `load` (or `error`); a fetch resolves (or rejects).
    pub fn deliver(&mut self, id: u64, result: Result<Loaded, String>) {
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return;
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        let Some(node) = state.borrow_mut().in_flight.remove(&id) else { return };
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let (function, args): (&str, Vec<v8::Local<v8::Value>>) = match (node, result) {
            (Some(node), result) => {
                let size = match result {
                    Ok(Loaded::Image(image)) => {
                        let size = image.natural();
                        state.borrow_mut().images.insert(node, image);
                        Some(size)
                    }
                    Ok(Loaded::Text(_)) => None,
                    Err(e) => {
                        state.borrow_mut().report.console.push(format!("[warn] image: {e}"));
                        None
                    }
                };
                let Some(element) = wrap(tc, &state, node) else { return };
                let size: v8::Local<v8::Value> = match size {
                    Some((w, h)) => {
                        let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(tc, w.round()).into(), v8::Number::new(tc, h.round()).into()];
                        v8::Array::new_with_elements(tc, &items).into()
                    }
                    None => v8::null(tc).into(),
                };
                ("__olo_imageLoaded", vec![element.into(), size])
            }
            (None, Ok(Loaded::Text(fetched))) => (
                "__olo_fetched",
                vec![
                    v8::Number::new(tc, id as f64).into(),
                    v8::Boolean::new(tc, true).into(),
                    v8::Integer::new(tc, i32::from(fetched.status)).into(),
                    string(tc, &fetched.content_type),
                    bytes_array(tc, fetched.body, ByteKind::U8),
                    string(tc, &fetched.url),
                ],
            ),
            (None, Ok(Loaded::Image(_))) => return,
            (None, Err(e)) => {
                ("__olo_fetched", vec![v8::Number::new(tc, id as f64).into(), v8::Boolean::new(tc, false).into(), v8::null(tc).into(), v8::null(tc).into(), v8::null(tc).into(), string(tc, &e)])
            }
        };
        let Some(name) = v8::String::new(tc, function) else { return };
        let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        if f.call(tc, global.into(), &args).is_none() {
            let error = exception_text(tc, function);
            state.borrow_mut().report.errors.push(error);
        }
    }
    /// What the workers posted (messages, errors, console lines) reaches
    /// the page's scripts.
    fn pump_workers(&mut self) {
        let state = self.state.clone();
        let mut pending: Vec<(u32, worker::FromWorker)> = Vec::new();
        {
            let s = state.borrow();
            for (id, handle) in &s.workers {
                while let Some(m) = handle.try_recv() {
                    pending.push((*id, m));
                }
            }
        }
        if pending.is_empty() {
            return;
        }
        pending.sort_by_key(|(id, _)| *id);
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let Some(name) = v8::String::new(tc, "__olo_workerEvent") else { return };
        let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        for (id, message) in pending {
            let (kind, value): (&str, v8::Local<v8::Value>) = match message {
                worker::FromWorker::Message(bytes) => match clone::deserialize(tc, &bytes) {
                    Some(v) => ("message", v),
                    None => {
                        tc.reset();
                        ("error", string(tc, "message could not be read"))
                    }
                },
                worker::FromWorker::Error(e) => ("error", string(tc, &e)),
                worker::FromWorker::Console(line) => {
                    state.borrow_mut().report.console.push(format!("worker: {line}"));
                    continue;
                }
            };
            let args: [v8::Local<v8::Value>; 3] = [v8::Number::new(tc, f64::from(id)).into(), string(tc, kind), value];
            if f.call(tc, global.into(), &args).is_none() {
                let error = exception_text(tc, "worker event");
                state.borrow_mut().report.errors.push(error);
                tc.reset();
            }
        }
    }
    /// Run one script; an exception is recorded and returned, not fatal.
    pub fn run(&mut self, source: &str, name: &str) -> Result<(), String> {
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Err("stopped".into());
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let result = (|| {
            let code = v8::String::new(tc, source)?;
            let resource = string(tc, name);
            let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
            let script = v8::Script::compile(tc, code, Some(&origin))?;
            script.run(tc)
        })();
        if result.is_none() {
            let error = exception_text(tc, name);
            state.borrow_mut().report.errors.push(error.clone());
            return Err(error);
        }
        Ok(())
    }
    /// Run a module script: its imports fetched and compiled first, then
    /// the graph instantiated and evaluated (see `modules`).
    pub fn run_module(&mut self, source: &str, url: &str) -> Result<(), String> {
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Err("stopped".into());
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        match modules::run(tc, &state, url, Some(source)) {
            Ok(_) => Ok(()),
            Err(error) => {
                state.borrow_mut().report.errors.push(error.clone());
                Err(error)
            }
        }
    }
    /// Advance the page's clock to `now_ms` (milliseconds since its scripts
    /// ran): timers and animation frames due by then run, for at most
    /// `wall`. Returns when the next is due, and whether the document changed.
    pub fn advance(&mut self, now_ms: f64, wall: std::time::Duration) -> Advanced {
        self.advance_frame(now_ms, wall, None)
    }
    /// `advance`, saying whether this moment is a display frame
    /// (`Some(true)`: animation frame callbacks run, as a browser runs them
    /// once a frame; `Some(false)`: only timers and tasks; `None`: frames
    /// on the page's own 16 ms clock). `Advanced::wants_frame` says whether
    /// the page waits for one.
    pub fn advance_frame(&mut self, now_ms: f64, wall: std::time::Duration, frame: Option<bool>) -> Advanced {
        if self.guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Advanced { next_due_ms: None, changed: false, spent_ms: 0., ticks: 0, wants_frame: false };
        }
        let started = Instant::now();
        let before = {
            let mut s = self.state.borrow_mut();
            // Each advance has its own layout budget.
            s.run_layouts = 0;
            s.run_layout_seconds = 0.;
            s.run_limits = (LAYOUTS_PER_ADVANCE, LAYOUT_SECONDS_PER_ADVANCE);
            s.document.revision()
        };
        let mut next_due_ms = None;
        let mut ticks = 0;
        for _ in 0..10_000 {
            ticks += 1;
            self.pump_workers();
            // A frame's callbacks run on its first tick only.
            let as_frame = match frame {
                Some(f) => f64::from(u8::from(f && ticks == 1)),
                None => -1.,
            };
            match self.call_prelude_numbers("__olo_tick", &[now_ms, as_frame]) {
                Ok(next) if next < 0. => {
                    next_due_ms = None;
                    break;
                }
                // Anything later than now waits for its moment: ticking again
                // at this clock runs nothing (a timer 0.3 ms ahead spun this
                // loop to its cap, 10,000 ticks an advance, on stripe.com).
                Ok(next) if next > now_ms => {
                    next_due_ms = Some(next);
                    break;
                }
                // More is due now (scheduled by what just ran).
                Ok(_) => {
                    next_due_ms = Some(now_ms);
                    if started.elapsed() > wall {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let after = self.state.borrow().document.revision();
        let wants_frame = frame.is_some() && self.call_prelude_number("__olo_wants_frame", 0.).is_ok_and(|v| v > 0.);
        Advanced { next_due_ms, changed: after != before, spent_ms: started.elapsed().as_secs_f64() * 1000., ticks, wants_frame }
    }
    /// The last layout made for the scripts' measurements, if any: a render
    /// of the same document keeps its styles and texts.
    /// The navigation the page asked for since the last call (its URL and
    /// whether it replaces the current history entry).
    pub fn take_navigation(&self) -> Option<(String, bool)> {
        self.state.borrow_mut().navigation.take()
    }
    pub fn last_measured(&self) -> Option<Arc<olo_web::layout::Layout>> {
        self.state.borrow().measured.as_ref().map(|m| m.layout().clone())
    }
    /// Deliver input to the page: the events it implies (mouseover/out,
    /// enter/leave, pointer and mouse events, click, focus, keys, wheel,
    /// scroll) dispatched to `target` (the element under the pointer, or
    /// the focused one for keys). Returns whether a default was prevented.
    pub fn input(&mut self, input: &Input, target: Option<NodeId>) -> bool {
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return false;
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        {
            let mut s = state.borrow_mut();
            s.run_layouts = 0;
            s.run_layout_seconds = 0.;
            s.run_limits = (LAYOUTS_PER_ADVANCE, LAYOUT_SECONDS_PER_ADVANCE);
        }
        let (kind, down, key, code, text, dx, dy) = match &input.kind {
            InputKind::Move => ("move", false, "", "", None, 0., 0.),
            InputKind::Down => ("down", true, "", "", None, 0., 0.),
            InputKind::Up => ("up", false, "", "", None, 0., 0.),
            InputKind::Wheel { dx, dy } => ("wheel", false, "", "", None, *dx, *dy),
            InputKind::Key { down, key, code, text } => ("key", *down, key.as_str(), code.as_str(), text.as_deref(), 0., 0.),
            InputKind::Scroll => ("scroll", false, "", "", None, 0., 0.),
        };
        let json = format!(
            "{{\"kind\":\"{kind}\",\"x\":{},\"y\":{},\"scrollY\":{},\"buttons\":{},\"modifiers\":{},\"down\":{down},\"key\":{},\"code\":{},\"text\":{},\"dx\":{dx},\"dy\":{dy}}}",
            input.x,
            input.y,
            input.scroll_y,
            input.buttons,
            input.modifiers,
            json_string(key),
            json_string(code),
            text.map_or("null".to_owned(), json_string)
        );
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let Some(name) = v8::String::new(tc, "__olo_input") else { return false };
        let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return false };
        let json = string(tc, &json);
        let target: v8::Local<v8::Value> = match target.and_then(|n| wrap(tc, &state, n)) {
            Some(o) => o.into(),
            None => v8::null(tc).into(),
        };
        match f.call(tc, global.into(), &[json, target]) {
            Some(v) => v.boolean_value(tc),
            None => {
                let error = exception_text(tc, "input");
                state.borrow_mut().report.errors.push(error);
                false
            }
        }
    }
    /// The canvases the scripts drew on, as images, by element: bound into
    /// the page's images, a canvas renders like an image of its pixels.
    /// Whether a WebGL canvas has a readback in flight (the next render
    /// shows it): the browser renders once more soon.
    pub fn canvases_pending(&self) -> bool {
        self.state.borrow().gl.values().any(webgl::Context::readback_pending)
    }
    /// Animating WebGL canvases that would draw their frames into textures
    /// of the window's renderer (shown with no copy): each with its size and
    /// its GPU's UUID (the window's must match). Each is asked for once.
    /// A person scrolled a box (the wheel over it) to `offset`, already
    /// clamped: the document keeps it, and the box's `scroll` event fires.
    pub fn scroll_box(&mut self, node: NodeId, offset: [f64; 2]) -> bool {
        let moved = self.state.borrow_mut().document.set_scroll_offset(node, offset).unwrap_or(false);
        if moved {
            let guard = self.guard.clone();
            if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                return moved;
            }
            let _busy = Busy::new(&guard);
            let state = self.state.clone();
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc, scope);
            let global = context.global(tc);
            if let Some(element) = wrap(tc, &state, node)
                && let Some(name) = v8::String::new(tc, "__olo_scrolled")
                && let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok())
                && f.call(tc, global.into(), &[element.into()]).is_none()
            {
                let error = exception_text(tc, "scroll");
                state.borrow_mut().report.errors.push(error);
            }
        }
        moved
    }
    /// Counts every box scroll (by a person or a script): a render that
    /// saw it shows the offsets as they are.
    pub fn scroll_serial(&self) -> u64 {
        self.state.borrow().document.scroll_serial()
    }
    /// The page's window shown or not: document.hidden, and visibilitychange
    /// when it changes.
    pub fn set_hidden(&mut self, hidden: bool) {
        let _ = self.call_prelude_number("__olo_set_hidden", f64::from(u8::from(hidden)));
    }
    /// The GPU time the page's WebGL took since last asked (milliseconds).
    pub fn take_gpu_ms(&mut self) -> f64 {
        let mut s = self.state.borrow_mut();
        s.gl.values_mut().map(|c| c.take_gpu_ns()).sum::<u64>() as f64 / 1e6
    }
    /// Why the page's scripts were stopped, if they were: they used the
    /// heap they may, or a task ran past its time. Nothing runs after.
    pub fn stopped(&self) -> Option<&'static str> {
        match self.guard.stopped.load(std::sync::atomic::Ordering::Relaxed) {
            STOPPED_MEMORY => Some("it used all the memory a page may"),
            STOPPED_HUNG => Some("a script ran for more than ten seconds"),
            _ => None,
        }
    }
    pub fn shared_wanted(&mut self) -> Vec<(NodeId, u32, u32, [u8; 16])> {
        let mut s = self.state.borrow_mut();
        s.gl.iter_mut().filter(|(_, c)| c.touched).filter_map(|(n, c)| c.wants_shared().map(|(w, h, uuid)| (*n, w, h, uuid))).collect()
    }
    /// The window's shared textures for canvas `node`: (key, memory, its
    /// size, width, height).
    pub fn provide_shared(&mut self, node: NodeId, textures: Vec<(u64, std::os::fd::OwnedFd, u64, u32, u32)>) -> Vec<u64> {
        let mut s = self.state.borrow_mut();
        match s.gl.get_mut(&node) {
            Some(c) => {
                c.provide_shared(textures);
                c.take_released_shared()
            }
            None => textures.into_iter().map(|t| t.0).collect(),
        }
    }
    /// Canvas `node` shows where the window can't show a shared texture:
    /// its frames are read back from now on.
    pub fn refuse_shared(&mut self, node: NodeId) {
        if let Some(c) = self.state.borrow_mut().gl.get_mut(&node) {
            c.refuse_shared();
        }
    }
    /// The shared textures the canvases let go of, for the window to free.
    pub fn released_shared(&mut self) -> Vec<u64> {
        let mut s = self.state.borrow_mut();
        s.gl.values_mut().flat_map(|c| c.take_released_shared()).collect()
    }
    /// Whether a canvas in the page was drawn on since it was last shown
    /// (a render shows the drawing; nothing else in the document changed).
    pub fn canvases_dirty(&self) -> bool {
        let s = self.state.borrow();
        s.canvases.iter().any(|(_, c)| c.touched && c.dirty) || s.gl.iter().any(|(_, c)| c.touched && c.dirty)
    }
    /// The canvases' states, for the timing log: kind, size, touched,
    /// dirty, and whether a readback is in flight.
    pub fn canvas_states(&self) -> String {
        let s = self.state.borrow();
        let mut out = Vec::new();
        for (n, c) in s.canvases.iter() {
            out.push(format!("2d {:?} {}x{} touched={} dirty={}", n, c.width(), c.height(), c.touched, c.dirty));
        }
        for (n, c) in s.gl.iter() {
            out.push(format!("gl {:?} {}x{} touched={} dirty={} pending={}", n, c.width(), c.height(), c.touched, c.dirty, c.readback_pending()));
        }
        out.join("; ")
    }
    pub fn canvases(&self) -> Vec<(NodeId, Arc<olo_web::image::Image>)> {
        let mut s = self.state.borrow_mut();
        let timing = std::env::var_os("OLO_CANVAS_TIMING").is_some();
        let mut out: Vec<_> = s
            .canvases
            .iter_mut()
            .filter(|(_, c)| c.touched)
            .filter_map(|(n, c)| {
                let t = Instant::now();
                let i = c.image();
                if timing {
                    eprintln!("  canvas 2d {}x{}: {:.3} ms (dirty {})", c.width(), c.height(), t.elapsed().as_secs_f64() * 1000., i.is_some());
                }
                i.map(|i| (*n, i))
            })
            .collect();
        out.extend(s.gl.iter_mut().filter(|(_, c)| c.touched).filter_map(|(n, c)| {
            let t = Instant::now();
            let i = c.image();
            if timing {
                eprintln!("  canvas gl {}x{}: {:.3} ms (shared {})", c.width(), c.height(), t.elapsed().as_secs_f64() * 1000., i.as_ref().is_some_and(|i| i.shared_texture().is_some()));
            }
            i.map(|i| (*n, i))
        }));
        // Images the scripts loaded into `img` elements render too.
        out.extend(s.images.iter().map(|(n, i)| (*n, i.clone())));
        out
    }
    /// The document as it is now, copied, for rendering while the scripts
    /// keep theirs.
    pub fn snapshot(&self) -> Document {
        self.state.borrow().document.clone()
    }
    /// The page rendered revision `rendered`: changes before it, and
    /// before the last measurement, are of no use to anyone (the change
    /// log stays short, and so does every snapshot of the document).
    pub fn forget_changes_before(&self, rendered: u64) {
        let mut s = self.state.borrow_mut();
        let revision = s.measured.as_ref().map_or(rendered, |m| m.revision().min(rendered));
        s.document.forget_changes_before(revision);
    }
    pub fn revision(&self) -> u64 {
        self.state.borrow().document.revision()
    }
    /// What the page did so far (the counts keep growing while it lives).
    pub fn report(&self) -> Report {
        self.finish_report()
    }
    /// Evaluate an expression and return its value as text (objects as JSON).
    pub fn eval(&mut self, source: &str) -> Result<String, String> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let result = (|| {
            let code = v8::String::new(tc, source)?;
            let resource = string(tc, "eval");
            let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
            let script = v8::Script::compile(tc, code, Some(&origin))?;
            script.run(tc)
        })();
        let Some(value) = result else { return Err(exception_text(tc, "eval")) };
        if value.is_string() || value.is_number() || value.is_boolean() || value.is_null_or_undefined() {
            return Ok(value.to_rust_string_lossy(tc));
        }
        match v8::json::stringify(tc, value) {
            Some(json) => Ok(json.to_rust_string_lossy(tc)),
            None => Ok(value.to_rust_string_lossy(tc)),
        }
    }
    /// Run the page's scripts in order, fire the document's lifecycle events
    /// and the timers due within the budget.
    pub fn run_page(&mut self, scripts: &[Script]) -> Report {
        for script in scripts {
            if self.over_time() {
                self.state.borrow_mut().report.errors.push("scripts stopped: out of time".into());
                break;
            }
            self.state.borrow_mut().report.scripts += 1;
            if script.module {
                let _ = self.run_module(&script.source, &script.name);
            } else {
                self.set_current_script(script.node);
                let _ = self.run(&script.source, &script.name);
                self.set_current_script(None);
            }
        }
        let _ = self.call_prelude("__olo_lifecycle", &["DOMContentLoaded"]);
        self.pump();
        let _ = self.call_prelude("__olo_lifecycle", &["load"]);
        self.pump();
        self.finish_report()
    }
    fn over_time(&self) -> bool {
        let s = self.state.borrow();
        s.started.elapsed().as_secs_f64() > s.config.wall_seconds
    }
    /// Run timers due within the virtual budget, in order, without waiting.
    fn pump(&mut self) {
        let budget = self.state.borrow().config.budget_ms;
        let mut now = 0f64;
        for _ in 0..100_000 {
            if self.over_time() {
                break;
            }
            self.pump_workers();
            let Ok(next) = self.call_prelude_number("__olo_tick", now) else { break };
            if next < 0. {
                break;
            }
            now = now.max(next);
            if now > budget {
                break;
            }
        }
    }
    /// CSS animation events for the page's elements (animationstart,
    /// animationend), dispatched as AnimationEvents.
    pub fn animation_events(&mut self, events: &[(NodeId, &str, &str, f64)]) {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let global = context.global(scope);
        let Some(name) = v8::String::new(scope, "__olo_animationEvent") else { return };
        let Some(f) = global.get(scope, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        for (node, kind, animation, elapsed) in events {
            let Some(target) = wrap(scope, &state, *node) else { continue };
            let args: [v8::Local<v8::Value>; 4] = [target.into(), string(scope, kind).into(), string(scope, animation).into(), v8::Number::new(scope, *elapsed).into()];
            f.call(scope, global.into(), &args);
        }
    }
    /// `document.currentScript`: this element, or null.
    fn set_current_script(&mut self, node: Option<NodeId>) {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let global = context.global(scope);
        let Some(name) = v8::String::new(scope, "__olo_setCurrentScript") else { return };
        let Some(f) = global.get(scope, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        let value: v8::Local<v8::Value> = match node.and_then(|n| wrap(scope, &state, n)) {
            Some(o) => o.into(),
            None => v8::null(scope).into(),
        };
        f.call(scope, global.into(), &[value]);
    }
    fn call_prelude(&mut self, function: &str, args: &[&str]) -> Result<(), String> {
        // Under the watchdog, as every task is: a page's load handler that
        // never returns is stopped like any other script.
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Err("stopped".into());
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let name = v8::String::new(tc, function).ok_or("string")?;
        let f = global.get(tc, name.into()).ok_or("missing")?;
        let f = v8::Local::<v8::Function>::try_from(f).map_err(|_| "not a function")?;
        let args: Vec<v8::Local<v8::Value>> = args.iter().map(|a| v8::String::new(tc, a).unwrap().into()).collect();
        if f.call(tc, global.into(), &args).is_none() {
            let error = exception_text(tc, function);
            state.borrow_mut().report.errors.push(error.clone());
            return Err(error);
        }
        Ok(())
    }
    fn call_prelude_number(&mut self, function: &str, arg: f64) -> Result<f64, String> {
        self.call_prelude_numbers(function, &[arg])
    }
    fn call_prelude_numbers(&mut self, function: &str, args: &[f64]) -> Result<f64, String> {
        let guard = self.guard.clone();
        if guard.stopped.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return Err("stopped".into());
        }
        let _busy = Busy::new(&guard);
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let name = v8::String::new(tc, function).ok_or("string")?;
        let f = global.get(tc, name.into()).ok_or("missing")?;
        let f = v8::Local::<v8::Function>::try_from(f).map_err(|_| "not a function")?;
        let args: Vec<v8::Local<v8::Value>> = args.iter().map(|a| v8::Number::new(tc, *a).into()).collect();
        match f.call(tc, global.into(), &args) {
            Some(v) => Ok(v.number_value(tc).unwrap_or(-1.)),
            None => {
                let error = exception_text(tc, function);
                state.borrow_mut().report.errors.push(error.clone());
                Err(error)
            }
        }
    }
    fn finish_report(&self) -> Report {
        let mut s = self.state.borrow_mut();
        s.report.revision_after = s.document.revision();
        s.report.wall_ms = s.started.elapsed().as_secs_f64() * 1000.;
        s.report.clone()
    }
    /// The document, as the scripts left it, and the report.
    pub fn finish(mut self) -> (Document, Report) {
        let report = self.finish_report();
        // Drop every handle before the isolate.
        {
            let mut s = self.state.borrow_mut();
            s.wrappers.clear();
            s.templates = None;
            s.measure = None;
            s.measured = None;
        }
        modules::leave();
        let state = std::mem::replace(&mut self.state, Rc::new(RefCell::new(State::empty())));
        drop(self);
        let state = Rc::try_unwrap(state).ok().expect("the realm's state is only its own").into_inner();
        for id in state.ids {
            // SAFETY: each pointer came from Box::into_raw in `wrap` and is
            // freed exactly once, here, after every V8 object that held it.
            drop(unsafe { Box::from_raw(id) });
        }
        (state.document, report)
    }
}
impl State {
    fn empty() -> Self {
        Self {
            document: Document::new(Arc::from(""), Default::default()).expect("an empty document"),
            config: Config::default(),
            quirks: QuirksMode::NoQuirks,
            wrappers: HashMap::new(),
            ids: Vec::new(),
            templates: None,
            fetcher: None,
            canvases: HashMap::new(),
            gl: HashMap::new(),
            gl_calls: 0,
            gl_seconds: 0.,
            selectors: HashMap::new(),
            image_lookup: None,
            images: HashMap::new(),
            requests: Vec::new(),
            in_flight: HashMap::new(),
            modules: Default::default(),
            navigation: None,
            next_request: 1,
            workers: HashMap::new(),
            next_worker: 1,
            worker_fetcher: None,
            waker: None,
            measure: None,
            measured: None,
            layout_seconds: 0.,
            run_layouts: 0,
            run_layout_seconds: 0.,
            run_limits: (LAYOUTS, LAYOUT_SECONDS),
            report: Report::default(),
            started: Instant::now(),
        }
    }
    /// The layout to measure against: the last one while the document is
    /// unchanged, else a new one within the layout budget.
    fn measured(&mut self) -> Option<&olo_web::page::Measured> {
        let revision = self.document.revision();
        let stale = self.measured.as_ref().is_none_or(|m| m.revision() != revision);
        if stale && self.measure.is_some() && self.run_layouts < self.run_limits.0 && self.run_layout_seconds < self.run_limits.1 {
            let started = Instant::now();
            let measure = self.measure.as_mut()?;
            // The previous measurement is consumed: patched in place when
            // little changed, else seeding the next.
            self.measured = measure(&self.document, self.measured.take());
            let took = started.elapsed().as_secs_f64();
            self.layout_seconds += took;
            self.run_layout_seconds += took;
            self.run_layouts += 1;
            self.report.layouts += 1;
            self.report.layout_ms += took * 1000.;
        }
        self.measured.as_ref()
    }
}
pub(crate) fn exception_text(tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, '_, v8::HandleScope<'_>>>, name: &str) -> String {
    let mut text = String::new();
    if let Some(e) = tc.exception() {
        text.push_str(&e.to_rust_string_lossy(tc));
    }
    if let Some(m) = tc.message() {
        let line = m.get_line_number(tc).unwrap_or(0);
        text.push_str(&format!(" ({name}:{line})"));
    } else {
        text.push_str(&format!(" ({name})"));
    }
    if let Some(trace) = tc.stack_trace() {
        let trace = trace.to_rust_string_lossy(tc);
        if !trace.is_empty() && trace.len() < 2000 {
            text.push_str("\n  ");
            text.push_str(&trace.replace('\n', "\n  "));
        }
    }
    text
}
/// The DOM class of an element by its tag, for the prototype of its
/// wrapper. Sorted by tag for the lookup; tags not listed are HTMLElement
/// (or SVGElement) itself. Unknown HTML tags are HTMLUnknownElement.
const HTML_CLASSES: &[(&str, &str)] = &[
    ("a", "HTMLAnchorElement"),
    ("abbr", "HTMLElement"),
    ("address", "HTMLElement"),
    ("area", "HTMLAreaElement"),
    ("article", "HTMLElement"),
    ("aside", "HTMLElement"),
    ("audio", "HTMLAudioElement"),
    ("b", "HTMLElement"),
    ("base", "HTMLBaseElement"),
    ("bdi", "HTMLElement"),
    ("bdo", "HTMLElement"),
    ("blockquote", "HTMLQuoteElement"),
    ("body", "HTMLBodyElement"),
    ("br", "HTMLBRElement"),
    ("button", "HTMLButtonElement"),
    ("canvas", "HTMLCanvasElement"),
    ("caption", "HTMLTableCaptionElement"),
    ("cite", "HTMLElement"),
    ("code", "HTMLElement"),
    ("col", "HTMLTableColElement"),
    ("colgroup", "HTMLTableColElement"),
    ("data", "HTMLDataElement"),
    ("datalist", "HTMLDataListElement"),
    ("dd", "HTMLElement"),
    ("del", "HTMLModElement"),
    ("details", "HTMLDetailsElement"),
    ("dfn", "HTMLElement"),
    ("dialog", "HTMLDialogElement"),
    ("div", "HTMLDivElement"),
    ("dl", "HTMLDListElement"),
    ("dt", "HTMLElement"),
    ("em", "HTMLElement"),
    ("embed", "HTMLEmbedElement"),
    ("fieldset", "HTMLFieldSetElement"),
    ("figcaption", "HTMLElement"),
    ("figure", "HTMLElement"),
    ("font", "HTMLFontElement"),
    ("footer", "HTMLElement"),
    ("form", "HTMLFormElement"),
    ("frame", "HTMLFrameElement"),
    ("frameset", "HTMLFrameSetElement"),
    ("h1", "HTMLHeadingElement"),
    ("h2", "HTMLHeadingElement"),
    ("h3", "HTMLHeadingElement"),
    ("h4", "HTMLHeadingElement"),
    ("h5", "HTMLHeadingElement"),
    ("h6", "HTMLHeadingElement"),
    ("head", "HTMLHeadElement"),
    ("header", "HTMLElement"),
    ("hgroup", "HTMLElement"),
    ("hr", "HTMLHRElement"),
    ("html", "HTMLHtmlElement"),
    ("i", "HTMLElement"),
    ("iframe", "HTMLIFrameElement"),
    ("img", "HTMLImageElement"),
    ("input", "HTMLInputElement"),
    ("ins", "HTMLModElement"),
    ("kbd", "HTMLElement"),
    ("label", "HTMLLabelElement"),
    ("legend", "HTMLLegendElement"),
    ("li", "HTMLLIElement"),
    ("link", "HTMLLinkElement"),
    ("main", "HTMLElement"),
    ("map", "HTMLMapElement"),
    ("mark", "HTMLElement"),
    ("marquee", "HTMLMarqueeElement"),
    ("menu", "HTMLMenuElement"),
    ("meta", "HTMLMetaElement"),
    ("meter", "HTMLMeterElement"),
    ("nav", "HTMLElement"),
    ("noscript", "HTMLElement"),
    ("object", "HTMLObjectElement"),
    ("ol", "HTMLOListElement"),
    ("optgroup", "HTMLOptGroupElement"),
    ("option", "HTMLOptionElement"),
    ("output", "HTMLOutputElement"),
    ("p", "HTMLParagraphElement"),
    ("param", "HTMLParamElement"),
    ("picture", "HTMLPictureElement"),
    ("pre", "HTMLPreElement"),
    ("progress", "HTMLProgressElement"),
    ("q", "HTMLQuoteElement"),
    ("rp", "HTMLElement"),
    ("rt", "HTMLElement"),
    ("ruby", "HTMLElement"),
    ("s", "HTMLElement"),
    ("samp", "HTMLElement"),
    ("script", "HTMLScriptElement"),
    ("search", "HTMLElement"),
    ("section", "HTMLElement"),
    ("select", "HTMLSelectElement"),
    ("slot", "HTMLSlotElement"),
    ("small", "HTMLElement"),
    ("source", "HTMLSourceElement"),
    ("span", "HTMLSpanElement"),
    ("strong", "HTMLElement"),
    ("style", "HTMLStyleElement"),
    ("sub", "HTMLElement"),
    ("summary", "HTMLElement"),
    ("sup", "HTMLElement"),
    ("table", "HTMLTableElement"),
    ("tbody", "HTMLTableSectionElement"),
    ("td", "HTMLTableCellElement"),
    ("template", "HTMLTemplateElement"),
    ("textarea", "HTMLTextAreaElement"),
    ("tfoot", "HTMLTableSectionElement"),
    ("th", "HTMLTableCellElement"),
    ("thead", "HTMLTableSectionElement"),
    ("time", "HTMLTimeElement"),
    ("title", "HTMLTitleElement"),
    ("tr", "HTMLTableRowElement"),
    ("track", "HTMLTrackElement"),
    ("u", "HTMLElement"),
    ("ul", "HTMLUListElement"),
    ("var", "HTMLElement"),
    ("video", "HTMLVideoElement"),
    ("wbr", "HTMLElement"),
];
const SVG_CLASSES: &[(&str, &str)] = &[
    ("circle", "SVGCircleElement"),
    ("clipPath", "SVGClipPathElement"),
    ("defs", "SVGDefsElement"),
    ("ellipse", "SVGEllipseElement"),
    ("g", "SVGGElement"),
    ("image", "SVGImageElement"),
    ("line", "SVGLineElement"),
    ("linearGradient", "SVGLinearGradientElement"),
    ("mask", "SVGMaskElement"),
    ("path", "SVGPathElement"),
    ("pattern", "SVGPatternElement"),
    ("polygon", "SVGPolygonElement"),
    ("polyline", "SVGPolylineElement"),
    ("radialGradient", "SVGRadialGradientElement"),
    ("rect", "SVGRectElement"),
    ("stop", "SVGStopElement"),
    ("svg", "SVGSVGElement"),
    ("symbol", "SVGSymbolElement"),
    ("text", "SVGTextElement"),
    ("tspan", "SVGTSpanElement"),
    ("use", "SVGUseElement"),
];
fn tag_class(name: &str, namespace: Option<Namespace>) -> Option<&'static str> {
    match namespace {
        Some(Namespace::Html) => Some(match HTML_CLASSES.binary_search_by_key(&name, |(t, _)| t) {
            Ok(i) => HTML_CLASSES[i].1,
            // Custom elements (with a dash) are HTMLElement; the rest unknown.
            Err(_) if name.contains('-') => "HTMLElement",
            Err(_) => "HTMLUnknownElement",
        }),
        Some(Namespace::Svg) => SVG_CLASSES.binary_search_by_key(&name, |(t, _)| t).ok().map(|i| SVG_CLASSES[i].1),
        _ => None,
    }
}
/// `window.NAME` for an element with that id, or a form/img/iframe/embed/
/// object with that name; the first in document order. Names that scripts
/// probe for features (undefined checks) stay undefined when no element
/// has them.
fn named_global(scope: &mut v8::PinScope, key: v8::Local<v8::Name>, _args: v8::PropertyCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) -> v8::Intercepted {
    let Some(state) = scope.get_slot::<Shared>().cloned() else { return v8::Intercepted::kNo };
    let Ok(key) = v8::Local::<v8::String>::try_from(key) else { return v8::Intercepted::kNo };
    let name = key.to_rust_string_lossy(scope);
    if name.is_empty() || name.len() > 256 {
        return v8::Intercepted::kNo;
    }
    let found = {
        let s = state.borrow();
        find_by_id(&s.document, &name).or_else(|| find_by_name(&s.document, &name))
    };
    match found.and_then(|n| wrap(scope, &state, n)) {
        Some(obj) => {
            rv.set(obj.into());
            v8::Intercepted::kYes
        }
        None => v8::Intercepted::kNo,
    }
}
/// Perform one canvas call; `arguments` is a JS array. Returns a value for
/// the few calls that answer (gradient ids, image data).
fn canvas_op<'s>(scope: &mut v8::PinScope<'s, '_>, state: &Shared, node: NodeId, op: &str, arguments: v8::Local<v8::Value>) -> Option<v8::Local<'s, v8::Value>> {
    let array = v8::Local::<v8::Array>::try_from(arguments).ok();
    let count = array.map_or(0, |a| a.length());
    let num = |scope: &mut v8::PinScope, i: u32| -> f32 { array.and_then(|a| a.get_index(scope, i)).and_then(|v| v.number_value(scope)).unwrap_or(0.) as f32 };
    let text = |scope: &mut v8::PinScope, i: u32| -> String { array.and_then(|a| a.get_index(scope, i)).map(|v| v.to_rust_string_lossy(scope)).unwrap_or_default() };
    let nums: Vec<f32> = (0..count.min(16)).map(|i| num(scope, i)).collect();
    let n = |i: usize| nums.get(i).copied().unwrap_or(0.);
    // drawImage sources need the other canvas's pixels before this one is
    // borrowed mutably.
    if op == "drawImage" {
        let source = array.and_then(|a| a.get_index(scope, 0)).and_then(|v| node_of(scope, v));
        let pixels = {
            let mut s = state.borrow_mut();
            match source {
                Some(src) if s.canvases.contains_key(&src) => s.canvases.get(&src).map(|c| (c.pixels().to_vec(), c.width(), c.height())),
                Some(src) if s.images.contains_key(&src) => s.images.get(&src).map(|i| (i.rgba().to_vec(), i.width(), i.height())),
                Some(src) => s.gl.get_mut(&src).and_then(|c| c.image()).map(|i| (i.rgba().to_vec(), i.width(), i.height())),
                None => None,
            }
        };
        let Some((pixels, sw, sh)) = pixels else { return None };
        let mut s = state.borrow_mut();
        let c = s.canvases.get_mut(&node)?;
        // drawImage(image, dx, dy), (image, dx, dy, dw, dh), (image, sx, sy, sw, sh, dx, dy, dw, dh)
        let (sx, sy, sw2, sh2, dx, dy, dw, dh) = match count {
            3 => (0., 0., sw as f32, sh as f32, n(1), n(2), sw as f32, sh as f32),
            5 => (0., 0., sw as f32, sh as f32, n(1), n(2), n(3), n(4)),
            _ => (n(1), n(2), n(3), n(4), n(5), n(6), n(7), n(8)),
        };
        c.draw_image(&pixels, sw, sh, sx, sy, sw2, sh2, dx, dy, dw, dh);
        return None;
    }
    let mut s = state.borrow_mut();
    let c = s.canvases.get_mut(&node)?;
    match op {
        "save" => c.save(),
        "restore" => c.restore(),
        "reset" => c.reset_state(),
        "scale" => c.transform(tiny_skia::Transform::from_scale(n(0), n(1))),
        "rotate" => c.transform(tiny_skia::Transform::from_rotate(n(0).to_degrees())),
        "translate" => c.transform(tiny_skia::Transform::from_translate(n(0), n(1))),
        "transform" => c.transform(tiny_skia::Transform::from_row(n(0), n(1), n(2), n(3), n(4), n(5))),
        "setTransform" => {
            if count >= 6 {
                c.set_transform(tiny_skia::Transform::from_row(n(0), n(1), n(2), n(3), n(4), n(5)))
            } else {
                c.set_transform(tiny_skia::Transform::identity())
            }
        }
        "resetTransform" => c.set_transform(tiny_skia::Transform::identity()),
        "getTransform" => {
            let t = c.current_transform();
            drop(s);
            let items: Vec<v8::Local<v8::Value>> = [t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].iter().map(|v| v8::Number::new(scope, f64::from(*v)).into()).collect();
            return Some(v8::Array::new_with_elements(scope, &items).into());
        }
        "beginPath" => c.begin_path(),
        "closePath" => c.close_path(),
        "moveTo" => c.move_to(n(0), n(1)),
        "lineTo" => c.line_to(n(0), n(1)),
        "bezierCurveTo" => c.bezier_to(n(0), n(1), n(2), n(3), n(4), n(5)),
        "quadraticCurveTo" => c.quad_to(n(0), n(1), n(2), n(3)),
        "arc" => c.arc(n(0), n(1), n(2), n(3), n(4), count > 5 && n(5) != 0.),
        "arcTo" => c.arc_to_point(n(0), n(1), n(2), n(3), n(4)),
        "ellipse" => c.ellipse(n(0), n(1), n(2), n(3), n(4), n(5), n(6), count > 7 && n(7) != 0.),
        "rect" => c.rect(n(0), n(1), n(2), n(3)),
        "roundRect" => c.round_rect(n(0), n(1), n(2), n(3), n(4)),
        "fill" => c.fill(if count > 0 && text(scope, 0) == "evenodd" { tiny_skia::FillRule::EvenOdd } else { tiny_skia::FillRule::Winding }),
        "stroke" => c.stroke(),
        "clip" => c.clip(if count > 0 && text(scope, 0) == "evenodd" { tiny_skia::FillRule::EvenOdd } else { tiny_skia::FillRule::Winding }),
        "fillRect" => c.fill_rect(n(0), n(1), n(2), n(3)),
        "strokeRect" => c.stroke_rect(n(0), n(1), n(2), n(3)),
        "clearRect" => c.clear_rect(n(0), n(1), n(2), n(3)),
        "lineWidth" => c.set_line_width(n(0)),
        "lineCap" => {
            let v = text(scope, 0);
            c.set_line_cap(&v)
        }
        "lineJoin" => {
            let v = text(scope, 0);
            c.set_line_join(&v)
        }
        "miterLimit" => c.set_miter_limit(n(0)),
        "setLineDash" => {
            let offset = n(0);
            let dash: Vec<f32> = nums.iter().skip(1).copied().collect();
            c.set_dash(dash, offset)
        }
        "globalAlpha" => c.set_alpha(n(0)),
        "imageSmoothingEnabled" => c.set_smoothing(n(0) != 0.),
        "globalCompositeOperation" => {
            let v = text(scope, 0);
            c.set_composite(&v)
        }
        "fillStyle" | "strokeStyle" => {
            let fill = op == "fillStyle";
            let value = array.and_then(|a| a.get_index(scope, 0));
            let style = match value {
                Some(v) if v.is_number() => canvas::StyleValue::Gradient(v.number_value(scope).unwrap_or(0.) as u32),
                Some(v) => canvas::StyleValue::Color(v.to_rust_string_lossy(scope)),
                None => return None,
            };
            c.set_style(fill, style)
        }
        "createLinearGradient" => {
            let id = c.linear_gradient(n(0), n(1), n(2), n(3));
            drop(s);
            return Some(v8::Number::new(scope, f64::from(id)).into());
        }
        "createRadialGradient" => {
            let id = c.radial_gradient(n(0), n(1), n(2), n(3), n(4), n(5));
            drop(s);
            return Some(v8::Number::new(scope, f64::from(id)).into());
        }
        "addColorStop" => {
            let color = text(scope, 2);
            c.add_color_stop(n(0) as u32, n(1), &color)
        }
        "getImageData" => {
            let bytes = c.image_data(n(0) as i32, n(1) as i32, n(2).max(0.) as u32, n(3).max(0.) as u32);
            drop(s);
            let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
            let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
            return Some(buffer.into());
        }
        "putImageData" => {
            // (data as an ArrayBuffer, width, height, dx, dy)
            let data = array.and_then(|a| a.get_index(scope, 0)).and_then(|v| v8::Local::<v8::ArrayBuffer>::try_from(v).ok());
            let Some(data) = data else { return None };
            let bytes: Vec<u8> = {
                let store = data.get_backing_store();
                store.iter().map(|b| b.get()).collect()
            };
            c.put_image_data(&bytes, n(1) as u32, n(2) as u32, n(3) as i32, n(4) as i32)
        }
        _ => {}
    }
    None
}
/// A JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
/// Attribute names are case-insensitive on HTML elements (stored in lower
/// case) and case-sensitive elsewhere (SVG's viewBox).
fn attribute_name(state: &Shared, node: NodeId, name: String) -> String {
    let html = state.borrow().document.namespace(node).ok().flatten() == Some(Namespace::Html);
    if html && name.bytes().any(|b| b.is_ascii_uppercase()) { name.to_ascii_lowercase() } else { name }
}
/// The first form, img, iframe, embed or object with this `name`.
fn find_by_name(doc: &Document, name: &str) -> Option<NodeId> {
    let mut stack = vec![doc.root()];
    let mut seen = 0usize;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > 1 << 20 {
            return None;
        }
        if doc.node_type(n) == Ok(NodeType::Element) && matches!(doc.name(n), Ok(Some("form" | "img" | "iframe" | "embed" | "object"))) && doc.attribute(n, "name") == Ok(Some(name)) {
            return Some(n);
        }
        let Ok(children) = doc.children(n) else { continue };
        let children: Vec<NodeId> = children.collect();
        stack.extend(children.into_iter().rev());
    }
    None
}
/// One DOM class: a constructor nobody may call, with an internal field for
/// the node, installed on the global object.
fn class<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
    parent: Option<v8::Local<'s, v8::FunctionTemplate>>,
) -> Result<v8::Local<'s, v8::FunctionTemplate>, String> {
    let data: v8::Local<v8::Value> = v8::String::new(scope, name).ok_or("string")?.into();
    let t = v8::FunctionTemplate::builder(illegal_constructor).data(data).build(scope);
    let class_name = v8::String::new(scope, name).ok_or("string")?;
    t.set_class_name(class_name);
    t.instance_template(scope).set_internal_field_count(1);
    if let Some(p) = parent {
        t.inherit(p);
    }
    let f = t.get_function(scope).ok_or("function")?;
    let key = v8::String::new(scope, name).ok_or("string")?;
    global.set(scope, key.into(), f.into());
    Ok(t)
}
/// The native functions and the DOM classes, on the global object.
fn install(scope: &mut v8::PinScope<'_, '_>, state: &Shared) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);
    let natives = v8::Object::new(scope);
    for name in NATIVES {
        let data: v8::Local<v8::Value> = v8::String::new(scope, name).ok_or("string")?.into();
        let f = v8::FunctionTemplate::builder(native).data(data).build(scope);
        let f = f.get_function(scope).ok_or("function")?;
        let key = v8::String::new(scope, name).ok_or("string")?;
        natives.set(scope, key.into(), f.into());
    }
    let key = v8::String::new(scope, "__olo").ok_or("string")?;
    global.set(scope, key.into(), natives.into());
    // The DOM classes: constructors whose prototypes the prelude fills.
    let event_target = class(scope, global, "EventTarget", None)?;
    let node = class(scope, global, "Node", Some(event_target))?;
    let element = class(scope, global, "Element", Some(node))?;
    let html_element = class(scope, global, "HTMLElement", Some(element))?;
    let svg_element = class(scope, global, "SVGElement", Some(element))?;
    let text = class(scope, global, "Text", Some(node))?;
    let comment = class(scope, global, "Comment", Some(node))?;
    let document = class(scope, global, "Document", Some(node))?;
    let fragment = class(scope, global, "DocumentFragment", Some(node))?;
    state.borrow_mut().templates = Some(Templates {
        event_target: v8::Global::new(scope, event_target),
        node: v8::Global::new(scope, node),
        element: v8::Global::new(scope, element),
        html_element: v8::Global::new(scope, html_element),
        svg_element: v8::Global::new(scope, svg_element),
        text: v8::Global::new(scope, text),
        comment: v8::Global::new(scope, comment),
        document: v8::Global::new(scope, document),
        fragment: v8::Global::new(scope, fragment),
    });
    Ok(())
}
fn illegal_constructor(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let message = v8::String::new(scope, "Illegal constructor").unwrap();
    let exception = v8::Exception::type_error(scope, message);
    scope.throw_exception(exception);
}
const NATIVES: &[&str] = &[
    "nodeType",
    "nodeName",
    "namespace",
    "parent",
    "firstChild",
    "lastChild",
    "nextSibling",
    "previousSibling",
    "childNodes",
    "children",
    "data",
    "setData",
    "textContent",
    "setTextContent",
    "attr",
    "setAttr",
    "removeAttr",
    "attrNames",
    "append",
    "insertBefore",
    "remove",
    "createElement",
    "createText",
    "createComment",
    "createFragment",
    "templateContents",
    "clone",
    "query",
    "matches",
    "layoutGeneration",
    "wallNow",
    "navigate",
    "controlValue",
    "setControlValue",
    "controlChecked",
    "setControlChecked",
    "cssProperties",
    "innerHTML",
    "setInnerHTML",
    "outerHTML",
    "docRoot",
    "byId",
    "isConnected",
    "log",
    "fetchSync",
    "runScript",
    "canvasSize",
    "canvasOp",
    "canvasPixels",
    "loadImage",
    "fetchAsync",
    "structuredClone",
    "workerStart",
    "workerPost",
    "workerTerminate",
    "imageSize",
    "imageSource",
    "matchMedia",
    "scrollOffset",
    "setScrollOffset",
    "imagePixels",
    "glCreate",
    "glOp",
    "rect",
    "paddingRect",
    "scrollSize",
    "computed",
    "docSize",
    "revision",
    "url",
    "viewport",
    "userAgent",
    "language",
    "count",
];
fn throw(scope: &mut v8::PinScope, message: &str) {
    let message = v8::String::new(scope, message).unwrap();
    let exception = v8::Exception::type_error(scope, message);
    scope.throw_exception(exception);
}
/// The node a wrapper object holds.
fn node_of(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Option<NodeId> {
    let object = value.try_cast::<v8::Object>().ok()?;
    if object.internal_field_count() < 1 {
        return None;
    }
    let field = object.get_internal_field(scope, 0)?;
    let external = field.try_cast::<v8::External>().ok()?;
    let pointer = external.value() as *const NodeId;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the pointer was made in `wrap` from a Box the state keeps until
    // the realm ends, after every wrapper is dropped.
    Some(unsafe { *pointer })
}
/// The wrapper object of a node: one per node, for identity.
fn wrap<'s>(scope: &mut v8::PinScope<'s, '_>, state: &Shared, node: NodeId) -> Option<v8::Local<'s, v8::Object>> {
    if let Some(existing) = state.borrow().wrappers.get(&node) {
        return Some(v8::Local::new(scope, existing));
    }
    let (kind, name, namespace) = {
        let s = state.borrow();
        let doc = &s.document;
        let kind = doc.node_type(node).ok()?;
        (kind, doc.name(node).ok().flatten().map(|n| n.to_owned()), doc.namespace(node).ok().flatten())
    };
    let template = {
        let s = state.borrow();
        let t = s.templates.as_ref()?;
        let chosen = match kind {
            NodeType::Element => match namespace {
                Some(Namespace::Html) => &t.html_element,
                Some(Namespace::Svg) => &t.svg_element,
                _ => &t.element,
            },
            NodeType::Text => &t.text,
            NodeType::Comment | NodeType::ProcessingInstruction => &t.comment,
            NodeType::Document => &t.document,
            NodeType::Fragment => &t.fragment,
            NodeType::Doctype => &t.node,
        };
        v8::Local::new(scope, chosen)
    };
    let object = template.instance_template(scope).new_instance(scope)?;
    // HTML elements get their tag's class (HTMLDivElement, …) when the
    // prelude defined one: its prototype replaces HTMLElement's.
    if kind == NodeType::Element
        && let Some(name) = name.as_deref()
        && let Some(class) = tag_class(name, namespace)
    {
        let global = scope.get_current_context().global(scope);
        let class_key = v8::String::new(scope, class)?;
        if let Some(ctor) = global.get(scope, class_key.into()).and_then(|c| v8::Local::<v8::Object>::try_from(c).ok()) {
            let proto_key = v8::String::new(scope, "prototype")?;
            if let Some(proto) = ctor.get(scope, proto_key.into()) {
                object.set_prototype(scope, proto);
            }
        }
    }
    let boxed = Box::into_raw(Box::new(node));
    let external = v8::External::new(scope, boxed as *mut c_void);
    object.set_internal_field(0, external.into());
    let mut s = state.borrow_mut();
    s.ids.push(boxed);
    s.wrappers.insert(node, v8::Global::new(scope, object));
    Some(object)
}
pub(crate) fn string<'s>(scope: &mut v8::PinScope<'s, '_>, s: &str) -> v8::Local<'s, v8::Value> {
    v8::String::new(scope, s).map_or_else(|| v8::undefined(scope).into(), |v| v.into())
}
/// A script value as a GL argument: numbers (booleans as 0/1), strings,
/// typed arrays and buffers as their bytes.
fn gl_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> webgl::Arg {
    use webgl::Kind;
    if v.is_null_or_undefined() {
        return webgl::Arg::Null;
    }
    if v.is_number() || v.is_boolean() {
        return webgl::Arg::Num(v.number_value(scope).unwrap_or(0.));
    }
    if v.is_string() {
        return webgl::Arg::Str(v.to_rust_string_lossy(scope));
    }
    if let Ok(view) = v.try_cast::<v8::ArrayBufferView>() {
        let kind = if v.is_float32_array() {
            Kind::F32
        } else if v.is_int32_array() {
            Kind::I32
        } else if v.is_uint32_array() {
            Kind::U32
        } else if v.is_int16_array() {
            Kind::I16
        } else if v.is_uint16_array() {
            Kind::U16
        } else if v.is_int8_array() {
            Kind::I8
        } else if v.is_uint8_array() || v.is_uint8_clamped_array() {
            Kind::U8
        } else {
            Kind::Raw
        };
        let mut bytes = vec![0u8; view.byte_length()];
        view.copy_contents(&mut bytes);
        return webgl::Arg::Bytes(bytes, kind);
    }
    if let Ok(buffer) = v.try_cast::<v8::ArrayBuffer>() {
        let len = buffer.byte_length();
        // SAFETY: the buffer's store is `len` bytes long while it is alive.
        let bytes = buffer.data().map_or_else(Vec::new, |p| unsafe { std::slice::from_raw_parts(p.as_ptr() as *const u8, len) }.to_vec());
        return webgl::Arg::Bytes(bytes, Kind::Raw);
    }
    if let Ok(array) = v.try_cast::<v8::Array>() {
        let bytes = (0..array.length()).flat_map(|i| (array.get_index(scope, i).and_then(|x| x.number_value(scope)).unwrap_or(0.) as f32).to_le_bytes()).collect();
        return webgl::Arg::Bytes(bytes, Kind::F32);
    }
    webgl::Arg::Null
}
/// Premultiplied RGBA8 as straight RGBA8.
fn unpremultiply(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}
#[derive(Clone, Copy)]
pub(crate) enum ByteKind {
    U8,
    I32,
    F32,
}
/// Bytes as a typed array of the given kind over a fresh buffer.
pub(crate) fn bytes_array<'s>(scope: &mut v8::PinScope<'s, '_>, bytes: Vec<u8>, kind: ByteKind) -> v8::Local<'s, v8::Value> {
    let len = bytes.len();
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    match kind {
        ByteKind::U8 => v8::Uint8Array::new(scope, buffer, 0, len).map_or_else(|| v8::undefined(scope).into(), |a| a.into()),
        ByteKind::I32 => v8::Int32Array::new(scope, buffer, 0, len / 4).map_or_else(|| v8::undefined(scope).into(), |a| a.into()),
        ByteKind::F32 => v8::Float32Array::new(scope, buffer, 0, len / 4).map_or_else(|| v8::undefined(scope).into(), |a| a.into()),
    }
}
fn arg_string(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> String {
    let v = args.get(i);
    if v.is_null_or_undefined() { String::new() } else { v.to_rust_string_lossy(scope) }
}
fn nodes_array<'s>(scope: &mut v8::PinScope<'s, '_>, state: &Shared, nodes: &[NodeId]) -> v8::Local<'s, v8::Value> {
    let items: Vec<v8::Local<v8::Value>> = nodes.iter().filter_map(|n| wrap(scope, state, *n).map(|o| o.into())).collect();
    v8::Array::new_with_elements(scope, &items).into()
}
/// Every `__olo.*` function: `args.data()` names it.
thread_local! {
    /// OLO_JS_PROFILE: time spent in each native, by name (calls, seconds).
    static NATIVE_PROFILE: std::cell::RefCell<std::collections::HashMap<String, (u64, f64)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}
fn native_profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_JS_PROFILE").is_some())
}
/// The natives' profile so far, taken: by time spent, descending.
pub fn take_native_profile() -> Vec<(String, u64, f64)> {
    let mut v: Vec<_> = NATIVE_PROFILE.with(|p| std::mem::take(&mut *p.borrow_mut())).into_iter().map(|(k, (n, t))| (k, n, t)).collect();
    v.sort_by(|a, b| b.2.total_cmp(&a.2));
    v
}
fn native(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    if !native_profiling() {
        return native_inner(scope, args, rv);
    }
    let name = args.data().to_rust_string_lossy(scope);
    let started = Instant::now();
    native_inner(scope, args, rv);
    let spent = started.elapsed().as_secs_f64();
    NATIVE_PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        let e = p.entry(name).or_insert((0, 0.));
        e.0 += 1;
        e.1 += spent;
    });
}
fn native_inner(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(state) = scope.get_slot::<Shared>().cloned() else { return };
    let name = args.data().to_rust_string_lossy(scope);
    let node = |scope: &mut v8::PinScope, i: i32| node_of(scope, args.get(i));
    match name.as_str() {
        "nodeType" => {
            if let Some(n) = node(scope, 0) {
                let t = state.borrow().document.node_type(n).ok();
                rv.set_int32(match t {
                    Some(NodeType::Element) => 1,
                    Some(NodeType::Text) => 3,
                    Some(NodeType::ProcessingInstruction) => 7,
                    Some(NodeType::Comment) => 8,
                    Some(NodeType::Document) => 9,
                    Some(NodeType::Doctype) => 10,
                    Some(NodeType::Fragment) => 11,
                    None => 0,
                });
            }
        }
        "nodeName" => {
            if let Some(n) = node(scope, 0) {
                let s = state.borrow();
                let doc = &s.document;
                let text = match doc.node_type(n) {
                    Ok(NodeType::Element) => {
                        let name = doc.name(n).ok().flatten().unwrap_or("");
                        if doc.namespace(n).ok().flatten() == Some(Namespace::Html) { name.to_ascii_uppercase() } else { name.to_owned() }
                    }
                    Ok(NodeType::Text) => "#text".into(),
                    Ok(NodeType::Comment) => "#comment".into(),
                    Ok(NodeType::Document) => "#document".into(),
                    Ok(NodeType::Fragment) => "#document-fragment".into(),
                    Ok(NodeType::Doctype) => doc.doctype(n).ok().flatten().map(|d| d.name.to_owned()).unwrap_or_default(),
                    Ok(NodeType::ProcessingInstruction) => doc.name(n).ok().flatten().unwrap_or("").to_owned(),
                    Err(_) => String::new(),
                };
                drop(s);
                rv.set(string(scope, &text));
            }
        }
        "namespace" => {
            if let Some(n) = node(scope, 0) {
                let ns = state.borrow().document.namespace(n).ok().flatten();
                let text = match ns {
                    Some(Namespace::Html) => "http://www.w3.org/1999/xhtml",
                    Some(Namespace::Svg) => "http://www.w3.org/2000/svg",
                    Some(Namespace::MathMl) => "http://www.w3.org/1998/Math/MathML",
                    None => "",
                };
                rv.set(string(scope, text));
            }
        }
        "parent" | "firstChild" | "lastChild" | "nextSibling" | "previousSibling" => {
            if let Some(n) = node(scope, 0) {
                let related = {
                    let s = state.borrow();
                    let doc = &s.document;
                    match name.as_str() {
                        "parent" => doc.parent(n),
                        "firstChild" => doc.first_child(n),
                        "lastChild" => doc.last_child(n),
                        "nextSibling" => doc.next_sibling(n),
                        _ => doc.previous_sibling(n),
                    }
                    .ok()
                    .flatten()
                };
                match related.and_then(|r| wrap(scope, &state, r)) {
                    Some(o) => rv.set(o.into()),
                    None => rv.set_null(),
                }
            }
        }
        "childNodes" | "children" => {
            if let Some(n) = node(scope, 0) {
                let elements_only = name == "children";
                let nodes: Vec<NodeId> = {
                    let s = state.borrow();
                    let doc = &s.document;
                    let mut out = Vec::new();
                    let mut child = doc.first_child(n).ok().flatten();
                    while let Some(c) = child {
                        if !elements_only || doc.node_type(c).ok() == Some(NodeType::Element) {
                            out.push(c);
                        }
                        child = doc.next_sibling(c).ok().flatten();
                    }
                    out
                };
                rv.set(nodes_array(scope, &state, &nodes));
            }
        }
        "data" => {
            if let Some(n) = node(scope, 0) {
                let text = state.borrow().document.value(n).ok().flatten().map(|t| t.to_owned()).unwrap_or_default();
                rv.set(string(scope, &text));
            }
        }
        "setData" => {
            if let Some(n) = node(scope, 0) {
                let text = arg_string(scope, &args, 1);
                let _ = state.borrow_mut().document.set_value(n, text);
            }
        }
        // A form control's current value and checkedness (the document's
        // control state, which the engine shows).
        // A navigation the page asked for: the window goes there.
        "navigate" => {
            let url = arg_string(scope, &args, 0);
            let replace = args.get(1).boolean_value(scope);
            state.borrow_mut().navigation = Some((url, replace));
        }
        "controlValue" => {
            if let Some(n) = node(scope, 0) {
                let value = state.borrow().document.control_value(n).map(|v| v.into_owned()).unwrap_or_default();
                rv.set(string(scope, &value));
            }
        }
        "setControlValue" => {
            if let Some(n) = node(scope, 0) {
                let value = arg_string(scope, &args, 1);
                let _ = state.borrow_mut().document.set_control_value(n, &value);
            }
        }
        "controlChecked" => {
            if let Some(n) = node(scope, 0) {
                let checked = state.borrow().document.control_checked(n).unwrap_or(false);
                rv.set(v8::Boolean::new(scope, checked).into());
            }
        }
        "setControlChecked" => {
            if let Some(n) = node(scope, 0) {
                let checked = args.get(1).boolean_value(scope);
                let _ = state.borrow_mut().document.set_control_checked(n, checked);
            }
        }
        "textContent" => {
            if let Some(n) = node(scope, 0) {
                let mut out = String::new();
                let _ = html::text_content(&state.borrow().document, n, &mut out, 16 << 20);
                rv.set(string(scope, &out));
            }
        }
        "setTextContent" => {
            if let Some(n) = node(scope, 0) {
                let text = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let doc = &mut s.document;
                let _ = (|| -> olo_web::dom::Result<()> {
                    match doc.node_type(n)? {
                        NodeType::Text | NodeType::Comment | NodeType::ProcessingInstruction => {
                            doc.set_value(n, text)?;
                        }
                        _ => {
                            while let Some(c) = doc.first_child(n)? {
                                doc.detach(c)?;
                            }
                            if !text.is_empty() {
                                let t = doc.create_text(text)?;
                                doc.append_child(n, t)?;
                            }
                        }
                    }
                    Ok(())
                })();
            }
        }
        "attr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let value = state.borrow().document.attribute(n, &attribute).ok().flatten().map(|v| v.to_owned());
                match value {
                    Some(v) => rv.set(string(scope, &v)),
                    None => rv.set_null(),
                }
            }
        }
        "setAttr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let value = arg_string(scope, &args, 2);
                if attribute.is_empty() {
                    throw(scope, "InvalidCharacterError: empty attribute name");
                    return;
                }
                let _ = state.borrow_mut().document.set_attribute(n, &attribute, value);
            }
        }
        "removeAttr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let _ = state.borrow_mut().document.remove_attribute(n, &attribute);
            }
        }
        "attrNames" => {
            if let Some(n) = node(scope, 0) {
                let names: Vec<String> = state.borrow().document.attributes_ns(n).map(|a| a.map(|a| a.qualified_name.to_owned()).collect()).unwrap_or_default();
                let items: Vec<v8::Local<v8::Value>> = names.iter().map(|s| string(scope, s)).collect();
                rv.set(v8::Array::new_with_elements(scope, &items).into());
            }
        }
        "append" | "insertBefore" => {
            let (Some(parent), Some(child)) = (node(scope, 0), node(scope, 1)) else {
                throw(scope, "TypeError: not a node");
                return;
            };
            let before = if name == "insertBefore" { node(scope, 2) } else { None };
            let result = {
                let mut s = state.borrow_mut();
                let doc = &mut s.document;
                (|| -> olo_web::dom::Result<()> {
                    // A fragment's children move, the fragment stays empty.
                    if doc.node_type(child)? == NodeType::Fragment {
                        while let Some(c) = doc.first_child(child)? {
                            doc.detach(c)?;
                            if before.is_some() {
                                doc.insert_before(parent, c, before)?
                            } else {
                                doc.append_child(parent, c)?
                            };
                        }
                        return Ok(());
                    }
                    if doc.parent(child)?.is_some() {
                        doc.detach(child)?;
                    }
                    if before.is_some() {
                        doc.insert_before(parent, child, before)?
                    } else {
                        doc.append_child(parent, child)?
                    };
                    Ok(())
                })()
            };
            if let Err(e) = result {
                throw(scope, &format!("HierarchyRequestError: {e:?}"));
            }
        }
        "remove" => {
            if let Some(n) = node(scope, 0) {
                let _ = state.borrow_mut().document.detach(n);
            }
        }
        "createElement" => {
            let tag = arg_string(scope, &args, 0);
            let ns = arg_string(scope, &args, 1);
            let namespace = match ns.as_str() {
                "http://www.w3.org/2000/svg" => Namespace::Svg,
                "http://www.w3.org/1998/Math/MathML" => Namespace::MathMl,
                _ => Namespace::Html,
            };
            let tag = if namespace == Namespace::Html { tag.to_ascii_lowercase() } else { tag };
            if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.') {
                throw(scope, "InvalidCharacterError: bad tag name");
                return;
            }
            let created = state.borrow_mut().document.create_element(&tag, namespace).ok();
            match created.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => throw(scope, "could not create the element"),
            }
        }
        "templateContents" => {
            if let Some(n) = node(scope, 0) {
                let contents = state.borrow().document.template_contents(n).ok().flatten();
                if let Some(o) = contents.and_then(|c| wrap(scope, &state, c)) {
                    rv.set(o.into());
                }
            }
        }
        "createText" | "createComment" | "createFragment" => {
            let text = arg_string(scope, &args, 0);
            let created = {
                let mut s = state.borrow_mut();
                match name.as_str() {
                    "createText" => s.document.create_text(text).ok(),
                    "createComment" => s.document.create_comment(text).ok(),
                    _ => s.document.create_fragment().ok(),
                }
            };
            match created.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => throw(scope, "could not create the node"),
            }
        }
        "clone" => {
            if let Some(n) = node(scope, 0) {
                let deep = args.get(1).boolean_value(scope);
                let copy = {
                    let mut s = state.borrow_mut();
                    let doc = &mut s.document;
                    if deep {
                        doc.clone_subtree(n).ok()
                    } else {
                        (|| -> olo_web::dom::Result<NodeId> {
                            match doc.node_type(n)? {
                                NodeType::Element => {
                                    let name = doc.name(n)?.unwrap_or("div").to_owned();
                                    let ns = doc.namespace(n)?.unwrap_or(Namespace::Html);
                                    let e = doc.create_element(&name, ns)?;
                                    let attributes: Vec<(String, String)> = doc.attributes_ns(n)?.map(|a| (a.qualified_name.to_owned(), a.value.to_owned())).collect();
                                    for (k, v) in attributes {
                                        doc.set_attribute(e, &k, v)?;
                                    }
                                    Ok(e)
                                }
                                NodeType::Text => doc.create_text(doc.value(n)?.unwrap_or("").to_owned()),
                                NodeType::Comment => doc.create_comment(doc.value(n)?.unwrap_or("").to_owned()),
                                _ => doc.create_fragment(),
                            }
                        })()
                        .ok()
                    }
                };
                match copy.and_then(|c| wrap(scope, &state, c)) {
                    Some(o) => rv.set(o.into()),
                    None => throw(scope, "could not clone the node"),
                }
            }
        }
        "query" | "matches" => {
            let Some(root) = node(scope, 0) else { return };
            let selector = arg_string(scope, &args, 1);
            let all = name == "query" && args.get(2).boolean_value(scope);
            let matching = name == "matches";
            let started = native_profiling().then(Instant::now);
            let result = {
                let mut s = state.borrow_mut();
                let s = &mut *s;
                let quirks = s.quirks == QuirksMode::Quirks;
                select(&s.document, root, &selector, quirks, all, matching, &mut s.selectors)
            };
            if let Some(t) = started {
                let spent = t.elapsed().as_secs_f64();
                NATIVE_PROFILE.with(|p| {
                    let mut p = p.borrow_mut();
                    let e = p.entry(format!("{name} {selector}")).or_insert((0, 0.));
                    e.0 += 1;
                    e.1 += spent;
                });
            }
            match result {
                Err(e) => throw(scope, &format!("SyntaxError: '{selector}' is not a valid selector ({e})")),
                Ok(nodes) if matching => rv.set_bool(!nodes.is_empty()),
                Ok(nodes) if all => rv.set(nodes_array(scope, &state, &nodes)),
                Ok(nodes) => match nodes.first().and_then(|n| wrap(scope, &state, *n)) {
                    Some(o) => rv.set(o.into()),
                    None => rv.set_null(),
                },
            }
        }
        "innerHTML" | "outerHTML" => {
            if let Some(n) = node(scope, 0) {
                let mut out = String::new();
                let mut budget = 32 << 20;
                let s = state.borrow();
                let _ = if name == "innerHTML" { html::inner_html(&s.document, n, &mut out, &mut budget) } else { html::serialize(&s.document, n, &mut out, &mut budget) };
                drop(s);
                rv.set(string(scope, &out));
            }
        }
        "setInnerHTML" => {
            if let Some(n) = node(scope, 0) {
                let markup = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let quirks = s.quirks;
                let doc = &mut s.document;
                let result = (|| -> olo_web::dom::Result<()> {
                    let context = if doc.node_type(n)? == NodeType::Element { n } else { doc.root() };
                    let fragment = html::parse_fragment_into(doc, context, &markup, quirks)?;
                    // A template's markup is its contents, not its children.
                    let target = doc.template_contents(n)?.unwrap_or(n);
                    while let Some(c) = doc.first_child(target)? {
                        doc.detach(c)?;
                    }
                    while let Some(c) = doc.first_child(fragment)? {
                        doc.detach(c)?;
                        doc.append_child(target, c)?;
                    }
                    Ok(())
                })();
                drop(s);
                if let Err(e) = result {
                    throw(scope, &format!("innerHTML: {e:?}"));
                }
            }
        }
        "docRoot" => {
            let root = state.borrow().document.root();
            if let Some(o) = wrap(scope, &state, root) {
                rv.set(o.into());
            }
        }
        "byId" => {
            let id = arg_string(scope, &args, 0);
            let found = {
                let s = state.borrow();
                find_by_id(&s.document, &id)
            };
            match found.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => rv.set_null(),
            }
        }
        "isConnected" => {
            if let Some(n) = node(scope, 0) {
                let s = state.borrow();
                let doc = &s.document;
                let mut at = Some(n);
                let mut connected = false;
                while let Some(a) = at {
                    if a == doc.root() {
                        connected = true;
                        break;
                    }
                    at = doc.parent(a).ok().flatten();
                }
                rv.set_bool(connected);
            }
        }
        "log" => {
            let level = arg_string(scope, &args, 0);
            let text = arg_string(scope, &args, 1);
            let mut s = state.borrow_mut();
            if s.report.console.len() < 500 {
                let line = format!("[{level}] {}", text.chars().take(2000).collect::<String>());
                s.report.console.push(line);
            }
        }
        "fetchSync" => {
            let url = arg_string(scope, &args, 0);
            let method = arg_string(scope, &args, 1);
            let body = arg_string(scope, &args, 2);
            let result = {
                let mut s = state.borrow_mut();
                match s.fetcher.as_mut() {
                    Some(f) => f(&url, if method.is_empty() { "GET" } else { &method }, &body),
                    None => Err("no network for scripts".into()),
                }
            };
            match result {
                Ok(fetched) => {
                    let object = v8::Object::new(scope);
                    let pairs: [(&str, v8::Local<v8::Value>); 4] = [
                        ("status", v8::Integer::new(scope, i32::from(fetched.status)).into()),
                        ("contentType", string(scope, &fetched.content_type)),
                        ("body", bytes_array(scope, fetched.body, ByteKind::U8)),
                        ("url", string(scope, &fetched.url)),
                    ];
                    for (k, v) in pairs {
                        let key = v8::String::new(scope, k).unwrap();
                        object.set(scope, key.into(), v);
                    }
                    rv.set(object.into());
                }
                Err(e) => throw(scope, &format!("TypeError: fetch failed: {e}")),
            }
        }
        // fetch(): (url, method, body) → request id; `__olo_fetched`
        // follows when the browser has it. Null without a fetcher.
        "fetchAsync" => {
            let url = arg_string(scope, &args, 0);
            let method = arg_string(scope, &args, 1);
            let body = arg_string(scope, &args, 2);
            let mut s = state.borrow_mut();
            if s.fetcher.is_none() {
                rv.set_null();
            } else {
                let id = s.next_request;
                s.next_request += 1;
                s.in_flight.insert(id, None);
                s.requests.push(Request { id, url, kind: RequestKind::Text { method: if method.is_empty() { "GET".into() } else { method }, body } });
                rv.set(v8::Number::new(scope, id as f64).into());
            }
        }
        // structuredClone: through V8's serialisation.
        "structuredClone" => {
            if let Some(bytes) = clone::serialize(scope, args.get(0))
                && let Some(value) = clone::deserialize(scope, &bytes)
            {
                rv.set(value);
            }
        }
        // new Worker(url, source?): a thread with its own scope; its id.
        "workerStart" => {
            let url = arg_string(scope, &args, 0);
            let source = if args.get(1).is_null_or_undefined() { None } else { Some(arg_string(scope, &args, 1)) };
            let mut s = state.borrow_mut();
            let Some(fetcher) = s.worker_fetcher.clone() else {
                rv.set_null();
                return;
            };
            let id = s.next_worker;
            s.next_worker += 1;
            let waker = s.waker.clone();
            let user_agent = s.config.user_agent.clone();
            match worker::Handle::spawn(url, source, fetcher, waker, user_agent) {
                Ok(handle) => {
                    s.workers.insert(id, handle);
                    rv.set(v8::Number::new(scope, f64::from(id)).into());
                }
                Err(e) => {
                    s.report.console.push(format!("[error] worker: {e}"));
                    rv.set_null();
                }
            }
        }
        "workerPost" => {
            let id = args.get(0).number_value(scope).unwrap_or(0.) as u32;
            if let Some(bytes) = clone::serialize(scope, args.get(1))
                && let Some(handle) = state.borrow().workers.get(&id)
            {
                handle.post(bytes);
            }
        }
        "workerTerminate" => {
            let id = args.get(0).number_value(scope).unwrap_or(0.) as u32;
            state.borrow_mut().workers.remove(&id);
        }
        // A script inserted by a script: compiled and run here, in the global
        // scope, its exception recorded like a page script's. Returns whether
        // it ran to the end.
        "runScript" => {
            let source = arg_string(scope, &args, 0);
            let name = arg_string(scope, &args, 1);
            state.borrow_mut().report.scripts += 1;
            v8::tc_scope!(let tc, scope);
            let ran = (|| {
                let code = v8::String::new(tc, &source)?;
                let resource = string(tc, &name);
                let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
                let script = v8::Script::compile(tc, code, Some(&origin))?;
                script.run(tc)
            })()
            .is_some();
            if !ran {
                let error = exception_text(tc, &name);
                state.borrow_mut().report.errors.push(error);
            }
            rv.set_bool(ran);
        }
        // Geometry, from a layout of the document as it is now.
        "rect" | "paddingRect" | "scrollSize" => {
            if let Some(n) = node(scope, 0) {
                let mut s = state.borrow_mut();
                let kind = name.as_str();
                let values = s.measured().and_then(|m| match kind {
                    "rect" => m.border_box(n).map(|r| [r.x, r.y, r.width, r.height]),
                    "paddingRect" => m.padding_box(n).map(|r| [r.x, r.y, r.width, r.height]),
                    _ => m.scroll_size(n).map(|(w, h)| [0., 0., w, h]),
                });
                drop(s);
                match values {
                    Some(v) => {
                        let items: Vec<v8::Local<v8::Value>> = v.iter().map(|x| v8::Number::new(scope, *x).into()).collect();
                        rv.set(v8::Array::new_with_elements(scope, &items).into());
                    }
                    None => rv.set_null(),
                }
            }
        }
        // A scroll container's offset, [x, y] (CSS px).
        "scrollOffset" => {
            if let Some(n) = node(scope, 0) {
                let o = state.borrow().document.scroll_offset(n);
                let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, o[0]).into(), v8::Number::new(scope, o[1]).into()];
                rv.set(v8::Array::new_with_elements(scope, &items).into());
            }
        }
        // Scroll a box (scrollTop, scrollTo): clamped to what it holds, as
        // the measured layout has it; whether it moved (its `scroll` event
        // is the script side's to queue).
        "setScrollOffset" => {
            if let Some(n) = node(scope, 0) {
                let x = args.get(1).number_value(scope).unwrap_or(0.);
                let y = args.get(2).number_value(scope).unwrap_or(0.);
                let mut s = state.borrow_mut();
                let max = s.measured().and_then(|m| Some((m.scroll_size(n)?, m.padding_box(n)?))).map(|((w, h), p)| [(w - p.width).max(0.), (h - p.height).max(0.)]);
                let moved = match max {
                    Some(max) => {
                        let clamp = |v: f64, m: f64| if v.is_finite() { v.clamp(0., m) } else { 0. };
                        s.document.set_scroll_offset(n, [clamp(x, max[0]), clamp(y, max[1])]).unwrap_or(false)
                    }
                    None => false,
                };
                drop(s);
                rv.set(v8::Boolean::new(scope, moved).into());
            }
        }
        "computed" => {
            if let Some(n) = node(scope, 0) {
                let property = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let text = s.measured().and_then(|m| m.computed(n, &property));
                drop(s);
                match text {
                    Some(t) => rv.set(string(scope, &t)),
                    None => rv.set_null(),
                }
            }
        }
        "docSize" => {
            let mut s = state.borrow_mut();
            let size = s.measured().map(|m| m.document_size());
            drop(s);
            let (w, h) = size.unwrap_or_else(|| {
                let s = state.borrow();
                (s.config.viewport.0, s.config.viewport.1)
            });
            let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, w).into(), v8::Number::new(scope, h).into()];
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "revision" => {
            let r = state.borrow().document.revision();
            rv.set(v8::Number::new(scope, r as f64).into());
        }
        // The CSS properties and shorthands the engine knows, by name: what
        // `name in element.style` answers.
        "cssProperties" => {
            let names = css::properties::Property::ALL.iter().map(|p| p.as_str()).chain(css::properties::Shorthand::ALL.iter().map(|p| p.as_str()));
            let items: Vec<v8::Local<v8::Value>> = names.map(|n| string(scope, n).into()).collect();
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        // Real time since the realm began (ms), for time within a task.
        "wallNow" => {
            let t = state.borrow().started.elapsed().as_secs_f64() * 1000.;
            rv.set(v8::Number::new(scope, t).into());
        }
        // The layout's geometry identity: the same while nothing moved or
        // resized (a restyle, a transform), so observers need not look.
        "layoutGeneration" => {
            let mut s = state.borrow_mut();
            let g = s.measured().map_or(0, |m| m.layout().geometry_identity());
            rv.set(v8::Number::new(scope, g as f64).into());
        }
        // A canvas's raster, made or resized (which clears it).
        "canvasSize" => {
            if let Some(n) = node(scope, 0) {
                let w = args.get(1).number_value(scope).unwrap_or(300.).max(0.) as u32;
                let h = args.get(2).number_value(scope).unwrap_or(150.).max(0.) as u32;
                let mut s = state.borrow_mut();
                if let Some(c) = s.gl.get_mut(&n) {
                    c.resize(w, h);
                } else {
                    match s.canvases.get_mut(&n) {
                        Some(c) => c.resize(w, h),
                        None => {
                            if let Some(c) = canvas::Canvas::new(w, h) {
                                s.canvases.insert(n, c);
                            }
                        }
                    }
                }
            }
        }
        // A canvas's pixels (2D or WebGL), straight RGBA rows from the top,
        // as a Uint8Array: for textures and drawImage between canvases.
        "canvasPixels" => {
            if let Some(n) = node(scope, 0) {
                let mut s = state.borrow_mut();
                let bytes = if let Some(c) = s.canvases.get(&n) { Some(c.image_data(0, 0, c.width(), c.height())) } else { s.gl.get_mut(&n).and_then(|c| c.pixels()) };
                drop(s);
                if let Some(bytes) = bytes {
                    rv.set(bytes_array(scope, bytes, ByteKind::U8));
                }
            }
        }
        // Load an image into an `img` element: (element, absolute URL).
        // Answers [naturalWidth, naturalHeight] when the page has it
        // already, else undefined: it is requested, and `__olo_imageLoaded`
        // follows (null when nothing can load it).
        "loadImage" => {
            if let Some(n) = node(scope, 0) {
                let url = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let known = match s.image_lookup.as_mut() {
                    Some(f) => f(&url),
                    None => None,
                };
                match known {
                    Some(image) => {
                        let (w, h) = image.natural();
                        s.images.insert(n, image);
                        drop(s);
                        let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, w.round()).into(), v8::Number::new(scope, h.round()).into()];
                        rv.set(v8::Array::new_with_elements(scope, &items).into());
                    }
                    None if s.image_lookup.is_some() => {
                        let id = s.next_request;
                        s.next_request += 1;
                        s.in_flight.insert(id, Some(n));
                        s.requests.push(Request { id, url, kind: RequestKind::Image });
                    }
                    None => rv.set_null(),
                }
            }
        }
        // [naturalWidth, naturalHeight, pixelWidth, pixelHeight] of a loaded
        // image element, or null.
        // The source the engine shows an <img> from: its <picture>'s
        // sources, srcset and sizes chosen for this display (as written).
        "imageSource" => {
            if let Some(n) = node(scope, 0) {
                let chosen = {
                    let s = state.borrow();
                    let media = olo_web::css::media::Media::screen(s.config.viewport.0, s.config.viewport.1, s.config.scale, false);
                    olo_web::srcset::choose(&s.document, n, &media, None)
                };
                if let Some(choice) = chosen {
                    rv.set(string(scope, &choice.url).into());
                }
            }
        }
        // Whether a media query list holds for this page, by the engine's
        // own media evaluation (what its sheets' @media see).
        "matchMedia" => {
            let query = arg_string(scope, &args, 0);
            let holds = {
                let s = state.borrow();
                let media = olo_web::css::media::Media { scripting: true, ..olo_web::css::media::Media::screen(s.config.viewport.0, s.config.viewport.1, s.config.scale, s.config.dark) };
                olo_web::css::media::matches(&query, &media, None)
            };
            rv.set(v8::Boolean::new(scope, holds).into());
        }
        "imageSize" => {
            if let Some(n) = node(scope, 0) {
                let size = state.borrow().images.get(&n).map(|i| (i.natural(), i.width(), i.height()));
                if let Some(((w, h), pw, ph)) = size {
                    let items: Vec<v8::Local<v8::Value>> = vec![
                        v8::Number::new(scope, w.round()).into(),
                        v8::Number::new(scope, h.round()).into(),
                        v8::Number::new(scope, f64::from(pw)).into(),
                        v8::Number::new(scope, f64::from(ph)).into(),
                    ];
                    rv.set(v8::Array::new_with_elements(scope, &items).into());
                }
            }
        }
        // A loaded image's pixels, straight RGBA rows from the top (for
        // textures), as a Uint8Array.
        "imagePixels" => {
            if let Some(n) = node(scope, 0) {
                let bytes = state.borrow().images.get(&n).map(|i| unpremultiply(i.rgba()));
                if let Some(bytes) = bytes {
                    rv.set(bytes_array(scope, bytes, ByteKind::U8));
                }
            }
        }
        // A WebGL context for a canvas: (canvas, width, height,
        // premultipliedAlpha, preserveDrawingBuffer, version). True if the
        // GPU is there.
        "glCreate" => {
            let ok = node(scope, 0).is_some_and(|n| {
                let w = args.get(1).number_value(scope).unwrap_or(300.).max(1.) as u32;
                let h = args.get(2).number_value(scope).unwrap_or(150.).max(1.) as u32;
                let premultiplied = args.get(3).number_value(scope).unwrap_or(1.) != 0.;
                let preserve = args.get(4).number_value(scope).unwrap_or(0.) != 0.;
                let alpha = args.get(6).number_value(scope).unwrap_or(1.) != 0.;
                let mut s = state.borrow_mut();
                if s.gl.contains_key(&n) {
                    return true;
                }
                match webgl::Gl::make().and_then(|gl| webgl::Context::new(gl, w, h, premultiplied, preserve, alpha)) {
                    Ok(c) => {
                        s.canvases.remove(&n);
                        s.gl.insert(n, c);
                        true
                    }
                    Err(e) => {
                        s.report.console.push(format!("webgl: {e}"));
                        false
                    }
                }
            });
            rv.set_bool(ok);
        }
        // One GL call: the canvas, the operation, its arguments (an array).
        "glOp" => {
            let timing = Instant::now();
            if let Some(n) = node(scope, 0) {
                let op = arg_string(scope, &args, 1);
                let array = v8::Local::<v8::Array>::try_from(args.get(2)).ok();
                let count = array.map_or(0, |a| a.length());
                let mut converted = Vec::with_capacity(count as usize);
                for i in 0..count {
                    let v = array.and_then(|a| a.get_index(scope, i));
                    converted.push(v.map_or(webgl::Arg::Null, |v| gl_arg(scope, v)));
                }
                let out = {
                    let mut s = state.borrow_mut();
                    match s.gl.get_mut(&n) {
                        Some(c) => c.op(&op, &converted),
                        None => webgl::Out::None,
                    }
                };
                match out {
                    webgl::Out::None => {}
                    webgl::Out::Num(v) => rv.set(v8::Number::new(scope, v).into()),
                    webgl::Out::Bool(b) => rv.set_bool(b),
                    webgl::Out::Str(t) => rv.set(string(scope, &t)),
                    webgl::Out::Floats(f) => rv.set(bytes_array(scope, f.iter().flat_map(|x| x.to_le_bytes()).collect(), ByteKind::F32)),
                    webgl::Out::Ints(f) => rv.set(bytes_array(scope, f.iter().flat_map(|x| x.to_le_bytes()).collect(), ByteKind::I32)),
                    webgl::Out::Bytes(b) => rv.set(bytes_array(scope, b, ByteKind::U8)),
                }
            }
            let mut s = state.borrow_mut();
            s.gl_calls += 1;
            s.gl_seconds += timing.elapsed().as_secs_f64();
        }
        // One drawing call: the canvas, the operation, its arguments.
        "canvasOp" => {
            if let Some(n) = node(scope, 0) {
                let op = arg_string(scope, &args, 1);
                let result = canvas_op(scope, &state, n, &op, args.get(2));
                if let Some(v) = result {
                    rv.set(v);
                }
            }
        }
        "url" => {
            let url = state.borrow().config.url.clone();
            rv.set(string(scope, &url));
        }
        "viewport" => {
            let (w, h, scale) = {
                let s = state.borrow();
                (s.config.viewport.0, s.config.viewport.1, s.config.scale)
            };
            let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, w).into(), v8::Number::new(scope, h).into(), v8::Number::new(scope, scale).into()];
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "userAgent" => {
            let ua = state.borrow().config.user_agent.clone();
            rv.set(string(scope, &ua));
        }
        "language" => {
            let l = state.borrow().config.language.clone();
            rv.set(string(scope, &l));
        }
        "count" => {
            // Timers and listeners the prelude accounts for, for the report.
            let kind = arg_string(scope, &args, 0);
            let mut s = state.borrow_mut();
            if kind == "timer" {
                s.report.timers += 1;
            } else {
                s.report.listeners += 1;
            }
        }
        _ => {}
    }
}
/// Elements under `root` (or `root` itself for `matching`) matching the
/// selector list, in document order; `all` keeps every one.
/// A selector that is one tag, id or class alone: matched by a look at
/// the element, not through the selector engine (most querySelector
/// calls are one of these).
enum Simple<'a> {
    Tag(String),
    Id(&'a str),
    Class(&'a str),
}
fn simple_selector(selector: &str) -> Option<Simple<'_>> {
    let selector = selector.trim();
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit() || c == '-');
    if let Some(id) = selector.strip_prefix('#') {
        return ident(id).then_some(Simple::Id(id));
    }
    if let Some(class) = selector.strip_prefix('.') {
        return ident(class).then_some(Simple::Class(class));
    }
    ident(selector).then(|| Simple::Tag(selector.to_ascii_lowercase()))
}
fn select(doc: &Document, root: NodeId, selector: &str, quirks: bool, all: bool, matching: bool, compiled: &mut HashMap<(String, bool), Arc<Selectors>>) -> Result<Vec<NodeId>, String> {
    if let Some(simple) = simple_selector(selector) {
        let is_html = |n: NodeId| doc.namespace(n).ok().flatten() == Some(olo_web::dom::Namespace::Html);
        let hit = |n: NodeId| -> bool {
            match &simple {
                Simple::Tag(tag) => doc.name(n).ok().flatten().is_some_and(|name| if is_html(n) { name.eq_ignore_ascii_case(tag) } else { name == tag }),
                Simple::Id(id) => doc.attribute(n, "id").ok().flatten() == Some(id),
                Simple::Class(class) => doc.attribute(n, "class").ok().flatten().is_some_and(|c| c.split_ascii_whitespace().any(|t| if quirks { t.eq_ignore_ascii_case(class) } else { t == *class })),
            }
        };
        if matching {
            return Ok(if doc.node_type(root).map_err(|e| format!("{e:?}"))? == NodeType::Element && hit(root) { vec![root] } else { vec![] });
        }
        let mut out = Vec::new();
        let mut next = doc.first_child(root).ok().flatten();
        let mut stack: Vec<NodeId> = Vec::new();
        // Tree order under the root, by first child then next sibling.
        while let Some(n) = next {
            if doc.node_type(n).ok() == Some(NodeType::Element) && hit(n) {
                out.push(n);
                if !all {
                    break;
                }
            }
            if let Some(c) = doc.first_child(n).ok().flatten() {
                if let Some(s) = doc.next_sibling(n).ok().flatten() {
                    stack.push(s);
                }
                next = Some(c);
            } else {
                next = doc.next_sibling(n).ok().flatten().or_else(|| stack.pop());
            }
        }
        return Ok(out);
    }
    let selectors = match compiled.get(&(selector.to_owned(), quirks)) {
        Some(s) => s.clone(),
        None => {
            let syntax = css::Syntax::parse(Arc::from(selector), Default::default(), None).map_err(|e| format!("{e:?}"))?;
            let s = Arc::new(Selectors::compile(syntax.components(), CompileOptions { namespaces: &[], default_namespace: None }, Default::default(), None).map_err(|e| format!("{e:?}"))?);
            if compiled.len() >= 4096 {
                compiled.clear();
            }
            compiled.insert((selector.to_owned(), quirks), s.clone());
            s
        }
    };
    let tree = DomTree::new(doc);
    let options = MatchOptions { html_document: true, quirks, scope: Some(root), ..Default::default() };
    let hit = |node: NodeId| selectors.matches(&tree, node, options.clone(), MatchLimits::default(), None).ok().flatten().is_some();
    if matching {
        return Ok(if doc.node_type(root).map_err(|e| format!("{e:?}"))? == NodeType::Element && hit(root) { vec![root] } else { vec![] });
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut first = true;
    let mut visited = 0usize;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 2_000_000 {
            break;
        }
        if !first && doc.node_type(n).ok() == Some(NodeType::Element) && hit(n) {
            out.push(n);
            if !all {
                break;
            }
        }
        first = false;
        // Push children in reverse so the first child is handled next.
        let mut children = Vec::new();
        let mut child = doc.first_child(n).ok().flatten();
        while let Some(c) = child {
            children.push(c);
            child = doc.next_sibling(c).ok().flatten();
        }
        stack.extend(children.into_iter().rev());
    }
    Ok(out)
}
fn find_by_id(doc: &Document, id: &str) -> Option<NodeId> {
    if id.is_empty() {
        return None;
    }
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.node_type(n).ok() == Some(NodeType::Element) && doc.attribute(n, "id").ok().flatten() == Some(id) {
            return Some(n);
        }
        let mut children = Vec::new();
        let mut child = doc.first_child(n).ok().flatten();
        while let Some(c) = child {
            children.push(c);
            child = doc.next_sibling(c).ok().flatten();
        }
        stack.extend(children.into_iter().rev());
    }
    None
}
/// Where a script's source comes from: inline text, or a URL to fetch.
pub enum Source {
    Inline(Arc<str>),
    External(String),
}
/// The page's scripts in the order they run: classic and module scripts
/// (deferred ones included, since everything runs after parsing) in
/// document order, then async ones; `nomodule` scripts not at all, as in
/// any browser with modules. Each entry says whether it is a module. An
/// external URL is resolved against `base` with `url::Url`.
pub fn page_scripts(doc: &Document, base: &str) -> Vec<(Source, String, bool, NodeId)> {
    let page = base.to_owned();
    let base = url::Url::parse(base).ok();
    let mut classic = Vec::new();
    let mut asynchronous = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        let Ok(children) = doc.children(n) else { continue };
        let children: Vec<NodeId> = children.collect();
        for c in children.into_iter().rev() {
            stack.push(c);
        }
        if doc.node_type(n) != Ok(NodeType::Element) || doc.namespace(n) != Ok(Some(Namespace::Html)) || doc.name(n) != Ok(Some("script")) {
            continue;
        }
        let kind = doc.attribute(n, "type").ok().flatten().unwrap_or("").trim().to_ascii_lowercase();
        let module = kind == "module";
        let javascript = kind.is_empty()
            || matches!(kind.as_str(), "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "application/x-javascript" | "text/jscript" | "text/x-javascript");
        if !javascript && !module {
            continue;
        }
        if doc.attribute(n, "nomodule").ok().flatten().is_some() {
            continue;
        }
        let entry = match doc.attribute(n, "src").ok().flatten().filter(|s| !s.trim().is_empty()) {
            Some(src) => {
                let resolved = match &base {
                    Some(b) => b.join(src.trim()).map(|u| u.to_string()).unwrap_or_else(|_| src.trim().to_owned()),
                    None => src.trim().to_owned(),
                };
                (Source::External(resolved.clone()), resolved)
            }
            None => {
                let mut text = String::new();
                let _ = html::text_content(doc, n, &mut text, 16 << 20);
                if text.trim().is_empty() {
                    continue;
                }
                // An inline module's name is a URL, for its imports to resolve
                // against.
                let name = if module { format!("{page}#inline-module-{}", classic.len() + asynchronous.len() + 1) } else { format!("inline script {}", classic.len() + asynchronous.len() + 1) };
                (Source::Inline(Arc::from(text)), name)
            }
        };
        let is_async = doc.attribute(n, "async").ok().flatten().is_some() && matches!(entry.0, Source::External(_));
        let entry = (entry.0, entry.1, module, n);
        if is_async { asynchronous.push(entry) } else { classic.push(entry) }
    }
    classic.extend(asynchronous);
    classic
}
