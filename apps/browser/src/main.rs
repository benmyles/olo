//! NetPositive: the web browser, on Servo (engine.rs) in process. Each
//! window has a toolbar (Back, Forward, Reload/Stop, Home, the logins key
//! and the location field), a tab strip once there is more than one tab,
//! the bar offering to save a login (logins.rs), the page, drawn by
//! Servo's GL straight into an embedded subsurface, and a status line.
//! Input over the page is forwarded from egui (input.rs).

mod downloads;
mod engine;
mod favicon;
mod find;
mod guard;
mod input;
mod location;
mod logins;
mod media;
mod passkeys;
mod rtc;
mod scheme;
mod scroll;

use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use engine::{Engine, Event, Surface, Wake};
use euclid::Scale;
use input::{Pointer, Viewport};
use downloads::{Download, PageDownload, Source};
use logins::{Answer, Offer, OfferKind, Purpose};
use passkeys::{Ask, AskKind, PageRequest};
use serde::{Deserialize, Serialize};
use servo::{
    AuthenticationRequest, ContextMenu, ContextMenuAction, ContextMenuItem, Cursor, ColorPicker, EmbedderControl, FilePicker,
    LoadStatus, RgbColor, SelectElement, SelectElementOptionOrOptgroup, SimpleDialog, Theme, WebView, WebViewBuilder, WebViewId,
};
use tabula_keychain::{Request, Response};
use tabula_kit::{
    App, Cx, EmbedId, Embedded, Menu, MenuItem, Proxy, WindowId, WindowOptions,
    egui::{self, Align, CursorIcon, Id, Key, Rect, Sense, Shape, Stroke, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    icons,
    instance::{self, Launch},
    menu::{self, MenuAnchor, MenuRequest},
    metrics as m,
    style::{self, c, shade},
    widgets::{self, Button, CheckBox, MenuField, ScrollBar, ScrollView, StatusBar, TextControl},
};
use url::Url;

const APP_ID: &str = "tabula.browser";
const NAME: &str = "NetPositive";
const STATUS_HEIGHT: f32 = m::LIST_ROW;
const TAB_HEIGHT: f32 = widgets::TAB_HEIGHT;
const TAB_MAX: f32 = 220.0;
/// Toolbar buttons are square.
const TOOL: f32 = m::CONTROL_HEIGHT + m::TIGHT;
/// The bars offering to save a login or asking about a passkey.
const OFFER_HEIGHT: f32 = m::CONTROL_HEIGHT + 2.0 * m::SPACING;
const PASSKEY_CHOICE_WIDTH: f32 = 180.0;
const DOWNLOAD_ROW: f32 = 60.0;
/// The most a page may hand over as a `blob:` download.
const BLOB_MAX: usize = 1 << 30;
const FIND_W: f32 = 340.0;
const FIND_H: f32 =
    2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + 4.0 * m::MARK_ROW + 3.0 * m::TIGHT + m::SPACING + m::CONTROL_HEIGHT;
/// How long the scroll bars trust themselves over the page's reports.
const SCROLL_SETTLE: Duration = Duration::from_millis(300);
/// The colour sheet's ColorControl, and how wide it is with its fields.
const COLOR_RAMPS_H: f32 = 3.0 * m::CONTROL_HEIGHT + 2.0 * m::TIGHT;
const COLOR_W: f32 = 260.0 + m::SPACING + 96.0;
const HISTORY_MAX: usize = 1000;
const ZOOMS: [f32; 13] = [0.3, 0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

#[derive(Serialize, Deserialize, Clone)]
struct Bookmark {
    title: String,
    url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    home: String,
    bookmarks: Vec<Bookmark>,
    /// Pages visited, most recent last.
    history: Vec<Bookmark>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { home: "about:blank".into(), bookmarks: Vec::new(), history: Vec::new() }
    }
}

fn settings_path() -> PathBuf {
    tabula_kit::config_dir().join("browser.json")
}

/// Windows and their tabs, for in-place reload.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    windows: Vec<SavedWindow>,
}

#[derive(Serialize, Deserialize)]
struct SavedWindow {
    tabs: Vec<String>,
    active: usize,
    size: (u32, u32),
}

struct Tab {
    view: Option<WebView>,
    /// Where it is (or should go, before it has a view).
    url: String,
    title: String,
    status: Option<String>,
    load: Option<LoadStatus>,
    can_back: bool,
    can_forward: bool,
    cursor: Cursor,
    zoom: f32,
    /// A login just used here, offered for saving (the bar under the toolbar).
    offer: Option<Offer>,
    /// The page's passkey request, asked about in a bar.
    passkey: Option<Ask>,
    /// The page's requests for the camera or microphone, asked about in
    /// a bar one at a time.
    media: VecDeque<rtc::MediaAsk>,
    /// Where the page is scrolled, as it last said.
    scroll: Option<scroll::Metrics>,
    /// Where our scroll bars just put it, shown over the page's reports
    /// (which lag a frame or two) until it settles.
    scrolled: Option<(Instant, scroll::Metrics)>,
    /// The page's icon, and a new one to upload on the next frame.
    favicon: Option<egui::TextureHandle>,
    new_favicon: Option<egui::ColorImage>,
    /// Whether the page loading now has given an icon (if not, the last
    /// page's goes when it's done).
    favicon_seen: bool,
    /// The page has stopped answering (guard.rs), and why it was stopped
    /// if it was.
    hung: bool,
    stopped: Option<String>,
}

impl Tab {
    fn new(url: impl Into<String>) -> Tab {
        Tab {
            view: None,
            url: url.into(),
            title: String::new(),
            status: None,
            load: None,
            can_back: false,
            can_forward: false,
            cursor: Cursor::Default,
            zoom: 1.0,
            offer: None,
            passkey: None,
            media: VecDeque::new(),
            scroll: None,
            scrolled: None,
            favicon: None,
            new_favicon: None,
            favicon_seen: false,
            hung: false,
            stopped: None,
        }
    }

    /// Where to show the page scrolled.
    fn scroll(&self) -> Option<scroll::Metrics> {
        match self.scrolled {
            Some((at, m)) if at.elapsed() < SCROLL_SETTLE => Some(m),
            _ => self.scroll,
        }
    }

    fn label(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else if self.url == "about:blank" {
            "Untitled".into()
        } else {
            self.url.clone()
        }
    }

    fn loading(&self) -> bool {
        matches!(self.load, Some(LoadStatus::Started | LoadStatus::HeadParsed))
    }
}

/// A browser window.
struct Browser {
    embed: EmbedId,
    surface: Option<Surface>,
    scale: f32,
    tabs: Vec<Tab>,
    active: usize,
    /// The location field's text, and whether you're typing in it (then
    /// page navigation leaves it alone).
    location: String,
    editing: bool,
    focus_location: bool,
    /// Where the page is and how big, in device pixels.
    viewport: Option<Viewport>,
    size: dpi::PhysicalSize<u32>,
    pointer: Pointer,
    /// The page is the window's keyboard target (Servo's focus).
    page_focus: bool,
    title: String,
    menu: Option<ContextMenu>,
    select: Option<SelectElement>,
    /// A popup to open on the next frame (we need the egui context).
    popup: Option<(MenuAnchor, Menu, Option<usize>)>,
    /// The key button, and the logins menu to open under it.
    key: Rect,
    logins_menu: Option<Menu>,
}

impl Browser {
    fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }
    fn view(&self) -> Option<&WebView> {
        self.tab()?.view.as_ref()
    }
}

/// Windows that belong to a browser window.
enum Sheet {
    Dialog { owner: WindowId, dialog: SimpleDialog, text: String },
    Login { owner: WindowId, request: AuthenticationRequest, user: String, password: String, site: String },
    Files { picker: FilePicker, panel: FilePanel },
    /// `<input type=color>`.
    Color { owner: WindowId, picker: ColorPicker, rgb: [u8; 3] },
}

enum Win {
    Browser(Box<Browser>),
    Sheet(Box<Sheet>),
    /// The Find window for a browser window.
    Find { target: WindowId },
    Downloads,
}

struct NetPositive {
    engine: Engine,
    windows: BTreeMap<WindowId, Win>,
    settings: Settings,
    /// The browser window last used, for launches and new tabs from outside.
    front: Option<WindowId>,
    pasted: bool,
    proxy: Proxy,
    search: find::Search,
    downloads: Vec<Download>,
    next_download: u64,
    /// `blob:` downloads arriving from pages in pieces.
    blobs: BTreeMap<(WebViewId, u64), (String, Vec<u8>)>,
    /// The desktop is dark (or night): pages are told so.
    dark: bool,
    /// Which process each tab's page is in and whether it answers, and
    /// the thread keeping them in check (guard.rs).
    pings: guard::Pings,
    watch: guard::Watch,
    /// Cameras, microphones and calls in pages.
    rtc: rtc::Rtc,
}

impl NetPositive {
    fn theme(&self) -> Theme {
        if self.dark { Theme::Dark } else { Theme::Light }
    }

    fn open_browser(&mut self, cx: &mut Cx, urls: Vec<String>, active: usize, size: (u32, u32)) -> WindowId {
        let win = cx.open_window(WindowOptions::new(NAME, size.0, size.1).min_size(400, 240));
        let embed = cx.embed(win);
        let blank = urls.is_empty() || urls.iter().all(|u| u == "about:blank");
        let tabs: Vec<Tab> = if urls.is_empty() { vec![Tab::new(self.settings.home.clone())] } else { urls.into_iter().map(Tab::new).collect() };
        let active = active.min(tabs.len() - 1);
        let location = location::display(&tabs[active].url);
        self.windows.insert(
            win,
            Win::Browser(Box::new(Browser {
                embed,
                surface: None,
                scale: 2.0,
                tabs,
                active,
                location,
                editing: false,
                focus_location: blank,
                viewport: None,
                size: dpi::PhysicalSize::new(size.0 * 2, size.1 * 2),
                pointer: Pointer::default(),
                page_focus: false,
                title: NAME.into(),
                menu: None,
                select: None,
                popup: None,
                key: Rect::NOTHING,
                logins_menu: None,
            })),
        );
        self.front = Some(win);
        win
    }

    fn browser(&mut self, win: WindowId) -> Option<&mut Browser> {
        match self.windows.get_mut(&win) {
            Some(Win::Browser(b)) => Some(b),
            _ => None,
        }
    }

    fn find(&self, id: WebViewId) -> Option<(WindowId, usize)> {
        self.windows.iter().find_map(|(&win, w)| match w {
            Win::Browser(b) => b.tabs.iter().position(|t| t.view.as_ref().is_some_and(|v| v.id() == id)).map(|i| (win, i)),
            _ => None,
        })
    }

    fn new_view(&self, b: &Browser, url: Option<Url>) -> Option<WebView> {
        let surface = b.surface.as_ref()?;
        let mut builder = WebViewBuilder::new(&self.engine.servo, surface.context.clone())
            .delegate(self.engine.events.clone())
            .clipboard_delegate(self.engine.events.clone())
            .user_content_manager(self.engine.content.clone())
            .hidpi_scale_factor(Scale::new(b.scale));
        if let Some(url) = url {
            builder = builder.url(url);
        }
        let view = builder.build();
        self.prepare(&view, b);
        Some(view)
    }

    fn prepare(&self, view: &WebView, b: &Browser) {
        view.resize(b.size);
        // Pages that support dark go dark with the desktop (scheme.rs
        // keeps the rest black on white).
        view.notify_theme_change(self.theme());
        view.hide();
    }

    /// Give every tab without a view one (a new surface, or a restore).
    fn realize(&mut self, win: WindowId) {
        let Some(Win::Browser(b)) = self.windows.get(&win) else { return };
        let wanted: Vec<(usize, Option<Url>)> =
            b.tabs.iter().enumerate().filter(|(_, t)| t.view.is_none()).map(|(i, t)| (i, Url::parse(&t.url).ok())).collect();
        let views: Vec<(usize, Option<WebView>)> = wanted.into_iter().map(|(i, url)| (i, self.new_view(b, url))).collect();
        let Some(Win::Browser(b)) = self.windows.get_mut(&win) else { return };
        for (i, v) in views {
            b.tabs[i].view = v;
        }
        show_active(b);
    }

    fn load(&mut self, cx: &mut Cx, win: WindowId, url: Url) {
        let Some(b) = self.browser(win) else { return };
        b.editing = false;
        b.location = location::display(url.as_str());
        let Some(tab) = b.tabs.get_mut(b.active) else { return };
        // A tab that never left its blank page starts afresh, so the
        // blank page isn't in its history.
        if tab.url == "about:blank" && !tab.can_back {
            tab.view = None;
        }
        tab.url = url.to_string();
        tab.stopped = None;
        match &tab.view {
            Some(v) => v.load(url),
            None => self.realize(win),
        }
        cx.redraw(win);
    }

