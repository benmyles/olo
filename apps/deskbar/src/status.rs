//! The tray's view of the machine: clock, battery, Wi-Fi, volume and
//! agent screens, polled on background threads, and the small vector
//! glyphs that show them.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use olo_protocol::NotificationState;
use olo_system::audio;

use olo_kit::{
    egui::{self, Color32, Painter, Rect, Shape, Stroke, pos2, vec2},
    style::{self, c},
    theme::Palette,
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
    /// Unread notifications and Do Not Disturb (None: no notification
    /// service).
    pub notifications: Option<NotificationState>,
    /// Agent screens running (other than the one this Deskbar is on).
    pub agent_screens: usize,
}

/// What olo-session offers to upgrade (see its upgrade.rs).
fn upgrade() -> Vec<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(dir.join(format!("olo-upgrade-{display}.json"))) else { return Vec::new() };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    v["programs"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// The agent screens running: those `olo-screen` lists in
/// `$XDG_RUNTIME_DIR/olo-screens` whose session is still alive (as its
/// `Screen::alive` judges), leaving out the screen this Deskbar is on.
fn agent_screens() -> usize {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let own = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let Ok(read) = std::fs::read_dir(olo_kit::legacy::runtime_file(&dir, "olo-screens")) else { return 0 };
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

fn battery() -> Option<Battery> {
    olo_system::battery::battery().map(|b| Battery { percent: b.percent, charging: b.charging() })
}

fn wifi() -> Option<Wifi> {
    match olo_system::network::wifi() {
        Ok(w) => w.map(|w| Wifi { ssid: w.ssid, signal: w.strength }),
        Err(e) => {
            tracing::debug!("NetworkManager: {e}");
            None
        }
    }
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

/// Watch the machine; `send` is called whenever something changed. Battery
/// and Wi-Fi are polled every few seconds; volume changes arrive as events
/// from the sound server.
pub fn spawn(send: impl Fn(Status) + Send + 'static) {
    let shared = Arc::new(Mutex::new((Status::default(), None::<Status>, send)));
    let polled = shared.clone();
    std::thread::Builder::new()
        .name("deskbar-status".into())
        .spawn(move || {
            loop {
                let (battery, wifi, upgrade) = (battery(), wifi(), upgrade());
                publish(&polled, |s| {
                    s.battery = battery;
                    s.wifi = wifi.clone();
                    s.upgrade = upgrade.clone();
                });
                std::thread::sleep(Duration::from_secs(4));
            }
        })
        .expect("thread");
    // The bell's count follows each notification as it comes.
    let notes = shared.clone();
    std::thread::Builder::new()
        .name("deskbar-notifications".into())
        .spawn(move || {
            loop {
                let state = NotificationState::load();
                publish(&notes, |s| s.notifications = state);
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("thread");
    // Agent screens come and go at an agent's pace: look every second.
    let screens = shared.clone();
    std::thread::Builder::new()
        .name("deskbar-screens".into())
        .spawn(move || {
            loop {
                let n = agent_screens();
                publish(&screens, |s| s.agent_screens = n);
                std::thread::sleep(Duration::from_secs(1));
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

fn ink(p: &Palette) -> Color32 {
    c(p.panel_text)
}

/// Ink for an off or empty part: halfway to the background, so it reads in
/// light and dark alike (a darker shade vanishes on a dark tray).
fn dim(p: &Palette) -> Color32 {
    ink(p).lerp_to_gamma(c(p.panel_background), 0.6)
}

/// `v` points as whole device pixels.
fn whole(painter: &Painter, v: f32) -> f32 {
    let ppp = painter.pixels_per_point();
    (v * ppp).round() / ppp
}

pub fn draw_battery(painter: &Painter, r: Rect, p: &Palette, percent: u32, charging: bool) {
    // On device pixels at any scale: the body's outline a hairline, the
    // nub as far from its top as its bottom, the charge a line inside.
    let ctx = painter.ctx();
    let body = style::snap(ctx, Rect::from_min_size(pos2(r.left(), r.top() + 3.0), vec2(14.0, 10.0)));
    painter.rect_stroke(body, 1.5, Stroke::new(style::hairline(ctx), ink(p)), egui::StrokeKind::Inside);
    let gap = whole(painter, 3.0);
    let nub = Rect::from_min_max(pos2(body.right(), body.top() + gap), pos2(body.right() + whole(painter, 1.5), body.bottom() - gap));
    painter.rect_filled(nub, 0.5, ink(p));
    let level = (percent.min(100) as f32) / 100.0;
    let fill = if charging {
        c(p.success)
    } else if percent <= 10 {
        c(p.failure)
    } else if percent <= 20 {
        c(p.warning)
    } else {
        ink(p)
    };
    let inner = style::inset(ctx, body, 2.0);
    let w = whole(painter, inner.width() * level).max(style::hairline(ctx));
    painter.rect_filled(Rect::from_min_size(inner.min, vec2(w, inner.height())), 0.5, fill);
    if charging {
        let cx = body.center().x;
        let cy = body.center().y;
        let bolt = [pos2(cx + 1.0, cy - 5.0),
            pos2(cx - 2.5, cy + 0.5),
            pos2(cx - 0.2, cy + 0.5),
            pos2(cx - 1.0, cy + 5.0),
            pos2(cx + 2.5, cy - 0.5),
            pos2(cx + 0.2, cy - 0.5)];
        painter.add(Shape::convex_polygon(bolt[..3].to_vec(), c(p.hue(olo_kit::theme::Hue::Yellow)), Stroke::NONE));
        painter.add(Shape::line(bolt.iter().copied().chain([bolt[0]]).collect(), Stroke::new(0.8, ink(p))));
    }
}

pub fn draw_wifi(painter: &Painter, r: Rect, p: &Palette, signal: Option<u32>) {
    let origin = pos2(r.center().x, r.bottom() - 2.5);
    let bars = match signal {
        None => 0,
        Some(s) if s >= 67 => 3,
        Some(s) if s >= 34 => 2,
        Some(_) => 1,
    };
    painter.circle_filled(origin, 1.4, if signal.is_some() { ink(p) } else { dim(p) });
    for i in 1..=3 {
        let radius = 3.5 * i as f32;
        let color = if i <= bars { ink(p) } else { dim(p) };
        let pts: Vec<_> = (0..=12)
            .map(|k| {
                let a = std::f32::consts::PI * (1.25 + 0.5 * k as f32 / 12.0);
                origin + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(1.5, color)));
    }
}

pub fn draw_volume(painter: &Painter, r: Rect, p: &Palette, level: f32, muted: bool) {
    let x = r.left() + 1.0;
    let cy = r.center().y;
    let speaker = vec![
        pos2(x, cy - 2.5),
        pos2(x + 3.0, cy - 2.5),
        pos2(x + 7.0, cy - 6.0),
        pos2(x + 7.0, cy + 6.0),
        pos2(x + 3.0, cy + 2.5),
        pos2(x, cy + 2.5),
    ];
    painter.add(Shape::convex_polygon(speaker, ink(p), Stroke::NONE));
    if muted {
        let s = Stroke::new(1.4, c(p.failure));
        painter.line_segment([pos2(x + 9.5, cy - 3.0), pos2(x + 14.5, cy + 3.0)], s);
        painter.line_segment([pos2(x + 9.5, cy + 3.0), pos2(x + 14.5, cy - 3.0)], s);
        return;
    }
    let waves = if level <= 0.01 { 0 } else if level < 0.34 { 1 } else if level < 0.67 { 2 } else { 3 };
    for i in 1..=3 {
        let radius = 2.5 + 2.6 * i as f32;
        let color = if i <= waves { ink(p) } else { dim(p) };
        let center = pos2(x + 6.0, cy);
        let pts: Vec<_> = (0..=10)
            .map(|k| {
                let a = -0.8 + 1.6 * k as f32 / 10.0;
                center + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(1.3, color)));
    }
}

/// A newer build is ready: a green arrow rising out of a tray.
pub fn draw_upgrade(painter: &Painter, r: Rect, p: &Palette) {
    let cx = r.center().x;
    let green = c(p.success);
    let outline = Stroke::new(1.0, ink(p));
    // The arrow: head and shaft (each convex), then one outline round both.
    let (top, neck, foot) = (r.top() + 1.5, r.top() + 7.5, r.bottom() - 4.5);
    let head = vec![pos2(cx, top), pos2(cx + 5.5, neck), pos2(cx - 5.5, neck)];
    let shaft = Rect::from_min_max(pos2(cx - 2.5, neck), pos2(cx + 2.5, foot));
    painter.add(Shape::convex_polygon(head, green, Stroke::NONE));
    painter.rect_filled(shaft, 0.0, green);
    let edge = vec![
        pos2(cx, top),
        pos2(cx + 5.5, neck),
        pos2(cx + 2.5, neck),
        pos2(cx + 2.5, foot),
        pos2(cx - 2.5, foot),
        pos2(cx - 2.5, neck),
        pos2(cx - 5.5, neck),
        pos2(cx, top),
    ];
    painter.add(Shape::line(edge, outline));
    // The tray it rises from.
    let (l, rt, b) = (whole(painter, r.left() + 1.0), whole(painter, r.right() - 1.0), whole(painter, r.bottom() - 1.0));
    let post = b - whole(painter, 4.0);
    style::hline_above(painter, l, rt, b, ink(p));
    style::vline(painter, l, post, b, ink(p));
    style::vline_left_of(painter, rt, post, b, ink(p));
}

/// A bell, with a count of unread notifications; struck through and dim
/// while Do Not Disturb is on.
pub fn draw_bell(painter: &Painter, r: Rect, p: &Palette, unread: u32, quiet: bool) {
    let color = if quiet { dim(p) } else { ink(p) };
    let cx = r.center().x - 0.5;
    // The body: a dome widening to a lip.
    let top = r.top() + 2.5;
    let lip = r.bottom() - 4.0;
    let mut outline = Vec::new();
    for k in 0..=12 {
        let t = k as f32 / 12.0;
        let a = std::f32::consts::PI * (1.0 + t);
        outline.push(pos2(cx + a.cos() * 3.6, top + 3.6 + a.sin() * 3.6));
    }
    outline.extend([pos2(cx + 4.4, lip - 1.5), pos2(cx - 4.4, lip - 1.5)]);
    painter.add(Shape::convex_polygon(outline, color, Stroke::NONE));
    painter.rect_filled(style::snap(painter.ctx(), Rect::from_min_max(pos2(cx - 6.0, lip - 2.0), pos2(cx + 6.0, lip))), 1.0, color);
    // The clapper.
    painter.circle_filled(pos2(cx, lip + 1.6), 1.6, color);
    if quiet {
        let s = Stroke::new(1.4, ink(p));
        painter.line_segment([pos2(cx - 6.0, top - 0.5), pos2(cx + 6.0, lip + 3.0)], Stroke::new(3.0, c(p.panel_background)));
        painter.line_segment([pos2(cx - 6.0, top - 0.5), pos2(cx + 6.0, lip + 3.0)], s);
    }
    if unread > 0 {
        badge(painter, r, p, unread as usize, c(p.failure));
    }
}

/// A count in a pill on a glyph's top-right corner.
fn badge(painter: &Painter, r: Rect, p: &Palette, n: usize, fill: Color32) {
    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
    let font = egui::FontId::new(8.5, olo_kit::style::bold());
    let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
    let w = (galley.size().x + 4.0).max(10.0);
    let badge = style::snap(painter.ctx(), Rect::from_min_size(pos2(r.right() + 3.0 - w, r.top() - 1.5), vec2(w, 10.0)));
    // Ringed in the panel's colour, a line wide (whole device pixels).
    let ring = style::hairline(painter.ctx());
    painter.rect_filled(badge.expand(ring), 5.0 + ring, c(p.panel_background));
    painter.rect_filled(badge, 5.0, fill);
    painter.galley(badge.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// Agent screens are running: a monitor with a pointer on it, and their
/// count when there is more than one.
pub fn draw_screens(painter: &Painter, r: Rect, p: &Palette, count: usize) {
    // Lines are hairlines on device pixels; the screen is an odd number
    // of points wide, so its stand can stand in the middle.
    let hair = style::hairline(painter.ctx());
    let screen = style::snap(painter.ctx(), Rect::from_min_size(pos2(r.left() + 0.5, r.top() + 2.0), vec2(15.0, 10.0)));
    painter.rect_filled(screen, 1.5, c(p.keyboard_navigation).gamma_multiply(0.35));
    painter.rect_stroke(screen, 1.5, Stroke::new(hair, ink(p)), egui::StrokeKind::Inside);
    // The stand.
    let cx = screen.center().x;
    let stand = whole(painter, cx - hair / 2.0);
    let foot = whole(painter, r.bottom() - 1.0);
    style::vline(painter, stand, screen.bottom(), foot, ink(p));
    style::hline_above(painter, whole(painter, cx - 4.0), whole(painter, cx + 4.0), foot, ink(p));
    // The agent's pointer, in the middle of the screen.
    let (x, y) = (cx - 2.5, screen.top() + 1.5);
    painter.add(Shape::convex_polygon(vec![pos2(x, y), pos2(x, y + 5.5), pos2(x + 4.2, y + 4.2)], ink(p), Stroke::NONE));
    painter.line_segment([pos2(x + 1.8, y + 4.0), pos2(x + 3.4, y + 6.6)], Stroke::new(1.3, ink(p)));
    if count > 1 {
        badge(painter, r, p, count, c(p.keyboard_navigation));
    }
}

/// The windowing mode: Standard, two windows one over the other, in
/// outline; Auto-Tiling, a screen in tiles (a half beside two quarters);
/// Focus, one window in a viewfinder's corners. The tiles are filled in the
/// status-bar blue (the mode isn't Standard).
pub fn draw_mode(painter: &Painter, r: Rect, p: &Palette, mode: olo_protocol::WindowingMode) {
    use olo_protocol::WindowingMode as M;
    let ctx = painter.ctx();
    let on = mode != M::Standard;
    let line = if on { ink(p) } else { dim(p) };
    let (l, t) = (r.left() + 1.0, r.top() + 2.0);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(l + x0, t + y0), pos2(l + x1, t + y1));
    let tiles = match mode {
        M::Standard => {
            // The window behind, as far as the one in front lets it show.
            let (back, front) = (olo_kit::style::snap(ctx, rect(0.0, 0.0, 9.0, 8.0)), olo_kit::style::snap(ctx, rect(5.0, 4.0, 14.0, 12.0)));
            olo_kit::style::hline(painter, back.left(), back.right(), back.top(), line);
            olo_kit::style::vline(painter, back.left(), back.top(), back.bottom(), line);
            olo_kit::style::vline_left_of(painter, back.right(), back.top(), front.top(), line);
            olo_kit::style::hline_above(painter, back.left(), front.left(), back.bottom(), line);
            vec![front]
        }
        M::AutoTiling => vec![rect(0.0, 0.0, 6.0, 12.0), rect(7.0, 0.0, 14.0, 5.5), rect(7.0, 6.5, 14.0, 12.0)],
        M::Focus => {
            let f = olo_kit::style::snap(ctx, rect(0.0, 0.0, 14.0, 12.0));
            let (ax, ay) = (whole(painter, 4.0), whole(painter, 3.0));
            olo_kit::style::hline(painter, f.left(), f.left() + ax, f.top(), line);
            olo_kit::style::vline(painter, f.left(), f.top(), f.top() + ay, line);
            olo_kit::style::hline(painter, f.right() - ax, f.right(), f.top(), line);
            olo_kit::style::vline_left_of(painter, f.right(), f.top(), f.top() + ay, line);
            olo_kit::style::hline_above(painter, f.left(), f.left() + ax, f.bottom(), line);
            olo_kit::style::vline(painter, f.left(), f.bottom() - ay, f.bottom(), line);
            olo_kit::style::hline_above(painter, f.right() - ax, f.right(), f.bottom(), line);
            olo_kit::style::vline_left_of(painter, f.right(), f.bottom() - ay, f.bottom(), line);
            vec![rect(3.0, 3.0, 11.0, 9.0)]
        }
    };
    for tile in tiles {
        let tile = olo_kit::style::snap(ctx, tile);
        if on {
            painter.rect_filled(tile, 0.0, c(p.status_bar));
        }
        olo_kit::style::hline(painter, tile.left(), tile.right(), tile.top(), line);
        olo_kit::style::hline_above(painter, tile.left(), tile.right(), tile.bottom(), line);
        olo_kit::style::vline(painter, tile.left(), tile.top(), tile.bottom(), line);
        olo_kit::style::vline_left_of(painter, tile.right(), tile.top(), tile.bottom(), line);
    }
}

/// A coffee cup; full and steaming when `on` (caffeinated). Its lines are
/// hairlines, the cup's sides on device pixels.
pub fn draw_cup(painter: &Painter, r: Rect, p: &Palette, on: bool) {
    let color = if on { ink(p) } else { dim(p) };
    let hair = style::hairline(painter.ctx());
    let s = Stroke::new(hair, color);
    let bowl = egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 };
    let cup = style::snap(painter.ctx(), Rect::from_min_size(pos2(r.left() + 1.0, r.top() + 6.0), vec2(10.0, 9.0)));
    if on {
        let coffee = Rect::from_min_max(pos2(cup.left(), cup.top() + whole(painter, 2.0)), cup.max);
        painter.rect_filled(coffee, bowl, c(p.hue(olo_kit::theme::Hue::Brown)));
    }
    painter.rect_stroke(cup, bowl, s, egui::StrokeKind::Inside);
    // The handle, from the middle of the cup's side.
    let c0 = pos2(cup.right() - hair / 2.0, cup.top() + 4.0);
    let handle: Vec<_> = (0..=8)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 8.0;
            c0 + vec2(a.cos() * 2.8, a.sin() * 2.3)
        })
        .collect();
    painter.add(Shape::line(handle, s));
    if on {
        for dx in [3.5, 6.5] {
            let x = cup.left() + dx;
            let steam: Vec<_> = (0..=6)
                .map(|k| {
                    let t = k as f32 / 6.0;
                    pos2(x + (t * std::f32::consts::TAU).sin() * 0.9, cup.top() - 1.5 - t * 4.5)
                })
                .collect();
            painter.add(Shape::line(steam, s));
        }
    }
}

/// A key, its bow gold while requests are approved without asking. Its
/// lines are hairlines, the shaft and teeth on device pixels.
pub fn draw_key(painter: &Painter, r: Rect, p: &Palette, on: bool) {
    let color = if on { ink(p) } else { dim(p) };
    let hair = style::hairline(painter.ctx());
    let y = whole(painter, r.center().y);
    let bow = pos2(r.left() + 4.5, y + hair / 2.0);
    if on {
        painter.circle_filled(bow, 3.8, c(p.hue(olo_kit::theme::Hue::Yellow)));
    }
    painter.circle_stroke(bow, 3.8 - hair / 2.0, Stroke::new(hair, color));
    painter.circle_filled(bow, 1.1, color);
    // The shaft from the bow, its teeth hanging from the end.
    let end = whole(painter, r.right() - 0.5);
    style::hline(painter, whole(painter, bow.x + 3.8), end, y, color);
    style::vline(painter, end - hair, y, y + whole(painter, 4.0), color);
    style::vline(painter, end - whole(painter, 3.0), y, y + whole(painter, 3.0), color);
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
