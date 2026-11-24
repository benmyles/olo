//! Three immediate power policies. Driver readback is collected on demand,
//! off the window thread, and follows the follower while its pane is open.
use crate::{
    Msg, PanelView,
    form::{Ui, divider_for},
    server::Server,
};
use olo_kit::{
    Cx, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, MenuField, PaneSwitch, RadioButton, StringView, TabView},
};
use olo_protocol::{
    PowerStatus, Settings,
    power_mode::{Mode, Modes},
};
use olo_sync::Mutex;
use olo_system::{
    battery::{self, Battery, State as Charge},
    fans, gpu_tuning,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};

pub const TABS: [&str; 2] = ["Power Mode", "Battery & Idle"];

pub fn show_tab(st: &mut State, name: &str) {
    st.tab = usize::from(name.eq_ignore_ascii_case(TABS[1]));
    if name.eq_ignore_ascii_case("Graphics") {
        st.set_details(true);
    }
}

#[derive(Default, Clone)]
struct Facts {
    battery: Option<Battery>,
    limit: Option<battery::ChargeLimit>,
    presence: Option<bool>,
    charge_writable: bool,
    hardware: Vec<(String, String)>,
    issue: Option<String>,
    /// What each mode is set to do, as the computer keeps it (root's
    /// file), and what there is here for a mode to set: the GPUs (their
    /// PCI address, maker, whether NVIDIA's), the drives (serial, model)
    /// and whether the fans can be set.
    plan: gpu_tuning::Plan,
    gpus: Vec<(String, String, bool)>,
    drives: Vec<(String, String)>,
    fans: bool,
}

/// How a change to what the modes do is going (it is root's to keep:
/// asked for on a thread of its own, an administrator's password first).
#[derive(Debug, Clone, PartialEq, Default)]
enum Applying {
    #[default]
    No,
    Now,
    Done,
    Failed(String),
}

/// The top speeds a mode can hold the processors and a GPU to, in
/// percent of their fastest (Ben: "100% default but i can tune it down in
/// 10% increments to a min of 50%").
const TOPS: [u8; 6] = [100, 90, 80, 70, 60, 50];
/// A mode's column: how wide a preset's menu is, the "up to" between it
/// and its top speed, and the top speed's menu.
const PRESET: f32 = 130.0;
const UP_TO: f32 = 48.0;
const TOP: f32 = 80.0;

fn demo() -> bool {
    cfg!(test) || std::env::var_os("OLO_POWER_DEMO").is_some()
}

impl Facts {
    /// What each mode is set to do as the computer keeps it, and what
    /// there is here for a mode to set (the GPUs, the drives, the fans).
    fn read_what_modes_set(&mut self) {
        self.plan = gpu_tuning::Tuning::load().plan;
        self.gpus = olo_system::gpus::list().into_iter().filter(|g| matches!(g.driver.as_str(), "xe" | "nvidia")).map(|g| (g.pci.clone(), g.maker().to_owned(), g.vendor == 0x10de)).collect();
        self.drives = olo_system::nvme_power::drives().into_iter().map(|d| (d.serial, d.model)).collect();
        self.fans = fans::look(&fans::Places::here()).is_some_and(|f| f.installed);
    }

