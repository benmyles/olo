//! Running an app and opening its windows.

use std::any::Any;

use futures::{StreamExt, channel::mpsc};
use gpui::{AnyView, App, AppContext as _, Application, Bounds, Context, Entity, FocusHandle, Global, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window, WindowBounds, WindowHandle, canvas, div, px, size};
use olo_kit_base::Proxy;

use crate::{agent, style, theme};

/// How a new window should appear.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: SharedString,
    /// The content's logical size (decorations are app_server's).
    pub size: (f32, f32),
    pub min_size: Option<(f32, f32)>,
    /// Its largest (0: free along that side); no larger than its smallest
    /// along a side, it's fixed along it, and app_server neither tiles
    /// nor groups it (a Find panel's height).
    pub max_size: Option<(f32, f32)>,
    /// Paint the panel colour behind the content (most windows want it).
    pub panel: bool,
    /// A utility window (a small tool such as DeskCalc) that floats over
    /// auto-tiled windows rather than taking a tile (the xdg-toplevel-tag
    /// `floating`).
    pub floating: bool,
    /// A surface of the desktop's own (Deskbar, notifications, an
    /// overlay) rather than a window: where it sits.
    pub layer: Option<gpui::LayerShellOptions>,
    /// A dialog (never a tab), modal or not.
    pub dialog: Option<gpui::Dialog>,
    /// Never resized (an alert).
    pub fixed: bool,
    /// Covers this output while the session is locked (`lock_session`).
    pub lock_output: Option<String>,
    /// Belongs to the app's window that has the keyboard (a chat's Info
    /// over its chat): opens over it, never a tab beside it.
    pub child: bool,
    /// Belongs to another program's window, by the handle it exported
    /// (xdg-foreign): a portal's dialog over the app that asked.
    pub foreign_parent: Option<String>,
    /// Opens with its tab under the pointer (the xdg-toplevel-tag
    /// `under-pointer`): a tab torn off a strip, to move with it
    /// ([`begin_move`]).
    pub under_pointer: bool,
}

impl WindowOptions {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { title: title.into(), size: (width, height), min_size: None, max_size: None, panel: true, floating: false, layer: None, dialog: None, fixed: false, lock_output: None, child: false, foreign_parent: None, under_pointer: false }
    }

    /// A BAlert window `width` × `height`: fixed, modal over the app's
    /// window that has the keyboard (if any), never tiled or a tab.
    pub fn alert(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { floating: true, dialog: Some(gpui::Dialog::Modal), fixed: true, ..WindowOptions::new(title, width, height) }
    }

    /// A dialog (a panel, a sheet): floats, never a tab.
    pub fn dialog(mut self) -> Self {
        self.floating = true;
        self.dialog = Some(gpui::Dialog::Plain);
        self
    }

    /// A layer surface (see [`Self::layer`]), `namespace` saying what it
    /// is to the compositor, anchored to the edges given (top, bottom,
    /// left, right), on `layer`; refine it with [`Self::layer_options`].
    pub fn layer(mut self, namespace: &str, layer: gpui::LayerShellLayer, anchor: (bool, bool, bool, bool)) -> Self {
        self.layer = Some(gpui::LayerShellOptions {
            layer,
            anchor,
            exclusive_zone: 0,
            margin: (0, 0, 0, 0),
            keyboard: gpui::LayerKeyboard::None,
            namespace: namespace.to_owned(),
            output: None,
        });
        self
    }

    /// Changes the layer surface's options (exclusive zone, margins,
    /// keyboard, output).
    pub fn layer_options(mut self, f: impl FnOnce(&mut gpui::LayerShellOptions)) -> Self {
        if let Some(l) = self.layer.as_mut() {
            f(l);
        }
        self
    }

    /// Without the panel colour behind it (an overlay draws all of it).
    pub fn no_panel(mut self) -> Self {
        self.panel = false;
        self
    }

    /// Floats over auto-tiled windows (see [`Self::floating`]).
    pub fn floating(mut self) -> Self {
        self.floating = true;
        self
    }

    /// Opens over the app's window that has the keyboard, as its own
    /// (see [`Self::child`]).
    pub fn child(mut self) -> Self {
        self.child = true;
        self
    }

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    pub fn max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = Some((width, height));
        self
    }

    /// Opens over another program's window (see [`Self::foreign_parent`]),
    /// given as a portal gives it (`wayland:HANDLE`, or the handle
    /// alone); nothing if empty.
    pub fn foreign_parent(mut self, handle: impl Into<String>) -> Self {
        let handle = handle.into();
        self.foreign_parent = Some(handle.strip_prefix("wayland:").map(str::to_owned).unwrap_or(handle)).filter(|h| !h.is_empty());
        self
    }
}

