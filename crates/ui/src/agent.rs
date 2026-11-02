//! What each window shows, for agents: every widget says what it is
//! (its role, its name, its value, where it is) as it paints, kept with
//! the frame (a cached view's replayed with its paint), so each frame
//! leaves a tree of the window, which the socket agents and the
//! command palette read apps through (`olo_kit_base::commands`) answers
//! from at once (as egui's AccessKit tree did, without a frame drawn for
//! it). Its frame times too, and the menu commands the app takes.

use std::collections::HashMap;

use futures::{StreamExt, channel::mpsc};
use gpui::{AnyWindowHandle, App, Bounds, Global, Pixels, SharedString, Window, WindowId};
use olo_kit_base::commands::{self, FrameMark, FrameTimes, Menus, Query, UiNode, WindowTree};

/// One widget, as agents read it.
#[derive(Clone, Debug, Default)]
pub struct Node {
    /// AccessKit's role: "Button", "CheckBox", "TextInput", "Label", …;
    /// "Clickable" for an area that takes clicks and says nothing more.
    pub role: &'static str,
    pub name: Option<SharedString>,
    pub value: Option<SharedString>,
    /// Where it is in the window, in logical pixels.
    pub bounds: Bounds<Pixels>,
    pub enabled: bool,
    pub clickable: bool,
    pub focused: bool,
    pub checked: Option<bool>,
    pub selected: Option<bool>,
    /// A menu item standing for a minimized window.
    pub minimized: bool,
}

impl Node {
    /// A widget that takes clicks: a button named `name`.
    pub fn button(name: impl Into<SharedString>, bounds: Bounds<Pixels>) -> Node {
        Node { role: "Button", name: Some(name.into()), bounds, enabled: true, clickable: true, ..Default::default() }
    }

    /// Text drawn in the window.
    pub fn label(text: impl Into<SharedString>, bounds: Bounds<Pixels>) -> Node {
        Node { role: "Label", name: Some(text.into()), bounds, enabled: true, ..Default::default() }
    }
}

#[derive(Default)]
struct Painted {
    title: String,
    /// A layer surface's namespace ("deskbar").
    layer: Option<String>,
}

/// Every window's widgets as last painted, and how to take the app's
/// menu commands.
#[derive(Default)]
struct Agents {
    app_id: String,
    windows: HashMap<WindowId, Painted>,
    menus: Option<Box<dyn Fn(&mut App) -> Menus>>,
    invoke: Option<Box<dyn Fn(&str, &mut App) -> bool>>,
}

impl Global for Agents {}

/// Says what a widget is, as it paints. (Kept with the frame: a cached
/// view drawn from its last paint says it again.)
pub fn record(window: &mut Window, _cx: &mut App, node: Node) {
    window.push_paint_record(std::rc::Rc::new(node));
}

/// A window starts painting a frame (`Frame` does it): its title.
pub(crate) fn begin_frame(window: &Window, cx: &mut App, title: &str) {
    let painted = cx.default_global::<Agents>().windows.entry(window.window_handle().window_id()).or_default();
    if painted.title != title {
        painted.title = title.to_owned();
    }
}

/// The window is a layer surface, `namespace` ("deskbar"): agents and
/// app_server's hints tell it from windows by it.
pub(crate) fn set_layer(window: &Window, cx: &mut App, namespace: &str) {
    cx.default_global::<Agents>().windows.entry(window.window_handle().window_id()).or_default().layer = Some(namespace.to_owned());
}

/// A window closed: what it showed goes.
pub(crate) fn forget_closed(cx: &mut App) {
    let open: std::collections::HashSet<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
    cx.default_global::<Agents>().windows.retain(|id, _| open.contains(id));
    if let Some(paths) = cx.try_global::<Paths>().is_some().then(|| cx.global_mut::<Paths>()) {
        paths.0.retain(|id, _| open.contains(id));
    }
}

/// The file or folder each window shows (`set_path`).
#[derive(Default)]
struct Paths(HashMap<WindowId, std::path::PathBuf>);

impl Global for Paths {}

/// Say which file or folder `window` shows now: a document once saved
/// or opened, None when it has none. app_server asks (`{"path":ID}`) for
/// its tab's menu (Open in Tracker), as the egui kit's `Cx::set_path`.
pub fn set_path(window: &Window, cx: &mut App, path: Option<std::path::PathBuf>) {
    let id = window.window_handle().window_id();
    let paths = &mut cx.default_global::<Paths>().0;
    match path {
        Some(path) => paths.insert(id, path),
        None => paths.remove(&id),
    };
}

