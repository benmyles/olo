//! The kernel's settings Olo suggests, and Olo's own pieces in the
//! kernel (`olo_system::kernel_tuning`; Preferences ▸ Performance ▸
//! Kernel; Ben, 2026-11-16: "research what other linux kernel patches
//! are recommended for high performance, responsive desktop operating
//! systems and if you find any that are win-win over the fedora 45
//! kernel … integrate them into olo like bbr3 so that olo doctor can
//! offer them. let's also have a new tab in the performance prefs for
//! kernel tuning where this stuff can live and be configured and show
//! any stats/details/data in there about the running patches too"), as
//! root steps: each put in force now and at every start, or put back as
//! the kernel comes. kernel/README.md has what was measured for each.
//!
//! `olo-doctor --kernel` says what there is, to anyone, in JSON;
//! `olo-doctor --kernel SETTING on|off` (preemption, short-turns,
//! swap-last, slow-disks, bbr3, graphics-memory) is how Preferences, a
//! person at a terminal and an agent all set one.

use std::path::Path;

use olo_system::kernel_tuning::{self as tuning, Places, Setting};

use crate::{
    checks::{self, Roots},
    fix::{Step, UnitAction},
    helper,
};

/// The steps that put one of the kernel's settings `on` (now and at
/// every start) or back as the kernel comes: only what isn't so already
/// (every one can be done again). Olo's own pieces are `piece_steps`'.
pub fn steps(setting: Setting, on: bool, places: &Places) -> Vec<Step> {
    let mut steps = Vec::new();
    let root = places.at("/");
    match setting {
        Setting::Preemption => {
            let Some(p) = tuning::preemption(places) else { return steps };
            if on {
                // (A kernel built full needs no telling. One started
                // without it though it was kept is told in every entry
                // again.)
                if p.next() != "full" || p.lost {
                    steps.push(Step::AddKernelArgument { arg: tuning::PREEMPT_ARG.into(), root });
                }
                if p.live && p.now() != "full" {
                    steps.push(Step::WriteSysfs { path: places.preempt_switch(), value: "full".into() });
                    steps.push(Step::WriteFile { path: places.preempt_note(), content: "full\n".into(), mode: 0o644 });
                }
            } else {
                if p.kept.is_some() {
                    steps.push(Step::RemoveKernelArgument { key: tuning::PREEMPT_KEY.into(), root });
                }
                if p.live && p.now() != p.built {
                    steps.push(Step::WriteSysfs { path: places.preempt_switch(), value: p.built.clone() });
                    steps.push(Step::WriteFile { path: places.preempt_note(), content: format!("{}\n", p.built), mode: 0o644 });
                }
            }
        }
        Setting::SlowDisks => {
            let s = tuning::slow_disks(places);
            if on {
                if !s.kept {
                    steps.push(Step::WriteFile { path: places.slow_disk_rule(), content: tuning::SLOW_DISK_RULE.into(), mode: 0o644 });
                }
                // Those here now, as the rule does each that comes. (One
                // with a limit already has it: the kernel keeps a limit
                // as a share of its whole, and says 16,773,338 for the
                // 16,777,216 it was given.)
                for d in s.disks.iter().filter(|d| d.limit.is_none()) {
                    steps.push(Step::WriteSysfs { path: places.strict_limit(&d.name), value: "1".into() });
                    steps.push(Step::WriteSysfs { path: places.max_bytes(&d.name), value: tuning::SLOW_DISK_BYTES.to_string() });
                }
            } else {
                if places.slow_disk_rule().exists() {
                    steps.push(Step::RemoveFile { path: places.slow_disk_rule() });
                }
                for d in s.disks.iter().filter(|d| d.limit.is_some()) {
                    steps.push(Step::WriteSysfs { path: places.strict_limit(&d.name), value: "0".into() });
                    steps.push(Step::WriteSysfs { path: places.max_ratio(&d.name), value: "100".into() });
                }
            }
        }
        Setting::ShortTurns => {
            let Some(t) = tuning::turns(places) else { return steps };
            let (lines, unit, wanted) = (places.short_turns_lines(), places.short_turns_unit(), places.short_turns_wanted());
            let is = |path: &Path, text: &str| std::fs::read_to_string(path).is_ok_and(|t| t == text);
            // (A test's tree has no systemd: the link it would make to
            // the unit stands for choosing it.)
            let chosen = std::fs::symlink_metadata(&wanted).is_ok();
            let set = |ns: u64, steps: &mut Vec<Step>| {
                steps.push(Step::WriteSysfs { path: places.turn_switch(), value: ns.to_string() });
                steps.push(Step::WriteFile { path: places.turn_note(), content: format!("{ns}\n"), mode: 0o644 });
            };
            if on {
                if !is(&lines, tuning::SHORT_TURNS_LINES) {
                    steps.push(Step::WriteFile { path: lines, content: tuning::SHORT_TURNS_LINES.into(), mode: 0o644 });
                }
                let changed = !is(&unit, tuning::SHORT_TURNS_UNIT);
                if changed {
                    steps.push(Step::WriteFile { path: unit, content: tuning::SHORT_TURNS_UNIT.into(), mode: 0o644 });
                }
                if places.stand_in() {
                    if !chosen {
                        steps.push(Step::WriteFile { path: wanted, content: String::new(), mode: 0o644 });
                    }
                } else {
                    if changed {
                        steps.push(Step::Unit { name: tuning::SHORT_TURNS_SERVICE.into(), action: UnitAction::Reload });
                    }
                    if changed || !chosen {
                        steps.push(Step::Unit { name: tuning::SHORT_TURNS_SERVICE.into(), action: UnitAction::Enable });
                    }
                }
                if t.live && t.now_ns() > tuning::SHORT_TURN_NS {
                    set(tuning::SHORT_TURN_NS, &mut steps);
                }
            } else {
                if chosen {
                    steps.push(if places.stand_in() { Step::RemoveFile { path: wanted } } else { Step::Unit { name: tuning::SHORT_TURNS_SERVICE.into(), action: UnitAction::Disable } });
                }
                if lines.exists() {
                    steps.push(Step::RemoveFile { path: lines });
                }
                if unit.exists() {
                    steps.push(Step::RemoveFile { path: unit });
                    if !places.stand_in() {
                        steps.push(Step::Unit { name: tuning::SHORT_TURNS_SERVICE.into(), action: UnitAction::Reload });
                    }
                }
                if t.live && t.now_ns() != t.built_ns {
                    set(t.built_ns, &mut steps);
                }
            }
        }
        Setting::SwapLast => {
            let Some(s) = tuning::swap_last(places) else { return steps };
            let (sysctl, lines) = (places.swap_last_sysctl(), places.swap_last_lines());
            let is = |path: &Path, text: &str| std::fs::read_to_string(path).is_ok_and(|t| t == text);
            let set = |path: std::path::PathBuf, value: &str| Step::WriteSysfs { path, value: value.into() };
            if on {
                if !is(&sysctl, tuning::SWAP_LAST_SYSCTL) {
                    steps.push(Step::WriteFile { path: sysctl, content: tuning::SWAP_LAST_SYSCTL.into(), mode: 0o644 });
                }
                if s.generations.is_some() && !is(&lines, tuning::SWAP_LAST_LINES) {
                    steps.push(Step::WriteFile { path: lines, content: tuning::SWAP_LAST_LINES.into(), mode: 0o644 });
                }
                if s.generations == Some(true) {
                    steps.push(set(places.generations(), "0"));
                }
                if s.swappiness != 0 {
                    steps.push(set(places.swappiness(), "0"));
                }
                if s.room != tuning::SWAP_LAST_ROOM {
                    steps.push(set(places.room(), &tuning::SWAP_LAST_ROOM.to_string()));
                }
                // What went to swap before comes back at once, where
                // there's room for it twice over: only as the setting is
                // put on (on already, what's there was needed there).
                if !steps.is_empty() && s.can_bring_back() && !places.stand_in() {
                    steps.push(Step::SwapBack);
                }
            } else {
                // (Olo's own files only: the sysctls' file as an earlier
                // Olo wrote it, `vm.swappiness = 1`, is Olo's too.)
                if sysctl.exists() {
                    steps.push(Step::RemoveFile { path: sysctl });
                }
                if lines.exists() {
                    steps.push(Step::RemoveFile { path: lines });
                }
                if s.generations == Some(false) && s.generations_built {
                    steps.push(set(places.generations(), "y"));
                }
                if s.swappiness != tuning::KERNELS_SWAPPINESS {
                    steps.push(set(places.swappiness(), &tuning::KERNELS_SWAPPINESS.to_string()));
                }
                if s.room != tuning::KERNELS_ROOM {
                    steps.push(set(places.room(), &tuning::KERNELS_ROOM.to_string()));
                }
            }
        }
        Setting::Bbr3 | Setting::GraphicsMemory => {}
    }
    steps
}

