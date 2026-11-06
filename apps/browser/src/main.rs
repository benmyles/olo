//! Native shell over Olo's original engine, with network loading and initial
//! document links. Full web compatibility is an active, uncompleted goal.
mod links;
mod network;
mod worker;
use olo_ui::{
    Menu, MenuBar, WindowOptions, agent,
    gpui::{
        self, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, ScrollDelta, StatefulInteractiveElement as _, Styled as _,
        Window, canvas, div, point, px, size,
    },
    instance::{self, Launch},
    style, theme,
    widgets::{Button, StringView, TextField, text_field},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, mpsc},
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
    final_url: Option<String>,
    cwd: Option<std::path::PathBuf>,
    revision: u64,
    _address_events: gpui::Subscription,
    tx: mpsc::Sender<worker::Message>,
    cancel: network::Cancel,
    id: u64,
    key: Option<worker::Key>,
    tiles: Vec<(olo_web::paint::DeviceRect, Arc<RenderImage>)>,
    labels: Vec<worker::Label>,
    status: String,
    blocked_action: Option<&'static str>,
    scroll: i32,
    extent: f64,
    interaction: olo_web::page::Interaction,
    pressed: Option<olo_web::dom::NodeId>,
    interaction_mask: u64,
    viewport: Rc<std::cell::Cell<Bounds<gpui::Pixels>>>,
}
impl Browser {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved: Saved = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let source = if saved.source.is_empty() { std::env::args().nth(1).unwrap_or_else(|| "about:engine".into()) } else { saved.source };
        let address = text_field(cx, |f| f.name("Address").hint("Website URL or local HTML path").keeps_enter());
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
            final_url: None,
            cwd: None,
            tx: worker::start(olo_ui::proxy(cx)),
            cancel: network::Cancel::default(),
            id: 0,
            key: None,
            tiles: Vec::new(),
            labels: Vec::new(),
            status: "Loading…".into(),
            blocked_action: None,
            scroll: (saved.scroll * f64::from(window.scale_factor())).round() as i32,
            extent: 0.,
            interaction: Default::default(),
            pressed: None,
            interaction_mask: 0,
            viewport: Rc::new(std::cell::Cell::new(Bounds::default())),
        }
    }
    fn request(&mut self, width: u32, height: u32, scale: f64, palette: Arc<olo_theme::Palette>, cx: &mut Context<Self>) {
        if width == 0 || height == 0 {
            return;
        }
        let key = worker::Key { source: self.source.clone(), cwd: self.cwd.clone(), revision: self.revision, width, height, scale, palette };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = &self.key {
            self.scroll = (f64::from(self.scroll) * scale / old.scale).round() as i32;
        }
        self.status = "Loading…".into();
        self.cancel.cancel();
        self.cancel = network::Cancel::default();
        self.id += 1;
        let recycle = std::mem::take(&mut self.tiles).into_iter().map(|(_, image)| image).collect();
        self.labels.clear();
        self.key = Some(key.clone());
        let _ = self.tx.send(worker::Message::Render(Box::new(worker::Request { id: self.id, key, scroll: self.scroll, cancel: self.cancel.clone(), recycle, interaction: self.interaction })));
        cx.notify();
    }
    fn update_input(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else {
            return;
        };
        self.cancel.cancel();
        self.cancel = network::Cancel::default();
        self.id += 1;
        let _ =
            self.tx.send(worker::Message::Render(Box::new(worker::Request { id: self.id, key, scroll: self.scroll, cancel: self.cancel.clone(), recycle: Vec::new(), interaction: self.interaction })));
        cx.notify();
    }
    fn pointer(&mut self, at: gpui::Point<gpui::Pixels>, phase: u8, window: &mut Window, cx: &mut Context<Self>) {
        if self.key.is_none() {
            return;
        }
        let bounds = self.viewport.get();
        let scale = self.key.as_ref().map_or(1., |k| k.scale);
        let x = f64::from(f32::from(at.x - bounds.left()));
        let y = f64::from(f32::from(at.y - bounds.top())) + f64::from(self.scroll) / scale;
        let label = self.labels.iter().rev().find(|l| x >= l.rect.x && x < l.rect.x + l.rect.width && y >= l.rect.y && y < l.rect.y + l.rect.height);
        let node = label.and_then(|l| l.node);
        let link = label.and_then(|l| l.link.clone());
        let old = self.interaction;
        self.interaction.hover = node;
        if phase == 1 {
            window.focus(&self.focus);
            self.pressed = link.as_ref().map(|l| l.node);
            self.interaction.active = node;
            self.interaction.focus = self.pressed;
            self.interaction.keyboard_focus = false;
        }
        if phase == 2 {
            self.interaction.active = None;
            if let Some(link) = link
                && self.pressed.take() == Some(link.node)
            {
                if let Some(reason) = link.blocked {
                    self.status = reason.into();
                    self.blocked_action = Some(reason);
                } else {
                    self.address.update(cx, |f, cx| f.set_text(link.url.clone(), cx));
                    self.command("go", window, cx);
                    return;
                }
            }
            self.pressed = None;
        }
        if old.changes_style(self.interaction, self.interaction_mask) {
            self.update_input(cx);
        } else if phase == 2 {
            cx.notify();
        }
    }
    fn command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "go" => {
                window.focus(&self.focus);
                self.revision = self.revision.wrapping_add(1);
                self.source = self.address.read(cx).text().to_string();
                self.final_url = None;
                if self.source.is_empty() {
                    self.source = "about:engine".into()
                }
                self.interaction = Default::default();
                self.pressed = None;
                self.scroll = 0;
                self.key = None;
            }
            "reload" => {
                if let Some(url) = self.final_url.take() {
                    self.source = url;
                }
                self.key = None;
                self.revision = self.revision.wrapping_add(1);
            }
            "stop" => {
                self.id += 1; // Ignore replies queued just before cancellation.
                self.cancel.cancel();
                self.status = "Stopped".into();
            }
            "home" => {
                self.revision = self.revision.wrapping_add(1);
                self.source = "about:engine".into();
                self.final_url = None;
                self.address.update(cx, |f, cx| f.set_text("about:engine", cx));
                self.interaction = Default::default();
                self.pressed = None;
                self.scroll = 0;
                self.key = None;
            }
            "quit" => cx.quit(),
            _ => return,
        }
        if matches!(cmd, "go" | "home" | "reload") {
            self.cancel.cancel();
            self.id += 1;
            self.labels.clear();
            self.blocked_action = None;
            self.interaction = Default::default();
            self.interaction_mask = 0;
            self.pressed = None;
        }
        cx.notify();
    }
    fn menus(&self) -> Vec<Menu> {
        vec![Menu::new("File").item("Home", "home").item("Reload", "reload").item("Stop", "stop").separator().item("Close", "quit")]
    }
    fn recycle(&self, images: Vec<Arc<RenderImage>>) {
        if !images.is_empty() {
            let _ = self.tx.send(worker::Message::Recycle(images));
        }
    }
    fn merge_tiles(&mut self, tiles: Vec<(olo_web::paint::DeviceRect, Arc<RenderImage>)>) {
        let mut recycle = Vec::new();
        for (r, image) in tiles {
            if let Some(old) = self.tiles.iter_mut().find(|(old, _)| *old == r) {
                let previous = std::mem::replace(&mut old.1, image);
                if !Arc::ptr_eq(&previous, &old.1) {
                    recycle.push(previous);
                }
            } else {
                self.tiles.push((r, image));
            }
        }
        self.recycle(recycle);
    }
    fn reply(&mut self, message: worker::Reply, cx: &mut Context<Self>) {
        match message {
            worker::Reply::Location(id, url) if id == self.id => {
                // Preserve the request cache key through redirects; display and
                // persist the final URL. Reload uses this canonical location.
                self.final_url = Some(url.clone());
                self.address.update(cx, |f, cx| f.set_text(url, cx));
            }
            worker::Reply::Status(id, s) if id == self.id => self.status = s,
            worker::Reply::Error(id, s) if id == 0 || id == self.id => self.status = s,
            worker::Reply::Ready { id, title, tiles } if id == self.id => {
                self.merge_tiles(tiles);
                self.status = self.blocked_action.map_or(title, str::to_owned);
            }
            worker::Reply::Content { id, labels, extent, scroll, interaction_mask } if id == self.id => {
                self.labels = labels;
                self.interaction_mask = interaction_mask;
                self.extent = extent;
                self.scroll = scroll;
            }
            worker::Reply::Batch(id, tiles) if id == self.id => {
                self.merge_tiles(tiles);
            }
            worker::Reply::Ready { tiles, .. } | worker::Reply::Batch(_, tiles) => {
                self.recycle(tiles.into_iter().map(|(_, image)| image).collect());
                return;
            }
            _ => return,
        }
        cx.notify();
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.cancel.cancel()
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
        let focused = self.interaction.focus;
        let labels: Vec<_> = self.labels.iter().map(|l| (l.text.clone(), l.rect, l.link.clone())).collect();
        let viewport = self.viewport.clone();
        let scroll = self.scroll;
        let extent = self.extent;
        let surface = canvas(
            move |bounds, window, cx| {
                let bounds = style::snap(window, bounds);
                viewport.set(bounds);
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
                    for (text, r, link) in &labels {
                        agent::record(
                            window,
                            cx,
                            agent::Node {
                                document_content: true,
                                role: if link.is_some() { "Link" } else { "Label" },
                                name: Some(text.clone().into()),
                                value: link.as_ref().map(|l| l.url.clone().into()),
                                clickable: link.is_some(),
                                focused: link.as_ref().is_some_and(|l| Some(l.node) == focused),
                                enabled: true,
                                bounds: Bounds::new(point(bounds.left() + px(r.x as f32), bounds.top() + px((r.y - f64::from(scroll) / scale) as f32)), size(px(r.width as f32), px(r.height as f32))),
                                ..Default::default()
                            },
                        );
                    }
                });
            },
        )
        .w_full()
        .h_full();
        let scrolling = me.clone();
        let move_link = me.clone();
        let down_link = me.clone();
        let up_link = me.clone();
        let leave_link = me.clone();
        let hover_link = me.clone();
        let surface = div()
            .id("document")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(surface)
            .on_mouse_move(move |e, w, cx| move_link.update(cx, |v, cx| v.pointer(e.position, 0, w, cx)))
            .on_mouse_down(gpui::MouseButton::Left, move |e, w, cx| down_link.update(cx, |v, cx| v.pointer(e.position, 1, w, cx)))
            .on_mouse_up(gpui::MouseButton::Left, move |e, w, cx| up_link.update(cx, |v, cx| v.pointer(e.position, 2, w, cx)))
            .on_mouse_up_out(gpui::MouseButton::Left, move |_, _, cx| {
                leave_link.update(cx, |v, cx| {
                    let old = v.interaction;
                    v.pressed = None;
                    v.interaction.active = None;
                    v.interaction.hover = None;
                    if old.changes_style(v.interaction, v.interaction_mask) {
                        v.update_input(cx);
                    }
                })
            })
            .on_hover(move |over, _, cx| {
                if !over {
                    hover_link.update(cx, |v, cx| {
                        let old = v.interaction;
                        v.interaction.hover = None;
                        if old.changes_style(v.interaction, v.interaction_mask) {
                            v.update_input(cx);
                        }
                    });
                }
            })
            .on_scroll_wheel(move |e, w, cx| {
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
                        v.cancel.cancel();
                        v.cancel = network::Cancel::default();
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
                        let _ = v.tx.send(worker::Message::Render(Box::new(worker::Request { id: v.id, key, scroll: v.scroll, cancel: v.cancel.clone(), recycle, interaction: v.interaction })));
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
                serde_json::to_string(&Saved { source: v.final_url.clone().unwrap_or_else(|| v.source.clone()), scroll: f64::from(v.scroll) / v.key.as_ref().map_or(1., |k| k.scale) })
                    .unwrap_or_default()
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
                if let Some(source) = launch.args.first()
                    && let Some(v) = messages.borrow().as_ref()
                {
                    v.update(cx, |v, cx| {
                        v.address.update(cx, |f, cx| f.set_text(source.clone(), cx));
                        v.source = source.clone();
                        v.final_url = None;
                        v.cwd = launch.cwd.clone();
                        v.scroll = 0;
                        v.revision = v.revision.wrapping_add(1);
                        v.key = None;
                        cx.notify();
                    });
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