/// The app's menu bar for the command palette, and how to choose from
/// it (`{"menus":true}`, `{"invoke":"cmd"}`).
pub fn set_menus(cx: &mut App, menus: impl Fn(&mut App) -> Menus + 'static, invoke: impl Fn(&str, &mut App) -> bool + 'static) {
    let agents = cx.default_global::<Agents>();
    agents.menus = Some(Box::new(menus));
    agents.invoke = Some(Box::new(invoke));
}

fn tree(handle: AnyWindowHandle, cx: &mut App) -> Option<WindowTree> {
    let (size, active, records) = handle.update(cx, |_, window, _| (window.viewport_size(), window.is_window_active(), window.paint_records().to_vec())).ok()?;
    let agents = cx.try_global::<Agents>()?;
    let painted = agents.windows.get(&handle.window_id())?;
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let mut nodes = vec![UiNode { id: 1, role: "Window".into(), name: Some(painted.title.clone()), width: w, height: h, enabled: true, ..Default::default() }];
    for (i, n) in records.iter().filter_map(|r| r.downcast_ref::<Node>()).enumerate() {
        let text = |s: &Option<SharedString>| s.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        nodes.push(UiNode {
            id: i as u64 + 2,
            parent: Some(1),
            role: n.role.into(),
            name: text(&n.name),
            value: text(&n.value),
            x: f32::from(n.bounds.origin.x),
            y: f32::from(n.bounds.origin.y),
            width: f32::from(n.bounds.size.width),
            height: f32::from(n.bounds.size.height),
            enabled: n.enabled,
            clickable: n.clickable,
            focused: n.focused,
            checked: n.checked,
            selected: n.selected,
            minimized: n.minimized,
            ..Default::default()
        });
    }
    Some(WindowTree { title: painted.title.clone(), width: w as u32, height: h as u32, active, nodes, layer: painted.layer.clone(), ..Default::default() })
}

fn frames(handle: AnyWindowHandle, cx: &mut App) -> Option<FrameTimes> {
    let title = cx.try_global::<Agents>().and_then(|a| a.windows.get(&handle.window_id())).map(|p| p.title.clone()).unwrap_or_default();
    let (stats, gpu) = handle.update(cx, |_, window, _| (window.take_frame_stats(), window.gpu_specs())).ok()?;
    let adapter = gpu.map(|g| format!("{} ({})", g.device_name, g.driver_name)).unwrap_or_default();
    let marks = stats
        .marks
        .iter()
        .zip(stats.scrolls.iter().map(Some).chain(std::iter::repeat(None)))
        .map(|(&(at_us, ui_us, us), scrolls)| FrameMark { at_us, us, ui_us, submit_us: us.saturating_sub(ui_us), scrolls: scrolls.cloned().unwrap_or_default(), ..Default::default() })
        .collect();
    Some(FrameTimes { title, frames: stats.frames, slowest_us: stats.slowest_us, total_us: stats.total_us, adapter, marks })
}

