//! Agent Screens (`olo-screen`): screens of their own for agents, which
//! the person watches and can take over. With a command (below) it is the
//! agents' tool; without one it opens the window that shows the screens.
//! See `screens.rs` for what a screen is and HACKING.md, "Agent Screens".

mod atspi;
mod clip;
mod export;
mod find;
mod history;
mod moving;
mod picture;
mod record;
mod replay;
mod screens;
mod shares;
mod stats;
mod viewer;

use std::{path::PathBuf, time::Duration};

use olo_kit::{commands, instance};
use olo_protocol::{Frame, ImageFormat, Reply, Request, SyntheticEvent, WindowInfo};
use serde::Serialize;
use serde_json::json;

const APP_ID: &str = "olo.screen";

const USAGE: &str = "\
usage: olo-screen                         open Agent Screens (watch, take control)
       olo-screen watch [NAME]            the same, showing NAME
       olo-screen start [NAME] [--size WxH] [--scale N] [--sound]
                                             start a screen (default: agent, 1440x900, 1x, silent),
                                             or start a stopped or archived one again (as it was,
                                             unless given); prints it as JSON
       olo-screen list                    every screen, as JSON: its state (running, stopped,
                                             archived) and the name each was given
       olo-screen rename NAME [TITLE]     give a screen a name to show by (none: its own)
       olo-screen stop NAME               end a screen and everything on it; the screen is kept
                                             (stopped: its history, Chrome profile, passwords)
       olo-screen archive NAME            a stopped screen out of the way (listed as archived)
       olo-screen unarchive NAME          back among the stopped ones
       olo-screen delete NAME             a screen and everything it kept, to the Trash
       olo-screen record-video NAME [--seconds N] [--fps N] [--window ID] [--region X,Y,W,H]
                               [--show-clicks] [--show-keys] [--sound computer|microphone|both|none] [--as-gif]
                          [--no-pointer] [--pointer-size K] [--smooth-pointer] [--zoom-follow] [--background]
                          [--camera ID] [--encoder auto|cpu|nvidia] [--codec h264|hevc|av1] [--to PATH]
                                             a video of the screen (or one window, or part), into
                                             ~/Videos unless told; prints its path (JSON) at once
       olo-screen mark-video NAME TITLE   this moment of the video marked (a chapter)
       olo-screen caption-video NAME TEXT [--seconds N] | --clear
                                          a caption in the video (never on the screen), low in
                                          the middle, for N seconds or till the next
       olo-screen zoom-video NAME X,Y,W,H|out
                                          the video zoomed to that part of the screen (points),
                                          gliding there and held; out: back to the whole
       olo-screen pause-video NAME        nothing recorded till resume-video (the pause cut out)
       olo-screen resume-video NAME
       olo-screen stop-video NAME         the video finished; prints where it went
       olo-screen video-frame VIDEO [OUT.png] [--at SECONDS]
                                             the picture a video shows at a time (0: its first),
                                             to check one recorded
       olo-screen history NAME [--json] [--kind KIND] [--search TEXT] [--last N] [--clear]
                                             what was done on it: each action, what it matched, how it went
       olo-screen history NAME --look N [OUT.png]
                                             the screen as it was when action N (from 1) ended
       olo-screen history NAME --export OUT.mp4 [--from N] [--to N] [--zoom 1|2|3] [--encoder auto|cpu|nvidia|vaapi]
                                             as a video: its actions (N to N, from 1), the time
                                             between them skipped; each pixel zoomed (small
                                             screens easier to read), as far as H.264 allows
       olo-screen stats [NAME] [--json]   when each screen was started and last used, how long it
                                             ran and was in use, its actions by kind, its programs
       olo-screen env NAME                its environment, for a shell (eval \"$(…)\")
       olo-screen run NAME PROGRAM [ARGS…]
                                             start a program on it; prints its pid
       olo-screen take NAME|--new --window ID…
                                             the person's windows (ids on their display) onto the
                                             screen (--new: a new one), their programs running on
                                             as they go; only when the person asks
       olo-screen give NAME [--window ID…] [--workspace N] [--display OUTPUT]
                                             the screen's windows (default: all) to the person's
                                             display, onto its workspace N (from 1; default: the
                                             one showing) of OUTPUT (default: the active display)
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
                                             choose a menu command (as `menus` lists it,
                                             or by its label)
       olo-screen review NAME [--window ID] [--json]
                                             the layout review of Olo apps' windows as drawn
                                             now: errors (blurry, seams), then what almost
                                             lines up or sits off-centre, to look at closely
       olo-screen focus|close|minimize|zoom NAME [--window ID]
       olo-screen resize NAME WxH [--window ID]    its content's size
       olo-screen place NAME X Y [WxH] [--window ID]
                                             its frame's top left (and content's size)
       olo-screen click NAME X Y [--right|--middle] [--double]   (or X,Y, or --at X,Y)
       olo-screen click NAME FIND… [--right|--middle] [--double]
                                             the middle of what FIND matches (below)
       olo-screen set NAME FIND… TEXT     click into a field, replace its text
       olo-screen select NAME FIND… ITEM  open a dropdown, choose ITEM from its menu
                                             (a page's <select>: by its words, from its options)
       olo-screen wait NAME [FIND…] [--gone] [--idle] [--timeout MS]
                                             until it shows (or goes, or the windows stop changing)
       olo-screen wait NAME --free [--timeout MS]
                                             until the person hands the screen back (Take Control)
       olo-screen scroll-to NAME FIND…|WORDS [--at X,Y]
                                             scroll the list (under X,Y) until it shows
       olo-screen move NAME X Y
       olo-screen down NAME X Y [--right] | up NAME [--right]
       olo-screen drag NAME X1 Y1 X2 Y2 [--steps N] [--duration MS]
                                             press, move there in N steps (20) over MS (300), let go
       olo-screen scroll NAME DX DY [--no-end]       (or --dx N --dy N; --at X,Y for where)
                                             by pixels, as on a touchpad (positive: down/right);
                                             the fingers lift after, unless --no-end
       olo-screen wheel NAME NOTCHES [H-NOTCHES]
                                             mouse-wheel notches (positive: down/right), one at a time
       olo-screen key NAME CHORD…         e.g. Cmd+S, Ctrl+Shift+T, Enter, Esc, Tab, Down
       olo-screen type NAME TEXT          type text (what a US keyboard types)

While the person controls a screen (Agent Screens' Take Control), every
action on it is refused, doing nothing, until they hand it back: reading
it (windows, tree, look) still answers.

FIND is --name NAME (exact, else part of it; a value counts too), --role ROLE,
--match ROLE:TEXT, --text TEXT (in the name or value), --index N (from 0, when
several match) and --window ID. Actions print JSON: what they matched, and
after: the focused window and element.

A moved program keeps running: an Olo app re-executes there with its
state (one window of several by a copy of the app, where it can), any
other program's connection is replayed there with all its windows. The
person's programs on a screen go back to their display when it stops.

Coordinates are the screen's logical pixels, from its top left; Cmd is ⌘
(apps see it as Ctrl). Programs run on a screen with WAYLAND_DISPLAY and
OLO_SOCKET set to it; a screen's own Deskbar and desktop are there too.";

fn fail(message: impl std::fmt::Display) -> i32 {
    eprintln!("olo-screen: {message}");
    1
}

/// Every `--window ID` given.
fn window_ids(args: &[String]) -> Result<Vec<u64>, String> {
    args.windows(2).filter(|p| p[0] == "--window").map(|p| p[1].parse::<u64>().map_err(|_| format!("{} isn't a window's id", p[1]))).collect()
}

/// Why nothing moved.
fn stayed_why(o: &moving::Outcome) -> String {
    match o.stayed.first() {
        Some(s) => format!("nothing moved: {} {}", if s.app_id.is_empty() { "the window" } else { s.app_id.as_str() }, s.why),
        None => "nothing moved".into(),
    }
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
        let name = if matches!(command.as_str(), "start" | "take") { answer.as_ref().and_then(|v| v["name"].as_str().map(str::to_owned)).or(named.clone()) } else { named.clone() };
        if let Some(name) = name {
            let args = if named.is_some() { rest[1..].to_vec() } else { rest.to_vec() };
            let action = history::Action { at, ms: history::now_ms().saturating_sub(at), kind: command, args, result: answer, error: result.as_ref().err().cloned() };
            history::record(&name, &action);
            // (Its frames kept too, if what kept them has gone.)
            if !matches!(action.kind.as_str(), "start" | "stop" | "delete" | "archive") {
                screens::ensure_recorder(&name);
            }
        }
    }
    Some(result)
}

/// Keep `v` as the command's answer, for its history.
fn answered(v: serde_json::Value) {
    ANSWER.with(|a| *a.borrow_mut() = Some(v));
}

/// What an action is told while the person controls screen `name`.
fn controlled_answer(name: &str) -> String {
    format!(
        "the person is controlling screen {name} (Agent Screens' Take Control): nothing was done. Wait until they hand it back (olo-screen wait {name} --free), then look at it again before going on: they may have changed what's on it."
    )
}

fn run_command(args: &[String]) -> Option<Result<(), String>> {
    let command = args.first()?.as_str();
    let rest = &args[1..];
    let name = || rest.first().cloned().ok_or_else(|| format!("{command} needs a screen's name"));
    // While the person has the screen, an agent's actions wait for them
    // (reading it still answers): two hands on one pointer made a mess.
    if !matches!(command, "start" | "take" | "give")
        && (history::ACTIONS.contains(&command) || command == "maximize")
        && let Some(n) = rest.first()
        && screens::controlled(n)
    {
        return Some(Err(controlled_answer(n)));
    }
    let screen = || screens::get(&name()?);
    let input = |events: Vec<SyntheticEvent>| screen()?.input(events);
    Some(match command {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "watch" => return None,
        "start" => (|| {
            let given = rest.first().filter(|a| !a.starts_with("--")).cloned();
            // A stopped screen starts again as it was, unless told otherwise.
            let mut opts = given.as_deref().and_then(screens::kept).unwrap_or_default();
            if let Some(size) = option(rest, "--size") {
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                opts.width = w.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
                opts.height = h.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
            }
            if let Some(scale) = option(rest, "--scale") {
                opts.scale = scale.parse().map_err(|_| "--scale is a number")?;
            }
            opts.sound = opts.sound || has(rest, "--sound");
            let screen = screens::start(&given.unwrap_or_else(screens::free_name), &opts)?;
            print_json(&json!(screen));
            Ok(())
        })(),
        "list" => {
            let running = screens::list();
            let listed: Vec<serde_json::Value> = screens::all()
                .into_iter()
                .map(|(name, state)| {
                    let title = history::title(&name);
                    let mut v = match running.iter().find(|s| s.name == name) {
                        Some(s) => json!(s),
                        None => {
                            let kept = screens::kept(&name).unwrap_or_default();
                            json!({ "name": name, "width": kept.width, "height": kept.height, "scale": kept.scale, "sound": kept.sound })
                        }
                    };
                    v["state"] = json!(state);
                    v["title"] = json!(title);
                    v
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&listed).unwrap_or_default());
            Ok(())
        }
        "rename" => name().and_then(|n| history::set_title(&n, rest.get(1).map(String::as_str))),
        "stop" => name().and_then(|n| screens::stop(&n)),
        "archive" => name().and_then(|n| screens::archive(&n, true)),
        "unarchive" => name().and_then(|n| screens::archive(&n, false)),
        "delete" => name().and_then(|n| screens::delete(&n)),
        "env" => screen().map(|s| {
            for (k, v) in s.env() {
                println!("export {k}={}", shell_quote(&v));
            }
        }),
        "take" => (|| {
            let ids = window_ids(rest)?;
            if ids.is_empty() {
                return Err("take needs the windows to move (--window ID, as the person's display lists them)".into());
            }
            let person = moving::Place::person()?;
            let s = if has(rest, "--new") {
                let opts = screens::Options::default();
                screens::start(&screens::free_name(), &opts)?
            } else {
                screen()?
            };
            let outcome = moving::move_windows(&person, &ids, &moving::Place::screen(&s))?;
            let v = json!({ "name": s.name, "went": outcome.went, "stayed": outcome.stayed });
            print_json(&v);
            if outcome.went.is_empty() { Err(stayed_why(&outcome)) } else { Ok(()) }
        })(),
        "give" => screen().and_then(|s| {
            let workspace = option(rest, "--workspace").map(|n| n.parse::<u32>().ok().filter(|n| *n >= 1).ok_or("--workspace is a number from 1")).transpose()?;
            let outcome = moving::give(&s, Some(window_ids(rest)?), workspace.map(|n| n - 1), option(rest, "--display").cloned())?;
            let v = json!({ "went": outcome.went, "stayed": outcome.stayed });
            print_json(&v);
            if outcome.went.is_empty() { Err(stayed_why(&outcome)) } else { Ok(()) }
        }),
        "run" => screen().and_then(|s| {
            let pid = s.run(&rest[1..])?;
            println!("{pid}");
            answered(json!({ "pid": pid }));
            Ok(())
        }),
        // (Started with each screen: its frames for its history.)
        "record" => name().and_then(|n| record::run(&n)),
        // A video of the screen (olo-screenshot's recorder, on the screen's
        // display): started, its path said at once; `stop-video` finishes it.
        "record-video" => screen().and_then(|s| {
            let to = match option(rest, "--to") {
                Some(p) => std::path::PathBuf::from(p),
                None => video_path(&s.name),
            };
            let mut argv: Vec<String> = vec!["olo-screenshot".into(), "--record".into(), "--to".into(), to.display().to_string()];
            for flag in ["--seconds", "--fps", "--window", "--encoder", "--codec", "--region", "--pointer-size", "--camera"] {
                if let Some(v) = option(rest, flag) {
                    argv.extend([flag.to_owned(), v.to_owned()]);
                }
            }
            for flag in ["--no-pointer", "--show-clicks", "--show-keys", "--as-gif", "--background", "--smooth-pointer", "--zoom-follow"] {
                if has(rest, flag) {
                    argv.push(flag.into());
                }
            }
            if let Some(v) = option(rest, "--sound") {
                argv.extend(["--sound".to_owned(), v.to_owned()]);
            }
            let pid = s.run(&argv)?;
            // Said once it's recording (its encoder open: NVENC takes a
            // couple of seconds), so what's done next is in it.
            let display = std::path::Path::new(&s.wayland_display).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                let g = olo_protocol::recording::going_on(&display).filter(|g| g.pid == pid);
                if g.as_ref().is_some_and(|g| g.recording) {
                    break;
                }
                if g.is_none() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    return Err("the recording didn't start (olo-screenshot's log on the screen says why)".into());
                }
                if std::time::Instant::now() > until {
                    return Err("the recording hasn't started after 15 seconds".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let v = json!({ "pid": pid, "path": to, "recording": true });
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            answered(v);
            Ok(())
        }),
        // (The recorder's --stop, --pause and --resume, on the screen's
        // display.)
        "stop-video" | "pause-video" | "resume-video" | "zoom-video" | "caption-video" | "mark-video" => screen().and_then(|s| {
            let program = olo_xdg::find_program("olo-screenshot").ok_or("olo-screenshot isn't installed")?;
            let mut cmd = std::process::Command::new(program);
            let flag = match command {
                "pause-video" => "--pause",
                "resume-video" => "--resume",
                "zoom-video" => "--zoom-to",
                "caption-video" => "--caption",
                "mark-video" => "--mark",
                _ => "--stop",
            };
            cmd.arg(flag).env_remove("DISPLAY");
            if command == "zoom-video" {
                cmd.arg(positionals(rest).get(1).ok_or("zoom-video NAME X,Y,W,H|out")?);
            }
            if command == "mark-video" {
                cmd.arg(positionals(rest).get(1).cloned().unwrap_or_default());
            }
            if command == "caption-video" {
                // (--clear: the caption taken down.)
                let text = if has(rest, "--clear") { String::new() } else { positionals(rest).get(1).cloned().ok_or("caption-video NAME TEXT [--seconds N] | --clear")? };
                cmd.arg(text);
                if let Some(v) = option(rest, "--seconds") {
                    cmd.args(["--seconds", v]);
                }
            }
            for (k, v) in s.env() {
                cmd.env(k, v);
            }
            let out = cmd.output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("olo-screenshot: ").to_owned());
            }
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            answered(v);
            Ok(())
        }),
        // A picture out of a video (one recorded, to check it), as Viewer
        // shows it there.
        "video-frame" => (|| {
            let p = positionals(rest);
            let video = p.first().ok_or("video-frame VIDEO [OUT.png] [--at SECONDS]")?;
            let video = std::path::absolute(PathBuf::from(video)).map_err(|e| e.to_string())?;
            let at: f64 = option(rest, "--at").map_or(Ok(0.0), |v| v.parse()).map_err(|_| "--at takes seconds")?;
            let out = match p.get(1) {
                Some(o) => std::path::absolute(PathBuf::from(o)).map_err(|e| e.to_string())?,
                None => {
                    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    video.with_file_name(format!("{stem} at {at} s.png"))
                }
            };
            let image = olo_preview::video::picture_at(&olo_preview::input::Input::file(&video), at)?;
            let [w, h] = image.size;
            let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().and_then(|mut wr| wr.write_image_data(&image.rgba)).map_err(|e| e.to_string())?;
            print_json(&json!({ "path": out, "at": at, "width": w, "height": h }));
            Ok(())
        })(),
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
            // As a video: an MP4, quiet stretches cut.
            if let Some(out) = option(rest, "--export") {
                let out = std::path::absolute(PathBuf::from(out)).map_err(|e| e.to_string())?;
                let mut shown = -1i32;
                // Actions N to N (from 1, as listed), or all.
                let at = |flag| option(rest, flag).map(|v| v.parse::<usize>().map_err(|_| format!("{flag} is an action's number, from 1"))).transpose();
                let (from, to) = (at("--from")?, at("--to")?);
                let count = history::read(&name()?).len();
                let part: Option<Vec<usize>> = (from.is_some() || to.is_some()).then(|| (from.unwrap_or(1).max(1) - 1..to.unwrap_or(count).min(count)).collect());
                let zoom = option(rest, "--zoom").map(|z| z.trim_end_matches(['x', '×']).parse::<usize>().map_err(|_| "--zoom is 1, 2 or 3")).transpose()?.unwrap_or(1);
                let encoder = option(rest, "--encoder").map(|e| olo_video::Choice::parse(e).ok_or("--encoder is auto, cpu, nvidia or vaapi")).transpose()?.unwrap_or(olo_video::Choice::Auto);
                let made = export::export(&name()?, part.as_deref(), zoom, encoder, &out, &mut |f| {
                    let pct = (f * 100.0) as i32;
                    if pct / 10 != shown / 10 {
                        eprintln!("olo-screen: exporting… {pct}%");
                        shown = pct;
                    }
                    true
                })?;
                print_json(
                    &json!({ "path": out, "seconds": made.frames as f64 / 10.0, "frames": made.frames, "pictures_encoded": made.encoded, "encoder": made.encoder, "size": [made.size.0, made.size.1] }),
                );
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
                        filter.keeps(n, d, c, &t.popups)
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
            // (Asked to press a control by its name, as `click` does: said
            // what each is for. "invoke needs a menu command" alone was
            // read ten times without the agent seeing why.)
            if ["--name", "--role", "--match", "--text"].iter().any(|f| has(rest, f)) {
                return Err("invoke chooses one of the app's menu commands, by its command (`menus` lists them: `invoke NAME save`); to press a button or another control by its name, use `click NAME --role Button --name …`".into());
            }
            let command = positionals(rest).get(1).cloned().ok_or("invoke needs a menu command (`menus` lists them: `invoke NAME save`)")?;
            let w = app_window(&s, window_arg(rest)?)?;
            let pid = pid_of(&w)?;
            if commands::invoke(pid, &command) {
                return Ok(());
            }
            // Why not, and what there is like it.
            let entries = commands::menus(pid).map(|m| m.entries).unwrap_or_default();
            // Its label (or its command but for case), the one item so
            // named: that item, chosen. (Agents write what the menu says.)
            let named: Vec<&commands::Entry> = entries.iter().filter(|e| e.command != command && (find::plain(&e.label) == find::plain(&command) || find::plain(&e.command) == find::plain(&command))).collect();
            if let [e] = named[..]
                && e.enabled
                && commands::invoke(pid, &e.command)
            {
                print_json(&json!({ "invoked": e.command, "label": e.label }));
                return Ok(());
            }
            if let Some(e) = entries.iter().find(|e| e.command == command).or(named.first().copied()) {
                return Err(format!("{} ▸ {} ({:?}) is off now: it can't be chosen as things are", e.path.join(" ▸ "), e.label, e.command));
            }
            let mut like: Vec<&commands::Entry> = entries.iter().filter(|e| e.enabled).collect();
            like.sort_by_key(|e| std::cmp::Reverse(find::likeness(&command, &e.label).max(find::likeness(&command, &e.command))));
            let shown: Vec<String> = like.iter().take(8).map(|e| format!("{:?} ({})", e.command, e.label)).collect();
            Err(format!("{} has no command {command:?}{}", w.app_id, if shown.is_empty() { " (it says no menus)".into() } else { format!("; the nearest it has: {} (`menus` has them all)", shown.join(", ")) }))
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
            let reviews: Vec<commands::WindowReview> =
                asked.into_iter().flat_map(|r| r.join().ok().flatten().unwrap_or_default()).filter(|r| r.layer.is_none() && windows.iter().any(|w| w.title == r.title)).collect();
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
                    let f = into_view(&s, &q, find::find(&s, &q)?)?;
                    (f.center.0 as f64, f.center.1 as f64, Some(f))
                }
                None => {
                    let (x, y) = point(rest, "click")?;
                    (x, y, None)
                }
            };
            s.input(screens::click(x, y, button(rest), if has(rest, "--double") { 2 } else { 1 }))?;
            print_json(&json!({ "clicked": [x, y], "matched": matched, "after": find::after(&s) }));
            Ok(())
        })(),
        "move" => (|| {
            let (x, y) = point(rest, "move")?;
            input(vec![SyntheticEvent::Move { x, y }])
        })(),
        "down" => (|| {
            let (x, y) = point(rest, "down")?;
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
            // DX DY, or --dx/--dy (either alone), and --at X,Y for where.
            let by = |flag: &str| option(rest, flag).map(|v| v.parse::<f64>().map_err(|_| format!("{flag} is a number of pixels"))).transpose();
            let (dx, dy) = match (by("--dx")?, by("--dy")?) {
                (None, None) => {
                    let p = positionals(rest);
                    let n = |i: usize| p.get(i).and_then(|a| a.parse::<f64>().ok());
                    match (n(1), n(2)) {
                        (Some(dx), Some(dy)) => (dx, dy),
                        _ => return Err("scroll takes DX DY in pixels (positive: right and down), or --dx N and --dy N; --at X,Y for where".into()),
                    }
                }
                (dx, dy) => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
            };
            let s = screen()?;
            if let Some(a) = option(rest, "--at") {
                let (x, y) = pair(a).ok_or("--at is X,Y")?;
                s.input(vec![SyntheticEvent::Move { x, y }])?;
            }
            s.input_paced(screens::scroll(dx, dy, !has(rest, "--no-end")))
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
            s.type_text(&rest.get(1..).filter(|t| !t.is_empty()).ok_or("type needs text")?.join(" "))?;
            let after = find::after(&s);
            warn_if_not_text(&after);
            print_json(&json!({ "after": after }));
            Ok(())
        })(),
        "set" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("set needs the field: --name, --role or --match")?;
            let text = positionals(rest).get(1).cloned().ok_or("set needs the text to put in the field")?;
            let f = into_view(&s, &q, find::find(&s, &q)?)?;
            // Into the field (once more if the first click went astray:
            // the keys below would edit whatever has the focus).
            let into = |s: &screens::Screen| find::after(s).focused.is_some_and(|now| find::focus_in(&now, &f));
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
            s.input(events)?;
            s.type_text(&text)?;
            let after = find::after(&s);
            warn_if_not_text(&after);
            print_json(&json!({ "field": f, "after": after }));
            Ok(())
        })(),
        "select" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("select needs the dropdown: --name, --role or --match")?;
            let item = positionals(rest).get(1).cloned().ok_or("select needs the item to choose")?;
            let field = into_view(&s, &q, find::find(&s, &q)?)?;
            // A dropdown that says what it offers and draws its menu
            // itself (a web page's <select>: the browser's menu, whose
            // items are no things of their own on the screen): chosen by
            // the keyboard, by its words, and checked. (Waited for as
            // menu items, they never came: "its menu has no item (or
            // didn't open)", 31 times in the agents' histories, all in
            // Chrome.)
            if !field.options.is_empty() {
                let want = find::option_meant(&field.options, &item).map_err(|e| format!("{} {:?}: {e}", field.role, field.name.as_deref().unwrap_or("")))?.clone();
                if field.value.as_deref().map(find::plain) != Some(find::plain(&want)) {
                    s.input(screens::click(field.center.0 as f64, field.center.1 as f64, screens::LEFT, 1))?;
                    std::thread::sleep(Duration::from_millis(250));
                    s.type_text(&want)?;
                    std::thread::sleep(Duration::from_millis(150));
                    s.input(screens::chord(&screens::parse_chord("Enter")?))?;
                }
                let after = find::after(&s);
                let now = find::find(&s, &find::Query { index: 0, ..q.clone() }).ok().filter(|n| n.window == field.window);
                let says = now.as_ref().and_then(|n| n.value.clone()).unwrap_or_default();
                if find::plain(&says) != find::plain(&want) {
                    s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    return Err(format!("chose {want:?}, but the {} says {says:?}", field.role));
                }
                print_json(&json!({ "field": field, "chose": want, "after": after }));
                return Ok(());
            }
            // (What shows of a menu's kind before it opens: a menu bar's
            // own, a page's lists. More than that is its menu, open.)
            const ITEMS: [&str; 3] = ["MenuItem", "MenuItemRadio", "ListBoxOption"];
            let items_in = |trees: &[ScreenTree]| -> Vec<String> {
                ITEMS.iter().flat_map(|role| find::matches(trees, &find::Query { role: Some((*role).into()), ..Default::default() })).filter_map(|f| f.name).collect()
            };
            let before = items_in(&trees(&s, None)?).len();
            s.input(screens::click(field.center.0 as f64, field.center.1 as f64, screens::LEFT, 1))?;
            // Its menu opens in a frame or two; then choose the item: a
            // menu's item, or (a page's own dropdown) an option of a list.
            // A menu that has opened without the item says so at once; one
            // that doesn't open is given three seconds.
            let until = std::time::Instant::now() + Duration::from_secs(3);
            let mut open_for = 0;
            let chosen = loop {
                let trees = trees(&s, None)?;
                let item_of = |role: &str| find::matches(&trees, &find::Query { name: Some(item.clone()), role: Some(role.into()), ..Default::default() }).into_iter().next();
                if let Some(f) = ["MenuItem", "MenuItemRadio", "MenuItemCheckBox", "ListBoxOption", "ListItem"].into_iter().find_map(item_of) {
                    break f;
                }
                let mut shown: Vec<String> = items_in(&trees).into_iter().map(|n| format!("{n:?}")).collect();
                open_for = if shown.len() > before { open_for + 1 } else { 0 };
                if open_for >= 3 || std::time::Instant::now() >= until {
                    // What its menu does show, to choose from.
                    shown.dedup();
                    shown.truncate(20);
                    s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    return Err(if shown.is_empty() { format!("its menu didn't open (no item {item:?} showed)") } else { format!("its menu has no item {item:?}; it has: {}", shown.join(", ")) });
                }
                std::thread::sleep(Duration::from_millis(120));
            };
            s.input(screens::click(chosen.center.0 as f64, chosen.center.1 as f64, screens::LEFT, 1))?;
            print_json(&json!({ "field": field, "chose": chosen, "after": find::after(&s) }));
            Ok(())
        })(),
        "wait" => (|| {
            let s = screen()?;
            // (Until the person hands it back, by default as long as
            // they like.)
            if has(rest, "--free") {
                let ms = option(rest, "--timeout").map(|t| t.parse::<u64>().map_err(|_| "--timeout is milliseconds")).transpose()?;
                let until = ms.map(|ms| std::time::Instant::now() + Duration::from_millis(ms));
                while screens::controlled(&s.name) {
                    if until.is_some_and(|u| std::time::Instant::now() >= u) {
                        return Err(format!("the person is still controlling screen {}", s.name));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                print_json(&json!({ "free": true }));
                return Ok(());
            }
            let ms = option(rest, "--timeout").map(|t| t.parse::<u64>().map_err(|_| "--timeout is milliseconds")).transpose()?.unwrap_or(10_000);
            let timeout = Duration::from_millis(ms);
            if has(rest, "--idle") {
                find::wait_idle(&s, window_arg(rest)?, timeout)?;
            }
            if let Some(q) = find::Query::from_args(rest)? {
                let found = find::wait_for(&s, &q, has(rest, "--gone"), timeout)?;
                print_json(&json!({ "found": found }));
            } else if !has(rest, "--idle") {
                return Err("wait for what? --idle, --free, --name, --role, --match or --text (with --gone, until it goes)".into());
            }
            Ok(())
        })(),
        "scroll-to" => (|| {
            let s = screen()?;
            // (The words alone, as `scroll-to NAME "Pricing"`: what's
            // looked for, anywhere in a name or a value. Seven of eight
            // refusals in the agents' histories were that.)
            let q = match find::Query::from_args(rest)? {
                Some(q) => q,
                None => match positionals(rest).get(1) {
                    Some(text) => find::Query { text: Some(text.clone()), window: window_arg(rest)?, ..Default::default() },
                    None => return Err("scroll-to needs what to find: the words, or --name, --role, --match or --text".into()),
                },
            };
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
    const VALUED: [&str; 17] = [
        "--at",
        "--name",
        "--role",
        "--match",
        "--text",
        "--index",
        "--window",
        "--timeout",
        "--steps",
        "--duration",
        "--crop",
        "--name-contains",
        "--contains",
        "--depth",
        "--max-len",
        "--dx",
        "--dy",
    ];
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
        if s >= 3600 {
            format!("{} h {} min", s / 3600, s / 60 % 60)
        } else if s >= 60 {
            format!("{} min {} s", s / 60, s % 60)
        } else {
            format!("{s} s")
        }
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
        match v["failed"].as_u64() {
            Some(n) if n > 0 => format!(" ({n} failed)"),
            _ => String::new(),
        },
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
    let first = shown(s)?.0;
    if let Some(f) = &first
        && whole(f)
        && in_view(s, f)?
    {
        return Ok(f.clone());
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
    // Straight to it, if it's there out of sight (a web page's tree has
    // the whole page); else up to the top, then down; until it shows or
    // the list stops moving.
    let plan: &[(i32, usize)] = match &first {
        Some(f) if f.center.1 < view_of(s, f)?.1 => &[(-3, 1000)],
        Some(_) => &[(3, 1000)],
        None => &[(-10, 100), (3, 1000)],
    };
    for &(notches, rounds) in plan {
        let mut last = String::new();
        for _ in 0..rounds {
            s.input_paced(screens::wheel(0, notches))?;
            std::thread::sleep(Duration::from_millis(80));
            let (found, now) = shown(s)?;
            if let Some(f) = found
                && whole(&f)
                && in_view(s, &f)?
            {
                return settled(s, &shown, f);
            }
            if now == last {
                break;
            }
            last = now;
        }
    }
    Err(format!("scrolled the whole list; nothing matches {}", q.describe()))
}

/// `f` once it stops moving: a browser scrolls smoothly, so what the
/// wheel brought into view goes on gliding for a moment, and a click at
/// where it was read mid-glide missed it (`set` on a web form). Read again
/// every 50 ms until two reads agree, a second at most.
fn settled(s: &screens::Screen, shown: &dyn Fn(&screens::Screen) -> Result<(Option<find::Found>, String), String>, mut f: find::Found) -> Result<find::Found, String> {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        match shown(s)?.0 {
            Some(now) if (now.x, now.y, now.width, now.height) == (f.x, f.y, f.width, f.height) => return Ok(now),
            Some(now) => f = now,
            None => break,
        }
    }
    Ok(f)
}

/// Where a click lands on what shows: the part of the screen `f`'s window
/// shows its content in (x, y, right, bottom), else the whole screen.
fn view_of(s: &screens::Screen, f: &find::Found) -> Result<(f32, f32, f32, f32), String> {
    let screen = (0.0, 0.0, s.width as f32, s.height as f32);
    let content = s.windows()?.into_iter().find(|w| w.id == f.window).and_then(|w| w.content);
    Ok(content.map_or(screen, |c| (c.x.max(0) as f32, c.y.max(0) as f32, ((c.x + c.width) as f32).min(screen.2), ((c.y + c.height) as f32).min(screen.3))))
}

/// Whether a click at `f`'s middle lands on it: within what its window
/// shows, on the screen. (A web page's tree has the whole page: a field
/// below the fold was at y 1045 of a 900-point screen, clicked where
/// nothing is, and `set` failed seven times on 2026-11-13.)
fn in_view(s: &screens::Screen, f: &find::Found) -> Result<bool, String> {
    let (x, y) = f.center;
    let (left, top, right, bottom) = view_of(s, f)?;
    Ok(x >= left && x < right && y >= top && y < bottom)
}

/// `f`, scrolled into view first if it's out of sight (`scroll_to`).
fn into_view(s: &screens::Screen, q: &find::Query, f: find::Found) -> Result<find::Found, String> {
    if in_view(s, &f)? { Ok(f) } else { scroll_to(s, q, None) }
}

/// "X,Y" as two numbers.
fn pair(a: &str) -> Option<(f64, f64)> {
    let (x, y) = a.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Where, as agents write it: `X Y`, `X,Y` or `--at X,Y` (the screen's
/// logical points).
fn point(rest: &[String], what: &str) -> Result<(f64, f64), String> {
    if let Some(a) = option(rest, "--at") {
        return pair(a).ok_or_else(|| "--at is X,Y".to_owned());
    }
    let p = positionals(rest);
    if let Some(xy) = p.get(1).and_then(|a| pair(a)) {
        return Ok(xy);
    }
    match (p.get(1).and_then(|a| a.parse().ok()), p.get(2).and_then(|a| a.parse().ok())) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("{what} takes where: X Y, X,Y or --at X,Y (the screen's logical points){}", if what == "click" { ", or what to click: --name, --role, --match or --text" } else { "" })),
    }
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
    /// Where its open menus and dropdowns are (their items are in
    /// `nodes`, under a `Menu` each).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    popups: Vec<Frame>,
}

/// A window's open menus and dropdowns, put under it. A program gives a
/// popup's tree as a window's of its own (a menu's with no title, a
/// submenu's titled by its item), and app_server lists a popup with its
/// window, not as one: so the trees left over (`rest`: the program's
/// that no window took yet; not one titled as another window of the
/// program, `windows`) are matched to the window's `popups` by size,
/// each one's nodes moved to its popup's place on the screen, numbered
/// after the window's, its root a `Menu` under the window's. (They were
/// dropped: an open menu's items, a dropdown's, weren't in `tree` and
/// couldn't be clicked by name, as the skill says they can.) The popups
/// taken, in order.
fn with_popups(nodes: &mut Vec<commands::UiNode>, popups: &[Frame], rest: &mut Vec<commands::WindowTree>, windows: &[String]) -> Vec<Frame> {
    let root = nodes.first().map(|n| n.id);
    let mut shown = Vec::new();
    for p in popups {
        let fits = |t: &commands::WindowTree| t.layer.is_none() && t.width as i32 == p.width && t.height as i32 == p.height && (t.title.is_empty() || !windows.contains(&t.title));
        let Some(i) = rest.iter().position(fits) else { continue };
        let menu = rest.remove(i);
        let after = nodes.iter().map(|n| n.id).max().map_or(0, |m| m + 1);
        for mut n in menu.nodes {
            n.id += after;
            n.x += p.x as f32;
            n.y += p.y as f32;
            match n.parent {
                Some(parent) => n.parent = Some(parent + after),
                None => {
                    n.parent = root;
                    n.role = "Menu".into();
                }
            }
            nodes.push(n);
        }
        shown.push(*p);
    }
    shown
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
    // A program not on Olo's kit: its tree over AT-SPI (GTK, Qt).
    let asked: Vec<_> = pids.iter().map(|&pid| (pid, std::thread::spawn(move || commands::tree(pid).filter(|t| !t.is_empty()).or_else(|| atspi::trees(pid))))).collect();
    // Windows only: Deskbar and other layer surfaces have no window here.
    let mut answers: Vec<(u32, Vec<commands::WindowTree>)> =
        asked.into_iter().map(|(pid, t)| (pid, t.join().ok().flatten().unwrap_or_default().into_iter().filter(|t| t.layer.is_none()).collect())).collect();
    let mut out = Vec::new();
    let titles: Vec<String> = windows.iter().map(|w| w.title.clone()).collect();
    for w in windows {
        let (Some(content), Some(pid)) = (w.content, w.pid) else { continue };
        let Some((_, mine)) = answers.iter_mut().find(|(p, _)| *p == pid as u32) else { continue };
        // An app's windows are told apart by title (and by size, if two share one).
        // Chrome's are named by their page ("Page - Google Chrome").
        let Some(i) = mine
            .iter()
            .position(|t| t.title == w.title && t.width as i32 == content.width && t.height as i32 == content.height)
            .or_else(|| mine.iter().position(|t| t.title == w.title))
            .or_else(|| mine.iter().position(|t| !t.title.is_empty() && w.title.starts_with(&t.title) && t.width as i32 == content.width))
            .or_else(|| mine.iter().position(|t| t.width as i32 == content.width && t.height as i32 == content.height))
        else {
            continue;
        };
        let tree = mine.remove(i);
        let mut nodes: Vec<commands::UiNode> = tree
            .nodes
            .into_iter()
            .map(|mut n| {
                n.x += content.x as f32;
                n.y += content.y as f32;
                n
            })
            .collect();
        let popups = with_popups(&mut nodes, &w.popups, mine, &titles);
        out.push(ScreenTree { window: w.id, app_id: w.app_id, title: w.title, focused: w.focused, content, nodes, popups });
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
    fn keeps(&self, n: &commands::UiNode, depth: usize, c: Frame, popups: &[Frame]) -> bool {
        if self.depth.is_some_and(|d| depth > d) {
            return false;
        }
        if self.visible_only {
            // (A menu's items show wherever its popup is, the window's
            // content or not.)
            let within = |c: &Frame| n.x < (c.x + c.width) as f32 && n.y < (c.y + c.height) as f32 && n.x + n.width > c.x as f32 && n.y + n.height > c.y as f32;
            let inside = within(&c) || popups.iter().any(within);
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
            if !filter.keeps(n, d, c, &t.popups) {
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
            // (What a dropdown offers, where its items aren't things of
            // their own: what `select` can choose.)
            if !n.options.is_empty() {
                let shown: Vec<String> = n.options.iter().take(12).map(|o| format!("{:?}", filter.cut(o))).collect();
                let _ = write!(o, " options=[{}{}]", shown.join(", "), if n.options.len() > shown.len() { format!(", … {} in all", n.options.len()) } else { String::new() });
            }
            let _ = write!(o, " at {:.0},{:.0} {:.0}x{:.0}", n.x, n.y, n.width, n.height);
            if let (Some(row), Some(rows)) = (n.row, n.rows) {
                let _ = write!(o, " row {}/{rows}", row + 1);
            }
            for (on, word) in [
                (!n.enabled, "disabled"),
                (n.clickable && !matches!(n.role.as_str(), "Clickable" | "Button" | "Link" | "MenuItem" | "Row"), "clickable"),
                (n.focused, "focused"),
                (n.checked == Some(true), "checked"),
                (n.selected == Some(true), "selected"),
                (n.minimized, "minimized"),
            ] {
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
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
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
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        // (Re-executed by an upgrade, the same pid: what it said it
        // controlled before isn't so now.)
        screens::clear_controlled_by_me();
        let window = olo_kit::open_window(olo_kit::WindowOptions::new("Agent Screens", 1000.0, 640.0).min_size(660.0, 420.0).keep_size("main"), move |cx| viewer::Viewer::new(show, cx));
        // What the workers and other starts say goes to the window.
        olo_kit::message_handler(app, move |message, _| {
            window.messenger().send(message);
        });
    });
}

/// Where a video of screen `name` goes unless told: the Videos folder,
/// named for the screen and when.
fn video_path(name: &str) -> std::path::PathBuf {
    let dir = olo_xdg::user_dir("VIDEOS").unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos"));
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r fills `tm` from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    dir.join(format!("Agent Screen {name} {:04}-{:02}-{:02} at {:02}.{:02}.{:02}.mp4", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// An open menu's items are in its window's tree: the program's tree
    /// with no title, taken by its popup's size, moved to the popup's
    /// place, under a `Menu` under the window; a tree no popup fits is
    /// left.
    #[test]
    fn an_open_menu_is_under_its_window() {
        let node = |id, parent, role: &str, name: Option<&str>, x, y| commands::UiNode {
            id,
            parent,
            role: role.into(),
            name: name.map(str::to_owned),
            x,
            y,
            width: 100.0,
            height: 20.0,
            enabled: true,
            ..Default::default()
        };
        let tree = |title: &str, w, h, nodes| commands::WindowTree { title: title.into(), width: w, height: h, active: false, layer: None, surface: None, path: None, nodes };
        let mut nodes = vec![node(0, None, "Window", Some("Notes"), 50.0, 60.0), node(1, Some(0), "Button", Some("Save"), 60.0, 70.0)];
        let mut rest = vec![
            tree("", 300, 90, vec![node(0, None, "Window", None, 0.0, 0.0), node(1, Some(0), "MenuItem", Some("Other"), 4.0, 4.0)]),
            tree("", 200, 60, vec![node(0, None, "Window", None, 0.0, 0.0), node(1, Some(0), "MenuItem", Some("Copy"), 4.0, 4.0), node(2, Some(0), "MenuItem", Some("Paste"), 4.0, 24.0)]),
            // (Another window of the program, the size of a popup: a window's.)
            tree("Letter", 120, 40, vec![node(0, None, "Window", Some("Letter"), 0.0, 0.0)]),
            // (A submenu: titled by its item.)
            tree("Paste", 120, 40, vec![node(0, None, "Window", Some("Paste"), 0.0, 0.0), node(1, Some(0), "MenuItem", Some("As Plain Text"), 4.0, 4.0)]),
        ];
        let popups = [Frame { x: 400, y: 300, width: 200, height: 60 }, Frame { x: 0, y: 0, width: 77, height: 77 }, Frame { x: 598, y: 320, width: 120, height: 40 }];
        let shown = with_popups(&mut nodes, &popups, &mut rest, &["Notes".to_owned(), "Letter".to_owned()]);
        assert_eq!(shown, vec![popups[0], popups[2]]);
        assert_eq!(rest.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["", "Letter"], "the tree no popup fits stays, and another window's");
        assert_eq!(
            nodes.split_off(5).iter().map(|n| (n.id, n.parent, n.role.as_str(), n.name.as_deref(), n.x, n.y)).collect::<Vec<_>>(),
            vec![(5, Some(0), "Menu", Some("Paste"), 598.0, 320.0), (6, Some(5), "MenuItem", Some("As Plain Text"), 602.0, 324.0)]
        );
        let said: Vec<_> = nodes.iter().map(|n| (n.id, n.parent, n.role.as_str(), n.name.as_deref(), n.x, n.y)).collect();
        assert_eq!(
            said,
            vec![
                (0, None, "Window", Some("Notes"), 50.0, 60.0),
                (1, Some(0), "Button", Some("Save"), 60.0, 70.0),
                (2, Some(0), "Menu", None, 400.0, 300.0),
                (3, Some(2), "MenuItem", Some("Copy"), 404.0, 304.0),
                (4, Some(2), "MenuItem", Some("Paste"), 404.0, 324.0),
            ]
        );
        // Found by name, and listed in the outline under the window;
        // seen with --visible-only though outside the window's content.
        let t = ScreenTree { window: 7, app_id: "olo.notes".into(), title: "Notes".into(), focused: true, content: Frame { x: 50, y: 60, width: 300, height: 200 }, nodes, popups: shown };
        let found = find::matches(std::slice::from_ref(&t), &find::Query { role: Some("MenuItem".into()), name: Some("Paste".into()), ..Default::default() });
        assert_eq!(found.len(), 1);
        let o = outline(std::slice::from_ref(&t), &Filter { visible_only: true, ..Default::default() });
        assert!(o.contains("Menu at 400,300 100x20") && o.contains("MenuItem \"Copy\" at 404,304"), "{o}");
    }

    /// Where, as agents write it: `X Y`, `X,Y`, `--at X,Y`; anything else
    /// says how (it was "x must be a number" ten times on one screen).
    #[test]
    fn points_as_agents_write_them() {
        assert_eq!(point(&args(&["s", "600", "500"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "600,500"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "--at", "600, 500", "--double"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "--right", "-5", "7.5"]), "click"), Ok((-5.0, 7.5)));
        assert!(point(&args(&["s", "--at", "600"]), "click").unwrap_err().contains("X,Y"));
        assert!(point(&args(&["s"]), "move").unwrap_err().contains("--at X,Y"));
        // (--dx and --dy take values: not where.)
        assert_eq!(positionals(&args(&["s", "--dx", "3", "--dy", "900"])), args(&["s"]));
    }
}
