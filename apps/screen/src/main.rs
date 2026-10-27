//! Agent Screens (`tabula-screen`): screens of their own for agents, which
//! the person watches and can take over. With a command (below) it is the
//! agents' tool; without one it opens the window that shows the screens.
//! See `screens.rs` for what a screen is and HACKING.md, "Agent Screens".

mod screens;
mod viewer;

use std::path::PathBuf;

use serde::Serialize;
use serde_json::json;
use tabula_kit::{commands, instance};
use tabula_protocol::{Frame, ImageFormat, SyntheticEvent, WindowInfo};

const APP_ID: &str = "tabula.screen";

const USAGE: &str = "\
usage: tabula-screen                         open Agent Screens (watch, take control)
       tabula-screen watch [NAME]            the same, showing NAME
       tabula-screen start [NAME] [--size WxH] [--scale N] [--sound]
                                             start a screen (default: agent, 1440x900, 1x, silent);
                                             prints it as JSON
       tabula-screen list                    the screens, as JSON
       tabula-screen stop NAME               end a screen and everything on it
       tabula-screen env NAME                its environment, for a shell (eval \"$(…)\")
       tabula-screen run NAME PROGRAM [ARGS…]
                                             start a program on it; prints its pid
       tabula-screen look NAME OUT.png [--window ID] [--no-pointer]
                                             a picture of the screen (or one window)
       tabula-screen windows NAME            its windows (id, app, title, focused, frame, content), as JSON
       tabula-screen tree NAME [--window ID] [--json]
                                             what Tabula apps' windows show: buttons, fields,
                                             text, lists, with where they are on the screen
       tabula-screen menus NAME [--window ID]
                                             the menus of a window's app (default: the focused one)
       tabula-screen invoke NAME COMMAND [--window ID]
                                             choose a menu command (as `menus` lists it)
       tabula-screen click NAME X Y [--right|--middle] [--double]
       tabula-screen move NAME X Y
       tabula-screen down NAME X Y [--right] | up NAME [--right]
       tabula-screen drag NAME X1 Y1 X2 Y2
       tabula-screen scroll NAME DX DY       by pixels, as on a touchpad (positive: down/right)
       tabula-screen wheel NAME NOTCHES [H-NOTCHES]
       tabula-screen key NAME CHORD…         e.g. Cmd+S, Ctrl+Shift+T, Enter, Esc, Tab, Down
       tabula-screen type NAME TEXT          type text (what a US keyboard types)

Coordinates are the screen's logical pixels, from its top left; Cmd is ⌘
(apps see it as Ctrl). Programs run on a screen with WAYLAND_DISPLAY and
TABULA_SOCKET set to it; a screen's own Deskbar and desktop are there too.";

fn fail(message: impl std::fmt::Display) -> i32 {
    eprintln!("tabula-screen: {message}");
    1
}

fn number(s: Option<&String>, what: &str) -> Result<f64, String> {
    s.ok_or_else(|| format!("missing {what}"))?.parse().map_err(|_| format!("{what} must be a number"))
}

/// The value after `flag`, if given.
fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1))
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn button(args: &[String]) -> u32 {
    if has(args, "--right") {
        screens::RIGHT
    } else if has(args, "--middle") {
        screens::MIDDLE
    } else {
        screens::LEFT
    }
}