fn answer(query: Query, cx: &mut App) {
    match query {
        Query::Menus(reply) => {
            catch_up(cx);
            let menus = match cx.try_global::<Agents>() {
                Some(a) if a.menus.is_some() => {
                    // (Taken out while it runs: it may use the app.)
                    let menus = cx.global_mut::<Agents>().menus.take();
                    let out = menus.as_ref().map(|m| m(cx));
                    cx.global_mut::<Agents>().menus = menus;
                    out
                }
                // Else the front window's menu bar.
                Some(a) => {
                    let app_id = a.app_id.clone();
                    let bar = front(cx).and_then(|w| crate::menu::bar_menus(w.window_id(), cx));
                    let entries = bar.map(|(menus, _)| menus.iter().flat_map(|m| m.entries(&app_id).entries).collect()).unwrap_or_default();
                    Some(Menus { app_id, entries })
                }
                None => None,
            };
            let _ = reply.send(menus.unwrap_or_default());
        }
        Query::Invoke(command, reply) => {
            // (The menus as they are now: what's enabled.)
            catch_up(cx);
            let invoke = cx.default_global::<Agents>().invoke.take();
            let ok = match &invoke {
                Some(f) => f(&command, cx),
                // The front window's menu bar's, if it has the command.
                None => front(cx)
                    .and_then(|w| Some((w, crate::menu::bar_menus(w.window_id(), cx)?)))
                    .filter(|(_, (menus, _))| menus.iter().any(|m| m.find_action(&command).is_some_and(|a| a.enabled)))
                    .is_some_and(|(w, (_, on_command))| w.update(cx, |_, window, cx| on_command(&command, window, cx)).is_ok()),
            };
            cx.default_global::<Agents>().invoke = invoke;
            let _ = reply.send(ok);
        }
        Query::Tree(reply) => {
            catch_up(cx);
            let trees = cx.windows().into_iter().filter_map(|w| tree(w, cx)).collect();
            let _ = reply.send(trees);
        }
        // app_server's keyboard hints: the windows it names by surface,
        // what can be picked in each.
        Query::Hints(ids, reply) => {
            let trees = cx
                .windows()
                .into_iter()
                .filter_map(|w| {
                    let id = w.update(cx, |_, window, _| window.surface_id()).ok().flatten()?;
                    let mut t = tree(w, cx).filter(|_| ids.contains(&id))?;
                    t.surface = Some(id);
                    Some(t.pickable())
                })
                .collect();
            let _ = reply.send(trees);
        }
        Query::Frames(reply) => {
            let times = cx.windows().into_iter().filter_map(|w| frames(w, cx)).collect();
            let _ = reply.send(times);
        }
        Query::Review(reply) => {
            catch_up(cx);
            crate::review::start(cx, reply)
        }
        Query::Path(surface, reply) => {
            // The window with that surface, if it said what it shows.
            let window = cx.windows().into_iter().find(|w| w.update(cx, |_, window, _| window.surface_id() == Some(surface)).unwrap_or(false));
            let _ = reply.send(window.and_then(|w| cx.try_global::<Paths>()?.0.get(&w.window_id()).cloned()));
        }
        Query::LoseGpu(reply) => {
            let _ = reply.send(false);
        }
        // One of the app's windows being moved (a tab torn off) is over
        // another of its windows, or was let go there: the strip there
        // marks the place, or takes its tabs.
        Query::WindowOver(at) => {
            let onto = at.onto.and_then(|o| by_surface(cx, o));
            crate::widgets::strip::window_over(cx, onto, gpui::point(gpui::px(at.x as f32), gpui::px(at.y as f32)));
        }
        Query::WindowDropped(at) => {
            let point = gpui::point(gpui::px(at.x as f32), gpui::px(at.y as f32));
            match (by_surface(cx, at.window), at.onto.and_then(|o| by_surface(cx, o))) {
                (Some(window), Some(onto)) => crate::widgets::strip::window_dropped(cx, window, onto, point),
                _ => crate::widgets::strip::window_over(cx, None, point),
            }
        }
        Query::Focus(_) => {}
    }
}

/// The app's window with Wayland surface `id`.
fn by_surface(cx: &mut App, id: u32) -> Option<AnyWindowHandle> {
    cx.windows().into_iter().find(|w| w.update(cx, |_, window, _| window.surface_id() == Some(id)).unwrap_or(false))
}

/// The window in front: the active one, else the first.
/// Every window drawn as it is now, though not yet shown: one that isn't
/// showing draws only when it does, and an agent reads what it says now.
fn catch_up(cx: &mut App) {
    for w in cx.windows() {
        let _ = w.update(cx, |_, window, cx| window.draw_if_dirty(cx));
    }
}

fn front(cx: &App) -> Option<AnyWindowHandle> {
    cx.active_window().or_else(|| cx.windows().first().copied())
}

/// Starts answering on the app's socket.
pub(crate) fn init(cx: &mut App, app_id: &str) {
    cx.default_global::<Agents>().app_id = app_id.to_owned();
    let (tx, mut rx) = mpsc::unbounded::<Query>();
    let Some(listening) = commands::listen(move |q| {
        let _ = tx.unbounded_send(q);
    }) else {
        return;
    };
    cx.spawn(async move |cx| {
        // (Kept for as long as the app runs: dropping it removes the
        // socket.)
        let _listening = listening;
        while let Some(query) = rx.next().await {
            let _ = cx.update(|cx| answer(query, cx));
        }
    })
    .detach();
}
