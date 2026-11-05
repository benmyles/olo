//! Smart Tiling (Preferences ▸ AI): app_server starts this for the
//! Smart Tile shortcut. It looks at the active display (a screenshot, each
//! window's app, title and place, and what Olo's own apps show, from
//! their widget trees), asks the model chosen in Preferences for the best
//! tiling, and has app_server arrange the windows so (undoable with Undo
//! Window Arrangement). A notification shows how it is going and can
//! cancel it.

use olo_sync::Mutex;
use std::{
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_kit_base::{
    commands,
    notify::{Note, NoteEvent, NoteKind, Notifier},
};
use olo_protocol::{Client, Frame, Placement, Reply, Request, WindowInfo};
use serde_json::{Value, json};

/// The screenshot sent is no bigger than this on its long side.
const SHOT_EDGE: u32 = 1568;
/// Lines of each window's outline at most, and characters a line.
const OUTLINE_LINES: usize = 60;
const OUTLINE_CHARS: usize = 80;
/// How long Undo stays offered.
const UNDO_FOR: Duration = Duration::from_secs(60);

const SYSTEM: &str = "You arrange the windows on a person's screen. You see a screenshot of \
the screen, and for each window its app, title and current place, and for Olo's own apps an \
outline of what the window shows. Tile them: choose for every window a rectangle of the tiling \
area so that together they fill the area without overlapping. Give the most room to what the \
person is most likely working on (the focused window, documents, editors, terminals in use, \
long lists), and less to what is only glanced at (chat, music, logs, monitors, small utilities), \
which can sit in narrower columns or stacked at a side. Give each window at least the room its \
content needs to be readable, keep related windows next to each other, and keep windows near \
where they are now when there is no reason to move them. Rectangles are percentages of the \
tiling area (0–100) from its top left. Also say in one short sentence how you arranged them.";

fn main() -> ExitCode {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("olo-smart-tile: {e}");
            ExitCode::FAILURE
        }
    }
}

fn request(client: &mut Client, req: &Request) -> Result<Reply, String> {
    client.request(req).map_err(|e| e.to_string())
}

/// What the notification's buttons asked for.
#[derive(Default)]
struct Asked {
    cancel: bool,
    undo: bool,
}

