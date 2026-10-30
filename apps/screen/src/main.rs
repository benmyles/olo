//! Agent Screens (`olo-screen`): screens of their own for agents, which
//! the person watches and can take over. With a command (below) it is the
//! agents' tool; without one it opens the window that shows the screens.
//! See `screens.rs` for what a screen is and HACKING.md, "Agent Screens".

mod find;
mod history;
mod record;
mod replay;
mod screens;
mod viewer;

use std::{path::PathBuf, time::Duration};

use serde::Serialize;
use serde_json::json;
use olo_kit::{commands, instance};
use olo_protocol::{Frame, ImageFormat, Reply, Request, SyntheticEvent, WindowInfo};

const APP_ID: &str = "olo.screen";

const USAGE: &str = "\
usage: olo-screen                         open Agent Screens (watch, take control)
       olo-screen watch [NAME]            the same, showing NAME
       olo-screen start [NAME] [--size WxH] [--scale N] [--sound]
                                             start a screen (default: agent, 1440x900, 1x, silent);
                                             prints it as JSON
       olo-screen list                    the screens, as JSON
       olo-screen stop NAME               end a screen and everything on it
       olo-screen history NAME [--json] [--kind KIND] [--search TEXT] [--last N] [--clear]
                                             what was done on it: each action, what it matched, how it went
       olo-screen history NAME --look N [OUT.png]
                                             the screen as it was when action N (from 1) ended
       olo-screen stats [NAME] [--json]   when each screen was started and last used, how long it
                                             ran and was in use, its actions by kind, its programs
       olo-screen env NAME                its environment, for a shell (eval \"$(…)\")
       olo-screen run NAME PROGRAM [ARGS…]
                                             start a program on it; prints its pid
       olo-screen look NAME [OUT.png] [--window ID] [--crop X,Y,W,H] [--no-pointer]
                                             a picture of the screen (or one window, or part);
                                             without OUT, a new file by the logs; prints its path
       olo-screen windows NAME            its windows (id, app, title, focused, frame, content,
                                             popups: open menus' frames), as JSON
       olo-screen tree NAME [--window ID] [--json] [--role R] [--contains TEXT]
                          [--visible-only] [--depth N] [--max-len N] [--cells]
                                             what Olo apps' windows show: buttons, fields,
                                             text, lists (row N/M), open menus, with where they
                                             are on the screen
       olo-screen menus NAME [--window ID]
                                             the menus of a window's app (default: the focused one)
       olo-screen invoke NAME COMMAND [--window ID]
                                             choose a menu command (as `menus` lists it)
       olo-screen review NAME [--window ID] [--json]
                                             the layout review of Olo apps' windows as drawn
                                             now: errors (blurry, seams), then what almost
                                             lines up or sits off-centre, to look at closely
       olo-screen focus|close|minimize|zoom NAME [--window ID]
       olo-screen resize NAME WxH [--window ID]    its content's size
       olo-screen place NAME X Y [WxH] [--window ID]
                                             its frame's top left (and content's size)
       olo-screen click NAME X Y [--right|--middle] [--double]
       olo-screen click NAME FIND… [--right|--middle] [--double]
                                             the middle of what FIND matches (below)
       olo-screen set NAME FIND… TEXT     click into a field, replace its text
       olo-screen select NAME FIND… ITEM  open a dropdown, choose ITEM from its menu
       olo-screen wait NAME [FIND…] [--gone] [--idle] [--timeout MS]
                                             until it shows (or goes, or the windows stop changing)
       olo-screen scroll-to NAME FIND… [--at X,Y]
                                             scroll the list (under X,Y) until it shows
       olo-screen move NAME X Y
       olo-screen down NAME X Y [--right] | up NAME [--right]
       olo-screen drag NAME X1 Y1 X2 Y2 [--steps N] [--duration MS]
                                             press, move there in N steps (20) over MS (300), let go
       olo-screen scroll NAME DX DY [--no-end]
                                             by pixels, as on a touchpad (positive: down/right);
                                             the fingers lift after, unless --no-end
       olo-screen wheel NAME NOTCHES [H-NOTCHES]
                                             mouse-wheel notches (positive: down/right), one at a time
       olo-screen key NAME CHORD…         e.g. Cmd+S, Ctrl+Shift+T, Enter, Esc, Tab, Down
       olo-screen type NAME TEXT          type text (what a US keyboard types)

FIND is --name NAME (exact, else part of it; a value counts too), --role ROLE,
--match ROLE:TEXT, --text TEXT (in the name or value), --index N (from 0, when
several match) and --window ID. Actions print JSON: what they matched, and
after: the focused window and element.

Coordinates are the screen's logical pixels, from its top left; Cmd is ⌘
(apps see it as Ctrl). Programs run on a screen with WAYLAND_DISPLAY and
OLO_SOCKET set to it; a screen's own Deskbar and desktop are there too.";

fn fail(message: impl std::fmt::Display) -> i32 {
    eprintln!("olo-screen: {message}");
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

thread_local! {
    /// What the command printed last (its answer), for its history.
    static ANSWER: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
}

/// Run a command; its exit code, or None for the window. An action on a
/// screen goes in that screen's history (`history`), with what it
/// answered and how it went.
fn cli(args: &[String]) -> Option<Result<(), String>> {
    let command = args.first()?.clone();
    let at = history::now_ms();
    ANSWER.with(|a| a.borrow_mut().take());
    let result = run_command(args)?;
    if history::ACTIONS.contains(&command.as_str()) {
        let rest = &args[1..];
        let answer = ANSWER.with(|a| a.borrow_mut().take());
        // The screen: as named, or the one `start` chose.
        let named = rest.first().filter(|a| !a.starts_with("--")).cloned();
        let name = if command == "start" { answer.as_ref().and_then(|v| v["name"].as_str().map(str::to_owned)).or(named.clone()) } else { named.clone() };
        if let Some(name) = name {
            let args = if named.is_some() { rest[1..].to_vec() } else { rest.to_vec() };
            let action = history::Action { at, ms: history::now_ms().saturating_sub(at), kind: command, args, result: answer, error: result.as_ref().err().cloned() };
            history::record(&name, &action);
        }
    }
    Some(result)
}

/// Keep `v` as the command's answer, for its history.
fn answered(v: serde_json::Value) {
    ANSWER.with(|a| *a.borrow_mut() = Some(v));
}

fn run_command(args: &[String]) -> Option<Result<(), String>> {
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
            print_json(&json!(screen));
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
            answered(json!({ "pid": pid }));
            Ok(())
        }),
        // (Started with each screen: its frames for its history.)
        "record" => name().and_then(|n| record::run(&n)),
        "history" => (|| {
            let n = name()?;
            if has(rest, "--clear") {
                return history::clear(&n);
            }
            // The screen as it was when action N (from 1, as listed)
            // ended, as a picture.
            if let Some(n) = option(rest, "--look") {
                let n: usize = n.parse().map_err(|_| "--look is an action's number, from 1")?;
                let all = history::read(&name()?);
                let a = all.get(n.wrapping_sub(1)).ok_or_else(|| format!("there are {} actions", all.len()))?;
                let (when, frame) = record::frame_at(&record::index(&name()?), a.at + a.ms).ok_or("no frames were kept from then")?;
                let out = match rest.iter().skip_while(|a| *a != "--look").nth(2).filter(|a| !a.starts_with("--")) {
                    Some(o) => std::path::absolute(PathBuf::from(o)).map_err(|e| e.to_string())?,
                    None => screens::dir().join(format!("{}-history-{n}.png", name()?)),
                };
                let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width as u32, frame.height as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header().and_then(|mut w| w.write_image_data(&frame.rgba)).map_err(|e| e.to_string())?;
                print_json(&json!({ "path": out, "action": a, "frame_at": when }));
                return Ok(());
            }
            let kind = option(rest, "--kind");
            let search = option(rest, "--search");
            let mut actions: Vec<history::Action> = history::read(&n).into_iter().filter(|a| kind.is_none_or(|k| &a.kind == k) && search.is_none_or(|t| a.mentions(t))).collect();
            if let Some(last) = option(rest, "--last") {
                let last: usize = last.parse().map_err(|_| "--last is a number")?;
                actions.drain(..actions.len().saturating_sub(last));
            }
            if has(rest, "--json") {
                print_json(&json!(actions));
            } else if actions.is_empty() {
                println!("Nothing done on {n} yet.");
            } else {
                for a in &actions {
                    println!("{}", a.line());
                }
            }
            Ok(())
        })(),
        "stats" => {
            let running = screens::list();
            let mut names = history::names();
            for s in &running {
                if !names.contains(&s.name) {
                    names.push(s.name.clone());
                }
            }
            if let Some(n) = rest.first().filter(|a| !a.starts_with("--")) {
                names.retain(|x| x == n);
                if names.is_empty() {
                    names.push(n.clone());
                }
            }
            let now = history::now_ms();
            let all: Vec<serde_json::Value> = names
                .iter()
                .map(|n| {
                    let up = running.iter().find(|s| &s.name == n);
                    let st = history::stats(n, &history::read(n), up.is_some(), now);
                    let windows = up.and_then(|s| s.windows().ok()).map(|w| w.len());
                    let mut v = json!(st);
                    v["running"] = json!(up.is_some());
                    v["windows"] = json!(windows);
                    v
                })
                .collect();
            if has(rest, "--json") {
                print_json(&json!(all));
            } else {
                for v in &all {
                    println!("{}", stats_text(v));
                }
            }
            Ok(())
        }
        "look" => screen().and_then(|s| {
            // Where to: as given, or a new file beside the screen's logs.
            let out = match positionals(rest).get(1) {
                Some(out) => std::path::absolute(PathBuf::from(out)).map_err(|e| e.to_string())?,
                None => {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                    screens::dir().join(format!("{}-look-{ms}.png", s.name))
                }
            };
            let window = window_arg(rest)?;
            let pointer = !has(rest, "--no-pointer");
            match option(rest, "--crop") {
                None => s.picture(&out, window, pointer, ImageFormat::Png)?,
                Some(crop) => {
                    let n: Vec<f64> = crop.split(',').map(|v| v.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| "--crop is X,Y,W,H")?;
                    let [x, y, w, h] = n[..] else { return Err("--crop is X,Y,W,H".into()) };
                    crop_picture(&s, &out, window, pointer, (x, y, w, h))?;
                }
            }
            println!("{}", out.display());
            Ok(())
        }),
        "windows" => screen().and_then(|s| {
            println!("{}", serde_json::to_string_pretty(&json!(s.windows()?)).unwrap_or_default());
            Ok(())
        }),
        "tree" => screen().and_then(|s| {
            let only = window_arg(rest)?;
            let mut trees = trees(&s, only)?;
            let filter = Filter::from_args(rest)?;
            if has(rest, "--json") {
                for t in &mut trees {
                    let c = t.content;
                    let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
                    t.nodes.retain(|n| {
                        let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
                        depth.insert(n.id, d);
                        filter.keeps(n, d, c)
                    });
                }
                println!("{}", serde_json::to_string_pretty(&trees).unwrap_or_default());
            } else {
                print!("{}", outline(&trees, &filter));
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
        "review" => screen().and_then(|s| {
            let only = window_arg(rest)?;
            let windows: Vec<WindowInfo> = s.windows()?.into_iter().filter(|w| w.content.is_some() && only.is_none_or(|id| w.id == id)).collect();
            if let Some(id) = only.filter(|_| windows.is_empty()) {
                return Err(format!("no window {id}"));
            }
            // One question per app, all at once.
            let mut pids: Vec<u32> = windows.iter().filter_map(|w| w.pid.map(|p| p as u32)).collect();
            pids.sort_unstable();
            pids.dedup();
            let asked: Vec<_> = pids.into_iter().map(|pid| std::thread::spawn(move || commands::review(pid))).collect();
            let reviews: Vec<commands::WindowReview> = asked
                .into_iter()
                .flat_map(|r| r.join().ok().flatten().unwrap_or_default())
                .filter(|r| r.layer.is_none() && windows.iter().any(|w| w.title == r.title))
                .collect();
            if has(rest, "--json") {
                println!("{}", serde_json::to_string_pretty(&reviews).unwrap_or_default());
            } else if reviews.is_empty() {
                println!("no Olo windows to review");
            } else {
                for r in &reviews {
                    println!("{} ({}×{} at {}×): {} findings", r.title, r.width, r.height, r.scale, r.findings.len());
                    for f in &r.findings {
                        println!("  - {f}");
                    }
                }
            }
            Ok(())
        }),
        "click" => (|| {
            let s = screen()?;
            let (x, y, matched) = match find::Query::from_args(rest)? {
                Some(q) => {
                    let f = find::find(&s, &q)?;
                    (f.center.0 as f64, f.center.1 as f64, Some(f))
                }
                None => (number(rest.get(1), "x")?, number(rest.get(2), "y")?, None),
            };
            s.input(screens::click(x, y, button(rest), if has(rest, "--double") { 2 } else { 1 }))?;
            print_json(&json!({ "clicked": [x, y], "matched": matched, "after": find::after(&s) }));
            Ok(())
        })(),
        "move" => (|| input(vec![SyntheticEvent::Move { x: number(rest.get(1), "x")?, y: number(rest.get(2), "y")? }]))(),
        "down" => (|| {
            let (x, y) = (number(rest.get(1), "x")?, number(rest.get(2), "y")?);
            input(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button: button(rest), pressed: true }])
        })(),
        "up" => input(vec![SyntheticEvent::Button { button: button(rest), pressed: false }]),
        "drag" => (|| {
            let n = |i: usize, w: &str| number(rest.get(i), w);
            let steps = option(rest, "--steps").map(|s| s.parse::<u32>().map_err(|_| "--steps is a number")).transpose()?.unwrap_or(20);
            let ms = option(rest, "--duration").map(|s| s.parse::<u64>().map_err(|_| "--duration is milliseconds")).transpose()?.unwrap_or(300);
            screen()?.input_paced(screens::drag((n(1, "x1")?, n(2, "y1")?), (n(3, "x2")?, n(4, "y2")?), steps, Duration::from_millis(ms)))
        })(),
        "scroll" => (|| {
            let (dx, dy) = (number(rest.get(1), "dx")?, number(rest.get(2), "dy")?);
            screen()?.input_paced(screens::scroll(dx, dy, !has(rest, "--no-end")))
        })(),
        "wheel" => (|| {
            let dy = number(rest.get(1), "notches")? as i32;
            let dx = rest.get(2).filter(|a| !a.starts_with("--")).map(|_| number(rest.get(2), "horizontal notches")).transpose()?.unwrap_or(0.0) as i32;
            screen()?.input_paced(screens::wheel(dx, dy))
        })(),
        "key" => (|| {
            let chords = rest.get(1..).filter(|c| !c.is_empty()).ok_or("key needs a chord, such as Cmd+S")?;
            let mut events = Vec::new();
            for c in chords {
                events.extend(screens::chord(&screens::parse_chord(c)?));
            }
            let s = screen()?;
            s.input(events)?;
            print_json(&json!({ "after": find::after(&s) }));
            Ok(())
        })(),
        "type" => (|| {
            let s = screen()?;
            s.input(screens::type_text(&rest.get(1..).filter(|t| !t.is_empty()).ok_or("type needs text")?.join(" "))?)?;
            let after = find::after(&s);
            warn_if_not_text(&after);
            print_json(&json!({ "after": after }));
            Ok(())
        })(),
        "set" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("set needs the field: --name, --role or --match")?;
            let text = positionals(rest).get(1).cloned().ok_or("set needs the text to put in the field")?;
            let f = find::find(&s, &q)?;
            // Into the field (once more if the first click went astray:
            // the keys below would edit whatever has the focus).
            let into = |s: &screens::Screen| find::after(s).focused.is_some_and(|now| now.window == f.window && now.x == f.x && now.y == f.y);
            let mut focused = false;
            for _ in 0..2 {
                s.input(screens::click(f.center.0 as f64, f.center.1 as f64, screens::LEFT, 1))?;
                focused = into(&s);
                if focused {
                    break;
                }
            }
            if !focused && find::takes_text(&f.role) {
                return Err(format!("clicked the {} {:?} twice but it didn't take the focus; nothing typed", f.role, f.name.as_deref().unwrap_or("")));
            }
            let mut events = screens::chord(&screens::parse_chord("Ctrl+A")?);
            events.extend(screens::chord(&screens::parse_chord("Delete")?));
            events.extend(screens::type_text(&text)?);
            s.input(events)?;
            let after = find::after(&s);
            warn_if_not_text(&after);
            print_json(&json!({ "field": f, "after": after }));
            Ok(())
        })(),
        "select" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("select needs the dropdown: --name, --role or --match")?;
            let item = positionals(rest).get(1).cloned().ok_or("select needs the item to choose")?;
            let field = find::find(&s, &q)?;
            s.input(screens::click(field.center.0 as f64, field.center.1 as f64, screens::LEFT, 1))?;
            // Its menu opens in a frame or two; then choose the item.
            let want = find::Query { name: Some(item.clone()), role: Some("MenuItem".into()), window: Some(field.window), ..Default::default() };
            let chosen = match find::wait_for(&s, &want, false, Duration::from_secs(3)) {
                Ok(Some(f)) => f,
                _ => {
                    s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    return Err(format!("its menu has no item {item:?} (or didn't open)"));
                }
            };
            s.input(screens::click(chosen.center.0 as f64, chosen.center.1 as f64, screens::LEFT, 1))?;
            print_json(&json!({ "field": field, "chose": chosen, "after": find::after(&s) }));
            Ok(())
        })(),
        "wait" => (|| {
            let s = screen()?;
            let ms = option(rest, "--timeout").map(|t| t.parse::<u64>().map_err(|_| "--timeout is milliseconds")).transpose()?.unwrap_or(10_000);
            let timeout = Duration::from_millis(ms);
            if has(rest, "--idle") {
                find::wait_idle(&s, window_arg(rest)?, timeout)?;
            }
            if let Some(q) = find::Query::from_args(rest)? {
                let found = find::wait_for(&s, &q, has(rest, "--gone"), timeout)?;
                print_json(&json!({ "found": found }));
            } else if !has(rest, "--idle") {
                return Err("wait for what? --idle, --name, --role, --match or --text (with --gone, until it goes)".into());
            }
            Ok(())
        })(),
        "scroll-to" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("scroll-to needs what to find: --name, --role, --match or --text")?;
            let at = option(rest, "--at")
                .map(|a| {
                    let (x, y) = a.split_once(',').ok_or("--at is X,Y")?;
                    Ok::<_, String>((x.trim().parse::<f64>().map_err(|_| "--at is X,Y")?, y.trim().parse::<f64>().map_err(|_| "--at is X,Y")?))
                })
                .transpose()?;
            let f = scroll_to(&s, &q, at)?;
            print_json(&json!({ "found": f }));
            Ok(())
        })(),
        "focus" | "close" | "minimize" | "zoom" | "maximize" => (|| {
            let s = screen()?;
            let w = app_window(&s, window_arg(rest)?)?;
            let id = w.id;
            let request = match command {
                "focus" => Request::Activate { id },
                "close" => Request::Close { id },
                "minimize" => Request::Minimize { id },
                _ => Request::ToggleZoom { id },
            };
            ok(s.request(&request)?)?;
            print_json(&json!({ "window": id, "after": find::after(&s) }));
            Ok(())
        })(),
        "resize" | "place" => (|| {
            let s = screen()?;
            let w = app_window(&s, window_arg(rest)?)?;
            let (Some(frame), Some(content)) = (w.frame, w.content) else { return Err(format!("window {} isn't showing", w.id)) };
            let args = positionals(rest);
            let size = |a: Option<&String>| -> Result<(i32, i32), String> {
                let (w, h) = a.and_then(|a| a.split_once('x')).ok_or("the size is WIDTHxHEIGHT (of what the app draws)")?;
                Ok((w.parse().map_err(|_| "the size is WIDTHxHEIGHT")?, h.parse().map_err(|_| "the size is WIDTHxHEIGHT")?))
            };
            // Sizes are the content's (what the app draws); the frame adds the tab and border.
            let (extra_w, extra_h) = (frame.width - content.width, frame.height - content.height);
            let mut to = frame;
            if command == "resize" {
                let (cw, ch) = size(args.get(1))?;
                (to.width, to.height) = (cw + extra_w, ch + extra_h);
            } else {
                (to.x, to.y) = (number(args.get(1), "x")? as i32, number(args.get(2), "y")? as i32);
                if args.get(3).is_some() {
                    let (cw, ch) = size(args.get(3))?;
                    (to.width, to.height) = (cw + extra_w, ch + extra_h);
                }
            }
            ok(s.request(&Request::Place { id: w.id, frame: to })?)?;
            std::thread::sleep(Duration::from_millis(150));
            let now = s.windows()?.into_iter().find(|x| x.id == w.id);
            print_json(&json!({ "window": now }));
            Ok(())
        })(),
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    })
}

