//! Power: turning the displays off and sleeping when idle, caffeinated
//! mode, apps that ask to stay awake (zwp_idle_inhibit), the built-in
//! display's brightness and the brightness / volume keys.
//!
//! Hardware is only touched when app_server is the real session
//! (`Backend::IS_SESSION`); elsewhere (headless tests, nested) the backlight
//! is simulated and sleep is only logged. Everything that talks to the
//! system runs on a worker thread, never the compositor's.

use std::{
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use smithay::reexports::{
    calloop,
    wayland_server::{Resource, protocol::wl_surface::WlSurface},
};
use tabula_protocol::{Event, PowerStatus};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// How often idleness is checked.
pub const TICK: Duration = Duration::from_secs(5);
/// One press of a brightness or volume key.
const STEP: f32 = 1.0 / 16.0;

/// evdev codes of the keys handled here.
pub mod keys {
    pub const MUTE: u32 = 113;
    pub const VOLUME_DOWN: u32 = 114;
    pub const VOLUME_UP: u32 = 115;
    pub const BRIGHTNESS_DOWN: u32 = 224;
    pub const BRIGHTNESS_UP: u32 = 225;
}

enum Job {
    /// Hold an inhibitor (also against the lid switch with `lid`), or
    /// none.
    Caffeinate { on: bool, lid: bool },
    Brightness(u32),
    Suspend,
    Volume(f32),
    ToggleMute,
}

pub struct Power {
    last_activity: Instant,
    caffeinated: bool,
    /// When a timed caffeinated stretch ends.
    caffeinated_until: Option<SystemTime>,
    caffeinated_lid: bool,
    display_asleep: bool,
    /// Sleep was asked for in this idle stretch (don't ask again).
    slept: bool,
    inhibitors: Vec<WlSurface>,
    /// The real backlight (session only).
    backlight: Option<tabula_system::backlight::Backlight>,
    /// Perceived brightness, 0–1 (simulated when there is no backlight to drive).
    brightness: Option<f32>,
    /// Simulated volume and mute (not the session), for the indicator.
    volume: Option<(f32, bool)>,
    /// The volume after a key, from the worker (session only); taken by
    /// state set-up.
    pub volume_feedback: Option<calloop::channel::Channel<(f32, bool)>>,
    jobs: Option<Sender<Job>>,
}

impl std::fmt::Debug for Power {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Power").field("status", &self.status()).finish()
    }
}

impl Power {
    pub fn new(is_session: bool) -> Power {
        let (backlight, brightness, jobs, volume, volume_feedback) = if is_session {
            let backlight = tabula_system::backlight::internal();
            let brightness = backlight.as_ref().and_then(|b| b.level());
            let (tx, rx) = channel();
            let (feedback, feedback_rx) = calloop::channel::channel();
            std::thread::Builder::new().name("power".into()).spawn(move || worker(rx, feedback)).expect("thread");
            (backlight, brightness, Some(tx), None, Some(feedback_rx))
        } else {
            (None, Some(0.75), None, Some((0.5, false)), None)
        };
        Power {
            last_activity: Instant::now(),
            caffeinated: false,
            caffeinated_until: None,
            caffeinated_lid: false,
            display_asleep: false,
            slept: false,
            inhibitors: Vec::new(),
            backlight,
            brightness,
            volume,
            volume_feedback,
            jobs,
        }
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }

    fn apply_brightness(&self, level: f32) {
        if let Some(b) = &self.backlight {
            self.send(Job::Brightness(tabula_system::backlight::to_raw(level, b.max)));
        }
    }

    pub fn caffeinated(&self) -> bool {
        self.caffeinated
    }

    pub fn status(&self) -> PowerStatus {
        PowerStatus {
            caffeinated: self.caffeinated,
            idle_seconds: self.last_activity.elapsed().as_secs(),
            display_asleep: self.display_asleep,
            app_inhibitors: self.inhibitors.len() as u32,
            brightness: self.brightness,
            screen_saver: false,
            caffeinated_until: self.caffeinated_until.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            caffeinated_lid: self.caffeinated_lid,
        }
    }

    pub fn display_asleep(&self) -> bool {
        self.display_asleep
    }
}

