//! About Tabula: the BeOS "About This System" window — version, machine,
//! processor, memory, kernel and uptime. Facts are read from /proc and
//! /sys on a thread; uptime then ticks on locally.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tabula_kit::{
    egui::{Sense, Ui, vec2},
    icons, metrics as m,
    widgets::{BBox, Button, StringView, form_label, heading},
};

#[derive(Default, Clone)]
struct Facts {
    machine: String,
    processor: String,
    memory: String,
    kernel: String,
    /// Uptime when read, and when that was.
    uptime: Option<(Duration, Instant)>,
}

pub struct State {
    facts: Arc<Mutex<Option<Facts>>>,
    /// When Restart Tabula was last clicked, or why it failed.
    restarted: Option<Instant>,
    error: Option<String>,
}

impl State {
    pub fn new() -> State {
        let facts = Arc::new(Mutex::new(None));
        let out = facts.clone();
        std::thread::spawn(move || *out.lock().unwrap() = Some(gather()));
        State { facts, restarted: None, error: None }
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim_end_matches(['\0', '\n']).trim().to_owned()
}

fn gather() -> Facts {
    let machine = [read("/sys/firmware/devicetree/base/model"), read("/sys/class/dmi/id/product_name")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown".into());
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let cpuinfo = read("/proc/cpuinfo");
    let model = cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_owned()));
    let chip = [read("/sys/firmware/devicetree/base/compatible")
        .split('\0')
        .find(|s| s.starts_with("apple,t"))
        .map(apple_chip)]
    .into_iter()
    .flatten()
    .flatten()
    .next();
    let processor = match chip.or(model) {
        Some(name) => format!("{name}, {cpus} cores"),
        None => format!("{cpus} × {}", std::env::consts::ARCH),
    };
    let meminfo = read("/proc/meminfo");
    let total_kb: u64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    let memory = format!("{} GB", installed_gb(total_kb));
    let kernel = format!("Linux {}", read("/proc/sys/kernel/osrelease"));
    let uptime = read("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| (Duration::from_secs_f64(s), Instant::now()));
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

/// The tabula-session supervising us, if we run under one.
fn session_pid() -> Option<i32> {
    std::env::var("TABULA_SESSION_PID").ok()?.parse().ok().filter(|&p| p > 1)
}

/// Restart Tabula in place, as `tabula-session reload` does: the session
/// re-execs app_server, and every app (this one included) reconnects with
/// its windows where they were.
fn restart(pid: i32) -> Result<(), String> {
    // SAFETY: sending a signal.
    if unsafe { libc::kill(pid, libc::SIGHUP) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
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

pub fn ui(ui: &mut Ui, st: &mut State) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        icons::paint(ui.painter(), rect, "about");
        ui.add_space(m::SPACING);
        ui.vertical(|ui| {
            ui.add_space(12.0);
            heading(ui, "Tabula");
            ui.add(StringView::new(&format!("Version {}", env!("CARGO_PKG_VERSION"))).dim(true));
        });
    });
    let facts = st.facts.lock().unwrap().clone();
    BBox::plain().show(ui, |ui| {
        let Some(f) = facts else {
            ui.add(StringView::new("Looking…").dim(true));
            return;
        };
        let uptime = f.uptime.map(|(d, at)| uptime_text(d + at.elapsed())).unwrap_or_default();
        for (label, value) in [
            ("Machine:", &f.machine),
            ("Processor:", &f.processor),
            ("Memory:", &f.memory),
            ("Kernel:", &f.kernel),
            ("Uptime:", &uptime),
        ] {
            ui.horizontal(|ui| {
                form_label(ui, label, 90.0);
                ui.add(StringView::new(value));
            });
        }
        // Uptime counts seconds.
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    });
    ui.add_space(m::SPACING);
    ui.horizontal(|ui| {
        let pid = session_pid();
        // A restart takes a moment; don't let clicks pile up meanwhile.
        let settling = st.restarted.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        let note = match (&st.error, pid, settling) {
            (Some(e), _, _) => format!("Couldn't restart: {e}"),
            (None, None, _) => "Not running under tabula-session.".to_owned(),
            (None, Some(_), true) => "Restarting…".to_owned(),
            (None, Some(_), false) => String::new(),
        };
        ui.with_layout(tabula_kit::egui::Layout::right_to_left(tabula_kit::egui::Align::Center), |ui| {
            let button = ui.add_enabled_ui(pid.is_some() && !settling, |ui| ui.add(Button::new("Restart Tabula")));
            if let (true, Some(pid)) = (button.inner.clicked(), pid) {
                st.error = restart(pid).err();
                st.restarted = Some(Instant::now());
            }
            ui.add(StringView::new(&note).dim(true));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
