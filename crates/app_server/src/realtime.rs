//! app_server's realtime threads: its main loop (every input event, every
//! frame) and its input thread (libinput, the cursor moved while the loop
//! is held), above them. Made realtime with `olo_system::realtime` (never
//! put back for running long; app_server ended only should one be stuck,
//! 30 s without once sleeping, with a report). Only the
//! person's own app_server: a headless one (an agent's screen, a test
//! session) is background work beside it (`OLO_REALTIME=1` makes one
//! realtime anyway, to measure).

/// The main loop's priority and the input thread's (above it: the
/// pointer before everything); audio's threads are at 20
/// (`olo_protocol::performance::AUDIO_RT_PRIORITY`), the watchdog and
/// the supervisor at 21.
pub const MAIN_PRIORITY: u32 = 10;
pub const INPUT_PRIORITY: u32 = 12;

/// Whether this app_server's threads are made realtime.
pub fn wanted(headless: bool) -> bool {
    match std::env::var("OLO_REALTIME").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => !headless && olo_system::realtime::wanted(),
    }
}

/// Make the calling thread realtime at `priority`.
pub fn promote_self(priority: u32) -> Result<(), String> {
    olo_system::realtime::promote_self(priority)
}

/// The kept core's own threads (`olo_system::cores`): the main loop and
/// the input thread, each homed there by [`home_self`] as it starts, and
/// there alone while the core is kept (`Settings::keep_core`). Single
/// threads: one core is all either can use, and there nothing but them
/// and other realtime threads ever runs.
mod kept {
    use std::sync::{
        Mutex,
        atomic::{AtomicU8, Ordering},
    };

    /// Unknown (no setting applied yet), off, on.
    pub(super) static KEEP: AtomicU8 = AtomicU8::new(0);
    pub(super) static HOMED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    pub(super) fn on() -> bool {
        KEEP.load(Ordering::Relaxed) == 2
    }

    /// The processors kept, worked out once (the hardware doesn't change
    /// while app_server runs; a processor taken offline only narrows it).
    pub(super) fn cpus() -> Option<&'static [usize]> {
        static KEPT: std::sync::OnceLock<Option<Vec<usize>>> = std::sync::OnceLock::new();
        KEPT.get_or_init(olo_system::cores::kept).as_deref()
    }

    /// Thread `tid` homed on the kept core (`on`) or anywhere.
    pub(super) fn home(tid: i32, on: bool) {
        let Some(kept) = cpus() else { return };
        let to = if on { kept.to_vec() } else { olo_system::cores::online() };
        if let Err(e) = olo_system::cores::set_affinity(tid, &to) {
            // (intel_lpmd's low-power mode: the cgroup allows only its
            // low-power cores, which the kept one isn't among.)
            tracing::info!(tid, "not homed on processor {}: {e}", olo_system::cores::format_list(kept));
        }
    }

    /// Every thread of the person's kept off the core (`on`), or let
    /// back; the homed ones homed. Logged with what it did and took.
    pub(super) fn sweep(on: bool) {
        let Some(kept) = cpus() else {
            tracing::info!("no core kept: fewer than {} processors", olo_system::cores::FEWEST);
            return;
        };
        let started = std::time::Instant::now();
        let homed = HOMED.lock().unwrap().clone();
        for &tid in &homed {
            home(tid, on);
        }
        let (online, plain) = (olo_system::cores::online(), olo_system::cores::plain());
        let how = if on { olo_system::cores::Sweep::KeepOff { kept, plain: &plain, online: &online } } else { olo_system::cores::Sweep::GiveBack { kept, plain: &plain, online: &online } };
        let swept = olo_system::cores::sweep(&olo_system::cores::mine(), how, &homed);
        tracing::info!(
            processes = swept.processes,
            threads = swept.threads,
            failed = swept.failed,
            ms = started.elapsed().as_secs_f64() * 1000.0,
            "processor {} {}",
            olo_system::cores::format_list(kept),
            if on { "kept for app_server's loop and input thread: the person's other programs moved off it" } else { "given back to every program" }
        );
    }
}

static PERSONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This is the person's app_server: its threads may be homed on a kept
/// core and their programs swept off it (never a headless one's, even
/// realtime to measure: the sweep is of every program of the account's).
pub fn persons() {
    PERSONS.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The calling thread is one of the kept core's own (the main loop, the
/// input thread): homed there while it's kept.
pub fn home_self() {
    if !PERSONS.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let tid = olo_system::realtime::tid();
    {
        let mut homed = kept::HOMED.lock().unwrap();
        homed.retain(|t| std::path::Path::new(&format!("/proc/self/task/{t}")).exists());
        if !homed.contains(&tid) {
            homed.push(tid);
        }
    }
    if kept::on() {
        kept::home(tid, true);
    }
}

/// `Settings::keep_core` as it is (at start, and as it changes): the
/// sweep on the realtime worker, not the loop (every thread of the
/// account's, a few milliseconds).
pub fn keep_core(on: bool) {
    if !PERSONS.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let was = kept::KEEP.swap(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    // (Off at start: nothing was moved, nothing to give back.)
    if was == 0 && !on || was == (if on { 2 } else { 1 }) {
        return;
    }
    focused::send(focused::Ask::KeepCore(on));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_comes_first() {
        assert!(INPUT_PRIORITY > MAIN_PRIORITY && INPUT_PRIORITY <= 20);
    }
}

/// The program the keyboard is in, and the shell's (Deskbar's) threads
/// someone waits on, made realtime here: each window's looper (named
/// `w>` and its title, as BeOS's) and its display thread (`display`), at
/// the kit's priorities, below app_server's own. Only a program that
/// registered them with the guard (`olo_app::realtime::guarded`: a
/// thread there ends its program only stuck, 30 s, with a report):
/// Olo's. Put back when another program has the keyboard (never for
/// running long). On a worker (reading /proc never waits on the loop),
/// told by [`focus`] and [`always`].
mod focused {
    use std::sync::{OnceLock, mpsc};

    pub(super) enum Ask {
        Focus(Option<i32>),
        Always(i32),
        KeepCore(bool),
    }

    static TX: OnceLock<mpsc::Sender<Ask>> = OnceLock::new();

    pub(super) fn send(ask: Ask) {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let _ = std::thread::Builder::new().name("realtime".into()).spawn(move || work(rx));
            tx
        });
        let _ = tx.send(ask);
    }

    /// A window's looper's priority, and the display thread's (the kit's,
    /// `olo_interface::app::WINDOW_RT_PRIORITY`, `DISPLAY_RT_PRIORITY`).
    fn priority_of(name: &str) -> Option<u32> {
        if name.starts_with("w>") {
            Some(8)
        } else if name == "display" {
            Some(9)
        } else {
            None
        }
    }

    /// The program's loopers and display thread: by name, and marked as
    /// registered with its guard (`olo_app::realtime::marked`). A thread
    /// a looper starts unnamed takes its name (`w>…`) but isn't marked:
    /// made realtime too, Agent Screens' export thread encoded video at
    /// realtime, past rtkit's hard limit (as it was then), and the kernel
    /// ended the app (Ben, 2026-11-15).
    fn threads(pid: i32) -> Vec<(i32, u32)> {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return Vec::new() };
        tasks
            .flatten()
            .filter_map(|t| {
                let tid: i32 = t.file_name().to_str()?.parse().ok()?;
                let name = std::fs::read_to_string(t.path().join("comm")).ok()?;
                let priority = priority_of(name.trim_end())?;
                olo_system::realtime::marked(tid).then_some((tid, priority))
            })
            .collect()
    }

    fn promote(pid: i32) {
        if !olo_system::realtime::guarded(pid as u32) {
            return;
        }
        for (tid, priority) in threads(pid) {
            if !olo_system::realtime::is_realtime(tid as u32)
                && let Err(e) = olo_system::realtime::make_realtime(pid as u32, tid as u32, priority)
            {
                tracing::debug!(pid, tid, "not made realtime: {e}");
            }
        }
    }

    fn demote(pid: i32) {
        for (tid, _) in threads(pid) {
            if olo_system::realtime::is_realtime(tid as u32) && !olo_system::realtime::demote(tid) {
                tracing::debug!(pid, tid, "not put back: {}", std::io::Error::last_os_error());
            }
        }
    }

    fn work(rx: mpsc::Receiver<Ask>) {
        let (mut focused, mut always): (Option<i32>, Vec<i32>) = (None, Vec::new());
        while let Ok(ask) = rx.recv() {
            match ask {
                Ask::Focus(pid) => {
                    if let Some(old) = focused.filter(|old| Some(*old) != pid && !always.contains(old)) {
                        demote(old);
                    }
                    focused = pid;
                    // (Again for the same program: a window it opened since
                    // has a thread of its own.)
                    if let Some(pid) = pid {
                        promote(pid);
                    }
                }
                Ask::KeepCore(on) => super::kept::sweep(on),
                Ask::Always(pid) => {
                    always.retain(|p| std::path::Path::new(&format!("/proc/{p}")).exists());
                    if !always.contains(&pid) {
                        always.push(pid);
                    }
                    promote(pid);
                }
            }
        }
    }
}

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this app_server makes the focused program realtime (its own
/// threads are: the person's app_server).
pub fn enable() {
    ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The keyboard went to program `pid` (None: nowhere).
pub fn focus(pid: Option<i32>) {
    if ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        focused::send(focused::Ask::Focus(pid));
    }
}

/// Program `pid` is the shell's (Deskbar): realtime whatever has the
/// keyboard.
pub fn always(pid: i32) {
    if ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        focused::send(focused::Ask::Always(pid));
    }
}
