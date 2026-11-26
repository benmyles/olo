//! About Olo: the BeOS "About This System" window — version, machine,
//! processor, memory, kernel and uptime. Facts are read from /proc and
//! /sys on a thread; uptime then ticks on locally. And Software Update:
//! the release installed, whether a newer one is out (Check Now asks the
//! session to ask GitHub at once, Upgrade to install it), and Olo's other
//! releases, an older one to go back to among them (Ben, 2026-11-24: "add
//! something in about olo desktop like a button to check for new releases
//! and also the ability to install an older release too when
//! available"). The session does the asking and the installing
//! (crates/session's upgrade.rs, told by a choice and a signal, as
//! Deskbar tells it); this pane follows its two files, by inotify.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use olo_kit::{
    Bounds, Cx, IntoElement as _, ParentElement as _, Styled as _, brand, canvas, div, icons, metrics as m, point, px, size, style, theme,
    widgets::{BBox, Button, MenuField, StatusBar, StringView, heading},
};
use olo_sync::Mutex;

use crate::{Msg, PanelView, form::Ui, panel};

#[derive(Default, Clone)]
struct Facts {
    machine: String,
    processor: String,
    memory: String,
    kernel: String,
    /// Uptime when read, and when that was.
    uptime: Option<(Duration, Instant)>,
}

/// Test data: a machine as long-winded as they come.
fn test_facts() -> Facts {
    Facts {
        machine: "Lenovo ThinkPad P1 Gen 9 (21KV-CTO1WW) Mobile Workstation".into(),
        processor: "Intel Core Ultra 7 356H (16 cores: 4 performance, 8 efficient, 4 low-power), 5.1 GHz".into(),
        memory: "64 GB LPDDR5X-8533".into(),
        kernel: "Linux 7.3.0-2.fc44.aurora.aarch64 (Fedora Linux 44, Workstation Edition)".into(),
        uptime: Some((Duration::from_secs(3 * 86_400 + 7 * 3600 + 42 * 60), Instant::now())),
    }
}

/// Software Update, as the session and GitHub say it.
#[derive(Default, Clone)]
struct Update {
    /// The release installed (its version, "2026.11.24-1e237b51"), none
    /// where Olo was built from source.
    installed: Option<String>,
    /// What the session last heard from GitHub (`upgrade-releases-DISPLAY.json`).
    heard: serde_json::Value,
    /// What it offers and is doing (`upgrade-DISPLAY.json`: Deskbar's).
    available: serde_json::Value,
    /// Olo's releases, the newest first, once asked for (or why not).
    releases: Option<Result<Vec<olo_release::Listed>, String>>,
}

pub struct State {
    facts: Arc<Mutex<Option<Facts>>>,
    update: Arc<Mutex<Update>>,
    /// The release chosen in Other releases (its tag).
    chosen: Option<String>,
    /// The session's two files followed (kept alive with the pane).
    _watch: Option<olo_kit::watch::Watch>,
    /// When Reload Olo was last clicked, or why it failed.
    restarted: Option<Instant>,
    error: Option<String>,
    /// A frame is due at the next second (uptime counts them).
    ticking: bool,
}

