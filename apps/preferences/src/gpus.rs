//! Preferences ▸ Power's Power Modes and Graphics tabs: what each power
//! mode, and running on battery, puts the processors and each GPU on
//! (the plan: `olo_system::gpu_tuning::Plan`, `olo_system::cpu_tuning`),
//! each GPU, what it drives and its Custom knobs, and what records and
//! plays video. Applied as root by `olo-gpu-tune set` through polkit,
//! kept for the computer, set again at every start and followed as the
//! mode or the power supply changes (`olo-gpu-tune follow`). A sleeping
//! NVIDIA GPU isn't asked what it allows (asking wakes it) unless the
//! person asks: what it said last time is shown.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Arc,
};

use olo_sync::Mutex;
use olo_system::{
    cpu_tuning::CpuPreset,
    gpu_tuning::{self, Caps, GpuTune, Preset, Tuning, When},
    gpus::{self, Gpu},
};
use olo_ui::{
    gpui::{Context, ParentElement as _, SharedString, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, CheckBox, EmptyState, MenuField, Slider, StringView},
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
    /// The GPU Ask It is asking now (its PCI address).
    asking: Option<String>,
    /// The power mode's follower, as installed.
    follower: Follower,
    /// The power mode now (None: no daemon), and whether on battery.
    mode: Option<olo_system::power_profile::Mode>,
    on_battery: bool,
    /// The GPUs whose VA-API driver encodes AV1 (Intel's, AMD's), by
    /// name: screen recordings can be made on them.
    av1: Vec<String>,
    /// The GPU that decodes video through VA-API (its name), and whether
    /// its driver has H.264 and HEVC (Fedora's has only AV1 and VP9).
    decodes: Option<(String, bool)>,
}