/// The arguments that aren't flags (nor flags' values).
fn positionals(args: &[String]) -> Vec<String> {
    const VALUED: [&str; 15] = ["--at", "--name", "--role", "--match", "--text", "--index", "--window", "--timeout", "--steps", "--duration", "--crop", "--name-contains", "--contains", "--depth", "--max-len"];
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if VALUED.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with("--") || a.parse::<f64>().is_ok() {
            out.push(a.clone());
        }
    }
    out
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    answered(v.clone());
}

/// A screen's stats (`history::Stats` and whether it runs) for people.
fn stats_text(v: &serde_json::Value) -> String {
    let ms = |k: &str| v[k].as_u64().unwrap_or(0);
    let dur = |ms: u64| {
        let s = ms / 1000;
        if s >= 3600 { format!("{} h {} min", s / 3600, s / 60 % 60) } else if s >= 60 { format!("{} min {} s", s / 60, s % 60) } else { format!("{s} s") }
    };
    let when = |k: &str| v[k].as_u64().map(history::clock).unwrap_or_else(|| "never".into());
    let kinds: Vec<String> = v["actions"].as_object().map(|o| o.iter().map(|(k, n)| format!("{k} {n}")).collect()).unwrap_or_default();
    let programs: Vec<String> = v["programs"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    let state = if v["running"].as_bool().unwrap_or(false) { format!("running, {} windows", v["windows"].as_u64().unwrap_or(0)) } else { "stopped".into() };
    format!(
        "{}: {state}\n  started {}, last used {}\n  ran {}, in use {}\n  actions: {}{}\n  programs: {}",
        v["name"].as_str().unwrap_or("?"),
        when("started"),
        when("last_used"),
        dur(ms("ran_ms")),
        dur(ms("active_ms")),
        if kinds.is_empty() { "none".into() } else { kinds.join(", ") },
        match v["failed"].as_u64() { Some(n) if n > 0 => format!(" ({n} failed)"), _ => String::new() },
        if programs.is_empty() { "none".into() } else { programs.join(", ") },
    )
}

fn ok(reply: Reply) -> Result<(), String> {
    match reply {
        Reply::Error { message } => Err(message),
        _ => Ok(()),
    }
}

/// Typing that went somewhere that doesn't take text says so.
fn warn_if_not_text(after: &find::After) {
    match &after.focused {
        Some(f) if !find::takes_text(&f.role) => eprintln!("olo-screen: the keys went to a {} ({}), not a text field", f.role, f.name.as_deref().unwrap_or("unnamed")),
        None if after.window.is_none() => eprintln!("olo-screen: no window has the focus; the keys went nowhere"),
        _ => {}
    }
}

/// A picture of part of the screen: X,Y,W,H in the screen's logical pixels.
fn crop_picture(s: &screens::Screen, out: &std::path::Path, window: Option<u64>, pointer: bool, (x, y, w, h): (f64, f64, f64, f64)) -> Result<(), String> {
    let raw = screens::dir().join(format!("crop-{}.rgba", std::process::id()));
    s.picture(&raw, window, pointer, ImageFormat::Rgba)?;
    let bytes = std::fs::read(&raw).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&raw);
    let bytes = bytes?;
    let word = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let (Some(pw), Some(ph)) = (word(0), word(4)) else { return Err("the picture was cut short".into()) };
    let pixels = &bytes[8..];
    // A window's picture starts at the window; the screen's, at its corner.
    let origin = match window {
        Some(id) => s.windows()?.into_iter().find(|w| w.id == id).and_then(|w| w.frame).map_or((0.0, 0.0), |f| (f.x as f64, f.y as f64)),
        None => (0.0, 0.0),
    };
    let scale = if window.is_some() { s.scale } else { pw as f64 / s.width.max(1) as f64 };
    let px = |v: f64| (v * scale).round().max(0.0) as usize;
    let (x0, y0) = (px(x - origin.0).min(pw), px(y - origin.1).min(ph));
    let (x1, y1) = ((x0 + px(w)).min(pw), (y0 + px(h)).min(ph));
    if x1 <= x0 || y1 <= y0 {
        return Err("--crop is outside the picture".into());
    }
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for row in y0..y1 {
        data.extend_from_slice(pixels.get((row * pw + x0) * 4..(row * pw + x1) * 4).ok_or("the picture was cut short")?);
    }
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), (x1 - x0) as u32, (y1 - y0) as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&data)).map_err(|e| e.to_string())
}

