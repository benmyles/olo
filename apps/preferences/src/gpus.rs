//! Preferences ▸ GPUs: each GPU, what it drives, and its tuning: a
//! preset (Cooler and Quieter, Balanced, Fastest) or its knobs by hand
//! (`olo_system::gpu_tuning`). Applied as root by `olo-gpu-tune set`
//! through polkit, kept for the computer and set again at every start.
//! A sleeping NVIDIA GPU isn't asked what it allows (asking wakes it)
//! unless the person asks: what it said last time is shown.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Arc,
};

use olo_sync::Mutex;
use olo_system::{
    gpu_tuning::{self, Caps, GpuTune, Preset, Tuning},
    gpus::{self, Gpu},
};
use olo_ui::{
    gpui::{Context, ParentElement as _, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, CheckBox, EmptyState, RadioButton, Slider, StringView},
};

use crate::{Msg, PanelView, form::Ui};

/// The panel's state in its window.
pub struct State {
    shared: Arc<Mutex<Shared>>,
    asked: bool,
    /// The tuning kept, followed: the power mode's follower changes it.
    watch: Option<olo_ui::watch::Watch>,
}

impl State {
    pub fn new() -> Self {
        State { shared: Default::default(), asked: false, watch: None }
    }
}

/// The root service that puts the GPUs on the power mode's preset
/// (`olo-gpu-tune follow`, installed by `olo-gpu-tune set`), and the
/// program it runs.
const FOLLOWER: &str = "/etc/systemd/system/olo-gpu-follow.service";
const INSTALLED: &str = "/usr/local/libexec/olo-gpu-tune";

/// Whether the power mode's follower is installed, and from this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Follower {
    #[default]
    Missing,
    /// From an older build (Apply brings it up to date).
    Old,
    Current,
}

/// The follower as installed, against the `olo-gpu-tune` beside this
/// program (on the reading thread: the two compared byte for byte only
/// when they're one size).
fn follower() -> Follower {
    if !std::path::Path::new(FOLLOWER).exists() {
        return Follower::Missing;
    }
    let Ok(exe) = std::env::current_exe() else { return Follower::Current };
    let ours = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)")).with_file_name("olo-gpu-tune");
    let len = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).ok();
    match (len(std::path::Path::new(INSTALLED)), len(&ours)) {
        // (Ours not here to compare: as it is.)
        (_, None) => Follower::Current,
        (Some(a), Some(b)) if a == b && std::fs::read(INSTALLED).ok() == std::fs::read(&ours).ok() => Follower::Current,
        _ => Follower::Old,
    }
}

#[derive(Default)]
struct Shared {
    /// Each GPU and what it allows: None while being read.
    gpus: Option<Vec<Entry>>,
    /// The tuning kept (/etc/olo/gpus.json), and as it's being edited.
    kept: Tuning,
    edited: Tuning,
    /// Being applied (polkit may be asking for a password).
    applying: bool,
    /// What the last Apply said.
    said: Option<Result<String, String>>,
    /// The power mode's follower, as installed.
    follower: Follower,
    /// The power mode now (None: no daemon).
    mode: Option<olo_system::power_profile::Mode>,
}

#[derive(Clone)]
struct Entry {
    gpu: Gpu,
    /// What it allows (None: asleep, never asked).
    caps: Option<Caps>,
    asleep: bool,
}

