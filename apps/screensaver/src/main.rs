//! olo-screensaver: app_server starts it after idleness (see
//! app_server's saver.rs) on a connection of its own and ends it at the
//! first input. It covers every output with an overlay and runs the chosen
//! module from olo-screensavers on each, with the pointer hidden, drawn
//! with Olo's kit (`olo_screensavers::kit_canvas`), a frame each time the
//! display can show one: each screen is a window on a looper of its own,
//! so one screen's show doesn't wait for another's.
//!
//! `--module NAME --options JSON [--desktop PNG]`: the desktop picture,
//! for Lens and Flip, is written by app_server as the saver starts; it is
//! read on a thread and the module waits for it (black meanwhile).

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_kit::{CursorStyle, Cx, Element, LayerKeyboard, LayerShellLayer, RenderImage, Styled as _, View, WindowHandle, WindowOptions, view_canvas};
use olo_screensavers::{Frame, Info, Module, Options, kit_canvas::KitCanvas, pos2};

const APP_ID: &str = "olo.screensaver";

/// How long to wait for app_server's picture of the desktop.
const DESKTOP_WAIT: Duration = Duration::from_secs(2);
/// The largest side the picture is kept at (larger is halved): the
/// modules show it whole on one screen.
const MAX_SIDE: u32 = 4096;

/// The desktop picture, once read (None: there is none to wait for).
enum Desktop {
    Waiting(Instant),
    Ready(Arc<RenderImage>),
    None,
}

/// A screen's show.
struct Screen {
    desktop: Desktop,
    module: RefCell<Box<dyn Module>>,
    last: Cell<Option<Instant>>,
}

fn args() -> (String, Options, Option<PathBuf>) {
    let mut module = "Blackness".to_owned();
    let mut options = Options::new();
    let mut desktop = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--module" => module = it.next().unwrap_or(module),
            "--options" => options = it.next().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            "--desktop" => desktop = it.next().map(PathBuf::from),
            _ => {}
        }
    }
    (module, options, desktop)
}

/// Half the size, averaging 2×2 blocks of straight RGBA.
fn halve(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let at = |dx: u32, dy: u32, i: usize| rgba[(((2 * y + dy) * w + 2 * x + dx) * 4) as usize + i] as u32;
            for i in 0..4 {
                out.push(((at(0, 0, i) + at(1, 0, i) + at(0, 1, i) + at(1, 1, i)) / 4) as u8);
            }
        }
    }
    (out, nw, nh)
}

/// Read the picture once app_server has written all of it.
fn read_desktop(path: PathBuf, proxy: olo_kit::Proxy) {
    std::thread::Builder::new()
        .name("desktop".into())
        .spawn(move || {
            let start = Instant::now();
            while start.elapsed() < DESKTOP_WAIT {
                if let Some(img) = olo_xdg::decode_png(&path) {
                    let _ = std::fs::remove_file(&path);
                    let (mut rgba, mut w, mut h) = (img.rgba, img.width, img.height);
                    while w.max(h) > MAX_SIDE {
                        (rgba, w, h) = halve(&rgba, w, h);
                    }
                    proxy.send(olo_screensavers::kit_canvas::desktop_image(rgba, w, h));
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            proxy.send(None::<Arc<RenderImage>>);
        })
        .expect("thread");
}

impl View for Screen {
    /// The desktop picture, read (or none after all).
    type Message = Option<Arc<RenderImage>>;

    fn message(&mut self, image: Self::Message, cx: &mut Cx<Self>) {
        self.desktop = match image {
            Some(image) => Desktop::Ready(image),
            None => Desktop::None,
        };
        cx.invalidate();
    }

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        view_canvas(|screen: &Screen, g, b| {
            g.set_cursor_style(CursorStyle::None, b);
            // A frame each time the display can show one, for as long
            // as the saver runs (only this drawing: nothing else is
            // worked out between frames).
            g.paint_again_within(0.0);
            let rect = olo_screensavers::Rect::from_min_max(pos2(f32::from(b.left()), f32::from(b.top())), pos2(f32::from(b.right()), f32::from(b.bottom())));
            let desktop = match &screen.desktop {
                Desktop::Waiting(since) if since.elapsed() < DESKTOP_WAIT => {
                    g.paint_quad(olo_kit::fill(b, olo_kit::black()));
                    return;
                }
                Desktop::Ready(image) => Some(image.clone()),
                _ => None,
            };
            let now = Instant::now();
            let dt = screen.last.get().map_or(0.0, |t| (now - t).as_secs_f32());
            screen.last.set(Some(now));
            let canvas = KitCanvas::new(g, rect, desktop);
            screen.module.borrow_mut().draw(&Frame { canvas: &canvas, rect, dt });
            canvas.finish();
        })
        .size_full()
    }
}

/// Different screens, different shows.
fn screen_seed(output: &Option<String>, index: usize) -> u64 {
    output.as_deref().unwrap_or("").bytes().fold(index as u64 + 1, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let (module, options, desktop) = args();
    let info: &'static Info = olo_screensavers::find(&module).unwrap_or(&olo_screensavers::MODULES[0]);
    olo_kit::run(APP_ID, move |app| {
        if !olo_kit::has_compositor() {
            // Nothing to draw on: nothing to do.
            eprintln!("olo-screensaver: no screen to cover");
            return olo_kit::quit();
        }
        let waits = desktop.is_some() && info.uses_desktop;
        let mut outputs: Vec<Option<String>> = olo_kit::display_names().into_iter().flatten().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        let mut screens: Vec<WindowHandle<Screen>> = Vec::new();
        for (index, output) in outputs.into_iter().enumerate() {
            let seed = olo_screensavers::seed() ^ screen_seed(&output, index);
            let options_for = options.clone();
            let window = WindowOptions::new("Screen Saver", 1.0, 1.0).layer("screensaver", LayerShellLayer::Overlay, (true, true, true, true)).no_panel().layer_options(|l| {
                // Over the Deskbar too.
                l.exclusive_zone = -1;
                l.keyboard = LayerKeyboard::OnDemand;
                l.output = output;
            });
            screens.push(olo_kit::open_window(window, move |_| Screen {
                desktop: if waits { Desktop::Waiting(Instant::now()) } else { Desktop::None },
                module: RefCell::new(info.make(&options_for, seed)),
                last: Cell::new(None),
            }));
        }
        // The desktop picture, once read, to every screen.
        if let (Some(path), true) = (desktop, waits) {
            read_desktop(path, olo_kit::proxy());
        }
        olo_kit::message_handler(app, move |m, _| {
            if let Ok(image) = m.downcast::<Option<Arc<RenderImage>>>() {
                for screen in &screens {
                    let _ = screen.messenger().send((*image).clone());
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn halving_averages() {
        let rgba = [0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 100, 100, 100, 255];
        let (out, w, h) = super::halve(&rgba, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![100, 100, 100, 255]);
    }
}
