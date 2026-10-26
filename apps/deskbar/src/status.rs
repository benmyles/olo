//! The tray's view of the machine: clock, battery, Wi-Fi and volume, polled
//! on a background thread, and the small vector glyphs that show them.

use std::{path::Path, process::Command, time::Duration};

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

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn battery() -> Option<Battery> {
    for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path();
        if read(&p.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let percent = read(&p.join("capacity"))?.parse().ok()?;
        let status = read(&p.join("status")).unwrap_or_default();
        return Some(Battery { percent, charging: status == "Charging" || status == "Full" });
    }
    None
}

fn wifi() -> Option<Wifi> {
    // Only show Wi-Fi on machines that have it.
    let has = std::fs::read_dir("/sys/class/net").ok()?.flatten().any(|e| e.path().join("wireless").exists());
    if !has {
        return None;
    }
    let out = Command::new("nmcli").args(["-t", "-f", "ACTIVE,SSID,SIGNAL", "dev", "wifi"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        // ACTIVE:SSID:SIGNAL with ':' in SSIDs escaped as '\:'.
        let fields: Vec<String> = split_escaped(line);
        if fields.len() >= 3 && fields[0] == "yes" {
            return Some(Wifi { ssid: Some(fields[1].clone()), signal: fields[2].parse().unwrap_or(0) });
        }
    }
    Some(Wifi { ssid: None, signal: 0 })
}

fn split_escaped(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            out.last_mut().unwrap().push(ch);
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(ch);
        }
    }
    out
}

fn volume() -> Option<Volume> {
    let out = Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SINK@"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let level = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(Volume { level, muted: text.contains("MUTED") })
}

pub fn toggle_mute() {
    let _ = Command::new("wpctl").args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).status();
}

pub fn set_volume(level: f32) {
    let _ = Command::new("wpctl").args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{level:.2}")]).status();
}

/// Poll every few seconds; `send` is called whenever something changed.
pub fn spawn(send: impl Fn(Status) + Send + 'static) {
    std::thread::Builder::new()
        .name("deskbar-status".into())
        .spawn(move || {
            let mut last = None;
            let mut tick = 0u32;
            let mut wifi_cache = None;
            loop {
                // nmcli is comparatively slow; ask it less often.
                if tick % 3 == 0 {
                    wifi_cache = wifi();
                }
                let s = Status { battery: battery(), wifi: wifi_cache.clone(), volume: volume() };
                if last.as_ref() != Some(&s) {
                    send(s.clone());
                    last = Some(s);
                }
                tick = tick.wrapping_add(1);
                std::thread::sleep(Duration::from_secs(4));
            }
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
        let bolt = vec![
            pos2(cx + 1.0, cy - 5.0),
            pos2(cx - 2.5, cy + 0.5),
            pos2(cx - 0.2, cy + 0.5),
            pos2(cx - 1.0, cy + 5.0),
            pos2(cx + 2.5, cy - 0.5),
            pos2(cx + 0.2, cy - 0.5),
        ];
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