/// Read every GPU, what each allows and the tuning kept, on a thread
/// (`wake`: that GPU asked even if it's asleep). The window isn't shown
/// until it's in, briefly (`hold`).
fn read(shared: Arc<Mutex<Shared>>, proxy: olo_ui::Proxy, wake: Option<String>, hold: Option<olo_ui::FirstShowHold>) {
    // Tests (the layout lint): a hybrid laptop's two, as the ThinkPad P1
    // Gen 9's are.
    if cfg!(test) {
        let gpu = |pci: &str, name: &str, driver: &str, boot: bool, connectors: &[&str]| Gpu {
            pci: pci.into(),
            vendor: 0,
            device: 0,
            driver: driver.into(),
            name: name.into(),
            boot,
            card: None,
            render: None,
            connectors: connectors.iter().map(|c| c.to_string()).collect(),
            sysfs: Default::default(),
        };
        let xe = Caps::Xe(gpu_tuning::XeCaps { lowest: 100, efficient: 700, highest: 2450, min_now: 700, max_now: 2450, profiles: true, power_saving_now: false });
        let nv = Caps::Nvidia(gpu_tuning::NvidiaCaps { power: None, min_gpu_mhz: 180, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let mut s = shared.lock().unwrap();
        s.follower = Follower::Current;
        s.mode = Some(olo_system::power_profile::Mode::Balanced);
        s.gpus = Some(vec![
            Entry { gpu: gpu("0000:00:02.0", "Intel Graphics", "xe", true, &["eDP-1", "DP-1", "HDMI-A-1"]), caps: Some(xe), asleep: false },
            Entry { gpu: gpu("0000:01:00.0", "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU", "nvidia", false, &["HDMI-A-2"]), caps: Some(nv), asleep: true },
        ]);
        return;
    }
    std::thread::Builder::new()
        .name("gpus".into())
        .spawn(move || {
            let gpus: Vec<Entry> = gpus::list()
                .into_iter()
                .map(|g| {
                    let wake = wake.as_deref() == Some(g.pci.as_str());
                    Entry { caps: gpu_tuning::caps(&g, wake), asleep: gpu_tuning::asleep(&g), gpu: g }
                })
                .collect();
            let kept = Tuning::load();
            let follower = follower();
            let mode = olo_system::power_profile::modes().ok().flatten().map(|m| m.active);
            {
                let mut s = shared.lock().unwrap();
                s.follower = follower;
                s.mode = mode;
                // (What's being edited stays; a first read starts it.)
                if s.gpus.is_none() || s.edited == s.kept {
                    s.edited = kept.clone();
                }
                s.kept = kept;
                s.gpus = Some(gpus);
            }
            proxy.send(Msg::Redraw);
            drop(hold);
        })
        .expect("thread");
}

/// Apply what's edited: `olo-gpu-tune set`, as root through polkit.
fn apply(shared: Arc<Mutex<Shared>>, proxy: olo_ui::Proxy) {
    let tuning = {
        let mut s = shared.lock().unwrap();
        s.applying = true;
        s.said = None;
        s.edited.clone()
    };
    std::thread::Builder::new()
        .name("gpus-apply".into())
        .spawn(move || {
            let said = run_tune(&tuning);
            shared.lock().unwrap().applying = false;
            shared.lock().unwrap().said = Some(said);
            proxy.send(Msg::Redraw);
            read(shared, proxy, None, None);
        })
        .expect("thread");
}

fn run_tune(tuning: &Tuning) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let tune = exe.with_file_name("olo-gpu-tune");
    let json = serde_json::to_string(tuning).map_err(|e| e.to_string())?;
    // (Polkit asks for the password with Olo's agent: there's no way
    // across to root in process. In tests, as this user: nothing changes.)
    let mut cmd = if std::env::var_os("OLO_REAL_SESSION").is_some() && std::env::var_os("OLO_SCREEN").is_none() {
        let mut c = Command::new("pkexec");
        c.arg("--disable-internal-agent").arg(&tune);
        c
    } else {
        return Err("GPUs are tuned only in the person's own session (polkit asks there).".into());
    };
    let mut child = cmd.arg("set").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("{}: {e}", tune.display()))?;
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(json.as_bytes());
    }
    let errors: Vec<String> = child.stdout.take().map(|o| BufReader::new(o).lines().map_while(Result::ok).filter_map(|l| l.strip_prefix("error: ").map(str::to_owned)).collect()).unwrap_or_default();
    let status = child.wait().map_err(|e| e.to_string())?;
    match status.code() {
        Some(0) => Ok("Applied: kept for this computer, and set again at every start.".into()),
        // pkexec: the password dialog dismissed, or not allowed.
        Some(126) | Some(127) => Err("Not applied: an administrator's password is needed.".into()),
        _ if !errors.is_empty() => Err(format!("Not all of it applied: {}.", errors.join("; "))),
        _ => Err("Not applied.".into()),
    }
}