/// The steps for one of Olo's own pieces in the kernel: on, as Olo
/// Doctor's fix installs it (its service, which builds the module for
/// each kernel and loads it at every start); off, its service stopped
/// (which takes the module's work out of the running kernel) and
/// unchosen, its parts left for the next time.
fn piece_steps(setting: Setting, on: bool, places: &Places) -> Result<Vec<Step>, String> {
    let (service, program, name, unit) = match setting {
        Setting::Bbr3 => (checks::BBR3_SERVICE, checks::BBR3_PROGRAM, "BBRv3", checks::BBR3_UNIT),
        Setting::GraphicsMemory => (checks::GPU_MEMORY_SERVICE, checks::GPU_MEMORY_PROGRAM, "the graphics-memory patch", checks::GPU_MEMORY_UNIT),
        _ => return Ok(Vec::new()),
    };
    let wanted = places.at("/etc/systemd/system/multi-user.target.wants").join(service);
    if places.stand_in() {
        // (A test's tree has no systemd and no kernel to load into: the
        // unit, the link systemd would make to it, and what the service
        // would leave in the kernel stand for it.)
        let write = |path: &str, content: &str| Step::WriteFile { path: places.at(path), content: content.into(), mode: 0o644 };
        let now = match setting {
            Setting::Bbr3 => tuning::bbr3(places).is_some_and(|b| b.on()),
            _ => tuning::graphics_memory(places).is_some_and(|g| g.on()),
        };
        return Ok(match (on, now, setting) {
            (true, false, Setting::Bbr3) => {
                vec![
                    write(&format!("/etc/systemd/system/{service}"), unit),
                    Step::WriteFile { path: wanted, content: String::new(), mode: 0o644 },
                    write("/proc/sys/net/ipv4/tcp_congestion_control", "bbr3\n"),
                    write("/run/olo-bbr3/status", "bbr3\n"),
                ]
            }
            (true, false, _) => vec![
                write(&format!("/etc/systemd/system/{service}"), unit),
                Step::WriteFile { path: wanted, content: String::new(), mode: 0o644 },
                write("/sys/kernel/livepatch/olo_gpu_memory/enabled", "1\n"),
            ],
            (false, _, Setting::Bbr3) if std::fs::symlink_metadata(&wanted).is_ok() => vec![Step::RemoveFile { path: wanted }, write("/proc/sys/net/ipv4/tcp_congestion_control", "cubic\n")],
            (false, _, _) if std::fs::symlink_metadata(&wanted).is_ok() => vec![Step::RemoveFile { path: wanted }, write("/sys/kernel/livepatch/olo_gpu_memory/enabled", "0\n")],
            _ => Vec::new(),
        });
    }
    let roots = &Roots::real();
    if on {
        if !roots.bin.join(program).is_file() {
            return Err(format!("This Olo has no {program} beside it to install {name} from."));
        }
        return Ok(match setting {
            Setting::Bbr3 => checks::bbr3_steps(roots),
            _ => checks::graphics_memory_steps(roots),
        });
    }
    let mut steps = Vec::new();
    let state = (roots.units)(service).ok();
    if std::fs::symlink_metadata(&wanted).is_ok() || state.as_ref().is_some_and(|s| s.enabled) {
        steps.push(Step::Unit { name: service.into(), action: UnitAction::Disable });
    }
    if state.as_ref().is_some_and(|s| s.active) {
        steps.push(Step::Unit { name: service.into(), action: UnitAction::Stop });
    }
    Ok(steps)
}