/// A window's root: the panel behind the app's view, Olo's font and the
/// palette's text colour for what it draws, and the start of each frame
/// for agents ([`agent`]).
pub struct Frame {
    title: SharedString,
    content: AnyView,
    panel: bool,
    /// Has the keyboard when nothing in the window does, so the menu
    /// bar's keys reach it.
    focus: FocusHandle,
}

impl Frame {
    pub fn set_title(&mut self, title: impl Into<SharedString>, window: &mut Window) {
        self.title = title.into();
        window.set_window_title(&self.title);
    }
}

gpui::actions!(olo_frame, [FocusNext, FocusPrev]);

type DefaultClick = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Each window's default button, as last drawn.
#[derive(Default)]
struct DefaultButtons(std::collections::HashMap<gpui::WindowId, DefaultClick>);

impl Global for DefaultButtons {}

/// The window's default button is drawn: Return clicks it.
pub(crate) fn set_default_button(window: &Window, cx: &mut App, click: DefaultClick) {
    cx.default_global::<DefaultButtons>().0.insert(window.window_handle().window_id(), click);
}

/// The window's default button, if one is drawn (and can be clicked).
pub(crate) fn default_button(window: &Window, cx: &App) -> Option<DefaultClick> {
    cx.try_global::<DefaultButtons>().and_then(|d| d.0.get(&window.window_handle().window_id())).cloned()
}

type TabStep = std::rc::Rc<dyn Fn(i32, &mut Window, &mut App)>;

/// Each window's tab keys' taker (its first tab view), as last drawn.
#[derive(Default)]
struct TabKeys(std::collections::HashMap<gpui::WindowId, TabStep>);

impl Global for TabKeys {}

/// A tab view is drawn: Next and Previous Tab step it, unless one drawn
/// before it in the window has them.
pub(crate) fn set_tab_keys(window: &Window, cx: &mut App, step: TabStep) {
    cx.default_global::<TabKeys>().0.entry(window.window_handle().window_id()).or_insert(step);
}

/// Next Tab (+1: ⌃Tab, ⌃Page Down) or Previous Tab (−1: ⌃⇧Tab, ⌃Page Up);
/// 0 for any other key.
pub fn tab_step(k: &gpui::Keystroke) -> i32 {
    let m = &k.modifiers;
    if !m.control || m.alt || m.platform {
        return 0;
    }
    match k.key.as_str() {
        "tab" if m.shift => -1,
        "tab" => 1,
        "pageup" if !m.shift => -1,
        "pagedown" if !m.shift => 1,
        _ => 0,
    }
}

/// Tab and ⇧Tab move the keyboard through the window's controls, fields
/// and lists, as BeOS's keyboard navigation does (a view of its own that
/// takes Tab, a terminal or an editor, binds it in its key context).
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new("tab", FocusNext, None), gpui::KeyBinding::new("shift-tab", FocusPrev, None)]);
}