#[derive(Clone)]
struct Entry {
    gpu: Gpu,
    /// What it allows (None: asleep, never asked, or it didn't answer).
    caps: Option<Caps>,
    asleep: bool,
    /// Asked, what its driver said instead of answering.
    unanswered: Option<String>,
    /// The kernel has its power management in an error state.
    power_failed: bool,
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
            vendor: if driver == "nvidia" { 0x10de } else { 0x8086 },
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
        s.av1 = vec!["Intel Graphics".into()];
        s.decodes = Some(("Intel Graphics".into(), false));
        s.gpus = Some(vec![
            Entry { gpu: gpu("0000:00:02.0", "Intel Graphics", "xe", true, &["eDP-1", "DP-1", "HDMI-A-1"]), caps: Some(xe), asleep: false, unanswered: None, power_failed: false },
            Entry {
                gpu: gpu("0000:01:00.0", "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU", "nvidia", false, &["HDMI-A-2"]),
                caps: Some(nv),
                asleep: true,
                unanswered: None,
                power_failed: false,
            },
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
                    let (caps, unanswered) = gpu_tuning::caps_or_why(&g, wake);
                    Entry { caps, unanswered, asleep: gpu_tuning::asleep(&g), power_failed: gpu_tuning::power_failed(&g), gpu: g }
                })
                .collect();
            let kept = Tuning::load();
            let follower = follower();
            let mode = olo_system::power_profile::modes().ok().flatten().map(|m| m.active);
            let on_battery = !olo_system::battery::on_ac();
            // (Each driver asked, NVIDIA's left asleep: about 15 ms.)
            let av1: Vec<String> = olo_vaapi::abilities()
                .into_iter()
                .filter(|a| a.av1.is_some())
                .map(|a| gpus.iter().find(|e| e.gpu.render.as_deref() == Some(a.node.as_path())).map_or_else(|| "The GPU".to_owned(), |e| e.gpu.name.clone()))
                .collect();
            let name_of = |node: &std::path::Path| gpus.iter().find(|e| e.gpu.render.as_deref() == Some(node)).map_or_else(|| "The GPU".to_owned(), |e| e.gpu.name.clone());
            use olo_vaapi::decode::{Codec, decoder_for};
            let all = decoder_for(Codec::H264).is_some() && decoder_for(Codec::Hevc).is_some();
            let decodes = decoder_for(Codec::Av1).or_else(|| decoder_for(Codec::Vp9)).or_else(|| decoder_for(Codec::H264)).map(|(node, _)| (name_of(&node), all));
            {
                let mut s = shared.lock().unwrap();
                s.follower = follower;
                s.mode = mode;
                s.on_battery = on_battery;
                s.av1 = av1;
                s.decodes = decodes;
                // (What's being edited stays; a first read starts it.)
                if s.gpus.is_none() || s.edited == s.kept {
                    s.edited = kept.clone();
                }
                s.kept = kept;
                s.gpus = Some(gpus);
                s.asking = None;
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

pub(crate) fn run_tune(tuning: &Tuning) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let tune = exe.with_file_name("olo-gpu-tune");
    let json = serde_json::to_string(tuning).map_err(|e| e.to_string())?;
    // (Polkit asks for the password with Olo's agent: there's no way
    // across to root in process. In tests, as this user: nothing changes.)
    let mut cmd = if olo_system::env::at_the_computer() {
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

/// What screen recordings encode on: the most powerful there is (an
/// NVIDIA GPU's encoder where there's one), that GPU, an Intel or AMD
/// GPU through VA-API (AV1 only: what Fedora's drivers encode), or the
/// processor (`Settings::screen_recording`, read by olo-screenshot's
/// recorder).
fn recordings(ui: &mut Ui, gpus: &[Entry], av1: &[String]) {
    let nvidia = gpus.iter().find(|e| e.gpu.vendor == 0x10de).map(|e| e.gpu.name.clone());
    let vaapi = av1.first().cloned();
    let current = ui.s.screen_recording.encoder.clone();
    let codec = ui.s.screen_recording.codec.clone();
    let most = match (&nvidia, &vaapi) {
        (Some(n), _) => format!("The most powerful ({n})"),
        (None, Some(v)) if codec == "av1" => format!("The most powerful ({v})"),
        _ => "The most powerful (the processor)".to_owned(),
    };
    let on_nvidia = nvidia.as_ref().map(|n| format!("{n} (NVENC)"));
    let on_vaapi = vaapi.as_ref().map(|v| format!("{v} (AV1 only, VA-API)"));
    let mut choices: Vec<(String, &str)> = vec![("auto".into(), most.as_str())];
    if let Some(n) = &on_nvidia {
        choices.push(("nvidia".into(), n.as_str()));
    }
    if let Some(v) = &on_vaapi {
        choices.push(("vaapi".into(), v.as_str()));
    }
    choices.push(("cpu".into(), "The processor (OpenH264)"));
    ui.bbox("Screen Recordings", |ui| {
        ui.choose("recording-encoder", &choices, &current, |f| f.label("Encode on:").divider(100.0), |s, v| s.screen_recording.encoder = v);
        // NVIDIA's encoder: H.264 or HEVC (AV1, no smaller on screens
        // there, is olo-screenshot's `--codec av1`). Without one, H.264 on
        // the processor or AV1 on the Intel or AMD GPU.
        let av1_label = vaapi.as_ref().map(|v| format!("AV1 (on {v})"));
        let formats: Vec<(String, &str)> = match (&nvidia, &av1_label) {
            (Some(_), _) if current != "cpu" && current != "vaapi" => vec![("h264".into(), "H.264 (plays everywhere)"), ("hevc".into(), "HEVC (about half the size)")],
            (None, Some(a)) if current == "auto" => vec![("h264".into(), "H.264 (plays everywhere, on the processor)"), ("av1".into(), a.as_str())],
            _ => Vec::new(),
        };
        if !formats.is_empty() {
            ui.choose("recording-codec", &formats, &codec, |f| f.label("Format:").divider(100.0), |s, v| s.screen_recording.codec = v);
        }
        ui.note(if current == "vaapi" {
            "Recordings are AV1, the only format Fedora's driver for this GPU encodes: a 2560 × 1600 screen takes it about 9 ms a frame, and most players and browsers play it."
        } else if nvidia.is_some() || vaapi.is_some() {
            "A GPU's encoder takes a 4K screen in about 15 ms a frame, the processor about 110: recordings of a lot of motion keep every frame on the GPU, about nine a second on the processor."
        } else {
            "Recordings are encoded on the processor (OpenH264): a 4K screen takes about 110 ms a frame, so a recording of a lot of motion keeps about nine a second."
        });
    });
}

/// What plays video (the Viewer's and Quick Look's player): the most
/// efficient there is (the built-in GPU where it decodes the video, else
/// NVIDIA's, on battery too, else the processor),
/// NVIDIA's GPU (NVDEC) where there's one, or the processor
/// (`Settings::video`, read by olo-preview's player).
fn playback(ui: &mut Ui, gpus: &[Entry], decodes: Option<(&str, bool)>) {
    let nvidia = gpus.iter().find(|e| e.gpu.vendor == 0x10de).map(|e| e.gpu.name.clone());
    let current = ui.s.video.decoder.clone();
    let on_intel = decodes.map(|(n, all)| if all { format!("{n} (every video)") } else { format!("{n} (AV1 and VP9)") });
    let on_nvidia = nvidia.as_ref().map(|n| format!("{n} (NVDEC)"));
    let mut choices: Vec<(String, &str)> = vec![("auto".into(), "Automatic")];
    if let Some(n) = &on_intel {
        choices.push(("intel".into(), n.as_str()));
    }
    if let Some(n) = &on_nvidia {
        choices.push(("nvidia".into(), n.as_str()));
    }
    choices.push(("cpu".into(), "The processor"));
    let menu = "The Viewer's View ▸ Decode With chooses for the video it shows.";
    let rest = if nvidia.is_some() { ", NVIDIA's GPU for the rest, else the processor" } else { ", else the processor" };
    let note = match decodes {
        Some((i, true)) => format!("Automatic: {i} for every video (H.264, HEVC, VP9, AV1), 4K at 60 frames a second with a quarter of a core. {menu}"),
        Some((i, false)) => format!("Automatic: {i} for AV1 and VP9 (Fedora's driver has no H.264 or HEVC: Olo Doctor installs the full one){rest}. {menu}"),
        None if nvidia.is_some() => format!("NVIDIA's GPU decodes H.264, HEVC, VP9 and AV1, 4K at 60 frames a second with little of the processor. {menu}"),
        None => format!("Video is decoded on the processor. {menu}"),
    };
    let battery = ui.s.video.nvidia_on_battery;
    ui.bbox("Video Playback", |ui| {
        ui.choose("video-decoder", &choices, &current, |f| f.label("Decode on:").divider(100.0), |s, v| s.video.decoder = v);
        ui.note(note);
        // (On battery, Automatic wakes NVIDIA's GPU too, unless not.)
        if nvidia.is_some() {
            ui.enabled_if(current == "auto", |ui| ui.check("Automatic wakes NVIDIA's GPU on battery too", battery, |s, v| s.video.nvidia_on_battery = v));
            ui.add(
                div()
                    .pl(px(m::MARK_SIZE + m::MARK_GAP))
                    .child(StringView::new("Its decoder, on its battery preset (Power Modes), costs less than the processor's cores; off, the processor decodes on battery.").dim(true).wrap(true)),
            );
        }
    });
}

/// What a GPU drives, said plainly.
fn drives(g: &Gpu) -> String {
    if g.connectors.is_empty() {
        return "no display ports".into();
    }
    format!("its ports: {}", g.connectors.join(", "))
}

/// Read the GPUs and the tuning once the panel shows, and again as the
/// follower changes it (a power mode chosen, the power supply changing).
fn ensure_read(ui: &mut Ui, st: &mut State) {
    if !st.asked {
        st.asked = true;
        let hold = olo_ui::hold_first_show(ui.window);
        read(st.shared.clone(), olo_ui::proxy(ui.cx), None, hold);
        let (shared, proxy) = (st.shared.clone(), olo_ui::proxy(ui.cx));
        if !cfg!(test) {
            st.watch = olo_ui::watch::watch([std::path::PathBuf::from(gpu_tuning::FILE), std::path::PathBuf::from(FOLLOWER)], move |_| read(shared.clone(), proxy.clone(), None, None));
        }
    }
}

/// A short name for a GPU in the plan's rows ("NVIDIA RTX PRO 2000",
/// not its whole marketing name).
fn short_name(g: &Gpu) -> String {
    let n = g.name.trim();
    for cut in [" Blackwell", " Ada ", " Generation", " Laptop GPU", " Laptop", " (TM)"] {
        if let Some(i) = n.find(cut) {
            return n[..i].trim().to_owned();
        }
    }
    n.to_owned()
}

/// The column of the plan a situation is, and its name.
fn when_name(w: When) -> &'static str {
    match w {
        When::Mode(m) => m.label(),
        When::Battery => "On Battery",
    }
}

/// Preferences ▸ Power ▸ Power Modes, below the mode: what each mode, and
/// running on battery, puts the processors and each GPU on.
pub fn modes(ui: &mut Ui, st: &mut State) {
    ensure_read(ui, st);
    let (gpus, kept, edited, follower, mode, on_battery) = {
        let s = st.shared.lock().unwrap();
        (s.gpus.clone(), s.kept.clone(), s.edited.clone(), s.follower, s.mode, s.on_battery)
    };
    let Some(gpus) = gpus else {
        ui.note("Reading…");
        return;
    };
    // (A row's name and four menus within the pane's box, each menu wide
    // enough for "Cooler and Quieter".)
    // (On the 4-point grid, as the gaps are: whole device pixels at every
    // scale, 1.25× and 1.75× too; at 146 and 150 every cell started on a
    // half pixel at 1.25× and the gaps came out 11.2 and 12.8.)
    const LABEL_W: f32 = 152.0;
    const CELL_W: f32 = 144.0;
    let plain = theme::palette(ui.cx).panel_text;
    ui.bbox("What Each Mode Sets", |ui| {
        // The columns: each mode, and running on battery (bold: in force).
        let mut head = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().w(px(LABEL_W)).flex_shrink_0());
        for w in When::ALL {
            let now = match w {
                When::Mode(m) => mode == Some(m),
                When::Battery => on_battery,
            };
            head = head.child(div().w(px(CELL_W)).flex_shrink_0().child(StringView::new(when_name(w)).bold(now)));
        }
        ui.add(head);
        // A row: its name, and a menu per column.
        let row = |ui: &mut Ui, id: &str, name: String, cells: Vec<(Vec<SharedString>, usize, Box<dyn Fn(&mut Tuning, usize)>)>| {
            let mut r = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().w(px(LABEL_W)).flex_shrink_0().child(StringView::new(name.clone()).color(style::c(plain))));
            for (k, (options, chosen, set)) in cells.into_iter().enumerate() {
                let shared = st.shared.clone();
                let on = ui.on_view(move |_, i: usize, _, cx: &mut Context<PanelView>| {
                    let mut s = shared.lock().unwrap();
                    set(&mut s.edited, i);
                    s.said = None;
                    cx.notify();
                });
                let named = format!("{name} {}", match When::ALL[k] {
                    When::Battery => "on battery".to_owned(),
                    w => format!("in {}", when_name(w)),
                });
                r = r.child(div().w(px(CELL_W)).flex_shrink_0().child(MenuField::new(format!("plan-{id}-{k}"), options, chosen).width(CELL_W).name(named).on_change(on)));
            }
            ui.add(r);
        };
        // The processors.
        let cpu_names = |battery: bool| -> Vec<SharedString> { battery.then(|| SharedString::from("As the Mode")).into_iter().chain(CpuPreset::ALL.iter().map(|p| SharedString::from(p.name()))).collect() };
        let cells = When::ALL
            .into_iter()
            .map(|w| {
                let battery = w == When::Battery;
                let chosen = edited.plan.cpu(w).and_then(|p| CpuPreset::ALL.iter().position(|q| *q == p)).map_or(0, |i| i + usize::from(battery));
                let set: Box<dyn Fn(&mut Tuning, usize)> = Box::new(move |t: &mut Tuning, i: usize| {
                    let preset = if battery { i.checked_sub(1).map(|i| CpuPreset::ALL[i]) } else { Some(CpuPreset::ALL[i]) };
                    t.plan.set_cpu(w, preset);
                });
                (cpu_names(battery), chosen, set)
            })
            .collect();
        row(ui, "cpu", "Processors".into(), cells);
        // Each GPU that can be tuned.
        for e in gpus.iter().filter(|e| !matches!(e.caps, Some(Caps::None))) {
            let (pci, nvidia) = (e.gpu.pci.clone(), e.gpu.vendor == 0x10de);
            let names = |battery: bool| -> Vec<SharedString> { battery.then(|| SharedString::from("As the Mode")).into_iter().chain(Preset::ALL.iter().map(|p| SharedString::from(p.name()))).collect() };
            let cells = When::ALL
                .into_iter()
                .map(|w| {
                    let battery = w == When::Battery;
                    let chosen = edited.plan.gpu(w, &pci, nvidia).and_then(|p| Preset::ALL.iter().position(|q| *q == p)).map_or(0, |i| i + usize::from(battery));
                    let pci = pci.clone();
                    let set: Box<dyn Fn(&mut Tuning, usize)> = Box::new(move |t: &mut Tuning, i: usize| {
                        let preset = if battery { i.checked_sub(1).map(|i| Preset::ALL[i]) } else { Some(Preset::ALL[i]) };
                        t.plan.set_gpu(w, &pci, nvidia, preset);
                    });
                    (names(battery), chosen, set)
                })
                .collect();
            row(ui, &e.gpu.pci, short_name(&e.gpu), cells);
        }
        ui.note("Processors: Saving Power slows them and turns turbo off, Fastest keeps them quick; a GPU's Cooler and Quieter caps its clocks, Fastest lifts them, Custom is its own settings (Graphics). On battery, whatever the mode, a row set to something other than As the Mode is on that: NVIDIA's GPU saves power then, as video wakes it on battery too.");
        match (mode, follower) {
            (_, Follower::Missing) => ui.note("Apply once to let Olo follow the mode and the power supply: an administrator's password is asked, this once."),
            (_, Follower::Old) => ui.note("What follows them is from an older Olo: Apply to bring it up to date (an administrator's password is asked)."),
            (Some(m), Follower::Current) => {
                let gpus_now: Vec<String> = gpus.iter().filter(|e| !matches!(e.caps, Some(Caps::None))).map(|e| format!("{} {}", short_name(&e.gpu), kept.plan.gpu_now(m, on_battery, &e.gpu.pci, e.gpu.vendor == 0x10de).name())).collect();
                let cpu = kept.plan.cpu_now(m, on_battery).name();
                ui.note(format!("Now {}{}: the processors {cpu}, {}.", m.label(), if on_battery { ", on battery" } else { "" }, gpus_now.join(", ")));
            }
            (None, Follower::Current) => ui.note("No power profiles daemon is running, so there's no power mode to follow."),
        }
    });
    apply_row(ui, st, follower);
}

