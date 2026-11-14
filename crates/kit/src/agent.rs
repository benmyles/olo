//! What each window shows, for agents. Every view says what it is (its
//! role, its name, its value, where it is), and that is kept in the
//! window's tree: the tree agents read *is* the window's own, there
//! whether or not the window has drawn lately. The socket agents and
//! the command palette read apps through (`olo_kit_base::commands`)
//! answers from it: each window's looper is asked, and says what it has.
//! Its frame times too, and the menu commands the app takes.

use std::{path::PathBuf, time::Duration};

use olo_interface::{Bounds, Pixels, SharedString, WindowCx, px};
use olo_kit_base::commands::{self, FrameMark, FrameTimes, Menus, Query, UiNode, WindowTree};

pub use olo_interface::agent::Node;

/// How long a window's looper has to answer (one that's busy is left
/// out: an agent isn't kept waiting on it).
const WAIT: Duration = Duration::from_millis(1000);

/// What the kit keeps with a window for agents.
#[derive(Default)]
struct Shown {
    /// A layer surface's namespace ("deskbar").
    layer: Option<String>,
    /// The file or folder the window shows.
    path: Option<PathBuf>,
}

/// Says a part of a drawing is something (a key of a drawn keypad), for
/// agents: kept with the drawing.
pub fn record(g: &mut olo_interface::Painter, node: Node) {
    g.agent(node);
}

/// The window is a layer surface, `namespace` ("deskbar"): agents and
/// app_server's hints tell it from windows by it.
pub(crate) fn set_layer(cx: &mut WindowCx, namespace: &str) {
    cx.window_state::<Shown>().layer = Some(namespace.to_owned());
}

/// Say which file or folder the window shows now: a document once saved
/// or opened, None when it has none. app_server asks (`{"path":ID}`) for
/// its tab's menu (Open in Tracker).
pub fn set_path(cx: &mut WindowCx, path: Option<PathBuf>) {
    cx.window_state::<Shown>().path = path;
}

/// The window's tree, as agents read it.
pub(crate) fn tree(cx: &mut WindowCx) -> WindowTree {
    let size = cx.viewport_size();
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let title = cx.window_title().to_string();
    let mut nodes = vec![UiNode { id: 1, role: "Window".into(), name: Some(title.clone()), width: w, height: h, enabled: true, ..Default::default() }];
    // Text cut short to fit is read whole: a node named by what shows of
    // it ("Quarterly rep…") has its whole name, and one no node names is
    // a label of its own.
    let mut cuts: Vec<(Bounds<Pixels>, SharedString, bool)> = cx.cut_texts().into_iter().map(|(b, full)| (b, full, false)).collect();
    for (i, n) in cx.agent_nodes().into_iter().enumerate() {
        let text = |s: &Option<SharedString>| s.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        // What shows of it: its bounds in what clips it (a scroll view's,
        // which stays).
        let at = n.bounds;
        let shown = n.clip.map_or(at, |c| at.intersect(&c));
        let offscreen = n.bounds.size.width > px(0.0) && n.bounds.size.height > px(0.0) && (shown.size.width <= px(0.0) || shown.size.height <= px(0.0));
        let b = if offscreen { at } else { shown };
        let name = whole_name(text(&n.name), at, &mut cuts);
        nodes.push(UiNode {
            id: i as u64 + 2,
            parent: Some(1),
            role: n.role.into(),
            name,
            value: text(&n.value),
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
            offscreen,
            enabled: n.enabled,
            clickable: n.clickable,
            focused: n.focused,
            checked: n.checked,
            selected: n.selected,
            minimized: n.minimized,
            ..Default::default()
        });
    }
    for (b, full, _) in cuts.into_iter().filter(|(.., named)| !named) {
        let id = nodes.len() as u64 + 1;
        nodes.push(UiNode {
            id,
            parent: Some(1),
            role: "Label".into(),
            name: Some(full.trim().to_string()),
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
            enabled: true,
            ..Default::default()
        });
    }
    let active = cx.is_window_active();
    let surface = cx.surface_id();
    let shown = cx.window_state::<Shown>();
    WindowTree { title, width: w as u32, height: h as u32, active, nodes, layer: shown.layer.clone(), surface, path: shown.path.clone(), ..Default::default() }
}

/// A node's name, whole where it's a text cut short (`cuts`, each marked
/// once some node names it): "Quarterly rep…" over a cut of "Quarterly
/// report.pdf" is "Quarterly report.pdf".
fn whole_name(name: Option<String>, at: Bounds<Pixels>, cuts: &mut [(Bounds<Pixels>, SharedString, bool)]) -> Option<String> {
    let mut name = name;
    for (_, full, named) in cuts.iter_mut().filter(|(b, ..)| b.intersects(&at)) {
        match &name {
            Some(s) if s == full.trim() => *named = true,
            Some(s) if s.strip_suffix('…').is_some_and(|head| full.starts_with(head.trim_end())) => {
                name = Some(full.trim().to_string());
                *named = true;
            }
            _ => {}
        }
    }
    name
}