impl Render for Frame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        agent::begin_frame(window, cx, &self.title);
        // Drawn again below if there still is one.
        cx.default_global::<DefaultButtons>().0.remove(&window.window_handle().window_id());
        cx.default_global::<TabKeys>().0.remove(&window.window_handle().window_id());
        let p = theme::palette(cx);
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            // A press anywhere in the window puts away a menu open over it.
            .capture_any_mouse_down(|e, window, cx| crate::menu::press_in(e.position, window, cx))
            .on_action(|_: &FocusNext, window, _| window.focus_next())
            .on_action(|_: &FocusPrev, window, _| window.focus_prev())
            // The menu bar's keys, when nothing in the window took them
            // (a text field's ⌘C is its own).
            .on_key_down(|e: &KeyDownEvent, window, cx| {
                if let Some((command, on_command)) = crate::menu::bar_shortcut(window, cx, &e.keystroke) {
                    on_command(&command, window, cx);
                    cx.stop_propagation();
                    return;
                }
                // Next and Previous Tab: the window's tab view.
                let step = tab_step(&e.keystroke);
                if step != 0
                    && let Some(f) = cx.try_global::<TabKeys>().and_then(|t| t.0.get(&window.window_handle().window_id())).cloned()
                {
                    f(step, window, cx);
                    cx.stop_propagation();
                    return;
                }
                // Return nothing took: the default button.
                if e.keystroke.key == "enter"
                    && !e.keystroke.modifiers.modified()
                    && let Some(click) = cx.try_global::<DefaultButtons>().and_then(|d| d.0.get(&window.window_handle().window_id())).cloned()
                {
                    click(window, cx);
                    cx.stop_propagation();
                }
            })
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(gpui::relative(style::LINE_HEIGHT))
            .text_color(style::c(p.panel_text));
        if self.panel {
            root = root.bg(style::c(p.panel_background));
        }
        let title = self.title.clone();
        // Last: the frame is drawn (for a review asked for).
        let done = canvas(|_, _, _| {}, move |_, _, window, _| crate::review::finish(window, &title)).absolute().size_0();
        root.child(self.content.clone()).child(done)
    }
}

struct AppId(String);

impl Global for AppId {}

/// Moves `window` with the pointer, as if dragged by its tab, while the
/// button pressed in another of the app's windows is still held: a tab
/// torn off a strip, now a window (opened [`WindowOptions::under_pointer`]),
/// follows the pointer, and let go over another window's strip goes in
/// there (`widgets::strip::on_window_dropped`). Once it has drawn (a
/// window that isn't on screen yet can't be moved).
pub fn begin_move(window: &mut Window) {
    window.on_next_frame(|window, _| window.on_next_frame(|window, _| window.start_window_move()));
}

/// The app's id ("olo.deskcalc"), as [`run`] was given it.
pub fn app_id(cx: &App) -> String {
    cx.try_global::<AppId>().map(|a| a.0.clone()).unwrap_or_default()
}

/// Opens a window showing the view `build` makes.
pub fn open_window<V: Render + 'static>(cx: &mut App, options: WindowOptions, build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> Option<WindowHandle<Frame>> {
    // (A surface no bigger than nothing has no buffer to show: app_server
    // ends the connection of a program that offers one.)
    let (w, h) = (options.size.0.max(1.0), options.size.1.max(1.0));
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let title = options.title.clone();
    let panel = options.panel;
    let namespace = options.layer.as_ref().map(|l| l.namespace.clone());
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
            app_id: Some(app_id(cx)),
            window_min_size: options.min_size.map(|(w, h)| size(px(w), px(h))),
            window_max_size: options.max_size.map(|(w, h)| size(px(w), px(h))),
            toplevel_tag: {
                let tags: Vec<&str> = [(options.floating, "floating"), (options.under_pointer, "under-pointer")].into_iter().filter_map(|(on, tag)| on.then_some(tag)).collect();
                (!tags.is_empty()).then(|| tags.join(" "))
            },
            kind: if options.lock_output.is_some() {
                gpui::WindowKind::SessionLock
            } else if options.layer.is_some() {
                gpui::WindowKind::LayerShell
            } else if options.child {
                // (Its parent is the window with the keyboard.)
                gpui::WindowKind::Floating
            } else {
                gpui::WindowKind::Normal
            },
            session_lock_output: options.lock_output.clone(),
            // A layer that draws no panel (an overlay drawing its own
            // panel, the command palette's) shows what's under it.
            window_background: if options.layer.is_some() && !options.panel { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque },
            layer_shell: options.layer.clone(),
            dialog: options.dialog,
            foreign_parent: options.foreign_parent.clone(),
            is_resizable: !options.fixed,
            focus: options.layer.as_ref().is_none_or(|l| l.keyboard != gpui::LayerKeyboard::None),
            ..Default::default()
        },
        move |window, cx| {
            if let Some(n) = &namespace {
                agent::set_layer(window, cx, n);
            }
            let content = build(window, cx).into();
            let frame = cx.new(|cx| Frame { title, content, panel, focus: cx.focus_handle() });
            if window.focused(cx).is_none() {
                let focus = frame.read(cx).focus.clone();
                window.focus(&focus);
            }
            frame
        },
    )
    .ok()
}

