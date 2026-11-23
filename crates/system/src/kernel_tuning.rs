//! The kernel's settings Olo suggests for a computer someone sits at, as
//! anyone may read them: what each is now, what the next start brings,
//! and figures of what it governs. Nothing here changes anything:
//! setting one is root's (Olo Doctor's helper, `olo-doctor --kernel`),
//! Olo Doctor offers each, and Preferences ▸ Performance ▸ Kernel shows
//! and switches them. kernel/TUNING.md has what was measured for each,
//! what else was measured and left as it comes, and how each is kept
//! across a kernel update.
//!
//! - [`Preemption`]: how soon the kernel switches to a program that was
//!   just woken while another is running. Fedora's kernel starts lazy
//!   (it waits for its next tick, up to a millisecond); it can be told
//!   to switch at once ("full"), at its start (`preempt=full`) and, where
//!   it isn't locked down, while it runs.
//! - [`Turns`]: how long a busy program keeps a processor before one
//!   that is waiting gets it (the scheduler's base slice). The kernel
//!   lengthens it with the number of processors (2.8 ms on eight and
//!   more); kept at its base, 0.7 ms, what a window, a key and the
//!   kernel's own workers wait under load is a fraction of what it was.
//! - [`SlowDisks`]: a limit on what may wait in memory to be written to
//!   a removable disk (its `bdi/strict_limit` and `bdi/max_bytes`, set by
//!   a udev rule as each is plugged in), so a copy's progress is true and
//!   ejecting is quick.
//! - [`DirtyWriteback`]: byte caps (`vm.dirty_background_bytes` 256 MiB,
//!   `vm.dirty_bytes` 1 GiB) on machines with large RAM (>= 12 GiB) so
//!   builds stream writes steadily instead of flushing gigabytes at once.
//! - [`NvmeScheduler`]: the `kyber` I/O scheduler on non-rotational NVMe
//!   drives (udev rule + sysfs), keeping read latency bounded during
//!   heavy synchronous writes and discards.
//!
//! Beside them, Olo's own pieces in the kernel, shown in the same place:
//! [`Bbr3`] (TCP's congestion control, a module), [`GraphicsMemory`] (a
//! live patch) and [`GraphicsPages`] (a module with a probe).
//!
//! `OLO_KERNEL_ROOT=DIR` puts all of it under DIR (tests: a made-up
//! computer in a scratch folder, which Doctor's steps then write to as
//! the person).

use std::path::{Path, PathBuf};

/// One of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Preemption,
    ShortTurns,
    SlowDisks,
    DirtyWriteback,
    NvmeScheduler,
    Bbr3,
    GraphicsMemory,
    GraphicsPages,
}

impl Setting {
    pub const ALL: [Setting; 8] =
        [Setting::Preemption, Setting::ShortTurns, Setting::SlowDisks, Setting::DirtyWriteback, Setting::NvmeScheduler, Setting::Bbr3, Setting::GraphicsMemory, Setting::GraphicsPages];

    /// As the command line says it.
    pub fn word(self) -> &'static str {
        match self {
            Setting::Preemption => "preemption",
            Setting::ShortTurns => "short-turns",
            Setting::SlowDisks => "slow-disks",
            Setting::DirtyWriteback => "dirty-writeback",
            Setting::NvmeScheduler => "nvme-scheduler",
            Setting::Bbr3 => "bbr3",
            Setting::GraphicsMemory => "graphics-memory",
            Setting::GraphicsPages => "graphics-pages",
        }
    }

    pub fn parse(word: &str) -> Option<Setting> {
        Setting::ALL.into_iter().find(|s| s.word() == word.trim())
    }
}

/// The start argument that asks for full preemption, and its key.
pub const PREEMPT_ARG: &str = "preempt=full";
pub const PREEMPT_KEY: &str = "preempt";

/// What may wait in memory for a removable disk: 16 MB, about a second
/// of a USB stick's writing.
pub const SLOW_DISK_BYTES: u64 = 16 << 20;

/// Byte caps on unwritten dirty pages for large-RAM machines: background
/// flushing begins at 256 MiB, and writers throttle at 1 GiB.
pub const DIRTY_BACKGROUND_BYTES: u64 = 256 << 20;
pub const DIRTY_BYTES: u64 = 1 << 30;
/// Minimum total RAM before byte caps are offered (below 12 GiB, Linux's
/// default 20% dirty_ratio can already be close to or below 1 GiB).
pub const DIRTY_MIN_RAM_BYTES: u64 = 12 << 30;
/// Linux's default percentage thresholds, restored when turned off.
pub const DIRTY_DEFAULT_BACKGROUND_RATIO: u64 = 10;
pub const DIRTY_DEFAULT_RATIO: u64 = 20;
pub const DIRTY_WRITEBACK_CONF: &str = "# Olo: cap unwritten data in memory so writes stream to disk smoothly\n# instead of flushing gigabytes at once (Preferences ▸ Performance ▸ Disks;\n# kernel/TUNING.md).\nvm.dirty_background_bytes = 268435456\nvm.dirty_bytes = 1073741824\n";

/// I/O scheduler Olo suggests for non-rotational NVMe drives.
pub const NVME_SCHEDULER: &str = "kyber";
/// The udev rule that selects kyber for each NVMe drive at start and hot-plug.
pub const NVME_SCHEDULER_RULE: &str = include_str!("../../../data/udev/62-olo-nvme-scheduler.rules");

/// A short turn: the scheduler's base slice before the kernel lengthens
/// it for the number of processors (`sysctl_sched_base_slice`'s 0.7 ms,
/// times one more than the doublings of processors up to eight: 2.8 ms
/// on eight and more), in nanoseconds.
pub const SHORT_TURN_NS: u64 = 700_000;

/// The unit that sets short turns at every start, and what it applies:
/// the scheduler's file is in debugfs, which is mounted beside the
/// start's other steps and not before them, so its line is not among
/// /etc/tmpfiles.d's (they may run first) but a file of its own, applied
/// by systemd's own tool once debugfs is there. The second line is Olo's
/// note of it, which anyone can read (the kernel tells only root).
pub const SHORT_TURNS_SERVICE: &str = "olo-short-turns.service";
pub const SHORT_TURNS_UNIT: &str = "[Unit]\nDescription=Olo: short turns on the processors (the scheduler's base slice)\nAfter=sys-kernel-debug.mount\nWants=sys-kernel-debug.mount\nConditionPathExists=/etc/olo/short-turns.conf\n\n[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart=/usr/bin/systemd-tmpfiles --create /etc/olo/short-turns.conf\n\n[Install]\nWantedBy=multi-user.target\n";
pub const SHORT_TURNS_LINES: &str = "# Olo: short turns on the processors (Preferences ▸ Performance ▸ Kernel; kernel/TUNING.md).\nw /sys/kernel/debug/sched/base_slice_ns - - - - 700000\nf+ /run/olo-kernel/turns 0644 root root - 700000\n";

/// The kernel series Olo's BBRv3 module has passed its self-check on
/// (kernel/bbr3/CHECKED: "7.2", "7.3").
const BBR3_CHECKED: &str = include_str!("../../../kernel/bbr3/CHECKED");

/// A kernel release's series ("7.2.15-300.fc45.x86_64" → "7.2").
pub fn series(release: &str) -> String {
    release.split(['.', '-']).take(2).collect::<Vec<_>>().join(".")
}

/// Whether Olo's BBRv3 module was checked on `series`.
pub fn bbr3_checked_on(series: &str) -> bool {
    BBR3_CHECKED.lines().map(str::trim).any(|l| !l.starts_with('#') && l == series)
}

/// The rule that limits each removable disk as it comes.
pub const SLOW_DISK_RULE: &str = include_str!("../../../data/udev/61-olo-slow-disks.rules");

/// Where the settings are: the system's own places, or a test's tree
/// (`OLO_KERNEL_ROOT`).
#[derive(Debug, Clone, PartialEq)]
pub struct Places {
    root: PathBuf,
    stand_in: bool,
}

impl Places {
    /// The system's, or `OLO_KERNEL_ROOT`'s.
    pub fn here() -> Places {
        match std::env::var_os("OLO_KERNEL_ROOT").filter(|r| !r.is_empty()) {
            Some(root) => Places::under(Path::new(&root)),
            None => Places { root: "/".into(), stand_in: false },
        }
    }