fn run() -> Result<(), String> {
    let mut client = Client::connect().map_err(|e| e.to_string())?;
    let Reply::Settings { settings } = request(&mut client, &Request::GetSettings)? else { return Err("no settings".into()) };
    let asked = Arc::new(Mutex::new(Asked::default()));
    let notes = {
        let asked = asked.clone();
        Notifier::with_events("Smart Tiling", "olo-smart-tile", move |e| {
            if let NoteEvent::Action { key, .. } = e {
                let mut a = asked.lock().unwrap();
                match key.as_str() {
                    "cancel" => a.cancel = true,
                    "undo" => a.undo = true,
                    "preferences" => {
                        let _ = olo_xdg::spawn_detached("olo-preferences ai");
                    }
                    _ => {}
                }
            }
        })
    };
    let feature = &settings.ai.smart_tiling;
    if !feature.enabled {
        notes.show(None, Note::new("Smart Tiling is off").body("Turn it on in Preferences ▸ AI.").icon("ai").action("preferences", "Preferences…"));
        linger();
        return Ok(());
    }
    let Reply::Arrangement { output, area, windows } = request(&mut client, &Request::Arrangement)? else { return Err("no arrangement".into()) };
    let windows: Vec<WindowInfo> = windows.into_iter().filter(|w| !w.minimized && w.frame.is_some()).collect();
    match windows.len() {
        0 => return Ok(()),
        1 => {
            let p = Placement { id: windows[0].id, x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
            request(&mut client, &Request::Arrange { windows: vec![p] })?;
            return Ok(());
        }
        _ => {}
    }

    // The picture first, before our notification is in it; the trees
    // meanwhile (each app can take a frame or two to answer).
    let trees = {
        let windows = windows.clone();
        std::thread::spawn(move || outlines(&windows))
    };
    let shot = screenshot(&mut client, &output)?;
    let model = feature.model_name();
    let started = Instant::now();
    let note = |done: f32, body: &str| Note::progress("Smart Tiling", done).body(body).icon("ai").action("cancel", "Cancel");
    let id = notes.show(None, note(0.1, &format!("Looking at {} windows…", windows.len())));
    let trees = trees.join().unwrap_or_default();
    let content = describe(area, &windows, &trees, &shot);

    // The model, on a thread of its own: we keep the note ticking and
    // listen for Cancel.
    let ask = olo_ai::Ask { system: SYSTEM.into(), content, schema: schema(), max_tokens: 16000 };
    let (ai, feature) = (settings.ai.clone(), feature.clone());
    let answer = std::thread::spawn(move || olo_ai::ask(&ai, &feature, &ask));
    while !answer.is_finished() {
        if asked.lock().unwrap().cancel {
            notes.close(id);
            linger();
            return Ok(());
        }
        let secs = started.elapsed().as_secs();
        // Creeps towards the end: how long a model takes isn't known.
        let done = 0.2 + 0.7 * (1.0 - (-(secs as f32) / 40.0).exp());
        notes.show(Some(id), note(done, &format!("Asking {model}… {secs} s")));
        std::thread::sleep(Duration::from_millis(500));
    }
    let answer = answer.join().map_err(|_| "the model call panicked".to_string())?;
    let (placements, summary) = match answer.map_err(|e| e.to_string()).and_then(|a| placements(&a, &windows)) {
        Ok(p) => p,
        Err(e) => {
            notes.show(Some(id), Note::new("Smart Tiling didn't work").body(e.clone()).icon("ai").kind(NoteKind::Error));
            linger();
            return Err(format!("no arrangement from the model: {e}"));
        }
    };
    let placed = placements.len();
    request(&mut client, &Request::Arrange { windows: placements })?;
    notes.show(Some(id), Note::new(format!("Smart tiled {placed} windows")).body(summary).icon("ai").action("undo", "Undo"));
    let until = Instant::now() + UNDO_FOR;
    while Instant::now() < until {
        if std::mem::take(&mut asked.lock().unwrap().undo) {
            request(&mut client, &Request::RunShortcut { id: "undo-arrangement".into(), window: None })?;
            notes.close(id);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Long enough for the notifier's thread to deliver the last note.
fn linger() {
    std::thread::sleep(Duration::from_millis(500));
}

/// The display as a PNG no bigger than [`SHOT_EDGE`].
fn screenshot(client: &mut Client, output: &str) -> Result<Vec<u8>, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let path = dir.join(format!("olo-smart-tile-{}.png", std::process::id()));
    let req = Request::Screenshot { path: path.display().to_string(), output: Some(output.into()), window: None, border: true, pointer: false, format: Default::default(), as_shown: false };
    request(client, &req)?;
    let image = image::open(&path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    let mut image = image?;
    if image.width().max(image.height()) > SHOT_EDGE {
        image = image.resize(SHOT_EDGE, SHOT_EDGE, image::imageops::FilterType::Triangle);
    }
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

/// What each of Olo's own windows shows, as a short outline (by
/// window id); apps not on the kit have none.
fn outlines(windows: &[WindowInfo]) -> Vec<(u64, String)> {
    let mut pids: Vec<u32> = windows.iter().filter_map(|w| w.pid.map(|p| p as u32)).collect();
    pids.sort_unstable();
    pids.dedup();
    let asked: Vec<_> = pids.into_iter().map(|pid| (pid, std::thread::spawn(move || commands::tree(pid)))).collect();
    let mut answers: Vec<(u32, Vec<commands::WindowTree>)> = asked.into_iter().map(|(pid, t)| (pid, t.join().ok().flatten().unwrap_or_default())).collect();
    let mut out = Vec::new();
    for w in windows {
        let (Some(pid), Some(content)) = (w.pid, w.content) else { continue };
        let Some((_, mine)) = answers.iter_mut().find(|(p, _)| *p == pid as u32) else { continue };
        // An app's windows are told apart by title, then by size.
        let Some(i) = mine.iter().position(|t| t.title == w.title).or_else(|| mine.iter().position(|t| t.width as i32 == content.width && t.height as i32 == content.height)) else {
            continue;
        };
        out.push((w.id, outline(&mine.remove(i))));
    }
    out
}

fn outline(tree: &commands::WindowTree) -> String {
    let mut depth: std::collections::HashMap<u64, usize> = Default::default();
    let mut lines = Vec::new();
    for n in &tree.nodes {
        let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
        depth.insert(n.id, d);
        let visible = n.width >= 1.0 && n.height >= 1.0 && n.x < tree.width as f32 && n.y < tree.height as f32;
        if !visible || (n.name.is_none() && n.value.is_none()) {
            continue;
        }
        let mut line = format!("{}{}", "  ".repeat(d.min(6)), n.role);
        if let Some(name) = &n.name {
            line += &format!(" \"{name}\"");
        }
        if let Some(value) = &n.value {
            line += &format!(" value=\"{value}\"");
        }
        lines.push(line.chars().take(OUTLINE_CHARS).collect::<String>());
        if lines.len() == OUTLINE_LINES {
            lines.push("…".into());
            break;
        }
    }
    lines.join("\n")
}

/// Percent of the area, from its top left.
fn percent(area: Frame, f: Frame) -> (i32, i32, i32, i32) {
    let p = |v: i32, of: i32| (v as f32 * 100.0 / of.max(1) as f32).round() as i32;
    (p(f.x - area.x, area.width), p(f.y - area.y, area.height), p(f.width, area.width), p(f.height, area.height))
}

/// The question: the screenshot, then the area and every window.
fn describe(area: Frame, windows: &[WindowInfo], trees: &[(u64, String)], shot: &[u8]) -> Vec<Value> {
    let mut text = format!("The tiling area is {}×{} logical pixels. The screenshot is the whole display.\n\nWindows, front first:\n", area.width, area.height);
    for w in windows {
        let Some(frame) = w.frame else { continue };
        let (x, y, ww, h) = percent(area, frame);
        let focused = if w.focused { ", focused" } else { "" };
        text += &format!("\nWindow {}: app \"{}\", title \"{}\"{focused}; now at x={x}% y={y}% w={ww}% h={h}% ({}×{} px)\n", w.id, w.app_id, w.title, frame.width, frame.height);
        if let Some((_, o)) = trees.iter().find(|(id, _)| *id == w.id)
            && !o.is_empty()
        {
            text += "What it shows:\n";
            text += o;
            text += "\n";
        }
    }
    vec![olo_ai::png_block(shot), olo_ai::text_block(text)]
}

/// The answer's shape.
fn schema() -> Value {
    let n = json!({"type": "number"});
    json!({
        "type": "object",
        "properties": {
            "windows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}, "x": n, "y": n, "w": n, "h": n},
                    "required": ["id", "x", "y", "w", "h"],
                    "additionalProperties": false
                }
            },
            "summary": {"type": "string"}
        },
        "required": ["windows", "summary"],
        "additionalProperties": false
    })
}

/// The model's answer as placements of our windows, and its summary.
fn placements(answer: &Value, windows: &[WindowInfo]) -> Result<(Vec<Placement>, String), String> {
    let f = |v: &Value| v.as_f64().map(|v| (v / 100.0) as f32).filter(|v| v.is_finite());
    let mut out: Vec<Placement> = Vec::new();
    for w in answer["windows"].as_array().ok_or("no windows in the answer")? {
        let id = w["id"].as_u64().ok_or("a window without an id")?;
        if !windows.iter().any(|ours| ours.id == id) || out.iter().any(|p| p.id == id) {
            continue;
        }
        let (Some(x), Some(y), Some(ww), Some(h)) = (f(&w["x"]), f(&w["y"]), f(&w["w"]), f(&w["h"])) else { continue };
        if ww <= 0.0 || h <= 0.0 {
            continue;
        }
        out.push(Placement { id, x, y, w: ww, h });
    }
    if out.is_empty() {
        return Err("The model placed none of the windows.".into());
    }
    Ok((out, answer["summary"].as_str().unwrap_or_default().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64) -> WindowInfo {
        WindowInfo {
            id,
            app_id: "olo.terminal".into(),
            title: "t".into(),
            pid: None,
            workspace: 0,
            minimized: false,
            focused: false,
            chosen: false,
            rolled: false,
            covered: false,
            zoomed: false,
            zoomed_along: None,
            kept: false,
            group: None,
            groupable: true,
            tiled: false,
            part: None,
            floating: false,
            float: None,
            output: None,
            tab: None,
            behind: false,
            frame: Some(Frame { x: 0, y: 0, width: 10, height: 10 }),
            content: None,
            least: None,
            tab_frame: None,
            popups: vec![],
            settling: false,
            stack: None,
            glowing: false,
            surface: None,
        }
    }

    #[test]
    fn reads_placements() {
        let answer = json!({"summary": "s", "windows": [
            {"id": 1, "x": 0, "y": 0, "w": 60, "h": 100},
            {"id": 2, "x": 60, "y": 0, "w": 40, "h": 100},
            {"id": 2, "x": 0, "y": 0, "w": 10, "h": 10},
            {"id": 9, "x": 0, "y": 0, "w": 10, "h": 10},
        ]});
        let (p, s) = placements(&answer, &[window(1), window(2)]).unwrap();
        assert_eq!(s, "s");
        assert_eq!(p.len(), 2);
        assert_eq!((p[1].x, p[1].w), (0.6, 0.4));
        assert!(placements(&json!({"windows": [{"id": 9, "x": 0, "y": 0, "w": 1, "h": 1}]}), &[window(1)]).is_err());
    }

    #[test]
    fn percent_of_the_area() {
        let area = Frame { x: 100, y: 50, width: 1000, height: 500 };
        assert_eq!(percent(area, Frame { x: 600, y: 50, width: 500, height: 250 }), (50, 0, 50, 50));
    }
}
