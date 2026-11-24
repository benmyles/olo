//! Performance: the supervisor that keeps the desktop responsive
//! (olo-supervisor; off by default), what it may do, and every number its
//! judgements go by. The settings live in their own file
//! (`performance.json`), which the supervisor and Deskbar's item follow;
//! a watcher thread keeps the panel current when they change it (Deskbar
//! turning it on, an alert's Ignore This Process).
//!
//! And the scheduler the computer runs under, where its kernel can load
//! one (sched_ext): the kernel's own or LAVD (`scx_lavd`, recommended),
//! with what is scheduling now as the kernel says it. That isn't a
//! setting of the person's but the computer's: choosing one has Olo
//! Doctor set it (`olo-doctor --scheduler`, as root through polkit), and
//! what's shown is read from the system (`olo_system::scheduler`).
//!
//! And the kernel's settings Olo suggests, with Olo's own pieces in the
//! kernel (the Kernel tab; `olo_system::kernel_tuning`): each with a
//! switch, what it does, and its figures as they are, read every two
//! seconds while the tab is shown and not at all otherwise. They're the
//! computer's too: a switch has Olo Doctor set it (`olo-doctor --kernel`).
//!
//! Beside it, the kept core (`Settings::keep_core`, app_server's, and the
//! system's services kept off it too, Olo Doctor's as root:
//! `olo-doctor --keep-core`), and the pointer on a thread of its own:
//! app_server's settings (`Settings::input_thread`), through the form
//! like any other.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Child, Cx, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px, style, theme,
    widgets::{Button, CheckBox, RadioButton, Slider, StringView, TabView, TextControl, TextField, TextFieldEvent, dashboard::Facts, heading},
};
use olo_protocol::performance::{PerformanceSettings, Thresholds};
use olo_sync::Mutex;
use olo_system::{
    kernel_tuning::{self as tuning, Setting},
    scheduler::{self, Choice},
};

use crate::{Msg, PanelView, form::Ui, panel};

/// The scheduler as last read, and a change of it under way.
#[derive(Clone, Default)]
struct Scheduler {
    /// None until read; Some(None): this kernel can't load one (nothing
    /// is shown).
    state: Option<Option<scheduler::State>>,
    /// The one being put in force (the administrator's password asked,
    /// the steps run): shown as chosen meanwhile.
    applying: Option<Choice>,
    /// How the last change went, until the next.
    said: Option<Result<String, String>>,
    /// The kept core's processors (None: too few to keep one), and those
    /// the system's services are kept off (Olo Doctor's drop-in).
    kept: Option<Vec<usize>>,
    system_kept: Option<Vec<usize>>,
    /// The system's part being changed (Olo Doctor, a password asked),
    /// and how it went.
    keeping: bool,
    keep_said: Option<Result<String, String>>,
}

/// The kernel's settings as last read, and a switch of one under way.
#[derive(Clone, Default)]
struct Kernel {
    /// None until read.
    state: Option<tuning::State>,
    /// The one being switched, and to what (the administrator's password
    /// asked, the steps run): shown so meanwhile.
    applying: Option<(Setting, bool)>,
    /// How the last switch went, until the next.
    said: Option<Result<String, String>>,
}

/// The Kernel tab's reader: a thread that reads the settings and their
/// figures every two seconds while the tab is shown (the connections on
/// BBRv3 every fifth time: listing them is the kernel's walk of its
/// whole table), and sleeps until it's shown otherwise; it ends with the
/// panel.
struct KernelWatch {
    shared: Arc<Mutex<Kernel>>,
    /// The tab is shown; the panel has closed.
    shown: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    thread: Option<std::thread::Thread>,
}

