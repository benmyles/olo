//! Native shell over Olo's original engine, with network loading and initial
//! document links. Full web compatibility is an active, uncompleted goal.
mod history;
mod links;
mod navigation;
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
    #[serde(default)]
    history: Option<history::History>,
}
struct Browser {
    address: Entity<TextField>,
    focus: gpui::FocusHandle,
    source: String,
    history: history::History,
    loaded_group: u64,
    document_ready: bool,
    navigation: u64,
    seek_anchor: bool,
    focus_scroll: Option<olo_web::dom::NodeId>,
    focus_targets: Arc<navigation::FocusTargets>,
    document_snapshot: gpui::SharedString,
    snapshot_identity: (u64, u64),
    /// What the engine substituted on the current page, as the worker reported it.
    fallbacks: String,
    cwd: Option<std::path::PathBuf>,
    revision: u64,
    _address_events: gpui::Subscription,
    _focus_events: gpui::Subscription,
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
        let restoring = !saved.source.is_empty();
        let source = if saved.source.is_empty() { std::env::args().nth(1).unwrap_or_else(|| "about:engine".into()) } else { saved.source };
        let restored_history = saved.history.and_then(history::History::restored);
        let has_history = restored_history.is_some();
        let mut history = restored_history.unwrap_or_else(|| history::History::new(source, None));
        if restoring && !has_history {
            history.scroll(saved.scroll);
        }
        let source = history.current().url.clone();
        let cwd = history.current().cwd.clone();
        let loaded_group = history.current().document;
        let scroll = history.current().scroll;
        let address = text_field(cx, |f| f.name("Address").hint("Website URL or local HTML path").keeps_enter());
        address.update(cx, |f, cx| f.set_text(source.clone(), cx));
        let events = cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, olo_ui::widgets::TextFieldEvent::Entered) {
                this.command("go", window, cx)
            }
        });
        let focus = cx.focus_handle().tab_stop(true);
        let focus_events = cx.on_blur(&focus, window, |this: &mut Self, window, cx| {
            if this.focus.is_focused(window) {
                return;
            }
            let old = this.interaction;
            this.interaction.focus = None;
            this.interaction.keyboard_focus = false;
            if old.changes_style(this.interaction, this.interaction_mask) {
                this.update_input(cx);
            } else {
                cx.notify();
            }
        });
        window.focus(&focus);
        Self {
            focus,
            revision: 0,
            _address_events: events,
            _focus_events: focus_events,
            address,
            source,
            history,
            loaded_group,
            document_ready: false,
            navigation: 1,
            seek_anchor: !restoring,
            focus_scroll: None,
            focus_targets: Arc::default(),
            document_snapshot: Default::default(),
            snapshot_identity: (0, 0),
            fallbacks: String::new(),
            cwd,
            tx: worker::start(olo_ui::proxy(cx)),
            cancel: network::Cancel::default(),
            id: 0,
            key: None,
            tiles: Vec::new(),
            labels: Vec::new(),
            status: "Loading…".into(),
            blocked_action: None,
            scroll: (scroll * f64::from(window.scale_factor())).round() as i32,
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
        let recycle = std::mem::take(&mut self.tiles).into_iter().map(|(_, image)| image).collect();
        self.labels.clear();
        self.key = Some(key);
        self.send_render(recycle, cx);
    }
    fn update_input(&mut self, cx: &mut Context<Self>) {
        self.send_render(Vec::new(), cx);
    }
    fn send_render(&mut self, recycle: Vec<Arc<RenderImage>>, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else {
            return;
        };
        self.cancel.cancel();
        self.cancel = network::Cancel::default();
        self.id += 1;
        let fragment = self.history.current().url.split_once('#').map(|(_, f)| gpui::SharedString::from(f.to_owned()));
        let _ = self.tx.send(worker::Message::Render(Box::new(worker::Request {
            id: self.id,
            key,
            scroll: self.scroll,
            cancel: self.cancel.clone(),
            recycle,
            interaction: self.interaction,
            navigation: self.navigation,
            fragment,
            seek_anchor: self.seek_anchor,
            focus_scroll: self.focus_scroll,
        })));
        cx.notify();
    }
    fn css_scroll(&self) -> f64 {
        f64::from(self.scroll) / self.key.as_ref().map_or(1., |k| k.scale)
    }
    fn discard_targets(&mut self) {
        let old = std::mem::take(&mut self.focus_targets);
        if !old.is_empty() {
            let _ = self.tx.send(worker::Message::RecycleTargets(old));
        }
    }
    fn begin_navigation(&mut self, entry: history::Entry, same: bool, seek: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.cancel.cancel();
        self.id += 1;
        self.navigation += 1;
        self.seek_anchor = seek;
        self.focus_scroll = None;
        self.interaction = Default::default();
        self.pressed = None;
        self.blocked_action = None;
        self.address.update(cx, |f, cx| f.set_text(entry.url.clone(), cx));
        if same {
            self.status = "Finding fragment…".into();
            self.update_input(cx);
        } else {
            self.source = entry.url;
            self.cwd = entry.cwd;
            self.loaded_group = entry.document;
            self.document_ready = false;
            self.revision = self.revision.wrapping_add(1);
            self.scroll = (entry.scroll * f64::from(window.scale_factor())).round() as i32;
            self.key = None;
            self.labels.clear();
            self.interaction_mask = 0;
            self.document_snapshot = Default::default();
            self.discard_targets();
            cx.notify();
        }
    }
    fn navigate(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.history.scroll(self.css_scroll());
        let same = self.document_ready && url.contains('#') && url.split('#').next() == self.history.current().url.split('#').next();
        if url != self.history.current().url {
            self.history.push(url, self.cwd.clone(), same);
        }
        let entry = self.history.current().clone();
        self.begin_navigation(entry, same, true, window, cx);
    }
    fn traverse(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.history.scroll(self.css_scroll());
        let Some(entry) = self.history.go(delta) else {
            return;
        };
        let same = self.document_ready && entry.document == self.loaded_group;
        if same {
            self.scroll = (entry.scroll * f64::from(window.scale_factor())).round() as i32;
        }
        self.begin_navigation(entry, same, false, window, cx);
    }
    fn tab_link(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let at = self.interaction.focus.and_then(|node| self.focus_targets.position(node));
        let next = match (at, backward) {
            (Some(i), true) => i.checked_sub(1),
            (Some(i), false) => (i + 1 < self.focus_targets.len()).then_some(i + 1),
            (None, true) => self.focus_targets.len().checked_sub(1),
            (None, false) => (!self.focus_targets.is_empty()).then_some(0),
        };
        let Some(i) = next else {
            self.interaction.focus = None;
            self.interaction.keyboard_focus = false;
            self.address.update(cx, |f, cx| {
                f.focus(window);
                f.select_all_text(cx);
            });
            self.update_input(cx);
            return;
        };
        let target = &self.focus_targets[i];
        self.interaction.focus = Some(target.link.node);
        self.interaction.keyboard_focus = true;
        self.focus_scroll = Some(target.link.node);
        self.update_input(cx);
    }
    fn activate(&mut self, link: Arc<links::Link>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = link.blocked {
            self.status = reason.into();
            self.blocked_action = Some(reason);
            cx.notify();
        } else {
            self.navigate(link.url.to_string(), window, cx);
        }
    }
    /// Input for the page's scripts: the pointer's document position, keys.
    fn send_input(&mut self, kind: worker::InputKind, at: Option<gpui::Point<gpui::Pixels>>, modifiers: gpui::Modifiers) {
        let Some(key) = &self.key else { return };
        let bounds = self.viewport.get();
        let scale = key.scale;
        let (x, y) = at.map_or((0., 0.), |at| (f64::from(f32::from(at.x - bounds.left())) / scale, f64::from(f32::from(at.y - bounds.top())) / scale + f64::from(self.scroll) / scale));
        let _ = self.tx.send(worker::Message::Input(worker::Input {
            kind,
            x,
            y,
            scroll_y: f64::from(self.scroll) / scale,
            buttons: u32::from(self.pressed.is_some() || self.interaction.active.is_some()),
            modifiers: u8::from(modifiers.shift) | u8::from(modifiers.control) << 1 | u8::from(modifiers.alt) << 2 | u8::from(modifiers.platform) << 3,
        }));
    }
    fn pointer(&mut self, at: gpui::Point<gpui::Pixels>, phase: u8, window: &mut Window, cx: &mut Context<Self>) {
        self.send_input(
            match phase {
                1 => worker::InputKind::Down,
                2 => worker::InputKind::Up,
                _ => worker::InputKind::Move,
            },
            Some(at),
            window.modifiers(),
        );
        if self.key.is_none() || phase == 0 && self.interaction_mask & (1u64 << olo_web::css::selectors::State::Hover as u8) == 0 {
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
                self.activate(link, window, cx);
                return;
            }
            self.pressed = None;
        }
        if old.changes_style(self.interaction, self.interaction_mask) {
            self.update_input(cx);
        } else if phase != 0 {
            cx.notify();
        }
    }
    fn command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "go" => {
                let url = self.address.read(cx).text().to_owned();
                self.navigate(if url.trim().is_empty() { "about:engine".into() } else { url }, window, cx);
            }
            "home" => self.navigate("about:engine".into(), window, cx),
            "back" => self.traverse(-1, window, cx),
            "forward" => self.traverse(1, window, cx),
            "reload" => {
                self.history.scroll(self.css_scroll());
                self.begin_navigation(self.history.current().clone(), false, false, window, cx);
            }
            "stop" => {
                self.cancel.cancel();
                self.id += 1;
                self.status = "Stopped".into();
                cx.notify();
            }
            "next-link" => self.tab_link(false, window, cx),
            "previous-link" => self.tab_link(true, window, cx),
            "open-link" => {
                if let Some(target) = self.interaction.focus.and_then(|node| self.focus_targets.position(node)).and_then(|i| self.focus_targets.get(i)) {
                    self.activate(target.link.clone(), window, cx);
                }
            }
            "quit" => cx.quit(),
            _ => {}
        }
    }
    fn menus(&self) -> Vec<Menu> {
        vec![
            Menu::new("File").item("Home", "home").item("Reload", "reload").item("Stop", "stop").separator().item("Close", "quit"),
            Menu::new("Navigate")
                .item("Back", "back")
                .enabled(self.history.can_go(-1))
                .item("Forward", "forward")
                .enabled(self.history.can_go(1))
                .separator()
                .item("Next Link", "next-link")
                .enabled(!self.focus_targets.is_empty())
                .item("Previous Link", "previous-link")
                .enabled(!self.focus_targets.is_empty())
                .item("Open Link", "open-link")
                .enabled(self.interaction.focus.and_then(|node| self.focus_targets.position(node)).is_some()),
        ]
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
                self.history.location(url.clone());
                self.address.update(cx, |f, cx| f.set_text(url, cx));
            }
            worker::Reply::Status(id, s) if id == self.id => self.status = s,
            worker::Reply::Error(id, s) if id == 0 || id == self.id => self.status = s,
            worker::Reply::Ready { id, title, fallbacks, tiles } if id == self.id => {
                self.document_ready = true;
                self.merge_tiles(tiles);
                self.status = self.blocked_action.map_or(title, str::to_owned);
                self.fallbacks = fallbacks;
                let (document, navigation) = self.snapshot_identity;
                self.document_snapshot = serde_json::json!({"url":self.history.current().url,"navigation":navigation,"document":document,"fallbacks":self.fallbacks}).to_string().into();
            }
            worker::Reply::Content { id, labels, extent, scroll, interaction_mask, document, target, focus_targets } if id == self.id => {
                self.labels = labels;
                self.interaction_mask = interaction_mask;
                self.extent = extent;
                self.scroll = scroll;
                self.interaction.target = target;
                self.seek_anchor = false;
                self.focus_scroll = None;
                let old = std::mem::replace(&mut self.focus_targets, focus_targets);
                if !Arc::ptr_eq(&old, &self.focus_targets) && !old.is_empty() {
                    let _ = self.tx.send(worker::Message::RecycleTargets(old));
                }
                if self.snapshot_identity != (document, self.navigation) {
                    self.snapshot_identity = (document, self.navigation);
                    self.document_snapshot = serde_json::json!({"url":self.history.current().url,"navigation":self.navigation,"document":document,"fallbacks":self.fallbacks}).to_string().into();
                }
            }
            worker::Reply::Batch(id, tiles) if id == self.id => {
                self.merge_tiles(tiles);
            }
            worker::Reply::Content { focus_targets, .. } => {
                if !focus_targets.is_empty() {
                    let _ = self.tx.send(worker::Message::RecycleTargets(focus_targets));
                }
                return;
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
        let back = me.clone();
        let forward = me.clone();
        let go = me.clone();
        let stop = me.clone();
        let controls = div()
            .flex()
            .gap(px(olo_ui::metrics::SPACING))
            .p(px(olo_ui::metrics::SPACING))
            .child(Button::new("Back").enabled(self.history.can_go(-1)).on_click(move |w, cx| back.update(cx, |v, cx| v.command("back", w, cx))))
            .child(Button::new("Forward").enabled(self.history.can_go(1)).on_click(move |w, cx| forward.update(cx, |v, cx| v.command("forward", w, cx))))
            .child(div().flex_1().child(self.address.clone()))
            .child(Button::new("Go").on_click(move |w, cx| go.update(cx, |v, cx| v.command("go", w, cx))))
            .child(Button::new("Stop").on_click(move |w, cx| stop.update(cx, |v, cx| v.command("stop", w, cx))));
        let p = theme::palette(cx);
        let request = me.downgrade();
        let key = self.key.clone();
        let palette = p.clone();
        let tiles = self.tiles.clone();
        let focused = self.interaction.focus;
        let document_snapshot = self.document_snapshot.clone();
        let labels: Vec<_> = self.labels.iter().map(|l| (l.text.clone(), l.rect, l.link.clone(), l.image)).collect();
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
                agent::record(window, cx, agent::Node { role: "Document", name: Some("Page".into()), value: Some(document_snapshot.clone()), bounds, enabled: true, ..Default::default() });
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (r, image) in &tiles {
                        let b = Bounds::new(
                            point(bounds.left() + px((f64::from(r.x) / scale) as f32), bounds.top() + px((f64::from(r.y - scroll) / scale) as f32)),
                            size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                        );
                        let _ = window.paint_image_nearest(b, image.clone());
                    }
                    for (text, r, link, image) in &labels {
                        agent::record(
                            window,
                            cx,
                            agent::Node {
                                document_content: true,
                                role: if link.is_some() {
                                    "Link"
                                } else if *image {
                                    "Image"
                                } else {
                                    "Label"
                                },
                                name: Some(text.clone()),
                                value: link.as_ref().map(|l| l.url.clone()),
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
                    v.send_input(worker::InputKind::Wheel { dx: 0., dy: -f64::from(delta) }, Some(e.position), e.modifiers);
                    if v.scroll == old {
                        return;
                    }
                    v.send_input(worker::InputKind::Scroll, None, e.modifiers);
                    v.seek_anchor = false;
                    v.focus_scroll = None;
                    if let Some(key) = v.key.clone() {
                        let mut recycle = Vec::new();
                        let height = key.height as i32;
                        v.tiles.retain(|(r, image)| {
                            let keep = r.y + r.height as i32 > v.scroll && r.y < v.scroll + height;
                            if !keep {
                                recycle.push(image.clone());
                            }
                            keep
                        });
                        v.send_render(recycle, cx);
                    }
                    cx.notify();
                })
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_action(cx.listener(|v, _: &olo_ui::FocusNext, window, cx| {
                if v.focus.is_focused(window) {
                    v.tab_link(false, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|v, _: &olo_ui::FocusPrev, window, cx| {
                if v.focus.is_focused(window) {
                    v.tab_link(true, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|v, e: &gpui::KeyDownEvent, window, cx| {
                if v.focus.is_focused(window) {
                    let key = match e.keystroke.key.as_str() {
                        "enter" => "Enter".to_owned(),
                        "escape" => "Escape".to_owned(),
                        "tab" => "Tab".to_owned(),
                        "backspace" => "Backspace".to_owned(),
                        "delete" => "Delete".to_owned(),
                        "space" => " ".to_owned(),
                        "left" => "ArrowLeft".to_owned(),
                        "right" => "ArrowRight".to_owned(),
                        "up" => "ArrowUp".to_owned(),
                        "down" => "ArrowDown".to_owned(),
                        "home" => "Home".to_owned(),
                        "end" => "End".to_owned(),
                        "pageup" => "PageUp".to_owned(),
                        "pagedown" => "PageDown".to_owned(),
                        k => e.keystroke.key_char.clone().unwrap_or_else(|| k.to_owned()),
                    };
                    let code = match key.as_str() {
                        " " => "Space".to_owned(),
                        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphabetic()) => format!("Key{}", k.to_ascii_uppercase()),
                        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()) => format!("Digit{k}"),
                        k => k.to_owned(),
                    };
                    v.send_input(worker::InputKind::Key { down: true, key, code, text: e.keystroke.key_char.clone() }, None, e.keystroke.modifiers);
                }
                if e.keystroke.modifiers.alt && e.keystroke.key == "left" {
                    v.command("back", window, cx);
                    cx.stop_propagation();
                } else if e.keystroke.modifiers.alt && e.keystroke.key == "right" {
                    v.command("forward", window, cx);
                    cx.stop_propagation();
                } else if v.focus.is_focused(window) && e.keystroke.key == "enter" {
                    v.command("open-link", window, cx);
                    cx.stop_propagation();
                } else if e.keystroke.modifiers.control && e.keystroke.key == "l" {
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
            .child(div().p(px(olo_ui::metrics::WINDOW_INSET)).child(StringView::new(self.status.clone()).fit(true).height(olo_ui::metrics::LABEL_ROW)))
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
                let mut history = v.history.clone();
                history.scroll(v.css_scroll());
                serde_json::to_string(&Saved { source: history.current().url.clone(), scroll: v.css_scroll(), history: Some(history) }).unwrap_or_default()
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
                    && let Some(handle) = handle
                {
                    let _ = handle.update(cx, |_, w, cx| {
                        v.update(cx, |v, cx| {
                            v.history.scroll(v.css_scroll());
                            v.history.push(source.clone(), launch.cwd.clone(), false);
                            v.begin_navigation(v.history.current().clone(), false, true, w, cx);
                        })
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
                if !v.read(cx).menus().iter().any(|m| m.find_action(cmd).is_some_and(|a| a.enabled)) {
                    return false;
                }
                h.update(cx, |_, w, cx| v.update(cx, |v, cx| v.command(cmd, w, cx))).is_ok()
            },
        );
    });
}