    fn new_tab(&mut self, cx: &mut Cx, win: WindowId, url: Option<String>, select: bool) {
        let url = url.unwrap_or_else(|| self.settings.home.clone());
        let Some(Win::Browser(b)) = self.windows.get(&win) else { return };
        let view = self.new_view(b, Url::parse(&url).ok());
        let Some(b) = self.browser(win) else { return };
        let mut tab = Tab::new(url.clone());
        tab.view = view;
        b.tabs.insert(b.active + 1, tab);
        if select {
            b.active += 1;
            b.location = location::display(&url);
            b.editing = false;
            b.focus_location = url == "about:blank";
        }
        show_active(b);
        cx.redraw(win);
    }

    fn select_tab(&mut self, cx: &mut Cx, win: WindowId, i: usize) {
        let Some(b) = self.browser(win) else { return };
        if i >= b.tabs.len() || i == b.active {
            return;
        }
        b.active = i;
        b.location = location::display(&b.tabs[i].url);
        b.editing = false;
        show_active(b);
        cx.redraw(win);
    }

    fn close_tab(&mut self, cx: &mut Cx, win: WindowId, i: usize) {
        let Some(b) = self.browser(win) else { return };
        if b.tabs.len() <= 1 {
            return self.close_browser(cx, win);
        }
        if let Some(v) = &b.tabs[i].view {
            rtc::forget_view(v.id());
        }
        b.tabs.remove(i);
        if b.active > i || b.active == b.tabs.len() {
            b.active = b.active.saturating_sub(1);
        }
        b.location = location::display(&b.tabs[b.active].url);
        b.editing = false;
        show_active(b);
        cx.redraw(win);
    }