/// Preferences ▸ Power ▸ Graphics: each GPU (what it drives, whether it's
/// awake, its Custom settings), and what records and plays video.
pub fn graphics(ui: &mut Ui, st: &mut State) {
    ensure_read(ui, st);
    let (gpus, edited, follower) = {
        let s = st.shared.lock().unwrap();
        (s.gpus.clone(), s.edited.clone(), s.follower)
    };
    let Some(gpus) = gpus else {
        ui.note("Reading…");
        return;
    };
    if gpus.is_empty() {
        ui.add(EmptyState::new("No GPUs found.").icon("gpu"));
    } else {
        // Side by side, two to a row.
        let mut boxes: Vec<Vec<olo_ui::gpui::AnyElement>> = gpus.iter().map(|e| ui.collect(|ui| gpu_box(ui, e, &edited, &st.shared))).collect();
        let column = |items: Vec<olo_ui::gpui::AnyElement>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
        while !boxes.is_empty() {
            let left = boxes.remove(0);
            let right = if boxes.is_empty() { Vec::new() } else { boxes.remove(0) };
            ui.add(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(column(left)).child(column(right)));
        }
        apply_row(ui, st, follower);
        // (Below Apply, a section of its own: it's taken at once, not
        // applied with the tuning.)
        ui.space(m::SECTION - 2.0 * m::STACK);
    }
    let (av1, decodes) = {
        let s = st.shared.lock().unwrap();
        (s.av1.clone(), s.decodes.clone())
    };
    // (Side by side: under each other, the pane is taller than the
    // smallest screen.)
    ui.columns(|ui| recordings(ui, &gpus, &av1), |ui| playback(ui, &gpus, decodes.as_ref().map(|(n, all)| (n.as_str(), *all))));
}

