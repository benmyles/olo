//! The tray's view of the machine: clock, battery, Wi-Fi and volume, polled
//! on a background thread, and the small vector glyphs that show them.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use tabula_system::audio;

use tabula_kit::{
    egui::{self, Color32, Painter, Rect, Shape, Stroke, pos2, vec2},
    style::{c, shade},
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
}

fn battery() -> Option<Battery> {
    tabula_system::battery::battery().map(|b| Battery { percent: b.percent, charging: b.charging() })
}

fn wifi() -> Option<Wifi> {
    match tabula_system::network::wifi() {
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
                let (battery, wifi) = (battery(), wifi());
                publish(&polled, |s| {
                    s.battery = battery;
                    s.wifi = wifi.clone();
                });
                std::thread::sleep(Duration::from_secs(4));
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

fn local_now() -> libc::tm {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

pub fn clock_text(date: bool) -> String {
    let tm = local_now();
    if date {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {} {}", DAYS[tm.tm_wday as usize % 7], MONTHS[tm.tm_mon as usize % 12], tm.tm_mday)
    } else {
        let h = tm.tm_hour % 12;
        format!("{}:{:02} {}", if h == 0 { 12 } else { h }, tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" })
    }
}

pub fn until_next_minute() -> Duration {
    let tm = local_now();
    Duration::from_secs((60 - tm.tm_sec.clamp(0, 59)) as u64) + Duration::from_millis(50)
}

// ---- glyphs -------------------------------------------------------------------

fn ink(p: &Palette) -> Color32 {
    c(p.panel_text)
}

pub fn draw_battery(painter: &Painter, r: Rect, p: &Palette, percent: u32, charging: bool) {
    let body = Rect::from_min_size(pos2(r.left() + 0.5, r.top() + 4.5), vec2(13.0, 8.0));
    painter.rect_stroke(body, 1.5, Stroke::new(1.0, ink(p)), egui::StrokeKind::Middle);
    painter.rect_filled(Rect::from_min_size(pos2(body.right() + 0.5, body.center().y - 2.0), vec2(1.5, 4.0)), 0.5, ink(p));
    let level = (percent.min(100) as f32) / 100.0;
    let fill = if charging {
        c(p.success)
    } else if percent <= 10 {
        c(p.failure)
    } else if percent <= 20 {
        Color32::from_rgb(230, 160, 0)
    } else {
        ink(p)
    };
    let inner = body.shrink(1.5);
    let w = (inner.width() * level).max(1.0);
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
        painter.add(Shape::convex_polygon(bolt[..3].to_vec(), Color32::from_rgb(255, 214, 0), Stroke::NONE));
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
    let dim = shade(p, p.panel_background, 1.35);
    painter.circle_filled(origin, 1.4, if signal.is_some() { ink(p) } else { dim });
    for i in 1..=3 {
        let radius = 3.5 * i as f32;
        let color = if i <= bars { ink(p) } else { dim };
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
        let color = if i <= waves { ink(p) } else { shade(p, p.panel_background, 1.35) };
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

/// A coffee cup; full and steaming when `on` (caffeinated).
pub fn draw_cup(painter: &Painter, r: Rect, p: &Palette, on: bool) {
    let color = if on { ink(p) } else { shade(p, p.panel_background, 1.6) };
    let s = Stroke::new(1.2, color);
    let cup = Rect::from_min_size(pos2(r.left() + 1.5, r.top() + 6.5), vec2(9.0, 8.0));
    if on {
        let coffee = Rect::from_min_max(pos2(cup.left(), cup.top() + 1.5), cup.max);
        painter.rect_filled(coffee, egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, Color32::from_rgb(130, 82, 45));
    }
    painter.rect_stroke(cup, egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, s, egui::StrokeKind::Middle);
    // The handle.
    let c0 = pos2(cup.right() + 0.5, cup.top() + 3.5);
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
            painter.add(Shape::line(steam, Stroke::new(1.0, color)));
        }
    }
}