fn worker(rx: Receiver<Job>, feedback: calloop::channel::Sender<(f32, bool)>) {
    let mut inhibitor = None;
    while let Ok(mut job) = rx.recv() {
        // Only the newest brightness matters (a held key repeats).
        while let Job::Brightness(_) = job {
            match rx.try_recv() {
                Ok(next @ Job::Brightness(_)) => job = next,
                Ok(next) => {
                    run(job, &mut inhibitor, &feedback);
                    job = next;
                }
                Err(_) => break,
            }
        }
        run(job, &mut inhibitor, &feedback);
    }
}

fn run(job: Job, inhibitor: &mut Option<tabula_system::power::Inhibitor>, feedback: &calloop::channel::Sender<(f32, bool)>) {
    use tabula_system::{audio, backlight, power};
    let volume_key = matches!(job, Job::Volume(_) | Job::ToggleMute);
    let r: Result<(), String> = match job {
        Job::Caffeinate { on: true, lid } => {
            *inhibitor = None;
            let what = if lid { "sleep:idle:handle-lid-switch" } else { "sleep:idle" };
            power::inhibit(what, "Tabula", "Caffeinated", true).map(|i| *inhibitor = Some(i)).map_err(|e| e.to_string())
        }
        Job::Caffeinate { on: false, .. } => {
            *inhibitor = None;
            Ok(())
        }
        Job::Brightness(raw) => match backlight::internal() {
            Some(b) => b.set_raw(raw).map_err(|e| e.to_string()),
            None => Ok(()),
        },
        Job::Suspend => power::suspend().map_err(|e| e.to_string()),
        Job::Volume(delta) => audio::volume()
            .and_then(|v| {
                audio::set_volume((v.level + delta).clamp(0.0, 1.0))?;
                if v.muted { audio::set_muted(false) } else { Ok(()) }
            })
            .map_err(|e| e.to_string()),
        Job::ToggleMute => audio::toggle_mute().map_err(|e| e.to_string()),
    };
    if let Err(e) = r {
        warn!("power: {e}");
    }
    if volume_key && let Ok(v) = audio::volume() {
        let _ = feedback.send((v.level, v.muted));
    }
}

impl<B: Backend> AnvilState<B> {
    pub fn power_status(&self) -> tabula_protocol::PowerStatus {
        tabula_protocol::PowerStatus { screen_saver: self.saver.running(), ..self.power.status() }
    }

    pub fn power_changed(&mut self) {
        let status = self.power_status();
        self.ipc.broadcast(Event::PowerChanged { status });
    }

    /// Any input: the user is here.
    pub fn power_activity(&mut self) {
        let p = &mut self.power;
        p.last_activity = Instant::now();
        p.slept = false;
        if p.display_asleep {
            p.display_asleep = false;
            if let Some(level) = p.brightness {
                p.apply_brightness(level);
            }
            info!("displays awake");
            self.power_changed();
        }
        self.saver_activity();
    }

    /// Every [`TICK`]: turn displays off or sleep if idle long enough.
    pub fn power_tick(&mut self) {
        self.ensure_locker();
        self.saver_tick();
        let saver = self.settings.screen_saver.clone();
        if saver.never_fade.is_some() && self.pointer_corner() == saver.never_fade {
            self.power.last_activity = Instant::now();
        }
        let before = self.power.inhibitors.len();
        self.power.inhibitors.retain(|s| s.is_alive());
        let pruned = self.power.inhibitors.len() != before;
        if self.power.caffeinated_until.is_some_and(|t| SystemTime::now() >= t) {
            info!("caffeinated time is up");
            self.set_caffeinated(false, None, false);
        }
        let settings = self.settings.power.clone();
        let p = &mut self.power;
        if p.caffeinated || !p.inhibitors.is_empty() {
            p.last_activity = Instant::now();
        }
        let idle = p.last_activity.elapsed();
        let mut changed = pruned;
        let mut lock = false;
        let minutes = |m: u32| Duration::from_secs(m as u64 * 60);
        // On the lock screen nobody is working: displays go off after a minute.
        let display_off = if self.lock.locked() { 1 } else { settings.display_off_minutes };
        if display_off > 0 && idle >= minutes(display_off) && !p.display_asleep {
            p.display_asleep = true;
            p.send(Job::Brightness(0));
            info!(?idle, "displays asleep");
            changed = true;
            lock = settings.lock_on_display_off;
        }
        let display_asleep = p.display_asleep;
        if settings.sleep_minutes > 0
            && idle >= minutes(settings.sleep_minutes)
            && !p.slept
            && (!settings.sleep_on_battery_only || !tabula_system::battery::on_ac())
        {
            p.slept = true;
            if p.jobs.is_some() {
                info!(?idle, "sleeping");
                p.send(Job::Suspend);
            } else {
                info!(?idle, "would sleep now (not the session)");
            }
        }
        if display_asleep {
            self.stop_screen_saver();
        } else if saver.enabled && saver.run_minutes > 0 && idle >= minutes(saver.run_minutes) {
            self.start_screen_saver(false);
        }
        if self.saver.running() && saver.lock && idle >= minutes(saver.lock_minutes.max(saver.run_minutes)) {
            lock = true;
        }
        if lock {
            self.lock_screen();
        }
        if changed {
            self.power_changed();
        }
    }