impl KernelWatch {
    fn new() -> KernelWatch {
        let (shared, shown, closed): (Arc<Mutex<Kernel>>, _, _) = (Default::default(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        // Tests (the layout lint): a computer with everything on, and
        // the longest of what each says.
        if cfg!(test) {
            shared.lock().unwrap().state = Some(made_up_kernel());
            return KernelWatch { shared, shown, closed, thread: None };
        }
        let (out, proxy, is_shown, is_closed) = (shared.clone(), olo_kit::proxy(), shown.clone(), closed.clone());
        let thread = std::thread::Builder::new()
            .name("kernel-settings".into())
            .spawn(move || {
                let places = tuning::Places::here();
                let (mut last, mut looks): (Option<tuning::State>, u32) = (None, 0);
                while !is_closed.load(Ordering::Relaxed) {
                    if !is_shown.load(Ordering::Relaxed) {
                        // (Shown again: everything afresh.)
                        looks = 0;
                        std::thread::park();
                        continue;
                    }
                    let now = match &last {
                        Some(before) if looks % 5 != 0 => tuning::look_again(&places, before),
                        _ => tuning::look(&places),
                    };
                    looks += 1;
                    last = Some(now.clone());
                    {
                        let mut k = out.lock().unwrap();
                        if k.state.as_ref() != Some(&now) {
                            k.state = Some(now);
                            proxy.send(Msg::Redraw);
                        }
                    }
                    std::thread::park_timeout(Duration::from_secs(2));
                }
            })
            .expect("thread");
        KernelWatch { shared, shown, closed, thread: Some(thread.thread().clone()) }
    }

    /// The tab came to be shown, or stopped being.
    fn show(&self, on: bool) {
        if self.shown.swap(on, Ordering::Relaxed) != on
            && on
            && let Some(t) = &self.thread
        {
            t.unpark();
        }
    }
}

impl Drop for KernelWatch {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Some(t) = &self.thread {
            t.unpark();
        }
    }
}

/// A computer with every setting on, for the layout lint.
#[cfg(test)]
fn made_up_kernel() -> tuning::State {
    tuning::State {
        preemption: Some(tuning::Preemption { built: "lazy".into(), started: Some("full".into()), kept: Some("full".into()), switched: None, live: true, lost: false }),
        turns: Some(tuning::Turns { built_ns: 2_800_000, switched_ns: Some(tuning::SHORT_TURN_NS), kept: true, live: true }),
        slow_disks: tuning::SlowDisks {
            can: true,
            kept: true,
            disks: vec![tuning::SlowDisk { name: "sdb".into(), model: "Kingston DataTraveler 3.0".into(), limit: Some(tuning::SLOW_DISK_BYTES) }],
            may_wait: 15_355_826_176,
            waiting: 30_195_712,
        },
        dirty_writeback: Some(tuning::DirtyWriteback {
            ram_bytes: 66_827_145_216,
            bg_bytes: tuning::DIRTY_BACKGROUND_BYTES,
            dirty_bytes: tuning::DIRTY_BYTES,
            bg_ratio: 0,
            dirty_ratio: 0,
            bg_now_bytes: tuning::DIRTY_BACKGROUND_BYTES,
            limit_now_bytes: tuning::DIRTY_BYTES,
            waiting_bytes: 30_195_712,
            kept: true,
        }),
        nvme_scheduler: Some(tuning::NvmeScheduler {
            kept: true,
            disks: vec![tuning::NvmeDisk { name: "nvme0n1".into(), model: "SKHynix HFS002TFM9X179N".into(), scheduler: "kyber".into(), has_kyber: true }],
        }),
        bbr3: Some(tuning::Bbr3 {
            control: "bbr3".into(),
            loaded: true,
            kept: true,
            said: Some("bbr3".into()),
            on_it: Some(tuning::OnIt { connections: 137, of: 152, round_trip_ms: Some(118.4), sent: 386_748_267, sent_again: 35_939 }),
            series: "7.9".into(),
            checked: false,
        }),
        graphics_memory: Some(tuning::GraphicsMemory { patched: true, kept: true, said: None }),
        graphics_pages: Some(tuning::GraphicsPages {
            loaded: true,
            offered: true,
            in_force: true,
            kept: true,
            checked: true,
            changed: Some(20_481),
            said: Some("in force".into()),
            kernel: "7.3.4-300.fc45.x86_64".into(),
        }),
    }
}

#[cfg(not(test))]
fn made_up_kernel() -> tuning::State {
    tuning::State::default()
}

pub struct State {
    settings: Option<PerformanceSettings>,
    scheduler: Arc<Mutex<Scheduler>>,
    kernel: KernelWatch,
    rx: Receiver<PerformanceSettings>,
    /// Our own last change: the watcher's echoes of older files are
    /// ignored for a moment after it.
    edited: Option<Instant>,
    /// The background programs (comma separated).
    background: Child<TextField>,
    _watch: Option<olo_kit::watch::Watch>,
    /// The tab shown (`TABS`).
    tab: usize,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let (tx, rx) = channel();
        // Tests (the layout lint): the defaults, not this account's file.
        let (settings, watch) = if cfg!(test) {
            drop(tx);
            (Some(PerformanceSettings::default()), None)
        } else {
            let proxy = olo_kit::proxy();
            let read = Arc::new(move || {
                let _ = tx.send(PerformanceSettings::load());
                proxy.send(Msg::Redraw);
            });
            let now = read.clone();
            std::thread::Builder::new().name("performance-read".into()).spawn(move || now()).expect("thread");
            (None, olo_kit::watch::watch([PerformanceSettings::path()], move |_| read()))
        };
        let background = olo_kit::widgets::text_field(cx, |f| f.hint("Program names, comma separated"));
        cx.subscribe(&background, |v, e: &TextFieldEvent, cx| {
            if !matches!(e, TextFieldEvent::Changed) {
                return;
            }
            let panel::State::Performance(st) = &v.state else { return };
            let text = st.background.read(cx).text().to_owned();
            change(v, |s| s.background = text.split(',').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect());
            cx.notify();
        });
        // The scheduler: read as the panel opens and again every few
        // seconds while it's open (the kernel can put one out at any
        // time). Tests: LAVD chosen and scheduling.
        let scheduler: Arc<Mutex<Scheduler>> = Default::default();
        if cfg!(test) {
            let mut s = scheduler.lock().unwrap();
            s.state = Some(Some(scheduler::State { chosen: Choice::Lavd, installed: true, loaded: Some("lavd_1.1.4_x86_64_unknown_linux_gnu".into()) }));
            // (The ThinkPad's, its services not yet kept off: the longest
            // the box gets.)
            s.kept = Some(vec![2]);
        } else {
            let (out, proxy) = (Arc::downgrade(&scheduler), olo_kit::proxy());
            std::thread::Builder::new()
                .name("scheduler".into())
                .spawn(move || {
                    let places = scheduler::Places::here();
                    let kept = olo_system::cores::kept();
                    // Ends once the panel (the only strong reference) has closed.
                    while let Some(shared) = out.upgrade() {
                        let now = scheduler::look(&places);
                        let system_kept = olo_system::cores::system_kept();
                        {
                            let mut s = shared.lock().unwrap();
                            if s.state.as_ref() != Some(&now) || s.system_kept != system_kept || s.kept != kept {
                                s.state = Some(now);
                                (s.system_kept, s.kept) = (system_kept, kept.clone());
                                proxy.send(Msg::Redraw);
                            }
                        }
                        drop(shared);
                        std::thread::sleep(Duration::from_secs(5));
                    }
                })
                .expect("thread");
        }
        State { settings, scheduler, kernel: KernelWatch::new(), rx, edited: None, background, _watch: watch, tab: 0 }
    }
}

/// Performance's tabs.
pub const TABS: [&str; 5] = ["Supervisor", "Watchdog", "Scheduler", "Kernel", "Disks"];
/// The Kernel and Disks tabs' places among them (both read `tuning::State`).
const KERNEL_TAB: usize = 3;
const DISKS_TAB: usize = 4;

fn is_kernel_tab(tab: usize) -> bool {
    tab == KERNEL_TAB || tab == DISKS_TAB
}

/// Performance shown on tab `name` (`--tab`).
pub fn show_tab(st: &mut State, name: &str) {
    if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(name)) {
        st.tab = i;
        st.kernel.show(is_kernel_tab(i));
    }
}

/// Switch one of the kernel's settings: Olo Doctor does it (`olo-doctor
/// --kernel SETTING on|off`: its helper, as root through polkit, which
/// asks for the password), on a thread; how it went is said under the
/// boxes.
fn set_kernel(shared: Arc<Mutex<Kernel>>, proxy: olo_kit::Proxy, setting: Setting, on: bool) {
    {
        let mut k = shared.lock().unwrap();
        if k.applying.is_some() {
            return;
        }
        k.applying = Some((setting, on));
        k.said = None;
    }
    proxy.send(Msg::Redraw);
    std::thread::Builder::new()
        .name("kernel-apply".into())
        .spawn(move || {
            let said = crate::ask_doctor(&["--kernel", setting.word(), if on { "on" } else { "off" }]);
            let now = tuning::look(&tuning::Places::here());
            let mut k = shared.lock().unwrap();
            (k.applying, k.said, k.state) = (None, Some(said), Some(now));
            drop(k);
            proxy.send(Msg::Redraw);
        })
        .expect("thread");
}

