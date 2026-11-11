//! The program: its looper, its windows' loopers, and how it ends.
//!
//! BeOS's `BApplication` took over the main thread and gave every
//! window a thread of its own. Here [`run`] does: the main thread runs
//! the app's looper (what happens to the program as a whole: a launch
//! forwarded from another copy, the display's news), each window opened
//! with [`AppCx::open_window`] runs on a looper of its own, so one
//! window working never holds another up, and the display thread
//! (`crate::display`) reads input and draws for them all.

use std::{
    marker::PhantomData,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use olo_app::{Looper, LooperHandle, Messenger};
use olo_sync::Mutex;

use crate::{
    Bounds, Cx, Pixels, SharedString, Size, View, WindowBackgroundAppearance, WindowDecorations, WindowId, WindowKind, WindowParams,
    display::{DisplayEvent, display},
    point, px, size,
    window::{WindowCx, with_window},
};

struct Shared {
    app_id: String,
    main: LooperHandle,
    windows: AtomicUsize,
    /// Stays running with no windows (a service, Deskbar).
    resident: AtomicBool,
    quitting: AtomicBool,
    /// Every window's looper, to end with the program.
    loopers: Mutex<Vec<LooperHandle>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

type Hook = std::sync::Arc<dyn Fn() + Send + Sync>;

/// What the program does when app_server's connection is lost, and
/// when a display comes or goes.
static LOST: Mutex<Option<Hook>> = Mutex::new(None);
static DISPLAYS_CHANGED: Mutex<Vec<Hook>> = Mutex::new(Vec::new());

/// What the program does when its connection to app_server is gone (it
/// restarted in place, or the GPU device was lost): `f` runs on the
/// app's looper. Without it, the program ends.
pub fn on_compositor_lost(f: impl Fn() + Send + Sync + 'static) {
    *LOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

/// Has `f` run (on the app's looper) whenever a display comes, goes or
/// changes: [`displays`] lists them then.
pub fn on_displays_changed(f: impl Fn() + Send + Sync + 'static) {
    DISPLAYS_CHANGED.lock().unwrap_or_else(|e| e.into_inner()).push(std::sync::Arc::new(f));
}

/// Whether there's a compositor to show windows on (not a headless
/// run: tests, the layout lint).
pub fn has_compositor() -> bool {
    !crate::display::display().headless
}

/// Every display's bounds, in points, as app_server last said.
pub fn displays() -> Vec<Bounds<Pixels>> {
    crate::display::display().displays()
}

/// Every display's name (its output's: "eDP-1"), in [`displays`]' order:
/// what a layer surface is put on by (`LayerShellOptions::output`).
pub fn display_names() -> Vec<Option<String>> {
    crate::display::display().display_names()
}

/// Whether the program is ending.
pub fn is_quitting() -> bool {
    shared().is_some_and(|s| s.quitting.load(Ordering::Acquire))
}

fn shared() -> Option<&'static Shared> {
    SHARED.get()
}

/// The program's id (`olo.tracker`).
pub fn app_id() -> &'static str {
    shared().map(|s| s.app_id.as_str()).unwrap_or("")
}

pub(crate) fn window_opened() {
    if let Some(shared) = shared() {
        shared.windows.fetch_add(1, Ordering::AcqRel);
    }
}

pub(crate) fn window_closed() {
    let Some(shared) = shared() else { return };
    let left = shared.windows.fetch_sub(1, Ordering::AcqRel).saturating_sub(1);
    if left == 0 && !shared.resident.load(Ordering::Acquire) {
        quit();
    }
}

/// How many windows the program has open.
pub fn window_count() -> usize {
    shared().map(|s| s.windows.load(Ordering::Acquire)).unwrap_or(0)
}

/// Ends the program: every looper finishes what it's taking and stops.
/// From any thread.
pub fn quit() {
    let Some(shared) = shared() else { return };
    if shared.quitting.swap(true, Ordering::AcqRel) {
        return;
    }
    shared.main.quit();
}

/// Has `f` run on every window the program has, each on its looper.
/// From any thread.
pub fn all_windows(f: impl Fn(&mut WindowCx) + Send + Sync + 'static) {
    let Some(shared) = shared() else { return };
    let f = std::sync::Arc::new(f);
    for looper in shared.loopers.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let f = f.clone();
        looper.post(move |looper| crate::window::each(looper, |cx| f(cx)));
    }
}