    fn close_browser(&mut self, cx: &mut Cx, win: WindowId) {
        // The views go before their GL context; then the window and its
        // surface (`embed_lost` finds nothing left to drop).
        if let Some(Win::Browser(mut b)) = self.windows.remove(&win) {
            b.tabs.iter().filter_map(|t| t.view.as_ref()).for_each(|v| rtc::forget_view(v.id()));
            b.tabs.clear();
            self.drop_surface(b.surface.take());
        }
        let sheets: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| match w {
                Win::Sheet(s) => sheet_owner(s) == Some(win),
                Win::Find { target } => *target == win,
                Win::Browser(_) | Win::Downloads => false,
            })
            .map(|(&id, _)| id)
            .collect();
        for s in sheets {
            self.windows.remove(&s);
            cx.close_window(s);
        }
        cx.close_window(win);
        if self.front == Some(win) {
            self.front = self.windows.iter().find(|(_, w)| matches!(w, Win::Browser(_))).map(|(&id, _)| id);
        }
        if !self.windows.values().any(|w| matches!(w, Win::Browser(_))) {
            self.save_settings();
            cx.quit();
        }
    }

    fn drop_surface(&mut self, surface: Option<Surface>) {
        let Some(surface) = surface else { return };
        let others = self.windows.values().any(|w| matches!(w, Win::Browser(b) if b.surface.is_some()));
        if others { drop(surface) } else { surface.terminate() }
    }

    fn save_settings(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.settings) {
            let path = settings_path();
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    fn remember(&mut self, url: &str, title: &str) {
        if url == "about:blank" || url.starts_with("data:") {
            return;
        }
        let h = &mut self.settings.history;
        h.retain(|e| e.url != url);
        h.push(Bookmark { title: title.to_string(), url: url.to_string() });
        if h.len() > HISTORY_MAX {
            h.drain(..h.len() - HISTORY_MAX);
        }
    }

    /// Run Servo and act on what the views said.
    fn pump(&mut self, cx: &mut Cx) {
        let events = self.engine.pump();
        let mut painted = Vec::new();
        for (id, event) in events {
            let Some((win, i)) = self.find(id) else {
                if let Event::NewWebView(_) = event {
                    // Its opener has gone; the request drops (denied).
                }
                continue;
            };
            let is_active = matches!(self.windows.get(&win), Some(Win::Browser(b)) if b.active == i);
            match event {
                Event::Frame => {
                    if is_active && !painted.contains(&win) {
                        painted.push(win);
                    }
                    continue;
                }
                Event::NewWebView(request) => {
                    let Some(Win::Browser(b)) = self.windows.get(&win) else { continue };
                    let Some(surface) = &b.surface else { continue };
                    let view = request
                        .builder(surface.context.clone())
                        .delegate(self.engine.events.clone())
                        .clipboard_delegate(self.engine.events.clone())
                        .user_content_manager(self.engine.content.clone())
                        .hidpi_scale_factor(Scale::new(b.scale))
                        .build();
                    self.prepare(&view, b);
                    let Some(b) = self.browser(win) else { continue };
                    let mut tab = Tab::new("about:blank");
                    tab.view = Some(view);
                    b.tabs.insert(i + 1, tab);
                    b.active = i + 1;
                    b.location.clear();
                    b.editing = false;
                    show_active(b);
                }
                Event::Control(control) => self.control(cx, win, i, control),
                Event::LoginUsed { username, password } => {
                    let Some(Win::Browser(b)) = self.windows.get(&win) else { continue };
                    let url = b.tabs[i].url.clone();
                    let Some(view) = b.tabs[i].view.as_ref().map(|v| v.id()) else { continue };
                    if !logins::web(&url) {
                        continue;
                    }
                    let offer =
                        Offer { url: url.clone(), username, password, kind: OfferKind::New, busy: false, error: None };
                    let request = Request::Check {
                        url: url.clone(),
                        username: offer.username.clone(),
                        password: offer.password.clone(),
                    };
                    logins::ask(self.proxy.clone(), win, view, url, Purpose::Check(Box::new(offer)), request, false);
                }
                Event::Passkey(request) => self.passkey_request(cx, win, i, request),
                Event::Rtc(request) => {
                    let Some(b) = self.browser(win) else { continue };
                    let Some(view) = b.tabs[i].view.clone() else { continue };
                    if let Some(ask) = self.rtc.request(&view, request)
                        && let Some(b) = self.browser(win)
                    {
                        b.tabs[i].media.push_back(ask);
                        cx.redraw(win);
                    }
                }
                Event::Download(download) => self.page_download(cx, win, i, id, download),
                Event::Found(found) => {
                    let missing = format!("“{}” isn’t on this page.", self.search.text);
                    if !found && let Some(b) = self.browser(win) {
                        b.tabs[i].status = Some(missing);
                    }
                    cx.redraw(win);
                }
                Event::Filled(result) => {
                    let Some(b) = self.browser(win) else { continue };
                    match result.as_str() {
                        "none" => b.tabs[i].status = Some("There’s no login form on this page.".into()),
                        "moved" => b.tabs[i].status = Some("The page changed before the login could be filled.".into()),
                        _ => {}
                    }
                }
                Event::Copy(text) => cx.copy(text),
                Event::Permission(request) => {
                    // No prompts for cameras, locations or notifications yet.
                    tracing::info!("denied {:?}", request.feature());
                    request.deny();
                }
                Event::Authenticate(request) => {
                    let site = request.url().host_str().unwrap_or("this site").to_string();
                    let message = format!("{site} asks you to log in.");
                    let mut o = WindowOptions::alert("Log In", &message).parent(win);
                    o.size.1 += 2 * (m::CONTROL_HEIGHT as u32 + m::TIGHT as u32);
                    o.min_size = Some(o.size);
                    let id = cx.open_window(o);
                    self.windows.insert(
                        id,
                        Win::Sheet(Box::new(Sheet::Login { owner: win, request, user: String::new(), password: String::new(), site })),
                    );
                }
                event => {
                    let mut visited = None;
                    let Some(b) = self.browser(win) else { continue };
                    let tab = &mut b.tabs[i];
                    match event {
                        Event::Url(url) => {
                            tab.url = url.to_string();
                            if is_active && !b.editing {
                                b.location = location::display(url.as_str());
                            }
                        }
                        Event::Title(title) => tab.title = title.unwrap_or_default(),
                        Event::Status(status) => tab.status = status,
                        Event::Load(status) => {
                            if status == LoadStatus::Started {
                                // The page asking is gone, and its devices go.
                                tab.passkey = None;
                                tab.media.clear();
                                if let Some(v) = &tab.view {
                                    rtc::forget_view(v.id());
                                }
                                tab.hung = false;
                                tab.scroll = None;
                                tab.scrolled = None;
                                tab.favicon_seen = false;
                            }
                            if status == LoadStatus::Complete {
                                if !tab.favicon_seen {
                                    tab.favicon = None;
                                }
                                visited = Some((tab.url.clone(), tab.title.clone()));
                            }
                            tab.load = Some(status);
                        }
                        Event::Cursor(cursor) => tab.cursor = cursor,
                        Event::Favicon => {
                            tab.new_favicon = tab.view.as_ref().and_then(|v| v.favicon().and_then(|i| favicon::square(&i)));
                            tab.favicon_seen = tab.new_favicon.is_some();
                        }
                        Event::Scroll(metrics) => {
                            if tab.scroll == Some(metrics) {
                                continue;
                            }
                            tab.scroll = Some(metrics);
                        }
                        Event::History { can_back, can_forward } => {
                            tab.can_back = can_back;
                            tab.can_forward = can_forward;
                        }
                        Event::Closed => {
                            // window.close() from the page.
                            self.close_tab(cx, win, i);
                            continue;
                        }
                        Event::Crashed(reason) => tracing::warn!("tab crashed: {reason}"),
                        Event::HideControl(id) => {
                            if b.menu.as_ref().is_some_and(|m| m.id() == id) {
                                b.menu = None;
                            }
                            if b.select.as_ref().is_some_and(|s| s.id() == id) {
                                b.select = None;
                            }
                        }
                        _ => {}
                    }
                    if let Some((url, title)) = visited {
                        self.remember(&url, &title);
                    }
                }
            }
            cx.redraw(win);
        }
        for win in painted {
            if let Some(Win::Browser(b)) = self.windows.get(&win)
                && let (Some(view), Some(surface)) = (b.view(), &b.surface)
            {
                view.paint();
                use servo::RenderingContext;
                surface.context.present();
            }
        }
    }

    /// Menus, pickers and dialogs a page asks for.
    fn control(&mut self, cx: &mut Cx, win: WindowId, _tab: usize, control: EmbedderControl) {
        let Some(b) = self.browser(win) else { return };
        let Some(view) = b.viewport else { return };
        // Controls are placed in CSS pixels: logical ones times the zoom.
        let zoom = b.tab().and_then(|t| t.view.as_ref()).map(|v| v.page_zoom() * v.pinch_zoom()).unwrap_or(1.0);
        let page_rect = |r: servo::DeviceIntRect| {
            Rect::from_min_max(
                view.rect.min + vec2(r.min.x as f32, r.min.y as f32) * zoom,
                view.rect.min + vec2(r.max.x as f32, r.max.y as f32) * zoom,
            )
        };
        match control {
            EmbedderControl::ContextMenu(menu) => {
                let pos = page_rect(menu.position()).min;
                let mut items = Menu::new("");
                let mut sep = false;
                for (n, item) in menu.items().iter().enumerate() {
                    match item {
                        ContextMenuItem::Item { label, enabled, action } => {
                            if sep && !items.items.is_empty() {
                                items = items.separator();
                            }
                            sep = false;
                            // Servo's words for its own shell; ours for tabs.
                            let label = match action {
                                ContextMenuAction::OpenLinkInNewWebView => "Open Link in New Tab",
                                ContextMenuAction::OpenImageInNewView => "Open Image in New Tab",
                                ContextMenuAction::CopyImageLink => "Copy Image Address",
                                _ => label.as_str(),
                            };
                            items = items.item(label, format!("{n}"), None).enabled(*enabled);
                            // Ours, after Servo's Copy for the link or image.
                            match action {
                                ContextMenuAction::CopyLink => items = items.item("Download Linked File", "dl-link", None),
                                ContextMenuAction::CopyImageLink => items = items.item("Download Image", "dl-image", None),
                                _ => {}
                            }
                        }
                        ContextMenuItem::Separator => sep = true,
                    }
                }
                b.popup = Some((MenuAnchor::At(pos), items, None));
                b.menu = Some(menu);
            }
            EmbedderControl::SelectElement(select) => {
                let rect = page_rect(select.position());
                let mut items = Menu::new("");
                let selected = select.selected_options();
                let add = |items: Menu, o: &servo::SelectElementOption| {
                    items.item(&o.label, format!("{}", o.id), None).enabled(!o.is_disabled).checked(selected.contains(&o.id))
                };
                for entry in select.options() {
                    match entry {
                        SelectElementOptionOrOptgroup::Option(o) => items = add(items, o),
                        SelectElementOptionOrOptgroup::Optgroup { label, options } => {
                            if !items.items.is_empty() {
                                items = items.separator();
                            }
                            items = items.item(label, "", None).enabled(false);
                            for o in options {
                                items = add(items, o);
                            }
                        }
                    }
                }
                // The chosen option lies over the field, as in a BeOS menu field.
                let current = selected.first().and_then(|id| items.items.iter().position(|i| matches!(i, MenuItem::Action(a) if a.command == id.to_string())));
                b.popup = Some((MenuAnchor::Over(rect, current.unwrap_or(0)), items, current));
                b.select = Some(select);
            }
            EmbedderControl::SimpleDialog(dialog) => {
                let host = b.tab().and_then(|t| Url::parse(&t.url).ok()).and_then(|u| u.host_str().map(str::to_string));
                let title = host.unwrap_or_else(|| NAME.into());
                let (text, prompt) = match &dialog {
                    SimpleDialog::Prompt(p) => (p.current_value().to_string(), true),
                    _ => (String::new(), false),
                };
                let mut o = WindowOptions::alert(title, dialog.message()).parent(win);
                if prompt {
                    o.size.1 += m::CONTROL_HEIGHT as u32 + m::SPACING as u32;
                    o.min_size = Some(o.size);
                }
                let id = cx.open_window(o);
                self.windows.insert(id, Win::Sheet(Box::new(Sheet::Dialog { owner: win, dialog, text })));
            }
            EmbedderControl::FilePicker(picker) => {
                let dir = picker
                    .current_paths()
                    .first()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                    .unwrap_or_else(|| "/".into());
                let types: Vec<String> = picker.filter_patterns().iter().map(|f| f.0.clone()).collect();
                let types: Vec<&str> = types.iter().map(String::as_str).collect();
                let mut panel = FilePanel::open(dir);
                if !types.is_empty() {
                    panel = panel.types(&types);
                }
                let id = cx.open_window(WindowOptions::new("Choose a File", PANEL_SIZE.0, PANEL_SIZE.1).min_size(400, 300).parent(win));
                self.windows.insert(id, Win::Sheet(Box::new(Sheet::Files { picker, panel })));
            }
            EmbedderControl::ColorPicker(picker) => {
                let rgb = picker.current_color().map(|c| [c.red, c.green, c.blue]).unwrap_or_default();
                let w = (2.0 * m::WINDOW_INSET + COLOR_W).round() as u32;
                let h = (2.0 * m::WINDOW_INSET + COLOR_RAMPS_H + m::SPACING + m::CONTROL_HEIGHT).round() as u32;
                let id = cx.open_window(WindowOptions::new("Choose a Colour", w, h).min_size(w, h).max_size(w, h).parent(win));
                self.windows.insert(id, Win::Sheet(Box::new(Sheet::Color { owner: win, picker, rgb })));
            }
            EmbedderControl::InputMethod(_) => {}
        }
    }

    fn command_for(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        let home = self.settings.home.clone();
        match command {
            "new-window" => {
                let size = cx.window_size(win).unwrap_or_else(|| default_size(cx));
                self.open_browser(cx, Vec::new(), 0, size);
            }
            "new-tab" => self.new_tab(cx, win, None, true),
            "close-tab" => {
                if let Some(i) = self.browser(win).map(|b| b.active) {
                    self.close_tab(cx, win, i);
                }
            }
            "close-window" => self.close_browser(cx, win),
            "quit" => {
                self.save_settings();
                cx.quit();
            }
            "open-location" => {
                if let Some(b) = self.browser(win) {
                    b.focus_location = true;
                }
                cx.redraw(win);
            }
            "home" => {
                if let Ok(url) = Url::parse(&home) {
                    self.load(cx, win, url);
                }
            }
            "set-home" => {
                if let Some(url) = self.browser(win).and_then(|b| b.tab().map(|t| t.url.clone())) {
                    self.settings.home = url;
                    self.save_settings();
                }
            }
            "add-bookmark" => {
                if let Some((url, title)) = self.browser(win).and_then(|b| b.tab().map(|t| (t.url.clone(), t.label()))) {
                    if self.settings.bookmarks.iter().any(|m| m.url == url) {
                        self.settings.bookmarks.retain(|m| m.url != url);
                    } else {
                        self.settings.bookmarks.push(Bookmark { title, url });
                    }
                    self.save_settings();
                }
            }
            "find" => self.open_find(cx, win),
            "downloads" => self.show_downloads(cx),
            "find-again" => {
                if self.search.text.is_empty() {
                    self.open_find(cx, win);
                } else {
                    self.find_in(win);
                }
            }
            "clear-history" => {
                self.settings.history.clear();
                self.save_settings();
            }
            "logins" => self.ask_logins(win, Purpose::Menu, |url| Request::Logins { url }),
            "unlock-passwords" => self.ask_logins(win, Purpose::Unlock, |_| Request::Unlock),
            "open-passwords" => {
                if let Some(program) = tabula_xdg::find_program("tabula-passwords")
                    && let Err(e) = tabula_xdg::open::spawn(&[program.into_os_string()])
                {
                    tracing::warn!("couldn't start Passwords: {e}");
                }
            }
            "save-login" => {
                let Some(b) = self.browser(win) else { return };
                let Some(tab) = b.tabs.get_mut(b.active) else { return };
                let (Some(offer), Some(view)) = (&mut tab.offer, &tab.view) else { return };
                offer.busy = true;
                offer.error = None;
                let (view, url, request) = (view.id(), offer.url.clone(), offer.request());
                logins::ask(self.proxy.clone(), win, view, url, Purpose::Save, request, true);
                cx.redraw(win);
            }
            "passkey-ok" => self.passkey_confirm(cx, win),
            "passkey-cancel" => {
                let token = self.engine.token.clone();
                if let Some(b) = self.browser(win)
                    && let Some(tab) = b.tabs.get_mut(b.active)
                    && let Some(ask) = tab.passkey.take()
                    && let Some(view) = &tab.view
                {
                    passkeys::settle(view, &token, ask.id, Err(("NotAllowedError", "The request was cancelled.")));
                }
                cx.redraw(win);
            }
            "stop-page" => {
                let Some(b) = self.browser(win) else { return };
                let Some(tab) = b.tabs.get_mut(b.active) else { return };
                let Some(view) = tab.view.clone() else { return };
                tab.hung = false;
                if let Some(pid) = self.pings.pid(view.id()) {
                    self.watch.kill(pid);
                }
                cx.redraw(win);
            }
            "wait-page" => {
                let Some(b) = self.browser(win) else { return };
                let Some(tab) = b.tabs.get_mut(b.active) else { return };
                tab.hung = false;
                if let Some(id) = tab.view.as_ref().map(|v| v.id()) {
                    self.pings.wait(id);
                }
                cx.redraw(win);
            }
            "media-allow" | "media-block" => {
                if let Some(b) = self.browser(win)
                    && let Some(tab) = b.tabs.get_mut(b.active)
                    && let Some(view) = tab.view.clone()
                    && let Some(ask) = tab.media.pop_front()
                {
                    // Asks after it that this answers go too.
                    let allow = command == "media-allow";
                    let covered = |a: &rtc::MediaAsk| a.origin == ask.origin && (a.video.is_none() || ask.video.is_some()) && (a.audio.is_none() || ask.audio.is_some());
                    let (same, rest): (VecDeque<_>, VecDeque<_>) = tab.media.drain(..).partition(covered);
                    tab.media = rest;
                    self.rtc.decide(&view, ask, allow);
                    for a in same {
                        self.rtc.decide(&view, a, allow);
                    }
                }
                cx.redraw(win);
            }
            "dismiss-stopped" => {
                if let Some(b) = self.browser(win)
                    && let Some(tab) = b.tabs.get_mut(b.active)
                {
                    tab.stopped = None;
                }
                cx.redraw(win);
            }
            "dismiss-login" => {
                if let Some(b) = self.browser(win)
                    && let Some(tab) = b.tabs.get_mut(b.active)
                {
                    tab.offer = None;
                }
                cx.redraw(win);
            }
            _ => {
                if let Some(url) = command.strip_prefix("go:") {
                    if let Ok(url) = Url::parse(url) {
                        self.load(cx, win, url);
                    }
                    return;
                }
                if let Some(n) = command.strip_prefix("tab:").and_then(|n| n.parse::<usize>().ok()) {
                    return self.select_tab(cx, win, n);
                }
                if let Some(url) = command.strip_prefix("download:") {
                    let referer = self.browser(win).and_then(|b| b.tab()).map(|t| t.url.clone());
                    return self.start_download(cx, url.to_string(), None, referer);
                }
                if let Some((vault, id)) = command.strip_prefix("fill:").and_then(|r| r.split_once('/')) {
                    let (vault, id) = (vault.to_string(), id.to_string());
                    return self.ask_logins(win, Purpose::Fill, |url| Request::Fill { url, vault, id });
                }
                self.view_command(cx, win, command);
            }
        }
    }

    /// Commands for the page itself.
    fn view_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) {
        let pasted = std::mem::take(&mut self.pasted);
        let Some(b) = self.browser(win) else { return };
        let Some(tab) = b.tabs.get_mut(b.active) else { return };
        let Some(view) = &tab.view else { return };
        match command {
            "back" => {
                tab.stopped = None;
                view.go_back(1);
            }
            "forward" => {
                tab.stopped = None;
                view.go_forward(1);
            }
            "reload" => {
                tab.stopped = None;
                view.reload();
            }
            "stop" => {
                // Servo has no stop: going nowhere ends the load.
                if tab.loading()
                    && let Some(url) = view.url()
                {
                    view.load(url);
                }
            }
            "zoom-in" | "zoom-out" | "zoom-reset" => {
                let z = view.page_zoom();
                let next = match command {
                    "zoom-in" => ZOOMS.iter().copied().find(|&s| s > z + 0.01).unwrap_or(z),
                    "zoom-out" => ZOOMS.iter().rev().copied().find(|&s| s < z - 0.01).unwrap_or(z),
                    _ => 1.0,
                };
                view.set_page_zoom(next);
                tab.zoom = next;
            }
            "copy" => {
                view.notify_input_event(servo::InputEvent::EditingAction(servo::ClipboardAction::Copy));
            }
            "cut" => {
                view.notify_input_event(servo::InputEvent::EditingAction(servo::ClipboardAction::Cut));
            }
            "paste" if !pasted => cx.paste(win),
            "select-all" | "undo" | "redo" => {
                let (key, code, shift) = match command {
                    "select-all" => ("a", servo::Code::KeyA, false),
                    "undo" => ("z", servo::Code::KeyZ, false),
                    _ => ("z", servo::Code::KeyZ, true),
                };
                let mut modifiers = servo::Modifiers::CONTROL;
                if shift {
                    modifiers |= servo::Modifiers::SHIFT;
                }
                for state in [servo::KeyState::Down, servo::KeyState::Up] {
                    view.notify_input_event(servo::InputEvent::Keyboard(servo::KeyboardEvent::new(
                        keyboard_types::KeyboardEvent {
                            state,
                            key: servo::Key::Character(key.into()),
                            code,
                            location: servo::Location::Standard,
                            modifiers,
                            repeat: false,
                            is_composing: false,
                        },
                    )));
                }
            }
            _ => {}
        }
        cx.redraw(win);
    }

    fn browser_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let scale = ui.ctx().pixels_per_point();
        let full = ui.max_rect();
        let mut commands: Vec<String> = Vec::new();
        let mut navigate: Option<Url> = None;
        let content_id = Id::unique("page");
        let Some(Win::Browser(b)) = self.windows.get_mut(&win) else { return };

        // Toolbar.
        let bar = Rect::from_min_size(full.min, vec2(full.width(), m::TOOLBAR_HEIGHT));
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, c(p.panel_background));
        style::hline(&painter, bar.left(), bar.right(), bar.bottom() - 1.0, shade(&p, p.panel_background, 1.3));
        let (can_back, can_forward, loading) =
            b.tab().map(|t| (t.can_back, t.can_forward, t.loading())).unwrap_or_default();
        let mut x = bar.left() + m::SPACING;
        let y = bar.top() + m::TIGHT;
        for (glyph, enabled, command) in [
            (Glyph::Back, can_back, "back"),
            (Glyph::Forward, can_forward, "forward"),
            (if loading { Glyph::Stop } else { Glyph::Reload }, true, if loading { "stop" } else { "reload" }),
            (Glyph::Home, true, "home"),
        ] {
            let r = Rect::from_min_size(pos2(x, y), vec2(TOOL, m::CONTROL_HEIGHT));
            x += TOOL + m::TIGHT;
            let clicked = ui.add_enabled_ui(enabled, |ui| ui.put(r, Button::new("").min_width(TOOL))).inner.clicked();
            let color = if enabled { c(p.control_text) } else { style::disabled_text(&p) };
            paint_glyph(&painter, r, glyph, color);
            if clicked {
                commands.push(command.into());
            }
        }
        // The key: the page's logins, from Passwords.
        let web = b.tab().is_some_and(|t| logins::web(&t.url));
        let r = Rect::from_min_size(pos2(x, y), vec2(TOOL, m::CONTROL_HEIGHT));
        x += TOOL + m::TIGHT;
        b.key = r;
        let key = ui.add_enabled_ui(web, |ui| ui.put(r, Button::new("").min_width(TOOL))).inner;
        let icon = Rect::from_center_size(r.center(), vec2(16.0, 16.0));
        if web {
            icons::paint(&painter, icon, "key");
        } else {
            icons::paint_tinted(&painter, icon, "key", egui::Color32::from_white_alpha(90));
        }
        if key.on_hover_text("Fill in a login from Passwords").clicked() {
            commands.push("logins".into());
        }
        x += m::SPACING - m::TIGHT;
        let field = Rect::from_min_max(pos2(x, y), pos2(bar.right() - m::SPACING, y + m::CONTROL_HEIGHT));
        let response = ui.put(field, TextControl::new(&mut b.location).hint("Search or enter address"));
        if std::mem::take(&mut b.focus_location) {
            response.request_focus();
            select_all(ui.ctx(), response.id, &b.location);
        }
        if response.has_focus() {
            b.editing = true;
        }
        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                let cwd = std::env::current_dir().ok();
                navigate = location::parse(&b.location, cwd.as_deref());
                if navigate.is_some() {
                    // The page takes the keyboard, as in every browser.
                    ui.memory_mut(|m| m.request_focus(content_id));
                }
            } else if b.editing && ui.input(|i| i.key_pressed(Key::Escape)) || !b.editing {
                b.location = b.tab().map(|t| location::display(&t.url)).unwrap_or_default();
            }
            b.editing = false;
        }

        // Tabs.
        let mut top = bar.bottom();
        if b.tabs.len() > 1 {
            for (i, t) in b.tabs.iter_mut().enumerate() {
                if let Some(image) = t.new_favicon.take() {
                    t.favicon = Some(ui.ctx().load_texture(format!("favicon-{i}-{:?}", b.embed), image, egui::TextureOptions::LINEAR));
                }
            }
            let strip = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), TAB_HEIGHT));
            if let Some(cmd) = tab_strip(ui, strip, b) {
                commands.push(cmd);
            }
            top = strip.bottom();
        }

        // Bars about a stuck or stopped page, the page's passkey request
        // and a login just used.
        if let Some(tab) = b.tabs.get_mut(b.active) {
            if tab.hung {
                let text = format!("“{}” isn’t responding.", truncate(&tab.label(), 60));
                let buttons = vec![("Wait", true, "wait-page"), ("Stop Page", true, "stop-page")];
                let bar = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), OFFER_HEIGHT));
                notice(ui, &painter, bar, Notice { icon: "warning", text, error: false, choice: None, buttons }, &mut commands);
                top = bar.bottom();
            } else if let Some(text) = &tab.stopped {
                let buttons = vec![("OK", true, "dismiss-stopped")];
                let bar = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), OFFER_HEIGHT));
                notice(ui, &painter, bar, Notice { icon: "stop", text: text.clone(), error: false, choice: None, buttons }, &mut commands);
                top = bar.bottom();
            }
            if let Some(ask) = tab.media.front() {
                let buttons = vec![("Allow", true, "media-allow"), ("Block", true, "media-block")];
                let bar = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), OFFER_HEIGHT));
                let icon = if ask.video.is_some() { "camera" } else { "microphone" };
                notice(ui, &painter, bar, Notice { icon, text: ask.message(), error: false, choice: None, buttons }, &mut commands);
                top = bar.bottom();
            }
            if let Some(ask) = &mut tab.passkey {
                let busy = ask.busy;
                let (text, error) = match &ask.error {
                    Some(e) => (e.clone(), true),
                    None => (ask.message(), false),
                };
                let cancel = ("Cancel", true, "passkey-cancel");
                let (buttons, choice) = match &mut ask.kind {
                    AskKind::Create { .. } => (vec![(if busy { "Saving…" } else { "Save Passkey" }, !busy, "passkey-ok"), cancel], None),
                    AskKind::Get { passkeys: None, .. } => (vec![cancel], None),
                    AskKind::Get { passkeys: Some(list), .. } if list.is_empty() => (vec![("OK", true, "passkey-cancel")], None),
                    AskKind::Get { passkeys: Some(list), chosen, .. } => {
                        let labels: Vec<String> = list.iter().map(passkeys::label).collect();
                        let sign_in = (if busy { "Signing In…" } else { "Sign In" }, !busy, "passkey-ok");
                        (vec![sign_in, cancel], (labels.len() > 1).then_some((chosen, labels)))
                    }
                };
                let bar = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), OFFER_HEIGHT));
                notice(ui, &painter, bar, Notice { icon: "key", text, error, choice, buttons }, &mut commands);
                top = bar.bottom();
            }
            if let Some(offer) = &tab.offer {
                let save = match (&offer.kind, offer.busy) {
                    (_, true) => "Saving…",
                    (OfferKind::New, _) => "Save",
                    (OfferKind::Update { .. }, _) => "Update",
                };
                let (text, error) = match &offer.error {
                    Some(e) => (e.clone(), true),
                    None => (offer.message(), false),
                };
                let buttons = vec![(save, !offer.busy, "save-login"), ("Not Now", true, "dismiss-login")];
                let bar = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), OFFER_HEIGHT));
                notice(ui, &painter, bar, Notice { icon: "key", text, error, choice: None, buttons }, &mut commands);
                top = bar.bottom();
            }
        }

        // Status line.
        let status = Rect::from_min_max(pos2(full.left(), full.bottom() - STATUS_HEIGHT), full.max);
        style::hline(&painter, status.left(), status.right(), status.top(), c(p.shine));
        style::hline(&painter, status.left(), status.right(), status.top() - 1.0, shade(&p, p.panel_background, 1.3));
        if let Some(t) = b.tab() {
            let text = match (&t.status, &t.load) {
                (Some(s), _) if !s.is_empty() => s.clone(),
                (_, Some(LoadStatus::Started)) => "Contacting…".into(),
                (_, Some(LoadStatus::HeadParsed)) => "Loading…".into(),
                _ if (t.zoom - 1.0).abs() > 0.01 => format!("Zoom {:.0}%", t.zoom * 100.0),
                _ => String::new(),
            };
            let right = if t.loading() {
                let w = 100.0;
                let r = Rect::from_min_size(pos2(status.right() - m::SPACING - w, status.center().y - 4.0), vec2(w, 8.0));
                let r = style::snap(ui.ctx(), r);
                style::sunken(&painter, r, &p, false);
                let f = if t.load == Some(LoadStatus::Started) { 0.15 } else { 0.6 };
                let fill = Rect::from_min_size(r.min + vec2(1.0, 1.0), vec2((r.width() - 2.0) * f, r.height() - 2.0));
                painter.rect_filled(style::snap(ui.ctx(), fill), 0.0, c(p.status_bar));
                r.left() - m::SPACING
            } else {
                status.right() - m::SPACING
            };
            let clip = Rect::from_min_max(pos2(status.left() + m::SPACING, status.top()), pos2(right, status.bottom()));
            style::label(&painter.with_clip_rect(clip), clip.left(), status.center().y + 0.5, Align::Min, text, style::plain_font(), c(p.panel_text));
        }

        // The page, with BeOS scroll bars (Servo draws none): always the
        // vertical one, the horizontal one when the page is too wide.
        let shown = b.tab().and_then(Tab::scroll);
        let wide = shown.is_some_and(|m| m.wide());
        let area = style::snap(ui.ctx(), Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), status.top() - 1.0)));
        let content = Rect::from_min_max(area.min, area.max - vec2(widgets::SCROLL_BAR, if wide { widgets::SCROLL_BAR } else { 0.0 }));
        let mut moved = shown.unwrap_or_default();
        let (mut changed, m0) = (false, moved);
        let vbar = Rect::from_min_max(pos2(content.right(), area.top()), pos2(area.right(), content.bottom()));
        changed |= ScrollBar::vertical(("page-v", b.embed)).show(ui, vbar, &mut moved.y, m0.height, m0.view_height);
        if wide {
            let hbar = Rect::from_min_max(pos2(area.left(), content.bottom()), pos2(content.right(), area.bottom()));
            changed |= ScrollBar::horizontal(("page-h", b.embed)).show(ui, hbar, &mut moved.x, m0.width, m0.view_width);
            painter.rect_filled(Rect::from_min_max(content.max, area.max), 0.0, c(p.panel_background));
        }
        if changed && let Some(t) = b.tabs.get_mut(b.active) {
            t.scrolled = Some((Instant::now(), moved));
            if let Some(view) = &t.view {
                view.evaluate_javascript(scroll::scroll_to(moved.x, moved.y), |_| {});
            }
        }
        if b.tab().is_some_and(|t| t.scrolled.is_some_and(|(at, _)| at.elapsed() < SCROLL_SETTLE)) {
            ui.ctx().request_repaint_after(SCROLL_SETTLE);
        }
        painter.rect_filled(content, 0.0, c(p.document_background));
        let size = dpi::PhysicalSize::new(
            (content.width() * scale).round().max(1.0) as u32,
            (content.height() * scale).round().max(1.0) as u32,
        );
        let viewport = Viewport { rect: content, scale };
        if b.viewport.map(|v| (v.rect, v.scale)) != Some((content, scale)) {
            cx.place_embed(b.embed, content);
            b.viewport = Some(viewport);
        }
        if b.size != size || (b.scale - scale).abs() > f32::EPSILON {
            b.size = size;
            b.scale = scale;
            if let Some(s) = &b.surface {
                s.resize(size);
            }
            for t in &b.tabs {
                if let Some(v) = &t.view {
                    v.resize(size);
                    if (v.hidpi_scale_factor().get() - scale).abs() > f32::EPSILON {
                        v.set_hidpi_scale_factor(Scale::new(scale));
                    }
                }
            }
        }
        let page = ui.interact(content, content_id, Sense::click_and_drag());
        if page.hovered()
            && let Some(t) = b.tab()
        {
            ui.ctx().set_cursor_icon(cursor_icon(t.cursor));
        }
        // Not the Enter that went to the location field and handed the page focus.
        let keyboard = page.has_focus() && navigate.is_none();
        if keyboard {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    content_id,
                    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }
        let events = ui.input(|i| i.events.clone());
        let fwd = input::forward(&events, viewport, keyboard, &mut b.pointer);
        if fwd.clicked {
            page.request_focus();
            b.menu = None;
            b.select = None;
        }
        if let Some(text) = fwd.paste {
            *self.engine.events.paste.borrow_mut() = Some(text);
            self.pasted = true;
        }
        // The page keeps its focus while popups (its own menus and
        // <select>s, the menu bar) have the keyboard; only another control
        // in the window takes it.
        // Before the click itself goes in: a focus change after it would
        // blur what was clicked (and close a <select>'s list).
        if keyboard || fwd.clicked {
            b.page_focus = true;
        } else if ui.ctx().memory(|m| m.focused()).is_some_and(|id| id != content_id) {
            b.page_focus = false;
        }
        let focused = b.page_focus;
        if let Some(view) = b.view() {
            for e in fwd.events {
                view.notify_input_event(e);
            }
            if view.focused() != focused {
                view.set_focused(focused);
            }
        }

        // The key button's logins.
        if let Some(menu) = b.logins_menu.take() {
            let request = MenuRequest {
                menu,
                source: Id::unique("logins-menu"),
                anchor: MenuAnchor::Below(b.key),
                selected: None,
            };
            menu::request_popup(ui.ctx(), request);
        }
        if let Some(chosen) = widgets::popup_result(ui.ctx(), Id::unique("logins-menu")) {
            commands.push(chosen);
        }

        // Page menus and <select> lists.
        if let Some((anchor, menu, selected)) = b.popup.take() {
            menu::request_popup(ui.ctx(), MenuRequest { menu, source: Id::unique("page-menu"), anchor, selected });
        }
        if let Some(chosen) = widgets::popup_result(ui.ctx(), Id::unique("page-menu")) {
            if chosen.starts_with("dl-")
                && let Some(menu) = b.menu.take()
            {
                let info = menu.element_info();
                if let Some(url) = if chosen == "dl-link" { &info.link_url } else { &info.image_url } {
                    commands.push(format!("download:{url}"));
                }
                menu.dismiss();
            } else if let Some(menu) = b.menu.take()
                && let Ok(n) = chosen.parse::<usize>()
                && let Some(ContextMenuItem::Item { action, .. }) = menu.items().get(n).cloned()
            {
                match action {
                    ContextMenuAction::OpenLinkInNewWebView | ContextMenuAction::OpenImageInNewView => {
                        let info = menu.element_info();
                        let url = if action == ContextMenuAction::OpenLinkInNewWebView { &info.link_url } else { &info.image_url };
                        if let Some(url) = url {
                            commands.push(format!("open-tab:{url}"));
                        }
                        menu.dismiss();
                    }
                    action => menu.select(action),
                }
            } else if let Some(mut select) = b.select.take()
                && let Ok(n) = chosen.parse::<usize>()
            {
                select.select(vec![n]);
                select.submit();
            }
        }

        // The window's title follows the page.
        let title = match b.tab().map(Tab::label) {
            Some(t) if t != "Untitled" => format!("{t} — {NAME}"),
            _ => NAME.into(),
        };
        if title != b.title {
            b.title = title.clone();
            cx.set_title(win, title);
        }

        if let Some(url) = navigate {
            self.load(cx, win, url);
        }
        for cmd in commands {
            if let Some(url) = cmd.strip_prefix("open-tab:") {
                self.new_tab(cx, win, Some(url.to_string()), false);
            } else if let Some(n) = cmd.strip_prefix("close:").and_then(|n| n.parse().ok()) {
                self.close_tab(cx, win, n);
            } else {
                self.command_for(cx, win, &cmd);
            }
        }
    }

    /// The page in tab `i` (view `view`) has something to download.
    fn page_download(&mut self, cx: &mut Cx, win: WindowId, i: usize, view: WebViewId, download: PageDownload) {
        let Some(b) = self.browser(win) else { return };
        let tab = &b.tabs[i];
        let page = tab.url.clone();
        match download {
            PageDownload::Unknown { url } => {
                if url != page {
                    return;
                }
                // Servo can't show it: fetch it as a file and go back.
                if tab.can_back
                    && let Some(view) = &tab.view
                {
                    view.go_back(1);
                }
                self.start_download(cx, url, None, None);
            }
            PageDownload::Link { url, name } => self.start_download(cx, url, Some(name), Some(page)),
            PageDownload::Data { id, name, seq, last, data } => {
                use base64::Engine as _;
                let entry = self.blobs.entry((view, id)).or_insert_with(|| (name, Vec::new()));
                let fits = entry.1.len() < BLOB_MAX;
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) if fits && (seq == 0 || !entry.1.is_empty()) => entry.1.extend_from_slice(&bytes),
                    _ => {
                        self.blobs.remove(&(view, id));
                        return;
                    }
                }
                if last && let Some((name, bytes)) = self.blobs.remove(&(view, id)) {
                    self.start(cx, String::new(), Source::Bytes(bytes), Some(name));
                }
            }
        }
    }

    /// Download `url` (with Servo's cookies for it) into the Downloads folder.
    fn start_download(&mut self, cx: &mut Cx, url: String, suggested: Option<String>, referer: Option<String>) {
        let Ok(parsed) = Url::parse(&url) else { return };
        if !matches!(parsed.scheme(), "http" | "https" | "data") {
            if let Some(b) = self.front.and_then(|w| self.browser(w))
                && let Some(t) = b.tabs.get_mut(b.active)
            {
                t.status = Some(format!("NetPositive can’t download {} addresses.", parsed.scheme()));
            }
            return;
        }
        let cookies = if parsed.scheme() == "data" {
            String::new()
        } else {
            let jar = self.engine.servo.site_data_manager().cookies_for_url(parsed, servo::CookieSource::HTTP);
            jar.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ")
        };
        let user_agent = servo::prefs::get().user_agent.clone();
        let source = Source::Http { url: url.clone(), cookies, referer, user_agent };
        self.start(cx, url, source, suggested);
    }

    fn start(&mut self, cx: &mut Cx, url: String, source: Source, suggested: Option<String>) {
        self.next_download += 1;
        let id = self.next_download;
        let name = suggested
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| downloads::url_name(&url))
            .map(|n| downloads::clean_name(&n))
            .unwrap_or_else(|| "download".into());
        let (download, cancel) = Download::new(id, url, name);
        self.downloads.push(download);
        downloads::start(self.proxy.clone(), id, source, suggested, downloads::folder(), cancel);
        self.show_downloads(cx);
    }

    fn show_downloads(&mut self, cx: &mut Cx) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Downloads)) {
            cx.redraw(id);
            return cx.activate(id);
        }
        let id = cx.open_window(WindowOptions::new("Downloads", 440, 300).min_size(320, 160));
        self.windows.insert(id, Win::Downloads);
    }

    fn downloads_ui(&mut self, cx: &mut Cx, _win: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, c(p.panel_background));
        let bottom = full.bottom() - m::CONTROL_HEIGHT - 2.0 * m::SPACING;
        let list = Rect::from_min_max(full.min + vec2(m::SPACING, m::SPACING), pos2(full.right() - m::SPACING, bottom));
        enum Act {
            Stop(u64),
            Open(PathBuf),
            Show(PathBuf),
            Retry(u64),
        }
        let mut acts = Vec::new();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        ScrollView::new("downloads").background(p.document_background).show_rows(
            &mut child,
            DOWNLOAD_ROW,
            self.downloads.len(),
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in range {
                    // Newest first.
                    let d = &self.downloads[self.downloads.len() - 1 - row];
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), DOWNLOAD_ROW), Sense::hover());
                    let painter = ui.painter();
                    if row > 0 {
                        style::hline(painter, r.left(), r.right(), r.top(), shade(&p, p.document_background, 1.147));
                    }
                    let kind = tabula_kit::files::kind_for(Path::new(&d.name), false, 0o644);
                    let icon = Rect::from_min_size(pos2(r.left() + m::SPACING, r.center().y - 16.0), vec2(32.0, 32.0));
                    icons::paint(painter, icon, kind.icon());
                    // Buttons, from the right.
                    let mut right = r.right() - m::SPACING;
                    let buttons: &[(&str, u8)] = match d.state {
                        downloads::State::Running => &[("Stop", 0)],
                        downloads::State::Done => &[("Show", 2), ("Open", 1)],
                        _ => &[("Retry", 3)],
                    };
                    for &(label, what) in buttons {
                        let w = style::label_width(ui.ctx(), label, style::plain_font()) + 2.0 * m::CONTROL_PAD;
                        let w = w.max(64.0);
                        let b = Rect::from_min_size(pos2(right - w, r.center().y - m::CONTROL_HEIGHT / 2.0), vec2(w, m::CONTROL_HEIGHT));
                        right -= w + m::SPACING;
                        if ui.place(b, Button::new(label).min_width(w)).clicked() {
                            acts.push(match (what, &d.path) {
                                (0, _) => Act::Stop(d.id),
                                (1, Some(path)) => Act::Open(path.clone()),
                                (2, Some(path)) => Act::Show(path.clone()),
                                _ => Act::Retry(d.id),
                            });
                        }
                    }
                    let x = icon.right() + m::SPACING;
                    let text = Rect::from_min_max(pos2(x, r.top()), pos2(right, r.bottom()));
                    let clip = ui.painter().with_clip_rect(text);
                    let color = c(p.document_text);
                    style::label(&clip, x, r.top() + 14.0, Align::Min, &d.name, style::plain_font(), color);
                    let status = d.status();
                    let failed = matches!(d.state, downloads::State::Failed(_));
                    let dim = if failed { c(p.failure) } else { style::disabled_text(&p) };
                    if d.state == downloads::State::Running && d.total.is_some() {
                        let bar = Rect::from_min_max(pos2(x, r.top() + 26.0), pos2(text.right(), r.top() + 36.0));
                        ui.place(bar, StatusBar::new(d.fraction()).bar_height(10.0));
                        style::label(&clip, x, r.top() + 46.0, Align::Min, status, style::small_font(), dim);
                    } else {
                        style::label(&clip, x, r.top() + 34.0, Align::Min, status, style::small_font(), dim);
                    }
                }
            },
        );
        if self.downloads.is_empty() {
            let text = style::disabled_text(&p);
            style::label(&painter, list.center().x, list.center().y, Align::Center, "No downloads", style::plain_font(), text);
        }
        // Clear (the finished ones) and where they go.
        let y = bottom + m::SPACING;
        let finished = self.downloads.iter().any(|d| d.state != downloads::State::Running);
        let clear = Rect::from_min_size(pos2(full.right() - m::SPACING - m::BUTTON_MIN_WIDTH, y), vec2(m::BUTTON_MIN_WIDTH, m::CONTROL_HEIGHT));
        if ui.add_enabled_ui(finished, |ui| ui.put(clear, Button::new("Clear"))).inner.clicked() {
            self.downloads.retain(|d| d.state == downloads::State::Running);
        }
        let folder = downloads::folder();
        let label = format!("Saved in {}", tabula_xdg::display_path(&folder));
        style::label(&painter, full.left() + m::SPACING, clear.center().y, Align::Min, label, style::plain_font(), c(p.panel_text));
        for act in acts {
            match act {
                Act::Stop(id) => {
                    if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                        d.stop();
                    }
                }
                Act::Open(path) => {
                    if let Err(e) = tabula_xdg::open::open(&tabula_xdg::open::Target::Path(path)) {
                        tracing::warn!("couldn't open the download: {e}");
                    }
                }
                Act::Show(path) => {
                    let dir = path.parent().map(Path::to_path_buf).unwrap_or(folder.clone());
                    if let Some(program) = tabula_xdg::find_program("tabula-tracker") {
                        let _ = tabula_xdg::open::spawn(&[program.into_os_string(), dir.into_os_string()]);
                    }
                }
                Act::Retry(id) => {
                    if let Some(i) = self.downloads.iter().position(|d| d.id == id) {
                        let old = self.downloads.remove(i);
                        if old.url.is_empty() {
                            // Page contents are gone once handed over.
                            self.downloads.insert(i, old);
                        } else {
                            self.start_download(cx, old.url.clone(), Some(old.name.clone()), None);
                        }
                    }
                }
            }
        }
    }

    fn open_find(&mut self, cx: &mut Cx, target: WindowId) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Find { target: t } if *t == target)) {
            return cx.activate(id);
        }
        let (w, h) = (FIND_W as u32, FIND_H as u32);
        let id = cx.open_window(WindowOptions::new("Find", w, h).min_size(w, h).max_size(900, h).parent(target));
        self.windows.insert(id, Win::Find { target });
    }

    /// Search the active tab's page from its selection.
    fn find_in(&mut self, win: WindowId) {
        let events = self.engine.events.clone();
        let script = find::script(&self.search);
        let Some(view) = self.browser(win).and_then(|b| b.tab()).and_then(|t| t.view.clone()) else { return };
        let id = view.id();
        view.evaluate_javascript(script, move |r| {
            let found = matches!(r, Ok(servo::JSValue::String(s)) if s == "found");
            events.push_id(id, Event::Found(found));
        });
    }

    fn find_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui, target: WindowId) {
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let s = &mut self.search;
        ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::top_down(Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            let field = ui.add(TextControl::new(&mut s.text).label("Find:").divider(40.0).autofocus(true));
            if field.gained_focus() {
                // The last search, ready to be typed over.
                select_all(ui.ctx(), field.id, &s.text);
            }
            ui.add_space(m::SPACING - m::TIGHT);
            ui.add(CheckBox::new(&mut s.case, "Case-sensitive"));
            ui.add(CheckBox::new(&mut s.words, "Whole words only"));
            ui.add(CheckBox::new(&mut s.wrap, "Wrap-around search"));
            ui.add(CheckBox::new(&mut s.backwards, "Search backwards"));
        });
        let bw = m::BUTTON_MIN_WIDTH;
        let y = full.bottom() - m::CONTROL_HEIGHT;
        let go_r = Rect::from_min_size(pos2(full.right() - bw, y), vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = go_r.translate(vec2(-(bw + m::SPACING + 2.0), 0.0));
        let cancel = ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
        let ready = !s.text.is_empty();
        let go = ui.add_enabled_ui(ready, |ui| ui.put(go_r, Button::new("Find").default(true))).inner.clicked()
            || ready && ui.input(|i| i.key_pressed(Key::Enter));
        if go || cancel {
            self.windows.remove(&window);
            cx.close_window(window);
        }
        if go {
            cx.activate(target);
            self.find_in(target);
        }
    }

    fn sheet_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(Win::Sheet(sheet)) = self.windows.get_mut(&win) else { return };
        let done = match &mut **sheet {
            Sheet::Dialog { dialog, text, .. } => {
                let buttons: &[&str] = match dialog {
                    SimpleDialog::Alert(_) => &["OK"],
                    _ => &["Cancel", "OK"],
                };
                let message = dialog.message().to_string();
                if let SimpleDialog::Prompt(_) = dialog {
                    let column = widgets::alert_frame(ui, "info");
                    let p = style::palette(ui.ctx());
                    let mut job = egui::text::LayoutJob::single_section(message, egui::TextFormat::simple(style::plain_font(), c(p.panel_text)));
                    job.wrap.max_width = column.width();
                    let g = ui.painter().layout_job(job);
                    let text_bottom = column.top() + g.size().y;
                    ui.painter().galley(column.min, g, c(p.panel_text));
                    let r = Rect::from_min_size(pos2(column.left(), text_bottom + m::SPACING), vec2(column.width(), m::CONTROL_HEIGHT));
                    ui.put(r, TextControl::new(text).autofocus(true));
                    choice_row(ui, column, buttons)
                } else {
                    widgets::alert(ui, "info", &message, buttons)
                }
                .map(|n| n + 1 == buttons.len())
            }
            Sheet::Login { user, password, site, .. } => {
                let column = widgets::alert_frame(ui, "authentication");
                let p = style::palette(ui.ctx());
                style::label(ui.painter(), column.left(), column.top() + 8.0, Align::Min, format!("{site} asks you to log in."), style::plain_font(), c(p.panel_text));
                let mut y = column.top() + 16.0 + m::SPACING;
                let divider = 70.0;
                let r = Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), m::CONTROL_HEIGHT));
                ui.put(r, TextControl::new(user).label("Name:").divider(divider).autofocus(true));
                y += m::CONTROL_HEIGHT + m::TIGHT;
                let r = Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), m::CONTROL_HEIGHT));
                ui.put(r, TextControl::new(password).label("Password:").divider(divider).password(true));
                choice_row(ui, column, &["Cancel", "Log In"]).map(|n| n == 1)
            }
            Sheet::Color { rgb, .. } => {
                let p = style::palette(ui.ctx());
                let full = ui.max_rect();
                ui.painter().rect_filled(full, 0.0, c(p.panel_background));
                let inner = full.shrink(m::WINDOW_INSET);
                let ramps = Rect::from_min_size(inner.min, vec2(inner.width(), COLOR_RAMPS_H));
                ui.put(ramps, widgets::ColorControl::new("page-color", rgb));
                let row = Rect::from_min_max(pos2(inner.left(), inner.bottom() - m::CONTROL_HEIGHT), inner.max);
                let well = Rect::from_min_size(row.min, vec2(2.0 * m::CONTROL_HEIGHT, m::CONTROL_HEIGHT));
                ui.put(well, |ui: &mut egui::Ui| widgets::color_well(ui, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), well.size()));
                choice_row(ui, row, &["Cancel", "OK"]).map(|n| n == 1)
            }
            Sheet::Files { panel, .. } => match panel.ui(ui) {
                Some(PanelResult::Chosen(path)) => {
                    if let Sheet::Files { picker, .. } = &mut **sheet {
                        picker.select(&[path]);
                    }
                    Some(true)
                }
                Some(PanelResult::Cancelled) => Some(false),
                None => None,
            },
        };
        let Some(ok) = done else { return };
        let Some(Win::Sheet(sheet)) = self.windows.remove(&win) else { return };
        cx.close_window(win);
        match *sheet {
            Sheet::Dialog { dialog, text, .. } => match dialog {
                SimpleDialog::Prompt(mut p) if ok => {
                    p.set_current_value(&text);
                    p.confirm();
                }
                d if ok => d.confirm(),
                d => d.dismiss(),
            },
            Sheet::Login { request, user, password, .. } => {
                if ok {
                    request.authenticate(user, password);
                }
            }
            Sheet::Files { picker, .. } => {
                if ok { picker.submit() } else { picker.dismiss() }
            }
            Sheet::Color { owner, mut picker, rgb: [red, green, blue] } => {
                if ok {
                    picker.select(Some(RgbColor { red, green, blue }));
                }
                picker.submit();
                // Servo 0.7 sets the value but fires no events, so pages
                // never hear of it: fire them once the value has landed.
                if ok && let Some(view) = self.browser(owner).and_then(|b| b.view()) {
                    let script = format!(
                        r#"(function(want, tries) {{
  var e = document.activeElement;
  if (!e || e.type !== 'color') return;
  (function wait() {{
    if (e.value === want) {{
      e.dispatchEvent(new Event('input', {{bubbles: true}}));
      e.dispatchEvent(new Event('change', {{bubbles: true}}));
    }} else if (tries-- > 0) setTimeout(wait, 20);
  }})();
}})('#{red:02x}{green:02x}{blue:02x}', 50)"#
                    );
                    view.evaluate_javascript(script, |_| {});
                }
            }
        }
    }
}