    /// Caffeinated mode on (until turned off, or for `minutes`; through
    /// lid closes with `lid`) or off.
    pub fn set_caffeinated(&mut self, on: bool, minutes: Option<u32>, lid: bool) {
        let until = minutes.filter(|_| on).map(|m| SystemTime::now() + Duration::from_secs(m as u64 * 60));
        let lid = on && lid;
        let p = &mut self.power;
        if p.caffeinated == on && p.caffeinated_lid == lid && until.is_none() && p.caffeinated_until.is_none() {
            return;
        }
        if p.caffeinated != on || p.caffeinated_lid != lid {
            p.send(Job::Caffeinate { on, lid });
        }
        p.caffeinated = on;
        p.caffeinated_lid = lid;
        p.caffeinated_until = until;
        info!(on, ?minutes, lid, "caffeinated");
        self.power_activity();
        self.power_changed();
    }

    pub fn set_brightness(&mut self, level: f32) {
        let Some(_) = self.power.brightness else { return };
        let level = level.clamp(0.0, 1.0);
        self.power.brightness = Some(level);
        if !self.power.display_asleep {
            self.power.apply_brightness(level);
        }
        self.power_changed();
    }

    pub fn idle_inhibit(&mut self, surface: WlSurface, on: bool) {
        self.power.inhibitors.retain(|s| *s != surface && s.is_alive());
        if on {
            self.power.inhibitors.push(surface);
        }
        self.power_activity();
        self.power_changed();
    }

    /// Brightness and volume keys. True if `key` was one (then it is not
    /// passed to clients; both press and release are taken).
    pub fn media_key(&mut self, key: u32, pressed: bool) -> bool {
        let action: fn(&mut Self) = match key {
            keys::BRIGHTNESS_UP => |s| s.step_brightness(STEP),
            keys::BRIGHTNESS_DOWN => |s| s.step_brightness(-STEP),
            keys::VOLUME_UP => |s| s.volume_key(Job::Volume(STEP)),
            keys::VOLUME_DOWN => |s| s.volume_key(Job::Volume(-STEP)),
            keys::MUTE => |s| s.volume_key(Job::ToggleMute),
            _ => return false,
        };
        if pressed {
            action(self);
        }
        true
    }

    fn step_brightness(&mut self, delta: f32) {
        if let Some(level) = self.power.brightness {
            // Snap to the step grid so up then down returns exactly.
            let steps = ((level + delta) / STEP).round();
            let level = (steps * STEP).clamp(0.0, 1.0);
            self.set_brightness(level);
            self.show_indicator(crate::osd::Indicator::Brightness(level));
        }
    }

    fn volume_key(&mut self, job: Job) {
        if self.power.jobs.is_some() {
            self.power.send(job);
        } else if let Some((level, muted)) = self.power.volume {
            // Not the session: the sound server is left alone; pretend.
            let v = match job {
                Job::Volume(d) => (((level + d) / STEP).round() * STEP).clamp(0.0, 1.0),
                _ => level,
            };
            // A volume key unmutes, as the sound server does.
            let muted = match job {
                Job::ToggleMute => !muted,
                _ => false,
            };
            self.power.volume = Some((v, muted));
            info!(level = v, muted, "volume key (simulated)");
            self.show_indicator(crate::osd::Indicator::Volume { level: v, muted });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_simulates_a_backlight() {
        let p = Power::new(false);
        assert!(p.jobs.is_none() && p.backlight.is_none());
        assert_eq!(p.status().brightness, Some(0.75));
        assert!(!p.status().caffeinated);
    }
}