/// What a GPU drives, said plainly.
/// What screen recordings encode on: the most powerful there is (an
/// NVIDIA GPU's encoder where there's one), that GPU, or the processor
/// (`Settings::screen_recording`, read by olo-screenshot's recorder).
fn recordings(ui: &mut Ui, gpus: &[Entry]) {
    let nvidia = gpus.iter().find(|e| e.gpu.vendor == 0x10de).map(|e| e.gpu.name.clone());
    let current = ui.s.screen_recording.encoder.clone();
    let most = match &nvidia {
        Some(n) => format!("The most powerful ({n})"),
        None => "The most powerful (the processor)".to_owned(),
    };
    let on_gpu = nvidia.as_ref().map(|n| format!("{n} (NVENC)"));
    let mut choices: Vec<(String, &str)> = vec![("auto".into(), most.as_str())];
    if let Some(n) = &on_gpu {
        choices.push(("nvidia".into(), n.as_str()));
    }
    choices.push(("cpu".into(), "The processor (OpenH264)"));
    ui.bbox("Screen Recordings", |ui| {
        ui.choose("recording-encoder", &choices, &current, |f| f.label("Encode on:").divider(100.0), |s, v| s.screen_recording.encoder = v);
        ui.note(if nvidia.is_some() {
            "A GPU's encoder takes a 4K screen in about 13 ms a frame, the processor about 110: recordings of a lot of motion keep every frame on the GPU, about nine a second on the processor."
        } else {
            "Recordings are encoded on the processor (OpenH264): a 4K screen takes about 110 ms a frame, so a recording of a lot of motion keeps about nine a second."
        });
    });
}