/// Apply and Revert, for the plan and the GPUs' Custom settings (root's:
/// kept for the computer), and what the last Apply said.
fn apply_row(ui: &mut Ui, st: &mut State, follower: Follower) {
    let (kept, edited, applying, said) = {
        let s = st.shared.lock().unwrap();
        (s.kept.clone(), s.edited.clone(), s.applying, s.said.clone())
    };
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
}

/// Change GPU `pci`'s Custom settings. With `switch` (where it is now,
/// whether it's NVIDIA's, and the settings it runs at now, `from`), it
/// isn't on Custom yet: it's made Custom there, from those.
fn edit(
    ui: &Ui,
    shared: &Arc<Mutex<Shared>>,
    pci: &str,
    switch: Option<(When, bool, GpuTune)>,
    f: impl Fn(&mut GpuTune, f32) + 'static,
) -> impl Fn(f32, &mut Window, &mut olo_ui::gpui::App) + 'static {
    let (shared, pci) = (shared.clone(), pci.to_owned());
    ui.on_view(move |_, v: f32, _, cx: &mut Context<PanelView>| {
        let mut s = shared.lock().unwrap();
        let mut t = match &switch {
            Some((when, nvidia, from)) => {
                s.edited.plan.set_gpu(*when, &pci, *nvidia, Some(Preset::Custom));
                from.clone()
            }
            None => s.edited.custom_of(&pci),
        };
        f(&mut t, v);
        s.edited.set_custom(t);
        s.said = None;
        cx.notify();
    })
}

