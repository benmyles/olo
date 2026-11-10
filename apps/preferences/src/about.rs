//! About Olo: the BeOS "About This System" window — version, machine,
//! processor, memory, kernel and uptime. Facts are read from /proc and
//! /sys on a thread; uptime then ticks on locally.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use olo_sync::Mutex;
use olo_ui::{
    brand,
    gpui::{Bounds, Context, IntoElement as _, ParentElement as _, Styled as _, canvas, div, point, px, size},
    icons, metrics as m, style, theme,
    widgets::{BBox, Button, StringView, heading},
};

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

pub struct State {
    facts: Arc<Mutex<Option<Facts>>>,
    /// When Restart Olo was last clicked, or why it failed.
    restarted: Option<Instant>,
    error: Option<String>,
    /// A frame is due at the next second (uptime counts them).
    ticking: bool,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let facts = Arc::new(Mutex::new(None));
        // (Tests, the layout lint: a machine described at length, not
        // this one.)
        if cfg!(test) {
            *facts.lock().unwrap() = Some(test_facts());
            return State { facts, restarted: None, error: None, ticking: false };
        }
        let (out, proxy) = (facts.clone(), olo_ui::proxy(cx));
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(gather());
            proxy.send(Msg::Redraw);
        });
        State { facts, restarted: None, error: None, ticking: false }
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

pub fn ui(ui: &mut Ui, st: &mut State) {
    let dark = theme::palette(ui.cx).mode == olo_ui::palette::ThemeMode::Dark;
    let word_w = brand::wordmark_width(30.0, dark);
    let icon = canvas(
        |_, _, _| {},
        |b, _, window, _| {
            icons::paint(window, b, "olo");
        },
    )
    .w(px(64.0))
    .h(px(64.0))
    .flex_shrink_0();
    let title = match word_w {
        Some(w) => canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let at = Bounds { origin: point(b.left(), b.top()), size: size(px(w), px(30.0)) };
                brand::wordmark_left(window, at, dark);
            },
        )
        .w(px(w))
        .h(px(30.0))
        .into_any_element(),
        None => heading("Olo Desktop").into_any_element(),
    };
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(icon)
            .child(div().flex().flex_col().pt(px(6.0)).child(title).child(StringView::new(format!("Olo Desktop, version {}", env!("CARGO_PKG_VERSION"))).dim(true))),
    );
    let facts = st.facts.lock().unwrap().clone();
    let rows = ui.collect(|ui| {
        let Some(f) = facts else {
            ui.note("Looking…");
            return;
        };
        let uptime = f.uptime.map(|(d, at)| uptime_text(d + at.elapsed())).unwrap_or_default();
        for (label, value) in [("Machine:", &f.machine), ("Processor:", &f.processor), ("Memory:", &f.memory), ("Kernel:", &f.kernel), ("Uptime:", &uptime)] {
            // The label right-aligned in its column, as BeOS's.
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .child(div().w(px(90.0)).flex_shrink_0().flex().justify_end().pr(px(m::LABEL_GAP)).child(StringView::new(label)))
                    // (A long name wraps, under itself: a ThinkPad's model,
                    // a kernel's build.)
                    .child(div().flex_1().min_w(px(0.0)).pt(px((m::CONTROL_HEIGHT - style::line_row(style::PLAIN_SIZE)) / 2.0)).child(StringView::new(value.clone()).wrap(true))),
            );
        }
    });
    ui.add(BBox::plain().children(rows));
    // Uptime counts seconds.
    if !st.ticking {
        st.ticking = true;
        ui.cx
            .spawn(async move |view, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = view.update(cx, |v, cx| {
                    if let Some(st) = state_of(v) {
                        st.ticking = false;
                    }
                    cx.notify();
                });
            })
            .detach();
    }
    ui.space(m::SPACING);
    let pid = session_pid();
    // A restart takes a moment; don't let clicks pile up meanwhile.
    let settling = st.restarted.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
    let note = match (&st.error, pid, settling) {
        (Some(e), _, _) => format!("Couldn't restart: {e}"),
        (None, None, _) => "Not running under olo-session.".to_owned(),
        (None, Some(_), true) => "Restarting…".to_owned(),
        (None, Some(_), false) => String::new(),
    };
    let restart_now = ui.on_click_view(move |v, _, cx| {
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
            .child(Button::new("Restart Olo").enabled(pid.is_some() && !settling).on_click(restart_now)),
    );
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