fn sheet_owner(s: &Sheet) -> Option<WindowId> {
    match s {
        Sheet::Dialog { owner, .. } | Sheet::Login { owner, .. } | Sheet::Color { owner, .. } => Some(*owner),
        Sheet::Files { .. } => None,
    }
}

/// Cancel / OK along the bottom of an alert with fields.
fn choice_row(ui: &mut egui::Ui, column: Rect, buttons: &[&str]) -> Option<usize> {
    let row = Rect::from_min_max(pos2(column.left(), column.bottom() - m::CONTROL_HEIGHT), column.max);
    let mut chosen = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
        widgets::button_row(ui, |ui| {
            for (i, label) in buttons.iter().enumerate().rev() {
                if ui.add(Button::new(label).default(i + 1 == buttons.len())).clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    ui.input(|i| {
        if i.key_pressed(Key::Enter) {
            chosen = Some(buttons.len() - 1);
        } else if i.key_pressed(Key::Escape) {
            chosen = Some(0);
        }
    });
    chosen
}

fn select_all(ctx: &egui::Context, id: Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ctx, id);
    }
}

/// Only the active tab's view draws.
fn show_active(b: &mut Browser) {
    for (i, t) in b.tabs.iter().enumerate() {
        if let Some(v) = &t.view {
            if i == b.active {
                v.show();
                v.paint();
            } else {
                v.hide();
            }
        }
    }
    if let (Some(v), Some(s)) = (b.view(), &b.surface) {
        use servo::RenderingContext;
        v.paint();
        s.context.present();
    }
}

/// The tab strip: BeOS tabs side by side, the selected one open into the
/// page, each with a close box; middle-click closes too.
/// What a bar under the toolbar says and offers.
struct Notice<'a> {
    icon: &'a str,
    text: String,
    /// `text` is an error.
    error: bool,
    /// A pop-up of choices left of the buttons.
    choice: Option<(&'a mut usize, Vec<String>)>,
    /// (label, enabled, command), from the right: the default first.
    buttons: Vec<(&'a str, bool, &'a str)>,
}

/// A bar under the toolbar asking about a login, a passkey or a stuck
/// page: an icon, the message, and its controls. Clicks push their
/// commands.
fn notice(ui: &mut egui::Ui, painter: &egui::Painter, bar: Rect, n: Notice, commands: &mut Vec<String>) {
    let p = style::palette(ui.ctx());
    painter.rect_filled(bar, 0.0, shade(&p, p.panel_background, 1.06));
    style::hline(painter, bar.left(), bar.right(), bar.bottom() - 1.0, shade(&p, p.panel_background, 1.3));
    icons::paint(painter, Rect::from_center_size(pos2(bar.left() + m::SPACING + 8.0, bar.center().y), vec2(16.0, 16.0)), n.icon);
    let mut right = bar.right() - m::SPACING;
    let mut slot = |w: f32| {
        let r = Rect::from_min_size(pos2(right - w, bar.center().y - m::CONTROL_HEIGHT / 2.0), vec2(w, m::CONTROL_HEIGHT));
        right -= w + m::SPACING;
        r
    };
    for (label, enabled, command) in n.buttons {
        let w = style::label_width(ui.ctx(), label, style::plain_font()).max(m::BUTTON_MIN_WIDTH - 2.0 * m::CONTROL_PAD) + 2.0 * m::CONTROL_PAD;
        let r = slot(w);
        if ui.add_enabled_ui(enabled, |ui| ui.put(r, Button::new(label))).inner.clicked() {
            commands.push(command.into());
        }
    }
    if let Some((chosen, labels)) = n.choice {
        let options: Vec<&str> = labels.iter().map(String::as_str).collect();
        let r = slot(PASSKEY_CHOICE_WIDTH);
        ui.put(r, MenuField::new("passkey-choice", chosen, &options).width(PASSKEY_CHOICE_WIDTH));
    }
    let color = if n.error { c(p.failure) } else { c(p.panel_text) };
    let clip = Rect::from_min_max(pos2(bar.left() + m::SPACING * 2.0 + 16.0, bar.top()), pos2(right, bar.bottom()));
    style::label(&painter.with_clip_rect(clip), clip.left(), bar.center().y, Align::Min, n.text, style::plain_font(), color);
}

fn tab_strip(ui: &mut egui::Ui, strip: Rect, b: &Browser) -> Option<String> {
    let p = style::palette(ui.ctx());
    let painter = ui.painter().clone();
    let bg = p.panel_background;
    painter.rect_filled(strip, 0.0, shade(&p, bg, 1.07));
    style::hline(&painter, strip.left(), strip.right(), strip.bottom() - 1.0, shade(&p, bg, 1.3));
    let n = b.tabs.len() as f32;
    let new_w = TOOL;
    let width = ((strip.width() - 2.0 * m::TIGHT - new_w) / n).min(TAB_MAX).floor();
    let mut command = None;
    for (i, tab) in b.tabs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(strip.left() + m::TIGHT + i as f32 * width, strip.top() + 2.0), vec2(width, TAB_HEIGHT - 2.0));
        let selected = i == b.active;
        let resp = ui.interact(r, Id::unique(("tab", i)), Sense::click());
        if selected {
            painter.rect_filled(r, 0.0, c(bg));
            style::hline(&painter, r.left(), r.right(), r.top(), c(p.shine));
            style::vline(&painter, r.left(), r.top(), r.bottom(), c(p.shine));
            style::vline(&painter, r.right() - 1.0, r.top(), r.bottom(), shade(&p, bg, 1.4));
        } else {
            style::vline(&painter, r.right() - 1.0, r.top() + 3.0, r.bottom() - 3.0, shade(&p, bg, 1.3));
        }
        let close = Rect::from_center_size(pos2(r.right() - m::SPACING - 4.0, r.center().y), vec2(12.0, 12.0));
        let close_resp = ui.interact(close, Id::unique(("tab-close", i)), Sense::click());
        if selected || resp.hovered() || close_resp.hovered() {
            let color = if close_resp.hovered() { c(p.panel_text) } else { shade(&p, bg, 1.6) };
            let k = close.shrink(3.0);
            painter.line_segment([k.left_top(), k.right_bottom()], Stroke::new(1.5, color));
            painter.line_segment([k.right_top(), k.left_bottom()], Stroke::new(1.5, color));
        }
        let icon = Rect::from_min_size(pos2(r.left() + m::SPACING, r.center().y - 8.0), vec2(16.0, 16.0));
        let icon = style::snap(ui.ctx(), icon);
        match &tab.favicon {
            Some(t) => {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(t.id(), icon, uv, egui::Color32::WHITE);
            }
            None => {
                icons::paint(&painter, icon, "document");
            }
        }
        let font = if selected { style::bold_font() } else { style::plain_font() };
        let clip = Rect::from_min_max(pos2(icon.right() + m::TIGHT + 1.0, r.top()), pos2(close.left() - m::TIGHT, r.bottom()));
        let label = tab.label();
        let text_color = if selected { c(p.panel_text) } else { shade(&p, p.panel_text, 0.8) };
        style::label(&painter.with_clip_rect(clip), clip.left(), r.center().y, Align::Min, label, font, text_color);
        if close_resp.clicked() || resp.middle_clicked() {
            command = Some(format!("close:{i}"));
        } else if resp.clicked() {
            command = Some(format!("tab:{i}"));
        }
    }
    let plus = Rect::from_min_size(pos2(strip.left() + m::TIGHT + n * width + m::TIGHT, strip.top() + 2.0), vec2(new_w, TAB_HEIGHT - 4.0));
    let resp = ui.interact(plus, Id::unique("tab-new"), Sense::click());
    let color = if resp.hovered() { c(p.panel_text) } else { shade(&p, bg, 1.6) };
    let k = Rect::from_center_size(plus.center(), vec2(9.0, 9.0));
    style::hline(&painter, k.left(), k.right(), k.center().y.floor(), color);
    style::vline(&painter, k.center().x.floor(), k.top(), k.bottom(), color);
    if resp.clicked() {
        command = Some("new-tab".into());
    }
    command
}

