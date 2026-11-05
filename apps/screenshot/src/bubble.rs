//! The camera bubble (`olo-screenshot --camera-bubble [--camera ID]`):
//! the camera in a circle low in the screen's right corner, over
//! everything, as CleanShot X shows the presenter in a recording. The
//! recorder starts it with a recording that asks (`--camera`, Record… ▸
//! Show Camera) and ends it after: on the screen, so the person sees
//! themselves and a recording of the screen or a region has it. Dragged,
//! it moves (its layer surface's margins). Frames come from the camera
//! (`olo_camera`) on a worker: the middle square, mirrored as a mirror
//! shows you; the window takes only the latest (one waiting at a time).
//! It says "ready" and the camera's name on its standard output once the
//! camera's first frame is in, or why not and quits.

use olo_sync::Mutex;
use std::{io::Write as _, sync::Arc};

use olo_ui::{
    WindowOptions,
    gpui::{
        self, App, AppContext as _, BoxShadow, Context, Corners, InteractiveElement as _, IntoElement, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement as _, Pixels, Point, Render, RenderImage, Styled as _, Window, canvas, div, point, px,
    },
    theme,
};

const APP_ID: &str = "olo.screenshot";
/// The bubble's size and its margin from the screen's edges (points), and
/// room around it for its shadow.
const SIDE: f32 = 176.0;
const MARGIN: i32 = 24;
const SHADOW: f32 = 10.0;

/// The latest frame (BGRA, square), waiting for the window; and whether
/// the window has been told.
type Latest = Arc<Mutex<(Option<image::RgbaImage>, bool)>>;

enum Msg {
    Frame,
    Failed(String),
}

struct Bubble {
    latest: Latest,
    name: String,
    frame: Option<Arc<RenderImage>>,
    dropped: Vec<Arc<RenderImage>>,
    /// Its margins from the right and bottom edges (points).
    margin: (i32, i32),
    /// Being dragged: where it was taken (points in the window).
    drag: Option<Point<Pixels>>,
}

impl Bubble {
    fn message(&mut self, m: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match m {
            Msg::Frame => {
                let img = {
                    let mut l = self.latest.lock().unwrap_or_else(|e| e.into_inner());
                    l.1 = false;
                    l.0.take()
                };
                if let Some(img) = img {
                    let first = self.frame.is_none();
                    self.dropped.extend(self.frame.replace(Arc::new(RenderImage::new([image::Frame::new(img)]))));
                    if first {
                        println!("ready\t{}", self.name);
                        let _ = std::io::stdout().flush();
                    }
                    cx.notify();
                }
            }
            Msg::Failed(e) => {
                println!("{e}");
                let _ = std::io::stdout().flush();
                let _ = window;
                cx.quit();
            }
        }
    }

    /// Moved by `by` (points): its margins less that, kept on the screen.
    fn moved(&mut self, by: Point<Pixels>, window: &mut Window, cx: &App) {
        let screen = window.display(cx).map(|d| d.bounds().size).unwrap_or(gpui::size(px(1280.0), px(800.0)));
        let outer = (SIDE + 2.0 * SHADOW) as i32;
        let right = (self.margin.0 - f32::from(by.x).round() as i32).clamp(0, (f32::from(screen.width) as i32 - outer).max(0));
        let bottom = (self.margin.1 - f32::from(by.y).round() as i32).clamp(0, (f32::from(screen.height) as i32 - outer).max(0));
        if (right, bottom) != self.margin {
            self.margin = (right, bottom);
            window.set_layer_margin((0, right, bottom, 0));
        }
    }
}