    fn read(details: bool) -> Self {
        if demo() {
            return Self {
                battery: Some(Battery { percent: 79, state: Charge::NotCharging, time_to_empty: None, time_to_full: None, cycle_count: None, health: None, charge_limit: Some(80), power_watts: None }),
                limit: Some(battery::ChargeLimit { enabled: true, start: 75, end: 80, path: String::new() }),
                charge_writable: true,
                presence: Some(true),
                hardware: vec![
                    ("CPU".into(), "Turbo on, 8–100% performance range".into()),
                    ("Graphics".into(), "Intel driving the built-in display, NVIDIA asleep".into()),
                    ("Cooling".into(), "Fans at level 3 of 7".into()),
                ],
                gpus: vec![("0000:00:02.0".into(), "Intel".into(), false), ("0000:01:00.0".into(), "NVIDIA".into(), true)],
                drives: vec![("S7HDNJ0X123456".into(), "Samsung SSD 990 PRO 2TB".into())],
                fans: true,
                ..Self::default()
            };
        }
        let mut f = Self {
            battery: battery::battery(),
            limit: battery::charge_limit(),
            charge_writable: olo_xdg::session::real(),
            presence: olo_system::presence::sensor().map(|s| olo_system::presence::readable(&s)),
            ..Default::default()
        };
        if let Some(followed) = gpu_tuning::Followed::load(Path::new("/")) {
            if followed.version < gpu_tuning::FOLLOWER_VERSION {
                f.issue = Some("Update the power service in Olo Doctor to use these policies.".into());
            } else if !followed.failed.is_empty() {
                f.issue = Some("The power service reported a problem applying this mode. Olo Doctor has the details.".into());
            }
        } else {
            f.issue = Some("Olo Doctor can set up the hardware power service.".into());
        }
        // What a mode can be set to do here, whether or not the hardware's
        // state is asked for (read only with that pane open, the mode's
        // box showed the processors alone, as the mode comes: Ben,
        // 2026-11-21, "why no intel or nvidia").
        f.read_what_modes_set();
        if !details {
            return f;
        }
        if let Some(c) = olo_system::cpu_tuning::caps() {
            let mut parts = vec![olo_kit::words::tr(if c.turbo { "Turbo on" } else { "Turbo off" }).to_owned()];
            match (c.floor, c.top) {
                (Some(lo), Some(hi)) if lo == hi => parts.push(olo_kit::tr!("minimum and maximum both {}%", hi)),
                (Some(lo), Some(hi)) => parts.push(olo_kit::tr!("{}–{}% performance range", lo, hi)),
                (_, Some(hi)) => parts.push(olo_kit::tr!("ceiling {}%", hi)),
                _ => {}
            }
            if !c.preference.is_empty() {
                parts.push(c.preference.replace('_', " "));
            }
            f.hardware.push(("CPU".into(), parts.join(", ")));
        }
        for gpu in olo_system::gpus::list() {
            let ports: Vec<_> = gpu
                .connectors
                .iter()
                .filter(|port| {
                    gpu.card
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .is_some_and(|card| std::fs::read_to_string(format!("/sys/class/drm/{}-{port}/enabled", card.to_string_lossy())).is_ok_and(|s| s.trim() == "enabled"))
                })
                .cloned()
                .collect();
            let status = if gpu_tuning::asleep(&gpu) {
                "asleep".to_owned()
            } else if !ports.is_empty() {
                olo_kit::tr!(
                    "driving {}",
                    ports.iter().map(|p| if p.starts_with("eDP") || p.starts_with("LVDS") { olo_kit::words::tr("the built-in display").to_owned() } else { p.clone() }).collect::<Vec<_>>().join(", ")
                )
            } else {
                "awake".into()
            };
            f.hardware.push((gpu.maker().into(), status));
        }
        let cooling = fans::look(&fans::Places::here())
            .and_then(|f| {
                f.level().map(|(l, w)| {
                    if l == "auto" {
                        olo_kit::words::tr("Firmware curve").to_owned()
                    } else if w.is_empty() {
                        olo_kit::tr!("Fan level {}", l)
                    } else {
                        olo_kit::tr!("Fan level {} · {}", l, w)
                    }
                })
            })
            .unwrap_or_else(|| olo_kit::words::tr("Firmware curve").to_owned());
        let pressure = std::fs::read_to_string("/run/olo-fans/pressure").unwrap_or_default();
        let cooling = match pressure.trim() {
            "background" => olo_kit::tr!("{} · easing background work", cooling),
            "hot" => olo_kit::tr!("{} · limiting sustained power", cooling),
            _ => cooling,
        };
        f.hardware.push(("Cooling".into(), cooling));
        f
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    tab: usize,
    hardware: bool,
    idle_options: bool,
}

pub struct State {
    pub tab: usize,
    pub details: bool,
    more_idle: bool,
    facts: Arc<Mutex<Option<Facts>>>,
    refresh: SyncSender<()>,
    expanded: Arc<std::sync::atomic::AtomicBool>,
    watch: Option<olo_kit::watch::Watch>,
    observed: Option<(Option<Mode>, Option<u32>, bool)>,
    /// What the modes do as changed here and not yet applied (None: as
    /// the computer keeps it).
    edited: Option<gpu_tuning::Plan>,
    applying: Arc<Mutex<Applying>>,
}

impl State {
    pub fn saved(&self) -> Saved {
        Saved { tab: self.tab, hardware: self.details, idle_options: self.more_idle }
    }
    pub fn restore(&mut self, saved: Saved) {
        self.tab = saved.tab.min(1);
        self.more_idle = saved.idle_options;
        self.set_details(saved.hardware);
    }
    pub fn new(_cx: &mut Cx<PanelView>) -> Self {
        let facts = Arc::new(Mutex::new(demo().then(|| Facts::read(false))));
        let (refresh, jobs) = sync_channel(1);
        let expanded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (out, details, proxy) = (facts.clone(), expanded.clone(), olo_kit::proxy());
        std::thread::spawn(move || {
            while jobs.recv().is_ok() {
                *out.lock().unwrap() = Some(Facts::read(details.load(std::sync::atomic::Ordering::Relaxed)));
                proxy.send(Msg::Redraw);
            }
        });
        let _ = refresh.try_send(());
        Self { tab: 0, details: false, more_idle: false, facts, refresh, expanded, watch: None, observed: None, edited: None, applying: Arc::default() }
    }
    pub fn set_details(&mut self, open: bool) {
        self.details = open;
        self.expanded.store(open, std::sync::atomic::Ordering::Relaxed);
        self.watch = if open && !demo() {
            let tx = self.refresh.clone();
            olo_kit::watch::watch(["/run/olo-gpu-follow.json", "/run/olo-fans/status", "/run/olo-fans/pressure"].map(PathBuf::from), move |_| {
                let _ = tx.try_send(());
            })
        } else {
            None
        };
        if open {
            let _ = self.refresh.try_send(());
        }
    }
}

fn minutes_text(m: u32) -> String {
    match m {
        0 => "Never".into(),
        1 => "1 minute".into(),
        60 => "1 hour".into(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

pub fn duration_text(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => olo_kit::tr!("{} h {} min", h, m),
    }
}

/// "87% — 2 h 10 min remaining" and friends.
pub fn summary(b: &Battery) -> String {
    let detail = match b.state {
        Charge::Full => olo_kit::words::tr("fully charged").to_owned(),
        Charge::Charging => match b.time_to_full {
            Some(t) => olo_kit::tr!("charging, full in {}", duration_text(t)),
            None => "charging".to_owned(),
        },
        Charge::Discharging => match b.time_to_empty {
            Some(t) => format!("{} remaining", duration_text(t)),
            None => olo_kit::words::tr("on battery").to_owned(),
        },
        Charge::NotCharging => match b.charge_limit {
            Some(l) if l < 100 && b.percent + 2 >= l => olo_kit::tr!("plugged in, charge limit {}%", l),
            _ => olo_kit::words::tr("plugged in, not charging").to_owned(),
        },
        Charge::Unknown => String::new(),
    };
    if detail.is_empty() { format!("{}%", b.percent) } else { format!("{}% — {detail}", b.percent) }
}

/// A pop-up menu of `choices` (minutes; 0 is never) setting `value`.
pub fn minutes_menu(ui: &mut Ui, id: &str, label: &str, divider: f32, choices: &'static [u32], value: u32, set: impl Fn(&mut Settings, u32) + 'static) {
    let mut names: Vec<SharedString> = choices.iter().map(|&m| minutes_text(m).into()).collect();
    let i = match choices.iter().position(|&m| m == value) {
        Some(i) => i,
        None => {
            names.push(minutes_text(value).into());
            names.len() - 1
        }
    };
    let label = SharedString::from(label.to_owned());
    ui.menu(
        id,
        names,
        i,
        |f| f.label(label).divider(divider),
        move |s, i| {
            if let Some(&m) = choices.get(i) {
                set(s, m);
            }
        },
    );
}

pub fn ui(ui: &mut Ui, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let facts = st.facts.lock().unwrap().clone();
    // (Applied, and read again: what was edited is what the computer
    // keeps now.)
    if *st.applying.lock().unwrap() == Applying::Done && facts.as_ref().is_some_and(|f| Some(&f.plan) == st.edited.as_ref()) {
        st.edited = None;
        *st.applying.lock().unwrap() = Applying::No;
    }
    if let Some(p) = power {
        let key = (p.policy.modes.as_ref().map(|m| m.active), p.policy.percent, p.policy.on_battery);
        if st.observed.replace(key) != Some(key) {
            let _ = st.refresh.try_send(());
        }
    }
    let items = ui.collect(|ui| match st.tab {
        0 => modes_ui(ui, power, facts.as_ref(), server, st),
        _ => idle_ui(ui, facts.as_ref(), st),
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let crate::panel::State::Power(st) = &mut v.state {
            st.tab = *i;
        }
        cx.notify();
    });
    ui.add(
        div().flex_1().flex().flex_col().child(
            TabView::new("power-tabs", TABS.into_iter().map(SharedString::from).collect::<Vec<_>>(), st.tab)
                .grows()
                .on_select(pick)
                .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items)),
        ),
    );
}

/// A change to what the modes do, from a control: what is shown,
/// changed by `set`, and kept as edited unless that is what the computer
/// keeps already.
fn plan_change(ui: &Ui, shown: &gpu_tuning::Plan, kept: &gpu_tuning::Plan, set: Box<dyn Fn(&mut gpu_tuning::Plan, usize)>) -> Box<dyn Fn(&usize, &mut WindowCx)> {
    let (shown, kept) = (shown.clone(), kept.clone());
    Box::new(ui.on_view(move |v, i: &usize, cx| {
        if let crate::panel::State::Power(st) = &mut v.state {
            let mut p = st.edited.clone().unwrap_or_else(|| shown.clone());
            set(&mut p, *i);
            st.edited = (p != kept).then_some(p);
            *st.applying.lock().unwrap() = Applying::No;
        }
        cx.notify();
    }))
}

/// What the mode in use does, each thing a control: beside the modes,
/// following the one chosen (Ben, 2026-11-20: "2 columns and the second
/// column has the customization options specific for each mode, so
/// changing modes changes those options … let me customize all the
/// things that the mode does"). A change is that mode's alone and is
/// kept; the box then says it is customized and offers the way back.
/// What the session itself does (the display, the app in front) is
/// changed at once; what root does for the mode (the processors, the
/// GPUs, the fans, the drives) is changed here and set with Apply,
/// which asks for an administrator's password once for all of it.
fn mode_box(ui: &mut Ui, mode: Mode, on_battery: bool, facts: &Facts, st: &State) {
    use gpu_tuning::{Plan, Preset, When};
    use olo_kit::IntoElement as _;
    use olo_system::{cpu_tuning::CpuPreset, nvme_power::NvmePreset};
    let kept = facts.plan.clone();
    let plan = st.edited.clone().unwrap_or_else(|| kept.clone());
    let applying = st.applying.lock().unwrap().clone();
    let enabled = applying != Applying::Now;
    // A row: its label at the box's left, its menus ending at the box's
    // right, each row's as wide together as every other's (so they start
    // on one line too).
    let row = |ui: &mut Ui, label: SharedString, fields: Vec<olo_kit::Element>| {
        ui.add(div().flex().flex_row().items_center().child(StringView::new(label)).child(div().flex_1().min_w(px(m::SPACING))).children(fields));
    };
    let field = |ui: &Ui, id: String, names: Vec<SharedString>, at: usize, width: f32, set: Box<dyn Fn(&mut Plan, usize)>| {
        MenuField::new(id, names, at).width(width).enabled(enabled).on_change(plan_change(ui, &plan, &kept, set)).into_element()
    };
    // A preset and, beside it, how fast at the most: "Balanced up to 100%".
    let up_to = || div().w(px(UP_TO)).flex().justify_center().child(StringView::new("up to")).into_element();
    // (Where a row has no top speed: the room of one, so its menu starts
    // where the presets do.)
    let beside = || div().w(px(UP_TO + TOP)).into_element();
    let tops = || TOPS.iter().map(|t| SharedString::from(format!("{t}%"))).collect::<Vec<_>>();
    let top_at = |pct: u8| TOPS.iter().position(|t| *t <= pct).unwrap_or(0);
    let items = ui.collect(|ui| {
        let cpu = plan.mode_cpu(mode);
        let fields = vec![
            field(
                ui,
                "power-mode-cpu".into(),
                CpuPreset::ALL.iter().map(|p| olo_kit::words::tr(p.name()).into()).collect(),
                CpuPreset::ALL.iter().position(|p| *p == cpu).unwrap_or(0),
                PRESET,
                Box::new(move |p, i| p.set_mode(mode, "cpu", (CpuPreset::ALL[i] != gpu_tuning::cpu_for_mode(mode)).then(|| CpuPreset::ALL[i].id().into()))),
            ),
            up_to(),
            field(ui, "power-mode-cpu-top".into(), tops(), top_at(plan.mode_cpu_top(mode)), TOP, Box::new(move |p, i| p.set_mode(mode, "cpu-top", (TOPS[i] != 100).then(|| TOPS[i].to_string())))),
        ];
        row(ui, olo_kit::words::tr("Processors:").into(), fields);
        const PRESETS: [Preset; 3] = [Preset::Cooler, Preset::Balanced, Preset::Fastest];
        for (pci, maker, nvidia) in &facts.gpus {
            let (now, nvidia) = (plan.mode_gpu(mode, pci, *nvidia, on_battery), *nvidia);
            let (key, top_key) = (pci.clone(), format!("{pci}:top"));
            let fields = vec![
                field(
                    ui,
                    format!("power-mode-gpu-{pci}"),
                    PRESETS.iter().map(|p| olo_kit::words::tr(p.name()).into()).collect(),
                    PRESETS.iter().position(|p| *p == now).unwrap_or(1),
                    PRESET,
                    Box::new(move |p, i| p.set_mode(mode, &key, (PRESETS[i] != Plan::gpu_as_it_comes(mode, nvidia, on_battery)).then(|| PRESETS[i].id().into()))),
                ),
                up_to(),
                field(
                    ui,
                    format!("power-mode-gpu-top-{pci}"),
                    tops(),
                    top_at(plan.mode_gpu_top(mode, pci)),
                    TOP,
                    Box::new(move |p, i| p.set_mode(mode, &top_key, (TOPS[i] != 100).then(|| TOPS[i].to_string()))),
                ),
            ];
            row(ui, olo_kit::tr!("{} graphics:", maker).into(), fields);
        }
        if !facts.gpus.is_empty() {
            let ready = plan.mode_gpu_ready(mode);
            let on = plan_change(ui, &plan, &kept, Box::new(move |p, _| p.set_mode(mode, "gpu-ready", (!ready != (mode == Mode::Performance)).then(|| if ready { "off" } else { "on" }.into()))));
            ui.add_mark(CheckBox::new(olo_kit::words::of_text("Keep graphics ready between tasks"), ready).id("power-mode-gpu-ready").enabled(enabled).on_toggle(move |_, cx| on(&0, cx)));
        }
        // Cooling and storage, a little apart from the chips.
        if facts.fans || !facts.drives.is_empty() {
            ui.space(2.0);
        }
        if facts.fans {
            use fans::Profile;
            let now = plan.mode_fans(mode);
            let fields = vec![
                field(
                    ui,
                    "power-mode-fans".into(),
                    Profile::ALL.iter().map(|p| olo_kit::words::tr(p.label()).into()).collect(),
                    Profile::ALL.iter().position(|p| *p == now).unwrap_or(0),
                    PRESET,
                    Box::new(move |p, i| p.set_mode(mode, "fans", (Profile::ALL[i] != gpu_tuning::fans_for_mode(mode)).then(|| Profile::ALL[i].word().into()))),
                ),
                beside(),
            ];
            row(ui, olo_kit::words::tr("Fans:").into(), fields);
        }
        for (n, (serial, _model)) in facts.drives.iter().enumerate() {
            let now = plan.nvme(When::Mode(mode), serial).unwrap_or_default();
            let key = serial.clone();
            let fields = vec![
                field(
                    ui,
                    format!("power-mode-drive-{serial}"),
                    NvmePreset::ALL.iter().map(|p| olo_kit::words::tr(p.name()).into()).collect(),
                    NvmePreset::ALL.iter().position(|p| *p == now).unwrap_or(0),
                    PRESET,
                    Box::new(move |p, i| p.set_nvme(When::Mode(mode), &key, Some(NvmePreset::ALL[i]))),
                ),
                beside(),
            ];
            // (Its model is long, and set the whole column's width: the
            // drives by number where there is more than one.)
            row(ui, if facts.drives.len() == 1 { olo_kit::words::tr("Drive:").into() } else { olo_kit::tr!("Drive {}:", n + 1).into() }, fields);
        }
        // What the session itself does in the mode: changed at once, so
        // set apart from what Apply sets, and said to be.
        ui.space(2.0);
        let power = ui.s.power.clone();
        let at_once = ui.collect(|ui| {
            // (A label for live controls, in the labels' colour, and as
            // close to them as they are to each other.)
            ui.add(StringView::new("These take effect at once:").height(m::LABEL_ROW));
            ui.check("Limit the built-in display to 60 Hz", power.limits_refresh(mode), move |s, v| s.power.choose(mode, |c| c.limit_refresh = Some(v)));
            ui.check("Dim the built-in display when idle", power.dims_at_rest(mode), move |s, v| s.power.choose(mode, |c| c.dim_at_rest = Some(v)));
            ui.check("Favour the front app as you type and click", power.boosts_after_input(mode), move |s, v| s.power.choose(mode, |c| c.boost_after_input = Some(v)));
        });
        ui.add(div().flex().flex_col().gap(px(m::TIGHT)).children(at_once));
        ui.add(olo_kit::widgets::separator());
        // How it stands, and the way back.
        let customized = plan.customized(mode) || ui.s.power.modes.contains_key(mode.id());
        let waiting = st.edited.is_some();
        let says: SharedString = match &applying {
            Applying::Now => olo_kit::words::tr("Applying…").into(),
            Applying::Failed(why) => why.clone().into(),
            _ if waiting => olo_kit::words::tr("Not applied yet.").into(),
            _ if customized => olo_kit::words::tr("Customized.").into(),
            _ => olo_kit::words::tr("Defaults for this mode.").into(),
        };
        let revert = {
            let (shown, kept) = (plan.clone(), kept.clone());
            ui.on_view(move |v, _: &(), cx| {
                if let crate::panel::State::Power(st) = &mut v.state {
                    let mut p = st.edited.clone().unwrap_or_else(|| shown.clone());
                    p.revert(mode);
                    st.edited = (p != kept).then_some(p);
                    *st.applying.lock().unwrap() = Applying::No;
                }
                crate::prefs().edit_as("", olo_kit::words::tr("Restore Defaults"), |s| {
                    s.power.modes.remove(mode.id());
                });
                cx.notify();
            })
        };
        let apply = {
            let (state, refresh, proxy, edited) = (st.applying.clone(), st.refresh.clone(), olo_kit::proxy(), st.edited.clone());
            move |_: &olo_kit::ClickEvent, _: &mut WindowCx| {
                let Some(plan) = edited.clone() else { return };
                *state.lock().unwrap() = Applying::Now;
                let (state, refresh, proxy) = (state.clone(), refresh.clone(), proxy.clone());
                // (Root's to keep: asked for off the window's thread; the
                // administrator's password is polkit's to ask.)
                std::thread::spawn(move || {
                    let mut tuning = gpu_tuning::Tuning::load();
                    tuning.plan = plan;
                    *state.lock().unwrap() = match crate::backup::restore_tuning(&tuning) {
                        Ok(_) => Applying::Done,
                        Err(why) => Applying::Failed(why),
                    };
                    let _ = refresh.try_send(());
                    proxy.send(Msg::Redraw);
                });
            }
        };
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(StringView::new(says).dim(true).wrap(true)))
                .child(Button::new("Restore Defaults").enabled(enabled && (customized || waiting)).on_click(move |_, cx| revert(&(), cx)))
                .child(Button::new("Apply…").enabled(enabled && waiting).on_click(apply)),
        );
    });
    // (Named as the mode is, which is what ties it to the choice beside it.)
    ui.add(BBox::new(olo_kit::words::of_text(mode.label())).children(items));
}

