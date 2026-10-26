//! Servo, in process. One `Servo` for the app; one GL context per window
//! (`Surface`), drawing straight into the window's embedded subsurface
//! (`Cx::embed`); one `WebView` per tab sharing its window's context.
//!
//! Servo calls back on the main thread from inside `spin_event_loop`,
//! while the app is borrowed, so the delegate only queues what happened
//! (`Events`) and the app drains the queue afterwards.

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use servo::{
    AuthenticationRequest, ClipboardDelegate, ConsoleLogLevel, CreateNewWebViewRequest, Cursor, EmbedderControl,
    EmbedderControlId, EventLoopWaker, LoadStatus, Opts, PermissionRequest, RenderingContext, Servo, ServoBuilder,
    StringRequest, UserContentManager, UserScript, WebView, WebViewDelegate, WebViewId, WindowRenderingContext,
    resources::{Resource, ResourceReaderMethods},
};
use tabula_kit::{Embedded, Proxy};
use url::Url;

/// Posted to the app when Servo has work for the main thread.
pub struct Wake;

#[derive(Clone)]
struct Waker {
    proxy: Proxy,
    /// One wake in flight at a time; Servo can ask thousands of times a second.
    pending: Arc<AtomicBool>,
}

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.proxy.send(Wake);
        }
    }
}

/// What a web view told us.
pub enum Event {
    Frame,
    Url(Url),
    Title(Option<String>),
    Status(Option<String>),
    Load(LoadStatus),
    Cursor(Cursor),
    History { can_back: bool, can_forward: bool },
    Favicon,
    Closed,
    Crashed(String),
    NewWebView(CreateNewWebViewRequest),
    Control(EmbedderControl),
    HideControl(EmbedderControlId),
    Permission(PermissionRequest),
    Authenticate(AuthenticationRequest),
    Copy(String),
    /// A login was just used on the page (logins.rs's capture script).
    LoginUsed { username: String, password: String },
    /// A fill script finished: "filled", "none" or "moved".
    Filled(String),
}

#[derive(Default)]
pub struct Events {
    queue: RefCell<VecDeque<(WebViewId, Event)>>,
    /// What the capture script's messages start with.
    login_marker: String,
    /// Text for the next paste a page asks for (we read the clipboard
    /// before telling Servo to paste).
    pub paste: RefCell<Option<String>>,
}

impl Events {
    fn push(&self, webview: &WebView, event: Event) {
        self.push_id(webview.id(), event);
    }
    pub fn push_id(&self, id: WebViewId, event: Event) {
        self.queue.borrow_mut().push_back((id, event));
    }
    pub fn take(&self) -> VecDeque<(WebViewId, Event)> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }
}

impl WebViewDelegate for Events {
    fn notify_new_frame_ready(&self, w: WebView) {
        self.push(&w, Event::Frame);
    }
    fn notify_url_changed(&self, w: WebView, url: Url) {
        self.push(&w, Event::Url(url));
    }
    fn notify_page_title_changed(&self, w: WebView, title: Option<String>) {
        self.push(&w, Event::Title(title));
    }
    fn notify_status_text_changed(&self, w: WebView, status: Option<String>) {
        self.push(&w, Event::Status(status));
    }
    fn notify_load_status_changed(&self, w: WebView, status: LoadStatus) {
        self.push(&w, Event::Load(status));
    }
    fn notify_cursor_changed(&self, w: WebView, cursor: Cursor) {
        self.push(&w, Event::Cursor(cursor));
    }
    fn notify_favicon_changed(&self, w: WebView) {
        self.push(&w, Event::Favicon);
    }
    fn notify_history_changed(&self, w: WebView, entries: Vec<Url>, current: usize) {
        let can_forward = current + 1 < entries.len();
        self.push(&w, Event::History { can_back: current > 0, can_forward });
    }
    fn notify_closed(&self, w: WebView) {
        self.push(&w, Event::Closed);
    }
    fn notify_crashed(&self, w: WebView, reason: String, _backtrace: Option<String>) {
        self.push(&w, Event::Crashed(reason));
    }
    fn request_create_new(&self, parent: WebView, request: CreateNewWebViewRequest) {
        self.push(&parent, Event::NewWebView(request));
    }
    fn show_embedder_control(&self, w: WebView, control: EmbedderControl) {
        self.push(&w, Event::Control(control));
    }
    fn hide_embedder_control(&self, w: WebView, id: EmbedderControlId) {
        self.push(&w, Event::HideControl(id));
    }
    fn request_permission(&self, w: WebView, request: PermissionRequest) {
        self.push(&w, Event::Permission(request));
    }
    fn request_authentication(&self, w: WebView, request: AuthenticationRequest) {
        self.push(&w, Event::Authenticate(request));
    }
    fn show_console_message(&self, w: WebView, _level: ConsoleLogLevel, message: String) {
        // Only the capture script's reports; pages' own logging goes nowhere.
        #[derive(serde::Deserialize)]
        struct Used {
            u: String,
            p: String,
        }
        if let Some(json) = message.strip_prefix(&self.login_marker)
            && let Ok(used) = serde_json::from_str::<Used>(json)
            && !used.p.is_empty()
        {
            self.push(&w, Event::LoginUsed { username: used.u, password: used.p });
        }
    }
}