/// A size in words ("16 MB", "1 GB", "14.3 GB").
fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 && b % (1 << 30) == 0 => format!("{} GB", b >> 30),
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} KB", b as f64 / 1024.0),
    }
}

/// A count in words ("386.7 million", "35,939").
fn count(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1} billion", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1} million", n as f64 / 1e6),
        n => style::thousands(n as usize),
    }
}

/// What the Kernel tab says of each setting: its box's title, its
/// switch's label, whether it's on, what it does, and its figures. A
/// setting the computer hasn't is left out, or says why it can't be set
/// (no switch).
struct KernelBox {
    title: &'static str,
    setting: Setting,
    label: &'static str,
    /// None: it can't be set here (`about` says why).
    on: Option<bool>,
    /// Its switch is there and can't be pressed: what it would turn on
    /// can't be, here (`about` says why).
    locked: bool,
    /// Olo Doctor has more to say of it: a button that opens it.
    doctor: bool,
    about: String,
    facts: Vec<(&'static str, String)>,
}

fn kernel_boxes(s: &tuning::State, scheduled_by: Option<&str>) -> Vec<KernelBox> {
    let mut boxes = Vec::new();
    boxes.push(match &s.preemption {
        Some(p) => KernelBox {
            title: "Preemption",
            setting: Setting::Preemption,
            label: "Switch to a woken program at once",
            on: Some(p.on()),

            locked: false,
            doctor: false,
            // (A scheduler loaded over the kernel's asks for its switches
            // itself: the setting changes nothing while it schedules.)
            about: match scheduled_by {
                Some(name) => olo_kit::tr!("With this on, a program woken while the processors are busy runs at once instead of waiting for the kernel's next tick (full preemption). {} is scheduling now and decides that itself: this is for the kernel's own scheduler.", name),
                None => olo_kit::words::tr("With this on, a program woken while the processors are busy runs at once instead of waiting up to a millisecond for the kernel's next tick (full preemption).").into(),
            },
            facts: vec![("Now", p.now().to_owned()), ("Next start", p.next().to_owned())],
        },
        None => KernelBox {
            title: "Preemption",
            setting: Setting::Preemption,
            label: "",
            on: None,

            locked: false,
            doctor: false,
            about: olo_kit::words::tr("This kernel has one way of preempting only: it wasn't built to be told another.").into(),
            facts: Vec::new(),
        },
    });
    if let Some(t) = &s.turns {
        let ms = |ns: u64| olo_kit::tr!("{} ms", format!("{}", ns as f64 / 1e6));
        boxes.push(KernelBox {
            title: "Processor Turns",
            setting: Setting::ShortTurns,
            label: "Keep a busy program's turn short",
            on: Some(t.on()),

            locked: false,
            doctor: false,
            // (As preemption: a scheduler loaded over the kernel's has
            // turns of its own.)
            about: match scheduled_by {
                Some(name) => {
                    olo_kit::tr!("With this on, a busy program gives way to a waiting one after 0.7 ms instead of {} (base slice). {} is scheduling now and decides that itself.", ms(t.built_ns), name)
                }
                None => olo_kit::tr!("With this on, a busy program gives way to a waiting one after 0.7 ms instead of {}, so windows and typing keep up under load (base slice).", ms(t.built_ns)),
            },
            // (The same rows on and off: off, both say Linux's own.)
            facts: vec![("Turn now", ms(t.now_ns())), ("Linux's own", ms(t.built_ns)), ("Kept after restart", olo_kit::words::tr(if t.kept { "yes" } else { "no" }).to_owned())],
        });
    }
    let d = &s.slow_disks;
    let may_wait = if d.kept {
        olo_kit::tr!("{} for each", size(tuning::SLOW_DISK_BYTES))
    } else if d.may_wait > 0 {
        // (The size alone: "at most" says it's a limit.)
        size(d.may_wait)
    } else {
        olo_kit::words::tr("no limit").to_owned()
    };
    // (What may wait in memory to be written: "May wait" read as the disk
    // being let wait. Both reviews.)
    let mut slow_facts = vec![("Unwritten at most", may_wait)];
    // (When Writeback is shown above on the same Disks tab, it already
    // reports "Waiting now" for all disks; avoid repeating the same line.)
    if s.dirty_writeback.is_none() {
        slow_facts.push(("Waiting now", olo_kit::tr!("{}, all disks", size(d.waiting))));
    }
    slow_facts.push((
        "Disks plugged in",
        match d.disks.as_slice() {
            [] => olo_kit::words::tr("none").to_owned(),
            disks => disks
                .iter()
                .map(|k| format!("{}, {}", if k.model.is_empty() { &k.name } else { &k.model }, olo_kit::words::tr(if k.limit.is_some() { "limited" } else { "not limited" })))
                .collect::<Vec<_>>()
                .join("; "),
        },
    ));
    slow_facts.push(("Kept after restart", olo_kit::words::tr(if d.kept { "yes" } else { "no" }).to_owned()));
    boxes.push(if d.can {
        KernelBox {
            title: "Removable Disks",
            setting: Setting::SlowDisks,
            label: "Limit what waits to be written to them",
            on: Some(d.on()),

            locked: false,
            doctor: false,
            about: olo_kit::words::tr("A copy to a stick or a card goes at the disk's own pace, so its progress is true and ejecting is quick.").into(),
            facts: slow_facts,
        }
    } else {
        KernelBox {
            title: "Removable Disks",
            setting: Setting::SlowDisks,
            label: "",
            on: None,

            locked: false,
            doctor: false,
            about: olo_kit::words::tr("This kernel can't hold a disk to a limit of its own (Linux 6.2 and later can).").into(),
            facts: Vec::new(),
        }
    });
    if let Some(w) = &s.dirty_writeback {
        let starts = if w.bg_bytes > 0 { size(w.bg_bytes) } else { format!("{}% ({})", w.bg_ratio, size(w.bg_now_bytes)) };
        let limit = if w.dirty_bytes > 0 { size(w.dirty_bytes) } else { format!("{}% ({})", w.dirty_ratio, size(w.limit_now_bytes)) };
        boxes.push(KernelBox {
            title: "Writeback",
            setting: Setting::DirtyWriteback,
            label: "Cap unwritten data in memory",
            on: Some(w.on()),
            locked: false,
            doctor: false,
            about: olo_kit::words::tr("Starts flushing writes at 256 MB and caps them at 1 GB, instead of letting gigabytes pile up and stall reads or Btrfs commits (vm.dirty_bytes).").into(),
            facts: vec![
                ("Starts flushing", starts),
                ("Limit", limit),
                ("Waiting now", size(w.waiting_bytes)),
                ("Kept after restart", olo_kit::words::tr(if w.kept { "yes" } else { "no" }).to_owned()),
            ],
        });
    }
    if let Some(n) = &s.nvme_scheduler {
        let shared_sched = n.disks.first().map(|d| d.scheduler.as_str()).filter(|first| n.disks.iter().all(|d| d.scheduler == *first));
        let mut nvme_facts = Vec::new();
        match (n.disks.as_slice(), shared_sched) {
            ([], _) => nvme_facts.push(("Drives", "none".to_owned())),
            (disks, Some(sched)) => {
                let label = if disks.len() == 1 { "Drive" } else { "Drives" };
                let names = disks.iter().map(|d| if d.model.is_empty() { d.name.as_str() } else { d.model.as_str() }).collect::<Vec<_>>().join(", ");
                nvme_facts.push((label, names));
                nvme_facts.push(("Scheduler now", sched.to_owned()));
            }
            (disks, None) => {
                let names = disks
                    .iter()
                    .map(|d| {
                        let name = if d.model.is_empty() { d.name.as_str() } else { d.model.as_str() };
                        format!("{name} — {}", d.scheduler)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                nvme_facts.push(("Drives", names));
            }
        }
        nvme_facts.push(("Kept after restart", olo_kit::words::tr(if n.kept { "yes" } else { "no" }).to_owned()));
        boxes.push(if n.can() {
            KernelBox {
                title: "NVMe Scheduler",
                setting: Setting::NvmeScheduler,
                label: "Use the kyber I/O scheduler on NVMe drives",
                on: Some(n.on()),
                locked: false,
                doctor: false,
                about: olo_kit::words::tr(
                    "Keeps read latency bounded when heavy synchronous writes or discards flood an NVMe drive, whereas Linux's default \"none\" only throttles background writeback (queue/scheduler).",
                )
                .into(),
                facts: nvme_facts,
            }
        } else {
            KernelBox {
                title: "NVMe Scheduler",
                setting: Setting::NvmeScheduler,
                label: "",
                on: None,
                locked: false,
                doctor: false,
                about: olo_kit::words::tr("This kernel doesn't offer the kyber I/O scheduler on this computer's NVMe drives.").into(),
                facts: Vec::new(),
            }
        });
    }
    if let Some(b) = &s.bbr3 {
        let o = b.on_it.clone();
        boxes.push(KernelBox {
            title: "Network",
            setting: Setting::Bbr3,
            // (Named for what it does: "Send with BBRv3" was the only
            // switch named in jargon. Both reviews.)
            label: "Send at the pace the network can carry",
            on: Some(b.on()),

            locked: false,
            doctor: false,
            about: olo_kit::words::tr(
                "Linux's usual way (cubic) sends until the queues along the path fill, which adds delay; this one measures the path and stays under it (Olo's module, tcp_bbr3).",
            )
            .into(),
            facts: {
                let mut facts = vec![
                    ("New connections", b.control.clone()),
                    ("On it now", o.as_ref().map_or("—".to_owned(), |o| olo_kit::tr!("{} of {} connections", count(o.connections as u64), count(o.of as u64)).to_string())),
                ];
                // (How they go, while some go this way: none, two rows of
                // bare dashes read as broken. Opus's look.)
                if let Some(o) = o.as_ref().filter(|o| o.connections > 0) {
                    facts.push(("Usual round trip", o.round_trip_ms.map_or("—".to_owned(), |ms| olo_kit::tr!("{} ms", if ms < 10.0 { format!("{ms:.1}") } else { format!("{ms:.0}") }).to_string())));
                    facts.push((
                        "Sent again",
                        if o.sent > 0 { olo_kit::tr!("{}% of {} packets", format!("{:.2}", o.sent_again as f64 * 100.0 / o.sent as f64), count(o.sent)).to_string() } else { "—".to_owned() },
                    ));
                }
                // (Built for whatever kernel an update brings; whether it
                // has passed its self-check on that kernel's series.)
                facts.push((
                    "Tried with this kernel",
                    if b.series.is_empty() {
                        "—".to_owned()
                    } else if b.checked {
                        olo_kit::tr!("yes ({})", b.series).to_string()
                    } else {
                        olo_kit::tr!("not yet ({})", b.series).to_string()
                    },
                ));
                facts
            },
        });
    }
    if let Some(g) = &s.graphics_memory {
        // (Named for what it keeps safe, as the box under it is: both
        // are of Intel's graphics memory, and "Graphics" over "Graphics
        // Memory" told no one which did what. Both reviewers' looks,
        // 2026-11-21.)
        boxes.push(KernelBox {
            title: "Window Buffers",
            setting: Setting::GraphicsMemory,
            label: "Keep windows' graphics memory out of swap",
            on: Some(g.on()),

            locked: false,
            doctor: false,
            // ("Windows' buffers" read as Microsoft's: both reviews.)
            about: olo_kit::words::tr("What windows draw into stays out of swap, so the desktop never waits for it (Olo's live patch of Intel's driver).").into(),
            facts: vec![
                ("The patch", olo_kit::words::tr(if g.patched { "in the running kernel" } else { "not in the running kernel" }).to_owned()),
                ("Kept after restart", olo_kit::words::tr(if g.kept { "yes" } else { "no" }).to_owned()),
            ],
        });
    }
    if let Some(g) = &s.graphics_pages {
        let kernel = g.kernel.split('-').next().unwrap_or_default().to_owned();
        // (Where the module isn't in the kernel and can't be, the box
        // keeps its switch, its rows and its shape, and its paragraph
        // says why in place of what it does: a new kernel whose build
        // it wasn't checked on yet; a kernel whose pool it found, as it
        // loaded, not to be the one it was written for, of which Olo
        // Doctor has the module's own words. Both looks, twice.)
        let (unchecked, kept_out) = (!g.in_force && !g.checked, !g.in_force && g.checked && g.kept_out().is_some());
        let about: String = if unchecked {
            olo_kit::words::tr("This kernel's build hasn't been checked yet: Olo's module stays out until an Olo update checks it.").into()
        } else if kept_out {
            olo_kit::words::tr("Olo's module checks the kernel as it loads, and it stayed out of this one. Olo Doctor says why.").into()
        } else {
            olo_kit::words::tr("When no large piece is free, Intel's driver takes smaller ones and windows keep drawing (Olo's module, olo_gpu_pages).").into()
        };
        // (In the kernel and switched off is neither in force nor out of
        // it: a trial between its phases, a load switched off by hand.
        // The third review with Astra.)
        let mut facts = vec![(
            "The module",
            olo_kit::words::tr(if g.in_force {
                "in the running kernel"
            } else if g.loaded {
                "in the running kernel, switched off"
            } else {
                "not in the running kernel"
            })
            .to_owned(),
        )];
        match g.changed.filter(|_| g.in_force) {
            // (What it counts, since the module loaded: large pieces asked
            // for with no waiting allowed, given or refused, and whether
            // or not one would have been waited for: "asked for, none
            // waited for" and no more. In the kernel, it was checked: no
            // row need say so.)
            Some(n) => facts.push(("Large pieces", olo_kit::tr!("{} asked for, none waited for", count(n)))),
            None if g.in_force => {}
            // (Checked on this build, and out of it all the same: the
            // row would belie the paragraph over it. Opus's look.)
            None if kept_out => {}
            None if g.checked => facts.push(("Checked", olo_kit::tr!("on this kernel's build, {}", kernel))),
            None => facts.push(("Checked", olo_kit::tr!("not yet on this kernel's build, {}", kernel))),
        }
        facts.push(("Kept after restart", olo_kit::words::tr(if g.kept { "yes" } else { "no" }).to_owned()));
        boxes.push(KernelBox {
            title: "Drawing",
            setting: Setting::GraphicsPages,
            label: "Never wait for large pieces of graphics memory",
            // (In the kernel: on now and from every start. Out of it: the
            // person's choice, its service, which they can take back.
            // What isn't offered yet, or can't work here, can't be
            // chosen: not even while a trial has it in the kernel.)
            on: Some(if g.in_force { g.on() } else { g.kept }),
            locked: !g.kept && (!g.offered || unchecked || kept_out),
            doctor: kept_out,
            about,
            facts,
        });
    }
    boxes
}

/// Render either the Kernel tab (processors on the left, Olo's kernel pieces
/// on the right) or the Disks tab (writeback, NVMe scheduler and removable
/// disks stacked cleanly in one column so everything fits 1280 × 800).
fn kernel_tab(ui: &mut Ui, shared: &Arc<Mutex<Kernel>>, scheduled_by: Option<&str>, disks_only: bool) {
    let k = shared.lock().unwrap().clone();
    let Some(state) = k.state else { return ui.note("Reading…") };
    let p = theme::palette();
    // (The facts' labels in the secondary ink, as the paragraph over
    // them: in the disabled one they read as switched off, and they
    // name what is so now. Astra's look, 2026-11-20.)
    let (dim, text) = (style::secondary_text(&p), style::c(p.panel_text));
    let busy = k.applying.is_some();
    // (`labels`: how far every box's values stand from its labels' left.)
    // (`last`: its column's last box, which takes what height its column
    // has to spare, so both columns end on one line over the note below
    // them. Opus's look.)
    let one = |ui: &mut Ui, b: KernelBox, labels: f32, last: bool| {
        let build = |ui: &mut Ui| {
            let Some(on) = b.on else { return ui.note(b.about) };
            let shown = match k.applying {
                Some((s, to)) if s == b.setting => to,
                _ => on,
            };
            let (shared, proxy, setting) = (shared.clone(), olo_kit::proxy(), b.setting);
            let enabled = !busy && !b.locked;
            ui.add(CheckBox::new(b.label, shown).id(format!("kernel-{}", setting.word())).enabled(enabled).on_toggle(move |on, _| set_kernel(shared.clone(), proxy.clone(), setting, *on)));
            // (Under the label, past the mark.)
            let rows: Vec<(SharedString, SharedString)> = b.facts.into_iter().map(|(l, v)| (SharedString::from(l), SharedString::from(v))).collect();
            let about = StringView::new(b.about).dim(true).wrap(true);
            let about_el: olo_kit::Element = if disks_only {
                // (In the full-width Disks tab, cap the paragraph measure at
                // 600 pt so descriptions wrap into balanced lines instead of
                // running 120 characters wide. Opus's look.)
                olo_kit::IntoElement::into_any_element(div().max_w(px(600.0)).child(about))
            } else {
                olo_kit::IntoElement::into_any_element(about)
            };
            ui.add(
                div()
                    .pl(px(m::MARK_SIZE + m::MARK_GAP))
                    .flex()
                    .flex_col()
                    // (As far under the paragraph as the paragraph is
                    // under the check box: nearer, the facts read as its
                    // next line. Opus's look.)
                    .gap(px(m::STACK))
                    .child(about_el)
                    // (Its rows as far apart as the paragraph's lines: two
                    // boxes side by side with as many lines and rows
                    // between them are then as tall as each other.)
                    .child(Facts::new(rows).colors(dim, text).label_width(labels).row_height(style::PLAIN_SIZE * style::LINE_HEIGHT))
                    // (Under what says Olo Doctor knows more, the way
                    // there: as wide as its words, at the paragraph's
                    // left; as far over the box's edge as a row of
                    // text's own room leaves its letters.)
                    .children(b.doctor.then(|| {
                        div().flex().flex_row().pb(px(m::TIGHT)).child(Button::new("Open Olo Doctor").on_click(|_, _| {
                            let _ = olo_xdg::open::spawn_app(&["olo-doctor".into()], "olo.doctor");
                        }))
                    })),
            );
        };
        let title = b.title;
        if last { ui.growing_bbox(title, build) } else { ui.bbox(title, build) }
    };
    let is_disk = |s: Setting| matches!(s, Setting::DirtyWriteback | Setting::NvmeScheduler | Setting::SlowDisks);
    let said = |text: SharedString| olo_kit::IntoElement::into_any_element(StringView::new(text).dim(true).wrap(true));
    let line = match (&k.applying, &k.said) {
        (Some(_), _) => said(olo_kit::words::tr("Setting it: an administrator's password is asked…").into()),
        (None, Some(Err(why))) => olo_kit::IntoElement::into_any_element(StringView::new(olo_kit::tr!("Not changed. {}", why)).wrap(true).color(style::c(p.failure))),
        (None, Some(Ok(text))) => said(text.clone().into()),
        _ => said(olo_kit::words::tr("All these settings are for the whole computer, so changing one asks for an administrator's password.").into()),
    };
    let all = kernel_boxes(&state, scheduled_by);
    if disks_only {
        let mut disk_boxes: Vec<KernelBox> = all.into_iter().filter(|b| is_disk(b.setting)).collect();
        // Order: Writeback, NVMe Scheduler, Removable Disks.
        disk_boxes.sort_by_key(|b| match b.setting {
            Setting::DirtyWriteback => 0,
            Setting::NvmeScheduler => 1,
            Setting::SlowDisks => 2,
            _ => 3,
        });
        let shown: Vec<SharedString> = disk_boxes.iter().flat_map(|b| b.facts.iter().map(|(l, _)| olo_kit::words::of(SharedString::from(*l)))).collect();
        let labels = olo_kit::widgets::dashboard::facts_label_width(shown.iter().map(|l| l.as_ref()));
        let items = ui.collect(|ui| disk_boxes.into_iter().for_each(|b| one(ui, b, labels, false)));
        ui.add(div().flex().flex_col().gap(px(m::SPACING)).children(items).child(line));
        return;
    }
    // Kernel tab: processor settings on the left, Olo's own pieces in it on
    // the right; whose they all are under both, until a switch has something
    // to say in its place.
    let (settings, pieces): (Vec<KernelBox>, Vec<KernelBox>) =
        all.into_iter().filter(|b| !is_disk(b.setting)).partition(|b| !matches!(b.setting, Setting::Bbr3 | Setting::GraphicsMemory | Setting::GraphicsPages));
    // (A width for each column's labels, its widest as it's shown, each a
    // literal looked up: the values in one line down the column. One for
    // the whole tab set the left column's values far from their short
    // labels: the last review.)
    let width = |boxes: &[KernelBox]| {
        let shown: Vec<SharedString> = boxes.iter().flat_map(|b| b.facts.iter().map(|(l, _)| olo_kit::words::of(SharedString::from(*l)))).collect();
        olo_kit::widgets::dashboard::facts_label_width(shown.iter().map(|l| l.as_ref()))
    };
    let (left_labels, right_labels) = (width(&settings), width(&pieces));
    let equalize = settings.len() == pieces.len();
    let (settings_last, pieces_last) = (settings.len().saturating_sub(1), pieces.len().saturating_sub(1));
    let left = ui.collect(|ui| settings.into_iter().enumerate().for_each(|(i, b)| one(ui, b, left_labels, equalize && i == settings_last)));
    let right = ui.collect(|ui| pieces.into_iter().enumerate().for_each(|(i, b)| one(ui, b, right_labels, equalize && i == pieces_last)));
    // (The columns as far apart as boxes stand over one another, their
    // titles' room included: at `SPACING` the two read as one wall of
    // boxes. Opus's look.)
    let column = |items: Vec<olo_kit::Element>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
    ui.add(div().flex().flex_col().gap(px(m::SPACING)).child(div().flex().flex_row().gap(px(2.0 * m::STACK)).child(column(left)).child(column(right))).child(line));
}

/// Put `choice` in force: Olo Doctor does it (`olo-doctor --scheduler`:
/// its helper, as root through polkit, which asks for the password), on
/// a thread; how it went is said under the choices.
fn set_scheduler(shared: Arc<Mutex<Scheduler>>, proxy: olo_kit::Proxy, choice: Choice) {
    {
        let mut s = shared.lock().unwrap();
        if s.applying.is_some() || s.state.as_ref().and_then(|s| s.as_ref()).is_some_and(|s| s.chosen == choice) {
            return;
        }
        s.applying = Some(choice);
        s.said = None;
    }
    proxy.send(Msg::Redraw);
    std::thread::Builder::new()
        .name("scheduler-apply".into())
        .spawn(move || {
            let said = crate::ask_doctor(&["--scheduler", choice.word()]);
            // (A moment for it to load, so "now" says so.)
            if said.is_ok() {
                std::thread::sleep(Duration::from_millis(1500));
            }
            let now = scheduler::look(&scheduler::Places::here());
            let mut s = shared.lock().unwrap();
            s.applying = None;
            s.said = Some(said);
            s.state = Some(now);
            drop(s);
            proxy.send(Msg::Redraw);
        })
        .expect("thread");
}

/// The Scheduler tab: which scheduler the computer starts with, what
/// each is for, and what is scheduling now.
fn scheduler_tab(ui: &mut Ui, shared: &Arc<Mutex<Scheduler>>) {
    let sch = shared.lock().unwrap().clone();
    ui.bbox("Scheduler", |ui| {
        let state = match sch.state {
            None => return ui.note("Reading…"),
            Some(None) => return ui.note("This computer's kernel can't load another scheduler (sched_ext: Linux 6.12 and later have it), so the kernel's own schedules."),
            Some(Some(state)) => state,
        };
        let shown = sch.applying.unwrap_or(state.chosen);
        for choice in Choice::ALL {
            let (shared, proxy) = (shared.clone(), olo_kit::proxy());
            ui.add(
                RadioButton::new(choice.label(), shown == choice)
                    .id(format!("scheduler-{}", choice.word()))
                    .enabled(sch.applying.is_none())
                    .on_select(move |_, _| set_scheduler(shared.clone(), proxy.clone(), choice)),
            );
            // (Under the label, past the mark.)
            ui.add(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new(choice.about()).dim(true).wrap(true)));
        }
        match (&sch.applying, &sch.said) {
            (Some(_), _) => ui.note("Setting it: an administrator's password is asked…"),
            (None, Some(Err(why))) => ui.add(StringView::new(olo_kit::tr!("Not changed. {}", why)).wrap(true).color(style::c(theme::palette().failure))),
            _ => ui.note(state.now()),
        }
        ui.note("Chosen for the whole computer and kept across restarts. It's the computer's, not this account's, so choosing asks for an administrator's password.");
    });
}

/// The system's part of the kept core put as `on`: Olo Doctor does it
/// (`olo-doctor --keep-core`: its helper, as root through polkit), on a
/// thread; how it went is said in the box.
fn set_system_kept(shared: Arc<Mutex<Scheduler>>, proxy: olo_kit::Proxy, on: bool) {
    {
        let mut s = shared.lock().unwrap();
        if s.keeping {
            return;
        }
        s.keeping = true;
        s.keep_said = None;
    }
    proxy.send(Msg::Redraw);
    std::thread::Builder::new()
        .name("kept-core-apply".into())
        .spawn(move || {
            let said = crate::ask_doctor(&["--keep-core", if on { "on" } else { "off" }]);
            let system_kept = olo_system::cores::system_kept();
            let mut s = shared.lock().unwrap();
            (s.keeping, s.keep_said, s.system_kept) = (false, Some(said), system_kept);
            drop(s);
            proxy.send(Msg::Redraw);
        })
        .expect("thread");
}

/// The kept core (app_server's `keep_core`, through the form like any
/// other): which processor, and the system's services kept off it too,
/// which is root's (Olo Doctor).
fn kept_core_box(ui: &mut Ui, shared: &Arc<Mutex<Scheduler>>) {
    let sch = shared.lock().unwrap().clone();
    let Some(kept) = sch.kept.clone() else { return };
    let list = olo_system::cores::format_list(&kept);
    ui.bbox("Kept Core", |ui| {
        let on = ui.s.keep_core;
        ui.check("Keep a core for the window server", on, |s, v| s.keep_core = v);
        ui.note(&olo_kit::tr!("Every key press and every frame go through the window server's loop and its input thread. With this on they have processor {}, one of the fastest, to themselves: builds, browsers and every other program of yours run on the others, so nothing can keep them waiting.", list));
        let system_off = sch.system_kept.as_ref() == Some(&kept);
        let (text, button) = match (on, sch.system_kept.is_some()) {
            (true, _) if system_off => ("The system's services are kept off it too.", None),
            (true, _) => ("The system's services (updates, indexing) can still run there.", Some(("Keep Them Off", true))),
            (false, true) => ("The system's services are still kept off it.", Some(("Let Them Back", false))),
            (false, false) => ("", None),
        };
        if !text.is_empty() {
            let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(text).dim(true).wrap(true)));
            if let Some((label, want)) = button {
                let (shared, proxy) = (shared.clone(), olo_kit::proxy());
                row = row.child(Button::new(label).enabled(!sch.keeping).on_click(move |_, _| set_system_kept(shared.clone(), proxy.clone(), want)));
            }
            ui.add(row);
        }
        match (&sch.keeping, &sch.keep_said) {
            (true, _) => ui.note("Setting it: an administrator's password is asked…"),
            (false, Some(Err(why))) => ui.add(StringView::new(olo_kit::tr!("Not changed. {}", why)).wrap(true).color(style::c(theme::palette().failure))),
            _ => {}
        }
    });
}