fn modes_ui(ui: &mut Ui, power: Option<&PowerStatus>, facts: Option<&Facts>, server: &Server, st: &State) {
    let example = Modes { active: Mode::Balanced, available: Mode::ALL.to_vec(), degraded: None, drivers: vec![] };
    let modes = power.and_then(|p| p.policy.modes.as_ref()).or_else(|| demo().then_some(&example));
    // The modes on the left; on the right what the one in use does.
    let on_battery = power.is_some_and(|p| p.policy.on_battery);
    let right = match (modes, facts) {
        (Some(modes), Some(facts)) => ui.collect(|ui| mode_box(ui, modes.active, on_battery, facts, st)),
        _ => Vec::new(),
    };
    let left = ui.collect(|ui| modes_left(ui, power, facts, modes, server, st));
    if right.is_empty() {
        for item in left {
            ui.add(item);
        }
    } else {
        ui.add_columns(left, right);
    }
}

fn modes_left(ui: &mut Ui, power: Option<&PowerStatus>, facts: Option<&Facts>, modes: Option<&Modes>, server: &Server, st: &State) {
    let choices = ui.collect(|ui| {
        for mode in Mode::ALL {
            let enabled = modes.is_some_and(|m| m.available.contains(&mode));
            let server = server.clone();
            let description = if !enabled && modes.is_some() { "Unavailable with this computer's power driver." } else { mode.about() };
            ui.add(
                // (A mode the person changed says so, whichever is in use.)
                RadioButton::new(
                    if facts.is_some_and(|f| st.edited.as_ref().unwrap_or(&f.plan).customized(mode)) || ui.s.power.modes.contains_key(mode.id()) {
                        SharedString::from(olo_kit::tr!("{} — Customized", olo_kit::words::tr(mode.label())))
                    } else {
                        olo_kit::words::of_text(mode.label())
                    },
                    modes.is_some_and(|m| m.active == mode),
                )
                .id(format!("power-{}", mode.id()))
                .enabled(enabled)
                .on_select(move |_, _| server.set_mode(mode))
                .description(description),
            );
        }
        if modes.is_none() {
            ui.label(if power.is_some() { "The system power profiles service is unavailable." } else { "Reading power modes…" });
        }
        if let Some(note) = modes.and_then(Modes::degraded_text) {
            ui.label(note);
        }
        if let Some(error) = power.and_then(|p| p.policy.error.as_deref()) {
            ui.add(StringView::new(error.to_owned()).wrap(true));
        }
    });
    ui.add(div().flex().flex_col().gap(px(m::SPACING)).mb(px(m::TIGHT)).children(choices));
    if facts.is_some_and(|f| f.battery.is_some()) || power.is_some_and(|p| p.policy.percent.is_some()) {
        ui.choose(
            "automatic-saver",
            &[(0, "Off"), (10, "At 10%"), (15, "At 15%"), (20, "At 20%")],
            &ui.s.power.saver_percent,
            |f| f.label("Switch to Battery Saver:").divider(divider_for(&["Switch to Battery Saver:"])),
            |s, v| s.power.saver_percent = v,
        );
        if let Some(p) = power
            && let Some(mode) = p.policy.resume_on_ac
        {
            ui.label(olo_kit::tr!("Battery Saver at {}% · {} resumes on AC", p.policy.percent.unwrap_or_default(), olo_kit::words::tr(mode.label())));
        }
    }
    if let Some(issue) = facts.and_then(|f| f.issue.as_ref()) {
        ui.add(StringView::new(olo_kit::words::of_text(issue)).wrap(true));
        ui.button("Open Olo Doctor", |_, _| {
            std::thread::spawn(|| {
                let _ = olo_xdg::open::spawn(&["olo-doctor".into()]);
            });
        });
    }
    let toggle = ui.on_view(|v, open: &bool, cx| {
        if let crate::panel::State::Power(st) = &mut v.state {
            st.set_details(*open);
        }
        cx.notify();
    });
    ui.add(PaneSwitch::new("Current hardware state", st.details).id("power-hardware").on_toggle(toggle));
    if st.details {
        let rows = ui.collect(|ui| match facts {
            Some(f) if !f.hardware.is_empty() => {
                for (name, value) in &f.hardware {
                    ui.add(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(m::SPACING))
                            .child(div().w(px(76.0)).flex_shrink_0().child(StringView::new(olo_kit::words::of_text(name)).dim(true).wrap(true)))
                            .child(div().flex_1().min_w_0().child(StringView::new(value.clone()).wrap(true))),
                    );
                }

                let tx = st.refresh.clone();
                ui.button("Refresh", move |_, _| {
                    let _ = tx.try_send(());
                });
            }
            _ => ui.label("Reading hardware…"),
        });
        ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).flex().flex_col().gap(px(m::TIGHT)).children(rows));
    }
}