#[derive(Clone, Copy)]
enum Glyph {
    Back,
    Forward,
    Reload,
    Stop,
    Home,
}

/// Toolbar glyphs, drawn to the pixel grid.
fn paint_glyph(painter: &egui::Painter, r: Rect, glyph: Glyph, color: egui::Color32) {
    let c0 = r.center();
    let stroke = Stroke::new(1.5, color);
    match glyph {
        Glyph::Back | Glyph::Forward => {
            let d = if matches!(glyph, Glyph::Back) { -1.0 } else { 1.0 };
            let tip = pos2(c0.x + d * 6.0, c0.y);
            let pts = vec![tip, pos2(c0.x - d * 1.0, c0.y - 6.5), pos2(c0.x - d * 1.0, c0.y + 6.5)];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            let shaft = Rect::from_min_max(pos2(c0.x.min(c0.x - d * 6.0), c0.y - 2.0), pos2(c0.x.max(c0.x - d * 6.0), c0.y + 2.0));
            painter.rect_filled(shaft, 0.0, color);
        }
        Glyph::Reload => {
            let radius = 5.5;
            let pts: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let a = -0.35 + i as f32 / 24.0 * 5.0;
                    pos2(c0.x + radius * a.cos(), c0.y + radius * a.sin())
                })
                .collect();
            let end = *pts.last().unwrap_or(&c0);
            painter.add(Shape::line(pts, stroke));
            // The arrowhead at the end of the arc, pointing along it.
            let a: f32 = -0.35 + 5.0;
            let (tx, ty) = (-a.sin(), a.cos());
            let (nx, ny) = (a.cos(), a.sin());
            let head = vec![
                pos2(end.x + tx * 4.0, end.y + ty * 4.0),
                pos2(end.x + nx * 3.5, end.y + ny * 3.5),
                pos2(end.x - nx * 3.5, end.y - ny * 3.5),
            ];
            painter.add(Shape::convex_polygon(head, color, Stroke::NONE));
        }
        Glyph::Stop => {
            let k = Rect::from_center_size(c0, vec2(10.0, 10.0));
            painter.line_segment([k.left_top(), k.right_bottom()], Stroke::new(2.0, color));
            painter.line_segment([k.right_top(), k.left_bottom()], Stroke::new(2.0, color));
        }
        Glyph::Home => {
            let roof = vec![pos2(c0.x, c0.y - 7.0), pos2(c0.x + 7.5, c0.y), pos2(c0.x - 7.5, c0.y)];
            painter.add(Shape::convex_polygon(roof, color, Stroke::NONE));
            let body = Rect::from_min_max(pos2(c0.x - 5.0, c0.y - 0.5), pos2(c0.x + 5.0, c0.y + 6.0));
            painter.rect_filled(body, 0.0, color);
            let door = Rect::from_min_max(pos2(c0.x - 1.5, c0.y + 2.0), pos2(c0.x + 1.5, c0.y + 6.0));
            painter.rect_filled(door, 0.0, c(style::palette(painter.ctx()).control_background));
        }
    }
}