/// The pointer on a thread of its own (app_server's `input_thread`):
/// moved while app_server is busy, every motion kept.
fn pointer_box(ui: &mut Ui) {
    ui.bbox("Pointer", |ui| {
        let on = ui.s.input_thread;
        ui.check("Move the pointer on a thread of its own", on, |s, v| s.input_thread = v);
        ui.note("The pointer keeps moving, and none of its motion is lost, while the desktop is busy: a slow frame, or a wait in the graphics driver under load. New, and being tried: if the mouse, touchpad or keyboard misbehaves with it on, turn it off.");
    });
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
fn set<T: Clone + 'static>(ui: &Ui, f: impl Fn(&mut PerformanceSettings, T) + 'static) -> impl Fn(&T, &mut WindowCx) + 'static {
    ui.on_view(move |v, value: &T, cx| {
        change(v, |s| f(s, value.clone()));
        cx.notify();
    })
}

/// Whether this session has a background group to keep
/// (`olo_system::session_groups`: Olo's login made it), found once: it
/// can't change while the session lasts. (`OLO_BACKGROUND_DEMO`: as if
/// it had, for a picture.)
fn divided() -> bool {
    static DIVIDED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DIVIDED.get_or_init(|| {
        use olo_system::session_groups as groups;
        // (Not asked in a test: the lint's windows are the same whatever
        // session they are drawn in. Its index of settings came out
        // different the first time it ran in a divided session.)
        std::env::var_os("OLO_BACKGROUND_DEMO").is_some() || (!cfg!(test) && groups::own().is_some_and(|own| groups::find(std::path::Path::new(groups::MOUNT), &own).is_some()))
    })
}

