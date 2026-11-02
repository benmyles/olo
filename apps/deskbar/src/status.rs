//! The tray's view of the machine: clock, battery, Wi-Fi, volume and
//! agent screens, followed on background threads (as the services say
//! they change: nothing polled while nothing changes), and the small
//! vector glyphs that show them.

use olo_sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use olo_protocol::NotificationState;
use olo_system::audio;

use olo_ui::{
    gpui::{App, BorderStyle, Bounds, FontWeight, Hsla, PathBuilder, Pixels, Point, Window, point, px, quad, size, transparent_black},
    palette::{Hue, Palette, ThemeMode},
    style::{self, c},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wifi {
    pub ssid: Option<String>,
    pub signal: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    pub level: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub battery: Option<Battery>,
    pub wifi: Option<Wifi>,
    pub volume: Option<Volume>,
    /// Programs a newer build would replace (olo-session's offer).
    pub upgrade: Vec<String>,
    /// Upgrading was asked for while a build was running: olo-session
    /// upgrades once it has finished.
    pub upgrade_waiting: bool,
    /// Unread notifications and Do Not Disturb (None: no notification
    /// service).
    pub notifications: Option<NotificationState>,
    /// Agent screens running (other than the one this Deskbar is on).
    pub agent_screens: usize,
    /// Screens apps are sharing (olo-portal's).
    pub sharing: Vec<olo_protocol::sharing::Share>,
    /// The supervisor (Preferences ▸ Performance), if it is installed.
    pub supervisor: Option<Supervisor>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Supervisor {
    /// Turned on (its settings: at once, before it has answered).
    pub on: bool,
    /// What it published.
    pub state: olo_protocol::performance::SupervisorState,
}

/// What olo-session offers to upgrade (see its upgrade.rs).
fn upgrade_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("olo-upgrade-{}.json", olo_xdg::display_name()))
}

/// What's on offer, and whether olo-session waits for a build to finish
/// to upgrade.
fn upgrade() -> (Vec<String>, bool) {
    let Ok(text) = std::fs::read_to_string(upgrade_path()) else { return (Vec::new(), false) };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let programs = v["programs"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    (programs, v["waiting"].as_bool().unwrap_or(false))
}

/// The agent screens running: those `olo-screen` lists in
/// `$XDG_RUNTIME_DIR/olo-screens` whose session is still alive (as its
/// `Screen::alive` judges), leaving out the screen this Deskbar is on.
fn screens_dir() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    olo_xdg::legacy::runtime_file(&dir, "olo-screens")
}

/// A screen's own file in the screens' folder (not a viewer's frame).
fn screen_file(p: &std::path::Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.ends_with(".json") && !name.ends_with("-settings.json")
}

fn agent_screens() -> usize {
    let own = olo_xdg::display_name();
    let Ok(read) = std::fs::read_dir(screens_dir()) else { return 0 };
    read.flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".json") && !name.ends_with("-settings.json")
        })
        .filter_map(|e| std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()))
        .filter(|v| v["wayland_display"].as_str() != Some(own.as_str()))
        .filter(|v| {
            let cmdline = v["pid"].as_i64().and_then(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).ok()).unwrap_or_default();
            let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
            // (`tabula-…` for a screen started before the rename.)
            let session = args.first().is_some_and(|a| a.ends_with(b"olo-session") || a.ends_with(b"tabula-session"));
            let socket = v["olo_socket"].as_str().or(v["tabula_socket"].as_str());
            session && args.iter().any(|a| *a == b"--headless") && socket.is_some_and(|s| std::path::Path::new(s).exists())
        })
        .count()
}

enum AudioRequest {
    ToggleMute,
    Set(f32),
}

/// Volume changes go to a worker thread (the sound server can be slow to
/// answer), newest slider position winning.
fn audio_worker() -> &'static Mutex<Sender<AudioRequest>> {
    static TX: OnceLock<Mutex<Sender<AudioRequest>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<AudioRequest>();
        std::thread::Builder::new()
            .name("deskbar-audio".into())
            .spawn(move || {
                while let Ok(mut req) = rx.recv() {
                    // Skip slider positions already overtaken by newer ones.
                    while let Ok(next) = rx.try_recv() {
                        if let (AudioRequest::Set(_), AudioRequest::Set(_)) = (&req, &next) {
                            req = next;
                        } else {
                            run_audio(req);
                            req = next;
                        }
                    }
                    run_audio(req);
                }
            })
            .expect("thread");
        Mutex::new(tx)
    })
}

