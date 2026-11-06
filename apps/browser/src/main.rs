//! Development browser shell over Olo's original engine. Static local pages
//! first; full network/JS/web compatibility is an active, uncompleted goal.
mod worker;
use olo_ui::{
    Menu, MenuBar, WindowOptions, agent,
    gpui::{
        self, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, ScrollDelta, Styled as _, Window, canvas, div, point, px, size,
    },
    instance::{self, Launch},
    style, theme,
    widgets::{Button, StringView, TextField, text_field},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
const APP_ID: &str = "olo.browser";
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    source: String,
    scroll: f64,
}
struct Browser {
    address: Entity<TextField>,
    focus: gpui::FocusHandle,
    source: String,
    revision: u64,
    _address_events: gpui::Subscription,
    tx: mpsc::Sender<worker::Request>,
    cancel: Arc<AtomicBool>,
    id: u64,
    key: Option<worker::Key>,
    tiles: Vec<(olo_web::paint::DeviceRect, Arc<RenderImage>)>,
    labels: Vec<worker::Label>,
    status: String,
    scroll: i32,
    extent: f64,
}
impl Browser {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved: Saved = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let source = if saved.source.is_empty() { std::env::args().nth(1).unwrap_or_else(|| "about:engine".into()) } else { saved.source };
        let address = text_field(cx, |f| f.name("Address").hint("Local HTML path or about:engine").keeps_enter());
        address.update(cx, |f, cx| f.set_text(source.clone(), cx));
        let events = cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, olo_ui::widgets::TextFieldEvent::Entered) {
                this.command("go", window, cx)
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            focus,
            revision: 0,
            _address_events: events,
            address,
            source,
            tx: worker::start(olo_ui::proxy(cx)),
            cancel: Arc::new(AtomicBool::new(false)),
            id: 0,
            key: None,
            tiles: Vec::new(),
            labels: Vec::new(),
            status: "Loading…".into(),
            scroll: (saved.scroll * f64::from(window.scale_factor())).round() as i32,
            extent: 0.,
        }
    }
    fn request(&mut self, width: u32, height: u32, scale: f64, palette: Arc<olo_theme::Palette>, cx: &mut Context<Self>) {
        if width == 0 || height == 0 {
            return;
        }
        let key = worker::Key { source: self.source.clone(), revision: self.revision, width, height, scale, palette };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = &self.key {
            self.scroll = (f64::from(self.scroll) * scale / old.scale).round() as i32;
        }
        self.status = "Loading…".into();
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.id += 1;
        let recycle = std::mem::take(&mut self.tiles).into_iter().map(|(_, image)| image).collect();
        self.labels.clear();
        self.key = Some(key.clone());
        let _ = self.tx.send(worker::Request { id: self.id, key, scroll: self.scroll, cancel: self.cancel.clone(), recycle });
        cx.notify();
    }
    fn command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "go" => {
                window.focus(&self.focus);
                self.revision = self.revision.wrapping_add(1);
                self.source = self.address.read(cx).text().to_string();
                if self.source.is_empty() {
                    self.source = "about:engine".into()
                }
                self.scroll = 0;
                self.key = None;
            }
            "reload" => {
                self.key = None;
                self.revision = self.revision.wrapping_add(1);
            }
            "stop" => {
                self.id += 1; // Ignore replies queued just before cancellation.
                self.cancel.store(true, Ordering::Relaxed);
                self.status = "Stopped".into();
            }
            "home" => {
                self.revision = self.revision.wrapping_add(1);
                self.source = "about:engine".into();
                self.address.update(cx, |f, cx| f.set_text("about:engine", cx));
                self.scroll = 0;
                self.key = None;
            }
            "quit" => cx.quit(),
            _ => return,
        }
        cx.notify();
    }
    fn menus(&self) -> Vec<Menu> {
        vec![Menu::new("File").item("Home", "home").item("Reload", "reload").item("Stop", "stop").separator().item("Close", "quit")]
    }
    fn reply(&mut self, message: worker::Reply, cx: &mut Context<Self>) {
        match message {
            worker::Reply::Status(id, s) if id == self.id => self.status = s,
            worker::Reply::Error(id, s) if id == 0 || id == self.id => self.status = s,
            worker::Reply::Ready { id, title, tiles } if id == self.id => {
                for (r, image) in tiles {
                    if let Some(old) = self.tiles.iter_mut().find(|(old, _)| *old == r) { old.1 = image } else { self.tiles.push((r, image)) }
                }
                self.status = title;
            }
            worker::Reply::Content { id, labels, extent, scroll } if id == self.id => {
                self.labels = labels;
                self.extent = extent;
                self.scroll = scroll;
            }
            worker::Reply::Batch(id, tiles) if id == self.id => {
                for (r, image) in tiles {
                    if let Some(old) = self.tiles.iter_mut().find(|(old, _)| *old == r) { old.1 = image } else { self.tiles.push((r, image)) }
                }
            }
            _ => return,
        }
        cx.notify();
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed)
    }
}
impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let bar_me = me.clone();
        let bar = MenuBar::new(self.menus(), move |cmd, w, cx| bar_me.update(cx, |v, cx| v.command(cmd, w, cx)));
        let go = me.clone();
        let stop = me.clone();
        let controls = div()
            .flex()
            .gap(px(olo_ui::metrics::SPACING))
            .p(px(olo_ui::metrics::SPACING))
            .child(div().flex_1().child(self.address.clone()))
            .child(Button::new("Go").on_click(move |w, cx| go.update(cx, |v, cx| v.command("go", w, cx))))
            .child(Button::new("Stop").on_click(move |w, cx| stop.update(cx, |v, cx| v.command("stop", w, cx))));
        let p = theme::palette(cx);
        let request = me.downgrade();
        let key = self.key.clone();
        let palette = p.clone();
        let tiles = self.tiles.clone();
        let labels: Vec<_> = self.labels.iter().map(|l| (l.text.clone(), l.rect)).collect();
        let scroll = self.scroll;
        let extent = self.extent;
        let surface = canvas(
            move |bounds, window, cx| {
                let bounds = style::snap(window, bounds);
                let scale = f64::from(window.scale_factor());
                let width = (f64::from(f32::from(bounds.size.width)) * scale).round() as u32;
                let height = (f64::from(f32::from(bounds.size.height)) * scale).round() as u32;
                if key.as_ref().is_none_or(|k| k.width != width || k.height != height || k.scale != scale || k.palette != palette) {
                    let request = request.clone();
                    let palette = palette.clone();
                    cx.defer(move |cx| {
                        let _ = request.update(cx, |v, cx| v.request(width, height, scale, palette, cx));
                    });
                }
            },
            move |bounds, _, window, cx| {
                let bounds = style::snap(window, bounds);
                let scale = f64::from(window.scale_factor());
                window.note_scroll([0., (f64::from(scroll) / scale) as f32, 0., (extent - f64::from(f32::from(bounds.size.height))).max(0.) as f32]);
                window.paint_quad(gpui::fill(bounds, style::c(p.document_background)));
                agent::record(window, cx, agent::Node { role: "Document", name: Some("Page".into()), bounds, enabled: true, ..Default::default() });
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (r, image) in &tiles {
                        let b = Bounds::new(
                            point(bounds.left() + px((f64::from(r.x) / scale) as f32), bounds.top() + px((f64::from(r.y - scroll) / scale) as f32)),
                            size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                        );
                        let _ = window.paint_image_nearest(b, image.clone());
                    }
                    for (text, r) in &labels {
                        agent::record(
                            window,
                            cx,
                            agent::Node::label(
                                text.clone(),
                                Bounds::new(point(bounds.left() + px(r.x as f32), bounds.top() + px((r.y - f64::from(scroll) / scale) as f32)), size(px(r.width as f32), px(r.height as f32))),
                            ),
                        );
                    }
                });
            },
        )
        .w_full()
        .h_full();
        let scrolling = me.clone();
        let surface = div().flex_1().min_h_0().overflow_hidden().child(surface).on_scroll_wheel(move |e, w, cx| {
            scrolling.update(cx, |v, cx| {
                let delta = match e.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y),
                    ScrollDelta::Lines(p) => p.y * 40.,
                };
                let scale = f64::from(w.scale_factor());
                let max = v.key.as_ref().map_or(0, |key| worker::max_scroll(key, v.extent));
                let old = v.scroll;
                v.scroll = (v.scroll - (f64::from(delta) * scale).round() as i32).clamp(0, max);
                if v.scroll == old {
                    return;
                }
                if let Some(key) = v.key.clone() {
                    v.cancel.store(true, Ordering::Relaxed);
                    v.cancel = Arc::new(AtomicBool::new(false));
                    v.id += 1;
                    let mut recycle = Vec::new();
                    let height = key.height as i32;
                    v.tiles.retain(|(r, image)| {
                        let keep = r.y + r.height as i32 > v.scroll && r.y < v.scroll + height;
                        if !keep {
                            recycle.push(image.clone())
                        }
                        keep
                    });
                    let _ = v.tx.send(worker::Request { id: v.id, key, scroll: v.scroll, cancel: v.cancel.clone(), recycle });
                }
                cx.notify();
            })
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e: &gpui::KeyDownEvent, window, cx| {
                if e.keystroke.modifiers.control && e.keystroke.key == "l" {
                    v.address.update(cx, |f, cx| {
                        f.focus(window);
                        f.select_all_text(cx);
                    });
                    cx.stop_propagation();
                } else if e.keystroke.modifiers.control && e.keystroke.key == "r" {
                    v.command("reload", window, cx);
                    cx.stop_propagation();
                } else if e.keystroke.key == "escape" {
                    v.command("stop", window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(bar)
            .child(controls)
            .child(surface)
            .child(div().p(px(olo_ui::metrics::WINDOW_INSET)).child(StringView::new(self.status.clone()).wrap(true)))
    }
}
fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, |cx| {
        let view: Rc<RefCell<Option<Entity<Browser>>>> = Rc::default();
        let made = view.clone();
        let handle = olo_ui::open_window(cx, WindowOptions::new("Browser", 1000., 700.).min_size(640., 480.), move |window, cx| {
            let v = cx.new(|cx| Browser::new(window, cx));
            *made.borrow_mut() = Some(v.clone());
            v
        });
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let saved = view.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            saved.borrow().as_ref().map(|v| {
                let v = v.read(cx);
                serde_json::to_string(&Saved { source: v.source.clone(), scroll: f64::from(v.scroll) / v.key.as_ref().map_or(1., |k| k.scale) }).unwrap_or_default()
            })
        });
        let messages = view.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            if message.is::<worker::Reply>() {
                if let Some(v) = messages.borrow().as_ref() {
                    v.update(cx, |v, cx| v.reply(*message.downcast::<worker::Reply>().unwrap(), cx));
                }
            } else if message.is::<Launch>() {
                let launch = *message.downcast::<Launch>().unwrap();
                if let Some(source) = launch.args.first() {
                    let source = if !source.contains("://") && source != "about:engine" && std::path::Path::new(source).is_relative() {
                        launch.cwd.as_ref().map_or_else(|| source.clone(), |cwd| cwd.join(source).to_string_lossy().into_owned())
                    } else {
                        source.clone()
                    };
                    if let Some(v) = messages.borrow().as_ref() {
                        v.update(cx, |v, cx| {
                            v.address.update(cx, |f, cx| f.set_text(source.clone(), cx));
                            v.source = source;
                            v.scroll = 0;
                            v.revision = v.revision.wrapping_add(1);
                            v.key = None;
                            cx.notify();
                        });
                    }
                }
                olo_ui::raise(handle, cx);
            }
        });
        let menu_view = view.clone();
        agent::set_menus(
            cx,
            move |cx| {
                menu_view
                    .borrow()
                    .as_ref()
                    .map(|v| olo_ui::commands::Menus { app_id: APP_ID.into(), entries: v.read(cx).menus().iter().flat_map(|m| m.entries(APP_ID).entries).collect() })
                    .unwrap_or_default()
            },
            move |cmd, cx| {
                let Some(v) = view.borrow().clone() else { return false };
                let Some(h) = handle else { return false };
                h.update(cx, |_, w, cx| v.update(cx, |v, cx| v.command(cmd, w, cx))).is_ok()
            },
        );
    });
}