fn cursor_icon(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::None => CursorIcon::None,
        Cursor::Pointer => CursorIcon::PointingHand,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Move => CursorIcon::Move,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::EResize => CursorIcon::ResizeEast,
        Cursor::NResize => CursorIcon::ResizeNorth,
        Cursor::NeResize => CursorIcon::ResizeNorthEast,
        Cursor::NwResize => CursorIcon::ResizeNorthWest,
        Cursor::SResize => CursorIcon::ResizeSouth,
        Cursor::SeResize => CursorIcon::ResizeSouthEast,
        Cursor::SwResize => CursorIcon::ResizeSouthWest,
        Cursor::WResize => CursorIcon::ResizeWest,
        Cursor::EwResize => CursorIcon::ResizeHorizontal,
        Cursor::NsResize => CursorIcon::ResizeVertical,
        Cursor::NeswResize => CursorIcon::ResizeNeSw,
        Cursor::NwseResize => CursorIcon::ResizeNwSe,
        Cursor::ColResize => CursorIcon::ResizeColumn,
        Cursor::RowResize => CursorIcon::ResizeRow,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
        _ => CursorIcon::Default,
    }
}

impl App for NetPositive {
    fn theme_changed(&mut self, _cx: &mut Cx, palette: &tabula_kit::theme::Palette) {
        self.dark = palette.mode == tabula_kit::theme::ThemeMode::Dark;
        let theme = self.theme();
        for w in self.windows.values() {
            if let Win::Browser(b) = w {
                for view in b.tabs.iter().filter_map(|t| t.view.as_ref()) {
                    view.notify_theme_change(theme);
                }
            }
        }
    }

    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get(&window) {
            Some(Win::Browser(_)) => {
                self.front = Some(window);
                self.browser_ui(cx, window, ui);
            }
            Some(Win::Sheet(_)) => self.sheet_ui(cx, window, ui),
            Some(&Win::Find { target }) => self.find_ui(cx, window, ui, target),
            Some(Win::Downloads) => self.downloads_ui(cx, window, ui),
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        let Some(Win::Browser(b)) = self.windows.get(&window) else { return None };
        let tab = b.tab();
        let (can_back, can_forward, loading) = tab.map(|t| (t.can_back, t.can_forward, t.loading())).unwrap_or_default();
        let url = tab.map(|t| t.url.clone()).unwrap_or_default();
        let file = Menu::new("File")
            .item("New Window", "new-window", Some(Key::N))
            .item("New Tab", "new-tab", Some(Key::T))
            .item("Open Location…", "open-location", Some(Key::L))
            .separator()
            .item("Close Tab", "close-tab", Some(Key::W))
            .item_shift("Close Window", "close-window", Key::W)
            .separator()
            .item("Quit", "quit", Some(Key::Q));
        let edit = Menu::new("Edit")
            .item("Undo", "undo", Some(Key::Z))
            .item_shift("Redo", "redo", Key::Z)
            .separator()
            .item("Cut", "cut", Some(Key::X))
            .item("Copy", "copy", Some(Key::C))
            .item("Paste", "paste", Some(Key::V))
            .item("Select All", "select-all", Some(Key::A))
            .separator()
            .item("Find…", "find", Some(Key::F))
            .item("Find Again", "find-again", Some(Key::G));
        let view = Menu::new("View")
            .item(if loading { "Stop" } else { "Reload" }, if loading { "stop" } else { "reload" }, Some(Key::R))
            .separator()
            .item("Zoom In", "zoom-in", Some(Key::Equals))
            .item("Zoom Out", "zoom-out", Some(Key::Minus))
            .item("Actual Size", "zoom-reset", Some(Key::Num0));
        let mut go = Menu::new("Go")
            .item("Back", "back", Some(Key::OpenBracket))
            .enabled(can_back)
            .item("Forward", "forward", Some(Key::CloseBracket))
            .enabled(can_forward)
            .item_shift("Home", "home", Key::H)
            .item("Set Home Page", "set-home", None)
            .separator();
        let recent: Vec<&Bookmark> = self.settings.history.iter().rev().take(15).collect();
        for h in &recent {
            go = go.item(menu_label(h), format!("go:{}", h.url), None);
        }
        if !recent.is_empty() {
            go = go.separator().item("Clear History", "clear-history", None);
        }
        let marked = self.settings.bookmarks.iter().any(|m| m.url == url);
        let mut bookmarks = Menu::new("Bookmarks").item(if marked { "Remove Bookmark" } else { "Add Bookmark" }, "add-bookmark", Some(Key::D));
        if !self.settings.bookmarks.is_empty() {
            bookmarks = bookmarks.separator();
            for m in &self.settings.bookmarks {
                bookmarks = bookmarks.item(menu_label(m), format!("go:{}", m.url), None);
            }
        }
        let mut tabs = Menu::new("Window");
        for (i, t) in b.tabs.iter().enumerate() {
            let label = truncate(&t.label(), 48);
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].get(i).copied();
            tabs = tabs.item(label, format!("tab:{i}"), key).checked(i == b.active);
        }
        tabs = tabs.separator().item("Downloads", "downloads", None);
        Some(vec![file, edit, view, go, bookmarks, tabs])
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        if matches!(self.windows.get(&window), Some(Win::Browser(_))) {
            self.command_for(cx, window, command);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        match self.windows.get(&window) {
            Some(Win::Browser(_)) => self.close_browser(cx, window),
            Some(Win::Sheet(_) | Win::Find { .. } | Win::Downloads) => {
                // Closing a sheet cancels it (dropping the request does).
                self.windows.remove(&window);
                cx.close_window(window);
            }
            None => cx.close_window(window),
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Wake>() {
            return self.pump(cx);
        }
        let message = match message.downcast::<guard::Report>() {
            Ok(report) => return self.guard(cx, *report),
            Err(m) => m,
        };
        let message = match message.downcast::<rtc::Reply>() {
            Ok(reply) => {
                let view = self.windows.values().filter_map(|w| if let Win::Browser(b) = w { Some(b) } else { None }).flat_map(|b| b.tabs.iter()).filter_map(|t| t.view.as_ref()).find(|v| v.id() == reply.view);
                if let Some(view) = view {
                    view.evaluate_javascript(reply.script, |_| {});
                }
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Answer>() {
            Ok(answer) => return self.answer(cx, *answer),
            Err(m) => m,
        };
        let message = match message.downcast::<downloads::Update>() {
            Ok(update) => {
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == update.id) {
                    d.update(*update);
                }
                if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Downloads)) {
                    cx.redraw(id);
                }
                return;
            }
            Err(m) => m,
        };
        if let Ok(launch) = message.downcast::<Launch>() {
            self.launch(cx, &launch.args, launch.cwd.as_deref());
        }
    }

