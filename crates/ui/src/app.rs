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
}

impl WindowOptions {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { title: title.into(), size: (width, height), min_size: None, panel: true, floating: false, layer: None, dialog: None, fixed: false, lock_output: None }
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

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));
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
        let p = theme::palette(cx);
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
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

/// The app's id ("olo.deskcalc"), as [`run`] was given it.
pub fn app_id(cx: &App) -> String {
    cx.try_global::<AppId>().map(|a| a.0.clone()).unwrap_or_default()
}

/// Opens a window showing the view `build` makes.
pub fn open_window<V: Render + 'static>(cx: &mut App, options: WindowOptions, build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> Option<WindowHandle<Frame>> {
    let (w, h) = options.size;
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let title = options.title.clone();
    let panel = options.panel;
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
            app_id: Some(app_id(cx)),
            window_min_size: options.min_size.map(|(w, h)| size(px(w), px(h))),
            toplevel_tag: options.floating.then(|| "floating".to_string()),
            kind: if options.lock_output.is_some() {
                gpui::WindowKind::SessionLock
            } else if options.layer.is_some() {
                gpui::WindowKind::LayerShell
            } else {
                gpui::WindowKind::Normal
            },
            session_lock_output: options.lock_output.clone(),
            // A layer that draws no panel (an overlay drawing its own
            // panel, the command palette's) shows what's under it.
            window_background: if options.layer.is_some() && !options.panel { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque },
            layer_shell: options.layer.clone(),
            dialog: options.dialog,
            is_resizable: !options.fixed,
            focus: options.layer.as_ref().is_none_or(|l| l.keyboard != gpui::LayerKeyboard::None),
            ..Default::default()
        },
        move |window, cx| {
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
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let app_id = app_id.to_owned();
    Application::new().run(move |cx: &mut App| {
        cx.set_global(AppId(app_id.clone()));
        olo_kit_base::keys::set_app_id(&app_id);
        theme::init(cx);
        agent::init(cx, &app_id);
        crate::widgets::bind_keys(cx);
        bind_keys(cx);
        crate::upgrade::init(cx);
        crate::icons::init(cx);
        let (tx, mut rx) = mpsc::unbounded::<Box<dyn Any + Send>>();
        let proxy = Proxy::new(move |m| {
            let _ = tx.unbounded_send(m);
        });
        cx.set_global(Messages { handler: None, proxy });
        cx.spawn(async move |cx| {
            while let Some(message) = rx.next().await {
                let _ = cx.update(|cx| {
                    let handler = cx.global_mut::<Messages>().handler.take();
                    if let Some(mut handler) = handler {
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
            crate::menu::forget_closed(cx);
            if cx.windows().is_empty() && !cx.has_global::<Resident>() {
                cx.quit();
            }
        })
        .detach();
        build(cx);
    });
}