/// What a setting can't be set for here, if it can't.
fn cannot(setting: Setting, places: &Places) -> Option<&'static str> {
    match setting {
        Setting::Preemption if tuning::preemption(places).is_none() => Some("This kernel has one way of preempting only (it wasn't built to be told another)."),
        Setting::ShortTurns if tuning::turns(places).is_none() => {
            Some("This computer's turns on its processor are the short ones already (the kernel lengthens them where there are more processors).")
        }
        Setting::SwapLast if tuning::swap_last(places).is_none() => Some("This computer has no swap to hold back."),
        Setting::SlowDisks if !tuning::slow_disks(places).can => Some("This kernel can't hold a disk to a limit of its own (Linux 6.2 and later can)."),
        Setting::Bbr3 if tuning::bbr3(places).is_none() => Some("This computer has no TCP to set."),
        _ => None,
    }
}

/// Put `setting` on or off, saying how it goes; what was done, in a
/// line. As root where it needs to be (the helper, through pkexec: the
/// password is asked once), and not at all where nothing is to change.
pub fn apply(setting: Setting, on: bool, places: &Places, progress: &mut dyn FnMut(String)) -> Result<String, String> {
    if let Some(why) = cannot(setting, places) {
        return Err(why.into());
    }
    let steps = match setting {
        Setting::Bbr3 | Setting::GraphicsMemory => piece_steps(setting, on, places)?,
        _ => steps(setting, on, places),
    };
    // (Read before the steps: what the start argument can't change now.)
    let before = tuning::preemption(places);
    let (turns_before, swap_before) = (tuning::turns(places), tuning::swap_last(places));
    let brought_back = steps.contains(&Step::SwapBack);
    if !steps.is_empty() {
        // (A test's tree is the person's to write.)
        let results = if places.stand_in() { helper::run_steps_with(&steps, progress) } else { helper::run_as_root(&steps, progress)? };
        if let Some(e) = results.into_iter().find_map(Result::err) {
            return Err(e);
        }
    }
    Ok(match (setting, on) {
        (Setting::Preemption, true) => match before {
            Some(p) if !p.live && p.now() != "full" => "The kernel switches to a woken program at once from the next restart (it's locked down, so it can't be switched while it runs).".to_owned(),
            _ => "The kernel switches to a woken program at once, now and from every start.".to_owned(),
        },
        (Setting::Preemption, false) => match before {
            Some(p) if !p.live && p.now() != p.built => format!("The kernel's preemption is as it comes ({}) from the next restart.", p.built),
            Some(p) => format!("The kernel's preemption is as it comes ({}).", p.built),
            None => "The kernel's preemption is as it comes.".to_owned(),
        },
        (Setting::ShortTurns, true) => match turns_before {
            Some(t) if !t.live && t.now_ns() > tuning::SHORT_TURN_NS => {
                "A busy program's turn on a processor is 0.7 ms from the next restart (the kernel is locked down, so it can't be set while it runs).".to_owned()
            }
            _ => "A busy program's turn on a processor is 0.7 ms, now and from every start.".to_owned(),
        },
        (Setting::ShortTurns, false) => match turns_before {
            Some(t) => format!("Turns on the processors are as the kernel comes ({} ms).", t.built_ns as f64 / 1e6),
            None => "Turns on the processors are as the kernel comes.".to_owned(),
        },
        (Setting::SwapLast, true) => match swap_before {
            Some(s) if brought_back => {
                format!("Swap is used only when nothing else is left, now and from every start; the {:.1} GB that were in it are back in memory.", s.in_swap as f64 / (1u64 << 30) as f64)
            }
            Some(s) if s.in_swap > 0 && !s.on() => {
                format!(
                    "Swap is used only when nothing else is left, now and from every start; the {:.1} GB in it now come back as they're next used (there wasn't room to bring them back at once).",
                    s.in_swap as f64 / (1u64 << 30) as f64
                )
            }
            _ => "Swap is used only when nothing else is left, now and from every start.".to_owned(),
        },
        (Setting::SwapLast, false) => "Memory is made room in as the kernel comes: programs' own memory is moved to swap while there is files' cache to drop.".to_owned(),
        (Setting::SlowDisks, true) => "What waits in memory for a removable disk is limited to 16 MB: those here now, and each as it's plugged in.".to_owned(),
        (Setting::SlowDisks, false) => "Writes to removable disks wait in memory like any others, as the kernel comes.".to_owned(),
        (Setting::Bbr3, true) => "New connections send with BBRv3, now and from every start.".to_owned(),
        (Setting::Bbr3, false) => format!("TCP sends as before ({}).", tuning::bbr3(places).map(|b| b.control).unwrap_or_default()),
        (Setting::GraphicsMemory, true) => "Windows' graphics memory stays out of swap, now and from every start.".to_owned(),
        (Setting::GraphicsMemory, false) => "The graphics-memory patch is out of the running kernel, and stays out.".to_owned(),
    })
}