/// What GPU `caps` is set to now by `tune` (the tuning in force): an xe
/// GPU's clocks as its driver says (read as the panel was), NVIDIA's as
/// they were set (its driver doesn't say what clocks are locked).
fn in_force(tune: &GpuTune, caps: &Caps) -> String {
    match caps {
        Caps::Xe(x) => {
            let profile = if x.power_saving_now { ", the power-saving profile" } else { "" };
            format!("{}–{} MHz{profile}", x.min_now, x.max_now)
        }
        Caps::Nvidia(n) => {
            let mut s = match tune.max_gpu_mhz {
                Some(max) => format!("its clock up to {max} MHz"),
                None => format!("its clock as the driver has it, up to {} MHz", n.max_gpu_mhz),
            };
            if let Some(w) = tune.power_limit_w {
                s.push_str(&format!(", a {w} W power limit"));
            }
            if tune.persistent == Some(true) {
                s.push_str(", its driver kept ready");
            }
            s
        }
        Caps::None => String::new(),
    }
}

fn gpu_box(ui: &mut Ui, e: &Entry, edited: &Tuning, shared: &Arc<Mutex<Shared>>) {
    let g = &e.gpu;
    let pci = g.pci.clone();
    let nvidia = g.vendor == 0x10de;
    let (tune, driver, mode, on_battery) = {
        let s = shared.lock().unwrap();
        (s.kept.of(&pci), s.kept.driver.iter().find(|d| d.pci == pci).cloned(), s.mode.unwrap_or(olo_system::power_profile::Mode::Balanced), s.on_battery)
    };
    let now = tune.preset;
    let asking = shared.lock().unwrap().asking.as_deref() == Some(pci.as_str());
    ui.bbox(&g.name, |ui| {
        let state = if e.power_failed {
            "its power management failed"
        } else if e.asleep {
            "asleep: nothing uses it"
        } else {
            "awake"
        };
        ui.note(format!("{} driver, {state}; {}.", g.driver, drives(g)));
        let Some(caps) = e.caps.clone() else {
            // (Asking, it says so; asked, what its driver said: Ask It
            // came back to the same words, and looked to do nothing.)
            let words = match (&e.unanswered, e.power_failed) {
                _ if asking => "Asking it…".to_owned(),
                (Some(why), true) => format!("What it can be set to isn't known: {why}. It stays so until the computer restarts, which starts its driver again."),
                (Some(why), false) => format!("What it can be set to isn't known: {why}."),
                (None, _) => "What it can be set to isn't known yet: asking it wakes it for a moment.".to_owned(),
            };
            ui.note(words);
            let (shared, proxy, pci) = (shared.clone(), olo_ui::proxy(ui.cx), pci.clone());
            let wake = ui.on_click_view(move |_, _, cx| {
                shared.lock().unwrap().asking = Some(pci.clone());
                read(shared.clone(), proxy.clone(), Some(pci.clone()), None);
                cx.notify();
            });
            let label = if e.unanswered.is_some() { "Ask Again" } else { "Ask It" };
            ui.enabled_if(!asking, |ui| ui.button(label, wake));
            return;
        };
        if matches!(caps, Caps::None) {
            ui.note(format!("Nothing to tune for its driver ({}) yet.", g.driver));
            return;
        }
        // The settings show what it runs at now (Ben, 2026-11-07: "it
        // says nvidia is on cooler and quieter but the slider hasn't
        // changed?": they had been Custom's alone, under their own
        // heading). Moving one makes it Custom where it is now (the mode,
        // or on battery when that has a choice of its own), from these.
        let when = situation(&edited.plan, mode, on_battery, &pci, nvidia);
        let custom = edited.plan.gpu_now(mode, on_battery, &pci, nvidia) == Preset::Custom;
        let place = match when {
            When::Battery => "on battery".to_owned(),
            When::Mode(m) => format!("in {}", m.label()),
        };
        if custom {
            // (Made Custom here, not applied yet: it's still on its preset.)
            let on = if now == Preset::Custom { format!("On Custom {place}") } else { format!("Custom {place} once applied (on {} until then)", now.name()) };
            ui.note(format!("{on}: what's set below. Power Modes says what each mode puts it on."));
            knobs(ui, &caps, &edited.custom_of(&pci), driver.as_ref(), shared, &pci, None);
        } else {
            ui.note(format!("On {} now: {}. Moving a setting makes it Custom {place}; Power Modes says what each mode puts it on.", now.name(), in_force(&tune, &caps)));
            knobs(ui, &caps, &shown(&tune, &caps), driver.as_ref(), shared, &pci, Some((when, nvidia)));
        }
    });
}