    /// A tree of the caller's: a test's, or "/" itself (Olo Doctor's
    /// checks, whose roots are either).
    pub fn under(root: &Path) -> Places {
        Places { root: root.to_owned(), stand_in: root != Path::new("/") }
    }

    /// A made-up tree: nothing of the real system's is touched or asked.
    pub fn stand_in(&self) -> bool {
        self.stand_in
    }

    pub fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// The running kernel's switch of its preemption (root's to read and
    /// write; refused where the kernel is locked down).
    pub fn preempt_switch(&self) -> PathBuf {
        self.at("/sys/kernel/debug/sched/preempt")
    }

    /// Olo's note of a switch made since the start (the kernel tells no
    /// one but root what it's on): gone at the next start, as the switch
    /// is.
    pub fn preempt_note(&self) -> PathBuf {
        self.at("/run/olo-kernel/preemption")
    }

    /// The scheduler's base slice (root's to read and write, as the
    /// preemption switch is).
    pub fn turn_switch(&self) -> PathBuf {
        self.at("/sys/kernel/debug/sched/base_slice_ns")
    }

    /// Olo's note of what the turn was set to since the start, in
    /// nanoseconds (anyone's to read).
    pub fn turn_note(&self) -> PathBuf {
        self.at("/run/olo-kernel/turns")
    }

    /// What `SHORT_TURNS_SERVICE` applies at every start.
    pub fn short_turns_lines(&self) -> PathBuf {
        self.at("/etc/olo/short-turns.conf")
    }

    pub fn short_turns_unit(&self) -> PathBuf {
        self.at("/etc/systemd/system").join(SHORT_TURNS_SERVICE)
    }

    pub fn short_turns_wanted(&self) -> PathBuf {
        self.at("/etc/systemd/system/multi-user.target.wants").join(SHORT_TURNS_SERVICE)
    }

    pub fn slow_disk_rule(&self) -> PathBuf {
        self.at("/etc/udev/rules.d/61-olo-slow-disks.rules")
    }

    /// A disk's limit (`name`: "sdb").
    pub fn strict_limit(&self, name: &str) -> PathBuf {
        self.at("/sys/block").join(name).join("bdi/strict_limit")
    }

    pub fn max_bytes(&self, name: &str) -> PathBuf {
        self.at("/sys/block").join(name).join("bdi/max_bytes")
    }

    /// A disk's limit as a share of the kernel's whole (100: none).
    pub fn max_ratio(&self, name: &str) -> PathBuf {
        self.at("/sys/block").join(name).join("bdi/max_ratio")
    }

    pub fn dirty_writeback_conf(&self) -> PathBuf {
        self.at("/etc/sysctl.d/85-olo-dirty-writeback.conf")
    }

    pub fn vm_dirty_background_bytes(&self) -> PathBuf {
        self.at("/proc/sys/vm/dirty_background_bytes")
    }

    pub fn vm_dirty_bytes(&self) -> PathBuf {
        self.at("/proc/sys/vm/dirty_bytes")
    }

    pub fn vm_dirty_background_ratio(&self) -> PathBuf {
        self.at("/proc/sys/vm/dirty_background_ratio")
    }

    pub fn vm_dirty_ratio(&self) -> PathBuf {
        self.at("/proc/sys/vm/dirty_ratio")
    }

    pub fn nvme_scheduler_rule(&self) -> PathBuf {
        self.at("/etc/udev/rules.d/62-olo-nvme-scheduler.rules")
    }

    /// A block device's I/O scheduler choice (`name`: "nvme0n1").
    pub fn block_scheduler(&self, name: &str) -> PathBuf {
        self.at("/sys/block").join(name).join("queue/scheduler")
    }
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|t| t.trim().to_owned())
}

/// `key`'s value in a kernel command line ("preempt=full" → "full").
fn argument(line: &str, key: &str) -> Option<String> {
    line.split_whitespace().rev().find_map(|a| a.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim_matches('"').to_owned())
}

/// The command line the next start gets, where it's kept for anyone to
/// read, and the file it's in: /etc/kernel/cmdline (what new kernels'
/// entries get), else GRUB's defaults.
fn kept_command_line(places: &Places) -> Option<(String, PathBuf)> {
    let file = places.at("/etc/kernel/cmdline");
    if let Some(line) = read(&file) {
        return Some((line, file));
    }
    let file = places.at("/etc/default/grub");
    let grub = read(&file)?;
    grub.lines().find_map(|l| l.strip_prefix("GRUB_CMDLINE_LINUX=\"")?.strip_suffix('"')).map(|line| (line.to_owned(), file.clone()))
}

/// When the computer started, in seconds since 1970 (/proc/stat's
/// `btime`).
fn started_at(places: &Places) -> Option<u64> {
    read(&places.at("/proc/stat"))?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())
}

/// The kernel's preemption.
#[derive(Debug, Clone, PartialEq)]
pub struct Preemption {
    /// What the kernel starts on when nothing says ("lazy" on Fedora's).
    pub built: String,
    /// `preempt=` as it was started with.
    pub started: Option<String>,
    /// `preempt=` as the next start has it.
    pub kept: Option<String>,
    /// What it was switched to since the start (Olo's note).
    pub switched: Option<String>,
    /// The running kernel can be switched (it isn't locked down).
    pub live: bool,
    /// Full was kept for every start before this start, and yet this
    /// one was made without it: its boot entry doesn't have it (an entry
    /// made some other way than from the kept command line, as a kernel
    /// an update brought can be where there is no /etc/kernel/cmdline).
    /// Putting it on again puts it in every entry.
    pub lost: bool,
}

impl Preemption {
    /// What the kernel is on now.
    pub fn now(&self) -> &str {
        self.switched.as_deref().or(self.started.as_deref()).unwrap_or(&self.built)
    }

    /// What the next start brings.
    pub fn next(&self) -> &str {
        self.kept.as_deref().unwrap_or(&self.built)
    }

    /// Full, now and from every start.
    pub fn on(&self) -> bool {
        self.now() == "full" && self.next() == "full"
    }

    /// Someone chose another with a start argument of their own: theirs
    /// to keep (nothing is suggested).
    pub fn chosen_otherwise(&self) -> bool {
        self.kept.as_deref().is_some_and(|k| k != "full")
    }
}

/// What a kernel built with these options starts on, where it can be
/// told otherwise (CONFIG_PREEMPT_DYNAMIC); None where it can't.
fn built_preemption(config: &str) -> Option<String> {
    let has = |name: &str| config.lines().any(|l| l.strip_prefix(name).is_some_and(|rest| rest == "=y"));
    if !has("CONFIG_PREEMPT_DYNAMIC") {
        return None;
    }
    [("CONFIG_PREEMPT_LAZY", "lazy"), ("CONFIG_PREEMPT_VOLUNTARY", "voluntary"), ("CONFIG_PREEMPT_NONE", "none"), ("CONFIG_PREEMPT", "full")]
        .into_iter()
        .find(|(name, _)| has(name))
        .map(|(_, word)| word.to_owned())
}

/// The running kernel's build options say the same for as long as it
/// runs: read once.
fn built(places: &Places) -> Option<String> {
    let find = || built_preemption(&build_options(places)?);
    if places.stand_in {
        return find();
    }
    static BUILT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BUILT.get_or_init(find).clone()
}

/// None where this kernel has one way only.
pub fn preemption(places: &Places) -> Option<Preemption> {
    let built = built(places)?;
    let started = read(&places.at("/proc/cmdline")).and_then(|l| argument(&l, PREEMPT_KEY));
    // (Where the next start's command line is kept nowhere anyone can
    // read, it's taken to be as this start's was.)
    let (kept, kept_at) = match kept_command_line(places) {
        Some((line, file)) => (argument(&line, PREEMPT_KEY), std::fs::metadata(file).and_then(|m| m.modified()).ok()),
        None => (started.clone(), None),
    };
    let kept_at = kept_at.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    let lost = kept.as_deref() == Some("full") && started.as_deref() != Some("full") && matches!((kept_at, started_at(places)), (Some(kept), Some(start)) if kept < start);
    let switched = read(&places.preempt_note()).filter(|w| !w.is_empty());
    // (Locked down, the kernel takes no writes through debugfs: Secure
    // Boot's doing. Without the file there is no lockdown at all.)
    let live = read(&places.at("/sys/kernel/security/lockdown")).is_none_or(|l| l.contains("[none]"));
    Some(Preemption { built, started, kept, switched, live, lost })
}

