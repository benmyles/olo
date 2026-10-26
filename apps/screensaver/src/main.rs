//! tabula-screensaver: app_server starts it after idleness (see
//! app_server's saver.rs) on a connection of its own and ends it at the
//! first input. It covers every output with an overlay and runs the chosen
//! module from tabula-screensavers on each, with the pointer hidden.
//!
//! `--module NAME --options JSON [--desktop PNG]`: the desktop picture,
//! for Lens and Flip, is written by app_server as the saver starts; it is
//! read on a thread and the module waits for it (black meanwhile).

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use tabula_kit::{
    App, Cx, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Color32, ColorImage, TextureHandle, TextureOptions},
};
use tabula_screensavers::{Frame, Info, Module, Options};

const APP_ID: &str = "tabula.screensaver";

/// The desktop picture, once read (None: there is none to wait for).
enum Desktop {
    Waiting(Instant),
    Ready(ColorImage),
    Loaded(TextureHandle),
    None,
}

struct Screen {
    module: Box<dyn Module>,
    last: Option<Instant>,
}

struct Saver {
    info: &'static Info,
    options: Options,
    screens: HashMap<WindowId, Screen>,
    desktop: Desktop,
}

/// How long to wait for app_server's picture of the desktop.
const DESKTOP_WAIT: Duration = Duration::from_secs(2);

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

/// The largest texture side egui takes (the GPU's limit can be lower than
/// a HiDPI screen).
const MAX_TEXTURE: usize = 2048;

/// Half the size, averaging 2×2 blocks.
fn halve(img: &ColorImage) -> ColorImage {
    let [w, h] = img.size;
    let (nw, nh) = (w / 2, h / 2);
    let px = (0..nh)
        .flat_map(|y| (0..nw).map(move |x| (x, y)))
        .map(|(x, y)| {
            let at = |dx: usize, dy: usize| img.pixels[(2 * y + dy) * w + 2 * x + dx].to_array();
            let q = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            let avg = |i: usize| ((q[0][i] as u32 + q[1][i] as u32 + q[2][i] as u32 + q[3][i] as u32) / 4) as u8;
            Color32::from_rgba_premultiplied(avg(0), avg(1), avg(2), avg(3))
        })
        .collect();
    ColorImage::new([nw, nh], px)
}

/// Read the picture once app_server has written all of it.
fn read_desktop(path: PathBuf, proxy: tabula_kit::Proxy) {
    std::thread::Builder::new()
        .name("desktop".into())
        .spawn(move || {
            let start = Instant::now();
            while start.elapsed() < DESKTOP_WAIT {
                if let Some(img) = tabula_xdg::decode_png(&path) {
                    let _ = std::fs::remove_file(&path);
                    let px = img.rgba.as_chunks::<4>().0.iter().map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]));
                    let mut image = ColorImage::new([img.width as usize, img.height as usize], px.collect());
                    while image.size[0].max(image.size[1]) > MAX_TEXTURE {
                        image = halve(&image);
                    }
                    proxy.send(Some(image));
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            proxy.send(None::<ColorImage>);
        })
        .expect("thread");
}

impl App for Saver {
    fn ui(&mut self, _cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ctx.set_cursor_icon(egui::CursorIcon::None);
        ctx.request_repaint();
        let rect = ui.max_rect();
        if let Desktop::Ready(_) = self.desktop {
            let Desktop::Ready(image) = std::mem::replace(&mut self.desktop, Desktop::None) else { unreachable!() };
            self.desktop = Desktop::Loaded(ctx.load_texture("desktop", image, TextureOptions::LINEAR));
        }
        let desktop = match &self.desktop {
            Desktop::Waiting(since) if since.elapsed() < DESKTOP_WAIT => {
                ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
                return;
            }
            Desktop::Loaded(t) => Some((t.id(), t.size_vec2())),
            _ => None,
        };
        let (info, options) = (self.info, &self.options);
        let screen = self.screens.entry(window).or_insert_with(|| Screen {
            module: info.make(options, tabula_screensavers::seed() ^ window_seed(window)),
            last: None,
        });
        let now = Instant::now();
        let dt = screen.last.map_or(0.0, |t| (now - t).as_secs_f32());
        screen.last = Some(now);
        screen.module.draw(&Frame { painter: ui.painter(), rect, dt, desktop });
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if let Ok(image) = message.downcast::<Option<ColorImage>>() {
            self.desktop = match *image {
                Some(image) => Desktop::Ready(image),
                None => Desktop::None,
            };
            cx.redraw_all();
        }
    }
}

/// Different screens, different shows.
fn window_seed(w: WindowId) -> u64 {
    format!("{w:?}").bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
}

fn main() {
    let (module, options, desktop) = args();
    let info = tabula_screensavers::find(&module).unwrap_or(&tabula_screensavers::MODULES[0]);
    tabula_kit::run(APP_ID, |cx| {
        let mut outputs: Vec<Option<String>> = cx.outputs().iter().cloned().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        for output in outputs {
            cx.open_window(WindowOptions {
                title: "Screen Saver".into(),
                size: (1, 1),
                kind: WindowKind::Layer(LayerOptions {
                    layer: Layer::Overlay,
                    anchor: (true, true, true, true),
                    // Over the Deskbar too.
                    exclusive_zone: -1,
                    margin: (0, 0, 0, 0),
                    keyboard: true,
                    namespace: "screensaver".into(),
                    output,
                }),
                panel: false,
                ..Default::default()
            });
        }
        let desktop = match desktop {
            Some(path) if info.uses_desktop => {
                read_desktop(path, cx.proxy());
                Desktop::Waiting(Instant::now())
            }
            _ => Desktop::None,
        };
        Saver { info, options, screens: HashMap::new(), desktop }
    });
}