/// Scroll the list under `at` (or the middle of the window) until `q`
/// shows whole: to the top first, then down a few notches at a time.
fn scroll_to(s: &screens::Screen, q: &find::Query, at: Option<(f64, f64)>) -> Result<find::Found, String> {
    let shown = |s: &screens::Screen| -> Result<(Option<find::Found>, String), String> {
        let trees = trees(s, q.window)?;
        Ok((find::matches(&trees, q).into_iter().nth(q.index), outline(&trees, &Filter::default())))
    };
    let whole = |f: &find::Found| f.role != "Row" || f.height >= 12.0;
    if let (Some(f), _) = shown(s)?
        && whole(&f)
    {
        return Ok(f);
    }
    let (x, y) = match at {
        Some(p) => p,
        None => {
            let w = app_window(s, q.window)?;
            let c = w.content.ok_or("the window isn't showing")?;
            ((c.x + c.width / 2) as f64, (c.y + c.height / 2) as f64)
        }
    };
    s.input(vec![SyntheticEvent::Move { x, y }])?;
    // Up to the top, then down, until it shows or the list stops moving.
    for (notches, rounds) in [(-10, 100), (3, 1000)] {
        let mut last = String::new();
        for _ in 0..rounds {
            s.input_paced(screens::wheel(0, notches))?;
            std::thread::sleep(Duration::from_millis(80));
            let (found, now) = shown(s)?;
            if let Some(f) = found
                && whole(&f)
            {
                return Ok(f);
            }
            if now == last {
                break;
            }
            last = now;
        }
    }
    Err(format!("scrolled the whole list; nothing matches {}", q.describe()))
}

