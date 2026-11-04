//! Performance: the supervisor that keeps the desktop responsive
//! (olo-supervisor; off by default), what it may do, and every number its
//! judgements go by. The settings live in their own file
//! (`performance.json`), which the supervisor and Deskbar's item follow;
//! a watcher thread keeps the panel current when they change it (Deskbar
//! turning it on, an alert's Ignore This Process).

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};

use olo_protocol::performance::{PerformanceSettings, Thresholds};
use olo_ui::{
    gpui::{Context, Entity, ParentElement as _, SharedString, Styled as _, Window, div, px},
    metrics as m,
    widgets::{Button, CheckBox, Slider, StringView, TabView, TextControl, TextField, TextFieldEvent, heading},
};

use crate::{Msg, PanelView, form::Ui, panel};

pub struct State {
    settings: Option<PerformanceSettings>,
    rx: Receiver<PerformanceSettings>,
    /// Our own last change: the watcher's echoes of older files are
    /// ignored for a moment after it.
    edited: Option<Instant>,
    /// The background programs (comma separated).
    background: Entity<TextField>,
    _watch: Option<olo_ui::watch::Watch>,
    /// The tab shown: 0 Supervisor, 1 Watchdog.
    tab: usize,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let (tx, rx) = channel();
        // Tests (the layout lint): the defaults, not this account's file.
        let (settings, watch) = if cfg!(test) {
            drop(tx);
            (Some(PerformanceSettings::default()), None)
        } else {
            let proxy = olo_ui::proxy(cx);
            let read = Arc::new(move || {
                let _ = tx.send(PerformanceSettings::load());
                proxy.send(Msg::Redraw);
            });
            let now = read.clone();
            std::thread::Builder::new().name("performance-read".into()).spawn(move || now()).expect("thread");
            (None, olo_ui::watch::watch([PerformanceSettings::path()], move |_| read()))
        };
        let background = olo_ui::widgets::text_field(cx, |f| f.hint("Program names, comma separated"));
        cx.subscribe(&background, |v, field, e: &TextFieldEvent, cx| {
            if !matches!(e, TextFieldEvent::Changed) {
                return;
            }
            let text = field.read(cx).text().to_owned();
            change(v, |s| s.background = text.split(',').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect());
            cx.notify();
        })
        .detach();
        State { settings, rx, edited: None, background, _watch: watch, tab: 0 }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Performance(st) => Some(st),
        _ => None,
    }
}

/// A change to the settings, saved (off the UI thread).
fn change(v: &mut PanelView, f: impl FnOnce(&mut PerformanceSettings)) {
    let Some(st) = state_of(v) else { return };
    let Some(s) = st.settings.as_mut() else { return };
    let before = s.clone();
    f(s);
    if *s == before {
        return;
    }
    st.edited = Some(Instant::now());
    let s = s.clone();
    std::thread::spawn(move || {
        if let Err(e) = s.save() {
            tracing::warn!("performance settings: {e}");
        }
    });
}

/// A control's change to the settings.
fn set<T: 'static>(ui: &Ui, f: impl Fn(&mut PerformanceSettings, T) + 'static) -> impl Fn(T, &mut Window, &mut olo_ui::gpui::App) + 'static {
    ui.on_view(move |v, value: T, _, cx| {
        change(v, |s| f(s, value));
        cx.notify();
    })
}

fn check(ui: &mut Ui, label: &str, on: bool, f: impl Fn(&mut PerformanceSettings, bool) + 'static) {
    let c = CheckBox::new(label.to_owned(), on).enabled(ui.enabled()).on_toggle(set(ui, f));
    ui.add(c);
}

/// A slider labelled with its value.
type Knob<'a> = (&'a str, f32, std::ops::RangeInclusive<f32>, f32, String, fn(&mut Thresholds, f32));