impl State {
    pub fn new(_cx: &mut Cx<PanelView>) -> State {
        let facts = Arc::new(Mutex::new(None));
        // (Tests, the layout lint: a machine described at length, not
        // this one.)
        let update = Arc::new(Mutex::new(Update::default()));
        if cfg!(test) {
            *facts.lock().unwrap() = Some(test_facts());
            *update.lock().unwrap() = test_update();
            return State { facts, update, chosen: None, _watch: None, restarted: None, error: None, ticking: false };
        }
        // (Pictures of the update's states: `OLO_PREFS_DEMO_UPDATE=newer`,
        // the tests' release, a newer one out; `current`, none newer;
        // `installing`, the newer one going in; `first`, not looked yet;
        // this machine's own facts.
        // A build from source has no release, so shows none of them.)
        if let Some(demo) = std::env::var("OLO_PREFS_DEMO_UPDATE").ok() {
            let mut u = test_update();
            match demo.as_str() {
                "current" => {
                    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    u.heard = serde_json::json!({ "at": now.saturating_sub(300), "newer": false });
                }
                "installing" => u.available = serde_json::json!({ "installing": { "version": "v2026.11.24.2", "doing": "downloading", "fraction": 0.42 } }),
                "first" => u.heard = serde_json::json!({}),
                _ => {}
            }
            *update.lock().unwrap() = u;
            let (out, proxy) = (facts.clone(), olo_kit::proxy());
            std::thread::spawn(move || {
                *out.lock().unwrap() = Some(gather());
                proxy.send(Msg::Redraw);
            });
            return State { facts, update, chosen: None, _watch: None, restarted: None, error: None, ticking: false };
        }
        let (out, proxy) = (facts.clone(), olo_kit::proxy());
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(gather());
            proxy.send(Msg::Redraw);
        });
        // What's installed and what the session says, now and as it
        // changes; and the releases, asked for once on a thread.
        let (out, proxy) = (update.clone(), olo_kit::proxy());
        std::thread::spawn(move || {
            {
                let mut u = out.lock().unwrap();
                u.installed = prefix().and_then(|p| olo_release::installed_in(&p).version);
                reread(&mut u);
            }
            proxy.send(Msg::Redraw);
            let releases = olo_release::list();
            out.lock().unwrap().releases = Some(releases);
            proxy.send(Msg::Redraw);
        });
        let (out, proxy) = (update.clone(), olo_kit::proxy());
        let names = [heard_path(), available_path()].map(|p| p.file_name().map(|n| n.to_owned()));
        let watch = olo_kit::watch::watch([olo_xdg::state_runtime_dir()], move |changed| {
            if changed.iter().any(|c| names.iter().flatten().any(|n| c.file_name() == Some(n.as_os_str()))) {
                reread(&mut out.lock().unwrap());
                proxy.send(Msg::Redraw);
            }
        });
        State { facts, update, chosen: None, _watch: watch, restarted: None, error: None, ticking: false }
    }
}

/// Where Olo is installed (its programs' folder's parent: `~/.local`).
fn prefix() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent()?.parent().map(PathBuf::from)
}

/// The session's files (crates/session's `heard_path` and `state_path`).
fn heard_path() -> PathBuf {
    olo_xdg::state_runtime_dir().join(format!("upgrade-releases-{}.json", olo_xdg::display_name()))
}

fn available_path() -> PathBuf {
    olo_xdg::state_runtime_dir().join(format!("upgrade-{}.json", olo_xdg::display_name()))
}

fn reread(u: &mut Update) {
    let read = |p: PathBuf| std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null);
    u.heard = read(heard_path());
    u.available = read(available_path());
}

