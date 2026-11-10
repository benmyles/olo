//! Running an app and opening its windows.
//!
//! [`run`] starts the program (its look, its keyboard profile, the
//! agents' socket) and runs its looper on the main thread;
//! [`open_window`] opens a window on a looper of its own, the app's
//! view inside a [`Frame`], which paints the panel behind it and sets
//! Olo's font and the palette's text colour.

use olo_interface::{AnyChild, AppCx, Cx, Element, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowSpec, div, px};

use crate::{style, theme};

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
    pub layer: Option<olo_interface::LayerShellOptions>,
    /// A dialog (never a tab), modal or not.
    pub dialog: Option<olo_interface::Dialog>,
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
    /// What app_server knows this window by when the app comes back (a
    /// restart, an upgrade): its place, workspace, tab's place in its
    /// group and tile are this window's, not another's of the same title
    /// (the xdg-toplevel-tag word `restore:KEY`; [`Self::restore_key`]).
    pub restore_key: Option<String>,
    /// Opens full screen, on the display the person is at (asked before
    /// its first frame, so it's never drawn at `size` first): something
    /// that takes the whole of their attention, done with the keyboard
    /// (the palm trainer). `size` is what it's laid out at until then.
    pub fullscreen: bool,
    /// Opens at the size the window of this name was last left at, `size`
    /// the first time (`kept`): an app's main window, a document's.
    pub keep_size: Option<SharedString>,
    /// What it shows needs this height: in a window less tall (on a short
    /// screen) it's laid out at it all the same and scrolls (the window's
    /// `Frame` does it; a panel needn't).
    pub content_height: Option<f32>,
    /// As tall as what it shows needs (`size`'s height at least), never
    /// taller than the screen has room for, where it scrolls: see
    /// [`Self::fits_content`].
    pub fit_content: bool,
}

impl WindowOptions {
    pub fn new(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions {
            title: title.into(),
            size: (width, height),
            min_size: None,
            max_size: None,
            panel: true,
            floating: false,
            layer: None,
            dialog: None,
            fixed: false,
            lock_output: None,
            child: false,
            foreign_parent: None,
            under_pointer: false,
            restore_key: None,
            fullscreen: false,
            keep_size: None,
            content_height: None,
            fit_content: false,
        }
    }

    /// The window follows what it shows (HACKING, "Windows fit what they
    /// show"): a `fixed` one as tall as its content needs, `size`'s height
    /// at least, growing and shrinking with it (a tab with more rows, a
    /// display's brightness, a longer message); one the person sizes is
    /// never shorter than its content needs (grown when it needs more).
    /// Never taller than the shortest screen less room for its tab
    /// ([`screen_room`]): past that, its Frame scrolls what it shows, the
    /// window widened by the scroll bar. Every alert and dialog has it;
    /// give it to every other window whose content sets its size (forms,
    /// panels); a document's window is the person's to size. The content
    /// is laid out at its own height: a list in it shrinks (in a
    /// `flex_1().min_h_0()` box) and scrolls, never the whole of it.
    pub fn fits_content(mut self) -> Self {
        self.fit_content = true;
        self
    }

    /// A BAlert window `width` × `height`: fixed, modal over the app's
    /// window that has the keyboard (if any), never tiled or a tab.
    /// Follows what it shows (`fits_content`): `height` is its least.
    pub fn alert(title: impl Into<SharedString>, width: f32, height: f32) -> Self {
        WindowOptions { floating: true, dialog: Some(olo_interface::Dialog::Modal), fixed: true, fit_content: true, ..WindowOptions::new(title, width, height) }
    }

    /// A dialog (a panel, a sheet): floats, never a tab. Never smaller
    /// than what it shows needs, nor taller than the screen
    /// (`fits_content`).
    pub fn dialog(mut self) -> Self {
        self.floating = true;
        self.dialog = Some(olo_interface::Dialog::Plain);
        self.fit_content = true;
        self
    }

