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
    scroll_x: f64,
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
    /// Metadata delivered before Ready that changes what the window exposes.
    content_changed: bool,
    navigation: u64,
    seek_anchor: bool,
    focus_scroll: Option<olo_web::dom::NodeId>,
    /// What the document is posted with (a form's submission): only for the
    /// navigation that sent it; going back to it, or reloading, gets it.
    post: Option<Arc<worker::Post>>,
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
    wheel_serial: u64,
    /// Stop rejects defaults for input already handed to the page worker.
    wheel_cancelled_through: u64,
    wheel_pending: Option<(u64, gpui::DeferredWheel)>,
    /// The page's scroll containers, as the last render had them.
    scroll_areas: Arc<[worker::ScrollArea]>,
    status: String,
    blocked_action: Option<&'static str>,
    /// The page's scripts asked to go this far back or forward past its
    /// own entries (`history.go`): done at the next render.
    pending_traverse: Option<isize>,
    scroll: i32,
    scroll_x: i32,
    root_viewport: Option<worker::RootViewport>,
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
            history.scroll([saved.scroll_x, saved.scroll]);
        }
        let source = history.current().url.clone();
        let cwd = history.current().cwd.clone();
        let loaded_group = history.current().document;
        let scroll = history.current().scroll;
        let scroll_x = history.current().scroll_x;
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
            content_changed: false,
            navigation: 1,
            seek_anchor: !restoring,
            focus_scroll: None,
            post: None,
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
            wheel_serial: 0,
            wheel_cancelled_through: 0,
            wheel_pending: None,
            status: "Loading…".into(),
            blocked_action: None,
            pending_traverse: None,
            scroll: worker::device_scroll(scroll, f64::from(window.scale_factor())),
            scroll_x: worker::device_scroll(scroll_x, f64::from(window.scale_factor())),
            root_viewport: None,
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
            self.scroll = worker::device_scroll(f64::from(self.scroll) / old.scale, scale);
            self.scroll_x = worker::device_scroll(f64::from(self.scroll_x) / old.scale, scale);
            self.root_viewport = None;
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
            scroll_x: self.scroll_x,
            cancel: self.cancel.clone(),
            recycle,
            interaction: self.interaction,
            navigation: self.navigation,
            fragment,
            seek_anchor: self.seek_anchor,
            focus_scroll: self.focus_scroll,
            post: self.post.clone(),
        })));
        cx.notify();
    }
    fn css_scroll(&self) -> [f64; 2] {
        let scale = self.key.as_ref().map_or(1., |k| k.scale);
        [f64::from(self.scroll_x) / scale, f64::from(self.scroll) / scale]
    }
    fn discard_targets(&mut self) {
        let old = std::mem::take(&mut self.focus_targets);
        if !old.is_empty() {
            let _ = self.tx.send(worker::Message::RecycleTargets(old));
        }
    }
    fn begin_navigation(&mut self, entry: history::Entry, same: bool, seek: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.wheel_pending.take();
        self.cancel.cancel();
        if !same {
            let _ = self.tx.send(worker::Message::CancelStylesheets(self.navigation));
        }
        self.id += 1;
        self.navigation += 1;
        self.seek_anchor = seek;
        self.focus_scroll = None;
        self.interaction = Default::default();
        self.pressed = None;
        self.blocked_action = None;
        self.post = None;
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
            self.scroll = worker::device_scroll(entry.scroll, f64::from(window.scale_factor()));
            self.scroll_x = worker::device_scroll(entry.scroll_x, f64::from(window.scale_factor()));
            self.key = None;
            self.root_viewport = None;
            self.live = false;
            self.labels.clear();
            self.interaction_mask = 0;
            self.document_snapshot = Default::default();
            self.discard_targets();
            cx.notify();
        }
    }
    fn navigate(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate_with(url, None, window, cx);
    }
    /// Navigates, posting `post` when there is one (a form's submission:
    /// always another document, even to the same URL).
    fn navigate_with(&mut self, url: String, post: Option<Arc<worker::Post>>, window: &mut Window, cx: &mut Context<Self>) {
        self.history.scroll(self.css_scroll());
        let same = post.is_none() && self.document_ready && url.contains('#') && url.split('#').next() == self.history.current().url.split('#').next();
        if url != self.history.current().url || post.is_some() {
            self.history.push(url, self.cwd.clone(), same);
        }
        let entry = self.history.current().clone();
        self.begin_navigation(entry, same, true, window, cx);
        self.post = post;
    }
    fn traverse(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.history.scroll(self.css_scroll());
        let Some(entry) = self.history.go(delta) else {
            return;
        };
        let same = self.document_ready && entry.document == self.loaded_group;
        if same {
            self.scroll = worker::device_scroll(entry.scroll, f64::from(window.scale_factor()));
            self.scroll_x = worker::device_scroll(entry.scroll_x, f64::from(window.scale_factor()));
            // An entry of this document: its scripts take it (popstate:
            // a single-page app shows what that URL is).
            let _ = self.tx.send(worker::Message::Traverse(entry.url.clone()));
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
    /// A select's menu, opened over it with its chosen option lying over
    /// it (the kit's menu field popup); the option chosen goes to the page.
    fn open_select(&mut self, menu: worker::SelectMenu, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = &self.key else { return };
        let bounds = self.viewport.get();
        // The window's points are CSS pixels; the scroll is in device pixels.
        let scroll = if menu.fixed { 0. } else { f64::from(self.scroll) / key.scale };
        let field = Bounds::new(
            point(bounds.left() + px((menu.rect.x - if menu.fixed { 0. } else { f64::from(self.scroll_x) / key.scale }) as f32), bounds.top() + px((menu.rect.y - scroll) as f32)),
            gpui::size(px(menu.rect.width as f32), px(menu.rect.height as f32)),
        );
        let mut m = olo_ui::menu::Menu::new("Options");
        for (i, (label, disabled)) in menu.options.iter().enumerate() {
            m = m.item(if label.is_empty() { " ".to_owned() } else { label.clone() }, i.to_string()).enabled(!disabled);
        }
        let tx = self.tx.clone();
        let node = menu.node;
        olo_ui::menu::popup_over(window, cx, m, field, menu.current, move |command, _, _| {
            if let Ok(index) = command.parse::<usize>() {
                let _ = tx.send(worker::Message::ChooseOption(node, index));
            }
        });
    }
    fn send_input(&mut self, kind: worker::InputKind, at: Option<gpui::Point<gpui::Pixels>>, modifiers: gpui::Modifiers) {
        let Some(key) = &self.key else { return };
        let bounds = self.viewport.get();
        let scale = key.scale;
        // The window's points are CSS pixels; the scroll is in device pixels.
        let (x, y) =
            at.map_or((0., 0.), |at| (f64::from(f32::from(at.x - bounds.left())) + f64::from(self.scroll_x) / scale, f64::from(f32::from(at.y - bounds.top())) + f64::from(self.scroll) / scale));
        let _ = self.tx.send(worker::Message::Input {
            navigation: self.navigation,
            wheel: matches!(kind, worker::InputKind::Wheel { .. }).then_some(self.wheel_serial),
            input: worker::Input {
                kind,
                x,
                y,
                scroll_y: f64::from(self.scroll) / scale,
                scroll_x: f64::from(self.scroll_x) / scale,
                buttons: u32::from(self.pressed.is_some() || self.interaction.active.is_some()),
                modifiers: u8::from(modifiers.shift) | u8::from(modifiers.control) << 1 | u8::from(modifiers.alt) << 2 | u8::from(modifiers.platform) << 3,
            },
        });
    }
    /// The element (and link) under window point `at`, as the last
    /// render's labels have it.
    fn under(&self, at: gpui::Point<gpui::Pixels>) -> (Option<olo_web::dom::NodeId>, Option<Arc<links::Link>>) {
        let bounds = self.viewport.get();
        let scale = self.key.as_ref().map_or(1., |k| k.scale);
        let viewport_x = f64::from(f32::from(at.x - bounds.left()));
        let x = viewport_x + f64::from(self.scroll_x) / scale;
        let viewport_y = f64::from(f32::from(at.y - bounds.top()));
        let y = viewport_y + f64::from(self.scroll) / scale;
        let height_css = self.root_viewport.map_or(f64::from(f32::from(bounds.size.height)), |v| v.geometry.client[1]);
        let shift = |st: &olo_web::layout::Sticky| st.offset(f64::from(self.scroll) / scale, height_css);
        // Fixed and sticky labels paint over the page: tried first, where
        // they are shown.
        let label = self.labels.iter().rev().filter(|l| l.fixed || l.sticky.is_some()).chain(self.labels.iter().rev().filter(|l| !l.fixed && l.sticky.is_none())).find(|l| {
            let ly = if l.fixed { viewport_y } else { y - l.sticky.as_ref().map_or(0., shift) };
            let lx = if l.fixed { viewport_x } else { x };
            lx >= l.rect.x && lx < l.rect.x + l.rect.width && ly >= l.rect.y && ly < l.rect.y + l.rect.height
        });
        (label.and_then(|l| l.node), label.and_then(|l| l.link.clone()))
    }
    /// The wheel's default action, after the document has had a chance to cancel it.
    fn wheel_default(&mut self, dx: f64, dy: f64, at: gpui::Point<gpui::Pixels>, modifiers: gpui::Modifiers, cx: &mut Context<Self>) -> bool {
        let scale = self.key.as_ref().map_or(1., |k| k.scale);
        let bounds = self.viewport.get();
        let px = f64::from(f32::from(at.x - bounds.left()));
        let py = f64::from(f32::from(at.y - bounds.top()));
        let page_y = py + f64::from(self.scroll) / scale;
        let page_x = px + f64::from(self.scroll_x) / scale;
        let inside = |a: &worker::ScrollArea| {
            let y = if a.fixed { py } else { page_y };
            let x = if a.fixed { px } else { page_x };
            x >= a.rect.x && x < a.rect.x + a.rect.width && y >= a.rect.y && y < a.rect.y + a.rect.height
        };
        if let Some(i) = self.scroll_areas.iter().rposition(|a| inside(a) && a.takes(dx, dy)) {
            let mut areas = self.scroll_areas.to_vec();
            let a = &mut areas[i];
            // Kept here as it will be, so wheel steps before the
            // next render still know where the box's ends are.
            a.offset = [(a.offset[0] + dx).clamp(0., a.max[0]), (a.offset[1] + dy).clamp(0., a.max[1])];
            let _ = self.tx.send(worker::Message::ScrollBox { node: a.node, dx, dy });
            self.scroll_areas = areas.into();
            self.scrolled(cx);
            cx.notify();
            return true;
        }
        let user = self.root_viewport.map_or([false; 2], |r| r.user);
        let x = self.scroll_x.saturating_add(if user[0] { (dx * scale).round() as i32 } else { 0 });
        let y = self.scroll.saturating_add(if user[1] { (dy * scale).round() as i32 } else { 0 });
        let old = [self.scroll_x, self.scroll];
        self.scroll_page_to([x, y], modifiers, cx);
        old != [self.scroll_x, self.scroll]
    }
    fn scroll_page_to(&mut self, to: [i32; 2], modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let old = [self.scroll_x, self.scroll];
        let position = self.root_viewport.map_or(old, |v| v.clamp(to));
        [self.scroll_x, self.scroll] = position;
        if position == old {
            return;
        }
        self.send_input(worker::InputKind::Scroll, None, modifiers);
        self.scrolled(cx);
        self.seek_anchor = false;
        self.focus_scroll = None;
        if let Some(key) = self.key.clone() {
            let mut recycle = Vec::new();
            let height = key.height as i32;
            let scroll = self.scroll;
            let x = self.scroll_x;
            let width = key.width as i32;
            self.tiles.retain(|(r, image)| {
                let keep = r.y + r.height as i32 > scroll && r.y < scroll + height && r.x + r.width as i32 > x && r.x < x + width;
                if !keep {
                    recycle.push(image.clone());
                }
                keep
            });
            self.send_render(recycle, cx);
        }
        cx.notify();
    }
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
        let bounds = self.viewport.get();
        if self.root_viewport.is_some_and(|v| f64::from(f32::from(at.x - bounds.left())) >= v.geometry.client[0] || f64::from(f32::from(at.y - bounds.top())) >= v.geometry.client[1]) {
            return;
        }
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
                self.wheel_cancelled_through = self.wheel_serial;
                self.wheel_pending.take();
                self.cancel.cancel();
                let _ = self.tx.send(worker::Message::CancelStylesheets(self.navigation));
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
    fn merge_tiles(&mut self, tiles: Vec<(olo_web::paint::DeviceRect, Arc<RenderImage>)>) -> bool {
        let mut recycle = Vec::new();
        let mut changed = false;
        for (r, image) in tiles {
            if let Some(old) = self.tiles.iter_mut().find(|(old, _)| *old == r) {
                let previous = std::mem::replace(&mut old.1, image);
                if !Arc::ptr_eq(&previous, &old.1) {
                    changed = true;
                    recycle.push(previous);
                }
            } else {
                changed = true;
                self.tiles.push((r, image));
            }
        }
        self.recycle(recycle);
        changed
    }
    fn reply(&mut self, message: worker::Reply, cx: &mut Context<Self>) {
        match message {
            worker::Reply::DefaultWheel { navigation, serial, input, prevented } => {
                let mut moved = false;
                if navigation == self.navigation
                    && serial > self.wheel_cancelled_through
                    && !prevented
                    && let worker::InputKind::Wheel { dx, dy } = input.kind
                {
                    let bounds = self.viewport.get();
                    let at = point(bounds.left() + px((input.x - input.scroll_x) as f32), bounds.top() + px((input.y - input.scroll_y) as f32));
                    let modifiers =
                        gpui::Modifiers { shift: input.modifiers & 1 != 0, control: input.modifiers & 2 != 0, alt: input.modifiers & 4 != 0, platform: input.modifiers & 8 != 0, ..Default::default() };
                    moved = self.wheel_default(dx, dy, at, modifiers, cx);
                }
                if olo_web::env_on!("OLO_SCROLL_DEBUG") {
                    eprintln!("wheel reply {serial}: prevented {prevented}, moved {moved}, pending {:?}", self.wheel_pending.as_ref().map(|p| p.0));
                }
                if self.wheel_pending.as_ref().is_some_and(|(pending, _)| *pending == serial) {
                    if let Some((_, pending)) = self.wheel_pending.take() {
                        pending.complete(moved);
                    }
                }
                return;
            }
            worker::Reply::Location(id, url) if id == self.id => {
                // Preserve the request cache key through redirects; display and
                // persist the final URL. Reload uses this canonical location.
                self.history.location(url.clone());
                self.address.update(cx, |f, cx| f.set_text(url, cx));
            }
            // The page moved its session history: an entry of this document
            // pushed (pushState) or retitled (replaceState), or the page
            // traversed its own entries; or it asks to go past them.
            worker::Reply::History(id, change) if id == self.id => {
                use olo_js::HistoryChange as H;
                let url = match change {
                    H::Push(url) => {
                        self.history.scroll(self.css_scroll());
                        self.history.push(url.clone(), self.cwd.clone(), true);
                        url
                    }
                    H::Replace(url) => {
                        self.history.location(url.clone());
                        url
                    }
                    H::Go(url, delta) => {
                        let _ = self.history.go(delta);
                        self.history.location(url.clone());
                        url
                    }
                    H::Leave(delta) => {
                        self.pending_traverse = Some(delta);
                        cx.notify();
                        return;
                    }
                };
                self.address.update(cx, |f, cx| f.set_text(url, cx));
                cx.notify();
            }
            worker::Reply::Status(id, s) if id == self.id => self.status = s,
            // The page has something to show: a frame, which asks it for its next.
            worker::Reply::WantFrame(id) if id == self.id => self.animating = true,
            worker::Reply::Error(id, s) if id == 0 || id == self.id => self.status = s,
            worker::Reply::Ready { id, title, fallbacks, tiles, animating, retained } if id == self.id => {
                let status = self.blocked_action.map_or(title, str::to_owned);
                let content_changed = std::mem::take(&mut self.content_changed);
                let changed = self.merge_tiles(tiles) || content_changed || !retained || self.animating != animating || !self.document_ready || self.status != status || self.fallbacks != fallbacks;
                self.animating = animating;
                self.document_ready = true;
                self.status = status;
                self.fallbacks = fallbacks;
                let (document, navigation) = self.snapshot_identity;
                self.document_snapshot = serde_json::json!({"url":self.history.current().url,"navigation":navigation,"document":document,"fallbacks":self.fallbacks}).to_string().into();
                if !changed {
                    return;
                }
            }
            worker::Reply::Content { id, labels, extent, scroll, scroll_x, viewport, interaction_mask, document, target, focus_targets, scroll_areas, page_focus, live } if id == self.id => {
                self.content_changed |= self.page_focus != page_focus
                    || self.live != live
                    || self.extent != extent
                    || self.scroll != scroll
                    || self.scroll_x != scroll_x
                    || self.root_viewport != Some(viewport)
                    || self.interaction.target != target
                    || !Arc::ptr_eq(&self.focus_targets, &focus_targets)
                    || self.snapshot_identity != (document, self.navigation);
                self.labels = labels;
                self.page_focus = page_focus;
                self.live = live;
                self.scroll_areas = scroll_areas;
                self.interaction_mask = interaction_mask;
                self.extent = extent;
                self.scroll = scroll;
                self.scroll_x = scroll_x;
                self.root_viewport = Some(viewport);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The screen it is on and its own size, for pages' `screen` and
        // `outerWidth` (as Chrome tells them).
        if let Some(display) = window.display(cx) {
            let (screen, own) = (display.bounds().size, window.bounds().size);
            olo_js::set_display([f64::from(screen.width), f64::from(screen.height)], [f64::from(own.width), f64::from(own.height)]);
        }
        // history.go past the page's own entries: the window goes there.
        if let Some(delta) = self.pending_traverse.take() {
            self.traverse(delta, window, cx);
        }
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
        let gutter_background = p.panel_background;
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
            f32,
        )> = self.layers.iter().map(|l| (l.placement, l.scaled, l.tiles.clone(), l.shared, l.mask, l.yuv.clone(), l.backdrops.clone(), l.opacity)).collect();
        let shared_requests = std::mem::take(&mut self.shared_requests);
        let shared_retired = std::mem::take(&mut self.shared_retired);
        let made_shared: std::rc::Rc<std::cell::RefCell<Vec<u64>>> = Default::default();
        let made_into = made_shared.clone();
        let me_shared = cx.entity();
        let viewport = self.viewport.clone();
        let scroll = self.scroll;
        let scroll_x = self.scroll_x;
        let root_viewport = self.root_viewport;
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
                            if olo_web::env_on!("OLO_BROWSER_TIMING") {
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
                let room = root_viewport.map_or([0.; 2], |v| std::array::from_fn(|i| f64::from(v.max[i] - v.min[i]) / scale));
                window.note_scroll([(f64::from(scroll_x) / scale) as f32, (f64::from(scroll) / scale) as f32, room[0] as f32, room[1] as f32]);
                window.paint_quad(gpui::fill(bounds, style::c(root_viewport.map_or(p.document_background, |v| v.canvas))));
                let content_bounds = root_viewport.map_or(bounds, |v| {
                    let c = v.geometry.content;
                    Bounds::new(point(bounds.left() + px(c.x as f32), bounds.top()), size(px(c.width as f32), px(c.height as f32)))
                });
                agent::record(window, cx, agent::Node { role: "Document", name: Some("Page".into()), value: Some(document_snapshot.clone()), bounds, enabled: true, ..Default::default() });
                window.with_content_mask(Some(gpui::ContentMask { bounds: content_bounds }), |window| {
                    for (r, image) in &tiles {
                        let b = Bounds::new(
                            point(bounds.left() + px((f64::from(r.x - scroll_x) / scale) as f32), bounds.top() + px((f64::from(r.y - scroll) / scale) as f32)),
                            size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                        );
                        let _ = window.paint_image_nearest(b, image.clone());
                    }
                    // The layers above the base, in paint order: page runs
                    // scrolled with it, the fixed layer not, a sticky layer
                    // shifted as the scroll has it (CSS Positioned Layout 3 §3.4).
                    let scroll_css = f64::from(scroll) / scale;
                    let height_css = root_viewport.map_or(f64::from(f32::from(bounds.size.height)), |v| v.geometry.client[1]);
                    let page_rect = |r: &olo_web::paint::DeviceRect| {
                        Bounds::new(
                            point(bounds.left() + px((f64::from(r.x - scroll_x) / scale) as f32), bounds.top() + px((f64::from(r.y - scroll) / scale) as f32)),
                            size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                        )
                    };
                    for (placement, scaled, layer_tiles, shared, mask, yuv, backdrops, opacity) in &layers {
                        // A surface cut to its clip and rounded, on the GPU, at
                        // its opacity groups' opacity.
                        let content_mask = mask.as_ref().map(|(clip, _)| gpui::ContentMask { bounds: page_rect(clip) });
                        let corners = mask.as_ref().map_or(Default::default(), |(_, r)| gpui::Corners { top_left: px(r[0]), top_right: px(r[1]), bottom_right: px(r[2]), bottom_left: px(r[3]) });
                        let faded = (*opacity < 1.).then_some(*opacity);
                        // A video's picture as decoded, made RGB and scaled on the GPU.
                        if let Some((r, frame)) = yuv {
                            window.with_element_opacity(faded, |window| window.with_content_mask(content_mask, |window| window.paint_yuv_rounded(page_rect(r), corners, frame.clone())));
                            continue;
                        }
                        // A canvas frame in a shared texture, with the page.
                        if let Some((r, key, premultiplied)) = shared {
                            window.with_element_opacity(faded, |window| {
                                window.with_content_mask(content_mask, |window| {
                                    window.paint_shared_texture(page_rect(r), corners, olo_ui::gpui::ImageId(*key as usize), *premultiplied, !*scaled);
                                })
                            });
                            continue;
                        }
                        if mask.is_some() || faded.is_some() {
                            window.with_element_opacity(faded, |window| {
                                window.with_content_mask(content_mask, |window| {
                                    for (r, image) in layer_tiles {
                                        let _ = if *scaled {
                                            window.paint_image(page_rect(r), corners, image.clone(), 0, false)
                                        } else {
                                            window.paint_image_nearest_rounded(page_rect(r), corners, image.clone())
                                        };
                                    }
                                })
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
                        let (mx, my, clip, matrix) = match placement {
                            worker::Placement::Moved { dx, dy, clip, matrix } => (*dx, *dy, *clip, *matrix),
                            _ => (0, 0, None, None),
                        };
                        let mask = clip.map(|c| gpui::ContentMask {
                            bounds: Bounds::new(
                                point(bounds.left() + px((f64::from(c.x - scroll_x) / scale) as f32), bounds.top() + px((f64::from(c.y - scroll) / scale) as f32)),
                                size(px((f64::from(c.width) / scale) as f32), px((f64::from(c.height) / scale) as f32)),
                            ),
                        });
                        // What shows under its `backdrop-filter` boxes, blurred
                        // (on the GPU), placed as its tiles are, under them.
                        for (r, radii, sigma, saturation, saturate_first) in backdrops {
                            let y = f64::from(if scrolled { r.y + my - scroll } else { r.y }) / scale + dy;
                            let b = Bounds::new(
                                point(bounds.left() + px((f64::from(r.x + mx - if scrolled { scroll_x } else { 0 }) / scale) as f32), bounds.top() + px(y as f32)),
                                size(px((f64::from(r.width) / scale) as f32), px((f64::from(r.height) / scale) as f32)),
                            );
                            let corners = gpui::Corners { top_left: px(radii[0]), top_right: px(radii[1]), bottom_right: px(radii[2]), bottom_left: px(radii[3]) };
                            window.paint_backdrop(b, corners, px(*sigma), *saturation, *saturate_first);
                        }
                        // Turned or scaled (a spinner): each tile two
                        // triangles, its corners through the layer's matrix,
                        // sampled on the GPU; nothing painted again.
                        if let Some(m) = matrix {
                            window.with_content_mask(mask, |window| {
                                for (r, image) in layer_tiles {
                                    let at = |x: f64, y: f64| {
                                        let (dx, dy) = (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
                                        point(bounds.left() + px(((dx - f64::from(scroll_x)) / scale) as f32), bounds.top() + px(((dy - f64::from(scroll)) / scale) as f32))
                                    };
                                    // Half a texel in from its edges: sampled
                                    // between texels, an edge reads its atlas
                                    // neighbour's (a one-tile layer has a
                                    // transparent pixel round it to spare).
                                    let (w, h) = (r.width as f32, r.height as f32);
                                    let (u0, v0, u1, v1) = (0.5 / w, 0.5 / h, 1. - 0.5 / w, 1. - 0.5 / h);
                                    let (x0, y0) = (f64::from(r.x) + 0.5, f64::from(r.y) + 0.5);
                                    let (x1, y1) = (f64::from(r.x) + f64::from(r.width) - 0.5, f64::from(r.y) + f64::from(r.height) - 0.5);
                                    let corners = [(at(x0, y0), point(u0, v0)), (at(x1, y0), point(u1, v0)), (at(x1, y1), point(u1, v1)), (at(x0, y1), point(u0, v1))];
                                    let triangles = [[corners[0], corners[1], corners[2]], [corners[0], corners[2], corners[3]]];
                                    let _ = window.paint_image_triangles(image.clone(), &triangles, gpui::white());
                                }
                            });
                            continue;
                        }
                        window.with_content_mask(mask, |window| {
                            for (r, image) in layer_tiles {
                                let y = f64::from(if scrolled { r.y + my - scroll } else { r.y }) / scale + dy;
                                let b = Bounds::new(
                                    point(bounds.left() + px((f64::from(r.x + mx - if scrolled { scroll_x } else { 0 }) / scale) as f32), bounds.top() + px(y as f32)),
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
                        let left = r.x - if *fixed { 0. } else { f64::from(scroll_x) / scale };
                        let c = root_viewport.map(|v| v.geometry.content);
                        if top + r.height <= 0.
                            || top >= c.map_or(height_css, |c| c.height)
                            || left + r.width <= c.map_or(0., |c| c.x)
                            || left >= c.map_or(f64::from(f32::from(bounds.size.width)), |c| c.x + c.width)
                        {
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
                                    point(
                                        bounds.left() + px((r.x - if *fixed { 0. } else { f64::from(scroll_x) / scale }) as f32),
                                        bounds.top() + px((r.y - if *fixed { 0. } else { f64::from(scroll) / scale } + sticky.as_ref().map_or(0., shift)) as f32),
                                    ),
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
                                bounds: Bounds::new(
                                    point(bounds.left() + px((a.rect.x - if a.fixed { 0. } else { f64::from(scroll_x) / scale }) as f32), bounds.top() + px(top as f32)),
                                    size(px(a.rect.width as f32), px(a.rect.height as f32)),
                                ),
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
        // Native bars occupy the exact space reserved by page layout.
        // The same shared control handles either axis and accepts worker-owned
        // positions; the full page surface still supplies window/media sizes.
        let mut page_bars: Vec<gpui::AnyElement> = Vec::new();
        if let Some(view) = self.root_viewport {
            let g = view.geometry;
            let scale = g.full.scale;
            for axis in 0..2 {
                if !g.bars[axis] {
                    continue;
                }
                let me = me.clone();
                let min = view.min[axis];
                let position = if axis == 0 { self.scroll_x } else { self.scroll };
                let shown = g.client[axis] as f32;
                let total = shown + (f64::from(view.max[axis] - min) / scale) as f32;
                let bar = olo_ui::widgets::ScrollBar::controlled(
                    if axis == 0 { "page-scroll-x" } else { "page-scroll-y" },
                    if axis == 0 { gpui::Axis::Horizontal } else { gpui::Axis::Vertical },
                    (f64::from(position - min) / scale) as f32,
                    shown,
                    total,
                )
                .on_scroll(move |to, _, cx| {
                    me.update(cx, |v, cx| {
                        let mut position = [v.scroll_x, v.scroll];
                        position[axis] = min + (f64::from(to) * scale).round() as i32;
                        v.scroll_page_to(position, gpui::Modifiers::default(), cx);
                    })
                });
                let (x, y, w, h) = if axis == 0 { (0., g.client[1], g.client[0], g.thickness) } else { (g.client[0], 0., g.thickness, g.client[1]) };
                page_bars.push(div().absolute().left(px(x as f32)).top(px(y as f32)).w(px(w as f32)).h(px(h as f32)).child(bar).into_any_element());
            }
            if g.bars == [true, true] {
                page_bars.push(
                    div().absolute().left(px(g.client[0] as f32)).top(px(g.client[1] as f32)).w(px(g.thickness as f32)).h(px(g.thickness as f32)).bg(style::c(gutter_background)).into_any_element(),
                );
            }
        }
        // Boxes scrolling inside the page (`overflow: auto`): a slim bar over
        // each one in view, as the page's (in CSS pixels: the box's offset,
        // its height, its content's).
        let area_bars: Vec<gpui::AnyElement> = match &self.key {
            Some(key) => {
                let view = f64::from(key.height) / key.scale;
                let page_top = f64::from(self.scroll) / key.scale;
                let page_left = f64::from(self.scroll_x) / key.scale;
                self.scroll_areas
                    .iter()
                    .filter(|a| a.user[1] && a.max[1] > 0.5)
                    .filter_map(|a| {
                        let top = if a.fixed { a.rect.y } else { a.rect.y - page_top };
                        if top + a.rect.height <= 0. || top >= view || a.rect.width < 2. * f64::from(olo_ui::widgets::SLIM_BAR) {
                            return None;
                        }
                        let (me, node) = (me.clone(), a.node);
                        let bar = olo_ui::widgets::SlimScrollBar::new(
                            gpui::ElementId::Name(format!("area-scroll-{node:?}").into()),
                            a.offset[1] as f32,
                            a.rect.height as f32,
                            (a.rect.height + a.max[1]) as f32,
                        )
                        .on_scroll(move |to, _, cx| {
                            me.update(cx, |v, cx| {
                                let mut areas = v.scroll_areas.to_vec();
                                let Some(a) = areas.iter_mut().find(|a| a.node == node) else { return };
                                let to = f64::from(to).clamp(0., a.max[1]);
                                let dy = to - a.offset[1];
                                if dy.abs() < 0.5 {
                                    return;
                                }
                                a.offset[1] = to;
                                let _ = v.tx.send(worker::Message::ScrollBox { node, dx: 0., dy });
                                v.scroll_areas = areas.into();
                                v.scrolled(cx);
                                cx.notify();
                            });
                        });
                        Some(
                            div()
                                .absolute()
                                .left(px((a.rect.x - if a.fixed { 0. } else { page_left }) as f32))
                                .top(px(top as f32))
                                .w(px(a.rect.width as f32))
                                .h(px(a.rect.height as f32))
                                .child(bar)
                                .into_any_element(),
                        )
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let surface = div()
            .id("document")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .relative()
            .child(surface)
            .children(area_bars)
            .children(page_bars)
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
                    // Over a scroll container with room that way: the box
                    // scrolls (the innermost, painted last), not the page.
                    let (mut dx, mut dy) = (-f64::from(delta_x), -f64::from(delta));
                    // Shift turns a wheel sideways (a carousel, a wide table), as in Chrome.
                    if e.modifiers.shift && dx == 0. {
                        (dx, dy) = (dy, 0.);
                    }
                    if v.live {
                        v.wheel_serial = v.wheel_serial.wrapping_add(1);
                        if let Some(pending) = w.defer_wheel() {
                            v.wheel_pending = Some((v.wheel_serial, pending));
                        }
                        if olo_web::env_on!("OLO_SCROLL_DEBUG") {
                            eprintln!("wheel input {}: {dx},{dy}, pending {:?}", v.wheel_serial, v.wheel_pending.as_ref().map(|p| p.0));
                        }
                        v.send_input(worker::InputKind::Wheel { dx, dy }, Some(e.position), e.modifiers);
                    } else {
                        v.wheel_default(dx, dy, e.position, e.modifiers, cx);
                    }
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
    if olo_web::env_on!("OLO_BROWSER_TIMING") {
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
                serde_json::to_string(&Saved { source: history.current().url.clone(), scroll: v.css_scroll()[1], scroll_x: v.css_scroll()[0], history: Some(history) }).unwrap_or_default()
            })
        });
        let messages = view.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            if message.is::<worker::Reply>() {
                let reply = *message.downcast::<worker::Reply>().unwrap();
                if let worker::Reply::Navigate { id, url, replace, post } = reply {
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
                                v.navigate_with(url, post, w, cx)
                            })
                        });
                    }
                } else if let worker::Reply::SelectMenu(_, menu) = reply {
                    // A drop-down pressed: its menu over it, as a menu field's.
                    if let Some(v) = messages.borrow().as_ref()
                        && let Some(handle) = handle
                    {
                        let _ = handle.update(cx, |_, w, cx| v.update(cx, |v, cx| v.open_select(menu, w, cx)));
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