fn idle_ui(ui: &mut Ui, facts: Option<&Facts>, st: &State) {
    ui.bbox("Battery", |ui| match facts {
        None => ui.label("Reading battery…"),
        Some(f) => {
            let Some(b) = &f.battery else {
                ui.label("This computer has no battery.");
                return;
            };
            if b.state == Charge::NotCharging && f.limit.as_ref().is_some_and(|limit| limit.enabled && b.percent + 2 >= limit.end) {
                ui.label(format!("{}% — {}", b.percent, olo_kit::words::tr("Plugged in")));
            } else {
                ui.label(summary(b));
            }
            if let Some(l) = &f.limit {
                let (l, tx, facts) = (l.clone(), st.refresh.clone(), st.facts.clone());
                let label = olo_kit::tr!("Limit charging to {}%", l.end);
                let resume = if l.enabled { olo_kit::tr!("Charging resumes below {}%.", l.start) } else { olo_kit::words::tr("Turn on for a laptop usually plugged in.").to_owned() };
                ui.add(
                    CheckBox::new(label, l.enabled)
                        .id("charge-limit")
                        .enabled(f.charge_writable)
                        .on_toggle(move |on, _: &mut WindowCx| {
                            if demo() {
                                if let Some(l) = facts.lock().unwrap().as_mut().and_then(|f| f.limit.as_mut()) {
                                    l.enabled = *on;
                                }
                                olo_kit::proxy().send(Msg::Redraw);
                                return;
                            }
                            let (l, on, tx) = (l.clone(), *on, tx.clone());
                            std::thread::spawn(move || {
                                if let Err(e) = battery::set_charge_limit(&l, on) {
                                    tracing::warn!(?e, "charge limit");
                                }
                                let _ = tx.try_send(());
                            });
                        })
                        .description(resume),
                );
            }
        }
    });
    let battery = facts.is_some_and(|f| f.battery.is_some());
    ui.bbox("When Idle", |ui| {
        let values = ui.s.power.clone();
        let label_w = divider_for(&["Turn off displays", "Sleep"]);
        let mut head = div().flex().flex_row().gap(px(m::SPACING)).child(div().w(px(label_w)));
        for (dc, label) in [(true, "On battery"), (false, "Plugged in")] {
            if !dc || battery {
                head = head.child(div().flex_1().child(StringView::new(label).dim(true)));
            }
        }
        let mut rows = div().flex().flex_col().gap(px(m::STACK));
        for (sleep, label) in [(false, "Turn off displays"), (true, "Sleep")] {
            let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().w(px(label_w)).child(StringView::new(label)));
            for (dc, source) in [(true, "on battery"), (false, "plugged in")] {
                if dc && !battery {
                    continue;
                }
                let (off, suspend) = values.idle_minutes(dc);
                let current = if sleep { suspend } else { off };
                let mut choices = vec![5, 15, 30, 60, 0];
                if !choices.contains(&current) {
                    choices.insert(0, current);
                }
                let index = choices.iter().position(|&t| t == current).unwrap_or(0);
                let name = format!("{label} {source}");
                let items = ui.collect(|ui| {
                    ui.menu(
                        &format!("idle-{sleep}-{dc}"),
                        choices.iter().map(|m| minutes_text(*m).into()).collect(),
                        index,
                        |f| f.name(name),
                        move |s, i| {
                            if let Some(&m) = choices.get(i) {
                                s.power.set_idle_minutes(dc, sleep, m);
                            }
                        },
                    )
                });
                row = row.child(div().flex_1().min_w_0().children(items));
            }
            rows = rows.child(row);
        }
        ui.add(div().flex().flex_col().child(head).child(rows));
    });
    let toggle = ui.on_view(|v, open: &bool, cx| {
        if let crate::panel::State::Power(st) = &mut v.state {
            st.more_idle = *open;
        }
        cx.notify();
    });
    ui.add(PaneSwitch::new("Lock & presence", st.more_idle).id("idle-options").on_toggle(toggle));
    if st.more_idle {
        let controls = ui.collect(|ui| {
            ui.check("Lock when the displays turn off", ui.s.power.lock_on_display_off, |s, v| s.power.lock_on_display_off = v);
            presence_box(ui, facts.and_then(|f| f.presence));
        });
        ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).flex().flex_col().gap(px(m::SPACING)).children(controls));
    }
}