/// What the app does with messages from other threads ([`proxy`]).
struct Messages {
    handler: Option<Box<dyn FnMut(Box<dyn Any + Send>, &mut App)>>,
    proxy: Proxy,
    /// Its last window closed and it is ending: a launch forwarded now is
    /// refused (the launching process starts a copy of its own).
    quitting: bool,
}

impl Global for Messages {}

/// Something any thread can send messages to the app with (a launch
/// forwarded by another copy, a worker's result); [`message_handler`]
/// takes them on the UI thread.
pub fn proxy(cx: &App) -> Proxy {
    cx.global::<Messages>().proxy.clone()
}

/// Takes the messages sent through [`proxy`].
pub fn message_handler(cx: &mut App, handler: impl FnMut(Box<dyn Any + Send>, &mut App) + 'static) {
    cx.global_mut::<Messages>().handler = Some(Box::new(handler));
}

struct Resident;

impl Global for Resident {}

/// Keeps the app running with no windows open (an agent that opens them
/// when asked: the polkit agent, Notifications).
pub fn stay_resident(cx: &mut App) {
    cx.set_global(Resident);
}

/// Runs the app: its theme, its socket for agents, and `build`, which
/// opens its windows. It ends when its last window closes.
pub fn run(app_id: &str, build: impl FnOnce(&mut App) + 'static) {
    // The state an upgrade handed over, its file gone whether or not the
    // app asks for it.
    olo_kit_base::restore::claim();
    crate::upgrade::claim();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let app_id = app_id.to_owned();
    Application::new().run(move |cx: &mut App| {
        cx.set_global(AppId(app_id.clone()));
        olo_kit_base::keys::set_app_id(&app_id);
        theme::init(cx);
        agent::init(cx, &app_id);
        // The window's own keys first: a binding in a view's key context
        // as deep as a window-wide one wins only when it was bound later.
        bind_keys(cx);
        crate::widgets::bind_keys(cx);
        crate::editor::bind_keys(cx);
        crate::upgrade::init(cx);
        crate::icons::init(cx);
        let (tx, mut rx) = mpsc::unbounded::<Box<dyn Any + Send>>();
        let proxy = Proxy::new(move |m| {
            let _ = tx.unbounded_send(m);
        });
        cx.set_global(Messages { handler: None, proxy, quitting: false });
        cx.spawn(async move |cx| {
            while let Some(message) = rx.next().await {
                let _ = cx.update(|cx| {
                    let messages = cx.global_mut::<Messages>();
                    // A launch forwarded from another process is told the
                    // app took it, or dropped unanswered (so that process
                    // opens a copy of its own) if the app is ending or has
                    // nothing to take it with, or the process gave up.
                    if let Some(launch) = message.downcast_ref::<olo_kit_base::instance::Launch>()
                        && (messages.quitting || messages.handler.is_none() || !launch.take())
                    {
                        return;
                    }
                    if let Some(mut handler) = messages.handler.take() {
                        handler(message, cx);
                        cx.global_mut::<Messages>().handler = Some(handler);
                    }
                });
            }
        })
        .detach();
        cx.on_window_closed(|cx| {
            agent::forget_closed(cx);
            let open: std::collections::HashSet<gpui::WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
            cx.default_global::<DefaultButtons>().0.retain(|id, _| open.contains(id));
            cx.default_global::<TabKeys>().0.retain(|id, _| open.contains(id));
            crate::menu::forget_closed(cx);
            if cx.windows().is_empty() && !cx.has_global::<Resident>() {
                cx.global_mut::<Messages>().quitting = true;
                cx.quit();
            }
        })
        .detach();
        build(cx);
    });
}

/// Called when a display came or went, as the compositor told this app
/// (its `cx.displays()` lists it then). A window for a display the app
/// is told of some other way (app_server's `OutputsChanged`) may be
/// asked for before that, and refused: ask again from here.
pub fn on_displays_changed(cx: &mut App, f: impl FnMut(&mut App) + 'static) {
    // Deferred: the platform tells of a display inside a Wayland roundtrip,
    // which a window being opened makes, so this may come while the app's
    // entity is being updated (Deskbar: "cannot update while it is already
    // being updated", 2026-11-01). After that update, then.
    let f = std::rc::Rc::new(std::cell::RefCell::new(f));
    cx.on_displays_changed(move |cx| {
        let f = f.clone();
        cx.defer(move |cx| (f.borrow_mut())(cx));
    })
    .detach();
}