    /// A layer surface (see [`Self::layer`]), `namespace` saying what it
    /// is to the compositor, anchored to the edges given (top, bottom,
    /// left, right), on `layer`; refine it with [`Self::layer_options`].
    pub fn layer(mut self, namespace: &str, layer: olo_interface::LayerShellLayer, anchor: (bool, bool, bool, bool)) -> Self {
        self.layer = Some(olo_interface::LayerShellOptions {
            layer,
            anchor,
            exclusive_zone: 0,
            margin: (0, 0, 0, 0),
            keyboard: olo_interface::LayerKeyboard::None,
            namespace: namespace.to_owned(),
            output: None,
        });
        self
    }

    /// Changes the layer surface's options (exclusive zone, margins,
    /// keyboard, output).
    pub fn layer_options(mut self, f: impl FnOnce(&mut olo_interface::LayerShellOptions)) -> Self {
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

    /// Known by `key` when the app comes back (see [`Self::restore_key`]
    /// the field): the same window's key each time, unique among the
    /// app's windows (a document's path, a number the app keeps with the
    /// window). Spaces are taken out.
    pub fn restore_key(mut self, key: impl Into<String>) -> Self {
        self.restore_key = Some(key.into());
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

    /// Opens full screen (see [`Self::fullscreen`]).
    pub fn fullscreen(mut self) -> Self {
        self.fullscreen = true;
        self
    }

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// What it shows needs `height`: shorter, the window scrolls it.
    pub fn content_height(mut self, height: f32) -> Self {
        self.content_height = Some(height);
        self
    }

    /// Opens at the size the app's window called `name` was last left at
    /// (`size` until there is one), kept as the person changes it.
    pub fn keep_size(mut self, name: impl Into<SharedString>) -> Self {
        self.keep_size = Some(name.into());
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

impl WindowOptions {
    /// What the display thread is asked for.
    fn spec(&self) -> WindowSpec {
        let (mut w, mut h) = self.size;
        // (Held to what it may be now.)
        if let Some((min_w, min_h)) = self.min_size {
            (w, h) = (w.max(min_w), h.max(min_h));
        }
        if let Some((max_w, max_h)) = self.max_size {
            (w, h) = (if max_w > 0.0 { w.min(max_w) } else { w }, if max_h > 0.0 { h.min(max_h) } else { h });
        }
        // (A surface no bigger than nothing has no buffer to show:
        // app_server ends the connection of a program that offers one.)
        let (w, h) = (w.max(1.0), h.max(1.0));
        let restore = self.restore_key.as_ref().map(|k| format!("restore:{}", k.split_whitespace().collect::<String>()));
        let mut tags: Vec<&str> = [(self.floating, "floating"), (self.under_pointer, "under-pointer")].into_iter().filter_map(|(on, tag)| on.then_some(tag)).collect();
        tags.extend(restore.as_deref());
        WindowSpec {
            title: self.title.clone(),
            size: (w, h),
            min_size: self.min_size,
            max_size: self.max_size,
            kind: if self.lock_output.is_some() {
                olo_interface::WindowKind::SessionLock
            } else if self.layer.is_some() {
                olo_interface::WindowKind::LayerShell
            } else if self.child {
                // (Its parent is the window with the keyboard.)
                olo_interface::WindowKind::Floating
            } else {
                olo_interface::WindowKind::Normal
            },
            layer_shell: self.layer.clone(),
            dialog: self.dialog,
            toplevel_tag: (!tags.is_empty()).then(|| tags.join(" ")),
            foreign_parent: self.foreign_parent.clone(),
            session_lock_output: self.lock_output.clone(),
            // A layer that draws no panel (an overlay drawing its own
            // panel, the command palette's) shows what's under it.
            background: if self.layer.is_some() && !self.panel { olo_interface::WindowBackgroundAppearance::Transparent } else { olo_interface::WindowBackgroundAppearance::Opaque },
            fullscreen: self.fullscreen,
            resizable: !self.fixed,
            focus: self.layer.as_ref().is_none_or(|l| l.keyboard != olo_interface::LayerKeyboard::None),
        }
    }
}

/// A window's first view: the panel behind the app's view, Olo's font
/// and the palette's text colour for what it draws.
pub struct Frame {
    content: AnyChild,
    panel: bool,
}

impl View for Frame {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let mut root = div()
            .size_full()
            // A press anywhere in the window puts away a menu open over it
            // (and a cut text's tip).
            .on_window_mouse_down(|e, cx| {
                crate::cut::left(cx, true);
                crate::menu::press_in(e.position, cx)
            })
            // Text cut short: its whole in a tip (`cut`), at once with ⌘.
            // (Not while a button is held: a divider or a selection
            // dragged over cut text has no tip.)
            .on_window_mouse_move(|e, cx| {
                if e.pressed_button.is_some() {
                    crate::cut::left(cx, false);
                } else {
                    crate::cut::moved(cx, e.position, crate::cut::command_held(&e.modifiers))
                }
            })
            .on_hover(|over, cx| {
                if !*over {
                    crate::cut::left(cx, false);
                }
            })
            .on_modifiers_changed(|e, cx| {
                let at = cx.mouse_position();
                crate::cut::modifiers(cx, at, crate::cut::command_held(&e.modifiers))
            })
            // The menu bar's keys, when nothing in the window took them
            // (a text field's ⌘C is its own).
            .on_key_down(|e, cx| {
                if let Some((command, on_command)) = crate::menu::bar_shortcut(cx, &e.keystroke) {
                    on_command(&command, cx);
                    cx.stop_propagation();
                    return;
                }
                // ⌘W (the profile's Close) on a window whose app has no
                // Close of its own: closed as its tab's close button does,
                // the app's confirmations first.
                if crate::keys::pressed(&olo_kit_base::keys::Shortcut::command(crate::keys::evdev("w")), &e.keystroke) {
                    cx.request_close();
                    cx.stop_propagation();
                }
            })
            .font_family(style::UI_FONT)
            .text_size(px(style::PLAIN_SIZE))
            .line_height(olo_interface::relative(style::LINE_HEIGHT))
            .text_color(style::c(p.panel_text));
        if self.panel {
            root = root.bg(style::c(p.panel_background));
        }
        root.child(self.content.element())
    }
}

/// Opens a window on a looper of its own, showing the view `build`
/// makes there (the view the window's handle reaches).
pub fn open_window<V: View>(options: WindowOptions, build: impl FnOnce(&mut Cx<V>) -> V + Send + 'static) -> WindowHandle<V> {
    let panel = options.panel;
    let namespace = options.layer.as_ref().map(|l| l.namespace.clone());
    olo_interface::open_window(options.spec(), move |cx: &mut Cx<Frame>| {
        if let Some(namespace) = &namespace {
            crate::agent::set_layer(cx, namespace);
        }
        let content = cx.new_view(build);
        cx.set_main_view(&content);
        Frame { content: content.into(), panel }
    })
    .retyped()
}

/// Runs the program as `app_id` ("olo.deskcalc"): `build` opens its
/// windows; this returns when it quits.
pub fn run(app_id: &str, build: impl FnOnce(&mut AppCx)) {
    // Olo's own crates keep their info (what is rare and worth knowing
    // after), dependencies only their warnings.
    // (Ready for a crash first: its report, and stderr to the journal
    // when it went nowhere; `main` has done it already, as every
    // program's should.)
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,usvg=error,olo=info"));
    // Colours only for a terminal: not in the journal's lines.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(ansi).with_writer(std::io::stderr).try_init();
    olo_kit_base::keys::set_app_id(app_id);
    theme::init();
    olo_interface::run(app_id, |app| {
        crate::agent::init();
        build(app)
    });
    olo_kit_base::store::flush();
}