/// The kernel's build options for the running kernel, where they're
/// kept beside its modules or in /boot.
fn build_options(places: &Places) -> Option<String> {
    let release = read(&places.at("/proc/sys/kernel/osrelease"))?;
    read(&places.at("/lib/modules").join(&release).join("config")).or_else(|| read(&places.at("/boot").join(format!("config-{release}"))))
}

/// How many processors a list of them names ("0-15", "0-3,8-11").
fn processors(list: &str) -> u32 {
    list.trim()
        .split(',')
        .filter_map(|part| {
            let (a, b) = part.split_once('-').unwrap_or((part, part));
            Some(b.trim().parse::<u32>().ok()?.checked_sub(a.trim().parse::<u32>().ok()?)? + 1)
        })
        .sum()
}

/// How long a busy program keeps a processor before a waiting one gets
/// it: the scheduler's base slice.
#[derive(Debug, Clone, PartialEq)]
pub struct Turns {
    /// What the kernel starts on here, in nanoseconds: 0.7 ms, times
    /// one more than the doublings of its processors up to eight.
    pub built_ns: u64,
    /// What it was set to since the start (Olo's note), in nanoseconds.
    pub switched_ns: Option<u64>,
    /// Set at every start (Olo's unit, chosen, and its lines).
    pub kept: bool,
    /// The running kernel can be set (it isn't locked down).
    pub live: bool,
}

impl Turns {
    pub fn now_ns(&self) -> u64 {
        self.switched_ns.unwrap_or(self.built_ns)
    }

    /// Short, now and from every start.
    pub fn on(&self) -> bool {
        self.kept && self.now_ns() <= SHORT_TURN_NS
    }
}

/// None where there is nothing to shorten: one processor (the kernel's
/// own turn is the short one), or no processors to count.
pub fn turns(places: &Places) -> Option<Turns> {
    let count = processors(&read(&places.at("/sys/devices/system/cpu/online"))?);
    // (The kernel's own sum: `1 + ilog2(min(processors, 8))` times its
    // base, `SCHED_TUNABLESCALING_LOG`.)
    let built_ns = SHORT_TURN_NS * (1 + count.clamp(1, 8).ilog2() as u64);
    if built_ns <= SHORT_TURN_NS {
        return None;
    }
    let switched_ns = read(&places.turn_note()).and_then(|t| t.parse().ok());
    let kept = std::fs::read_to_string(places.short_turns_lines()).is_ok_and(|t| t == SHORT_TURNS_LINES)
        && places.short_turns_unit().is_file()
        && std::fs::symlink_metadata(places.short_turns_wanted()).is_ok();
    let live = read(&places.at("/sys/kernel/security/lockdown")).is_none_or(|l| l.contains("[none]"));
    Some(Turns { built_ns, switched_ns, kept, live })
}

/// A line's number in a file of "name number" lines (/proc/vmstat).
fn stat(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(' ')?.trim().parse().ok())
}

/// A removable disk that's here.
#[derive(Debug, Clone, PartialEq)]
pub struct SlowDisk {
    /// The kernel's name ("sdb").
    pub name: String,
    /// What it calls itself ("Kingston DataTraveler"), where it does.
    pub model: String,
    /// What may wait in memory for it, in bytes; None: as much as for
    /// any disk.
    pub limit: Option<u64>,
}

/// What may wait in memory for a removable disk.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SlowDisks {
    /// This kernel can hold a disk to a limit of its own (Linux 6.2 and
    /// later: `bdi/strict_limit`).
    pub can: bool,
    /// Each is limited as it's plugged in (Olo's rule is there).
    pub kept: bool,
    /// The removable disks here now.
    pub disks: Vec<SlowDisk>,
    /// What may wait for all disks together, the kernel's own limit, in
    /// bytes: what one slow disk may fill without a limit of its own.
    pub may_wait: u64,
    /// What waits now, in bytes.
    pub waiting: u64,
}

impl SlowDisks {
    /// On: the rule is there, and every removable disk here is limited.
    pub fn on(&self) -> bool {
        self.kept && self.disks.iter().all(|d| d.limit.is_some())
    }
}

/// Whether `disk` (a folder of /sys/block) is one the rule limits: its
/// media can be taken out (a card reader, a stick), it's on USB, or it's
/// an SD card; but not one on a USB link of 10 Gbit/s or faster, which
/// the limit can hold back. As data/udev/61-olo-slow-disks.rules has it.
fn removable(disk: &Path) -> bool {
    // (/sys/block/sdb is a link into the device tree: a USB disk's path
    // goes through its USB device, which says its link's speed in
    // Mbit/s.)
    let real = std::fs::canonicalize(disk).unwrap_or_default();
    let usb = real.ancestors().find(|d| d.join("speed").is_file() && d.join("idVendor").is_file());
    if usb.and_then(|d| read(&d.join("speed"))).and_then(|s| s.parse::<f32>().ok()).is_some_and(|mbit| mbit >= 10000.0) {
        return false;
    }
    if usb.is_some() || read(&disk.join("removable")).as_deref() == Some("1") {
        return true;
    }
    disk.file_name().is_some_and(|n| n.to_string_lossy().starts_with("mmcblk")) && read(&disk.join("device/type")).as_deref() == Some("SD")
}