/// Asks every window the program has for something, each on its
/// looper, and waits for them all (those that answer within `wait`:
/// a window whose looper is busy is left out). For a worker or a
/// service's thread, never a looper: it blocks.
pub fn ask_windows<T: Send + 'static>(wait: std::time::Duration, f: impl Fn(&mut WindowCx) -> Option<T> + Send + Sync + 'static) -> Vec<T> {
    let Some(shared) = shared() else { return Vec::new() };
    debug_assert!(!Looper::on_looper_thread(), "a looper must not wait on the others");
    let f = std::sync::Arc::new(f);
    let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<T>>>();
    let loopers: Vec<LooperHandle> = shared.loopers.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|l| !l.is_gone()).cloned().collect();
    let mut asked = 0;
    for looper in loopers {
        let (f, tx) = (f.clone(), tx.clone());
        if looper.post(move |looper| {
            let mut found = Vec::new();
            crate::window::each(looper, |cx| found.extend(f(cx)));
            let _ = tx.send(Some(found));
        }) {
            asked += 1;
        }
    }
    drop(tx);
    let deadline = std::time::Instant::now() + wait;
    let mut all = Vec::new();
    for _ in 0..asked {
        match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            Ok(Some(found)) => all.extend(found),
            Ok(None) => {}
            Err(_) => break,
        }
    }
    all
}

/// A window's frames since they were last asked for, and what draws
/// them.
pub struct WindowFrames {
    pub id: WindowId,
    pub stats: crate::FrameStats,
    pub gpu: Option<crate::GpuSpecs>,
}

/// Every window's frames since last asked (`{"frames":true}`): from the
/// display thread, which draws them. Waits for it: not for a looper.
pub fn take_frame_stats() -> Vec<WindowFrames> {
    display()
        .ask(|on| on.stages.values().map(|stage| WindowFrames { id: stage.id(), stats: stage.take_frame_stats(), gpu: stage.with_window(|w| w.gpu_specs()).flatten() }).collect())
        .unwrap_or_default()
}

/// The program's text system (its fonts, shaping and glyphs), for what
/// measures text with no window to hand.
pub fn text_system() -> std::sync::Arc<crate::TextSystem> {
    display().text_system.clone()
}

/// Every window draws everything again (the look changed).
pub fn refresh_all_windows() {
    all_windows(|cx| cx.refresh());
}

/// Keeps the program running with no windows (a service; something that
/// opens windows when it's asked to).
pub fn stay_resident() {
    if let Some(shared) = shared() {
        shared.resident.store(true, Ordering::Release);
    }
}

/// How a new window should be, as the display thread takes it. (Olo's
/// kit has its own options over this: `olo_kit::WindowOptions`.)
#[derive(Clone, Debug)]
pub struct WindowSpec {
    pub title: SharedString,
    /// Its content's size, in points (what frames it is app_server's).
    pub size: (f32, f32),
    pub min_size: Option<(f32, f32)>,
    pub max_size: Option<(f32, f32)>,
    pub kind: WindowKind,
    pub layer_shell: Option<crate::LayerShellOptions>,
    pub dialog: Option<crate::Dialog>,
    /// The xdg-toplevel-tag words app_server reads (`floating`…).
    pub toplevel_tag: Option<String>,
    pub foreign_parent: Option<String>,
    pub session_lock_output: Option<String>,
    pub background: WindowBackgroundAppearance,
    pub fullscreen: bool,
    /// The person can size it.
    pub resizable: bool,
    /// It takes the keyboard as it opens.
    pub focus: bool,
}

