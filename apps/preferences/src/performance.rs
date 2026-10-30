//! Performance: the supervisor that keeps the desktop responsive
//! (olo-supervisor; off by default), what it may do, and every number its
//! judgements go by. The settings live in their own file
//! (`performance.json`), which the supervisor and Deskbar's item follow;
//! a watcher thread keeps the panel current when they change it (Deskbar
//! turning it on, an alert's Ignore This Process).

use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{self, Ui, vec2},
    metrics as m,
    widgets::{BBox, Button, CheckBox, ScrollView, Slider, StringView, TextControl},
};
use olo_protocol::performance::{PerformanceSettings, Thresholds};

pub struct State {
    settings: Option<PerformanceSettings>,
    rx: Receiver<PerformanceSettings>,
    /// Our own last change: the watcher's echoes of older files are
    /// ignored for a moment after it.
    edited: Option<Instant>,
    /// The background programs as typed (comma separated).
    background: Option<String>,
    _watch: Option<olo_kit::watch::Watch>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Default for State {
    fn default() -> Self {
        let (tx, rx) = channel();
        let ctx: Arc<Mutex<Option<egui::Context>>> = Arc::default();
        let read = {
            let ctx = ctx.clone();
            Arc::new(move || {
                let _ = tx.send(PerformanceSettings::load());
                if let Some(c) = ctx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    c.request_repaint();
                }
            })
        };
        let now = read.clone();
        std::thread::Builder::new().name("performance-read".into()).spawn(move || now()).expect("thread");
        let watch = olo_kit::watch::watch([PerformanceSettings::path()], move |_| read());
        State { settings: None, rx, edited: None, background: None, _watch: watch, ctx }
    }
}

#[cfg(test)]
impl State {
    /// The defaults, already read, with an ignored process or two and no
    /// watcher (for the layout lint).
    pub fn sample() -> State {
        let mut s = PerformanceSettings { enabled: true, ..Default::default() };
        s.ignored = vec!["my-long-running-simulation".into(), "blender".into()];
        State { settings: Some(s), rx: channel().1, edited: None, background: None, _watch: None, ctx: Arc::default() }
    }
}