fn run_audio(req: AudioRequest) {
    let r = match req {
        AudioRequest::ToggleMute => audio::toggle_mute(),
        AudioRequest::Set(level) => audio::set_volume(level),
    };
    if let Err(e) = r {
        tracing::warn!("sound server: {e}");
    }
}

pub fn toggle_mute() {
    let _ = audio_worker().lock().unwrap().send(AudioRequest::ToggleMute);
}

pub fn set_volume(level: f32) {
    let _ = audio_worker().lock().unwrap().send(AudioRequest::Set(level));
}

type Shared<F> = Mutex<(Status, Option<Status>, F)>;

/// Apply `update` to the current status and send it if it changed.
fn publish<F: Fn(Status)>(shared: &Shared<F>, update: impl FnOnce(&mut Status)) {
    let mut g = shared.lock().unwrap();
    let (current, last, send) = &mut *g;
    update(current);
    if last.as_ref() != Some(current) {
        send(current.clone());
        *last = Some(current.clone());
    }
}

/// Watch the machine; `send` is called whenever something changed.
/// What other programs write to files (the bell's count, the agent
/// screens, a new build to upgrade to) follows those files as they change
/// (`olo_ui::watch`); battery and Wi-Fi as UPower and NetworkManager
/// say they change;
/// volume changes arrive as events from the sound server.
pub fn spawn(send: impl Fn(Status) + Send + 'static) {
    let shared = Arc::new(Mutex::new((Status::default(), None::<Status>, send)));
    // The battery and Wi-Fi as UPower and NetworkManager say they
    // change (nothing polled).
    let on_battery = shared.clone();
    olo_system::battery::watch(move |b| {
        let battery = b.map(|b| Battery { percent: b.percent, charging: b.charging() });
        publish(&on_battery, |s| s.battery = battery);
    });
    let on_wifi = shared.clone();
    if let Err(e) = olo_system::network::watch_wifi(move |w| {
        let wifi = w.map(|w| Wifi { ssid: w.ssid, signal: w.strength });
        publish(&on_wifi, |s| s.wifi = wifi);
    }) {
        tracing::debug!("NetworkManager: {e}");
    }
    let (notes, screens, upgrades, sharing) = (NotificationState::path(), screens_dir(), upgrade_path(), olo_protocol::SharingState::path());
    use olo_protocol::performance::{PerformanceSettings, SupervisorState};
    let (perf, supervising) = (PerformanceSettings::path(), SupervisorState::path());
    let installed = olo_xdg::find_program("olo-supervisor").is_some();
    let read = {
        let (notes, screens, upgrades, sharing, shared) = (notes.clone(), screens.clone(), upgrades.clone(), sharing.clone(), shared.clone());
        let (perf, supervising) = (perf.clone(), supervising.clone());
        move |changed: &[PathBuf]| {
            let all = changed.is_empty();
            // The screens' folder also has viewers' frames going through
            // it, several a second: only a screen's own file counts.
            let in_screens: Vec<&PathBuf> = changed.iter().filter(|p| p.parent() == Some(screens.as_path())).collect();
            let screens_changed = in_screens.iter().any(|p| screen_file(p)) || (in_screens.is_empty() && changed.contains(&screens));
            let (state, n, up) = (
                (all || changed.contains(&notes)).then(NotificationState::load),
                (all || screens_changed).then(agent_screens),
                (all || changed.contains(&upgrades)).then(upgrade),
            );
            let shares = (all || changed.contains(&sharing)).then(|| olo_protocol::SharingState::load().shares);
            let supervisor = (installed && (all || changed.contains(&perf) || changed.contains(&supervising)))
                .then(|| Supervisor { on: PerformanceSettings::load().enabled, state: SupervisorState::load() });
            publish(&shared, |s| {
                if supervisor.is_some() {
                    s.supervisor = supervisor;
                }
                if let Some(shares) = shares {
                    s.sharing = shares;
                }
                if let Some(state) = state {
                    s.notifications = state;
                }
                if let Some(n) = n {
                    s.agent_screens = n;
                }
                if let Some((up, waiting)) = up {
                    s.upgrade = up;
                    s.upgrade_waiting = waiting;
                }
            });
        }
    };
    read(&[]);
    let read = Arc::new(read);
    let on_change = read.clone();
    if let Some(w) = olo_ui::watch::watch([notes, screens, upgrades, sharing, perf, supervising], move |changed| on_change(changed)) {
        // Deskbar's for as long as it runs.
        std::mem::forget(w);
    }
    // A screen whose session ended without a word (killed) leaves its
    // file behind: looked at now and then while there are any.
    let counted = shared.clone();
    std::thread::Builder::new()
        .name("deskbar-screens".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                // (Likewise a share whose portal died.) The lock let go
                // before reading: `read` takes it again to publish.
                let again = {
                    let g = counted.lock().unwrap();
                    g.0.agent_screens > 0 || !g.0.sharing.is_empty()
                };
                if again {
                    read(&[]);
                }
            }
        })
        .expect("thread");
    std::thread::Builder::new()
        .name("deskbar-volume".into())
        .spawn(move || {
            audio::watch(|v| {
                let volume = v.map(|v| Volume { level: v.level, muted: v.muted });
                publish(&shared, |s| s.volume = volume);
            })
        })
        .expect("thread");
}