fn drives(g: &Gpu) -> String {
    if g.connectors.is_empty() {
        return "no display ports".into();
    }
    format!("its ports: {}", g.connectors.join(", "))
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    if !st.asked {
        st.asked = true;
        let hold = olo_ui::hold_first_show(ui.window);
        read(st.shared.clone(), olo_ui::proxy(ui.cx), None, hold);
        // Read again as the follower changes it (a power mode chosen).
        let (shared, proxy) = (st.shared.clone(), olo_ui::proxy(ui.cx));
        if !cfg!(test) {
            st.watch = olo_ui::watch::watch([std::path::PathBuf::from(gpu_tuning::FILE), std::path::PathBuf::from(FOLLOWER)], move |_| read(shared.clone(), proxy.clone(), None, None));
        }
    }
    let (gpus, kept, edited, applying, said, follower, mode) = {
        let s = st.shared.lock().unwrap();
        (s.gpus.clone(), s.kept.clone(), s.edited.clone(), s.applying, s.said.clone(), s.follower, s.mode)
    };
    let Some(gpus) = gpus else {
        ui.note("Reading…");
        return;
    };
    if gpus.is_empty() {
        ui.add(EmptyState::new("No GPUs found.").icon("gpu"));
        return;
    }
    // Side by side, two to a row.
    let mut boxes: Vec<Vec<olo_ui::gpui::AnyElement>> = gpus.iter().map(|e| ui.collect(|ui| gpu_box(ui, e, &edited.of(&e.gpu.pci), &st.shared))).collect();
    let column = |items: Vec<olo_ui::gpui::AnyElement>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
    while !boxes.is_empty() {
        let left = boxes.remove(0);
        let right = if boxes.is_empty() { Vec::new() } else { boxes.remove(0) };
        ui.add(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(column(left)).child(column(right)));
    }
    // The power mode sets them all, as it does the processors.
    ui.bbox("Power Mode", |ui| {
        ui.note("The power mode sets every GPU, as it does the processors: Power Saver puts them on Cooler and Quieter, Balanced on Balanced and Performance on Fastest, wherever it's chosen (Power, Deskbar's battery, the keyboard). A GPU set here stays so until the mode next changes.");
        match (mode, follower) {
            (_, Follower::Missing) => ui.note("Apply once to let it: an administrator's password is asked, this once."),
            (_, Follower::Old) => ui.note("What does it is from an older Olo: Apply to bring it up to date (an administrator's password is asked)."),
            (Some(m), Follower::Current) => ui.note(format!("It's {} now: every GPU on {}.", m.label(), gpu_tuning::for_mode(m).name())),
            (None, Follower::Current) => ui.note("No power profiles daemon is running, so there's no power mode to follow."),
        }
    });
    // (Nothing installed to follow it yet, or an older one: Apply
    // installs it.)
    let changed = edited != kept || follower != Follower::Current;
    let (s1, s2, p1) = (st.shared.clone(), st.shared.clone(), olo_ui::proxy(ui.cx));
    let failure = style::c(theme::palette(ui.cx).failure);
    let on_apply = ui.on_click_view(move |_, _, cx| {
        apply(s1.clone(), p1.clone());
        cx.notify();
    });
    let on_revert = ui.on_click_view(move |_, _, cx| {
        let mut s = s2.lock().unwrap();
        s.edited = s.kept.clone();
        s.said = None;
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Apply").default(true).enabled(changed && !applying).on_click(on_apply))
            .child(Button::new("Revert").enabled(changed && !applying).on_click(on_revert))
            .child(div().flex_1().min_w_0().child(match (applying, said) {
                (true, _) => StringView::new("Applying… (an administrator's password may be asked for)").dim(true).wrap(true),
                (false, Some(Ok(s))) => StringView::new(s).dim(true).wrap(true),
                (false, Some(Err(e))) => StringView::new(e).color(failure).wrap(true),
                (false, None) => StringView::new("Kept for this computer and set again at every start.").dim(true).wrap(true),
            })),
    );
    // (Below Apply, a section of its own: it's taken at once, not applied
    // with the tuning. The commit row ends its group: `SECTION` under it.)
    ui.space(m::SECTION - 2.0 * m::STACK);
    recordings(ui, &gpus);
}

/// Change GPU `pci`'s tuning by hand (its preset becomes Custom).
fn edit(ui: &Ui, shared: &Arc<Mutex<Shared>>, pci: &str, f: impl Fn(&mut GpuTune, f32) + 'static) -> impl Fn(f32, &mut Window, &mut olo_ui::gpui::App) + 'static {
    let (shared, pci) = (shared.clone(), pci.to_owned());
    ui.on_view(move |_, v: f32, _, cx: &mut Context<PanelView>| {
        let mut s = shared.lock().unwrap();
        let mut t = s.edited.of(&pci);
        f(&mut t, v);
        t.preset = Preset::Custom;
        s.edited.set(t);
        s.said = None;
        cx.notify();
    })
}

fn gpu_box(ui: &mut Ui, e: &Entry, tune: &GpuTune, shared: &Arc<Mutex<Shared>>) {
    let g = &e.gpu;
    let pci = g.pci.clone();
    ui.bbox(&g.name, |ui| {
        let state = if e.asleep { "asleep: nothing uses it" } else { "awake" };
        ui.note(format!("{} driver, {state}; {}.", g.driver, drives(g)));
        let Some(caps) = e.caps.clone() else {
            ui.note("What it can be set to isn't known yet: asking it wakes it for a moment.");
            let (shared, proxy, pci) = (shared.clone(), olo_ui::proxy(ui.cx), pci.clone());
            let wake = ui.on_click_view(move |_, _, cx| {
                read(shared.clone(), proxy.clone(), Some(pci.clone()), None);
                cx.notify();
            });
            ui.button("Ask It", wake);
            return;
        };
        if matches!(caps, Caps::None) {
            ui.note(format!("Nothing to tune for its driver ({}) yet.", g.driver));
            return;
        }
        // The presets.
        for preset in Preset::CHOSEN {
            let (shared, pci, caps) = (shared.clone(), pci.clone(), caps.clone());
            let pick = ui.on_view(move |_, _: (), _, cx| {
                let mut s = shared.lock().unwrap();
                s.edited.set(gpu_tuning::preset(&pci, preset, &caps));
                s.said = None;
                cx.notify();
            });
            ui.add(RadioButton::new(preset.name(), tune.preset == preset).id(format!("gpu-{}-{preset:?}", g.pci)).on_select(move |window, cx| pick((), window, cx)));
            let about = match preset {
                Preset::Cooler => "Slower, cooler and quieter; the battery lasts longer.",
                Preset::Balanced => "As its driver has it.",
                Preset::Fastest => "As fast as it goes: hotter and louder.",
                Preset::Custom => "",
            };
            ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new(about).dim(true).wrap(true)));
        }
        if tune.preset == Preset::Custom {
            ui.note("Set by hand, below.");
        }
        knobs(ui, &caps, tune, shared, &pci);
    });
}