    fn embedded(&mut self, cx: &mut Cx, embed: &Embedded) {
        let Some(Win::Browser(b)) = self.windows.get_mut(&embed.window) else { return };
        if b.embed != embed.id {
            return;
        }
        let scale = embed.scale as f32;
        if let Some(_surface) = &b.surface {
            // A new scale: views follow in the next frame's layout.
            b.scale = scale;
            cx.redraw(embed.window);
            return;
        }
        if b.size.width == 0 || b.size.height == 0 {
            let (w, h) = cx.window_size(embed.window).unwrap_or((960, 720));
            b.size = dpi::PhysicalSize::new((w as f32 * scale) as u32, (h as f32 * scale) as u32);
        }
        match Surface::new(embed, b.size) {
            Ok(s) => {
                b.surface = Some(s);
                b.scale = scale;
                self.realize(embed.window);
            }
            Err(e) => tracing::error!("can't draw web pages: {e}"),
        }
        cx.redraw(embed.window);
    }

    fn embed_lost(&mut self, _cx: &mut Cx, id: EmbedId) {
        let Some(win) = self.windows.iter().find_map(|(&w, x)| matches!(x, Win::Browser(b) if b.embed == id).then_some(w)) else { return };
        let Some(Win::Browser(b)) = self.windows.get_mut(&win) else { return };
        // Keep where each tab was; views come back on the new surface.
        for t in &mut b.tabs {
            if let Some(v) = t.view.take()
                && let Some(url) = v.url()
            {
                t.url = url.to_string();
            }
            t.load = None;
        }
        b.menu = None;
        b.select = None;
        let surface = b.surface.take();
        b.viewport = None;
        self.drop_surface(surface);
    }

    fn save_state(&mut self, cx: &Cx) -> Option<String> {
        let windows = self
            .windows
            .iter()
            .filter_map(|(&id, w)| match w {
                Win::Browser(b) => Some(SavedWindow {
                    tabs: b.tabs.iter().map(|t| t.view.as_ref().and_then(|v| v.url()).map(|u| u.to_string()).unwrap_or_else(|| t.url.clone())).collect(),
                    active: b.active,
                    size: cx.window_size(id).unwrap_or_else(|| default_size(cx)),
                }),
                _ => None,
            })
            .collect();
        serde_json::to_string(&Saved { windows }).ok()
    }
}

/// 1024×768 where it fits, else most of the screen.
fn default_size(cx: &Cx) -> (u32, u32) {
    let (sw, sh) = cx.screen_size().unwrap_or((1280, 800));
    (1024.min(sw * 9 / 10), 768.min(sh * 8 / 10))
}

/// What to say when Passwords didn't give what was asked.
fn trouble(result: &Result<Response, String>) -> String {
    match result {
        Ok(Response::Error { message }) => message.clone(),
        Ok(Response::NotSetUp) => "Passwords isn’t set up yet.".into(),
        Ok(Response::Locked) => "Passwords is locked.".into(),
        Err(e) => e.clone(),
        Ok(_) => "Passwords answered oddly.".into(),
    }
}