impl WindowSpec {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowSpec {
            title: title.into(),
            size: (width, height),
            min_size: None,
            max_size: None,
            kind: WindowKind::Normal,
            layer_shell: None,
            dialog: None,
            toplevel_tag: None,
            foreign_parent: None,
            session_lock_output: None,
            background: WindowBackgroundAppearance::Opaque,
            fullscreen: false,
            resizable: true,
            focus: true,
        }
    }

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    pub fn max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = Some((width, height));
        self
    }

    pub(crate) fn params(&self) -> WindowParams {
        let to_size = |(w, h): (f32, f32)| size(px(w), px(h));
        WindowParams {
            bounds: Bounds { origin: point(px(0.), px(0.)), size: to_size(self.size) },
            titlebar: Some(crate::TitlebarOptions { title: Some(self.title.clone()), appears_transparent: false, traffic_light_position: None }),
            kind: self.kind.clone(),
            is_movable: true,
            is_resizable: self.resizable,
            is_minimizable: true,
            focus: self.focus,
            show: true,
            display_id: None,
            toplevel_tag: self.toplevel_tag.clone(),
            app_id: Some(app_id().to_owned()).filter(|id| !id.is_empty()),
            layer_shell: self.layer_shell.clone(),
            dialog: self.dialog,
            foreign_parent: self.foreign_parent.clone(),
            session_lock_output: self.session_lock_output.clone(),
            background: self.background,
            decorations: WindowDecorations::Server,
            window_min_size: self.min_size.map(to_size),
            window_max_size: self.max_size.map(to_size),
        }
    }
}

/// A window opened by the program, as any thread may know it: somewhere
/// to send its own view messages.
pub struct WindowHandle<V> {
    id: WindowId,
    looper: LooperHandle,
    _view: PhantomData<fn() -> V>,
}

impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        WindowHandle { id: self.id, looper: self.looper.clone(), _view: PhantomData }
    }
}

impl<V: View> WindowHandle<V> {
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// Has `f` run on the window's own view, on its looper.
    pub fn update(&self, f: impl FnOnce(&mut V, &mut Cx<V>) + Send + 'static) {
        let id = self.id;
        self.looper.post(move |looper| {
            with_window(looper, id, |cx| {
                if let Some(view) = cx.core.main_view.or(cx.core.root_view) {
                    cx.update_view(view, f);
                }
            });
        });
    }

    /// A messenger to the window's own view.
    pub fn messenger(&self) -> Messenger<V::Message>
    where
        V::Message: Send,
    {
        let id = self.id;
        Messenger::new(self.looper.clone(), move |looper, message| {
            with_window(looper, id, |cx| {
                if let Some(view) = cx.core.main_view.or(cx.core.root_view) {
                    cx.update_view::<V, _>(view, |v, cx| v.message(message, cx));
                }
            });
        })
    }

    /// Does something with the window on its looper.
    pub fn with(&self, f: impl FnOnce(&mut WindowCx) + Send + 'static) {
        let id = self.id;
        self.looper.post(move |looper| {
            with_window(looper, id, f);
        });
    }

    pub fn close(&self) {
        self.with(|cx| cx.close_window());
    }

    /// Whether its looper still runs.
    pub fn is_open(&self) -> bool {
        !self.looper.is_gone()
    }

    /// The same window, its own view taken to be a `T` (what
    /// `WindowCx::set_main_view` named: a window whose first view is a
    /// frame round the view the program knows it by).
    pub fn retyped<T: View>(self) -> WindowHandle<T> {
        WindowHandle { id: self.id, looper: self.looper, _view: PhantomData }
    }

    /// The looper the window is on.
    pub fn looper(&self) -> &LooperHandle {
        &self.looper
    }
}