fn check(ui: &mut Ui, label: &str, on: bool, f: impl Fn(&mut PerformanceSettings, bool) + 'static) {
    let c = CheckBox::new(olo_kit::words::of_text(label), on).enabled(ui.enabled()).on_toggle(set(ui, f));
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
        row = row.child(div().flex_1().child(Slider::new(format!("perf-{label}"), value, range).label(olo_kit::words::of_text(label)).value_text(text).step(step).enabled(enabled).on_change(on)));
    }
    // One alone is as wide as one of a pair.
    if alone {
        row = row.child(div().flex_1());
    }
    ui.add(row);
}

fn section(ui: &mut Ui, text: &str) {
    ui.space(m::TIGHT);
    ui.add(heading(olo_kit::words::of_text(text)));
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
    let background = &st.background;
    // Tabs, so it fits a laptop's screen: what the supervisor does, the
    // watchdog's thresholds, and the computer's scheduler.
    let tab = st.tab;
    let scheduler = st.scheduler.clone();
    let kernel = st.kernel.shared.clone();
    st.kernel.show(is_kernel_tab(tab));
    let items = ui.collect(|ui| {
        if is_kernel_tab(tab) {
            // (What's loaded over the kernel's scheduler, as the
            // Scheduler tab's reader last saw it: "LAVD" for Olo's.)
            let loaded = scheduler.lock().unwrap().state.clone().flatten().and_then(|s| s.loaded);
            let by = loaded.map(|ops| if ops.starts_with("lavd") { "LAVD".to_owned() } else { ops });
            return kernel_tab(ui, &kernel, by.as_deref(), tab == DISKS_TAB);
        }
        if tab == 2 {
            scheduler_tab(ui, &scheduler);
            kept_core_box(ui, &scheduler);
            return pointer_box(ui);
        }
        if tab == 0 {
            // The power mode is Power's; found from here too.
            let open = ui.on_view(|_v, _: &(), _cx| {
                crate::prefs().open("power");
            });
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().child(StringView::new("The power mode (Power Saver, Balanced or Performance) is set in Power.").wrap(true)))
                    .child(Button::new("Open Power").on_click(move |_, cx| open(&(), cx))),
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
                    // (Its own note, right under it: "this" is the check
                    // box above, and what depends on it comes after.)
                    ui.note("A terminal job that stays busy for a minute or more is lowered too. Turning this off puts back batch scheduling and idle I/O; the nice value stays until the program ends.");
                    // What depends on it, set in under it and greyed with
                    // it off, all of it: at the check box's own edge
                    // they read as settings of their own, and live with
                    // it off as ones that still did something (both
                    // reviewers, 2026-11-20).
                    ui.enabled_if(s.priorities, |ui| {
                        ui.indented(|ui| {
                            let nice = s.background_nice as f32;
                            let on = set(ui, |s, v: f32| s.background_nice = v as i32);
                            let enabled = ui.enabled();
                            ui.add(div().flex().flex_row().gap(px(m::SPACING * 2.0)).child(div().flex_1().child(Slider::new("perf-nice", nice, 1.0..=19.0).label("Lower to nice").value_text(format!("{nice:.0}")).step(1.0).enabled(enabled).on_change(on))).child(div().flex_1()));
                            background.update(ui.cx, |f, cx| f.set_enabled(enabled, cx));
                            ui.add(TextControl::new(ui.cx, background).label("Background:"));
                            // Compilers and linkers in the session's
                            // background group, kept to the memory that
                            // is to be had for nothing
                            // (`olo_system::background`): by the person's
                            // own choice, and only in a session Olo's
                            // login divided (said, where it isn't).
                            let divided = divided();
                            ui.enabled_if(divided, |ui| {
                                let spare = CheckBox::new(olo_kit::words::of_text("Give compilers and linkers only spare memory"), s.background_memory && divided).enabled(ui.enabled()).on_toggle(set(ui, |s, v| s.background_memory = v));
                                ui.add(spare);
                            });
                            if divided {
                                ui.note("Those in the Background list use only spare memory. When memory runs short, they give it up first and slow down.");
                            } else {
                                // The way out, which stays to be read and
                                // pressed whatever the group above is: a
                                // line and Olo Doctor beside it, as the
                                // tab's first row has Power.
                                ui.add(
                                    div()
                                        .mt(px(m::TIGHT - m::STACK))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(m::SPACING))
                                        .child(div().flex_1().min_w_0().child(StringView::new("Unavailable in this session. Olo Doctor sets this up; then log out and back in.").dim(true).wrap(true)))
                                        .child(Button::new("Open Olo Doctor").on_click(|_, _| {
                                            let _ = olo_xdg::open::spawn_app(&["olo-doctor".into()], "olo.doctor");
                                        })),
                                );
                            }
                        });
                    });
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
                        ui.add(div().flex().flex_row().items_center().child(div().flex_1().child(StringView::new(name.clone()))).child(Button::new("Remove").min_width(72.0).on_click(move |_, cx| remove(&(), cx))));
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
                    pair(ui, ("Busy at least", th.spin_cpu, 50.0..=400.0, 5.0, olo_kit::tr!("{}% of a core", format!("{:.0}", th.spin_cpu)), |t, v| t.spin_cpu = v), Some(("For", th.spin_minutes, 0.5..=30.0, 0.5, format!("{:.1} min", th.spin_minutes), |t, v| t.spin_minutes = v)));
                    pair(ui, ("Waits less than", th.spin_switches, 0.0..=50.0, 0.5, olo_kit::tr!("{} a second", format!("{:.1}", th.spin_switches)), |t, v| t.spin_switches = v), Some(("And I/O under", th.spin_io_kb, 0.0..=256.0, 1.0, format!("{:.0} KB/s", th.spin_io_kb), |t, v| t.spin_io_kb = v)));
                });
                let leaks = ui.collect(|ui| {
                    section(ui, "Leaking memory");
                    pair(ui, ("Growing for", th.leak_minutes, 5.0..=240.0, 5.0, format!("{:.0} min", th.leak_minutes), |t, v| t.leak_minutes = v), Some(("At least", th.leak_rate_mb, 0.5..=50.0, 0.5, format!("{:.1} MB/min", th.leak_rate_mb), |t, v| t.leak_rate_mb = v)));
                    pair(ui, ("By at least", th.leak_growth_mb, 50.0..=4000.0, 50.0, format!("{:.0} MB", th.leak_growth_mb), |t, v| t.leak_growth_mb = v), Some(("Steadily (fit)", th.leak_fit, 0.5..=0.99, 0.01, format!("R² {:.2}", th.leak_fit), |t, v| t.leak_fit = v)));
                });
                let memory = ui.collect(|ui| {
                    section(ui, "Using too much memory");
                    pair(ui, ("Holding at least", th.memory_share, 5.0..=90.0, 1.0, olo_kit::tr!("{}% of RAM", format!("{:.0}", th.memory_share)), |t, v| t.memory_share = v), Some(("While less is free than", th.memory_available, 2.0..=50.0, 1.0, format!("{:.0}%", th.memory_available), |t, v| t.memory_available = v)));
                    pair(ui, ("Or stalls over", th.memory_stall, 1.0..=80.0, 1.0, olo_kit::tr!("{}% of the time", format!("{:.0}", th.memory_stall)), |t, v| t.memory_stall = v), None);
                });
                let zombies = ui.collect(|ui| {
                    section(ui, "Leaving zombies");
                    pair(ui, ("Unreaped for", th.zombie_minutes, 0.5..=60.0, 0.5, format!("{:.1} min", th.zombie_minutes), |t, v| t.zombie_minutes = v), Some(("By a parent leaving", th.zombie_least as f32, 1.0..=20.0, 1.0, olo_kit::tr!("{} of them", th.zombie_least), |t, v| t.zombie_least = v as u32)));
                    pair(ui, ("Urgent at", th.zombie_count as f32, 5.0..=500.0, 5.0, olo_kit::tr!("{} of them", th.zombie_count), |t, v| t.zombie_count = v as u32), None);
                });
                let column = |items: Vec<olo_kit::Element>| div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(items);
                for (a, b) in [(stuck, leaks), (memory, zombies)] {
                    ui.add(div().flex().flex_row().gap(px(m::SPACING * 3.0)).child(column(a)).child(column(b)));
                }
                ui.space(m::TIGHT);
                let changed = *th != Thresholds::default();
                let reset = set(ui, |s, _: ()| s.thresholds = Thresholds::default());
                let enabled = ui.enabled();
                ui.add(Button::new("Reset Thresholds").enabled(changed && enabled).on_click(move |_, cx| reset(&(), cx)));
            });
        });
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
            st.kernel.show(is_kernel_tab(*i));
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("performance-tabs", tabs, tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}