/// Seconds after the person walks away before the screen locks.
const AWAY: &[u32] = &[0, 10, 30, 60, 120, 300];

fn away_text(seconds: u32) -> String {
    // (In the language chosen, its forms for the number.)
    match seconds {
        0 => olo_kit::words::tr("Never").to_owned(),
        s if s < 60 => olo_kit::words::count(u64::from(s), "After 1 second", "After {n} seconds"),
        60 => olo_kit::words::tr("After a minute").to_owned(),
        s => olo_kit::words::count(u64::from(s / 60), "After 1 minute", "After {n} minutes"),
    }
}

/// A presence sensor (a ThinkPad's): lock when the person walks away,
/// wake when they come back. Only on a computer that has one.
fn presence_box(ui: &mut Ui, sensor: Option<bool>) {
    let Some(readable) = sensor else { return };
    let p = ui.s.power.clone();
    ui.bbox("Presence", |ui| {
        const LOCK: &str = "Lock when you walk away:";
        let divider = divider_for(&[LOCK]);
        let mut choices: Vec<u32> = AWAY.to_vec();
        if !choices.contains(&p.lock_when_away_seconds) {
            choices.push(p.lock_when_away_seconds);
        }
        let names: Vec<SharedString> = choices.iter().map(|&s| away_text(s).into()).collect();
        let selected = choices.iter().position(|&s| s == p.lock_when_away_seconds).unwrap_or(0);
        ui.menu("lock-away", names, selected, |f| f.label(SharedString::from(LOCK)).divider(divider), move |s, i| s.power.lock_when_away_seconds = choices.get(i).copied().unwrap_or(0));
        ui.check("Wake the display when you come back", p.wake_on_return, |s, v| s.power.wake_on_return = v);
        ui.check("Dim the display while you look away", p.dim_when_looking_away, |s, v| s.power.dim_when_looking_away = v);
        ui.note(if readable { "This computer's sensor sees when someone is in front of it." } else { "Olo can't read the sensor yet: Olo Doctor installs what it needs." });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On this computer: what a mode can be set to do is read without the
    /// hardware pane open (`cargo test -p olo-preferences
    /// reads_what_modes_set -- --ignored --nocapture`).
    #[test]
    #[ignore = "reads this computer's own hardware"]
    fn reads_what_modes_set_here() {
        let mut f = Facts::default();
        f.read_what_modes_set();
        println!("graphics: {:?}; drives: {}; fans: {}; chosen for modes: {:?}", f.gpus.iter().map(|g| &g.1).collect::<Vec<_>>(), f.drives.len(), f.fans, f.plan.chosen.keys().collect::<Vec<_>>());
        assert!(!f.gpus.is_empty());
    }

    fn b(percent: u32, state: Charge) -> Battery {
        Battery { percent, state, time_to_empty: None, time_to_full: None, cycle_count: None, health: None, charge_limit: None, power_watts: None }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(&b(100, Charge::Full)), "100% — fully charged");
        let mut d = b(57, Charge::Discharging);
        d.time_to_empty = Some(Duration::from_secs(2 * 3600 + 600));
        assert_eq!(summary(&d), "57% — 2 h 10 min remaining");
        let mut l = b(80, Charge::NotCharging);
        l.charge_limit = Some(80);
        assert_eq!(summary(&l), "80% — plugged in, charge limit 80%");
        assert_eq!(minutes_text(120), "2 hours");
        assert_eq!(minutes_text(0), "Never");
    }
}