/// What decides GPU `pci`'s preset now: being on battery, when the plan
/// has a choice for it there, else the power mode.
fn situation(plan: &gpu_tuning::Plan, mode: olo_system::power_profile::Mode, on_battery: bool, pci: &str, nvidia: bool) -> When {
    if on_battery && plan.gpu(When::Battery, pci, nvidia).is_some() { When::Battery } else { When::Mode(mode) }
}

/// The settings GPU `caps` runs at now, under the tuning in force
/// `tune`, as the knobs show them: xe's clocks and profile as its
/// driver said, NVIDIA's as they were set (none: its driver's own).
fn shown(tune: &GpuTune, caps: &Caps) -> GpuTune {
    let mut t = GpuTune { pci: tune.pci.clone(), preset: Preset::Custom, ..Default::default() };
    match caps {
        Caps::Xe(x) => {
            t.min_mhz = Some(x.min_now);
            t.max_mhz = Some(x.max_now);
            t.power_saving = Some(x.power_saving_now);
        }
        Caps::Nvidia(_) => {
            t.max_gpu_mhz = tune.max_gpu_mhz;
            t.power_limit_w = tune.power_limit_w;
            t.persistent = tune.persistent;
        }
        Caps::None => {}
    }
    t
}

/// GPU `pci`'s settings, `tune`: on Custom, Custom's (a knob not set
/// shows what Custom would set it to, the driver's own: xe's limits as
/// noted before it was first tuned, `driver`); on a preset, what it runs
/// at now (`shown`), and `switch` (where it is now, whether it's
/// NVIDIA's) makes it Custom there as one is moved.
fn knobs(ui: &mut Ui, caps: &Caps, tune: &GpuTune, driver: Option<&gpu_tuning::DriverLimits>, shared: &Arc<Mutex<Shared>>, pci: &str, switch: Option<(When, bool)>) {
    let switch = switch.map(|(w, nvidia)| (w, nvidia, tune.clone()));
    let mhz = |v: f32| format!("{v:.0} MHz");
    match caps {
        Caps::Xe(x) => {
            let (lo, hi) = (x.lowest as f32, x.highest as f32);
            // (As `gpu_tuning::apply` takes a knob left unset.)
            let own_min = driver.and_then(|d| d.min_mhz).unwrap_or(x.efficient.max(x.lowest)).clamp(x.lowest, x.highest);
            let own_max = driver.and_then(|d| d.max_mhz).unwrap_or(x.highest).clamp(own_min, x.highest);
            let min = tune.min_mhz.unwrap_or(own_min) as f32;
            let max = tune.max_mhz.unwrap_or(own_max) as f32;
            // (The other end kept as shown: it had taken the one moved.)
            let on_min = edit(ui, shared, pci, switch.clone(), move |t, v| {
                let v = v.round() as u32;
                t.min_mhz = Some(v);
                t.max_mhz = Some(t.max_mhz.unwrap_or(own_max).max(v));
            });
            ui.add(Slider::new(format!("gpu-{pci}-min"), min, lo..=hi).step(50.0).label("Lowest clock").value_text(mhz(min)).on_change(on_min));
            let on_max = edit(ui, shared, pci, switch.clone(), move |t, v| {
                let v = v.round() as u32;
                t.max_mhz = Some(v);
                t.min_mhz = Some(t.min_mhz.unwrap_or(own_min).min(v));
            });
            ui.add(Slider::new(format!("gpu-{pci}-max"), max, lo..=hi).step(50.0).label("Highest clock").value_text(mhz(max)).on_change(on_max));
            if x.profiles {
                let saving = tune.power_saving.unwrap_or(false);
                let on = edit(ui, shared, pci, switch.clone(), |t, v| t.power_saving = Some(v > 0.5));
                ui.add(CheckBox::new("Power-saving profile", saving).id(format!("gpu-{pci}-saving")).on_toggle(move |b, w, cx| on(if b { 1.0 } else { 0.0 }, w, cx)));
            }
        }
        Caps::Nvidia(n) => {
            match n.power {
                Some((least, most, _, now)) => {
                    let w = tune.power_limit_w.unwrap_or(now) as f32;
                    let on = edit(ui, shared, pci, switch.clone(), |t, v| t.power_limit_w = Some(v.round() as u32));
                    ui.add(Slider::new(format!("gpu-{pci}-power"), w, least as f32..=most as f32).step(1.0).label("Power limit").value_text(format!("{w:.0} W")).on_change(on));
                }
                None => ui.note("Its power limit is the computer's to set, as a laptop's is."),
            }
            let max = tune.max_gpu_mhz.unwrap_or(n.max_gpu_mhz) as f32;
            let on = edit(ui, shared, pci, switch.clone(), |t, v| t.max_gpu_mhz = Some(gpu_tuning::round15(v.round() as u32)));
            ui.add(Slider::new(format!("gpu-{pci}-max"), max, n.min_gpu_mhz as f32..=n.max_gpu_mhz as f32).step(15.0).label("Highest clock").value_text(mhz(max)).on_change(on));
            let persistent = tune.persistent.unwrap_or(n.persistent_now.unwrap_or(false));
            let on = edit(ui, shared, pci, switch.clone(), |t, v| t.persistent = Some(v > 0.5));
            ui.add(CheckBox::new("Keep its driver ready", persistent).id(format!("gpu-{pci}-persistent")).on_toggle(move |b, w, cx| on(if b { 1.0 } else { 0.0 }, w, cx)));
            ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new("Its work starts sooner, but it never sleeps: more power, more warmth.").dim(true).wrap(true)));
        }
        Caps::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_system::gpu_tuning::{NvidiaCaps, XeCaps};

    /// Each GPU says what the mode has it on, in numbers (Ben,
    /// 2026-11-07: "changing the power mode in prefs -> power doesn't
    /// change the clock setting of the nvidia gpu?": its box showed only
    /// Custom's slider, at its highest clock whatever the mode).
    #[test]
    fn what_each_gpu_is_on_now() {
        let nvidia = Caps::Nvidia(NvidiaCaps { power: None, min_gpu_mhz: 180, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let cooler = gpu_tuning::preset("0000:01:00.0", Preset::Cooler, &nvidia);
        assert_eq!(in_force(&cooler, &nvidia), "its clock up to 2160 MHz");
        let fastest = gpu_tuning::preset("0000:01:00.0", Preset::Fastest, &nvidia);
        assert_eq!(in_force(&fastest, &nvidia), "its clock as the driver has it, up to 3090 MHz");
        let xe = Caps::Xe(XeCaps { lowest: 400, efficient: 650, highest: 2450, min_now: 650, max_now: 1600, profiles: true, power_saving_now: true });
        assert_eq!(in_force(&GpuTune::default(), &xe), "650–1600 MHz, the power-saving profile");
    }

    /// On a preset, the settings show what it runs at now, and what
    /// decides that (Ben, 2026-11-07: "it says nvidia is on cooler and
    /// quieter but the slider hasn't changed?").
    #[test]
    fn the_settings_show_what_it_runs_at() {
        use olo_system::power_profile::Mode;
        let nvidia = Caps::Nvidia(NvidiaCaps { power: None, min_gpu_mhz: 180, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let cooler = gpu_tuning::preset("0000:01:00.0", Preset::Cooler, &nvidia);
        assert_eq!(shown(&cooler, &nvidia).max_gpu_mhz, Some(2160));
        // (Fastest: its driver's own, which the slider shows as its top.)
        assert_eq!(shown(&gpu_tuning::preset("0000:01:00.0", Preset::Fastest, &nvidia), &nvidia).max_gpu_mhz, None);
        let xe = Caps::Xe(XeCaps { lowest: 400, efficient: 650, highest: 2450, min_now: 1600, max_now: 2450, profiles: true, power_saving_now: false });
        let t = shown(&GpuTune::default(), &xe);
        assert_eq!((t.min_mhz, t.max_mhz, t.power_saving, t.preset), (Some(1600), Some(2450), Some(false), Preset::Custom));
        // On battery NVIDIA's has a choice of its own (Cooler and
        // Quieter), Intel's follows the mode; plugged in, both the mode.
        let plan = gpu_tuning::Plan::default();
        assert_eq!(situation(&plan, Mode::Performance, true, "0000:01:00.0", true), When::Battery);
        assert_eq!(situation(&plan, Mode::Performance, true, "0000:00:02.0", false), When::Mode(Mode::Performance));
        assert_eq!(situation(&plan, Mode::Balanced, false, "0000:01:00.0", true), When::Mode(Mode::Balanced));
    }
}