/// The program, as its own looper's code is handed it.
pub struct AppCx<'a> {
    pub(crate) looper: &'a mut Looper,
}

impl<'a> AppCx<'a> {
    /// The program, on its own looper (from a message taken there).
    pub fn on(looper: &'a mut Looper) -> AppCx<'a> {
        AppCx { looper }
    }
}

impl AppCx<'_> {
    /// Opens a window on a looper of its own, around the view `build`
    /// makes there. Returns at once: the window is opened by its looper.
    pub fn open_window<V: View>(&mut self, options: WindowSpec, build: impl FnOnce(&mut Cx<V>) -> V + Send + 'static) -> WindowHandle<V> {
        open_window(options, build)
    }

    /// The app's looper.
    pub fn looper(&mut self) -> &mut Looper {
        self.looper
    }

    pub fn quit(&mut self) {
        quit()
    }
}

/// Opens a window on a looper of its own (see [`AppCx::open_window`]).
/// From any thread of a running program.
pub fn open_window<V: View>(options: WindowSpec, build: impl FnOnce(&mut Cx<V>) -> V + Send + 'static) -> WindowHandle<V> {
    let id = display().next_window_id();
    // (BeOS named a window's thread "w>" and its title.)
    let name = format!("w>{}", options.title);
    let looper = Looper::spawn(name, move |looper| {
        // Its frames' work is done here: on a fast core.
        crate::util_floor();
        let title = options.title.clone();
        if let Err(e) = crate::window::open(looper, id, options.params(), title, build) {
            tracing::error!("a window couldn't be opened: {e:#}");
            looper.quit();
            return;
        }
        // The looper is the window's: it ends when its last window has.
        looper.on_idle(|looper| {
            if crate::window::count(looper) == 0 {
                looper.quit();
            }
        });
    });
    if let Some(shared) = shared() {
        let mut loopers = shared.loopers.lock().unwrap_or_else(|e| e.into_inner());
        loopers.retain(|l| !l.is_gone());
        loopers.push(looper.clone());
    }
    WindowHandle { id, looper, _view: PhantomData }
}

/// Runs the program: `build` opens its windows; this returns when the
/// program quits (its last window closed, unless it's
/// [`stay_resident`]).
pub fn run(app_id: &str, build: impl FnOnce(&mut AppCx)) {
    let mut looper = Looper::new("app");
    let state =
        Shared { app_id: app_id.to_owned(), main: looper.handle(), windows: AtomicUsize::new(0), resident: AtomicBool::new(false), quitting: AtomicBool::new(false), loopers: Mutex::new(Vec::new()) };
    if SHARED.set(state).is_err() {
        panic!("a program runs once");
    }
    let display = display();
    let handle = looper.handle();
    display.set_events(Some(Messenger::new(handle, |_looper, event: DisplayEvent| match event {
        DisplayEvent::CompositorLost => {
            let lost = LOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match lost {
                Some(lost) => lost(),
                None => {
                    tracing::warn!("the display is gone");
                    quit();
                }
            }
        }
        // What may be pasted changed: every window's own view says what
        // it shows again (its menus' Paste).
        DisplayEvent::ClipboardChanged => all_windows(|cx| cx.invalidate_main_view()),
        DisplayEvent::DisplaysChanged => {
            let hooks: Vec<Hook> = DISPLAYS_CHANGED.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for hook in hooks {
                hook();
            }
            all_windows(|cx| cx.displays_changed());
        }
        DisplayEvent::KeyboardLayoutChanged => {}
    })));
    build(&mut AppCx { looper: &mut looper });
    looper.run();
    // Every window's looper ends, then the display thread.
    if let Some(shared) = shared() {
        for looper in shared.loopers.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            looper.quit();
        }
    }
    display.set_events(None);
    display.post(|on| on.stop());
}

/// The size a window is given when nothing says.
pub fn default_window_size() -> Size<Pixels> {
    crate::DEFAULT_WINDOW_SIZE
}