/// Every window's tree. (Each looper shows what changed first, so the
/// tree is the window as it would be drawn now.)
fn trees() -> Vec<WindowTree> {
    olo_interface::ask_windows(WAIT, |cx| {
        cx.flush_now();
        Some(tree(cx))
    })
}

fn frames() -> Vec<FrameTimes> {
    // (Through each window's looper, so how long the answer takes is how
    // long the app would keep a person waiting.)
    let titles = olo_interface::ask_windows(WAIT, |cx| Some((cx.window_id(), cx.window_title().to_string())));
    olo_interface::take_frame_stats()
        .into_iter()
        .map(|w| {
            let title = titles.iter().find(|(id, _)| *id == w.id).map(|(_, t)| t.clone()).unwrap_or_default();
            let adapter = w.gpu.map(|g| format!("{} ({})", g.device_name, g.driver_name)).unwrap_or_default();
            let stats = w.stats;
            let marks = stats
                .marks
                .iter()
                .zip(stats.scrolls.iter().map(Some).chain(std::iter::repeat(None)))
                .map(|(&(at_us, ui_us, us), scrolls)| FrameMark { at_us, us, ui_us, submit_us: us.saturating_sub(ui_us), scrolls: scrolls.cloned().unwrap_or_default(), ..Default::default() })
                .collect();
            FrameTimes { title, frames: stats.frames, slowest_us: stats.slowest_us, total_us: stats.total_us, adapter, marks }
        })
        .collect()
}

/// Answers a question from the socket (on its thread: each window is
/// asked on its own looper, and none is waited on for long).
fn answer(query: Query) {
    match query {
        Query::Tree(reply) => {
            let _ = reply.send(trees());
        }
        // app_server's keyboard hints: the windows it names by surface,
        // what can be picked in each.
        Query::Hints(ids, reply) => {
            let _ = reply.send(trees().into_iter().filter(|t| t.surface.is_some_and(|s| ids.contains(&s))).map(|t| t.pickable()).collect());
        }
        Query::Frames(reply) => {
            let _ = reply.send(frames());
        }
        Query::Review(reply) => {
            let dark = crate::theme::palette().mode != olo_theme::ThemeMode::Light;
            let _ =
                reply.send(olo_interface::ask_windows(Duration::from_millis(3000), move |cx| (!cx.window_state::<crate::review::NotReviewed>().0).then(|| crate::review::review_window(cx, dark).0)));
        }
        Query::Path(surface, reply) => {
            let found = olo_interface::ask_windows(WAIT, move |cx| (cx.surface_id() == Some(surface)).then(|| cx.window_state::<Shown>().path.clone())).into_iter().next().flatten();
            let _ = reply.send(found);
        }
        // The menus of the window with the keyboard (else the first that
        // has any), or of the one app_server names.
        Query::Menus(reply) => {
            let _ = reply.send(menus(None));
        }
        Query::MenusIn(surface, reply) => {
            let _ = reply.send(menus(Some(surface)));
        }
        Query::Invoke(command, reply) => {
            // The window with the keyboard takes it, else the first that
            // does.
            let taken = olo_interface::ask_windows(WAIT, move |cx| Some((cx.is_window_active(), cx.window_id(), command.clone())));
            let _ = reply.send(invoke(taken, None));
        }
        Query::InvokeIn(ask, reply) => {
            let surface = ask.surface;
            let command = ask.command.clone();
            let taken = olo_interface::ask_windows(WAIT, move |cx| (cx.surface_id() == Some(surface)).then(|| (true, cx.window_id(), command.clone())));
            let _ = reply.send(invoke(taken, Some(surface)));
        }
        // A test of a driver reset (debug builds only): the device is
        // marked lost.
        Query::LoseGpu(reply) => {
            let lose = cfg!(debug_assertions);
            if lose {
                olo_interface::lose_gpu_device();
            }
            let _ = reply.send(lose);
        }
        // app_server says one of the app's windows, being moved, is over
        // another of its windows, or was let go there: the strip there
        // marks the place, or takes its tabs.
        Query::WindowOver(at) => crate::widgets::strip::window_over(at.onto, olo_interface::point(olo_interface::px(at.x as f32), olo_interface::px(at.y as f32))),
        Query::WindowDropped(at) => {
            let point = olo_interface::point(olo_interface::px(at.x as f32), olo_interface::px(at.y as f32));
            match at.onto {
                Some(onto) => crate::widgets::strip::window_dropped(at.window, onto, point),
                None => crate::widgets::strip::window_over(None, point),
            }
        }
        // app_server's keyboard hints picked a text field, a slider or a
        // scroll bar (a click would change it): it gets the keyboard. A
        // tree's ids are its window's: by surface, else the window with
        // the keyboard.
        Query::Focus(node) => {
            let mut windows = olo_interface::ask_windows(WAIT, |cx| Some((cx.is_window_active(), cx.window_id())));
            windows.sort_by_key(|(active, _)| !*active);
            if let Some((_, id)) = windows.first().copied() {
                olo_interface::ask_windows(WAIT, move |cx| (cx.window_id() == id).then(|| focus_node(cx, node)));
            }
        }
        Query::FocusIn(ask, reply) => {
            let taken = olo_interface::ask_windows(WAIT, move |cx| (cx.surface_id() == Some(ask.surface)).then(|| focus_node(cx, ask.node)));
            let _ = reply.send(taken.into_iter().any(|t| t));
        }
        Query::Move(to, reply) => crate::upgrade::move_to(to, reply),
    }
}

