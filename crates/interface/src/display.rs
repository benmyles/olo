//! The display thread: the program's one connection to app_server, and
//! where its windows are drawn.
//!
//! On BeOS every window had two threads: the program's, which ran its
//! looper, and one in app_server, which drew what the program's sent it
//! and went on moving and showing the window while the program's was
//! busy. This thread is the second of those, kept on the program's side
//! (windows are Wayland surfaces: a program draws its own). It reads the
//! connection (input, sizes, frame callbacks) and hands each window's
//! looper what is its; it keeps what each window last showed (its
//! [`crate::stage::Stage`]: every view's drawing, where it is, what
//! scrolls) and draws frames from that, so a window goes on scrolling
//! and animating at the display's rate while its looper works.
//!
//! Nothing a program writes runs here: only the kit. Other threads reach
//! it with [`Display::post`], which never waits.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use anyhow::Result;
use olo_app::Messenger;
use olo_sync::Mutex;

use crate::{
    HeadlessClient, LinuxClient, LinuxCommon, PlatformDisplay, PlatformWindow, TextSystem, WaylandClient, WindowId, WindowParams,
    stage::{Stage, WindowLink},
};

/// Something for the display thread to do.
pub(crate) type Command = Box<dyn FnOnce(&mut OnDisplay) + Send>;

/// What the display thread tells the program as a whole (its app
/// looper).
#[derive(Clone, Debug)]
pub(crate) enum DisplayEvent {
    /// A display came, went or changed.
    DisplaysChanged,
    /// The connection to app_server is gone (it restarted): the program
    /// starts again with its state.
    CompositorLost,
    /// What's on the clipboard changed.
    ClipboardChanged,
    /// The keyboard's layout changed.
    KeyboardLayoutChanged,
    /// What happened to the session lock (`lock_session`).
    SessionLock(crate::SessionLockEvent),
}

pub(crate) enum Client {
    Wayland(WaylandClient),
    Headless(HeadlessClient),
}

impl Client {
    pub(crate) fn with_common<R>(&self, f: impl FnOnce(&mut LinuxCommon) -> R) -> R {
        match self {
            Client::Wayland(c) => c.with_common(f),
            Client::Headless(c) => c.with_common(f),
        }
    }

    pub(crate) fn open_window(&self, id: WindowId, params: WindowParams) -> Result<Box<dyn PlatformWindow>> {
        match self {
            Client::Wayland(c) => c.open_window(id, params),
            Client::Headless(c) => c.open_window(id, params),
        }
    }

    pub(crate) fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        match self {
            Client::Wayland(c) => c.displays(),
            Client::Headless(c) => c.displays(),
        }
    }

    /// The Wayland client, when there's a compositor.
    pub(crate) fn wayland(&self) -> Option<&WaylandClient> {
        match self {
            Client::Wayland(c) => Some(c),
            Client::Headless(_) => None,
        }
    }

    fn run(&self) {
        match self {
            Client::Wayland(c) => LinuxClient::run(c),
            Client::Headless(c) => LinuxClient::run(c),
        }
    }
}

/// What lives on the display thread.
pub(crate) struct OnDisplay {
    pub(crate) client: Rc<Client>,
    pub(crate) stages: HashMap<WindowId, Rc<Stage>>,
}

impl OnDisplay {
    pub(crate) fn stage(&self, id: WindowId) -> Option<Rc<Stage>> {
        self.stages.get(&id).cloned()
    }

    /// Every window goes (the program is quitting).
    pub(crate) fn close_all(&mut self) {
        self.stages.clear();
    }

    /// Ends the display thread's loop (the program is quitting).
    pub(crate) fn stop(&mut self) {
        self.client.with_common(|common| {
            common.quitting = true;
            common.signal.stop();
        });
    }

    /// Ends the display thread's loop, unless what's on the clipboard is
    /// the program's: then it runs on, with no windows, until that's
    /// gone (taken off, `clipboard::withdraw`, or something else copied),
    /// so what was copied can still be pasted after the program quit.
    /// Whether it runs on.
    pub(crate) fn stop_or_linger(&mut self) -> bool {
        let lingers = match &*self.client {
            Client::Wayland(c) => LinuxClient::linger_for_clipboard(c),
            Client::Headless(_) => false,
        };
        if !lingers {
            self.stop();
        }
        lingers
    }
}

thread_local! {
    static ON_DISPLAY: RefCell<Option<OnDisplay>> = const { RefCell::new(None) };
}