/// Tell the session (`olo-session upgrade CHOICE`, as Deskbar does): a
/// choice written where it reads it, then SIGUSR2. "check", "release",
/// "release:TAG".
fn tell_session(pid: i32, choice: &str) -> Result<(), String> {
    let path = olo_xdg::state_runtime_dir().join(format!("upgrade-choice-{}", olo_xdg::display_name()));
    std::fs::write(&path, choice).map_err(|e| e.to_string())?;
    // SAFETY: sending a signal.
    if unsafe { libc::kill(pid, libc::SIGUSR2) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

/// Test data: a release installed, a newer one out, three to choose from.
fn test_update() -> Update {
    let listed = |tag: &str, title: &str, version: &str, published: u64| olo_release::Listed { tag: tag.into(), title: title.into(), version: version.into(), published, size: 589_872_238 };
    Update {
        installed: Some("2026.11.24-9f9208b6".into()),
        heard: serde_json::json!({ "at": 1_795_500_000u64, "title": "Olo 2026.11.24 (update 2)", "tag": "v2026.11.24.2", "version": "2026.11.24-1e237b51", "newer": true }),
        available: serde_json::Value::Null,
        releases: Some(Ok(vec![
            listed("v2026.11.24.2", "Olo 2026.11.24 (update 2)", "2026.11.24-1e237b51", 1_795_512_965),
            listed("v2026.11.24", "Olo 2026.11.24", "2026.11.24-9f9208b6", 1_795_507_540),
            listed("v2026.11.17", "Olo 2026.11.17", "2026.11.17-4e8d72a5", 1_794_946_419),
        ])),
    }
}

/// A release as the pane names it: its title where the list has it, else
/// "Olo" and its version's date.
fn release_name(version: &str, releases: Option<&Vec<olo_release::Listed>>) -> String {
    releases.and_then(|rs| rs.iter().find(|r| r.version == version)).map(|r| r.title.clone()).unwrap_or_else(|| format!("Olo {}", version.split('-').next().unwrap_or(version)))
}

/// What the header says Olo is: the release installed, by its title
/// ("Olo 2026.11.24 (update 2)"), or that it was built from source (the
/// crate's own version, 0.1.0, said nothing).
fn version_line(u: &Update) -> String {
    match &u.installed {
        // (Under the wordmark: its date and update, not "Olo" again.)
        Some(v) => {
            let name = release_name(v, u.releases.as_ref().and_then(|r| r.as_ref().ok()));
            name.strip_prefix("Olo ").map(str::to_owned).unwrap_or(name)
        }
        None => olo_kit::words::tr("Built from source").to_string(),
    }
}

/// How long ago `at` (seconds since the epoch) was, in words.
fn ago(at: u64) -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(at, |d| d.as_secs());
    let mins = now.saturating_sub(at) / 60;
    match mins {
        0 => olo_kit::words::tr("just now").to_string(),
        1..=59 => olo_kit::words::count(mins, "1 minute ago", "{n} minutes ago"),
        _ => olo_kit::words::count(mins / 60, "1 hour ago", "{n} hours ago"),
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::About(st) => Some(st),
        _ => None,
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim_end_matches(['\0', '\n']).trim().to_owned()
}

fn gather() -> Facts {
    let machine = [read("/sys/firmware/devicetree/base/model"), read("/sys/class/dmi/id/product_name")].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "Unknown".into());
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let cpuinfo = read("/proc/cpuinfo");
    let model = cpuinfo.lines().find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_owned()));
    let chip = [read("/sys/firmware/devicetree/base/compatible").split('\0').find(|s| s.starts_with("apple,t")).map(apple_chip)].into_iter().flatten().flatten().next();
    let processor = match chip.or(model) {
        Some(name) => format!("{name}, {cpus} cores"),
        None => format!("{cpus} × {}", std::env::consts::ARCH),
    };
    let meminfo = read("/proc/meminfo");
    let total_kb: u64 = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.split_whitespace().next()?.parse().ok()).unwrap_or(0);
    let memory = format!("{} GB", installed_gb(total_kb));
    let kernel = format!("Linux {}", read("/proc/sys/kernel/osrelease"));
    let uptime = read("/proc/uptime").split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).map(|s| (Duration::from_secs_f64(s), Instant::now()));
    Facts { machine, processor, memory, kernel, uptime }
}

/// MemTotal leaves out what firmware and the GPU carve-out reserve: round
/// up to the size that was bought.
fn installed_gb(total_kb: u64) -> u64 {
    const SIZES: &[u64] = &[1, 2, 4, 6, 8, 12, 16, 18, 24, 32, 36, 48, 64, 96, 128, 192, 256, 384, 512];
    let gb = total_kb as f64 / 1024.0 / 1024.0;
    SIZES.iter().copied().find(|&s| s as f64 >= gb).unwrap_or(gb.ceil() as u64)
}