/// Run a command; its exit code, or None for the window.
fn cli(args: &[String]) -> Option<Result<(), String>> {
    let command = args.first()?.as_str();
    let rest = &args[1..];
    let name = || rest.first().cloned().ok_or_else(|| format!("{command} needs a screen's name"));
    let screen = || screens::get(&name()?);
    let input = |events: Vec<SyntheticEvent>| screen()?.input(events);
    Some(match command {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "watch" => return None,
        "start" => (|| {
            let mut opts = screens::Options::default();
            if let Some(size) = option(rest, "--size") {
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                opts.width = w.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
                opts.height = h.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
            }
            if let Some(scale) = option(rest, "--scale") {
                opts.scale = scale.parse().map_err(|_| "--scale is a number")?;
            }
            opts.sound = has(rest, "--sound");
            let given = rest.first().filter(|a| !a.starts_with("--")).cloned();
            let screen = screens::start(&given.unwrap_or_else(screens::free_name), &opts)?;
            println!("{}", serde_json::to_string_pretty(&screen).unwrap_or_default());
            Ok(())
        })(),
        "list" => {
            println!("{}", serde_json::to_string_pretty(&screens::list()).unwrap_or_default());
            Ok(())
        }
        "stop" => name().and_then(|n| screens::stop(&n)),
        "env" => screen().map(|s| {
            for (k, v) in s.env() {
                println!("export {k}={}", shell_quote(&v));
            }
        }),
        "run" => screen().and_then(|s| {
            let pid = s.run(&rest[1..])?;
            println!("{pid}");
            Ok(())
        }),
        "look" => screen().and_then(|s| {
            let out = rest.get(1).filter(|a| !a.starts_with("--")).ok_or("look needs a file to write")?;
            let window = option(rest, "--window").map(|w| w.parse::<u64>().map_err(|_| "--window is a window's id")).transpose()?;
            let out = std::path::absolute(PathBuf::from(out)).map_err(|e| e.to_string())?;
            s.picture(&out, window, !has(rest, "--no-pointer"), ImageFormat::Png)?;
            println!("{}", out.display());
            Ok(())
        }),
        "windows" => screen().and_then(|s| {
            println!("{}", serde_json::to_string_pretty(&json!(s.windows()?)).unwrap_or_default());
            Ok(())
        }),
        "tree" => screen().and_then(|s| {
            let only = window_arg(rest)?;
            let trees = trees(&s, only)?;
            if has(rest, "--json") {
                println!("{}", serde_json::to_string_pretty(&trees).unwrap_or_default());
            } else {
                print!("{}", outline(&trees));
            }
            Ok(())
        }),
        "menus" => screen().and_then(|s| {
            let w = app_window(&s, window_arg(rest)?)?;
            let menus = commands::menus(pid_of(&w)?).ok_or_else(|| format!("{} didn't give its menus", w.app_id))?;
            println!("{}", serde_json::to_string_pretty(&menus).unwrap_or_default());
            Ok(())
        }),
        "invoke" => screen().and_then(|s| {
            let command = rest.get(1).filter(|a| !a.starts_with("--")).ok_or("invoke needs a menu command (see `menus`)")?;
            let w = app_window(&s, window_arg(rest)?)?;
            if commands::invoke(pid_of(&w)?, command) { Ok(()) } else { Err(format!("{} has no command {command:?} that can be chosen now", w.app_id)) }
        }),
        "click" => (|| {
            let (x, y) = (number(rest.get(1), "x")?, number(rest.get(2), "y")?);
            input(screens::click(x, y, button(rest), if has(rest, "--double") { 2 } else { 1 }))
        })(),
        "move" => (|| input(vec![SyntheticEvent::Move { x: number(rest.get(1), "x")?, y: number(rest.get(2), "y")? }]))(),
        "down" => (|| {
            let (x, y) = (number(rest.get(1), "x")?, number(rest.get(2), "y")?);
            input(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button: button(rest), pressed: true }])
        })(),
        "up" => input(vec![SyntheticEvent::Button { button: button(rest), pressed: false }]),
        "drag" => (|| {
            let n = |i: usize, w: &str| number(rest.get(i), w);
            input(screens::drag((n(1, "x1")?, n(2, "y1")?), (n(3, "x2")?, n(4, "y2")?)))
        })(),
        "scroll" => (|| {
            let (dx, dy) = (number(rest.get(1), "dx")?, number(rest.get(2), "dy")?);
            // The fingers lift after, so nothing glides on.
            input(vec![SyntheticEvent::Scroll { dx, dy }, SyntheticEvent::Scroll { dx: 0.0, dy: 0.0 }])
        })(),
        "wheel" => (|| {
            let dy = number(rest.get(1), "notches")? as i32;
            let dx = rest.get(2).map(|_| number(rest.get(2), "horizontal notches")).transpose()?.unwrap_or(0.0) as i32;
            input(vec![SyntheticEvent::Wheel { dx, dy }])
        })(),
        "key" => (|| {
            let chords = rest.get(1..).filter(|c| !c.is_empty()).ok_or("key needs a chord, such as Cmd+S")?;
            let mut events = Vec::new();
            for c in chords {
                events.extend(screens::chord(&screens::parse_chord(c)?));
            }
            input(events)
        })(),
        "type" => (|| input(screens::type_text(&rest.get(1..).filter(|t| !t.is_empty()).ok_or("type needs text")?.join(" "))?))(),
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    })
}

fn window_arg(args: &[String]) -> Result<Option<u64>, String> {
    option(args, "--window").map(|w| w.parse::<u64>().map_err(|_| "--window is a window's id".to_owned())).transpose()
}

fn pid_of(w: &WindowInfo) -> Result<u32, String> {
    w.pid.map(|p| p as u32).ok_or_else(|| format!("window {} has no process", w.id))
}

