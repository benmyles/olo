//! Native shell over Olo's original engine, with network loading and initial
//! document links. Full web compatibility is an active, uncompleted goal.
mod cookies;
mod frames;
mod history;
mod links;
mod media;
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
    /// The page's layers above its base tiles, in paint order.
    layers: Vec<worker::LayerTiles>,
    /// Images no longer shown: taken out of the window's GPU atlas at the
    /// next paint (it keeps every image it was given until told; never
    /// told, a live page's 60 frames a second filled the GPU's memory),
    /// then sent to the worker to be dropped there.
    retired: Vec<Arc<RenderImage>>,
    /// Canvases asking for shared textures (made at the next paint, which
    /// has the renderer), the page's shared textures, and those to free.
    shared_requests: Vec<(olo_web::dom::NodeId, u32, u32, [u8; 16])>,
    /// The page wants frames: each frame the window draws asks for its next.
    animating: bool,
    shared_keys: Vec<u64>,
    shared_retired: Vec<u64>,
    labels: Vec<worker::Label>,
    /// The element with the page's focus (agents see it focused).
    page_focus: Option<olo_web::dom::NodeId>,
    /// The page's scripts run (see `worker::Reply::Content::live`).
    live: bool,
    /// The page's scroll containers, as the last render had them.
    scroll_areas: Arc<[worker::ScrollArea]>,
    status: String,
    blocked_action: Option<&'static str>,
    scroll: i32,
    extent: f64,
    interaction: olo_web::page::Interaction,
    pressed: Option<olo_web::dom::NodeId>,
    interaction_mask: u64,
    viewport: Rc<std::cell::Cell<Bounds<gpui::Pixels>>>,
    /// While the wheel scrolls, what passes under the pointer is not
    /// hovered (as in Chrome): restyling for each element passing under it
    /// kept cancelling itself (BBC's page burned a core, rendering
    /// nothing). When the scroll last moved, where the pointer is, and the
    /// hover made once the scroll has stopped.
    scrolled_at: Option<std::time::Instant>,
    pointer_at: Option<gpui::Point<gpui::Pixels>>,
    hover_after_scroll: Option<gpui::Task<()>>,
    /// What the last render request asked for besides its scroll: one
    /// asking the same at another scroll leaves the one in flight to
    /// finish (a restyle for a hover, then tiles) instead of cancelling it
    /// for each wheel step to begin it again (it never finished while the
    /// page scrolled).
    last_asked: Option<Asked>,
}
type Asked = (worker::Key, olo_web::page::Interaction, u64, Option<gpui::SharedString>, bool, Option<olo_web::dom::NodeId>);
/// How long after the last wheel step the page is still scrolling (no hover).
const SCROLL_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);
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
            layers: Vec::new(),
            retired: Vec::new(),
            shared_requests: Vec::new(),
            animating: false,
            shared_keys: Vec::new(),
            shared_retired: Vec::new(),
            labels: Vec::new(),
            scroll_areas: Arc::from([]),
            page_focus: None,
            live: false,
            status: "Loading…".into(),
            blocked_action: None,
            scroll: (scroll * f64::from(window.scale_factor())).round() as i32,
            extent: 0.,
            interaction: Default::default(),
            pressed: None,
            scrolled_at: None,
            pointer_at: None,
            hover_after_scroll: None,
            last_asked: None,
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
            // Another document: its page's shared textures go with it.
            if old.source != key.source || old.revision != key.revision {
                self.shared_retired.append(&mut self.shared_keys);
                self.shared_requests.clear();
            }
        }
        self.status = "Loading…".into();
        let shown = std::mem::take(&mut self.tiles).into_iter().map(|(_, image)| image).collect();
        self.recycle(shown);
        self.labels.clear();
        self.key = Some(key);
        self.send_render(Vec::new(), cx);
    }
    fn update_input(&mut self, cx: &mut Context<Self>) {
        self.send_render(Vec::new(), cx);
    }
    fn send_render(&mut self, recycle: Vec<Arc<RenderImage>>, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else {
            return;
        };
        let fragment = self.history.current().url.split_once('#').map(|(_, f)| gpui::SharedString::from(f.to_owned()));
        let asked: Asked = (key.clone(), self.interaction, self.navigation, fragment.clone(), self.seek_anchor, self.focus_scroll);
        if self.last_asked.as_ref() != Some(&asked) || self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            self.cancel.cancel();
            self.cancel = network::Cancel::default();
        }
        self.last_asked = Some(asked);
        self.id += 1;
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
    /// The element (and link) under window point `at`, as the last
    /// render's labels have it.
    fn under(&self, at: gpui::Point<gpui::Pixels>) -> (Option<olo_web::dom::NodeId>, Option<Arc<links::Link>>) {
        let bounds = self.viewport.get();
        let scale = self.key.as_ref().map_or(1., |k| k.scale);
        let x = f64::from(f32::from(at.x - bounds.left()));
        let viewport_y = f64::from(f32::from(at.y - bounds.top()));
        let y = viewport_y + f64::from(self.scroll) / scale;
        let height_css = f64::from(f32::from(bounds.size.height));
        let shift = |st: &olo_web::layout::Sticky| st.offset(f64::from(self.scroll) / scale, height_css);
        // Fixed and sticky labels paint over the page: tried first, where
        // they are shown.
        let label = self.labels.iter().rev().filter(|l| l.fixed || l.sticky.is_some()).chain(self.labels.iter().rev().filter(|l| !l.fixed && l.sticky.is_none())).find(|l| {
            let ly = if l.fixed { viewport_y } else { y - l.sticky.as_ref().map_or(0., shift) };
            x >= l.rect.x && x < l.rect.x + l.rect.width && ly >= l.rect.y && ly < l.rect.y + l.rect.height
        });
        (label.and_then(|l| l.node), label.and_then(|l| l.link.clone()))
    }
    /// The wheel moved the page: no hover until it stops, then the hover
    /// of what is under the pointer by then.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        self.scrolled_at = Some(std::time::Instant::now());
        self.hover_after_scroll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCROLL_SETTLE).await;
            let _ = this.update(cx, |v, cx| {
                v.scrolled_at = None;
                v.hover_after_scroll = None;
                let Some(at) = v.pointer_at else { return };
                if v.key.is_none() || v.interaction_mask & (1u64 << olo_web::css::selectors::State::Hover as u8) == 0 {
                    return;
                }
                let old = v.interaction;
                v.interaction.hover = v.under(at).0;
                if old.changes_style(v.interaction, v.interaction_mask) {
                    v.update_input(cx);
                }
            });
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
        self.pointer_at = Some(at);
        if self.key.is_none() || phase == 0 && self.interaction_mask & (1u64 << olo_web::css::selectors::State::Hover as u8) == 0 {
            return;
        }
        // Scrolling: hovered once it stops (`scrolled`).
        if phase == 0 && self.scrolled_at.is_some_and(|t| t.elapsed() < SCROLL_SETTLE) {
            return;
        }
        let (node, link) = self.under(at);
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
            // A page whose scripts run hears the click first and follows
            // the link itself unless they cancel it (apple.com's search
            // button opens its panel in place of going to its href).
            if let Some(link) = link
                && self.pressed.take() == Some(link.node)
                && !self.live
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
    /// Images the window stops showing (see `retired`).
    fn recycle(&mut self, images: Vec<Arc<RenderImage>>) {
        self.retired.extend(images);
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
            // The page has something to show: a frame, which asks it for its next.
            worker::Reply::WantFrame(id) if id == self.id => self.animating = true,
            worker::Reply::Error(id, s) if id == 0 || id == self.id => self.status = s,
            worker::Reply::Ready { id, title, fallbacks, tiles, animating } if id == self.id => {
                self.animating = animating;
                self.document_ready = true;
                self.merge_tiles(tiles);
                self.status = self.blocked_action.map_or(title, str::to_owned);
                self.fallbacks = fallbacks;
                let (document, navigation) = self.snapshot_identity;
                self.document_snapshot = serde_json::json!({"url":self.history.current().url,"navigation":navigation,"document":document,"fallbacks":self.fallbacks}).to_string().into();
            }
            worker::Reply::Content { id, labels, extent, scroll, interaction_mask, document, target, focus_targets, scroll_areas, page_focus, live } if id == self.id => {
                self.labels = labels;
                self.page_focus = page_focus;
                self.live = live;
                self.scroll_areas = scroll_areas;
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
                // A render's content, layers and tiles come before its Ready:
                // one frame for them all, drawn at the Ready.
                return;
            }
            worker::Reply::Batch(id, tiles) if id == self.id => {
                self.merge_tiles(tiles);
            }
            worker::Reply::Layers(id, layers) if id == self.id => {
                // Most of a layer's tiles come back unchanged: only those no
                // longer in any layer are retired.
                let kept: std::collections::HashSet<*const RenderImage> = layers.iter().flat_map(|l| l.tiles.iter().map(|(_, i)| Arc::as_ptr(i))).collect();
                let old = std::mem::replace(&mut self.layers, layers);
                self.recycle(old.into_iter().flat_map(|l| l.tiles.into_iter().map(|(_, image)| image)).filter(|i| !kept.contains(&Arc::as_ptr(i))).collect());
                return;
            }
            worker::Reply::SharedWanted(id, wanted) if id == self.id => {
                self.shared_requests.extend(wanted);
            }
            worker::Reply::DropShared(keys) => {
                self.shared_keys.retain(|k| !keys.contains(k));
                self.shared_retired.extend(keys);
            }
            worker::Reply::Layers(_, layers) => {
                self.recycle(layers.into_iter().flat_map(|l| l.tiles.into_iter().map(|(_, image)| image)).collect());
                return;
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
        let page_focus = self.page_focus;
        let document_snapshot = self.document_snapshot.clone();
        let labels: Vec<_> = self.labels.iter().map(|l| (l.text.clone(), l.rect, l.link.clone(), l.image, l.fixed, l.sticky, l.control.clone(), l.node)).collect();
        let scroll_areas = self.scroll_areas.clone();
        #[allow(clippy::type_complexity)]
        let layers: Vec<(
            worker::Placement,
            bool,
            Vec<(olo_web::paint::DeviceRect, Arc<RenderImage>)>,
            Option<(olo_web::paint::DeviceRect, u64, bool)>,
            Option<(olo_web::paint::DeviceRect, [f32; 4])>,
            Option<(olo_web::paint::DeviceRect, Arc<gpui::YuvFrame>)>,
            Vec<(olo_web::paint::DeviceRect, [f32; 4], f32, f32, bool)>,
        )> = self.layers.iter().map(|l| (l.placement, l.scaled, l.tiles.clone(), l.shared, l.mask, l.yuv.clone(), l.backdrops.clone())).collect();
        let shared_requests = std::mem::take(&mut self.shared_requests);
        let shared_retired = std::mem::take(&mut self.shared_retired);
        let made_shared: std::rc::Rc<std::cell::RefCell<Vec<u64>>> = Default::default();
        let made_into = made_shared.clone();
        let me_shared = cx.entity();
        let viewport = self.viewport.clone();
        let scroll = self.scroll;
        let extent = self.extent;
        let retired = std::mem::take(&mut self.retired);
        let animating = self.animating;
        let recycle = self.tx.clone();
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
                // What is no longer shown leaves the GPU atlas, then goes to
                // the worker to be dropped off this thread.
                if !retired.is_empty() {
                    for image in &retired {
                        let _ = window.drop_image(image.clone());
                    }
                    let _ = recycle.send(worker::Message::Recycle(retired));
                }
                for key in &shared_retired {
                    window.drop_shared_texture(olo_ui::gpui::ImageId(*key as usize));
                }
                // An animating page's frames come with the window's own: at
                // the next one, its next is asked for.
                if animating {
                    let tx = recycle.clone();
                    window.on_next_frame(move |_, _| {
                        let _ = tx.send(worker::Message::Frame);
                    });
                }
                // WebGL draws on the window's GPU (frames shared with no copy;
                // a page's canvas wakes no other GPU). Said before any canvas.
                static NAMED: std::sync::Once = std::sync::Once::new();
                NAMED.call_once(|| {
                    if let Some(uuid) = window.renderer_device_uuid() {
                        olo_js::webgl::prefer_device(uuid);
                    }
                });
                // Shared textures for animating WebGL canvases: three each
                // (one shown, one drawn into, one spare), on the canvas's GPU
                // only (else none: it stays read back).
                if !shared_requests.is_empty() {
                    let mut given = Vec::new();
                    for (node, w, h, uuid) in shared_requests {
                        let mut textures = Vec::new();
                        for _ in 0..3 {
                            let made = window.create_shared_texture(w, h);
                            if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
                                eprintln!(
                                    "  shared: made {:?} (window GPU {:02x?}, canvas GPU {:02x?})",
                                    made.as_ref().map(|(id, t)| (id.0, t.memory_size)),
                                    made.as_ref().map(|(_, t)| t.device_uuid),
                                    uuid
                                );
                            }
                            match made {
                                Some((id, t)) if t.device_uuid == uuid => {
                                    made_into.borrow_mut().push(id.0 as u64);
                                    textures.push((id.0 as u64, t.fd, t.memory_size, t.width, t.height));
                                }
                                Some((id, _)) => window.drop_shared_texture(id),
                                None => {}
                            }
                        }
                        if textures.len() < 3 {
                            for (key, ..) in textures.drain(..) {
                                window.drop_shared_texture(olo_ui::gpui::ImageId(key as usize));
                                made_into.borrow_mut().retain(|k| *k != key);
                            }
                        }
                        given.push((node, textures));
                    }
                    let _ = recycle.send(worker::Message::SharedTextures(given));
                    let made: Vec<u64> = std::mem::take(&mut *made_into.borrow_mut());
                    if !made.is_empty() {
                        let me = me_shared.clone();
                        cx.defer(move |cx| me.update(cx, |v, _| v.shared_keys.extend(made)));
                    }
                }
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
                    // The layers above the base, in paint order: page runs
                    // scrolled with it, the fixed layer not, a sticky layer
                    // shifted as the scroll has it (CSS Positioned Layout 3 §3.4).
                    let scroll_css = f64::from(scroll) / scale;
                    let height_css = f64::from(f32::from(bounds.size.height));
                    let page_rect = |r: &olo_web::paint::DeviceRect| {
                        Bounds::new(
                            point(bounds.left() + px((f64::from(r.x) / scale) as f32), bounds.top() + px((f64::from(r.y - scroll) / scale) as f32)),
                            size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                        )
                    };
                    for (placement, scaled, layer_tiles, shared, mask, yuv, backdrops) in &layers {
                        // A surface cut to its clip and rounded, on the GPU.
                        let content_mask = mask.as_ref().map(|(clip, _)| gpui::ContentMask { bounds: page_rect(clip) });
                        let corners = mask.as_ref().map_or(Default::default(), |(_, r)| gpui::Corners { top_left: px(r[0]), top_right: px(r[1]), bottom_right: px(r[2]), bottom_left: px(r[3]) });
                        // A video's picture as decoded, made RGB and scaled on the GPU.
                        if let Some((r, frame)) = yuv {
                            window.with_content_mask(content_mask, |window| window.paint_yuv_rounded(page_rect(r), corners, frame.clone()));
                            continue;
                        }
                        // A canvas frame in a shared texture, with the page.
                        if let Some((r, key, premultiplied)) = shared {
                            window.with_content_mask(content_mask, |window| {
                                window.paint_shared_texture(page_rect(r), corners, olo_ui::gpui::ImageId(*key as usize), *premultiplied, !*scaled);
                            });
                            continue;
                        }
                        if mask.is_some() {
                            window.with_content_mask(content_mask, |window| {
                                for (r, image) in layer_tiles {
                                    let _ = if *scaled {
                                        window.paint_image(page_rect(r), corners, image.clone(), 0, false)
                                    } else {
                                        window.paint_image_nearest_rounded(page_rect(r), corners, image.clone())
                                    };
                                }
                            });
                            continue;
                        }
                        let (dy, scrolled) = match placement {
                            worker::Placement::Page | worker::Placement::Moved { .. } => (0., true),
                            worker::Placement::Fixed => (0., false),
                            worker::Placement::Sticky(st) => (st.offset(scroll_css, height_css), true),
                        };
                        // A moved layer: its tiles shifted by its translation
                        // (whole device pixels), within its ancestors' clip.
                        let (mx, my, clip) = match placement {
                            worker::Placement::Moved { dx, dy, clip } => (*dx, *dy, *clip),
                            _ => (0, 0, None),
                        };
                        let mask = clip.map(|c| gpui::ContentMask {
                            bounds: Bounds::new(
                                point(bounds.left() + px((f64::from(c.x) / scale) as f32), bounds.top() + px((f64::from(c.y - scroll) / scale) as f32)),
                                size(px((f64::from(c.width) / scale) as f32), px((f64::from(c.height) / scale) as f32)),
                            ),
                        });
                        // What shows under its `backdrop-filter` boxes, blurred
                        // (on the GPU), placed as its tiles are, under them.
                        for (r, radii, sigma, saturation, saturate_first) in backdrops {
                            let y = f64::from(if scrolled { r.y + my - scroll } else { r.y }) / scale + dy;
                            let b = Bounds::new(
                                point(bounds.left() + px((f64::from(r.x + mx) / scale) as f32), bounds.top() + px(y as f32)),
                                size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                            );
                            let corners = gpui::Corners { top_left: px(radii[0]), top_right: px(radii[1]), bottom_right: px(radii[2]), bottom_left: px(radii[3]) };
                            window.paint_backdrop(b, corners, px(*sigma), *saturation, *saturate_first);
                        }
                        window.with_content_mask(mask, |window| {
                            for (r, image) in layer_tiles {
                                let y = f64::from(if scrolled { r.y + my - scroll } else { r.y }) / scale + dy;
                                let b = Bounds::new(
                                    point(bounds.left() + px((f64::from(r.x + mx) / scale) as f32), bounds.top() + px(y as f32)),
                                    size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                                );
                                // Tiles texel for pixel; a surface at another
                                // density sampled linearly (as Chrome scales a canvas).
                                let _ = if *scaled { window.paint_image(b, Default::default(), image.clone(), 0, false) } else { window.paint_image_nearest(b, image.clone()) };
                            }
                        });
                    }
                    let shift = |st: &olo_web::layout::Sticky| st.offset(scroll_css, height_css);
                    for (text, r, link, image, fixed, sticky, control, node) in &labels {
                        // The worker's labels reach past the viewport (they
                        // serve the scroll near it): agents read what shows.
                        let top = r.y - if *fixed { 0. } else { f64::from(scroll) / scale } + sticky.as_ref().map_or(0., shift);
                        if top + r.height <= 0. || top >= height_css {
                            continue;
                        }
                        agent::record(
                            window,
                            cx,
                            agent::Node {
                                document_content: true,
                                role: if let Some(c) = control {
                                    c.role
                                } else if link.is_some() {
                                    "Link"
                                } else if *image {
                                    "Image"
                                } else {
                                    "Label"
                                },
                                name: Some(text.clone()),
                                value: control.as_ref().and_then(|c| c.value.clone()).or_else(|| link.as_ref().map(|l| l.url.clone())),
                                checked: control.as_ref().and_then(|c| c.checked),
                                clickable: link.is_some() || control.is_some(),
                                focused: link.as_ref().is_some_and(|l| Some(l.node) == focused) || control.is_some() && node.is_some() && (*node == focused || *node == page_focus),
                                enabled: !control.as_ref().is_some_and(|c| c.disabled),
                                bounds: Bounds::new(
                                    point(bounds.left() + px(r.x as f32), bounds.top() + px((r.y - if *fixed { 0. } else { f64::from(scroll) / scale } + sticky.as_ref().map_or(0., shift)) as f32)),
                                    size(px(r.width as f32), px(r.height as f32)),
                                ),
                                ..Default::default()
                            },
                        );
                    }
                    // The page's scroll containers: an agent scrolls one by
                    // the wheel over it (`olo-screen scroll` at its bounds).
                    for a in scroll_areas.iter() {
                        let top = a.rect.y - if a.fixed { 0. } else { f64::from(scroll) / scale };
                        agent::record(
                            window,
                            cx,
                            agent::Node {
                                document_content: true,
                                role: "ScrollArea",
                                value: Some(format!("{:.0},{:.0} of {:.0},{:.0}", a.offset[0], a.offset[1], a.max[0], a.max[1]).into()),
                                enabled: true,
                                bounds: Bounds::new(point(bounds.left() + px(a.rect.x as f32), bounds.top() + px(top as f32)), size(px(a.rect.width as f32), px(a.rect.height as f32))),
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
                    let (delta_x, delta) = match e.delta {
                        ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                        ScrollDelta::Lines(p) => (p.x * 40., p.y * 40.),
                    };
                    let scale = f64::from(w.scale_factor());
                    // Over a scroll container with room that way: the box
                    // scrolls (the innermost, painted last), not the page.
                    let (mut dx, mut dy) = (-f64::from(delta_x), -f64::from(delta));
                    // Shift turns a wheel sideways (a carousel, a wide table), as in Chrome.
                    if e.modifiers.shift && dx == 0. {
                        (dx, dy) = (dy, 0.);
                    }
                    let bounds = v.viewport.get();
                    let px = f64::from(f32::from(e.position.x - bounds.left()));
                    let py = f64::from(f32::from(e.position.y - bounds.top()));
                    let page_y = py + f64::from(v.scroll) / scale;
                    let inside = |a: &worker::ScrollArea| {
                        let y = if a.fixed { py } else { page_y };
                        px >= a.rect.x && px < a.rect.x + a.rect.width && y >= a.rect.y && y < a.rect.y + a.rect.height
                    };
                    if let Some(i) = v.scroll_areas.iter().rposition(|a| inside(a) && a.takes(dx, dy)) {
                        let mut areas = v.scroll_areas.to_vec();
                        let a = &mut areas[i];
                        // Kept here as it will be, so wheel steps before the
                        // next render still know where the box's ends are.
                        a.offset = [(a.offset[0] + dx).clamp(0., a.max[0]), (a.offset[1] + dy).clamp(0., a.max[1])];
                        let _ = v.tx.send(worker::Message::ScrollBox { node: a.node, dx, dy });
                        v.scroll_areas = areas.into();
                        v.send_input(worker::InputKind::Wheel { dx, dy }, Some(e.position), e.modifiers);
                        v.scrolled(cx);
                        return;
                    }
                    // Sideways with nothing to take it: the page doesn't scroll sideways.
                    if dy == 0. {
                        v.send_input(worker::InputKind::Wheel { dx, dy }, Some(e.position), e.modifiers);
                        return;
                    }
                    let max = v.key.as_ref().map_or(0, |key| worker::max_scroll(key, v.extent));
                    let old = v.scroll;
                    v.scroll = (v.scroll - (f64::from(delta) * scale).round() as i32).clamp(0, max);
                    v.send_input(worker::InputKind::Wheel { dx: 0., dy: -f64::from(delta) }, Some(e.position), e.modifiers);
                    if v.scroll == old {
                        return;
                    }
                    v.send_input(worker::InputKind::Scroll, None, e.modifiers);
                    v.scrolled(cx);
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
/// What a page may use, from the machine's size: the browser's process (its
/// one page) runs in a scope of its own with these, so a page that leaks,
/// spins or floods the GPU's memory slows or ends alone, never the machine
/// (a GPU-atlas leak once took 35 GB of a 62 GB laptop and its session).
/// Memory: a quarter of the RAM before it is reclaimed hard (at most 4 GiB),
/// three eighths at most (6 GiB), 256 MiB of swap. CPU: a quarter of the
/// cores, at least two, at half the weight of the desktop's programs.
fn page_limits() -> olo_xdg::scope::Limits {
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| m.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map_or(8 << 30, |kb| kb * 1024);
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u64);
    const GIB: u64 = 1 << 30;
    olo_xdg::scope::Limits {
        memory_high: (total / 4).min(4 * GIB),
        memory_max: (total * 3 / 8).min(6 * GIB),
        swap_max: 256 << 20,
        cpu_quota_us: (cores / 4).max(2) * 1_000_000,
        cpu_weight: 50,
        tasks_max: 512,
    }
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let limits = page_limits();
    if std::env::var_os("OLO_BROWSER_TIMING").is_some() {
        eprintln!("Browser: limits {limits:?}");
    }
    olo_xdg::scope::confine_self(APP_ID, limits);
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
                let reply = *message.downcast::<worker::Reply>().unwrap();
                if let worker::Reply::Navigate { id, url, replace } = reply {
                    // The page's own navigation (a script, a form, a link it
                    // let be followed): as a link's, replacing the history
                    // entry when it asked to; only to the web or, from a
                    // local page, local files.
                    let from_file = messages.borrow().as_ref().is_some_and(|v| v.read(cx).history.current().url.starts_with("file:"));
                    let allowed = url::Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") || u.scheme() == "file" && from_file);
                    if !allowed {
                        if let Some(v) = messages.borrow().as_ref() {
                            v.update(cx, |v, cx| {
                                v.status = "This link's URL scheme is not supported yet".into();
                                cx.notify();
                            });
                        }
                    } else if let Some(v) = messages.borrow().as_ref()
                        && let Some(handle) = handle
                        && v.read(cx).id == id
                    {
                        let _ = handle.update(cx, |_, w, cx| {
                            v.update(cx, |v, cx| {
                                if replace {
                                    v.history.location(url.clone());
                                }
                                v.navigate(url, w, cx)
                            })
                        });
                    }
                } else if let Some(v) = messages.borrow().as_ref() {
                    v.update(cx, |v, cx| v.reply(reply, cx));
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