/// Two sliders side by side, each labelled with its value.
fn pair(ui: &mut Ui, a: Knob, b: Option<Knob>) {
    let enabled = ui.enabled();
    let alone = b.is_none();
    let mut row = div().flex().flex_row().gap(px(m::SPACING * 2.0));
    for (label, value, range, step, text, apply) in std::iter::once(a).chain(b) {
        let on = set(ui, move |s, v: f32| apply(&mut s.thresholds, v));
        row = row.child(div().flex_1().child(Slider::new(format!("perf-{label}"), value, range).label(label.to_owned()).value_text(text).step(step).enabled(enabled).on_change(on)));
    }
    // One alone is as wide as one of a pair.
    if alone {
        row = row.child(div().flex_1());
    }
    ui.add(row);
}

fn section(ui: &mut Ui, text: &str) {
    ui.space(m::TIGHT);
    ui.add(heading(text.to_owned()));
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    while let Ok(settings) = st.rx.try_recv() {
        if st.edited.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
            let text = settings.background.join(", ");
            st.background.update(ui.cx, |f, cx| f.set_text(text, cx));
            st.settings = Some(settings);
        }
    }
    let Some(s) = st.settings.clone() else {
        ui.note("Reading…");
        return;
    };
    let background = st.background.clone();
    // Two tabs, so it fits a laptop's screen: what the supervisor does, and
    // the watchdog's thresholds.
    let tab = st.tab;
    let items = ui.collect(|ui| {
        if tab == 0 {
            // The power mode is Power's; found from here too.
            let open = ui.on_view(|v, _: (), _, cx| {
                v.prefs.update(cx, |p, cx| p.open("power", cx));
            });
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().child(StringView::new("The power mode (Power Saver, Balanced or Performance) is set in Power.").wrap(true)))
                    .child(Button::new("Open Power").on_click(move |window, cx| open((), window, cx))),
            );
            ui.bbox("Supervisor", |ui| {
                check(ui, "Keep the desktop responsive", s.enabled, |s, v| s.enabled = v);
                ui.note("Lowers builds and other background work so the active window’s app and Olo itself stay quick, and watches for stuck processes, leaks and memory hogs. What it does is in Activity Monitor’s Supervisor section, and Deskbar’s gauge turns it on and off.");
            });
        }
        ui.enabled_if(s.enabled, |ui| {
            if tab == 0 {
                ui.bbox("Priorities", |ui| {
                    check(ui, "Lower background work", s.priorities, |s, v| s.priorities = v);
                    let nice = s.background_nice as f32;
                    let on = set(ui, |s, v: f32| s.background_nice = v as i32);
                    let enabled = ui.enabled();
                    ui.add(div().flex().flex_row().gap(px(m::SPACING * 2.0)).child(div().flex_1().child(Slider::new("perf-nice", nice, 1.0..=19.0).label("Lower to nice").value_text(format!("{nice:.0}")).step(1.0).enabled(enabled).on_change(on))).child(div().flex_1()));
                    background.update(ui.cx, |f, cx| f.set_enabled(enabled, cx));
                    ui.add(TextControl::new(background.clone()).label("Background:"));
                    ui.note("A terminal’s job busy for a minute and more is lowered too. Batch scheduling and idle I/O are put back when this is turned off; a raised nice stays until the program ends.");
                });
                ui.bbox("Never Warned About", |ui| {
                    if s.ignored.is_empty() {
                        ui.note("None. An alert’s Ignore This Process adds one here.");
                    }
                    for (i, name) in s.ignored.iter().enumerate() {
                        let remove = set(ui, move |s, _: ()| {
                            if i < s.ignored.len() {
                                s.ignored.remove(i);
                            }
                        });
                        ui.add(div().flex().flex_row().items_center().child(div().flex_1().child(StringView::new(name.clone()))).child(Button::new("Remove").min_width(72.0).on_click(move |window, cx| remove((), window, cx))));
                    }
                });
                return;
            }
            ui.bbox("Watchdog", |ui| {
                check(ui, "Watch for stuck processes, leaks and memory hogs", s.watchdog, |s, v| s.watchdog = v);
                check(ui, "Ask what to do about urgent ones", s.alerts, |s, v| s.alerts = v);
                ui.note("Each is judged from several signals together; a finding says how sure it is and why.");
                let th = &s.thresholds;
                // Four kinds, two by two (so it fits a laptop's screen).
                let stuck = ui.collect(|ui| {
                    section(ui, "Stuck in a loop");
                    pair(ui, ("Busy at least", th.spin_cpu, 50.0..=400.0, 5.0, format!("{:.0}% of a core", th.spin_cpu), |t, v| t.spin_cpu = v), Some(("For", th.spin_minutes, 0.5..=30.0, 0.5, format!("{:.1} min", th.spin_minutes), |t, v| t.spin_minutes = v)));
                    pair(ui, ("Waits less than", th.spin_switches, 0.0..=50.0, 0.5, format!("{:.1} a second", th.spin_switches), |t, v| t.spin_switches = v), Some(("And I/O under", th.spin_io_kb, 0.0..=256.0, 1.0, format!("{:.0} KB/s", th.spin_io_kb), |t, v| t.spin_io_kb = v)));
                });
                let leaks = ui.collect(|ui| {
                    section(ui, "Leaking memory");
                    pair(ui, ("Growing for", th.leak_minutes, 5.0..=240.0, 5.0, format!("{:.0} min", th.leak_minutes), |t, v| t.leak_minutes = v), Some(("At least", th.leak_rate_mb, 0.5..=50.0, 0.5, format!("{:.1} MB/min", th.leak_rate_mb), |t, v| t.leak_rate_mb = v)));
                    pair(ui, ("By at least", th.leak_growth_mb, 50.0..=4000.0, 50.0, format!("{:.0} MB", th.leak_growth_mb), |t, v| t.leak_growth_mb = v), Some(("Steadily (fit)", th.leak_fit, 0.5..=0.99, 0.01, format!("R² {:.2}", th.leak_fit), |t, v| t.leak_fit = v)));
                });
                let memory = ui.collect(|ui| {
                    section(ui, "Using too much memory");
                    pair(ui, ("Holding at least", th.memory_share, 5.0..=90.0, 1.0, format!("{:.0}% of RAM", th.memory_share), |t, v| t.memory_share = v), Some(("While less is free than", th.memory_available, 2.0..=50.0, 1.0, format!("{:.0}%", th.memory_available), |t, v| t.memory_available = v)));
                    pair(ui, ("Or stalls over", th.memory_stall, 1.0..=80.0, 1.0, format!("{:.0}% of the time", th.memory_stall), |t, v| t.memory_stall = v), None);
                });
                let zombies = ui.collect(|ui| {
                    section(ui, "Leaving zombies");
                    pair(ui, ("Unreaped for", th.zombie_minutes, 0.5..=60.0, 0.5, format!("{:.1} min", th.zombie_minutes), |t, v| t.zombie_minutes = v), Some(("By a parent leaving", th.zombie_least as f32, 1.0..=20.0, 1.0, format!("{} of them", th.zombie_least), |t, v| t.zombie_least = v as u32)));
                    pair(ui, ("Urgent at", th.zombie_count as f32, 5.0..=500.0, 5.0, format!("{} of them", th.zombie_count), |t, v| t.zombie_count = v as u32), None);
                });
                let column = |items: Vec<olo_ui::gpui::AnyElement>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
                for (a, b) in [(stuck, leaks), (memory, zombies)] {
                    ui.add(div().flex().flex_row().gap(px(m::SPACING * 3.0)).child(column(a)).child(column(b)));
                }
                ui.space(m::TIGHT);
                let changed = *th != Thresholds::default();
                let reset = set(ui, |s, _: ()| s.thresholds = Thresholds::default());
                let enabled = ui.enabled();
                ui.add(Button::new("Reset Thresholds").enabled(changed && enabled).on_click(move |window, cx| reset((), window, cx)));
            });
        });
    });
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.tab = i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Supervisor", "Watchdog"].into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("performance-tabs", tabs, tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}