pub fn slow_disks(places: &Places) -> SlowDisks {
    let kept = std::fs::read_to_string(places.slow_disk_rule()).is_ok_and(|t| t == SLOW_DISK_RULE);
    let (mut disks, mut can) = (Vec::new(), false);
    for e in std::fs::read_dir(places.at("/sys/block")).into_iter().flatten().flatten() {
        let (dir, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
        can |= places.strict_limit(&name).exists();
        // (A reader with no card in it has no size: nothing to write to.)
        if !removable(&dir) || read(&dir.join("size")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) == 0 {
            continue;
        }
        let model = [read(&dir.join("device/vendor")), read(&dir.join("device/model")).or_else(|| read(&dir.join("device/name")))]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let strict = read(&places.strict_limit(&name)).as_deref() == Some("1");
        let limit = read(&places.max_bytes(&name)).and_then(|b| b.parse().ok()).filter(|_| strict);
        disks.push(SlowDisk { name, model, limit });
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    let vmstat = read(&places.at("/proc/vmstat")).unwrap_or_default();
    let pages = |name: &str| stat(&vmstat, name).unwrap_or(0) * 4096;
    SlowDisks { can, kept, disks, may_wait: pages("nr_dirty_threshold"), waiting: pages("nr_dirty") + pages("nr_writeback") }
}

/// Byte caps on unwritten dirty pages in memory (`vm.dirty_background_bytes`,
/// `vm.dirty_bytes`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DirtyWriteback {
    /// Total RAM on this machine, in bytes (`/proc/meminfo`'s `MemTotal`).
    pub ram_bytes: u64,
    /// Current sysctl settings (setting `*_bytes` zeroes `*_ratio` and vice versa).
    pub bg_bytes: u64,
    pub dirty_bytes: u64,
    pub bg_ratio: u64,
    pub dirty_ratio: u64,
    /// Effective thresholds now from `/proc/vmstat`, in bytes.
    pub bg_now_bytes: u64,
    pub limit_now_bytes: u64,
    /// Currently waiting to be written (`nr_dirty + nr_writeback`), in bytes.
    pub waiting_bytes: u64,
    /// Olo's sysctl file (`/etc/sysctl.d/85-olo-dirty-writeback.conf`) is in place.
    pub kept: bool,
}

impl DirtyWriteback {
    /// Capped at Olo's byte limits, now and at every start.
    pub fn on(&self) -> bool {
        self.kept && self.bg_bytes == DIRTY_BACKGROUND_BYTES && self.dirty_bytes == DIRTY_BYTES
    }

    /// Someone already set custom dirty limits of their own without Olo's file:
    /// theirs to keep (Doctor doesn't nag).
    pub fn chosen_otherwise(&self) -> bool {
        !self.kept && (self.bg_bytes > 0 || self.dirty_bytes > 0 || self.bg_ratio != DIRTY_DEFAULT_BACKGROUND_RATIO || self.dirty_ratio != DIRTY_DEFAULT_RATIO)
    }
}

/// Total RAM from `/proc/meminfo` (`MemTotal: N kB`), in bytes.
fn mem_total(places: &Places) -> Option<u64> {
    let text = read(&places.at("/proc/meminfo"))?;
    let kb: u64 = text.lines().find_map(|l| l.strip_prefix("MemTotal:")?.split_whitespace().next()?.parse().ok())?;
    Some(kb.saturating_mul(1024))
}

/// None on machines whose `/proc/sys/vm/dirty_bytes` isn't there, or with
/// less than `DIRTY_MIN_RAM_BYTES` of RAM (unless Olo's file is already kept,
/// so it can still be turned off).
pub fn dirty_writeback(places: &Places) -> Option<DirtyWriteback> {
    let dirty_bytes: u64 = read(&places.vm_dirty_bytes())?.parse().ok()?;
    let bg_bytes: u64 = read(&places.vm_dirty_background_bytes())?.parse().ok()?;
    let dirty_ratio: u64 = read(&places.vm_dirty_ratio()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let bg_ratio: u64 = read(&places.vm_dirty_background_ratio()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let kept = std::fs::read_to_string(places.dirty_writeback_conf()).is_ok_and(|t| t == DIRTY_WRITEBACK_CONF);
    let ram_bytes = mem_total(places).unwrap_or(0);
    if !kept && ram_bytes < DIRTY_MIN_RAM_BYTES {
        return None;
    }
    let vmstat = read(&places.at("/proc/vmstat")).unwrap_or_default();
    let pages = |name: &str| stat(&vmstat, name).unwrap_or(0) * 4096;
    let bg_now_bytes = match pages("nr_dirty_background_threshold") {
        0 if bg_bytes > 0 => bg_bytes,
        0 => ram_bytes.saturating_mul(bg_ratio) / 100,
        n => n,
    };
    let limit_now_bytes = match pages("nr_dirty_threshold") {
        0 if dirty_bytes > 0 => dirty_bytes,
        0 => ram_bytes.saturating_mul(dirty_ratio) / 100,
        n => n,
    };
    Some(DirtyWriteback { ram_bytes, bg_bytes, dirty_bytes, bg_ratio, dirty_ratio, bg_now_bytes, limit_now_bytes, waiting_bytes: pages("nr_dirty") + pages("nr_writeback"), kept })
}

/// An NVMe drive seen in `/sys/block`.
#[derive(Debug, Clone, PartialEq)]
pub struct NvmeDisk {
    /// The kernel's name ("nvme0n1").
    pub name: String,
    /// What it calls itself ("SKHynix HFS002TFM9X179N"), where it does.
    pub model: String,
    /// Active scheduler ("none", "kyber", "mq-deadline", "bfq").
    pub scheduler: String,
    /// Whether `kyber` is available in `queue/scheduler`.
    pub has_kyber: bool,
}

/// The I/O scheduler on non-rotational NVMe drives (`kyber` vs `none`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NvmeScheduler {
    /// Olo's udev rule (`/etc/udev/rules.d/62-olo-nvme-scheduler.rules`) is in place.
    pub kept: bool,
    /// Non-rotational NVMe drives here.
    pub disks: Vec<NvmeDisk>,
}

impl NvmeScheduler {
    /// At least one NVMe drive here supports `kyber`.
    pub fn can(&self) -> bool {
        self.disks.iter().any(|d| d.has_kyber)
    }

    /// On: the rule is there and every kyber-capable NVMe drive here is on `kyber`.
    pub fn on(&self) -> bool {
        self.kept && self.can() && self.disks.iter().filter(|d| d.has_kyber).all(|d| d.scheduler == NVME_SCHEDULER)
    }

    /// Someone already chose a scheduler other than `none` without Olo's rule.
    pub fn chosen_otherwise(&self) -> bool {
        !self.kept && self.disks.iter().any(|d| !d.scheduler.is_empty() && d.scheduler != "none")
    }
}

/// Parse `queue/scheduler` (e.g. `"[none] mq-deadline kyber bfq"`) into
/// `(active, available)`.
fn parse_scheduler(text: &str) -> (String, Vec<String>) {
    let mut active = String::new();
    let mut avail = Vec::new();
    for word in text.split_whitespace() {
        if let Some(chosen) = word.strip_prefix('[').and_then(|w| w.strip_suffix(']')) {
            active = chosen.to_owned();
            avail.push(chosen.to_owned());
        } else {
            avail.push(word.to_owned());
        }
    }
    (active, avail)
}

fn is_nvme_disk_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("nvme") else { return false };
    let Some((ctrl, ns)) = rest.split_once('n') else { return false };
    !ctrl.is_empty() && ctrl.bytes().all(|b| b.is_ascii_digit()) && !ns.is_empty() && ns.bytes().all(|b| b.is_ascii_digit())
}

/// None where there are no NVMe drives exposing `queue/scheduler` and no
/// Olo rule is installed.
pub fn nvme_scheduler(places: &Places) -> Option<NvmeScheduler> {
    let kept = std::fs::read_to_string(places.nvme_scheduler_rule()).is_ok_and(|t| t == NVME_SCHEDULER_RULE);
    let mut disks = Vec::new();
    for e in std::fs::read_dir(places.at("/sys/block")).into_iter().flatten().flatten() {
        let (dir, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
        if !is_nvme_disk_name(&name) {
            continue;
        }
        if read(&dir.join("size")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) == 0 {
            continue;
        }
        if read(&dir.join("queue/rotational")).as_deref() == Some("1") {
            continue;
        }
        let Some(sched_text) = read(&places.block_scheduler(&name)) else { continue };
        let (scheduler, avail) = parse_scheduler(&sched_text);
        if scheduler.is_empty() {
            continue;
        }
        let has_kyber = avail.iter().any(|a| a == NVME_SCHEDULER);
        let model = read(&dir.join("device/model")).unwrap_or_default().replace('_', " ");
        disks.push(NvmeDisk { name, model, scheduler, has_kyber });
    }
    disks.sort_by(|a, b| a.name.cmp(&b.name));
    if disks.is_empty() && !kept {
        return None;
    }
    Some(NvmeScheduler { kept, disks })
}

/// The connections that are on BBRv3 now, of all TCP's, and what they
/// have to show for it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OnIt {
    /// Connections on it, and connections in all (listeners aside).
    pub connections: u32,
    pub of: u32,
    /// Their round trips' middle, in milliseconds (None: none measured).
    pub round_trip_ms: Option<f32>,
    /// Segments of data they have sent, and segments sent again.
    pub sent: u64,
    pub sent_again: u64,
}

/// `flows`' connections on `control`, summed.
pub fn on_it(flows: &[crate::netdiag::Flow], control: &str) -> OnIt {
    // (A listener and a closed socket send nothing: 10 and 7.)
    let live = |f: &&crate::netdiag::Flow| f.state != 10 && f.state != 7;
    let mine: Vec<&crate::netdiag::Flow> = flows.iter().filter(live).filter(|f| f.control == control).collect();
    let mut trips: Vec<u32> = mine.iter().map(|f| f.rtt_us).filter(|r| *r > 0).collect();
    trips.sort_unstable();
    OnIt {
        connections: mine.len() as u32,
        of: flows.iter().filter(live).count() as u32,
        round_trip_ms: trips.get(trips.len() / 2).map(|us| *us as f32 / 1000.0),
        sent: mine.iter().map(|f| f.sent as u64).sum(),
        sent_again: mine.iter().map(|f| f.sent_again as u64).sum(),
    }
}

/// BBRv3 for TCP (Olo's module `tcp_bbr3`, or the kernel's own bbr where
/// that is BBRv3; apps/bbr3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bbr3 {
    /// What new connections use ("bbr3", "cubic").
    pub control: String,
    /// Olo's module is in the kernel.
    pub loaded: bool,
    /// The service that sees to it at every start is there and chosen.
    pub kept: bool,
    /// How the service's last start went ("bbr3", "bbr", "failed: why").
    pub said: Option<String>,
    /// The connections on it now (None: the kernel wouldn't list them).
    pub on_it: Option<OnIt>,
    /// The running kernel's series ("7.2"), and whether Olo's module
    /// passed its self-check on it. The module is built for whatever
    /// kernel an update brings; a series it wasn't checked on yet is
    /// said, where it's in use.
    pub series: String,
    pub checked: bool,
}