/// What there is, as JSON: every setting with whether it's on (now and
/// from every start), what it is now, and its figures; null where the
/// computer has nothing to set.
pub fn status(places: &Places) -> serde_json::Value {
    use serde_json::json;
    let s = tuning::look(places);
    json!({
        "preemption": s.preemption.map(|p| json!({
            "on": p.on(), "now": p.now(), "next_start": p.next(), "built": p.built, "started_with": p.started, "can_switch_now": p.live, "boot_entry_lacks_it": p.lost,
        })),
        "short-turns": s.turns.map(|t| json!({
            "on": t.on(), "now_ns": t.now_ns(), "as_the_kernel_comes_ns": t.built_ns, "short_ns": tuning::SHORT_TURN_NS, "kept_at_start": t.kept, "can_set_now": t.live,
        })),
        "swap-last": s.swap_last.map(|w| json!({
            "on": w.on(), "swappiness": w.swappiness, "multi_generational_lru": w.generations, "room_kept_free": w.room, "kept_at_start": w.kept,
            "swap_bytes": w.swap, "in_swap_bytes": w.in_swap, "available_bytes": w.available,
        })),
        "slow-disks": json!({
            "on": s.slow_disks.on(), "can": s.slow_disks.can, "rule": s.slow_disks.kept, "limit_bytes": tuning::SLOW_DISK_BYTES,
            "may_wait_bytes": s.slow_disks.may_wait, "waiting_bytes": s.slow_disks.waiting,
            "disks": s.slow_disks.disks.iter().map(|d| json!({ "name": d.name, "model": d.model, "limit_bytes": d.limit })).collect::<Vec<_>>(),
        }),
        "bbr3": s.bbr3.map(|b| json!({
            "on": b.on(), "control": b.control, "module_loaded": b.loaded, "kept_at_start": b.kept, "said": b.said,
            "kernel_series": b.series, "checked_on_series": b.checked,
            "on_it": b.on_it.map(|o| json!({
                "connections": o.connections, "of": o.of, "round_trip_ms": o.round_trip_ms, "segments_sent": o.sent, "segments_sent_again": o.sent_again,
            })),
        })),
        "graphics-memory": s.graphics_memory.map(|g| json!({ "on": g.on(), "patched": g.patched, "kept_at_start": g.kept, "said": g.said })),
    })
}