/// Two sliders side by side, each labelled with its value.
fn pair(ui: &mut Ui, a: (&str, &mut f32, std::ops::RangeInclusive<f32>, f32, String), b: Option<(&str, &mut f32, std::ops::RangeInclusive<f32>, f32, String)>) {
    let w = ((ui.available_width() - m::SPACING * 2.0) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING * 2.0;
        let (label, value, range, step, text) = a;
        ui.add(Slider::new(value, range).label(label).value_text(text).step(step).width(w));
        if let Some((label, value, range, step, text)) = b {
            ui.add(Slider::new(value, range).label(label).value_text(text).step(step).width(w));
        }
    });
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(m::TIGHT);
    olo_kit::widgets::heading(ui, text);
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    st.ctx.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| ui.ctx().clone());
    while let Ok(settings) = st.rx.try_recv() {
        if st.edited.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
            st.background = None;
            st.settings = Some(settings);
        }
    }
    let Some(mut s) = st.settings.clone() else {
        ui.add(StringView::new("Reading…").dim(true));
        return;
    };
    let rect = ui.available_rect_before_wrap();
    ScrollView::new("performance").border(false).auto_hide(true).show_in(ui, rect, |ui, _| {
        ui.set_width(rect.width() - widgets_scroll_room());
        BBox::new("Supervisor").show(ui, |ui| {
            ui.add(CheckBox::new(&mut s.enabled, "Keep the desktop responsive"));
            ui.add(
                StringView::new("Lowers builds and other background work so the active window’s app and Olo itself stay quick, and watches for stuck processes, leaks and memory hogs. What it does is in Activity Monitor’s Supervisor section, and Deskbar’s gauge turns it on and off.")
                    .dim(true)
                    .wrap(true),
            );
        });
        ui.add_enabled_ui(s.enabled, |ui| {
            BBox::new("Priorities").show(ui, |ui| {
                ui.add(CheckBox::new(&mut s.priorities, "Lower background work"));
                let mut nice = s.background_nice as f32;
                let text = format!("{nice:.0}");
                pair(ui, ("Lower to nice", &mut nice, 1.0..=19.0, 1.0, text), None);
                s.background_nice = nice as i32;
                let text = st.background.get_or_insert_with(|| s.background.join(", "));
                let r = ui.add(TextControl::new(text).label("Background:").hint("Program names, comma separated"));
                if r.changed() {
                    s.background = text.split(',').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect();
                }
                ui.add(StringView::new("A terminal’s job busy for a minute and more is lowered too. Batch scheduling and idle I/O are put back when this is turned off; a raised nice stays until the program ends.").dim(true).wrap(true));
            });
            BBox::new("Watchdog").show(ui, |ui| {
                ui.add(CheckBox::new(&mut s.watchdog, "Watch for stuck processes, leaks and memory hogs"));
                ui.add(CheckBox::new(&mut s.alerts, "Ask what to do about urgent ones"));
                ui.add(StringView::new("Each is judged from several signals together; a finding says how sure it is and why.").dim(true).wrap(true));
                let th = &mut s.thresholds;
                heading(ui, "Stuck in a loop");
                let (cpu, mins) = (th.spin_cpu, th.spin_minutes);
                pair(ui, ("Busy at least", &mut th.spin_cpu, 50.0..=400.0, 5.0, format!("{cpu:.0}% of a core")), Some(("For", &mut th.spin_minutes, 0.5..=30.0, 0.5, format!("{mins:.1} min"))));
                let (sw, io) = (th.spin_switches, th.spin_io_kb);
                pair(ui, ("Waits less than", &mut th.spin_switches, 0.0..=50.0, 0.5, format!("{sw:.1} a second")), Some(("And I/O under", &mut th.spin_io_kb, 0.0..=256.0, 1.0, format!("{io:.0} KB/s"))));
                heading(ui, "Leaking memory");
                let (lm, lr) = (th.leak_minutes, th.leak_rate_mb);
                pair(ui, ("Growing for", &mut th.leak_minutes, 5.0..=240.0, 5.0, format!("{lm:.0} min")), Some(("At least", &mut th.leak_rate_mb, 0.5..=50.0, 0.5, format!("{lr:.1} MB/min"))));
                let (lg, lf) = (th.leak_growth_mb, th.leak_fit);
                pair(ui, ("By at least", &mut th.leak_growth_mb, 50.0..=4000.0, 50.0, format!("{lg:.0} MB")), Some(("Steadily (fit)", &mut th.leak_fit, 0.5..=0.99, 0.01, format!("R² {lf:.2}"))));
                heading(ui, "Using too much memory");
                let (ms, ma) = (th.memory_share, th.memory_available);
                pair(ui, ("Holding at least", &mut th.memory_share, 5.0..=90.0, 1.0, format!("{ms:.0}% of RAM")), Some(("While less is free than", &mut th.memory_available, 2.0..=50.0, 1.0, format!("{ma:.0}%"))));
                let stall = th.memory_stall;
                pair(ui, ("Or stalls over", &mut th.memory_stall, 1.0..=80.0, 1.0, format!("{stall:.0}% of the time")), None);
                heading(ui, "Leaving zombies");
                let (zm, mut zl, mut zc) = (th.zombie_minutes, th.zombie_least as f32, th.zombie_count as f32);
                let (zl_text, zc_text) = (format!("{zl:.0} of them"), format!("{zc:.0} of them"));
                pair(ui, ("Unreaped for", &mut th.zombie_minutes, 0.5..=60.0, 0.5, format!("{zm:.1} min")), Some(("By a parent leaving", &mut zl, 1.0..=20.0, 1.0, zl_text)));
                pair(ui, ("Urgent at", &mut zc, 5.0..=500.0, 5.0, zc_text), None);
                th.zombie_least = zl as u32;
                th.zombie_count = zc as u32;
                ui.add_space(m::TIGHT);
                let changed = *th != Thresholds::default();
                if ui.add_enabled(changed, Button::new("Reset Thresholds")).clicked() {
                    *th = Thresholds::default();
                }
            });
            BBox::new("Never Warned About").show(ui, |ui| {
                if s.ignored.is_empty() {
                    ui.add(StringView::new("None. An alert’s Ignore This Process adds one here.").dim(true).wrap(true));
                }
                let mut remove = None;
                for (i, name) in s.ignored.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(StringView::new(name));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(Button::new("Remove").min_width(72.0)).clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    s.ignored.remove(i);
                }
            });
        });
        ui.allocate_space(vec2(0.0, m::TIGHT));
    });
    if st.settings.as_ref() != Some(&s) {
        st.settings = Some(s.clone());
        st.edited = Some(Instant::now());
        std::thread::spawn(move || {
            if let Err(e) = s.save() {
                tracing::warn!("performance settings: {e}");
            }
        });
    }
}

/// Room kept at the right for the scroll bar when it shows.
fn widgets_scroll_room() -> f32 {
    olo_kit::widgets::SCROLL_BAR + m::TIGHT
}
