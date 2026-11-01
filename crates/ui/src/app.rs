//! Running an app and opening its windows.

use std::any::Any;

use futures::{StreamExt, channel::mpsc};
use gpui::{AnyView, App, AppContext as _, Application, Bounds, Context, Entity, Global, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window, WindowBounds, WindowHandle, canvas, div, px, size};
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
}

impl WindowOptions {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { title: title.into(), size: (width, height), min_size: None, panel: true, floating: false }
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
}

impl Frame {
    pub fn set_title(&mut self, title: impl Into<SharedString>, window: &mut Window) {
        self.title = title.into();
        window.set_window_title(&self.title);
    }
}

impl Render for Frame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        agent::begin_frame(window, cx, &self.title);
        let p = theme::palette(cx);
        let mut root = div().size_full().font_family(style::UI_FONT).text_size(px(style::PLAIN_SIZE)).text_color(style::c(p.panel_text));
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
            ..Default::default()
        },
        move |window, cx| {
            let content = build(window, cx).into();
            cx.new(|_| Frame { title, content, panel })
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

/// Runs the app: its theme, its socket for agents, and `build`, which
/// opens its windows. It ends when its last window closes.
pub fn run(app_id: &str, build: impl FnOnce(&mut App) + 'static) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let app_id = app_id.to_owned();
    Application::new().run(move |cx: &mut App| {
        cx.set_global(AppId(app_id.clone()));
        theme::init(cx);
        agent::init(cx, &app_id);
        crate::widgets::bind_keys(cx);
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
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        build(cx);
    });
}