fn menu_label(b: &Bookmark) -> String {
    truncate(if b.title.is_empty() { &b.url } else { &b.title }, 60)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

impl NetPositive {
    /// Ask Passwords about the active tab's page (starting Passwords if
    /// it isn't running).
    fn ask_logins(&mut self, win: WindowId, purpose: Purpose, request: impl FnOnce(String) -> Request) {
        let Some(b) = self.browser(win) else { return };
        let Some(tab) = b.tab() else { return };
        let Some(view) = tab.view.as_ref().map(|v| v.id()) else { return };
        let url = tab.url.clone();
        if !logins::web(&url) {
            return;
        }
        logins::ask(self.proxy.clone(), win, view, url.clone(), purpose, request(url), true);
    }

    /// The page in tab `i` asks for a passkey: check the relying party,
    /// then ask in a bar (a sign-in first finds the page's passkeys).
    fn passkey_request(&mut self, cx: &mut Cx, win: WindowId, i: usize, request: PageRequest) {
        let token = self.engine.token.clone();
        let proxy = self.proxy.clone();
        let Some(b) = self.browser(win) else { return };
        let tab = &mut b.tabs[i];
        let Some(view) = tab.view.clone() else { return };
        let (id, requested, challenge, kind) = match request {
            PageRequest::Cancel { id } => {
                if tab.passkey.as_ref().is_some_and(|a| a.id == id) {
                    tab.passkey = None;
                    cx.redraw(win);
                }
                return;
            }
            PageRequest::Create { id, rp, user, challenge, exclude } => {
                let kind = AskKind::Create {
                    rp_name: rp.name,
                    user_id: user.id,
                    user_name: user.name,
                    display_name: user.display_name,
                    exclude,
                };
                (id, rp.id, challenge, kind)
            }
            PageRequest::Get { id, rp_id, challenge, allow } => (id, rp_id, challenge, AskKind::Get { allow, passkeys: None, chosen: 0 }),
        };
        if tab.passkey.is_some() {
            return passkeys::settle(&view, &token, id, Err(("NotAllowedError", "Another passkey request is open.")));
        }
        let rp_id = match passkeys::rp_id(requested.as_deref(), &tab.url) {
            Ok(rp_id) => rp_id,
            Err(e) => {
                passkeys::settle(&view, &token, id, Err(("SecurityError", &e)));
                tab.status = Some(e);
                return cx.redraw(win);
            }
        };
        let url = tab.url.clone();
        if let AskKind::Get { allow, .. } = &kind {
            let request = Request::Passkeys { url: url.clone(), rp_id: rp_id.clone(), allow: allow.clone() };
            logins::ask(proxy, win, view.id(), url.clone(), Purpose::Passkeys(id), request, true);
        }
        tab.passkey = Some(Ask { id, url, rp_id, challenge, kind, busy: false, error: None });
        cx.redraw(win);
    }

    /// Save Passkey or Sign In: have Passwords make or use the key.
    fn passkey_confirm(&mut self, cx: &mut Cx, win: WindowId) {
        let proxy = self.proxy.clone();
        let Some(b) = self.browser(win) else { return };
        let Some(tab) = b.tabs.get_mut(b.active) else { return };
        let (Some(ask), Some(view)) = (&mut tab.passkey, &tab.view) else { return };
        let (purpose, request) = match &ask.kind {
            AskKind::Create { rp_name, user_id, user_name, display_name, exclude } => (
                Purpose::PasskeyCreate(ask.id),
                Request::PasskeyCreate {
                    url: ask.url.clone(),
                    rp_id: ask.rp_id.clone(),
                    rp_name: rp_name.clone(),
                    user_id: user_id.clone(),
                    user_name: user_name.clone(),
                    display_name: display_name.clone(),
                    exclude: exclude.clone(),
                },
            ),
            AskKind::Get { passkeys: Some(list), chosen, .. } => {
                let Some(p) = list.get(*chosen) else { return };
                let data = passkeys::client_data("webauthn.get", &ask.challenge, &ask.url);
                let request = Request::PasskeySign {
                    url: ask.url.clone(),
                    rp_id: ask.rp_id.clone(),
                    vault: p.vault.clone(),
                    id: p.id.clone(),
                    client_data_hash: passkeys::hash(&data),
                };
                (Purpose::PasskeySign(ask.id, data), request)
            }
            AskKind::Get { .. } => return,
        };
        ask.busy = true;
        ask.error = None;
        logins::ask(proxy, win, view.id(), ask.url.clone(), purpose, request, true);
        cx.redraw(win);
    }

    /// Passwords answered.
    fn answer(&mut self, cx: &mut Cx, answer: Answer) {
        let Answer { win, view, url, purpose, result, waited } = answer;
        let events = self.engine.events.clone();
        let token = self.engine.token.clone();
        let Some((_, i)) = self.find(view).filter(|(w, _)| *w == win) else { return };
        let Some(b) = self.browser(win) else { return };
        let host = tabula_keychain::host(&url).unwrap_or_else(|| "this site".into());
        match purpose {
            Purpose::Menu | Purpose::Unlock => {
                if matches!(result, Ok(Response::Unlocked)) {
                    return self.ask_logins(win, Purpose::Menu, |url| Request::Logins { url });
                }
                // Only for the page it was asked about.
                if b.active != i || b.tabs[i].url != url {
                    return;
                }
                let mut menu = Menu::new("Logins");
                match &result {
                    Ok(Response::Logins { logins }) if logins.is_empty() => {
                        menu = menu.item(format!("No Logins for {host}"), "none", None).enabled(false);
                    }
                    Ok(Response::Logins { logins }) => {
                        for l in logins {
                            let label = if l.username.is_empty() {
                                l.title.clone()
                            } else {
                                format!("{} — {}", l.title, l.username)
                            };
                            menu = menu.item(truncate(&label, 60), format!("fill:{}/{}", l.vault, l.id), None);
                        }
                    }
                    Ok(Response::Locked) => menu = menu.item("Unlock Passwords…", "unlock-passwords", None),
                    Ok(Response::NotSetUp) => menu = menu.item("Set Up Passwords…", "open-passwords", None),
                    Ok(Response::Error { message }) => {
                        menu = menu.item(truncate(message, 60), "none", None).enabled(false)
                    }
                    Err(e) => menu = menu.item(truncate(e, 60), "none", None).enabled(false),
                    Ok(_) => {}
                }
                menu = menu.separator().item("Open Passwords", "open-passwords", None);
                b.logins_menu = Some(menu);
                if waited {
                    cx.activate(win);
                }
            }
            Purpose::Fill => match &result {
                Ok(Response::Credentials { username, password, otp }) => {
                    let Some(view) = b.tabs[i].view.clone() else { return };
                    let Ok(origin) = Url::parse(&url).map(|u| u.origin().ascii_serialization()) else { return };
                    let script = logins::fill_script(&origin, username, password);
                    let id = view.id();
                    view.evaluate_javascript(script, move |r| {
                        let result = match r {
                            Ok(servo::JSValue::String(s)) => s,
                            _ => "none".into(),
                        };
                        events.push_id(id, Event::Filled(result));
                    });
                    if waited {
                        cx.activate(win);
                    }
                    if let Some(otp) = otp {
                        cx.copy_secret(otp.clone(), std::time::Duration::from_secs(90));
                        b.tabs[i].status = Some("The one-time code is on the clipboard.".into());
                    }
                }
                Ok(Response::Error { message }) => b.tabs[i].status = Some(message.clone()),
                Err(e) => b.tabs[i].status = Some(e.clone()),
                Ok(_) => {}
            },
            Purpose::Check(mut offer) => {
                offer.kind = match &result {
                    Ok(Response::Known | Response::NotSetUp) => return,
                    Ok(Response::Changed { vault, id, title }) => {
                        OfferKind::Update { vault: vault.clone(), id: id.clone(), title: title.clone() }
                    }
                    // New, locked, or Passwords isn't running: saving will start or unlock it.
                    _ => OfferKind::New,
                };
                b.tabs[i].offer = Some(*offer);
            }
            Purpose::Passkeys(id) => {
                let Some(ask) = b.tabs[i].passkey.as_mut().filter(|a| a.id == id) else { return };
                match &result {
                    Ok(Response::Passkeys { passkeys }) => {
                        if let AskKind::Get { passkeys: list, .. } = &mut ask.kind {
                            *list = Some(passkeys.clone());
                        }
                    }
                    other => ask.error = Some(trouble(other)),
                }
                if waited {
                    cx.activate(win);
                }
            }
            Purpose::PasskeyCreate(id) | Purpose::PasskeySign(id, _) => {
                let tab = &mut b.tabs[i];
                let Some(ask) = tab.passkey.as_mut().filter(|a| a.id == id) else { return };
                let Some(view) = tab.view.clone() else { return };
                let settled = match (&result, &purpose) {
                    (
                        Ok(Response::PasskeyCreated { title, credential_id, attestation_object, authenticator_data, public_key }),
                        _,
                    ) => {
                        let value = serde_json::json!({
                            "id": credential_id,
                            "clientDataJSON": passkeys::client_data("webauthn.create", &ask.challenge, &ask.url),
                            "attestationObject": attestation_object,
                            "authenticatorData": authenticator_data,
                            "publicKey": public_key,
                        });
                        Some((Ok(value), format!("Saved the passkey in “{title}” in Passwords.")))
                    }
                    (Ok(Response::Assertion { credential_id, authenticator_data, signature, user_handle }), Purpose::PasskeySign(_, data)) => {
                        let value = serde_json::json!({
                            "id": credential_id,
                            "clientDataJSON": data,
                            "authenticatorData": authenticator_data,
                            "signature": signature,
                            "userHandle": user_handle,
                        });
                        Some((Ok(value), "Signed in with a passkey from Passwords.".to_string()))
                    }
                    (Ok(Response::PasskeyExists), _) => Some((
                        Err(("InvalidStateError", "A passkey for this account is already saved.")),
                        "Passwords already has a passkey for this account.".to_string(),
                    )),
                    (other, _) => {
                        ask.busy = false;
                        ask.error = Some(trouble(other));
                        None
                    }
                };
                if let Some((value, status)) = settled {
                    passkeys::settle(&view, &token, id, value);
                    tab.passkey = None;
                    tab.status = Some(status);
                }
                if waited {
                    cx.activate(win);
                }
            }
            Purpose::Save => {
                let tab = &mut b.tabs[i];
                match &result {
                    Ok(Response::Saved { title, updated }) => {
                        tab.offer = None;
                        let verb = if *updated { "Updated" } else { "Saved" };
                        tab.status = Some(format!("{verb} “{title}” in Passwords."));
                    }
                    other => {
                        if let Some(offer) = &mut tab.offer {
                            offer.busy = false;
                            offer.error = Some(match other {
                                Ok(Response::Error { message }) => message.clone(),
                                Err(e) => e.clone(),
                                _ => "Passwords couldn’t save it.".into(),
                            });
                        }
                    }
                }
            }
        }
        cx.redraw(win);
    }

    /// URLs (or paths) from the command line or a forwarded launch.
    /// Every second, from the guard thread: ask each tab's page whether
    /// it's alive and where, set its process's priority, and show which
    /// are stuck; and tell the tabs whose page was stopped.
    fn guard(&mut self, cx: &mut Cx, report: guard::Report) {
        match report {
            guard::Report::Tick => {
                let mut priorities: BTreeMap<u32, guard::Priority> = BTreeMap::new();
                for (&win, w) in self.windows.iter_mut() {
                    let Win::Browser(b) = w else { continue };
                    let mut changed = false;
                    for (i, tab) in b.tabs.iter_mut().enumerate() {
                        let Some(view) = &tab.view else { continue };
                        self.pings.ask(view);
                        let hung = self.pings.hung(view.id());
                        if hung != tab.hung {
                            tab.hung = hung;
                            changed |= i == b.active;
                        }
                        let p = if hung {
                            guard::Priority::Hung
                        } else if i == b.active {
                            guard::Priority::Visible
                        } else {
                            guard::Priority::Hidden
                        };
                        if let Some(pid) = self.pings.pid(view.id()) {
                            // A process with several tabs goes as its likeliest.
                            let e = priorities.entry(pid).or_insert(p);
                            *e = (*e).max(p);
                        }
                    }
                    if changed {
                        cx.redraw(win);
                    }
                }
                for (pid, p) in priorities {
                    self.watch.set_priority(pid, p);
                }
            }
            guard::Report::Stopped { pid, memory } => {
                let views = self.pings.views_in(pid);
                tracing::debug!("page process {pid} stopped (memory: {memory}), showing {} tabs", views.len());
                for id in views {
                    self.pings.forget(id);
                    let Some((win, i)) = self.find(id) else { continue };
                    let Some(b) = self.browser(win) else { continue };
                    let tab = &mut b.tabs[i];
                    tab.hung = false;
                    tab.stopped = Some(if memory {
                        "This page was stopped because it used too much memory.".into()
                    } else {
                        "This page was stopped.".into()
                    });
                    cx.redraw(win);
                }
            }
        }
    }

    fn launch(&mut self, cx: &mut Cx, args: &[String], cwd: Option<&Path>) {
        let urls: Vec<Url> = args.iter().filter(|a| !a.starts_with("--")).filter_map(|a| location::parse(a, cwd)).collect();
        match self.front.filter(|w| self.windows.contains_key(w)) {
            Some(win) if !urls.is_empty() => {
                for url in urls {
                    self.new_tab(cx, win, Some(url.to_string()), true);
                }
                cx.activate(win);
            }
            Some(win) => cx.activate(win),
            None => {
                let size = default_size(cx);
                self.open_browser(cx, urls.into_iter().map(|u| u.to_string()).collect(), 0, size);
            }
        }
    }
}

fn main() {
    // Pictures are decoded into big buffers that come and go as pages
    // scroll. glibc raises its mmap threshold past any size it has freed,
    // so after the first one they'd come from the heap and stay there,
    // fragmented, long after they're freed; a fixed threshold keeps every
    // big buffer in a mapping of its own, given back on free.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
    }
    guard::init();
    rtc::live::init();
    tabula_preview::audio::set_identity(NAME, "tabula-netpositive");
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Servo runs each site's pages in a process of their own: this program
    // again, started with the constellation's token (engine.rs).
    if let [flag, token] = &args[..]
        && flag == "--content-process"
    {
        guard::content_process(token.clone());
        return;
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, move |cx| {
        instance::listen(APP_ID, cx.proxy());
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let engine = Engine::new(cx.proxy());
        let rtc = rtc::Rtc::new(cx.proxy(), &engine.token);
        let mut app = NetPositive {
            engine,
            rtc,
            windows: BTreeMap::new(),
            settings,
            front: None,
            pasted: false,
            search: find::Search::default(),
            downloads: Vec::new(),
            next_download: 0,
            blobs: BTreeMap::new(),
            dark: cx.palette().mode == tabula_kit::theme::ThemeMode::Dark,
            proxy: cx.proxy(),
            pings: guard::Pings::default(),
            watch: guard::watch(cx.proxy()),
        };
        let restored: Option<Saved> = cx.restored_state().and_then(|s| serde_json::from_str(&s).ok());
        match restored {
            Some(saved) if !saved.windows.is_empty() => {
                for w in saved.windows {
                    app.open_browser(cx, w.tabs, w.active, w.size);
                }
            }
            _ => {
                let cwd = std::env::current_dir().ok();
                app.launch(cx, &args, cwd.as_deref());
            }
        }
        app
    })
}