/// Apple SoC names from the device tree's compatible string.
fn apple_chip(compat: &str) -> Option<String> {
    Some(
        match compat {
            "apple,t8103" => "Apple M1",
            "apple,t6000" => "Apple M1 Pro",
            "apple,t6001" => "Apple M1 Max",
            "apple,t6002" => "Apple M1 Ultra",
            "apple,t8112" => "Apple M2",
            "apple,t6020" => "Apple M2 Pro",
            "apple,t6021" => "Apple M2 Max",
            "apple,t6022" => "Apple M2 Ultra",
            _ => return None,
        }
        .to_owned(),
    )
}

/// The olo-session supervising us, if we run under one.
fn session_pid() -> Option<i32> {
    std::env::var("OLO_SESSION_PID").ok()?.parse().ok().filter(|&p| p > 1)
}

/// Restart Olo in place, as `olo-session reload` does: the session
/// re-execs app_server, and every app (this one included) reconnects with
/// its windows where they were.
fn restart(pid: i32) -> Result<(), String> {
    // SAFETY: sending a signal.
    if unsafe { libc::kill(pid, libc::SIGHUP) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

pub fn uptime_text(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins, secs) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    let unit = |n: u64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(unit(days, "day"));
    }
    if hours > 0 {
        parts.push(unit(hours, "hour"));
    }
    if days == 0 {
        parts.push(unit(mins, "minute"));
    }
    if days == 0 && hours == 0 {
        parts.push(unit(secs, "second"));
    }
    parts.join(", ")
}

/// How long until what the window says of time changes: its uptime
/// (to the second in its first hour, the minute in its first day, the
/// hour after), and how long ago Olo looked for a release (to the minute
/// in the first hour, the hour after). Woken then, not each second.
fn next_change(up: Duration, looked_ago: Option<Duration>) -> Duration {
    let until = |d: Duration, step: u64| Duration::from_secs(step) - Duration::from_nanos((d.as_nanos() % (u128::from(step) * 1_000_000_000)) as u64);
    let uptime = match up.as_secs() {
        0..3600 => until(up, 1),
        3600..86400 => until(up, 60),
        _ => until(up, 3600),
    };
    let looked = looked_ago.map(|ago| if ago.as_secs() < 3600 { until(ago, 60) } else { until(ago, 3600) });
    uptime.min(looked.unwrap_or(Duration::MAX)).max(Duration::from_millis(20))
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let dark = theme::palette().mode == olo_kit::palette::ThemeMode::Dark;
    let word_w = brand::wordmark_width(30.0, dark);
    let icon = canvas(|g, b| {
        icons::paint(g, b, "olo");
    })
    .w(px(64.0))
    .h(px(64.0))
    .flex_shrink_0();
    let title = match word_w {
        Some(w) => canvas(move |g, b| {
            let at = Bounds { origin: point(b.left(), b.top()), size: size(px(w), px(30.0)) };
            brand::wordmark_left(g, at, dark);
        })
        .w(px(w))
        .h(px(30.0))
        .into_element(),
        None => heading("Olo Desktop").into_element(),
    };
    ui.add(div().flex().flex_row().gap(px(m::SPACING)).child(icon).child(div().flex().flex_col().pt(px(6.0)).child(title).child(StringView::new(version_line(&st.update.lock().unwrap())).dim(true))));
    let facts = st.facts.lock().unwrap().clone();
    let up = facts.as_ref().and_then(|f| f.uptime).map(|(d, at)| d + at.elapsed());
    let rows = ui.collect(|ui| {
        let Some(f) = facts else {
            ui.note("Looking…");
            return;
        };
        let uptime = f.uptime.map(|(d, at)| uptime_text(d + at.elapsed())).unwrap_or_default();
        // (Each value a paragraph that may wrap, its label on its first
        // line: the same top, the line as tall as the paragraph's.)
        let top = (m::CONTROL_HEIGHT - style::line_row(style::PLAIN_SIZE)) / 2.0;
        let line = style::PLAIN_SIZE * style::LINE_HEIGHT;
        for (label, value) in [("Machine:", &f.machine), ("Processor:", &f.processor), ("Memory:", &f.memory), ("Kernel:", &f.kernel), ("Uptime:", &uptime)] {
            // The label right-aligned in its column, as BeOS's.
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .min_h(px(m::CONTROL_HEIGHT))
                    .child(div().w(px(90.0)).flex_shrink_0().flex().justify_end().pr(px(m::LABEL_GAP)).pt(px(top)).child(StringView::new(label).line(line)))
                    // (A long name wraps, under itself: a ThinkPad's model,
                    // a kernel's build.)
                    .child(div().flex_1().min_w(px(0.0)).pt(px(top)).child(StringView::new(value.clone()).wrap(true))),
            );
        }
    });
    ui.add(BBox::plain().children(rows));
    // The uptime, and how long ago Olo looked, as they change.
    if !st.ticking {
        st.ticking = true;
        let looked = st.update.lock().unwrap().heard["at"].as_u64().map(|at| {
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
            now.saturating_sub(Duration::from_secs(at))
        });
        let wait = up.map_or(Duration::from_secs(1), |up| next_change(up, looked));
        ui.cx.after_do(wait, |v, cx| {
            if let Some(st) = state_of(v) {
                st.ticking = false;
            }
            cx.notify();
        });
    }
    software_update(ui, st);
    let pid = session_pid();
    // A restart takes a moment; don't let clicks pile up meanwhile.
    let settling = st.restarted.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
    let note = match (&st.error, pid, settling) {
        (Some(e), _, _) => olo_kit::tr!("Couldn't reload: {}", e),
        (None, None, _) => olo_kit::words::tr("Not running under olo-session.").to_owned(),
        (None, Some(_), true) => "Reloading…".to_owned(),
        (None, Some(_), false) => String::new(),
    };
    let restart_now = ui.on_click_view(move |v, cx| {
        if let (Some(st), Some(pid)) = (state_of(v), pid) {
            st.error = restart(pid).err();
            st.restarted = Some(Instant::now());
        }
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(m::SPACING))
            .child(StringView::new(note).dim(true))
            // (Not while a release installs: a restart halfway through.)
            .child(Button::new("Reload Olo").min_width(update_button()).enabled(pid.is_some() && !settling && !st.update.lock().unwrap().available["installing"].is_object()).on_click(restart_now)),
    );
}