fn knobs(ui: &mut Ui, caps: &Caps, tune: &GpuTune, shared: &Arc<Mutex<Shared>>, pci: &str) {
    let mhz = |v: f32| format!("{v:.0} MHz");
    match caps {
        Caps::Xe(x) => {
            let (lo, hi) = (x.lowest as f32, x.highest as f32);
            let min = tune.min_mhz.unwrap_or(x.min_now) as f32;
            let max = tune.max_mhz.unwrap_or(x.max_now) as f32;
            let on_min = edit(ui, shared, pci, |t, v| {
                let v = v.round() as u32;
                t.min_mhz = Some(v);
                t.max_mhz = Some(t.max_mhz.unwrap_or(v).max(v));
            });
            ui.add(Slider::new(format!("gpu-{pci}-min"), min, lo..=hi).step(50.0).label("Lowest clock").value_text(mhz(min)).on_change(on_min));
            let on_max = edit(ui, shared, pci, |t, v| {
                let v = v.round() as u32;
                t.max_mhz = Some(v);
                t.min_mhz = t.min_mhz.map(|m| m.min(v));
            });
            ui.add(Slider::new(format!("gpu-{pci}-max"), max, lo..=hi).step(50.0).label("Highest clock").value_text(mhz(max)).on_change(on_max));
            if x.profiles {
                let saving = tune.power_saving.unwrap_or(x.power_saving_now);
                let on = edit(ui, shared, pci, |t, v| t.power_saving = Some(v > 0.5));
                ui.add(CheckBox::new("Power-saving profile", saving).id(format!("gpu-{pci}-saving")).on_toggle(move |b, w, cx| on(if b { 1.0 } else { 0.0 }, w, cx)));
            }
        }
        Caps::Nvidia(n) => {
            match n.power {
                Some((least, most, _, now)) => {
                    let w = tune.power_limit_w.unwrap_or(now) as f32;
                    let on = edit(ui, shared, pci, |t, v| t.power_limit_w = Some(v.round() as u32));
                    ui.add(Slider::new(format!("gpu-{pci}-power"), w, least as f32..=most as f32).step(1.0).label("Power limit").value_text(format!("{w:.0} W")).on_change(on));
                }
                None => ui.note("Its power limit is the computer's to set, as a laptop's is."),
            }
            let max = tune.max_gpu_mhz.unwrap_or(n.max_gpu_mhz) as f32;
            let on = edit(ui, shared, pci, |t, v| t.max_gpu_mhz = Some(gpu_tuning::round15(v.round() as u32)));
            ui.add(Slider::new(format!("gpu-{pci}-max"), max, n.min_gpu_mhz as f32..=n.max_gpu_mhz as f32).step(15.0).label("Highest clock").value_text(mhz(max)).on_change(on));
            let persistent = tune.persistent.unwrap_or(n.persistent_now.unwrap_or(false));
            let on = edit(ui, shared, pci, |t, v| t.persistent = Some(v > 0.5));
            ui.add(CheckBox::new("Keep its driver ready", persistent).id(format!("gpu-{pci}-persistent")).on_toggle(move |b, w, cx| on(if b { 1.0 } else { 0.0 }, w, cx)));
            ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new("Its work starts sooner, but it never sleeps: more power, more warmth.").dim(true).wrap(true)));
        }
        Caps::None => {}
    }
}
