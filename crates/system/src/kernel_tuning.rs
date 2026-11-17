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
//! - [`SlowDisks`]: a limit on what may wait in memory to be written to
//!   a removable disk (its `bdi/strict_limit` and `bdi/max_bytes`, set by
//!   a udev rule as each is plugged in), so a copy's progress is true and
//!   ejecting is quick.
//!
//! Beside them, Olo's own pieces in the kernel, shown in the same place:
//! [`Bbr3`] (TCP's congestion control, a module) and [`GraphicsMemory`]
//! (a live patch).
//!
//! `OLO_KERNEL_ROOT=DIR` puts all of it under DIR (tests: a made-up
//! computer in a scratch folder, which Doctor's steps then write to as
//! the person).

use std::path::{Path, PathBuf};

/// One of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Preemption,
    SlowDisks,
    Bbr3,
    GraphicsMemory,
}

impl Setting {
    pub const ALL: [Setting; 4] = [Setting::Preemption, Setting::SlowDisks, Setting::Bbr3, Setting::GraphicsMemory];

    /// As the command line says it.
    pub fn word(self) -> &'static str {
        match self {
            Setting::Preemption => "preemption",
            Setting::SlowDisks => "slow-disks",
            Setting::Bbr3 => "bbr3",
            Setting::GraphicsMemory => "graphics-memory",
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
    let find = || {
        let release = read(&places.at("/proc/sys/kernel/osrelease"))?;
        let config = read(&places.at("/lib/modules").join(&release).join("config")).or_else(|| read(&places.at("/boot").join(format!("config-{release}"))))?;
        built_preemption(&config)
    };
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

/// All of it, as it is now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub preemption: Option<Preemption>,
    pub slow_disks: SlowDisks,
    pub bbr3: Option<Bbr3>,
    pub graphics_memory: Option<GraphicsMemory>,
}

pub fn look(places: &Places) -> State {
    State { preemption: preemption(places), slow_disks: slow_disks(places), bbr3: bbr3(places), graphics_memory: graphics_memory(places) }
}

/// [`look`] for a reader that looks every couple of seconds: everything
/// read again but the connections on BBRv3, which are as `before` had
/// them (a few small files, where listing connections is the kernel's
/// walk of its whole table). Such a reader takes a whole `look` every
/// fifth time.
pub fn look_again(places: &Places, before: &State) -> State {
    let on_it = before.bbr3.as_ref().map(|b| b.on_it.clone());
    State { preemption: preemption(places), slow_disks: slow_disks(places), bbr3: bbr3_with(places, on_it), graphics_memory: graphics_memory(places) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!((bbr3(&places), graphics_memory(&places)), (None, None));
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
        assert_eq!(Setting::parse("slow-disks"), Some(Setting::SlowDisks));
        assert!(Setting::ALL.iter().all(|s| Setting::parse(s.word()) == Some(*s)));
        let _ = std::fs::remove_dir_all(root);
    }
}