/// The width of the box's buttons: one column, whichever is there.
fn update_button() -> f32 {
    // (One column of buttons, as wide as its widest in the language shown:
    // Check Now under Upgrade & Reload, Install… under it, and Reload Olo
    // below the box as wide. Measured once for each language: the box is
    // drawn again each second the window is open, for its uptime.)
    thread_local! {
        static KEPT: std::cell::Cell<Option<(&'static str, f32)>> = const { std::cell::Cell::new(None) };
    }
    let code = olo_kit::words::current().code;
    KEPT.with(|kept| match kept.get() {
        Some((of, width)) if of == code => width,
        _ => {
            let width = UPDATE_BUTTONS.iter().map(|l| olo_kit::widgets::button_width(l, false)).fold(88.0, f32::max);
            kept.set(Some((code, width)));
            width
        }
    })
}

const UPDATE_BUTTONS: [&str; 4] = ["Check Now", "Upgrade & Reload", "Install…", "Reload Olo"];

/// The window's width: its widest row in the language shown (each state's
/// words, a release's title as long as they come, the release menu), the
/// column of buttons beside it, 444 at least. Wider than English's where a
/// language needs it: German's "Aktualisieren und neu starten" left its
/// headline 444 points, cut short; widened by the column alone, a hair
/// from its button.
pub fn window_width() -> u32 {
    let font = style::ui_font(olo_kit::FontWeight::NORMAL);
    let w = |s: &str| f32::from(style::text_width(s.to_owned(), style::PLAIN_SIZE, font.clone())).ceil();
    let tr = |s: &'static str| olo_kit::words::tr(s).to_string();
    let release = "Olo 2026.12.31 (update 20)";
    let doing = ["Downloading", "Unpacking", "Putting it in place"].iter().map(|d| w(&olo_kit::tr!("{}, {}%", tr(d), 100))).fold(0.0, f32::max);
    let installing = w(&olo_kit::tr!("Installing {}…", release)) + m::SPACING + doing;
    let menu = w(&tr("Release:")) + m::LABEL_GAP + w(&olo_kit::tr!("{} (installed)", release)) + 2.0 * m::CONTROL_PAD + olo_kit::widgets::MENU_MARK_ROOM;
    let rows = [
        olo_kit::tr!("{} is available.", release).to_string(),
        tr("Olo is up to date."),
        tr("Looking for a newer release…"),
        tr("Couldn't check for a newer release."),
        tr("The last install didn't go through."),
        tr("Olo looks for a newer release every hour."),
        tr("The first look is two minutes after it starts."),
        olo_kit::tr!("Checked {}.", olo_kit::words::count(59, "1 minute ago", "{n} minutes ago")).to_string(),
    ];
    // (Each row's words beside the column of buttons, but what installs and
    // how far: no button beside that, the box's whole width.)
    let text = rows.iter().map(|r| w(r)).fold(menu, f32::max) + m::SPACING + update_button();
    (2.0 * (m::WINDOW_INSET + m::BOX_INSET) + text.max(installing)).max(444.0).ceil() as u32
}

/// What the status part of Software Update says: a line and its detail
/// under it (two lines in every state, so nothing below moves as it
/// changes; Opus's review, 2026-11-24), and its button; or an install's
/// bar.
enum Status {
    /// Two rows: what there is (and its action: Upgrade & Reload), then
    /// the check (when Olo last looked, or why it couldn't) beside Check
    /// Now, the same row in every state (each row's words go with its
    /// button: the restart note beside Check Again read as one sentence).
    Line {
        title: String,
        action: Option<(&'static str, &'static str)>,
        checked: String,
        can_check: bool,
    },
    Installing {
        title: String,
        detail: String,
        fraction: f32,
    },
}

/// Software Update: whether there's newer (and installing it), and the
/// release to have, an older one too.
fn software_update(ui: &mut Ui, st: &mut State) {
    let u = st.update.lock().unwrap().clone();
    let pid = session_pid();
    let releases = u.releases.as_ref().and_then(|r| r.as_ref().ok());
    let can = pid.is_some() && u.installed.is_some();
    let busy = u.available["installing"].is_object();
    let rows = ui.collect(|ui| {
        // (A build from source: its session doesn't look for releases.)
        if u.installed.is_none() {
            ui.add(StringView::new(olo_kit::words::tr("This Olo upgrades from the build it came from, not from releases.")).wrap(true));
            return;
        }
        // (A release chosen by its tag says its tag: named by the list.)
        let named = |v: &str| releases.and_then(|rs| rs.iter().find(|r| r.tag == v || r.version == v)).map_or_else(|| v.to_owned(), |r| r.title.clone());
        let checked = u.heard["at"].as_u64().map(|at| olo_kit::tr!("Checked {}.", ago(at)).to_string()).unwrap_or_default();
        let installing = u.available["installing"].clone();
        let status = if busy {
            let doing = match installing["doing"].as_str() {
                Some("unpacking") => "Unpacking",
                Some("installing") => "Putting it in place",
                _ => "Downloading",
            };
            let fraction = installing["fraction"].as_f64().unwrap_or(0.0) as f32;
            Status::Installing {
                title: olo_kit::tr!("Installing {}…", named(installing["version"].as_str().unwrap_or_default())),
                detail: olo_kit::tr!("{}, {}%", olo_kit::words::tr(doing), (fraction * 100.0).round() as u32),
                fraction,
            }
        } else if let Some(failed) = u.available["failed"].as_str() {
            Status::Line { title: olo_kit::words::tr("The last install didn't go through.").into(), action: None, checked: failed.to_owned(), can_check: true }
        } else if u.heard["asking"].as_bool() == Some(true) {
            Status::Line { title: olo_kit::words::tr("Looking for a newer release…").into(), action: None, checked, can_check: false }
        } else if let Some(e) = u.heard["error"].as_str() {
            Status::Line { title: olo_kit::words::tr("Couldn't check for a newer release.").into(), action: None, checked: e.to_owned(), can_check: true }
        // (Newer than what is installed now: what the session heard
        // before an upgrade names the release just put in, until it
        // has asked again. Ben, 2026-11-26: "when it comes back it
        // still says the release i just installed is available".)
        } else if u.heard["newer"].as_bool() == Some(true) && u.heard["version"].as_str().is_none_or(|v| Some(v) != u.installed.as_deref()) {
            Status::Line {
                title: olo_kit::tr!("{} is available.", u.heard["title"].as_str().unwrap_or("A newer release")),
                // (Not the window's default: Return in About starting a
                // 900 MB install was one key from an accident. Restart in
                // its name: what it does when it's done.)
                // ("Reload", not "Restart": Ben, 2026-11-26, "restart
                // makes it sound like we will restart the whole
                // computer", which Deskbar's own Restart does.)
                action: Some(("Upgrade & Reload", "release")),
                // (And a look for one newer still: Ben, 2026-11-25, "we
                // should still have a Check Again button or something in
                // case i want to see if there's a newer release
                // available".)
                checked,
                can_check: true,
            }
        } else if u.heard["at"].is_u64() {
            Status::Line { title: olo_kit::words::tr("Olo is up to date.").into(), action: None, checked, can_check: true }
        } else {
            Status::Line {
                title: olo_kit::words::tr("Olo looks for a newer release every hour.").into(),
                action: None,
                checked: olo_kit::words::tr("The first look is two minutes after it starts.").into(),
                can_check: true,
            }
        };
        // Two rows, each a control's height, a button at the end of each
        // where it has one (Upgrade & Reload, Check Now under it, Install…
        // under that in the next row: one column of buttons, rows at the
        // box's pitch); while it installs, what and how far in the first,
        // the bar in the second: the rows below stay where they are in
        // every state.
        let column = update_button();
        let row = |ui: &mut Ui, words: StringView, button: Option<(&'static str, &'static str)>, on: bool| {
            let mut line = div().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w(px(0.0)).child(words));
            line = match button {
                Some((label, choice)) => {
                    let tell = ui.on_click_view(move |_, cx| {
                        if let Some(pid) = session_pid() {
                            let _ = tell_session(pid, choice);
                        }
                        cx.notify();
                    });
                    line.child(Button::new(label).min_width(column).enabled(can && on).on_click(tell))
                }
                None => line.child(div().w(px(column)).flex_shrink_0()),
            };
            ui.add(line);
        };
        match status {
            // (No button while it installs: it can't be stopped halfway.)
            Status::Installing { title, detail, fraction } => {
                ui.add(
                    div()
                        .h(px(m::CONTROL_HEIGHT))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().min_w(px(0.0)).child(StringView::new(title).cut(true)))
                        .child(StringView::new(detail)),
                );
                ui.add(div().h(px(m::CONTROL_HEIGHT)).flex().flex_col().justify_center().child(StatusBar::new(fraction)));
            }
            Status::Line { title, action, checked, can_check } => {
                row(ui, StringView::new(title).cut(true), action, true);
                row(ui, StringView::new(if checked.is_empty() { " ".to_owned() } else { checked }).dim(true).cut(true), Some(("Check Now", "check")), can_check);
            }
        }
        // The release to have: the installed one marked; another chosen,
        // Install… (going back asks first).
        match &u.releases {
            None => ui.note("Asking GitHub for Olo's releases…"),
            Some(Err(e)) => ui.note(olo_kit::tr!("Olo's releases couldn't be listed: {}", e)),
            Some(Ok(list)) if !list.is_empty() => {
                let names: Vec<olo_kit::SharedString> = list
                    .iter()
                    .map(|r| if Some(&r.version) == u.installed.as_ref() { olo_kit::tr!("{} (installed)", r.title) } else { r.title.clone() }.into())
                    .collect();
                let at = st.chosen.as_ref().and_then(|t| list.iter().position(|r| &r.tag == t)).or_else(|| list.iter().position(|r| Some(&r.version) == u.installed.as_ref())).unwrap_or(0);
                let tags: Vec<String> = list.iter().map(|r| r.tag.clone()).collect();
                let chooser = MenuField::new("about-releases", names, at).label("Release:").enabled(can && !busy).on_change(ui.on_view(move |v, i: &usize, cx| {
                    if let Some(st) = state_of(v) {
                        st.chosen = tags.get(*i).cloned();
                    }
                    cx.notify();
                }));
                let release = list[at].clone();
                let is_installed = Some(&release.version) == u.installed.as_ref();
                let installed_name = u.installed.as_deref().map(|v| release_name(v, releases)).unwrap_or_default();
                let older = u.installed.as_ref().and_then(|v| list.iter().find(|r| &r.version == v)).is_some_and(|i| release.published < i.published);
                let install = ui.on_click_view(move |_, _| {
                    let Some(pid) = session_pid() else { return };
                    let mb = (release.size as f64 / 1_000_000.0).round() as u64;
                    let (message, yes) = if older {
                        (
                            olo_kit::tr!(
                                "Go back to {}?\n\nIt replaces {}. Olo will download it (about {}\u{a0}MB), install it and reload in place. Your settings are kept; what Olo gained since then is gone until you upgrade again.",
                                release.title,
                                installed_name,
                                mb
                            ),
                            "Go Back",
                        )
                    } else {
                        (olo_kit::tr!("Install {}?\n\nOlo will download it (about {}\u{a0}MB), install it and reload in place.", release.title, mb), "Install")
                    };
                    let tag = release.tag.clone();
                    // (Going back warned of; on to a newer one, Olo's own.)
                    olo_kit::widgets::open_app_alert("Software Update", if older { "warning" } else { "about" }, &message, &[yes, "Cancel"], move |chosen| {
                        if chosen == Some(0) {
                            let _ = tell_session(pid, &format!("release:{tag}"));
                        }
                    });
                });
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().min_w(px(0.0)).child(chooser))
                        .child(Button::new("Install…").min_width(column).enabled(can && !is_installed && !busy).on_click(install)),
                );
            }
            Some(Ok(_)) => ui.note("Olo has no releases yet."),
        }
    });
    ui.add(BBox::new("Software Update").children(rows));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window wakes when what it says of time next changes: each second
    /// in the uptime's first hour, at the next minute in its first day, the
    /// next hour after; sooner where "Checked … ago." changes first.
    #[test]
    fn it_wakes_when_its_words_change() {
        let s = |n: u64| Duration::from_secs(n);
        let ms = |n: u64| Duration::from_millis(n);
        assert_eq!(next_change(s(65) + ms(300), None), ms(700));
        assert_eq!(next_change(s(2 * 3600 + 30), None), s(30));
        assert_eq!(next_change(s(3 * 86400 + 600), None), s(3000));
        assert_eq!(next_change(s(3 * 86400 + 600), Some(s(5 * 60 + 15))), s(45));
        assert_eq!(next_change(s(3 * 86400 + 600), Some(s(2 * 3600 + 100))), s(3000));
        assert_eq!(next_change(s(3 * 86400), None), s(3600), "just changed: the next is a step away");
    }

    #[test]
    fn uptime() {
        assert_eq!(uptime_text(Duration::from_secs(65)), "1 minute, 5 seconds");
        assert_eq!(uptime_text(Duration::from_secs(3 * 3600 + 120)), "3 hours, 2 minutes");
        assert_eq!(uptime_text(Duration::from_secs(2 * 86400 + 3600 + 60)), "2 days, 1 hour");
        assert_eq!(installed_gb(32_367_952), 32);
        assert_eq!(installed_gb(15_900_000), 16);
        assert_eq!(apple_chip("apple,t6021").as_deref(), Some("Apple M2 Max"));
    }
}