// ---- clock --------------------------------------------------------------------

unsafe extern "C" {
    fn tzset();
}

fn local_now() -> libc::tm {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    unsafe {
        // Pick up a time zone changed since the last tick.
        tzset();
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

/// `unix` seconds as a time of day, in the Deskbar clock's style (without
/// seconds).
pub fn time_of_day(unix: u64, clock: &olo_protocol::ClockSettings) -> String {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    let tm = unsafe {
        tzset();
        let t = unix as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    let clock = olo_protocol::ClockSettings { show_seconds: false, show_day: false, ..clock.clone() };
    format_clock(&tm, false, &clock)
}

/// The clock row's date ("Mon Oct 26") and time (in the clock's style;
/// the date already names the day).
pub fn clock_text(clock: &olo_protocol::ClockSettings) -> (String, String) {
    let tm = local_now();
    let clock = olo_protocol::ClockSettings { show_day: false, ..clock.clone() };
    (format_clock(&tm, true, &clock), format_clock(&tm, false, &clock))
}

fn format_clock(tm: &libc::tm, date: bool, clock: &olo_protocol::ClockSettings) -> String {
    if date {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        return format!("{} {} {}", DAYS[tm.tm_wday as usize % 7], MONTHS[tm.tm_mon as usize % 12], tm.tm_mday);
    }
    let n = |v: i32| v.max(0) as u32;
    clock.format(n(tm.tm_hour), n(tm.tm_min), n(tm.tm_sec), n(tm.tm_wday))
}

/// Until the clock next changes: the next second, or the next minute.
/// Seconds from now until `minute` (after midnight, local time) comes
/// next.
pub fn seconds_until(minute: u32) -> u32 {
    let tm = local_now();
    let now = tm.tm_hour as u32 * 3600 + tm.tm_min as u32 * 60 + tm.tm_sec as u32;
    let at = minute * 60;
    if at > now { at - now } else { at + 86_400 - now }
}

pub fn until_next_tick(seconds: bool) -> Duration {
    if seconds {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        return Duration::from_nanos(1_000_000_000 - now.subsec_nanos() as u64) + Duration::from_millis(10);
    }
    let tm = local_now();
    Duration::from_secs((60 - tm.tm_sec.clamp(0, 59)) as u64) + Duration::from_millis(50)
}

// ---- glyphs -------------------------------------------------------------------
//
// Each is drawn in a 16-point square `r`: lines that run straight are on
// device pixels at every scale (`style`'s lines and snapped boxes), the
// rest exact-coverage paths (`PathBuilder`) and the GPU's discs, rings
// and arcs (`paint_sector`).

fn ink(p: &Palette) -> Hsla {
    c(p.panel_text)
}

/// Ink for an off or empty part: halfway to the background, so it reads in
/// light and dark alike (a darker shade vanishes on a dark tray).
fn dim(p: &Palette) -> Hsla {
    c(olo_ui::palette::mix(p.panel_text, p.panel_background, 0.6))
}

/// `v` points as whole device pixels.
fn whole(window: &Window, v: f32) -> f32 {
    let s = window.scale_factor();
    (v * s).round() / s
}

fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: pt(x, y), size: size(px(w), px(h)) }
}

fn corners(min: (f32, f32), max: (f32, f32)) -> Bounds<Pixels> {
    Bounds::from_corners(pt(min.0, min.1), pt(max.0, max.1))
}

/// `pts` filled.
fn polygon(window: &mut Window, pts: &[(f32, f32)], color: Hsla) {
    let mut path = PathBuilder::fill();
    path.move_to(pt(pts[0].0, pts[0].1));
    for &(x, y) in &pts[1..] {
        path.line_to(pt(x, y));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A line through `pts`, `width` wide (closed back to the first).
fn polyline(window: &mut Window, pts: &[(f32, f32)], width: f32, closed: bool, color: Hsla) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(pt(pts[0].0, pts[0].1));
    for &(x, y) in &pts[1..] {
        path.line_to(pt(x, y));
    }
    if closed {
        path.close();
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn disc(window: &mut Window, c: (f32, f32), r: f32, color: Hsla) {
    window.paint_sector(pt(c.0, c.1), px(0.0), px(r), 0.0, std::f32::consts::TAU, color);
}

/// A line `width` wide round `c` at `radius`, from `start` radians
/// clockwise from 12 o'clock, `sweep` round.
fn arc(window: &mut Window, c: (f32, f32), radius: f32, width: f32, start: f32, sweep: f32, color: Hsla) {
    window.paint_sector(pt(c.0, c.1), px(radius - width / 2.0), px(radius + width / 2.0), start, sweep, color);
}

/// `b` filled, its corners rounded `radius`.
fn round_rect(window: &mut Window, b: Bounds<Pixels>, radius: f32, color: Hsla) {
    window.paint_quad(quad(b, px(radius), color, px(0.0), transparent_black(), BorderStyle::Solid));
}

/// `b`'s outline, a hairline inside it, its corners rounded `radius`
/// (and filled `fill`).
fn round_frame(window: &mut Window, b: Bounds<Pixels>, radius: f32, fill: Hsla, line: Hsla) {
    window.paint_quad(quad(b, px(radius), fill, style::hairline(window), line, BorderStyle::Solid));
}

fn left_top(r: Bounds<Pixels>) -> (f32, f32) {
    (f32::from(r.left()), f32::from(r.top()))
}

pub fn draw_battery(window: &mut Window, r: Bounds<Pixels>, p: &Palette, percent: u32, charging: bool) {
    // On device pixels at any scale: the body's outline a hairline, the
    // nub as far from its top as its bottom, the charge a line inside.
    let (l, t) = left_top(r);
    let body = style::snap(window, rect(l, t + 3.0, 14.0, 10.0));
    round_frame(window, body, 1.5, transparent_black(), ink(p));
    let gap = whole(window, 3.0);
    let (bl, bt, br, bb) = (f32::from(body.left()), f32::from(body.top()), f32::from(body.right()), f32::from(body.bottom()));
    round_rect(window, corners((br, bt + gap), (br + whole(window, 1.5), bb - gap)), 0.5, ink(p));
    let fill = if charging {
        c(p.success)
    } else if percent <= 10 {
        c(p.failure)
    } else if percent <= 20 {
        c(p.warning)
    } else {
        ink(p)
    };
    let inner = style::inset(window, body, 2.0);
    let level = (percent.min(100) as f32) / 100.0;
    let w = whole(window, f32::from(inner.size.width) * level).max(f32::from(style::hairline(window)));
    round_rect(window, Bounds { origin: inner.origin, size: size(px(w), inner.size.height) }, 0.5, fill);
    if charging {
        let (cx, cy) = ((bl + br) / 2.0, (bt + bb) / 2.0);
        let bolt = [(cx + 1.0, cy - 5.0), (cx - 2.5, cy + 0.5), (cx - 0.2, cy + 0.5), (cx - 1.0, cy + 5.0), (cx + 2.5, cy - 0.5), (cx + 0.2, cy - 0.5)];
        polygon(window, &bolt, c(p.hue(Hue::Yellow)));
        polyline(window, &bolt, 0.8, true, ink(p));
    }
}

pub fn draw_wifi(window: &mut Window, r: Bounds<Pixels>, p: &Palette, signal: Option<u32>) {
    let origin = (f32::from(r.center().x), f32::from(r.bottom()) - 2.5);
    let bars = match signal {
        None => 0,
        Some(s) if s >= 67 => 3,
        Some(s) if s >= 34 => 2,
        Some(_) => 1,
    };
    disc(window, origin, 1.4, if signal.is_some() { ink(p) } else { dim(p) });
    // A quarter turn each, centred on straight up.
    let pi = std::f32::consts::PI;
    for i in 1..=3 {
        let color = if i <= bars { ink(p) } else { dim(p) };
        arc(window, origin, 3.5 * i as f32, 1.5, 1.75 * pi, 0.5 * pi, color);
    }
}

pub fn draw_volume(window: &mut Window, r: Bounds<Pixels>, p: &Palette, level: f32, muted: bool) {
    let x = f32::from(r.left()) + 1.0;
    let cy = f32::from(r.center().y);
    polygon(window, &[(x, cy - 2.5), (x + 3.0, cy - 2.5), (x + 7.0, cy - 6.0), (x + 7.0, cy + 6.0), (x + 3.0, cy + 2.5), (x, cy + 2.5)], ink(p));
    if muted {
        let red = c(p.failure);
        window.paint_segment(pt(x + 9.5, cy - 3.0), pt(x + 14.5, cy + 3.0), px(1.4), false, red);
        window.paint_segment(pt(x + 9.5, cy + 3.0), pt(x + 14.5, cy - 3.0), px(1.4), false, red);
        return;
    }
    let waves = if level <= 0.01 { 0 } else if level < 0.34 { 1 } else if level < 0.67 { 2 } else { 3 };
    // Round 3 o'clock, 0.8 radians each way.
    let half_turn = std::f32::consts::FRAC_PI_2;
    for i in 1..=3 {
        let color = if i <= waves { ink(p) } else { dim(p) };
        arc(window, (x + 6.0, cy), 2.5 + 2.6 * i as f32, 1.3, half_turn - 0.8, 1.6, color);
    }
}

/// A newer build is ready: a green arrow rising out of a tray.
pub fn draw_upgrade(window: &mut Window, r: Bounds<Pixels>, p: &Palette) {
    let cx = f32::from(r.center().x);
    let (rt, rb) = (f32::from(r.top()), f32::from(r.bottom()));
    // The arrow, filled, then its outline.
    let (top, neck, foot) = (rt + 1.5, rt + 7.5, rb - 4.5);
    let edge = [(cx, top), (cx + 5.5, neck), (cx + 2.5, neck), (cx + 2.5, foot), (cx - 2.5, foot), (cx - 2.5, neck), (cx - 5.5, neck)];
    polygon(window, &edge, c(p.success));
    polyline(window, &edge, 1.0, true, ink(p));
    // The tray it rises from.
    let (l, right, b) = (whole(window, f32::from(r.left()) + 1.0), whole(window, f32::from(r.right()) - 1.0), whole(window, rb - 1.0));
    let post = b - whole(window, 4.0);
    style::hline_above(window, px(l), px(right), px(b), ink(p));
    style::vline(window, px(l), px(post), px(b), ink(p));
    style::vline_left_of(window, px(right), px(post), px(b), ink(p));
}

/// A bell, with a count of unread notifications; struck through and dim
/// while Do Not Disturb is on.
pub fn draw_bell(window: &mut Window, cx_: &mut App, r: Bounds<Pixels>, p: &Palette, unread: u32, quiet: bool) {
    let color = if quiet { dim(p) } else { ink(p) };
    let cx = f32::from(r.center().x) - 0.5;
    // The body: a dome widening to a lip.
    let top = f32::from(r.top()) + 2.5;
    let lip = f32::from(r.bottom()) - 4.0;
    let mut outline: Vec<(f32, f32)> = (0..=12)
        .map(|k| {
            let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
            (cx + a.cos() * 3.6, top + 3.6 + a.sin() * 3.6)
        })
        .collect();
    outline.extend([(cx + 4.4, lip - 1.5), (cx - 4.4, lip - 1.5)]);
    polygon(window, &outline, color);
    let base = style::snap(window, corners((cx - 6.0, lip - 2.0), (cx + 6.0, lip)));
    round_rect(window, base, 1.0, color);
    // The clapper.
    disc(window, (cx, lip + 1.6), 1.6, color);
    if quiet {
        let (a, b) = (pt(cx - 6.0, top - 0.5), pt(cx + 6.0, lip + 3.0));
        window.paint_segment(a, b, px(3.0), false, c(p.panel_background));
        window.paint_segment(a, b, px(1.4), false, ink(p));
    }
    if unread > 0 {
        badge(window, cx_, r, p, unread as usize, c(p.failure));
    }
}

/// A count in a pill on a glyph's top-right corner.
fn badge(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, p: &Palette, n: usize, fill: Hsla) {
    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
    let font = style::ui_font(FontWeight::BOLD);
    let w = (f32::from(style::text_width(window, text.clone(), 8.5, font.clone())) + 4.0).max(10.0);
    let b = style::snap(window, rect(f32::from(r.right()) + 3.0 - w, f32::from(r.top()) - 1.5, w, 10.0));
    // Ringed in the panel's colour, a line wide (whole device pixels).
    let ring = style::hairline(window);
    let outer = Bounds::from_corners(point(b.left() - ring, b.top() - ring), point(b.right() + ring, b.bottom() + ring));
    round_rect(window, outer, 5.0 + f32::from(ring), c(p.panel_background));
    round_rect(window, b, 5.0, fill);
    style::label(window, cx, b.center().x, b.center().y, style::Align::Center, text, 8.5, font, olo_ui::gpui::white());
}

/// A monitor: its screen (`fill` behind a hairline), on a stand on
/// device pixels; the screen an odd number of points wide, so the stand
/// stands in the middle. Its screen.
fn monitor(window: &mut Window, r: Bounds<Pixels>, p: &Palette, fill: Hsla) -> Bounds<Pixels> {
    let hair = f32::from(style::hairline(window));
    let (l, t) = left_top(r);
    let screen = style::snap(window, rect(l + 0.5, t + 2.0, 15.0, 10.0));
    round_frame(window, screen, 1.5, fill, ink(p));
    let cx = f32::from(screen.center().x);
    let stand = whole(window, cx - hair / 2.0);
    let foot = whole(window, f32::from(r.bottom()) - 1.0);
    style::vline(window, px(stand), screen.bottom(), px(foot), ink(p));
    style::hline_above(window, px(whole(window, cx - 4.0)), px(whole(window, cx + 4.0)), px(foot), ink(p));
    screen
}

/// Agent screens are running: a monitor with a pointer on it, and their
/// count when there is more than one.
pub fn draw_screens(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, p: &Palette, count: usize) {
    let screen = monitor(window, r, p, c(p.keyboard_navigation).opacity(0.35));
    // The agent's pointer, in the middle of the screen.
    let (x, y) = (f32::from(screen.center().x) - 2.5, f32::from(screen.top()) + 1.5);
    polygon(window, &[(x, y), (x, y + 5.5), (x + 4.2, y + 4.2)], ink(p));
    window.paint_segment(pt(x + 1.8, y + 4.0), pt(x + 3.4, y + 6.6), px(1.3), false, ink(p));
    if count > 1 {
        badge(window, cx, r, p, count, c(p.keyboard_navigation));
    }
}

/// The supervisor (Preferences ▸ Performance): a speedometer, a dial open
/// at the bottom with its needle up at speed (the dial tinted) while it's
/// on, resting low and dim while off; what it found as a count, red if
/// any is urgent.
pub fn draw_gauge(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, p: &Palette, on: bool, findings: usize, urgent: bool) {
    let line = if on { ink(p) } else { dim(p) };
    let centre = style::snap_centred(window, pt(f32::from(r.center().x), f32::from(r.top()) + 8.5), 1.0, 1.0).center();
    let centre = (f32::from(centre.x), f32::from(centre.y));
    let radius = 6.6;
    let pi = std::f32::consts::PI;
    // From the lower left over the top to the lower right.
    if on {
        let rim: Vec<(f32, f32)> = (0..=24).map(|i| 1.25 * pi - 1.5 * pi * i as f32 / 24.0).map(|a| (centre.0 + radius * a.cos(), centre.1 - radius * a.sin())).collect();
        polygon(window, &rim, c(p.keyboard_navigation).opacity(0.3));
    }
    arc(window, centre, radius, 1.3, 1.25 * pi, 1.5 * pi, line);
    // (Straight up, clear of the count beside it.)
    let a = if on { pi * 0.5 } else { pi * 1.1 };
    let tip = (centre.0 + a.cos() * (radius - 1.6), centre.1 - a.sin() * (radius - 1.6));
    window.paint_segment(pt(centre.0, centre.1), pt(tip.0, tip.1), px(1.4), false, line);
    disc(window, centre, 1.4, line);
    if on && findings > 0 {
        badge(window, cx, r, p, findings, c(if urgent { p.failure } else { p.warning }));
    }
}

/// This Deskbar's display, of several: a monitor with the display's
/// number in it ("D1" in its workspaces' names).
pub fn draw_display(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, p: &Palette, number: i32) {
    let screen = monitor(window, r, p, c(p.panel_background));
    style::label(window, cx, screen.center().x, screen.center().y, style::Align::Center, olo_protocol::display_number_text(number), 8.0, style::ui_font(FontWeight::BOLD), ink(p));
}

/// A screen being shared: the screen, with the red dot of a recording
/// in its corner (it is being seen elsewhere: never easy to miss).
pub fn draw_sharing(window: &mut Window, r: Bounds<Pixels>, p: &Palette) {
    let screen = monitor(window, r, p, c(p.failure).opacity(0.18));
    // The dot, ringed in the panel's colour so it stands off the frame.
    let at = style::snap_centred(window, pt(f32::from(screen.right()) - 1.0, f32::from(screen.top()) + 1.0), 5.0, 5.0).center();
    let at = (f32::from(at.x), f32::from(at.y));
    disc(window, at, 3.4, c(p.panel_background));
    disc(window, at, 2.5, c(p.failure));
}

/// A box's outline in hairlines on device pixels (its edges inside it).
fn outline(window: &mut Window, b: Bounds<Pixels>, color: Hsla) {
    style::hline(window, b.left(), b.right(), b.top(), color);
    style::hline_above(window, b.left(), b.right(), b.bottom(), color);
    style::vline(window, b.left(), b.top(), b.bottom(), color);
    style::vline_left_of(window, b.right(), b.top(), b.bottom(), color);
}

/// The windowing mode: Standard, two windows one over the other, in
/// outline; Auto-Tiling, a screen in tiles (a half beside two quarters);
/// Focus, one window in a viewfinder's corners. The tiles are filled in the
/// status-bar blue (the mode isn't Standard).
pub fn draw_mode(window: &mut Window, r: Bounds<Pixels>, p: &Palette, mode: olo_protocol::WindowingMode) {
    use olo_protocol::WindowingMode as M;
    let on = mode != M::Standard;
    let line = if on { ink(p) } else { dim(p) };
    let (l, t) = left_top(r);
    let (l, t) = (l + 1.0, t + 2.0);
    let at = |x0: f32, y0: f32, x1: f32, y1: f32| corners((l + x0, t + y0), (l + x1, t + y1));
    let tiles = match mode {
        M::Standard => {
            // The window behind, as far as the one in front lets it show.
            let (back, front) = (style::snap(window, at(0.0, 0.0, 9.0, 8.0)), style::snap(window, at(5.0, 4.0, 14.0, 12.0)));
            style::hline(window, back.left(), back.right(), back.top(), line);
            style::vline(window, back.left(), back.top(), back.bottom(), line);
            style::vline_left_of(window, back.right(), back.top(), front.top(), line);
            style::hline_above(window, back.left(), front.left(), back.bottom(), line);
            vec![front]
        }
        M::AutoTiling => vec![at(0.0, 0.0, 6.0, 12.0), at(7.0, 0.0, 14.0, 5.5), at(7.0, 6.5, 14.0, 12.0)],
        M::Focus => {
            let f = style::snap(window, at(0.0, 0.0, 14.0, 12.0));
            let (ax, ay) = (px(whole(window, 4.0)), px(whole(window, 3.0)));
            style::hline(window, f.left(), f.left() + ax, f.top(), line);
            style::vline(window, f.left(), f.top(), f.top() + ay, line);
            style::hline(window, f.right() - ax, f.right(), f.top(), line);
            style::vline_left_of(window, f.right(), f.top(), f.top() + ay, line);
            style::hline_above(window, f.left(), f.left() + ax, f.bottom(), line);
            style::vline(window, f.left(), f.bottom() - ay, f.bottom(), line);
            style::hline_above(window, f.right() - ax, f.right(), f.bottom(), line);
            style::vline_left_of(window, f.right(), f.bottom() - ay, f.bottom(), line);
            vec![at(3.0, 3.0, 11.0, 9.0)]
        }
    };
    for tile in tiles {
        let tile = style::snap(window, tile);
        if on {
            style::fill_rect(window, tile, c(p.status_bar));
        }
        outline(window, tile, line);
    }
}

/// A coffee cup; full and steaming when `on` (caffeinated). Its lines are
/// hairlines, the cup's sides on device pixels.
pub fn draw_cup(window: &mut Window, r: Bounds<Pixels>, p: &Palette, on: bool) {
    let color = if on { ink(p) } else { dim(p) };
    let hair = f32::from(style::hairline(window));
    let (l, t) = left_top(r);
    let cup = style::snap(window, rect(l + 1.0, t + 6.0, 10.0, 9.0));
    // A bowl: square at the top, round at the bottom.
    let bowl = olo_ui::gpui::Corners { top_left: px(0.0), top_right: px(0.0), bottom_left: px(3.0), bottom_right: px(3.0) };
    if on {
        let coffee = Bounds::from_corners(point(cup.left(), cup.top() + px(whole(window, 2.0))), cup.bottom_right());
        window.paint_quad(quad(coffee, bowl, c(p.hue(Hue::Brown)), px(0.0), transparent_black(), BorderStyle::Solid));
    }
    window.paint_quad(quad(cup, bowl, transparent_black(), px(hair), color, BorderStyle::Solid));
    // The handle, from the middle of the cup's side.
    let c0 = (f32::from(cup.right()) - hair / 2.0, f32::from(cup.top()) + 4.0);
    let handle: Vec<(f32, f32)> = (0..=8)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 8.0;
            (c0.0 + a.cos() * 2.8, c0.1 + a.sin() * 2.3)
        })
        .collect();
    polyline(window, &handle, hair, false, color);
    if on {
        for dx in [3.5, 6.5] {
            let x = f32::from(cup.left()) + dx;
            let steam: Vec<(f32, f32)> = (0..=6)
                .map(|k| {
                    let t = k as f32 / 6.0;
                    (x + (t * std::f32::consts::TAU).sin() * 0.9, f32::from(cup.top()) - 1.5 - t * 4.5)
                })
                .collect();
            polyline(window, &steam, hair, false, color);
        }
    }
}

/// A key, its bow gold while requests are approved without asking. Its
/// lines are hairlines, the shaft and teeth on device pixels.
pub fn draw_key(window: &mut Window, r: Bounds<Pixels>, p: &Palette, on: bool) {
    let color = if on { ink(p) } else { dim(p) };
    let hair = f32::from(style::hairline(window));
    let y = whole(window, f32::from(r.center().y));
    let bow = (f32::from(r.left()) + 4.5, y + hair / 2.0);
    if on {
        disc(window, bow, 3.8, c(p.hue(Hue::Yellow)));
    }
    window.paint_sector(pt(bow.0, bow.1), px(3.8 - hair), px(3.8), 0.0, std::f32::consts::TAU, color);
    disc(window, bow, 1.1, color);
    // The shaft from the bow, its teeth hanging from the end.
    let end = whole(window, f32::from(r.right()) - 0.5);
    style::hline(window, px(whole(window, bow.0 + 3.8)), px(end), px(y), color);
    style::vline(window, px(end - hair), px(y), px(y + whole(window, 4.0)), color);
    style::vline(window, px(end - whole(window, 3.0)), px(y), px(y + whole(window, 3.0)), color);
}

/// Whether the palette is dark (the wordmark's version).
pub fn dark(p: &Palette) -> bool {
    p.mode == ThemeMode::Dark
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_protocol::ClockSettings;

    #[test]
    fn formats() {
        // SAFETY: an all-zero tm is a valid value.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_wday, tm.tm_mon, tm.tm_mday) = (13, 5, 9, 1, 9, 26);
        let mut c = ClockSettings::default();
        assert_eq!(format_clock(&tm, false, &c), "1:05 PM");
        assert_eq!(format_clock(&tm, true, &c), "Mon Oct 26");
        c.twenty_four_hour = true;
        c.show_seconds = true;
        assert_eq!(format_clock(&tm, false, &c), "13:05:09");
        c.show_day = true;
        tm.tm_hour = 0;
        assert_eq!(format_clock(&tm, false, &c), "Mon 00:05:09");
        c.twenty_four_hour = false;
        assert_eq!(format_clock(&tm, false, &c), "Mon 12:05:09 AM");
    }
}
