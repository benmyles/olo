//! What each window shows, for agents: every widget says what it is
//! (its role, its name, its value, where it is) as it paints, so each
//! frame leaves a tree of the window, which the socket agents and the
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
    nodes: Vec<Node>,
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

/// Says what a widget is, as it paints.
pub fn record(window: &Window, cx: &mut App, node: Node) {
    cx.default_global::<Agents>().windows.entry(window.window_handle().window_id()).or_default().nodes.push(node);
}

/// A window starts painting a frame (`Frame` does it): what it showed
/// before goes.
pub(crate) fn begin_frame(window: &Window, cx: &mut App, title: &str) {
    let painted = cx.default_global::<Agents>().windows.entry(window.window_handle().window_id()).or_default();
    painted.nodes.clear();
    if painted.title != title {
        painted.title = title.to_owned();
    }
}

/// A window closed.
pub(crate) fn forget(window: WindowId, cx: &mut App) {
    cx.default_global::<Agents>().windows.remove(&window);
}

/// The app's menu bar for the command palette, and how to choose from
/// it (`{"menus":true}`, `{"invoke":"cmd"}`).
pub fn set_menus(cx: &mut App, menus: impl Fn(&mut App) -> Menus + 'static, invoke: impl Fn(&str, &mut App) -> bool + 'static) {
    let agents = cx.default_global::<Agents>();
    agents.menus = Some(Box::new(menus));
    agents.invoke = Some(Box::new(invoke));
}

fn tree(handle: AnyWindowHandle, cx: &mut App) -> Option<WindowTree> {
    let (size, active) = handle.update(cx, |_, window, _| (window.viewport_size(), window.is_window_active())).ok()?;
    let agents = cx.try_global::<Agents>()?;
    let painted = agents.windows.get(&handle.window_id())?;
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let mut nodes = vec![UiNode { id: 1, role: "Window".into(), name: Some(painted.title.clone()), width: w, height: h, enabled: true, ..Default::default() }];
    for (i, n) in painted.nodes.iter().enumerate() {
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
            ..Default::default()
        });
    }
    Some(WindowTree { title: painted.title.clone(), width: w as u32, height: h as u32, active, nodes, ..Default::default() })
}

fn frames(handle: AnyWindowHandle, cx: &mut App) -> Option<FrameTimes> {
    let title = cx.try_global::<Agents>().and_then(|a| a.windows.get(&handle.window_id())).map(|p| p.title.clone()).unwrap_or_default();
    let (stats, gpu) = handle.update(cx, |_, window, _| (window.take_frame_stats(), window.gpu_specs())).ok()?;
    let adapter = gpu.map(|g| format!("{} ({})", g.device_name, g.driver_name)).unwrap_or_default();
    let marks = stats
        .marks
        .iter()
        .map(|&(at_us, ui_us, us)| FrameMark { at_us, us, ui_us, submit_us: us.saturating_sub(ui_us), ..Default::default() })
        .collect();
    Some(FrameTimes { title, frames: stats.frames, slowest_us: stats.slowest_us, total_us: stats.total_us, adapter, marks })
}

fn answer(query: Query, cx: &mut App) {
    match query {
        Query::Menus(reply) => {
            let menus = match cx.try_global::<Agents>() {
                Some(a) if a.menus.is_some() => {
                    // (Taken out while it runs: it may use the app.)
                    let menus = cx.global_mut::<Agents>().menus.take();
                    let out = menus.as_ref().map(|m| m(cx));
                    cx.global_mut::<Agents>().menus = menus;
                    out
                }
                Some(a) => Some(Menus { app_id: a.app_id.clone(), entries: Vec::new() }),
                None => None,
            };
            let _ = reply.send(menus.unwrap_or_default());
        }
        Query::Invoke(command, reply) => {
            let invoke = cx.default_global::<Agents>().invoke.take();
            let ok = invoke.as_ref().is_some_and(|f| f(&command, cx));
            cx.default_global::<Agents>().invoke = invoke;
            let _ = reply.send(ok);
        }
        Query::Tree(reply) => {
            let trees = cx.windows().into_iter().filter_map(|w| tree(w, cx)).collect();
            let _ = reply.send(trees);
        }
        // (app_server's keyboard hints name surfaces; GPUI windows don't
        // say which is theirs yet.)
        Query::Hints(_, reply) => {
            let _ = reply.send(Vec::new());
        }
        Query::Frames(reply) => {
            let times = cx.windows().into_iter().filter_map(|w| frames(w, cx)).collect();
            let _ = reply.send(times);
        }
        Query::Review(reply) => {
            let _ = reply.send(Vec::new());
        }
        Query::Path(_, reply) => {
            let _ = reply.send(None);
        }
        Query::LoseGpu(reply) => {
            let _ = reply.send(false);
        }
        Query::Focus(_) | Query::WindowOver(_) | Query::WindowDropped(_) => {}
    }
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
