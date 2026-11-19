//! Coordinate manual choices and automatic Battery Saver on the real session.
//! One worker serializes daemon, battery, and UI events. The compositor only
//! receives changes to the public status. Its test desktops simulate selection.
use olo_protocol::power_mode::{Mode, Modes, State};
use olo_system::{battery, power_profile};
use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel;
use std::sync::mpsc::{self, Sender};

pub enum Change {
    Select(Mode),
    Threshold(u32),
    Supply(bool, Option<u32>),
    Daemon,
    Settled,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum Episode {
    #[default]
    Normal,
    Saving(Mode),
    Manual,
}

impl Episode {
    fn low(percent: Option<u32>, threshold: u32) -> bool {
        threshold > 0 && percent.is_some_and(|p| p <= threshold)
    }

    fn manual(self, supply: Option<(bool, Option<u32>)>, threshold: u32) -> Self {
        match supply {
            Some((true, percent)) if self != Self::Normal || Self::low(percent, threshold) => Self::Manual,
            _ => Self::Normal,
        }
    }

    fn reconcile(self, active: Mode, battery: bool, percent: Option<u32>, threshold: u32) -> (Self, Option<Mode>) {
        let low = battery && Self::low(percent, threshold);
        match self {
            Self::Saving(_) if active != Mode::PowerSaver => (if battery { Self::Manual } else { Self::Normal }, None),
            Self::Saving(previous) if !battery || threshold == 0 => (Self::Normal, Some(previous)),
            Self::Normal if low && active != Mode::PowerSaver => (Self::Saving(active), Some(Mode::PowerSaver)),
            _ if !battery => (Self::Normal, None),
            _ => (self, None),
        }
    }
}

pub fn simulated() -> State {
    State { modes: Some(Modes { active: Mode::Balanced, available: Mode::ALL.to_vec(), degraded: None, drivers: vec![] }), ..State::default() }
}

pub fn start(threshold: u32) -> (Sender<Change>, channel::Channel<State>) {
    let (send, receive) = mpsc::channel();
    let (publish, updates) = channel::channel();
    let events = send.clone();
    std::thread::Builder::new()
        .name("power-policy".into())
        .spawn(move || {
            let path = olo_xdg::state_runtime_dir().join("power-saver.json");
            let mut episode: Episode = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let mut threshold = threshold;
            let mut supply = None;
            let mut last_supply = None;
            let mut adapter: Option<battery::Supply> = None;
            let mut modes = power_profile::modes().ok().flatten();
            let mut previous = None;
            let daemon = events.clone();
            power_profile::watch(move |_| {
                let _ = daemon.send(Change::Daemon);
            });
            battery::watch_supply(move |b, dc| {
                let _ = events.send(Change::Supply(dc, b.map(|b| b.percent)));
            });
            loop {
                let change = match adapter.as_ref().and_then(battery::Supply::deadline) {
                    None => match receive.recv() {
                        Ok(e) => e,
                        Err(_) => break,
                    },
                    Some(end) => match receive.recv_timeout(end.saturating_duration_since(std::time::Instant::now())) {
                        Ok(e) => e,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => Change::Settled,
                    },
                };
                let before = episode;
                let mut requested = None;
                let mut error = None;
                match change {
                    Change::Select(mode) => {
                        requested = Some(mode);
                        episode = episode.manual(supply, threshold);
                    }
                    Change::Threshold(v) => threshold = v,
                    Change::Supply(dc, percent) => {
                        if last_supply.replace((dc, percent)) == Some((dc, percent)) {
                            continue;
                        }
                        let now = std::time::Instant::now();
                        let adapter = adapter.get_or_insert_with(|| battery::Supply::new(dc));
                        adapter.observe(dc, now);
                        supply = Some((adapter.battery(now), percent));
                    }
                    Change::Settled => {
                        if let (Some(a), Some((dc, _))) = (&mut adapter, &mut supply) {
                            *dc = a.battery(std::time::Instant::now());
                        }
                    }
                    // A signal can have waited behind a manual request. Read today's
                    // state here, so an older notification cannot undo that request.
                    Change::Daemon => {
                        let next = power_profile::modes().ok().flatten();
                        if let (Some(old), Some(new)) = (&modes, &next)
                            && old.active == Mode::PowerSaver
                            && new.active != Mode::PowerSaver
                        {
                            episode = episode.manual(supply, threshold);
                        }
                        modes = next;
                    }
                }
                if requested.is_none()
                    && let (Some(m), Some((battery, percent))) = (&modes, supply)
                    && m.available.contains(&Mode::PowerSaver)
                {
                    (episode, requested) = episode.reconcile(m.active, battery, percent, threshold);
                }
                if let Some(mode) = requested {
                    match power_profile::set(mode) {
                        Ok(()) => modes = power_profile::modes().ok().flatten(),
                        Err(e) => {
                            episode = before;
                            error = Some(format!("Could not select {}: {e}", mode.label()));
                        }
                    }
                }
                if episode != before {
                    let part = path.with_extension("part");
                    if let Ok(data) = serde_json::to_vec(&episode)
                        && let Err(e) = std::fs::write(&part, data).and_then(|()| std::fs::rename(&part, &path))
                    {
                        tracing::warn!(?e, "could not keep the low-battery choice through reload");
                    }
                }
                let (on_battery, percent) = supply.unwrap_or_default();
                let state = State {
                    modes: modes.clone(),
                    on_battery,
                    percent,
                    resume_on_ac: match episode {
                        Episode::Saving(m) => Some(m),
                        _ => None,
                    },
                    error,
                };
                if previous.as_ref() != Some(&state) {
                    if publish.send(state.clone()).is_err() {
                        break;
                    }
                    previous = Some(state);
                }
            }
        })
        .expect("power policy thread");
    (send, updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_a_mode_above_the_threshold_keeps_automatic_saver_armed() {
        let normal = Episode::Normal.manual(Some((true, Some(80))), 15);
        let (saving, next) = normal.reconcile(Mode::Performance, true, Some(15), 15);
        assert_eq!((saving, next), (Episode::Saving(Mode::Performance), Some(Mode::PowerSaver)));
        let manual = saving.manual(Some((true, Some(14))), 15);
        assert_eq!(manual.manual(Some((true, Some(16))), 15), Episode::Manual);
        assert_eq!(manual.manual(Some((false, Some(14))), 15), Episode::Normal);
        assert_eq!(Episode::Normal.manual(Some((true, Some(12))), 15), Episode::Manual);
    }

    #[test]
    fn crosses_the_threshold_restores_and_respects_a_manual_choice() {
        let (e, m) = Episode::Normal.reconcile(Mode::Performance, true, Some(80), 15);
        assert_eq!((e, m), (Episode::Normal, None));
        let (e, m) = e.reconcile(Mode::Performance, true, Some(15), 15);
        assert_eq!(m, Some(Mode::PowerSaver));
        assert_eq!(e.reconcile(Mode::PowerSaver, true, Some(16), 15), (e, None));
        assert_eq!(e.reconcile(Mode::PowerSaver, false, Some(14), 15), (Episode::Normal, Some(Mode::Performance)));
        assert_eq!(e.reconcile(Mode::PowerSaver, true, Some(14), 0), (Episode::Normal, Some(Mode::Performance)));
        let (manual, _) = e.reconcile(Mode::Balanced, true, Some(12), 15);
        assert_eq!(manual.reconcile(Mode::Balanced, true, Some(8), 15), (Episode::Manual, None));
        assert_eq!(manual.reconcile(Mode::Balanced, false, Some(8), 15), (Episode::Normal, None));
    }
}