impl Bbr3 {
    /// New connections use BBRv3, now and from every start.
    pub fn on(&self) -> bool {
        self.kept && (self.control == "bbr3" || self.said.as_deref() == Some("bbr") && self.control == "bbr")
    }
}

/// A made-up computer's connections (a test's tree has no kernel to
/// ask): lines of "control state rtt_us sent sent_again" in
/// `proc/net/olo-flows`.
fn stand_in_flows(places: &Places) -> Option<Vec<crate::netdiag::Flow>> {
    let text = read(&places.at("/proc/net/olo-flows"))?;
    Some(
        text.lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                Some(crate::netdiag::Flow {
                    control: f.next()?.to_owned(),
                    state: f.next()?.parse().ok()?,
                    rtt_us: f.next()?.parse().ok()?,
                    sent: f.next()?.parse().ok()?,
                    sent_again: f.next()?.parse().ok()?,
                })
            })
            .collect(),
    )
}

pub fn bbr3(places: &Places) -> Option<Bbr3> {
    bbr3_with(places, None)
}

/// [`bbr3`], with the connections on it as read `before` (Some) where
/// they needn't be read again: listing them has the kernel walk its
/// whole table of connections (4 ms of its time on a computer of 64 GB),
/// and they change slowly.
fn bbr3_with(places: &Places, before: Option<Option<OnIt>>) -> Option<Bbr3> {
    let control = read(&places.at("/proc/sys/net/ipv4/tcp_congestion_control"))?;
    let loaded = places.at("/sys/module/tcp_bbr3").is_dir();
    let kept = places.at("/etc/systemd/system/olo-bbr3.service").is_file() && std::fs::symlink_metadata(places.at("/etc/systemd/system/multi-user.target.wants/olo-bbr3.service")).is_ok();
    let said = read(&places.at("/run/olo-bbr3/status")).filter(|s| !s.is_empty());
    // (BBRv3 is Olo's "bbr3", or the kernel's own "bbr" where the
    // service found that to be it.)
    let name = if said.as_deref() == Some("bbr") { "bbr" } else { "bbr3" };
    let on_it = match before {
        Some(on_it) => on_it,
        None => {
            let flows = if places.stand_in { stand_in_flows(places) } else { crate::netdiag::flows().ok() };
            flows.map(|f| on_it(&f, name))
        }
    };
    let series = series(&read(&places.at("/proc/sys/kernel/osrelease")).unwrap_or_default());
    let checked = bbr3_checked_on(&series);
    Some(Bbr3 { control, loaded, kept, said, on_it, series, checked })
}

/// Windows' graphics memory kept out of swap (Olo's live patch
/// `olo_gpu_memory`, for Intel's xe driver; apps/gpu-memory).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphicsMemory {
    /// The patch is in the running kernel.
    pub patched: bool,
    /// The service that sees to it at every start is there and chosen.
    pub kept: bool,
    /// How the service's last start went.
    pub said: Option<String>,
}

impl GraphicsMemory {
    pub fn on(&self) -> bool {
        self.patched && self.kept
    }
}

/// None where there is nothing for it to patch (no xe driver), unless
/// it's there all the same.
pub fn graphics_memory(places: &Places) -> Option<GraphicsMemory> {
    let patched = read(&places.at("/sys/kernel/livepatch/olo_gpu_memory/enabled")).as_deref() == Some("1");
    let kept = places.at("/etc/systemd/system/olo-gpu-memory.service").is_file() && std::fs::symlink_metadata(places.at("/etc/systemd/system/multi-user.target.wants/olo-gpu-memory.service")).is_ok();
    if !patched && !kept && !places.at("/sys/module/xe").is_dir() {
        return None;
    }
    let said = read(&places.at("/run/olo-gpu-memory/status")).filter(|s| !s.is_empty());
    Some(GraphicsMemory { patched, kept, said })
}

/// The kernel builds Olo's module `olo_gpu_pages` was checked on, and
/// is loaded into (kernel/gpu-pages/QUALIFIED: `olo_kmod::builds`).
pub const GRAPHICS_PAGES_QUALIFIED: &str = include_str!("../../../kernel/gpu-pages/QUALIFIED");

/// Whether the module is offered where it isn't in already (Olo
/// Doctor's suggestion, the Kernel tab's switch). Not yet: what it
/// does in the kernel was shown in guests and reviewed twice with
/// Astra (2026-11-21), and both reviews left one thing to be measured
/// before it is anyone's to switch on: what it does to frames and to
/// the GPU's work on a real computer with Intel's graphics, by turns
/// on and off, with its owner's word (`tools/dev/gpu-pages-try`;
/// kernel/TUNING.md, "Graphics memory in 2 MB pieces", says what was
/// measured and what is still owed). A made-up computer always has it
/// (`OLO_KERNEL_ROOT`: tests, pictures); a real one never before then,
/// whatever the environment says (a switch there was, and the third
/// review had it out: an offer held back is held back for everyone).
pub const GRAPHICS_PAGES_OFFERED: bool = false;

/// Whether the module is offered to a reader of `places`.
pub fn graphics_pages_offered(places: &Places) -> bool {
    GRAPHICS_PAGES_OFFERED || places.stand_in
}

/// No program held for a 2 MB piece of graphics memory (Olo's module
/// `olo_gpu_pages`, for Intel's xe driver; apps/gpu-pages;
/// kernel/TUNING.md, "Graphics memory in 2 MB pieces").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphicsPages {
    /// The module is in the running kernel, switched on or not (a
    /// trial's between its phases is in, and off).
    pub loaded: bool,
    /// It may be chosen here (`graphics_pages_offered`): what is shown
    /// of it is not what may be done with it.
    pub offered: bool,
    /// The module is in the running kernel and switched on.
    pub in_force: bool,
    /// The service that sees to it at every start is there and chosen.
    pub kept: bool,
    /// The running kernel's build is one the module was checked on: it
    /// is loaded into no other.
    pub checked: bool,
    /// The tries for a 2 MB piece it has had made without waiting since
    /// it loaded (the module's own count).
    pub changed: Option<u64>,
    /// How the service's last start went: "in force", "not checked"
    /// (this kernel's build), "not needed", or "failed: " and why.
    pub said: Option<String>,
    /// The running kernel, as it names itself ("7.3.4-300.fc45.x86_64").
    pub kernel: String,
}

impl GraphicsPages {
    pub fn on(&self) -> bool {
        self.in_force && self.kept
    }

    /// Why the module is out of this kernel though its service tried, in
    /// its own words where it said (a kernel whose pool isn't the one it
    /// was written for), else the service's.
    pub fn kept_out(&self) -> Option<&str> {
        let why = self.said.as_deref().filter(|_| !self.in_force)?.strip_prefix("failed: ")?;
        // (What the fix mends, a build that lacked the kernel's headers
        // or make, isn't the module's doing.)
        if why.contains("headers aren't installed") || why.contains("make couldn't be run") {
            return None;
        }
        Some(why.split_once("its own words: ").map_or(why, |(_, words)| words).trim())
    }
}

/// None where there is nothing for it to change (no xe driver), and
/// where it isn't offered and nothing of it is here.
pub fn graphics_pages(places: &Places) -> Option<GraphicsPages> {
    graphics_pages_where(places, graphics_pages_offered(places))
}

/// [`graphics_pages`], offered or not.
pub fn graphics_pages_where(places: &Places, offered: bool) -> Option<GraphicsPages> {
    let parameters = places.at("/sys/module/olo_gpu_pages/parameters");
    let in_force = read(&parameters.join("on")).as_deref() == Some("Y");
    let kept = places.at("/etc/systemd/system/olo-gpu-pages.service").is_file() && std::fs::symlink_metadata(places.at("/etc/systemd/system/multi-user.target.wants/olo-gpu-pages.service")).is_ok();
    let loaded = parameters.is_dir();
    let here = loaded || places.at("/etc/systemd/system/olo-gpu-pages.service").is_file();
    if !here && !(offered && places.at("/sys/module/xe").is_dir()) {
        return None;
    }
    let release = read(&places.at("/proc/sys/kernel/osrelease")).unwrap_or_default();
    let checked = olo_kmod::builds::running(&places.at("/"), "ttm").is_some_and(|b| olo_kmod::builds::listed(GRAPHICS_PAGES_QUALIFIED, &release, &b));
    let changed = read(&parameters.join("changed")).and_then(|c| c.parse().ok());
    let said = read(&places.at("/run/olo-gpu-pages/status")).filter(|s| !s.is_empty());
    Some(GraphicsPages { loaded, offered, in_force, kept, checked, changed, said, kernel: release })
}

