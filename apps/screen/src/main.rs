//! Agent Screens (`tabula-screen`): screens of their own for agents, which
//! the person watches and can take over. With a command (below) it is the
//! agents' tool; without one it opens the window that shows the screens.
//! See `screens.rs` for what a screen is and HACKING.md, "Agent Screens".

mod screens;
mod viewer;

use std::path::PathBuf;

use serde_json::json;
use tabula_kit::instance;
use tabula_protocol::{ImageFormat, SyntheticEvent};

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
       tabula-screen windows NAME            its windows (id, app, title, focused, frame), as JSON
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