impl Render for Bubble {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (frame, name, dropped) = (self.frame.clone(), self.name.clone(), std::mem::take(&mut self.dropped));
        let p = theme::palette(cx);
        let ring = olo_ui::style::c(p.panel_background);
        let _ = window;
        div()
            .size_full()
            .on_mouse_down(MouseButton::Left, cx.listener(|b, e: &MouseDownEvent, _, _| b.drag = Some(e.position)))
            .on_mouse_move(cx.listener(|b, e: &MouseMoveEvent, window, cx| {
                if let Some(at) = b.drag.filter(|_| e.pressed_button == Some(MouseButton::Left)) {
                    b.moved(e.position - at, window, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|b, _: &MouseUpEvent, _, _| b.drag = None))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        for img in dropped {
                            let _ = window.drop_image(img);
                        }
                        let side = px(SIDE);
                        let at = bounds.origin + point(px(SHADOW), px(SHADOW * 0.6));
                        let circle = gpui::Bounds::new(at, gpui::size(side, side));
                        let round = Corners::all(side / 2.0);
                        window.paint_shadows(
                            circle,
                            round,
                            &[BoxShadow { color: gpui::hsla(0.0, 0.0, 0.0, 0.35), offset: point(px(0.0), px(3.0)), blur_radius: px(SHADOW * 0.8), spread_radius: px(0.0) }],
                        );
                        match frame {
                            Some(img) => {
                                let _ = window.paint_image(circle, round, img, 0, false);
                            }
                            None => window.paint_quad(gpui::fill(circle, gpui::hsla(0.0, 0.0, 0.15, 1.0)).corner_radii(round)),
                        }
                        // A thin ring in the panel's colour, so it shows on anything.
                        window.paint_quad(gpui::outline(circle, ring, gpui::BorderStyle::Solid).border_widths(px(2.0)).corner_radii(round));
                        olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Image", name: Some(format!("Camera: {name}").into()), bounds: circle, enabled: true, ..Default::default() });
                    },
                )
                .size_full(),
            )
    }
}

/// The middle square of `f`, mirrored, as an image for the window.
fn square(f: &olo_camera::Frame) -> Option<image::RgbaImage> {
    let side = f.width.min(f.height) as usize;
    let (x0, y0) = ((f.width as usize - side) / 2, (f.height as usize - side) / 2);
    let row = f.width as usize * 4;
    let mut out = Vec::with_capacity(side * side * 4);
    for y in y0..y0 + side {
        let line = &f.bgra[y * row + x0 * 4..y * row + (x0 + side) * 4];
        for px in line.as_chunks::<4>().0.iter().rev() {
            out.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    image::RgbaImage::from_raw(side as u32, side as u32, out)
}

/// `--camera-bubble [--camera ID]`: runs till it's ended (SIGTERM).
pub fn run(args: &[String]) -> i32 {
    let id = args.iter().position(|a| a == "--camera").and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--") && *v != "auto").cloned();
    let mut camera = match olo_camera::open(id.as_deref(), (640, 480)) {
        Ok(c) => c,
        Err(e) => {
            println!("{e}");
            return 1;
        }
    };
    let name = camera.info.name.clone();
    let latest: Latest = Arc::default();
    olo_ui::run(APP_ID, move |cx| {
        let options =
            WindowOptions::new("Camera", SIDE + 2.0 * SHADOW, SIDE + 2.0 * SHADOW).no_panel().layer("screenshot-camera", LayerShellLayer::Overlay, (false, true, false, true)).layer_options(|l| {
                l.margin = (0, MARGIN, MARGIN, 0);
                l.keyboard = LayerKeyboard::None;
            });
        let bubble = cx.new(|_| Bubble { latest: latest.clone(), name, frame: None, dropped: Vec::new(), margin: (MARGIN, MARGIN), drag: None });
        let view = bubble.clone();
        let handle = olo_ui::open_window(cx, options, move |_, _| view);
        let messages = bubble.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if let (Ok(m), Some(h)) = (m.downcast::<Msg>(), handle) {
                let _ = h.update(cx, |_, window, cx| messages.update(cx, |b, cx| b.message(*m, window, cx)));
            }
        });
        let proxy = olo_ui::proxy(cx);
        std::thread::Builder::new()
            .name("camera".into())
            .spawn(move || {
                loop {
                    match camera.frame() {
                        Ok(f) => {
                            let Some(img) = square(&f) else { continue };
                            let tell = {
                                let mut l = latest.lock().unwrap_or_else(|e| e.into_inner());
                                l.0 = Some(img);
                                !std::mem::replace(&mut l.1, true)
                            };
                            if tell {
                                proxy.send(Msg::Frame);
                            }
                        }
                        Err(e) => {
                            proxy.send(Msg::Failed(format!("the camera stopped: {e}")));
                            return;
                        }
                    }
                }
            })
            .ok();
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_square_mirrored() {
        // 4 × 2: columns 0..3, the middle square is columns 1 and 2.
        let bgra: Vec<u8> = (0..2).flat_map(|_| (0..4u8).flat_map(|x| [x, 0, 0, 255])).collect();
        let s = square(&olo_camera::Frame { width: 4, height: 2, bgra }).unwrap();
        assert_eq!(s.dimensions(), (2, 2));
        // Mirrored: column 2 first.
        assert_eq!(s.get_pixel(0, 0).0, [2, 0, 0, 255]);
        assert_eq!(s.get_pixel(1, 1).0, [1, 0, 0, 255]);
    }
}