/// Gives the keyboard to the widget with tree id `node` ([`tree`]: 1 is
/// the window, its agent nodes follow from 2; labels after those take
/// no keyboard). Whether anything took it.
fn focus_node(cx: &mut WindowCx, node: u64) -> bool {
    // (Its nodes as the tree said them: what changed shown first.)
    cx.flush_now();
    node >= 2 && cx.focus_agent_node((node - 2) as usize)
}

/// A window's menus, flattened for the command palette: the window
/// app_server names by its surface, else the one with the keyboard, else
/// the first that has any.
fn menus(surface: Option<u32>) -> Menus {
    let app_id = olo_interface::app_id().to_owned();
    let mut found = olo_interface::ask_windows(WAIT, move |cx| {
        if surface.is_some_and(|s| cx.surface_id() != Some(s)) {
            return None;
        }
        let menus = crate::menu::window_menus(cx);
        (!menus.is_empty()).then(|| (cx.is_window_active(), menus))
    });
    found.sort_by_key(|(active, _)| !*active);
    let entries = found.into_iter().next().map(|(_, menus)| menus.iter().flat_map(|menu| menu.entries(&app_id).entries).collect()).unwrap_or_default();
    Menus { app_id, entries }
}

/// Gives `command` to the window in front among those that answered.
fn invoke(mut windows: Vec<(bool, olo_interface::WindowId, String)>, _surface: Option<u32>) -> bool {
    windows.sort_by_key(|(active, ..)| !*active);
    let Some((_, id, command)) = windows.into_iter().next() else { return false };
    olo_interface::ask_windows(WAIT, move |cx| {
        (cx.window_id() == id).then(|| {
            INVOKING.with(|i| i.set(true));
            // (About, Keyboard Shortcuts, a settings item: the kit's own.)
            let taken = crate::app_menu::run(&command, cx) || cx.command(&command);
            INVOKING.with(|i| i.set(false));
            taken
        })
    })
    .into_iter()
    .next()
    .unwrap_or(false)
}

thread_local! {
    /// The command this looper is running came through the commands
    /// socket ([`invoking`]).
    static INVOKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the menu command being run now (`View::command`, on this
/// looper) was asked for through the commands socket (an agent's
/// `invoke`, the command palette), not chosen by a hand on this display
/// (a click, a key). For the few commands a program must not give itself:
/// Terminal's Auto Approve, which answers its agent's questions for it,
/// asks the person first when it's turned on this way.
pub fn invoking() -> bool {
    INVOKING.with(|i| i.get())
}

/// Starts answering on the app's socket.
pub(crate) fn init() {
    // (Kept for as long as the program runs: dropping it removes the
    // socket.)
    if let Some(listening) = commands::listen(answer) {
        std::mem::forget(listening);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_interface::{point, size};

    /// A node named by what shows of a cut text reads whole; one named
    /// whole already marks the cut named; a cut no node names is left
    /// for a label of its own.
    #[test]
    fn cut_text_read_whole() {
        let b = |x: f32, w: f32| Bounds { origin: point(px(x), px(0.0)), size: size(px(w), px(20.0)) };
        let mut cuts = vec![(b(0.0, 100.0), SharedString::from("Quarterly report 2026.pdf"), false), (b(200.0, 100.0), SharedString::from("Unclaimed and long"), false)];
        assert_eq!(whole_name(Some("Quarterly rep…".into()), b(0.0, 100.0), &mut cuts).as_deref(), Some("Quarterly report 2026.pdf"));
        assert!(cuts[0].2 && !cuts[1].2);
        // Elsewhere, or another text: as it was.
        assert_eq!(whole_name(Some("Quarterly rep…".into()), b(400.0, 50.0), &mut cuts).as_deref(), Some("Quarterly rep…"));
        assert_eq!(whole_name(Some("Other…".into()), b(200.0, 100.0), &mut cuts).as_deref(), Some("Other…"));
        assert!(!cuts[1].2);
        assert_eq!(whole_name(Some("Unclaimed and long".into()), b(210.0, 10.0), &mut cuts).as_deref(), Some("Unclaimed and long"));
        assert!(cuts[1].2);
    }
}