/// `olo-doctor --kernel [SETTING on|off]`; its exit status.
pub fn from_the_command_line(args: &[String]) -> i32 {
    let places = Places::here();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let (setting, on) = match words.as_slice() {
        [] => {
            println!("{}", serde_json::to_string_pretty(&status(&places)).unwrap_or_default());
            return 0;
        }
        [word, how @ ("on" | "off")] => match Setting::parse(word) {
            Some(setting) => (setting, *how == "on"),
            None => {
                eprintln!("olo-doctor: --kernel takes {}, not {word}", Setting::ALL.map(Setting::word).join(", "));
                return 2;
            }
        },
        _ => {
            eprintln!("olo-doctor: --kernel takes a setting ({}) and on or off; alone, it says what there is", Setting::ALL.map(Setting::word).join(", "));
            return 2;
        }
    };
    match apply(setting, on, &places, &mut |text| println!("  {text}")) {
        Ok(said) => {
            println!("{said}");
            0
        }
        Err(e) => {
            eprintln!("olo-doctor: {e}");
            1
        }
    }
}

/// The file system's root the kernel's settings are read under, for
/// Olo Doctor's checks (whose roots are "/" or a test's tree).
pub fn places(fs: &Path) -> Places {
    Places::under(fs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tree(name: &str) -> (PathBuf, Places) {
        let root = PathBuf::from(format!("/var/tmp/olo-doctor-kernel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.clone(), Places::under(&root))
    }

    fn put(places: &Places, path: &str, text: &str) {
        let path = places.at(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn run(steps: &[Step]) {
        for r in helper::run_steps(steps) {
            r.unwrap();
        }
    }

    /// Fedora's kernel as it comes, in a tree.
    fn fedora(places: &Places) {
        put(places, "proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        put(places, "lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_PREEMPT_LAZY=y\nCONFIG_PREEMPT_DYNAMIC=y\n");
        put(places, "proc/cmdline", "BOOT_IMAGE=(hd0,gpt2)/vmlinuz ro rhgb quiet\n");
        put(places, "etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet\n");
        put(places, "boot/loader/entries/x-7.2.15.conf", "title Fedora\noptions root=UUID=1 ro rhgb quiet\n");
        put(places, "sys/kernel/debug/sched/preempt", "none voluntary full (lazy)\n");
    }

    #[test]
    fn preemption_is_switched_now_and_kept_for_every_start_and_put_back() {
        let (root, places) = tree("preempt");
        assert!(steps(Setting::Preemption, true, &places).is_empty(), "a kernel that can't be told: nothing to do");
        fedora(&places);
        let on = steps(Setting::Preemption, true, &places);
        assert!(matches!(on.as_slice(), [Step::AddKernelArgument { arg, .. }, Step::WriteSysfs { value, .. }, Step::WriteFile { .. }] if arg == "preempt=full" && value == "full"), "{on:?}");
        run(&on);
        let read = |p: &str| std::fs::read_to_string(places.at(p)).unwrap();
        assert_eq!(read("etc/kernel/cmdline"), "root=UUID=1 ro rhgb quiet preempt=full\n");
        assert_eq!(read("boot/loader/entries/x-7.2.15.conf"), "title Fedora\noptions root=UUID=1 ro rhgb quiet preempt=full\n");
        assert_eq!(read("sys/kernel/debug/sched/preempt"), "full");
        assert!(tuning::preemption(&places).unwrap().on());
        assert!(steps(Setting::Preemption, true, &places).is_empty(), "again: nothing to do");
        // Off: the argument out, and the running kernel back on what it was built with.
        let off = steps(Setting::Preemption, false, &places);
        assert!(matches!(off.as_slice(), [Step::RemoveKernelArgument { key, .. }, Step::WriteSysfs { value, .. }, Step::WriteFile { .. }] if key == "preempt" && value == "lazy"), "{off:?}");
        run(&off);
        assert_eq!(read("etc/kernel/cmdline"), "root=UUID=1 ro rhgb quiet\n");
        let p = tuning::preemption(&places).unwrap();
        assert_eq!((p.now(), p.next(), p.on()), ("lazy", "lazy", false));
        assert!(steps(Setting::Preemption, false, &places).is_empty());
        // A start whose boot entry lacked it, though it was kept (a
        // kernel an update brought, its entry made some other way): put
        // in every entry again, and switched for now.
        put(&places, "etc/kernel/cmdline", "root=UUID=1 ro rhgb quiet preempt=full\n");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        put(&places, "proc/stat", &format!("btime {}\n", now + 3600));
        put(&places, "boot/loader/entries/x-7.3.3.conf", "title Fedora\noptions root=UUID=1 ro rhgb quiet\n");
        std::fs::remove_file(places.preempt_note()).unwrap();
        assert!(tuning::preemption(&places).unwrap().lost);
        let again = steps(Setting::Preemption, true, &places);
        assert!(matches!(again.as_slice(), [Step::AddKernelArgument { .. }, Step::WriteSysfs { .. }, Step::WriteFile { .. }]), "{again:?}");
        run(&again);
        assert_eq!(read("boot/loader/entries/x-7.3.3.conf"), "title Fedora\noptions root=UUID=1 ro rhgb quiet preempt=full\n");
        run(&steps(Setting::Preemption, false, &places));
        put(&places, "proc/stat", "btime 1\n");
        // Locked down: only the start argument, and it says so.
        put(&places, "sys/kernel/security/lockdown", "none [integrity] confidentiality\n");
        std::fs::remove_file(places.preempt_note()).unwrap();
        let locked = steps(Setting::Preemption, true, &places);
        assert!(matches!(locked.as_slice(), [Step::AddKernelArgument { .. }]), "{locked:?}");
        let said = apply(Setting::Preemption, true, &places, &mut |_| {}).unwrap();
        assert!(said.contains("from the next restart"), "{said}");
        let p = tuning::preemption(&places).unwrap();
        assert_eq!((p.now(), p.next()), ("lazy", "full"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn short_turns_are_set_now_and_kept_by_a_unit_and_put_back() {
        let (root, places) = tree("turns");
        assert!(steps(Setting::ShortTurns, true, &places).is_empty(), "no processors to count: nothing to do");
        assert!(apply(Setting::ShortTurns, true, &places, &mut |_| {}).is_err());
        put(&places, "sys/devices/system/cpu/online", "0-15\n");
        put(&places, "sys/kernel/debug/sched/base_slice_ns", "2800000\n");
        let on = steps(Setting::ShortTurns, true, &places);
        assert!(
            matches!(on.as_slice(), [Step::WriteFile { content: lines, .. }, Step::WriteFile { content: unit, .. }, Step::WriteFile { path: wanted, .. }, Step::WriteSysfs { value, .. }, Step::WriteFile { content: note, .. }]
            if lines == tuning::SHORT_TURNS_LINES && unit == tuning::SHORT_TURNS_UNIT && wanted.ends_with("multi-user.target.wants/olo-short-turns.service") && value == "700000" && note == "700000\n"),
            "{on:?}"
        );
        run(&on);
        let read = |p: &str| std::fs::read_to_string(places.at(p)).unwrap();
        assert_eq!(read("sys/kernel/debug/sched/base_slice_ns"), "700000");
        let t = tuning::turns(&places).unwrap();
        assert!(t.on() && t.now_ns() == tuning::SHORT_TURN_NS);
        assert!(steps(Setting::ShortTurns, true, &places).is_empty(), "again: nothing to do");
        assert!(apply(Setting::ShortTurns, true, &places, &mut |_| {}).unwrap().contains("0.7 ms, now and from every start"));
        // Off: the unit unchosen and gone with its lines, the turn as the kernel has it for sixteen processors.
        let off = steps(Setting::ShortTurns, false, &places);
        assert!(
            matches!(off.as_slice(), [Step::RemoveFile { .. }, Step::RemoveFile { .. }, Step::RemoveFile { .. }, Step::WriteSysfs { value, .. }, Step::WriteFile { .. }] if value == "2800000"),
            "{off:?}"
        );
        run(&off);
        let t = tuning::turns(&places).unwrap();
        assert_eq!((t.on(), t.kept, t.now_ns()), (false, false, 2_800_000));
        assert!(steps(Setting::ShortTurns, false, &places).is_empty());
        assert!(apply(Setting::ShortTurns, false, &places, &mut |_| {}).unwrap().contains("2.8 ms"));
        // Locked down: kept for every start only, and it says so.
        put(&places, "sys/kernel/security/lockdown", "none [integrity] confidentiality\n");
        let locked = steps(Setting::ShortTurns, true, &places);
        assert!(matches!(locked.as_slice(), [Step::WriteFile { .. }, Step::WriteFile { .. }, Step::WriteFile { .. }]), "{locked:?}");
        assert!(apply(Setting::ShortTurns, true, &places, &mut |_| {}).unwrap().contains("from the next restart"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn swap_is_put_last_now_and_kept_and_put_back() {
        let (root, places) = tree("swap");
        assert!(apply(Setting::SwapLast, true, &places, &mut |_| {}).is_err(), "no swap: nothing to hold back");
        put(&places, "proc/meminfo", "MemAvailable:   30000000 kB\nSwapTotal:      65262588 kB\nSwapFree:       55262588 kB\n");
        put(&places, "proc/sys/vm/swappiness", "1\n");
        put(&places, "proc/sys/vm/watermark_scale_factor", "10\n");
        put(&places, "sys/kernel/mm/lru_gen/enabled", "0x0007\n");
        put(&places, "proc/sys/kernel/osrelease", "7.2.15-300.fc45.x86_64\n");
        put(&places, "lib/modules/7.2.15-300.fc45.x86_64/config", "CONFIG_LRU_GEN=y\nCONFIG_LRU_GEN_ENABLED=y\n");
        // (As an earlier Olo left it.)
        put(&places, "etc/sysctl.d/80-olo-memory.conf", "vm.swappiness = 1\n");
        let on = steps(Setting::SwapLast, true, &places);
        assert!(
            matches!(on.as_slice(), [Step::WriteFile { content: sysctl, .. }, Step::WriteFile { content: lines, .. }, Step::WriteSysfs { path: lru, value: none }, Step::WriteSysfs { path: swappiness, value: zero }, Step::WriteSysfs { path: room, value: two }]
            if sysctl == tuning::SWAP_LAST_SYSCTL && lines == tuning::SWAP_LAST_LINES && lru.ends_with("lru_gen/enabled") && none == "0" && swappiness.ends_with("vm/swappiness") && zero == "0" && room.ends_with("watermark_scale_factor") && two == "200"),
            "{on:?}"
        );
        run(&on);
        let s = tuning::swap_last(&places).unwrap();
        assert!(s.on(), "{s:?}");
        assert!(steps(Setting::SwapLast, true, &places).is_empty(), "again: nothing to do, and what's in swap is left there");
        // Off: Olo's files gone, the kernel's own readiness, room and reclaim.
        let off = steps(Setting::SwapLast, false, &places);
        assert!(
            matches!(off.as_slice(), [Step::RemoveFile { .. }, Step::RemoveFile { .. }, Step::WriteSysfs { value: lru, .. }, Step::WriteSysfs { value: sixty, .. }, Step::WriteSysfs { value: ten, .. }] if lru == "y" && sixty == "60" && ten == "10"),
            "{off:?}"
        );
        run(&off);
        // (The kernel says its mask back, where the tree has the word written.)
        put(&places, "sys/kernel/mm/lru_gen/enabled", "0x0007\n");
        let s = tuning::swap_last(&places).unwrap();
        assert_eq!((s.on(), s.kept, s.swappiness, s.room), (false, false, 60, 10));
        assert!(steps(Setting::SwapLast, false, &places).is_empty());
        // A kernel without the multi-generational LRU: the sysctls alone.
        std::fs::remove_file(places.generations()).unwrap();
        let on = steps(Setting::SwapLast, true, &places);
        assert!(matches!(on.as_slice(), [Step::WriteFile { .. }, Step::WriteSysfs { .. }, Step::WriteSysfs { .. }]), "{on:?}");
        let said = apply(Setting::SwapLast, true, &places, &mut |_| {}).unwrap();
        assert!(said.contains("come back as they're next used"), "{said}");
        assert!(tuning::swap_last(&places).unwrap().on());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn slow_disks_get_the_rule_and_those_here_their_limit_and_back() {
        let (root, places) = tree("disks");
        for (name, removable) in [("nvme0n1", "0"), ("sdb", "1")] {
            put(&places, &format!("sys/block/{name}/removable"), removable);
            put(&places, &format!("sys/block/{name}/size"), "60000");
            put(&places, &format!("sys/block/{name}/bdi/strict_limit"), "0\n");
            put(&places, &format!("sys/block/{name}/bdi/max_bytes"), "15355826176\n");
            put(&places, &format!("sys/block/{name}/bdi/max_ratio"), "100\n");
        }
        let on = steps(Setting::SlowDisks, true, &places);
        assert!(
            matches!(on.as_slice(), [Step::WriteFile { content, .. }, Step::WriteSysfs { path: a, value: one }, Step::WriteSysfs { path: b, value: bytes }]
            if content == tuning::SLOW_DISK_RULE && a.ends_with("sdb/bdi/strict_limit") && one == "1" && b.ends_with("sdb/bdi/max_bytes") && bytes == "16777216"),
            "{on:?}"
        );
        run(&on);
        assert!(tuning::slow_disks(&places).on());
        assert!(steps(Setting::SlowDisks, true, &places).is_empty());
        // (As the kernel says it back.)
        put(&places, "sys/block/sdb/bdi/max_bytes", "16773338\n");
        assert!(tuning::slow_disks(&places).on() && steps(Setting::SlowDisks, true, &places).is_empty());
        let off = steps(Setting::SlowDisks, false, &places);
        assert!(
            matches!(off.as_slice(), [Step::RemoveFile { .. }, Step::WriteSysfs { value: zero, .. }, Step::WriteSysfs { path, value: all }] if zero == "0" && path.ends_with("sdb/bdi/max_ratio") && all == "100"),
            "{off:?}"
        );
        run(&off);
        assert!(!tuning::slow_disks(&places).kept);
        // With no removable disk here, only the rule; and its words to udev.
        std::fs::remove_dir_all(places.at("sys/block/sdb")).unwrap();
        assert!(matches!(steps(Setting::SlowDisks, true, &places).as_slice(), [Step::WriteFile { .. }]));
        assert!(tuning::SLOW_DISK_RULE.contains("ATTR{bdi/strict_limit}=\"1\", ATTR{bdi/max_bytes}=\"16777216\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn olos_pieces_are_chosen_and_unchosen_in_a_tree() {
        let (root, places) = tree("pieces");
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        put(&places, "sys/module/xe/refcnt", "1\n");
        for (setting, is_on) in [(Setting::Bbr3, (|p: &Places| tuning::bbr3(p).unwrap().on()) as fn(&Places) -> bool), (Setting::GraphicsMemory, |p: &Places| tuning::graphics_memory(p).unwrap().on())]
        {
            assert!(!is_on(&places));
            apply(setting, true, &places, &mut |_| {}).unwrap();
            assert!(is_on(&places), "{}", setting.word());
            apply(setting, true, &places, &mut |_| {}).unwrap();
            apply(setting, false, &places, &mut |_| {}).unwrap();
            assert!(!is_on(&places), "{}", setting.word());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn what_there_is_is_said_as_json_and_settings_are_named() {
        let (root, places) = tree("status");
        fedora(&places);
        put(&places, "proc/sys/net/ipv4/tcp_congestion_control", "cubic\n");
        let s = status(&places);
        assert_eq!(s["preemption"]["now"], "lazy");
        assert_eq!(s["preemption"]["on"], false);
        assert_eq!(s["slow-disks"]["limit_bytes"], 16777216u64);
        assert_eq!(s["bbr3"]["control"], "cubic");
        assert!(s["graphics-memory"].is_null(), "no xe here: nothing to say");
        // Every setting's name is one the JSON has.
        for setting in Setting::ALL {
            assert!(s.get(setting.word()).is_some(), "{}", setting.word());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