/// Window `id`, or the focused one.
fn app_window(s: &screens::Screen, id: Option<u64>) -> Result<WindowInfo, String> {
    let windows = s.windows()?;
    match id {
        Some(id) => windows.into_iter().find(|w| w.id == id).ok_or_else(|| format!("no window {id}")),
        None => windows.into_iter().find(|w| w.focused).ok_or_else(|| "no window has the focus; give --window".to_owned()),
    }
}

/// One window's tree, its nodes placed on the screen.
#[derive(Serialize)]
struct ScreenTree {
    window: u64,
    app_id: String,
    title: String,
    focused: bool,
    /// Where its content is on the screen.
    content: Frame,
    /// In tree order, a node's children after it; x and y are the screen's.
    nodes: Vec<commands::UiNode>,
}

/// What the windows on `s` (or just window `only`) show. Windows of apps
/// not built on the kit, or minimized, have none.
fn trees(s: &screens::Screen, only: Option<u64>) -> Result<Vec<ScreenTree>, String> {
    let windows: Vec<WindowInfo> = s.windows()?.into_iter().filter(|w| only.is_none_or(|id| w.id == id)).collect();
    if let Some(id) = only.filter(|_| windows.is_empty()) {
        return Err(format!("no window {id}"));
    }
    // One question per app, all at once: each can take a frame or two.
    let mut pids: Vec<u32> = windows.iter().filter(|w| w.content.is_some()).filter_map(|w| w.pid.map(|p| p as u32)).collect();
    pids.sort_unstable();
    pids.dedup();
    let asked: Vec<_> = pids.iter().map(|&pid| (pid, std::thread::spawn(move || commands::tree(pid)))).collect();
    let mut answers: Vec<(u32, Vec<commands::WindowTree>)> = asked.into_iter().map(|(pid, t)| (pid, t.join().ok().flatten().unwrap_or_default())).collect();
    let mut out = Vec::new();
    for w in windows {
        let (Some(content), Some(pid)) = (w.content, w.pid) else { continue };
        let Some((_, mine)) = answers.iter_mut().find(|(p, _)| *p == pid as u32) else { continue };
        // An app's windows are told apart by title (and by size, if two share one).
        let Some(i) = mine.iter().position(|t| t.title == w.title && t.width as i32 == content.width && t.height as i32 == content.height)
            .or_else(|| mine.iter().position(|t| t.title == w.title))
        else {
            continue;
        };
        let tree = mine.remove(i);
        let nodes = tree.nodes.into_iter().map(|mut n| {
            n.x += content.x as f32;
            n.y += content.y as f32;
            n
        });
        out.push(ScreenTree { window: w.id, app_id: w.app_id, title: w.title, focused: w.focused, content, nodes: nodes.collect() });
    }
    Ok(out)
}

/// The trees as an indented outline, one node a line.
fn outline(trees: &[ScreenTree]) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    for t in trees {
        let c = t.content;
        let _ = writeln!(o, "window {} {:?} ({}){} content {},{} {}x{}", t.window, t.title, t.app_id, if t.focused { " focused" } else { "" }, c.x, c.y, c.width, c.height);
        let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for n in &t.nodes {
            let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
            depth.insert(n.id, d);
            // Containers without a name say nothing an agent needs, and a
            // text run repeats its field's value.
            if n.role == "TextRun" || n.name.is_none() && n.value.is_none() && matches!(n.role.as_str(), "GenericContainer" | "Window" | "Pane" | "Group" | "ScrollView") {
                continue;
            }
            let _ = write!(o, "{:indent$}{}", "", n.role, indent = 2 + 2 * d.min(8));
            if let Some(name) = &n.name {
                let _ = write!(o, " {name:?}");
            }
            if let Some(value) = &n.value {
                let _ = write!(o, " value={value:?}");
            }
            let _ = write!(o, " at {:.0},{:.0} {:.0}x{:.0}", n.x, n.y, n.width, n.height);
            for (on, word) in [(!n.enabled, "disabled"), (n.clickable && n.role != "Clickable", "clickable"), (n.focused, "focused"), (n.checked == Some(true), "checked"), (n.selected == Some(true), "selected")] {
                if on {
                    let _ = write!(o, " {word}");
                }
            }
            o.push('\n');
        }
    }
    o
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = cli(&args) {
        std::process::exit(match result {
            Ok(()) => 0,
            Err(e) => fail(e),
        });
    }
    let show = args.get(1).cloned();
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, move |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut viewer = viewer::Viewer::new(cx);
        viewer.show(cx, show);
        viewer
    });
}