fn window_arg(args: &[String]) -> Result<Option<u64>, String> {
    option(args, "--window").map(|w| w.parse::<u64>().map_err(|_| "--window is a window's id".to_owned())).transpose()
}

fn pid_of(w: &WindowInfo) -> Result<u32, String> {
    w.pid.map(|p| p as u32).ok_or_else(|| format!("window {} has no process", w.id))
}

/// Window `id`; or the focused one, or the only one showing.
fn app_window(s: &screens::Screen, id: Option<u64>) -> Result<WindowInfo, String> {
    let windows = s.windows()?;
    let listed = |windows: &[WindowInfo]| windows.iter().map(|w| format!("\n  {} {:?} ({}){}", w.id, w.title, w.app_id, if w.minimized { " minimized" } else { "" })).collect::<String>();
    match id {
        Some(id) => windows.iter().find(|w| w.id == id).cloned().ok_or_else(|| format!("no window {id}; the windows are:{}", listed(&windows))),
        None => {
            if let Some(w) = windows.iter().find(|w| w.focused) {
                return Ok(w.clone());
            }
            let showing: Vec<&WindowInfo> = windows.iter().filter(|w| !w.minimized && w.frame.is_some()).collect();
            match showing.as_slice() {
                [only] => Ok((*only).clone()),
                [] => Err("no windows are showing".to_owned()),
                _ => Err(format!("no window has the focus; give --window ID:{}", listed(&windows))),
            }
        }
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
    // Windows only: Deskbar and other layer surfaces have no window here.
    let mut answers: Vec<(u32, Vec<commands::WindowTree>)> = asked
        .into_iter()
        .map(|(pid, t)| (pid, t.join().ok().flatten().unwrap_or_default().into_iter().filter(|t| t.layer.is_none()).collect()))
        .collect();
    let mut out = Vec::new();
    for w in windows {
        let (Some(content), Some(pid)) = (w.content, w.pid) else { continue };
        let Some((_, mine)) = answers.iter_mut().find(|(p, _)| *p == pid as u32) else { continue };
        // An app's windows are told apart by title (and by size, if two share one).
        // Chrome's are named by their page ("Page - Google Chrome").
        let Some(i) = mine.iter().position(|t| t.title == w.title && t.width as i32 == content.width && t.height as i32 == content.height)
            .or_else(|| mine.iter().position(|t| t.title == w.title))
            .or_else(|| mine.iter().position(|t| !t.title.is_empty() && w.title.starts_with(&t.title) && t.width as i32 == content.width))
            .or_else(|| mine.iter().position(|t| t.width as i32 == content.width && t.height as i32 == content.height))
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

/// What `tree` shows of the trees.
#[derive(Debug, Default)]
struct Filter {
    role: Option<String>,
    /// In the name or value (any case).
    contains: Option<String>,
    /// Only what can be seen: some area, inside its window's content.
    visible_only: bool,
    depth: Option<usize>,
    /// Names and values cut to this many characters.
    max_len: Option<usize>,
    /// A table's cells (a row is named by them already).
    cells: bool,
}

impl Filter {
    fn from_args(args: &[String]) -> Result<Filter, String> {
        let n = |flag: &str| option(args, flag).map(|v| v.parse::<usize>().map_err(|_| format!("{flag} is a number"))).transpose();
        Ok(Filter {
            role: option(args, "--role").cloned(),
            contains: option(args, "--name-contains").or(option(args, "--contains")).cloned(),
            visible_only: has(args, "--visible-only"),
            depth: n("--depth")?,
            max_len: n("--max-len")?,
            cells: has(args, "--cells"),
        })
    }

    fn narrows(&self) -> bool {
        self.role.is_some() || self.contains.is_some()
    }

    /// Keep `n` (at `depth`, in a window whose content is `c`)?
    fn keeps(&self, n: &commands::UiNode, depth: usize, c: Frame) -> bool {
        if self.depth.is_some_and(|d| depth > d) {
            return false;
        }
        if self.visible_only {
            let inside = n.x < (c.x + c.width) as f32 && n.y < (c.y + c.height) as f32 && n.x + n.width > c.x as f32 && n.y + n.height > c.y as f32;
            if n.width < 1.0 || n.height < 1.0 || !inside {
                return false;
            }
        }
        if self.role.as_ref().is_some_and(|r| !n.role.eq_ignore_ascii_case(r)) {
            return false;
        }
        if let Some(t) = &self.contains {
            let t = t.to_lowercase();
            if ![&n.name, &n.value].iter().any(|s| s.as_ref().is_some_and(|s| s.to_lowercase().contains(&t))) {
                return false;
            }
        }
        true
    }

    fn cut(&self, s: &str) -> String {
        match self.max_len {
            Some(max) if s.chars().count() > max => format!("{}…", s.chars().take(max).collect::<String>()),
            _ => s.to_owned(),
        }
    }
}

/// The trees as an indented outline, one node a line.
fn outline(trees: &[ScreenTree], filter: &Filter) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    for t in trees {
        let c = t.content;
        let _ = writeln!(o, "window {} {:?} ({}){} content {},{} {}x{}", t.window, t.title, t.app_id, if t.focused { " focused" } else { "" }, c.x, c.y, c.width, c.height);
        let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        let mut last: Option<&commands::UiNode> = None;
        // A table's rows are reported each once, by name and place; their
        // cells (and the unnamed area behind them) only with --cells.
        let mut in_row: Option<(f32, f32)> = None;
        for n in &t.nodes {
            let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
            depth.insert(n.id, d);
            let repeat = last.is_some_and(|l| l.role == n.role && l.name == n.name && l.value == n.value && l.x == n.x && l.y == n.y && l.width == n.width);
            last = Some(n);
            if n.role == "Row" {
                in_row = Some((n.y, n.y + n.height));
            } else if !matches!(n.role.as_str(), "Cell" | "Unknown") {
                in_row = None;
            }
            if repeat || !filter.cells && in_row.is_some() && n.role != "Row" {
                continue;
            }
            // Containers without a name say nothing an agent needs, and a
            // text run repeats its field's value.
            // Nor does a row scrolled out of sight.
            if n.role == "Row" && (n.width == 0.0 || n.height == 0.0) {
                continue;
            }
            if n.role == "TextRun" || n.name.is_none() && n.value.is_none() && matches!(n.role.as_str(), "GenericContainer" | "Window" | "Pane" | "Group" | "ScrollView") {
                continue;
            }
            if !filter.keeps(n, d, c) {
                continue;
            }
            let indent = if filter.narrows() { 2 } else { 2 + 2 * d.min(8) };
            let _ = write!(o, "{:indent$}{}", "", n.role);
            if let Some(name) = &n.name {
                let _ = write!(o, " {:?}", filter.cut(name));
            }
            if let Some(value) = &n.value {
                let _ = write!(o, " value={:?}", filter.cut(value));
            }
            let _ = write!(o, " at {:.0},{:.0} {:.0}x{:.0}", n.x, n.y, n.width, n.height);
            if let (Some(row), Some(rows)) = (n.row, n.rows) {
                let _ = write!(o, " row {}/{rows}", row + 1);
            }
            for (on, word) in [(!n.enabled, "disabled"), (n.clickable && !matches!(n.role.as_str(), "Clickable" | "Button" | "Link" | "MenuItem" | "Row"), "clickable"), (n.focused, "focused"), (n.checked == Some(true), "checked"), (n.selected == Some(true), "selected"), (n.minimized, "minimized")] {
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
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
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
    olo_kit::run(APP_ID, move |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut viewer = viewer::Viewer::new(cx);
        viewer.show(cx, show);
        viewer
    });
}