/// All of it, as it is now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub preemption: Option<Preemption>,
    pub turns: Option<Turns>,
    pub slow_disks: SlowDisks,
    pub dirty_writeback: Option<DirtyWriteback>,
    pub nvme_scheduler: Option<NvmeScheduler>,
    pub bbr3: Option<Bbr3>,
    pub graphics_memory: Option<GraphicsMemory>,
    pub graphics_pages: Option<GraphicsPages>,
}

pub fn look(places: &Places) -> State {
    State {
        preemption: preemption(places),
        turns: turns(places),
        slow_disks: slow_disks(places),
        dirty_writeback: dirty_writeback(places),
        nvme_scheduler: nvme_scheduler(places),
        bbr3: bbr3(places),
        graphics_memory: graphics_memory(places),
        graphics_pages: graphics_pages(places),
    }
}

/// [`look`] for a reader that looks every couple of seconds: everything
/// read again but the connections on BBRv3, which are as `before` had
/// them (a few small files, where listing connections is the kernel's
/// walk of its whole table). Such a reader takes a whole `look` every
/// fifth time.
pub fn look_again(places: &Places, before: &State) -> State {
    let on_it = before.bbr3.as_ref().map(|b| b.on_it.clone());
    State {
        preemption: preemption(places),
        turns: turns(places),
        slow_disks: slow_disks(places),
        dirty_writeback: dirty_writeback(places),
        nvme_scheduler: nvme_scheduler(places),
        bbr3: bbr3_with(places, on_it),
        graphics_memory: graphics_memory(places),
        graphics_pages: graphics_pages(places),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What reading the module's state costs here, on this computer's
    /// own kernel (`cargo test --release -p olo-system --lib
    /// what_a_look_at_graphics_pages_costs -- --ignored --nocapture`):
    /// the Kernel tab reads it every two seconds while it is shown.
    #[test]
    #[ignore = "a measurement"]
    fn what_a_look_at_graphics_pages_costs() {
        let places = Places::here();
        const TIMES: u32 = 2_000;
        let started = std::time::Instant::now();
        let mut there = 0u32;
        for _ in 0..TIMES {
            there += std::hint::black_box(graphics_pages_where(&places, true)).is_some() as u32;
        }
        println!("a look at the module's state: {:.1} µs ({there} of {TIMES} found something to say)", started.elapsed().as_secs_f64() * 1e6 / f64::from(TIMES));
        let started = std::time::Instant::now();
        for _ in 0..TIMES {
            std::hint::black_box(graphics_memory(&places));
        }
        println!("a look at the live patch's, beside it: {:.1} µs", started.elapsed().as_secs_f64() * 1e6 / f64::from(TIMES));
    }

    fn tree(name: &str) -> (PathBuf, Places) {
        let root = PathBuf::from(format!("/var/tmp/olo-kernel-tuning-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.clone(), Places::under(&root))
    }

    fn put(places: &Places, path: &str, text: &str) {
        let path = places.at(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn preemption_is_what_was_built_started_and_switched() {
        let (root, places) = tree("preempt");
        assert_eq!(preemption(&places), None, "no build options to go by");
        put(&places, "proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        put(&places, "lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_PREEMPT_BUILD=y\nCONFIG_PREEMPT_LAZY=y\nCONFIG_PREEMPTION=y\n# CONFIG_PREEMPT is not set\n");
        assert_eq!(preemption(&places), None, "a kernel that can't be told otherwise");
        put(&places, "lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_PREEMPT_BUILD=y\nCONFIG_PREEMPT_LAZY=y\nCONFIG_PREEMPT_DYNAMIC=y\n");
        put(&places, "proc/cmdline", "BOOT_IMAGE=(hd0,gpt2)/vmlinuz ro rhgb quiet\n");
        put(&places, "etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet\n");
        let p = preemption(&places).unwrap();
        assert_eq!((p.now(), p.next(), p.on(), p.live, p.chosen_otherwise()), ("lazy", "lazy", false, true, false));
        // Kept for the next start, and switched meanwhile.
        put(&places, "etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet preempt=full\n");
        let p = preemption(&places).unwrap();
        assert_eq!((p.now(), p.next(), p.on(), p.lost), ("lazy", "full", false, false));
        put(&places, "run/olo-kernel/preemption", "full\n");
        assert!(preemption(&places).unwrap().on());
        // Kept since before this start, and yet started without it: this
        // start's boot entry lacks it. (Kept after the start: only a
        // restart away.)
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        put(&places, "proc/stat", &format!("cpu  1 2 3\nbtime {}\n", now - 3600));
        assert!(!preemption(&places).unwrap().lost);
        put(&places, "proc/stat", &format!("cpu  1 2 3\nbtime {}\n", now + 3600));
        assert!(preemption(&places).unwrap().lost);
        put(&places, "proc/stat", &format!("cpu  1 2 3\nbtime {}\n", now - 3600));
        // Started with it: on with no note.
        std::fs::remove_file(places.preempt_note()).unwrap();
        put(&places, "proc/cmdline", "ro rhgb quiet preempt=full\n");
        assert!(preemption(&places).unwrap().on());
        // Someone's own choice, and a locked-down kernel.
        put(&places, "etc/kernel/cmdline", "ro preempt=none\n");
        put(&places, "sys/kernel/security/lockdown", "none [integrity] confidentiality\n");
        let p = preemption(&places).unwrap();
        assert!(p.chosen_otherwise() && !p.live);
        // Without /etc/kernel/cmdline, GRUB's defaults say.
        std::fs::remove_file(places.at("etc/kernel/cmdline")).unwrap();
        put(&places, "etc/default/grub", "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet preempt=full\"\n");
        assert_eq!(preemption(&places).unwrap().next(), "full");
        // Kept nowhere that can be read: as this start's.
        std::fs::remove_file(places.at("etc/default/grub")).unwrap();
        put(&places, "proc/cmdline", "ro rhgb quiet preempt=full\n");
        assert!(preemption(&places).unwrap().on());
        // A kernel built full has nothing to gain.
        put(&places, "lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_PREEMPT=y\nCONFIG_PREEMPT_DYNAMIC=y\n");
        assert_eq!(preemption(&places).unwrap().built, "full");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn turns_are_as_long_as_the_processors_make_them_until_kept_short() {
        let (root, places) = tree("turns");
        assert_eq!(turns(&places), None, "no processors to count");
        put(&places, "sys/devices/system/cpu/online", "0\n");
        assert_eq!(turns(&places), None, "one processor: the kernel's own turn is the short one");
        assert_eq!((processors("0-15"), processors("0-3,8-11"), processors("0"), processors("")), (16, 8, 1, 0));
        // 0.7 ms, and as much again for each doubling up to eight.
        for (online, ns) in [("0-1\n", 1_400_000), ("0-3\n", 2_100_000), ("0-7\n", 2_800_000), ("0-15\n", 2_800_000)] {
            put(&places, "sys/devices/system/cpu/online", online);
            let t = turns(&places).unwrap();
            assert_eq!((t.built_ns, t.now_ns(), t.on(), t.live), (ns, ns, false, true), "{online}");
        }
        // Set since the start (Olo's note), but not kept: on for now only.
        put(&places, "run/olo-kernel/turns", "700000\n");
        let t = turns(&places).unwrap();
        assert_eq!((t.now_ns(), t.on()), (SHORT_TURN_NS, false));
        // Kept: the lines as Olo writes them, the unit, and systemd's link to it.
        put(&places, "etc/olo/short-turns.conf", SHORT_TURNS_LINES);
        put(&places, "etc/systemd/system/olo-short-turns.service", SHORT_TURNS_UNIT);
        assert!(!turns(&places).unwrap().on(), "the unit isn't chosen");
        put(&places, "etc/systemd/system/multi-user.target.wants/olo-short-turns.service", "");
        assert!(turns(&places).unwrap().on());
        // Someone's own lines there aren't Olo's keeping.
        put(&places, "etc/olo/short-turns.conf", "w /sys/kernel/debug/sched/base_slice_ns - - - - 3000000\n");
        assert!(!turns(&places).unwrap().on());
        // A locked-down kernel takes no writes there.
        put(&places, "sys/kernel/security/lockdown", "none [integrity] confidentiality\n");
        assert!(!turns(&places).unwrap().live);
        // What the unit applies is what the note says, and nothing but the two.
        assert!(SHORT_TURNS_LINES.contains("w /sys/kernel/debug/sched/base_slice_ns - - - - 700000\n") && SHORT_TURNS_LINES.contains("/run/olo-kernel/turns"));
        assert!(SHORT_TURNS_UNIT.contains("After=sys-kernel-debug.mount") && SHORT_TURNS_UNIT.contains("systemd-tmpfiles --create /etc/olo/short-turns.conf"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn slow_disks_are_the_removable_ones_with_their_limits() {
        let (root, places) = tree("disks");
        put(&places, "proc/vmstat", "nr_dirty 100\nnr_writeback 28\nnr_dirty_threshold 3748981\n");
        // The computer's own disk, a stick, an empty card reader, an SD card.
        for (name, removable, size) in [("nvme0n1", "0", "1000"), ("sdb", "1", "60000"), ("sdc", "1", "0"), ("mmcblk0", "0", "5000")] {
            put(&places, &format!("sys/block/{name}/removable"), removable);
            put(&places, &format!("sys/block/{name}/size"), size);
            put(&places, &format!("sys/block/{name}/bdi/strict_limit"), "0\n");
            put(&places, &format!("sys/block/{name}/bdi/max_bytes"), "15355826176\n");
        }
        put(&places, "sys/block/sdb/device/vendor", "Kingston\n");
        put(&places, "sys/block/sdb/device/model", "DataTraveler 3.0\n");
        put(&places, "sys/block/mmcblk0/device/type", "SD\n");
        put(&places, "sys/block/mmcblk0/device/name", "SD64G\n");
        let s = slow_disks(&places);
        assert_eq!(s.disks.iter().map(|d| (d.name.as_str(), d.model.as_str(), d.limit)).collect::<Vec<_>>(), [("mmcblk0", "SD64G", None), ("sdb", "Kingston DataTraveler 3.0", None)]);
        assert_eq!((s.can, s.kept, s.on(), s.may_wait, s.waiting), (true, false, false, 3748981 * 4096, 128 * 4096));
        put(&places, "etc/udev/rules.d/61-olo-slow-disks.rules", SLOW_DISK_RULE);
        assert!(!slow_disks(&places).on(), "the rule, but the stick here isn't limited yet");
        for name in ["sdb", "mmcblk0"] {
            put(&places, &format!("sys/block/{name}/bdi/strict_limit"), "1\n");
            put(&places, &format!("sys/block/{name}/bdi/max_bytes"), "16777216\n");
        }
        let s = slow_disks(&places);
        assert!(s.on());
        assert_eq!(s.disks[1].limit, Some(SLOW_DISK_BYTES));
        // An eMMC, the computer's own disk on a small laptop, isn't one.
        put(&places, "sys/block/mmcblk0/device/type", "MMC\n");
        assert_eq!(slow_disks(&places).disks.len(), 1);
        // Disks on USB that don't say their media comes out (an SSD in an
        // enclosure): one at 5 Gbit/s is limited, those at 10 and 20
        // aren't.
        for (name, port, speed) in [("sdd", "2-1", "5000"), ("sde", "2-2", "10000"), ("sdf", "2-3", "20000")] {
            let device = format!("sys/devices/pci0000:00/0000:00:14.0/usb2/{port}");
            put(&places, &format!("{device}/speed"), speed);
            put(&places, &format!("{device}/idVendor"), "0781\n");
            let block = format!("{device}/{port}:1.0/host0/target0:0:0/0:0:0:0/block/{name}");
            put(&places, &format!("{block}/removable"), "0");
            put(&places, &format!("{block}/size"), "1000000");
            put(&places, &format!("{block}/bdi/strict_limit"), "0\n");
            std::os::unix::fs::symlink(places.at(&block), places.at(&format!("sys/block/{name}"))).unwrap();
        }
        assert_eq!(slow_disks(&places).disks.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["sdb", "sdd"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn olos_pieces_are_read_from_the_kernel_and_their_services() {
        let (root, places) = tree("pieces");
        assert_eq!((bbr3(&places), graphics_memory(&places), graphics_pages_where(&places, true)), (None, None, None));
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        let b = bbr3(&places).unwrap();
        assert_eq!((b.control.as_str(), b.loaded, b.on(), &b.on_it), ("cubic", false, false, &None));
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "bbr3\n");
        put(&places, "sys/module/tcp_bbr3/refcnt", "38\n");
        put(&places, "run/olo-bbr3/status", "bbr3\n");
        assert!(!bbr3(&places).unwrap().on(), "in use, but not kept");
        put(&places, "etc/systemd/system/olo-bbr3.service", "[Unit]\n");
        put(&places, "etc/systemd/system/multi-user.target.wants/olo-bbr3.service", "");
        // Its connections: three on it (one with no round trip yet), one
        // from before on cubic, a listener that isn't one.
        put(&places, "proc/net/olo-flows", "bbr3 1 18000 1000 2\nbbr3 1 42000 500 1\nbbr3 2 0 0 0\ncubic 1 9000 70 7\nbbr3 10 0 0 0\n");
        let b = bbr3(&places).unwrap();
        assert_eq!((b.on(), b.loaded), (true, true));
        assert_eq!(b.on_it, Some(OnIt { connections: 3, of: 4, round_trip_ms: Some(42.0), sent: 1500, sent_again: 3 }));
        // Looked at again a moment later: the connections as they were
        // (not listed again), the rest read afresh; a whole look reads
        // them again.
        let before = look(&places);
        put(&places, "proc/net/olo-flows", "bbr3 1 18000 1000 2\n");
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        let again = look_again(&places, &before).bbr3.unwrap();
        assert_eq!((again.control.as_str(), again.on_it.as_ref().map(|o| o.connections)), ("cubic", Some(3)));
        assert_eq!(look(&places).bbr3.unwrap().on_it.map(|o| o.connections), Some(1));
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "bbr3\n");
        // The kernel's series, and whether the module was checked on it.
        assert_eq!((b.series.as_str(), b.checked), ("", false));
        put(&places, "proc/sys/kernel/osrelease", "7.3.3-300.fc45.x86_64\n");
        let b = bbr3(&places).unwrap();
        assert_eq!((b.series.as_str(), b.checked), ("7.3", true));
        put(&places, "proc/sys/kernel/osrelease", "7.9.0-100.fc46.x86_64\n");
        assert_eq!((bbr3(&places).unwrap().series.as_str(), bbr3(&places).unwrap().checked), ("7.9", false));
        assert!(bbr3_checked_on("7.2") && !bbr3_checked_on("7") && !bbr3_checked_on("# The kernel series tcp_bbr3 has passed its full self-check on"));
        // The patch: nothing to say without xe, then in and kept.
        put(&places, "sys/module/xe/refcnt", "1\n");
        assert_eq!(graphics_memory(&places), Some(GraphicsMemory::default()));
        put(&places, "sys/kernel/livepatch/olo_gpu_memory/enabled", "1\n");
        put(&places, "etc/systemd/system/olo-gpu-memory.service", "[Unit]\n");
        put(&places, "etc/systemd/system/multi-user.target.wants/olo-gpu-memory.service", "");
        assert!(graphics_memory(&places).unwrap().on());
        // The module: not offered and nothing of it here, nothing to say;
        // offered, nothing in the kernel yet; in and switched on, but not
        // kept; kept; switched off by hand; and whether this kernel's
        // build is one it was checked on.
        assert_eq!(graphics_pages_where(&places, false), None);
        let fresh = graphics_pages_where(&places, true).unwrap();
        assert_eq!(fresh, GraphicsPages { offered: true, kernel: fresh.kernel.clone(), ..Default::default() });
        put(&places, "sys/module/olo_gpu_pages/parameters/on", "Y\n");
        put(&places, "sys/module/olo_gpu_pages/parameters/changed", "5120\n");
        put(&places, "run/olo-gpu-pages/status", "in force\n");
        let g = graphics_pages_where(&places, false).expect("in: said, offered or not");
        assert_eq!((g.in_force, g.kept, g.on(), g.changed, g.said.as_deref(), g.checked), (true, false, false, Some(5120), Some("in force"), false));
        put(&places, "etc/systemd/system/olo-gpu-pages.service", "[Unit]\n");
        put(&places, "etc/systemd/system/multi-user.target.wants/olo-gpu-pages.service", "");
        assert!(graphics_pages_where(&places, false).unwrap().on());
        put(&places, "sys/module/olo_gpu_pages/parameters/on", "N\n");
        assert!(!graphics_pages_where(&places, false).unwrap().on());
        // (The first build on the list, as the kernel shows its own and its module's.)
        let (line, write) = (GRAPHICS_PAGES_QUALIFIED.lines().find(|l| !l.starts_with('#') && !l.trim().is_empty()).unwrap(), |path: &str, bytes: Vec<u8>| {
            std::fs::create_dir_all(places.at(path).parent().unwrap()).unwrap();
            std::fs::write(places.at(path), bytes).unwrap();
        });
        let words: Vec<&str> = line.split_whitespace().collect();
        write("sys/kernel/notes", olo_kmod::builds::notes_for_tests(words[1]));
        write("sys/module/ttm/notes/.note.gnu.build-id", olo_kmod::builds::notes_for_tests(words[2]));
        assert!(!graphics_pages_where(&places, false).unwrap().checked, "another release");
        put(&places, "proc/sys/kernel/osrelease", &format!("{}\n", words[0]));
        assert!(graphics_pages_where(&places, false).unwrap().checked);
        // Kept out, in the module's own words; what the fix mends isn't that.
        let mut g = GraphicsPages { said: Some("failed: the kernel didn't take the module: Operation not supported (os error 95); its own words: this kernel's pool isn't the one this was written for (as it comes, the 2 MB try's flags are 0x392cc2 and the 1 MB try's 0x392cc2)".into()), ..Default::default() };
        assert_eq!(g.kept_out(), Some("this kernel's pool isn't the one this was written for (as it comes, the 2 MB try's flags are 0x392cc2 and the 1 MB try's 0x392cc2)"));
        g.said = Some("failed: this kernel's headers aren't installed (the kernel-devel package for 7.3.4-300.fc45.x86_64)".into());
        assert_eq!(g.kept_out(), None);
        g.said = Some("failed: the kernel refused the module (Key was rejected by service): with Secure Boot it loads only signed ones".into());
        assert_eq!(g.kept_out(), Some("the kernel refused the module (Key was rejected by service): with Secure Boot it loads only signed ones"));
        g.in_force = true;
        assert_eq!(g.kept_out(), None);
        assert_eq!(Setting::parse("graphics-pages"), Some(Setting::GraphicsPages));
        assert_eq!(Setting::parse("slow-disks"), Some(Setting::SlowDisks));
        assert_eq!(Setting::parse("dirty-writeback"), Some(Setting::DirtyWriteback));
        assert_eq!(Setting::parse("nvme-scheduler"), Some(Setting::NvmeScheduler));
        assert!(Setting::ALL.iter().all(|s| Setting::parse(s.word()) == Some(*s)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dirty_writeback_scales_by_ram_and_respects_custom_settings() {
        let (root, places) = tree("dirty");
        assert_eq!(dirty_writeback(&places), None, "no proc sysctls");
        put(&places, "proc/sys/vm/dirty_bytes", "0\n");
        put(&places, "proc/sys/vm/dirty_background_bytes", "0\n");
        put(&places, "proc/sys/vm/dirty_ratio", "20\n");
        put(&places, "proc/sys/vm/dirty_background_ratio", "10\n");
        // 8 GiB machine: under DIRTY_MIN_RAM_BYTES, not offered.
        put(&places, "proc/meminfo", "MemTotal:        8388608 kB\n");
        assert_eq!(dirty_writeback(&places), None);
        // 62 GiB machine: offered, off by default.
        put(&places, "proc/meminfo", "MemTotal:       65260884 kB\n");
        put(&places, "proc/vmstat", "nr_dirty 1000\nnr_writeback 200\nnr_dirty_background_threshold 708968\nnr_dirty_threshold 1419669\n");
        let d = dirty_writeback(&places).unwrap();
        assert!(!d.on() && !d.chosen_otherwise() && !d.kept);
        assert_eq!(d.waiting_bytes, 1200 * 4096);
        assert_eq!(d.limit_now_bytes, 1419669 * 4096);
        // Someone's custom ratio: respected.
        put(&places, "proc/sys/vm/dirty_ratio", "15\n");
        assert!(dirty_writeback(&places).unwrap().chosen_otherwise());
        put(&places, "proc/sys/vm/dirty_ratio", "0\n");
        put(&places, "proc/sys/vm/dirty_background_ratio", "0\n");
        put(&places, "proc/sys/vm/dirty_background_bytes", &format!("{DIRTY_BACKGROUND_BYTES}\n"));
        put(&places, "proc/sys/vm/dirty_bytes", &format!("{DIRTY_BYTES}\n"));
        assert!(dirty_writeback(&places).unwrap().chosen_otherwise(), "set without Olo's file");
        put(&places, "etc/sysctl.d/85-olo-dirty-writeback.conf", DIRTY_WRITEBACK_CONF);
        let d = dirty_writeback(&places).unwrap();
        assert!(d.on() && !d.chosen_otherwise());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nvme_scheduler_reads_nvme_namespaces_and_kyber_availability() {
        let (root, places) = tree("nvme");
        assert_eq!(nvme_scheduler(&places), None, "no NVMe drives");
        assert!(is_nvme_disk_name("nvme0n1") && is_nvme_disk_name("nvme12n3"));
        assert!(!is_nvme_disk_name("nvme0n1p1") && !is_nvme_disk_name("sda") && !is_nvme_disk_name("nvme0c0n1"));
        put(&places, "sys/block/nvme0n1/size", "1000000\n");
        put(&places, "sys/block/nvme0n1/queue/rotational", "0\n");
        put(&places, "sys/block/nvme0n1/queue/scheduler", "[none] mq-deadline kyber bfq\n");
        put(&places, "sys/block/nvme0n1/device/model", "SKHynix_HFS002TFM9X179N  \n");
        // Partitions and SATA disks are ignored.
        put(&places, "sys/block/nvme0n1p1/size", "1000\n");
        put(&places, "sys/block/nvme0n1p1/queue/scheduler", "[none] kyber\n");
        put(&places, "sys/block/sda/size", "1000\n");
        put(&places, "sys/block/sda/queue/scheduler", "[mq-deadline] kyber\n");
        let n = nvme_scheduler(&places).unwrap();
        assert_eq!(n.disks.len(), 1);
        assert_eq!(n.disks[0].name, "nvme0n1");
        assert_eq!(n.disks[0].model, "SKHynix HFS002TFM9X179N");
        assert_eq!(n.disks[0].scheduler, "none");
        assert!(n.can() && !n.on() && !n.chosen_otherwise());
        // Manual choice without Olo's rule: respected.
        put(&places, "sys/block/nvme0n1/queue/scheduler", "none [mq-deadline] kyber bfq\n");
        assert!(nvme_scheduler(&places).unwrap().chosen_otherwise());
        // Olo's rule + kyber active.
        put(&places, "sys/block/nvme0n1/queue/scheduler", "none mq-deadline [kyber] bfq\n");
        put(&places, "etc/udev/rules.d/62-olo-nvme-scheduler.rules", NVME_SCHEDULER_RULE);
        let n = nvme_scheduler(&places).unwrap();
        assert!(n.on() && !n.chosen_otherwise());
        assert!(NVME_SCHEDULER_RULE.contains("ATTR{queue/scheduler}=\"kyber\""));
        let _ = std::fs::remove_dir_all(root);
    }
}