/// The display thread, as other threads know it.
pub(crate) struct Display {
    commands: calloop::channel::Sender<Command>,
    /// The program's text system (fonts, shaping, glyphs): one, shared by
    /// every window's looper.
    pub(crate) text_system: Arc<TextSystem>,
    /// Whether there's a compositor (not a headless run: tests, a
    /// service with no display).
    pub(crate) headless: bool,
    next_window: AtomicU64,
    events: Mutex<Option<Messenger<DisplayEvent>>>,
    /// Every display's bounds, as last heard.
    displays: Mutex<Vec<(Option<String>, crate::Bounds<crate::Pixels>)>>,
    gone: AtomicBool,
}

static DISPLAY: OnceLock<Display> = OnceLock::new();
/// The display thread has ended, its windows and connection gone.
static ENDED: AtomicBool = AtomicBool::new(false);
/// Who waits for that (`wait_until_ended`), woken as it ends.
static WAITING: Mutex<Option<std::thread::Thread>> = Mutex::new(None);

/// The display thread, started the first time it's asked for.
pub(crate) fn display() -> &'static Display {
    DISPLAY.get_or_init(Display::start)
}

impl Display {
    fn start() -> Display {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("display".into())
            .spawn(move || {
                // Frames are drawn here: on a fast core at a high clock,
                // not where the scheduler puts a mostly idle thread.
                crate::util_floor();
                let wayland = std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty()).or_else(|| std::env::var_os("WAYLAND_SOCKET")).is_some();
                let client = if wayland && std::env::var_os("OLO_HEADLESS").is_none() { Client::Wayland(WaylandClient::new()) } else { Client::Headless(HeadlessClient::new()) };
                let headless = matches!(client, Client::Headless(_));
                let (commands, receiver) = calloop::channel::channel::<Command>();
                let run = |command: Command| {
                    ON_DISPLAY.with(|d| {
                        // (Taken out while it runs: a command that ends
                        // in a callback asking for the display finds it
                        // busy rather than borrowed twice.)
                        let taken = d.borrow_mut().take();
                        if let Some(mut on) = taken {
                            command(&mut on);
                            *d.borrow_mut() = Some(on);
                        }
                    })
                };
                match &client {
                    Client::Wayland(c) => c.take_commands(receiver, run),
                    Client::Headless(c) => c.take_commands(receiver, run),
                }
                let text_system = Arc::new(TextSystem::new(client.with_common(|common| common.text_system.clone())));
                client.with_common(|common| {
                    common.callbacks.displays_changed = Some(Box::new(|| {
                        // (Looked at once this turn is over: told of inside a Wayland roundtrip.)
                        display().post(|on| {
                            display().note_displays(on);
                            display().tell(DisplayEvent::DisplaysChanged);
                        });
                    }));
                    common.callbacks.compositor_lost = Some(Box::new(|| display().tell(DisplayEvent::CompositorLost)));
                    common.callbacks.clipboard_changed = Some(Box::new(|| {
                        // (Looked at once this turn is over: what told us may be in the middle of it.)
                        display().post(|on| {
                            crate::clipboard::refresh(on);
                            display().tell(DisplayEvent::ClipboardChanged);
                        });
                    }));
                    common.callbacks.keyboard_layout_change = Some(Box::new(|| display().tell(DisplayEvent::KeyboardLayoutChanged)));
                });
                // (The displays there are as it starts, with it: whoever asks
                // first, a screen saver covering each, gets them all.)
                let displays: Vec<(Option<String>, crate::Bounds<crate::Pixels>)> = client.displays().iter().map(|d| (d.name(), d.bounds())).collect();
                let _ = ready_tx.send((commands, text_system, headless, displays));
                let client = Rc::new(client);
                ON_DISPLAY.with(|d| *d.borrow_mut() = Some(OnDisplay { client: client.clone(), stages: HashMap::new() }));
                client.run();
                if let Some(display) = DISPLAY.get() {
                    display.gone.store(true, Ordering::Release);
                }
                // What's left goes with the thread (windows first, then
                // the connection).
                ON_DISPLAY.with(|d| {
                    if let Some(mut on) = d.borrow_mut().take() {
                        on.stages.clear();
                    }
                });
                drop(client);
                ENDED.store(true, Ordering::Release);
                if let Some(waiting) = WAITING.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    waiting.unpark();
                }
            })
            .expect("no thread for the display");
        let (commands, text_system, headless, displays) = ready_rx.recv().expect("the display thread ended as it began");
        Display { commands, text_system, headless, next_window: AtomicU64::new(1), events: Mutex::new(None), displays: Mutex::new(displays), gone: AtomicBool::new(false) }
    }

    /// Waits for the display thread to end (asked to stop), `most` at
    /// most: a program that's quitting never hangs on it. (None: however
    /// long it takes, asleep meanwhile: a program running on for what it
    /// copied.)
    pub(crate) fn wait_until_ended(&self, most: Option<std::time::Duration>) {
        *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current());
        let until = most.map(|most| std::time::Instant::now() + most);
        while !ENDED.load(Ordering::Acquire) {
            match until {
                None => std::thread::park(),
                Some(until) => {
                    let now = std::time::Instant::now();
                    if now >= until {
                        break;
                    }
                    std::thread::park_timeout(until - now);
                }
            }
        }
    }

    /// Every display's bounds, in points, as last heard.
    pub(crate) fn displays(&self) -> Vec<crate::Bounds<crate::Pixels>> {
        self.displays.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, bounds)| *bounds).collect()
    }

    /// Every display's name (its output's: "eDP-1"), in [`Self::displays`]'
    /// order; None for one that has none yet.
    pub(crate) fn display_names(&self) -> Vec<Option<String>> {
        self.displays.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(name, _)| name.clone()).collect()
    }

    /// Keeps what displays there are now (on the display thread).
    fn note_displays(&self, on: &OnDisplay) {
        let now: Vec<(Option<String>, crate::Bounds<crate::Pixels>)> = on.client.displays().iter().map(|d| (d.name(), d.bounds())).collect();
        *self.displays.lock().unwrap_or_else(|e| e.into_inner()) = now;
    }

    /// Has the display thread run `f`, after what it was already given.
    /// Never waits. (Dropped if the thread has ended.)
    pub(crate) fn post(&self, f: impl FnOnce(&mut OnDisplay) + Send + 'static) {
        let _ = self.commands.send(Box::new(f));
    }

    /// Has the display thread run `f` and waits for what it gives. For
    /// what a window can't go on without (opening it); None if the
    /// thread has ended. The display thread never waits for a looper, so
    /// this can't deadlock, but it's a round trip: not for every frame.
    pub(crate) fn ask<T: Send + 'static>(&self, f: impl FnOnce(&mut OnDisplay) -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.post(move |on| {
            let _ = tx.send(f(on));
        });
        rx.recv().ok()
    }

    /// A number for a new window.
    pub(crate) fn next_window_id(&self) -> WindowId {
        WindowId(self.next_window.fetch_add(1, Ordering::Relaxed))
    }

    /// Who hears what happens to the program as a whole.
    pub(crate) fn set_events(&self, to: Option<Messenger<DisplayEvent>>) {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = to;
    }

    fn tell(&self, event: DisplayEvent) {
        if let Some(to) = self.events.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            to.send(event);
        }
    }

    /// Asks app_server to lock the session; what happens to the lock is
    /// told to the app's looper (with the displays there are then: the
    /// ones to cover).
    pub(crate) fn lock_session(&self) -> Result<()> {
        self.ask(|_| {
            crate::platform::lock_session_on_display(|event| {
                display().post(move |on| {
                    display().note_displays(on);
                    display().tell(DisplayEvent::SessionLock(event));
                });
            })
        })
        .unwrap_or_else(|| Err(anyhow::anyhow!("the display is gone")))
    }

    /// Ends the session lock: sent to app_server before this returns.
    pub(crate) fn unlock_session(&self) {
        let _ = self.ask(|_| crate::platform::unlock_session_on_display());
    }

    /// Whether the display thread has ended.
    pub(crate) fn is_gone(&self) -> bool {
        self.gone.load(Ordering::Acquire)
    }

    /// Opens a window: its surface made, its stage ready for what its
    /// looper commits. Waits for the display thread (a few milliseconds:
    /// the GPU's side of a window is made here).
    pub(crate) fn open_window(&self, id: WindowId, params: WindowParams, link: WindowLink) -> Result<crate::stage::Opened> {
        self.ask(move |on| {
            let fullscreen = params.fullscreen;
            let window = on.client.open_window(id, params)?;
            // (Asked for with its first commit: app_server gives it the
            // whole display as its first size. The spec said so and
            // nothing asked: Setup opened as a window.)
            if fullscreen {
                window.toggle_fullscreen();
            }
            let (stage, opened) = Stage::new(id, window, link);
            on.stages.insert(id, stage);
            Ok(opened)
        })
        .unwrap_or_else(|| Err(anyhow::anyhow!("the display thread has ended")))
    }
}
