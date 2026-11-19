//! Give background builds less CPU time in Saver or as thermal headroom
//! runs short. Only a background job's own unit may be limited. Leave
//! existing quotas alone and release exactly the ones Olo installed.
use olo_system::{gpu_tuning::Followed, systemd};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Budget {
    wanted: Arc<AtomicBool>,
    _watch: Option<olo_kit::watch::Watch>,
    limited: BTreeSet<String>,
    state: PathBuf,
    quota: u64,
}

impl Budget {
    pub fn new() -> Self {
        let wanted = Arc::new(AtomicBool::new(false));
        let pressure = PathBuf::from("/run/olo-fans/pressure");
        let followed = Followed::path(Path::new("/"));
        let paths = [pressure.clone(), followed];
        let flag = wanted.clone();
        let read = move || {
            let hot = std::fs::read_to_string(&pressure).is_ok_and(|s| matches!(s.trim(), "background" | "hot"));
            let saving = Followed::load(Path::new("/")).is_some_and(|s| s.mode == "power-saver");
            flag.store(hot || saving, Ordering::Relaxed);
        };
        let state = olo_xdg::state_runtime_dir().join("background-power-quota.json");
        let real = olo_system::env::at_the_computer();
        let limited = if real { std::fs::read(&state).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default() } else { BTreeSet::new() };
        let watch = if real {
            read();
            olo_kit::watch::watch(paths, move |_| read())
        } else {
            None
        };
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self { wanted, _watch: watch, limited, state, quota: cpus as u64 * 500_000 }
    }

    pub fn needed(&self) -> bool {
        self.wanted.load(Ordering::Relaxed) || !self.limited.is_empty()
    }

    pub fn apply(&mut self, units: &[String]) {
        let wanted = self.wanted.load(Ordering::Relaxed);
        let before = self.limited.clone();
        self.limited.retain(|unit| {
            if wanted && units.contains(unit) {
                return true;
            }
            match systemd::user_cpu_quota(unit) {
                Ok(Some(quota)) if quota == self.quota => systemd::set_user_cpu_quota(unit, None).is_err(),
                Ok(_) => false,
                Err(olo_system::Error::MethodError(name, ..)) if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" => false,
                // Keep ownership across a temporary failure so a later pass can restore it.
                Err(_) => true,
            }
        });
        if wanted {
            for unit in units {
                if !self.limited.contains(unit) && matches!(systemd::user_cpu_quota(unit), Ok(None)) && systemd::set_user_cpu_quota(unit, Some(self.quota)).is_ok() {
                    self.limited.insert(unit.clone());
                }
            }
        }
        if before != self.limited
            && let Ok(data) = serde_json::to_vec(&self.limited)
        {
            let part = self.state.with_extension("part");
            let _ = std::fs::write(&part, data).and_then(|()| std::fs::rename(part, &self.state));
        }
    }
}