impl ClipboardDelegate for Events {
    fn set_text(&self, w: WebView, text: String) {
        self.push(&w, Event::Copy(text));
    }
    fn get_text(&self, _w: WebView, request: StringRequest) {
        match self.paste.borrow_mut().take() {
            Some(text) => request.success(text),
            // Pages only read the clipboard on a paste we started.
            None => request.failure("The clipboard is read only when you paste.".into()),
        }
    }
}

pub struct Engine {
    pub servo: Servo,
    pending: Arc<AtomicBool>,
    pub events: Rc<Events>,
    /// Scripts every page gets (the login capture).
    pub content: Rc<UserContentManager>,
}

impl Engine {
    pub fn new(proxy: Proxy) -> Engine {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let opts = Opts { config_dir: data_dir(), ..Default::default() };
        let pending = Arc::new(AtomicBool::new(false));
        let servo = ServoBuilder::default()
            .opts(opts)
            .event_loop_waker(Box::new(Waker { proxy, pending: pending.clone() }))
            .build();
        let token = crate::logins::token();
        let content = Rc::new(UserContentManager::new(&servo));
        content.add_script(Rc::new(UserScript::from(crate::logins::capture_script(&token))));
        let events = Events { login_marker: crate::logins::marker(&token), ..Default::default() };
        Engine { servo, pending, events: Rc::new(events), content }
    }

    /// Let Servo run what it woke us for; returns what the views said.
    pub fn pump(&self) -> VecDeque<(WebViewId, Event)> {
        self.pending.store(false, Ordering::Release);
        self.servo.spin_event_loop();
        self.events.take()
    }
}

/// Cookies, local storage and the cache.
fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let dir = base.join("tabula/browser");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// A window's GL context on its embedded surface.
pub struct Surface {
    pub context: Rc<WindowRenderingContext>,
    egl_display: Option<khronos_egl::Display>,
}

impl Surface {
    pub fn new(embed: &Embedded, size: dpi::PhysicalSize<u32>) -> Result<Surface, String> {
        // SAFETY: the kit keeps both alive until `App::embed_lost`, and we
        // drop the context there.
        let (display, window) = unsafe {
            (
                raw_window_handle::DisplayHandle::borrow_raw(embed.display),
                raw_window_handle::WindowHandle::borrow_raw(embed.surface),
            )
        };
        let context = WindowRenderingContext::new(display, window, size).map_err(|e| format!("{e:?}"))?;
        context.make_current().map_err(|e| format!("{e:?}"))?;
        // Mesa's default swap interval waits for a frame callback on each
        // swap; a hidden window never gets one and would hang the UI.
        // Frames are paced by Servo's refresh driver instead.
        let egl_display = egl().and_then(|egl| {
            let display = egl.get_current_display()?;
            if let Err(e) = egl.swap_interval(display, 0) {
                tracing::warn!("eglSwapInterval: {e}");
            }
            Some(display)
        });
        Ok(Surface { context: Rc::new(context), egl_display })
    }

    pub fn resize(&self, size: dpi::PhysicalSize<u32>) {
        if self.context.size() != size {
            self.context.resize(size);
        }
    }

    /// Done with the display (every surface on it is gone): forget it,
    /// so a later connection that happens to get the same `wl_display`
    /// address doesn't get Mesa's cached display for the dead one.
    pub fn terminate(self) {
        let display = self.egl_display;
        drop(self.context);
        if let (Some(egl), Some(display)) = (egl(), display) {
            let _ = egl.make_current(display, None, None, None);
            let _ = egl.terminate(display);
        }
    }
}

type Egl = khronos_egl::DynamicInstance<khronos_egl::EGL1_4>;

fn egl() -> Option<&'static Egl> {
    static EGL: OnceLock<Option<Egl>> = OnceLock::new();
    EGL.get_or_init(|| {
        // SAFETY: libEGL is the system's; surfman has it loaded already.
        let lib = unsafe { libloading::Library::new("libEGL.so.1") }.ok()?;
        unsafe { Egl::load_required_from(lib) }.ok()
    })
    .as_ref()
}

/// Servo's built-in pages and data, with our own error pages.
struct Resources;

impl ResourceReaderMethods for Resources {
    fn read(&self, file: Resource) -> Vec<u8> {
        match file {
            Resource::BluetoothBlocklist => &include_bytes!("../resources/gatt_blocklist.txt")[..],
            Resource::DomainList => &include_bytes!("../resources/public_domains.txt")[..],
            Resource::HstsPreloadList => &include_bytes!("../resources/hsts_preload.fstmap")[..],
            Resource::BadCertHTML => &include_bytes!("../resources/badcert.html")[..],
            Resource::NetErrorHTML => &include_bytes!("../resources/neterror.html")[..],
            Resource::BrokenImageIcon => &include_bytes!("../resources/rippy.png")[..],
            Resource::CrashHTML => &include_bytes!("../resources/crash.html")[..],
            Resource::DirectoryListingHTML => &include_bytes!("../resources/directory-listing.html")[..],
            Resource::AboutMemoryHTML => &include_bytes!("../resources/about-memory.html")[..],
            Resource::DebuggerJS => &include_bytes!("../resources/debugger.js")[..],
            Resource::JsonViewerHTML => &include_bytes!("../resources/json-viewer.html")[..],
        }
        .to_vec()
    }
    fn sandbox_access_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

servo::submit_resource_reader!(&Resources);
