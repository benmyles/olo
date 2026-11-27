//! Power: turning the displays off and sleeping when idle, caffeinated
//! mode, apps that ask to stay awake (zwp_idle_inhibit), the displays'
//! brightness (the built-in one's backlight; an external one's own kernel
//! backlight, as Apple silicon gives a Studio Display, or its USB control,
//! olo_system::hid_monitor) and the brightness / volume keys.
//! Each display starts at the brightness kept for it, or full.
//!
//! Hardware is only touched when app_server is the real session
//! (`Backend::IS_SESSION`); elsewhere (headless tests, nested) the backlight
//! is simulated and sleep is only logged. Everything that talks to the
//! system runs on a worker thread, never the compositor's.

use std::{
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_protocol::{Event, PowerStatus};
use smithay::reexports::{
    calloop,
    wayland_server::{Resource, protocol::wl_surface::WlSurface},
};
use tracing::{info, warn};

use crate::state::{AnvilState, Backend};

/// How often idleness is checked.
pub const TICK: Duration = Duration::from_secs(5);
/// Looking away this long, the built-in display dims (to [`DIM_TO`] of
/// its brightness): a glance aside doesn't.
const DIM_AFTER: Duration = Duration::from_secs(8);
const DIM_TO: f32 = 0.3;
/// One press of a brightness or volume key.
const STEP: f32 = 1.0 / 16.0;
/// After the displays change, how long to keep looking for their USB
/// brightness controls (USB can show up after the picture).
const LOOK_FOR: Duration = Duration::from_secs(30);

/// evdev codes of the keys handled here.
pub mod keys {
    pub const MUTE: u32 = 113;
    pub const VOLUME_DOWN: u32 = 114;
    pub const VOLUME_UP: u32 = 115;
    pub const BRIGHTNESS_DOWN: u32 = 224;
    pub const BRIGHTNESS_UP: u32 = 225;
    pub const KBD_TOGGLE: u32 = 228;
    pub const KBD_DOWN: u32 = 229;
    pub const KBD_UP: u32 = 230;
    /// A laptop's microphone key (a ThinkPad's F4).
    pub const MIC_MUTE: u32 = 248;
    /// The display key (a ThinkPad's F7): Preferences ▸ Display.
    pub const SWITCH_VIDEO_MODE: u32 = 227;
    /// The airplane key (a ThinkPad's F8, through Intel's HID events):
    /// every radio off, or back on.
    pub const RFKILL: u32 = 247;
    /// The notification centre key (a ThinkPad's F9): the notifications'
    /// history.
    pub const NOTIFICATION_CENTER: u32 = 444;
    /// Mission Control (an Apple keyboard's F3): the overview.
    pub const SCALE: u32 = 120;
    /// Spotlight (an Apple keyboard's F4): the command palette.
    pub const SEARCH: u32 = 217;
    /// A sleep key (logind's to act on but for Olo's inhibitor, held so
    /// a Magic Keyboard's Do Not Disturb key, which sends it, doesn't
    /// suspend the computer): asleep, as logind would.
    pub const SLEEP: u32 = 142;
}

enum Job {
    /// Hold an inhibitor (also against the lid switch with `lid`), or
    /// none.
    Caffeinate {
        on: bool,
        lid: bool,
    },
    Brightness(u32),
    KeyboardBacklight(u32),
    Suspend,
    Volume(f32),
    ToggleMute,
    /// The microphone key: mute the microphone in use, or unmute it.
    ToggleMicMute,
    /// The airplane key: every radio off, or back on.
    ToggleAirplane,
    /// Look for brightness controls for these connectors.
    FindDisplays(Vec<String>),
    /// An external display's brightness (by connector).
    DisplayBrightness(String, f32),
}

/// A display other than the built-in one whose brightness we set.
#[derive(Debug, Clone, PartialEq)]
struct External {
    output: String,
    level: f32,
}

/// Whether a connector is the built-in panel.
pub fn is_internal(connector: &str) -> bool {
    // (A test's stand-in for the built-in panel: `OLO_INTERNAL_OUTPUT`,
    // a headless screen's name.)
    static STAND_IN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if STAND_IN.get_or_init(|| std::env::var("OLO_INTERNAL_OUTPUT").ok().filter(|n| !n.is_empty())).as_deref() == Some(connector) {
        return true;
    }
    ["eDP", "LVDS", "DSI"].iter().any(|p| connector.starts_with(p))
}

/// The bottom of the built-in display's brightness is off (Ben,
/// 2026-11-12: "putting the screen on min brightness should turn the
/// screen off: right now it just gets dim but not off"), as a
/// MacBook's: the last step down switches the panel off, and it stays
/// off until its brightness is raised again (the key, a slider, an
/// agent), or until the displays have slept for idleness and someone
/// touches the computer again (`panel_back_on`). It isn't the display
/// asleep: nothing locks, its windows stay where they are, other
/// displays are untouched.
pub fn off_at(level: f32) -> bool {
    level <= 0.0
}

/// A display in HDR shows light as it's sent, and a panel's backlight
/// does nothing there (the ThinkPad's OLED ignored it): its brightness is
/// where SDR's white is, everything that isn't HDR, on even steps of
/// light (a log scale from `MIN_SDR_NITS`, level 0, to `MAX_SDR_NITS`,
/// level 1). The keys, a slider and an agent all move that, and it is
/// what the display's brightness is said to be (Ben, 2026-11-17: "the
/// brightness settings on my builtin monitor are broken i can't turn
/// them up or down ... i change it and then it just goes back to what
/// it was": in HDR Preferences' slider moved the backlight, which did
/// nothing, and then showed the level it had before).
pub fn sdr_level(nits: f32) -> f32 {
    use olo_protocol::color::{MAX_SDR_NITS, MIN_SDR_NITS};
    let (lo, hi) = (MIN_SDR_NITS.ln(), MAX_SDR_NITS.ln());
    ((nits.clamp(MIN_SDR_NITS, MAX_SDR_NITS).ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// [`sdr_level`] the other way: SDR's white at brightness `level`, in
/// whole cd/m².
pub fn sdr_nits_at(level: f32) -> f32 {
    use olo_protocol::color::{MAX_SDR_NITS, MIN_SDR_NITS};
    let (lo, hi) = (MIN_SDR_NITS.ln(), MAX_SDR_NITS.ln());
    (lo + (hi - lo) * level.clamp(0.0, 1.0)).exp().round()
}

/// Whether `output` is drawn in HDR now.
pub fn in_hdr(output: &smithay::output::Output) -> bool {
    matches!(crate::color::drawing(output), crate::color::Drawing::Hdr { .. })
}

/// A brightness kept as off isn't how a session starts: nobody should
/// sit down to a dark screen and not know why. The first step instead.
pub fn starting_level(kept: f32) -> f32 {
    if off_at(kept) { STEP } else { kept }
}

pub struct Power {
    pub policy: olo_protocol::power_mode::State,
    pub policy_tx: Option<Sender<crate::power_modes::Change>>,
    panel_before_saver: Option<smithay::output::Mode>,
    interactive_boost: bool,
    /// Dimmed by the power mode's dimming at rest (not by looking away):
    /// what leaving such a mode undoes.
    dimmed_at_rest: bool,
    last_activity: Instant,
    caffeinated: bool,
    /// When a timed caffeinated stretch ends.
    caffeinated_until: Option<SystemTime>,
    caffeinated_lid: bool,
    display_asleep: bool,
    /// The built-in display is switched off, its brightness at the
    /// bottom (`off_at`).
    panel_off: bool,
    /// Sleep was asked for in this idle stretch (don't ask again).
    slept: bool,
    inhibitors: Vec<WlSurface>,
    /// IPC connections keeping the screens awake while they stay open
    /// (`Request::KeepAwake`), and why.
    pub keep_awake: Vec<(std::rc::Rc<crate::ipc::Conn>, String)>,
    /// The real backlight (session only).
    backlight: Option<olo_system::backlight::Backlight>,
    /// Perceived brightness, 0–1 (simulated when there is no backlight to drive).
    pub brightness: Option<f32>,
    /// The keyboard's backlight, and its level (simulated likewise).
    keyboard: Option<olo_system::backlight::Backlight>,
    keyboard_level: Option<f32>,
    /// The level before a toggle turned it off.
    keyboard_was: f32,
    /// Simulated volume and mute (not the session), for the indicator.
    volume: Option<(f32, bool)>,
    /// The volume after a key, from the worker (session only); taken by
    /// state set-up.
    pub volume_feedback: Option<calloop::channel::Channel<crate::osd::Indicator>>,
    /// The microphone key's muting, pretended (headless: no sound server).
    pub mic_muted: bool,
    /// Airplane mode as simulated without a session (tests).
    pub airplane: bool,
    /// The presence sensor: the channel its watcher tells the loop on
    /// (the session's), the watcher's stop while it watches (only while
    /// a setting wants it), what it said last, and since when the person
    /// is away.
    pub presence_tx: Option<calloop::channel::Sender<olo_system::presence::Presence>>,
    pub presence_rx: Option<calloop::channel::Channel<olo_system::presence::Presence>>,
    presence_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub present: Option<olo_system::presence::Presence>,
    away_since: Option<Instant>,
    /// Since when the person in front looks away, and whether the built-in
    /// display is dimmed for it (`dim_when_looking_away`).
    looked_away_since: Option<Instant>,
    pub(crate) dimmed: bool,
    jobs: Option<Sender<Job>>,
    /// Displays found with a USB brightness control, and their levels
    /// (from the worker; taken by state set-up).
    pub displays_found: Option<calloop::channel::Channel<Vec<(String, f32)>>>,
    externals: Vec<External>,
    /// The connectors when we last looked, and since when they are so.
    looked: (Vec<String>, Instant),
    /// The built-in display has been set to its kept brightness.
    internal_set: bool,
    /// A brightness changed and is yet to be saved.
    unsaved: bool,
}

impl std::fmt::Debug for Power {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Power").field("status", &self.status()).finish()
    }
}

impl Power {
    pub fn new(is_session: bool) -> Power {
        let (presence_tx, presence_rx) = if is_session {
            let (tx, rx) = calloop::channel::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (keyboard, keyboard_level) = if is_session {
            let k = olo_system::backlight::keyboard();
            let level = k.as_ref().and_then(|k| k.level());
            (k, level)
        } else {
            (None, Some(0.5))
        };
        let (backlight, brightness, jobs, volume, volume_feedback, displays_found) = if is_session {
            let backlight = olo_system::backlight::internal();
            let brightness = backlight.as_ref().and_then(|b| b.level());
            let (tx, rx) = channel();
            let (feedback, feedback_rx) = calloop::channel::channel();
            let (found, found_rx) = calloop::channel::channel();
            std::thread::Builder::new().name("power".into()).spawn(move || worker(rx, feedback, found)).expect("thread");
            (backlight, brightness, Some(tx), None, Some(feedback_rx), Some(found_rx))
        } else {
            (None, Some(1.0), None, Some((0.5, false)), None, None)
        };
        Power {
            policy: if is_session { Default::default() } else { crate::power_modes::simulated() },
            policy_tx: None,
            panel_before_saver: None,
            interactive_boost: false,
            dimmed_at_rest: false,
            last_activity: Instant::now(),
            caffeinated: false,
            caffeinated_until: None,
            caffeinated_lid: false,
            display_asleep: false,
            panel_off: false,
            looked_away_since: None,
            dimmed: false,
            slept: false,
            inhibitors: Vec::new(),
            keep_awake: Vec::new(),
            backlight,
            brightness,
            keyboard,
            keyboard_level,
            keyboard_was: 0.5,
            volume,
            volume_feedback,
            mic_muted: false,
            airplane: false,
            presence_tx,
            presence_rx,
            presence_stop: None,
            present: None,
            away_since: None,
            jobs,
            displays_found,
            externals: Vec::new(),
            looked: (Vec::new(), Instant::now()),
            internal_set: !is_session,
            unsaved: false,
        }
    }

    /// The brightness of a display other than the built-in one, if we can
    /// set it.
    pub fn display_brightness(&self, connector: &str) -> Option<f32> {
        self.externals.iter().find(|e| e.output == connector).map(|e| e.level)
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }

    fn apply_brightness(&self, level: f32) {
        if let Some(b) = &self.backlight {
            self.send(Job::Brightness(olo_system::backlight::to_raw(level, b.max)));
        }
    }

    fn apply_keyboard(&self, level: f32) {
        if let Some(k) = &self.keyboard {
            self.send(Job::KeyboardBacklight(olo_system::backlight::to_raw(level, k.max)));
        }
    }

    /// For the app_server after this one (`handover`): caffeinated, until
    /// when (seconds since the epoch), and with the lid closed too.
    pub fn caffeine_handover(&self) -> (bool, Option<u64>, bool) {
        (self.caffeinated, self.caffeinated_until.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()), self.caffeinated_lid)
    }

    pub fn caffeinated(&self) -> bool {
        self.caffeinated
    }

    pub fn status(&self) -> PowerStatus {
        PowerStatus {
            policy: self.policy.clone(),
            caffeinated: self.caffeinated,
            idle_seconds: self.last_activity.elapsed().as_secs(),
            display_asleep: self.display_asleep,
            dimmed: self.dimmed,
            app_inhibitors: (self.inhibitors.len() + self.keep_awake.len()) as u32,
            brightness: self.brightness,
            panel_off: self.panel_off,
            screen_saver: false,
            caffeinated_until: self.caffeinated_until.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            caffeinated_lid: self.caffeinated_lid,
            keyboard_backlight: self.keyboard_level,
            locked: false,
            rescue: false,
            auto_approve_until: None,
        }
    }

    pub fn display_asleep(&self) -> bool {
        self.display_asleep
    }
}

/// What the worker keeps between jobs.
struct Worker {
    inhibitor: Option<olo_system::power::Inhibitor>,
    feedback: calloop::channel::Sender<crate::osd::Indicator>,
    found: calloop::channel::Sender<Vec<(String, f32)>>,
    /// External displays whose brightness we set, by connector.
    displays: std::collections::HashMap<String, Control>,
    /// Displays we could not read (said once).
    warned: std::collections::HashSet<std::path::PathBuf>,
}

fn worker(rx: Receiver<Job>, feedback: calloop::channel::Sender<crate::osd::Indicator>, found: calloop::channel::Sender<Vec<(String, f32)>>) {
    // The sleep key is Olo's (power.rs `keys::SLEEP`): a Magic Keyboard's
    // Do Not Disturb key sends it, and logind would suspend on it. Held
    // for the session (let go if app_server goes).
    let _sleep_key = olo_system::power::inhibit("handle-suspend-key", "Olo", "Olo handles the sleep key (a Magic Keyboard's Do Not Disturb sends it)", true)
        .map_err(|e| warn!("can't take the sleep key from logind: {e}"))
        .ok();
    // The power button too: logind shut down on a press; Olo does what
    // Preferences ▸ Keyboard says (power_button.rs). Held for the session.
    let _power_key =
        olo_system::power::inhibit("handle-power-key", "Olo", "Olo handles the power button (Preferences ▸ Keyboard)", true).map_err(|e| warn!("can't take the power button from logind: {e}")).ok();
    let mut w = Worker { inhibitor: None, feedback, found, displays: Default::default(), warned: Default::default() };
    while let Ok(mut job) = rx.recv() {
        // Only the newest brightness matters (a held key repeats).
        loop {
            let same = |a: &Job, b: &Job| match (a, b) {
                (Job::Brightness(_), Job::Brightness(_)) => true,
                (Job::DisplayBrightness(x, _), Job::DisplayBrightness(y, _)) => x == y,
                _ => false,
            };
            if !matches!(job, Job::Brightness(_) | Job::DisplayBrightness(..)) {
                break;
            }
            match rx.try_recv() {
                Ok(next) if same(&job, &next) => job = next,
                Ok(next) => {
                    run(job, &mut w);
                    job = next;
                }
                Err(_) => break,
            }
        }
        run(job, &mut w);
    }
}

/// How an external display's brightness is set.
enum Control {
    /// A backlight the kernel gives the connector (Apple silicon's display
    /// controller does for a Studio Display; the monitor's own USB control
    /// then takes the value but doesn't change the picture). Written
    /// through logind, as the built-in one's.
    Backlight(olo_system::backlight::Backlight),
    /// The monitor's USB brightness control.
    Hid(olo_system::hid_monitor::HidMonitor),
}

impl Control {
    fn set_level(&self, level: f32) -> Result<(), String> {
        match self {
            Control::Backlight(b) => b.set_level(level).map_err(|e| e.to_string()),
            Control::Hid(m) => m.set_level(level).map_err(|e| e.to_string()),
        }
    }
}

/// Find each connector's brightness control: a backlight of its own, else
/// a USB control matched by maker.
fn find_displays(
    displays: &mut std::collections::HashMap<String, Control>,
    warned: &mut std::collections::HashSet<std::path::PathBuf>,
    send: &calloop::channel::Sender<Vec<(String, f32)>>,
    connectors: Vec<String>,
) {
    let mut monitors = olo_system::hid_monitor::all();
    displays.clear();
    let mut found = Vec::new();
    for c in connectors {
        if let Some(b) = olo_system::backlight::for_connector(&c)
            && let Some(level) = b.level()
        {
            found.push((c.clone(), level));
            displays.insert(c, Control::Backlight(b));
            continue;
        }
        let Some(maker) = olo_system::edid::for_connector(&c).map(|e| e.maker) else { continue };
        let Some(i) = monitors.iter().position(|m| m.edid_vendor() == Some(maker.as_str())) else { continue };
        let m = monitors.remove(i);
        match m.level() {
            Ok(level) => {
                found.push((c.clone(), level));
                displays.insert(c, Control::Hid(m));
            }
            Err(e) => {
                if warned.insert(m.path.clone()) {
                    warn!(path = %m.path.display(), "can't read the display's brightness: {e} (is data/udev/70-olo-displays.rules installed?)");
                }
            }
        }
    }
    let _ = send.send(found);
}

fn run(job: Job, w: &mut Worker) {
    use olo_system::{audio, backlight, power};
    let Worker { inhibitor, feedback, found, displays, warned } = w;
    let volume_key = matches!(job, Job::Volume(_) | Job::ToggleMute);
    let r: Result<(), String> = match job {
        Job::Caffeinate { on: true, lid } => {
            *inhibitor = None;
            let what = if lid { "sleep:idle:handle-lid-switch" } else { "sleep:idle" };
            power::inhibit(what, "Olo", "Caffeinated", true).map(|i| *inhibitor = Some(i)).map_err(|e| e.to_string())
        }
        Job::Caffeinate { on: false, .. } => {
            *inhibitor = None;
            Ok(())
        }
        Job::Brightness(raw) => match backlight::internal() {
            Some(b) => b.set_raw(raw).map_err(|e| e.to_string()),
            None => Ok(()),
        },
        Job::KeyboardBacklight(raw) => match backlight::keyboard() {
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
        Job::ToggleMicMute => audio::toggle_input_mute().map(|muted| _ = feedback.send(crate::osd::Indicator::Microphone { muted })).map_err(|e| e.to_string()),
        Job::ToggleAirplane => olo_system::rfkill::toggle_airplane().map(|on| _ = feedback.send(crate::osd::Indicator::Airplane { on })).map_err(|e| e.to_string()),
        Job::FindDisplays(connectors) => {
            find_displays(displays, warned, found, connectors);
            Ok(())
        }
        Job::DisplayBrightness(c, level) => match displays.get(&c) {
            Some(m) => m.set_level(level).map_err(|e| format!("{c}: {e}")),
            None => Ok(()),
        },
    };
    if let Err(e) = r {
        warn!("power: {e}");
    }
    if volume_key && let Ok(v) = audio::volume() {
        let _ = feedback.send(crate::osd::Indicator::Volume { level: v.level, muted: v.muted });
    }
}

/// A screen nobody sits at (an agent's: `OLO_SCREEN`) is never idle: no
/// saver, no blanking, no sleep and no lock, whatever its settings say.
/// olo-screen writes them so, but the "agent" screen started its saver
/// five minutes after an in-place restart, then locked itself with the
/// person's lock screen, and stayed locked through every restart after
/// (2026-11-07). Its lock comes only when asked for (`Request::Lock`).
pub(crate) fn unattended() -> bool {
    static UNATTENDED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *UNATTENDED.get_or_init(|| std::env::var_os("OLO_SCREEN").is_some_and(|s| !s.is_empty()))
}

impl<B: Backend> AnvilState<B> {
    pub fn select_power_mode(&mut self, mode: olo_protocol::power_mode::Mode) -> Result<(), String> {
        if !self.power.policy.modes.as_ref().is_some_and(|m| m.available.contains(&mode)) {
            return Err("This power mode is unavailable".into());
        }
        if let Some(tx) = &self.power.policy_tx {
            return tx.send(crate::power_modes::Change::Select(mode)).map_err(|_| "The power service has stopped".into());
        }
        let mut policy = self.power.policy.clone();
        if let Some(m) = &mut policy.modes {
            m.active = mode;
        }
        self.apply_power_policy(policy);
        Ok(())
    }

    pub fn apply_power_policy(&mut self, policy: olo_protocol::power_mode::State) {
        self.power.policy = policy;
        self.apply_mode_choices();
        self.power_changed();
    }

    /// The power mode in use, if it is known.
    fn mode(&self) -> Option<olo_protocol::power_mode::Mode> {
        self.power.policy.modes.as_ref().map(|m| m.active)
    }

    /// Whether the mode in use keeps the built-in display to 60 Hz, dims
    /// it at rest, and favours the app in front after input: as the mode
    /// comes, or as the person chose for it (`PowerSettings::modes`).
    fn limits_refresh(&self) -> bool {
        self.mode().is_some_and(|m| self.settings.power.limits_refresh(m))
    }

    fn dims_at_rest(&self) -> bool {
        self.mode().is_some_and(|m| self.settings.power.dims_at_rest(m))
    }

    fn boosts_after_input(&self) -> bool {
        self.mode().is_some_and(|m| self.settings.power.boosts_after_input(m))
    }

    /// What the mode in use does in the session, put in force: after the
    /// mode changed, and after the person changed what a mode does.
    pub fn apply_mode_choices(&mut self) {
        if self.power.interactive_boost && !self.boosts_after_input() {
            self.power.interactive_boost = false;
            crate::realtime::burst(false);
        }
        // (It does nothing where the display is already as wanted.)
        self.apply_saver_refresh();
        if self.power.dimmed_at_rest && !self.dims_at_rest() {
            self.power.dimmed_at_rest = false;
            if self.power.dimmed {
                self.undim("the power mode no longer dims at rest");
            }
        }
    }

    /// Cap only the panel's refresh, at the chosen resolution. Its saved
    /// display mode always describes what the person wants after Saver.
    pub fn apply_saver_refresh(&mut self) {
        let Some(panel) = self.space.outputs().chain(self.mirrors.iter()).find(|o| is_internal(&o.name())).cloned() else {
            self.power.panel_before_saver = None;
            return;
        };
        let Some(now) = panel.current_mode() else { return };
        let want = if self.limits_refresh() {
            if now.refresh <= 60_000 {
                return;
            }
            let Some(capped) = panel.modes().into_iter().filter(|m| m.size == now.size && m.refresh <= 60_000).max_by_key(|m| m.refresh) else { return };
            self.power.panel_before_saver = Some(now);
            capped
        } else {
            let Some(previous) = self.power.panel_before_saver.take() else { return };
            if !panel.modes().contains(&previous) {
                return;
            }
            previous
        };
        if let Err(e) = self.backend_data.set_mode(&panel, want) {
            warn!(?e, "could not apply panel refresh policy");
        } else {
            self.backend_data.reset_buffers(&panel);
            self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        }
    }

    pub fn panel_mode_chosen(&mut self, mode: smithay::output::Mode) {
        self.power.panel_before_saver = self.limits_refresh().then_some(mode);
    }

    /// Watch the presence sensor while a setting wants it (locking when
    /// the person walks away, waking when they come back), not otherwise:
    /// the watcher waits on the sensor's buffer, which says only changes.
    pub fn presence_settings(&mut self) {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let s = &self.settings.power;
        let want = s.lock_when_away_seconds > 0 || s.wake_on_return || s.dim_when_looking_away;
        let p = &mut self.power;
        match (want, p.presence_stop.is_some()) {
            (true, false) => {
                let Some(tx) = p.presence_tx.clone() else { return };
                let stop = Arc::new(AtomicBool::new(false));
                let stopped = stop.clone();
                if olo_system::presence::watch(move |now| !stopped.load(Ordering::Relaxed) && tx.send(now).is_ok()).is_some() {
                    info!("watching the presence sensor");
                    p.presence_stop = Some(stop);
                } else {
                    info!("no presence sensor to watch (or Olo's rule isn't installed)");
                }
            }
            (false, true) => {
                if let Some(stop) = p.presence_stop.take() {
                    stop.store(true, Ordering::Relaxed);
                }
                (p.present, p.away_since, p.looked_away_since) = (None, None, None);
                if p.dimmed {
                    self.undim("dimming when looking away was turned off");
                }
            }
            _ => {}
        }
    }

    /// The sensor said where the person looks: the built-in display dimmed
    /// once they've looked away for [`DIM_AFTER`], back as they look again.
    fn attention_changed(&mut self, now: olo_system::presence::Presence) {
        let on = self.settings.power.dim_when_looking_away;
        if !(on && now.present && now.attentive == Some(false)) {
            self.power.looked_away_since = None;
            // (Gone, they come back to it dimmed; looking, it's back.)
            if self.power.dimmed && (now.attentive == Some(true) || !on) {
                self.undim("the person looks at the screen again");
            }
            return;
        }
        if self.power.looked_away_since.is_some() {
            return;
        }
        self.power.looked_away_since = Some(Instant::now());
        let _ = self.handle.insert_source(calloop::timer::Timer::from_duration(DIM_AFTER), move |_, _, state| {
            let p = &state.power;
            if p.looked_away_since.is_some_and(|t| t.elapsed() >= DIM_AFTER) && state.settings.power.dim_when_looking_away && !p.display_asleep && !p.dimmed {
                state.dim();
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    fn dim(&mut self) {
        let p = &mut self.power;
        p.dimmed = true;
        if let Some(level) = p.brightness {
            p.apply_brightness(level * DIM_TO);
        }
        self.dim_hdr_panel();
        info!("the person looks away: the display dimmed");
        self.power_changed();
    }

    fn dim_hdr_panel(&mut self) {
        let panel = self.space.outputs().find(|o| is_internal(&o.name()) && in_hdr(o)).cloned();
        if let Some(panel) = panel {
            if let Err(e) = self.apply_color(&panel) {
                warn!(?e, "could not adjust panel white");
            }
            self.backend_data.wake(Some(std::slice::from_ref(&panel)));
        }
    }

    fn undim(&mut self, why: &str) {
        let p = &mut self.power;
        p.dimmed = false;
        p.looked_away_since = None;
        if let Some(level) = p.brightness
            && !p.display_asleep
        {
            p.apply_brightness(level);
        }
        self.dim_hdr_panel();
        info!("{why}: the display's brightness back");
        self.power_changed();
    }

    /// The presence sensor said something new.
    pub fn presence_changed(&mut self, now: olo_system::presence::Presence) {
        let s = self.settings.power.clone();
        self.power.present = Some(now);
        self.attention_changed(now);
        if now.present {
            self.power.away_since = None;
            // (Back: the displays wake, to the lock screen if locked.)
            if s.wake_on_return && self.power.display_asleep {
                info!("the person came back: displays awake");
                self.power_activity();
            }
            return;
        }
        if self.power.away_since.is_some() {
            return;
        }
        self.power.away_since = Some(Instant::now());
        if s.lock_when_away_seconds > 0 {
            let after = Duration::from_secs(s.lock_when_away_seconds as u64);
            let _ = self.handle.insert_source(calloop::timer::Timer::from_duration(after), move |_, _, state| {
                // Still away since then: locked.
                if state.power.away_since.is_some_and(|t| t.elapsed() >= after) && state.settings.power.lock_when_away_seconds > 0 && !state.lock.locked() {
                    info!("the person walked away: locked");
                    state.lock_screen();
                }
                calloop::timer::TimeoutAction::Drop
            });
        }
    }

    pub fn power_status(&self) -> olo_protocol::PowerStatus {
        // (The built-in display in HDR: its brightness is SDR's white.)
        let hdr = self.space.outputs().find(|o| is_internal(&o.name()) && in_hdr(o)).map(|o| sdr_level(crate::color::applied(o).sdr_nits));
        olo_protocol::PowerStatus {
            brightness: hdr.or(self.power.brightness),
            screen_saver: self.saver.running(),
            locked: self.lock.locked(),
            rescue: self.rescue.is_some(),
            auto_approve_until: self.auto_approve_until(),
            ..self.power.status()
        }
    }

    pub fn power_changed(&mut self) {
        self.backend_data.wake(None);
        let status = self.power_status();
        self.ipc.broadcast(Event::PowerChanged { status });
    }

    /// One timer per interaction, with no per-input IPC. Move the deadline
    /// forward using the activity timestamp the idle policy already keeps.
    fn boost_interaction(&mut self) {
        if !B::IS_SESSION || self.power.interactive_boost || !self.boosts_after_input() {
            return;
        }
        const TAIL: Duration = Duration::from_millis(200);
        self.power.interactive_boost = true;
        crate::realtime::burst(true);
        let _ = self.handle.insert_source(calloop::timer::Timer::from_duration(TAIL), |_, _, state| {
            if !state.power.interactive_boost {
                return calloop::timer::TimeoutAction::Drop;
            }
            let elapsed = state.power.last_activity.elapsed();
            if elapsed < TAIL {
                return calloop::timer::TimeoutAction::ToDuration(TAIL - elapsed);
            }
            state.power.interactive_boost = false;
            crate::realtime::burst(false);
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// Any input: the user is here.
    pub fn power_activity(&mut self) {
        self.boost_interaction();
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
        let p = &mut self.power;
        p.last_activity = Instant::now();
        // (Whoever touches it now may not be who left it: below.)
        let came_back = p.display_asleep || p.slept;
        p.slept = false;
        // (Touching anything is looking: dimmed, it's back.)
        if p.dimmed {
            p.dimmed = false;
            p.looked_away_since = None;
            if let Some(level) = p.brightness
                && !p.display_asleep
            {
                p.apply_brightness(level);
            }
            self.dim_hdr_panel();
            info!("input: the display's brightness back");
            self.power_changed();
        }
        let p = &mut self.power;
        if p.display_asleep {
            p.display_asleep = false;
            if let Some(level) = p.brightness {
                p.apply_brightness(level);
            }
            if let Some(level) = p.keyboard_level {
                p.apply_keyboard(level);
            }
            info!("displays awake");
            self.backend_data.set_displays_on(true);
            self.power_changed();
        }
        if came_back {
            self.panel_back_on();
        }
        self.saver_activity();
    }

    /// The built-in display, switched off at the bottom of its
    /// brightness, back on at the first step. Off lasts while whoever
    /// chose it is there: once the displays have slept for idleness (or
    /// the computer was put to sleep for it: `slept`), the first touch
    /// brings the panel back with them, so nobody comes back to a dark
    /// screen and can't tell why. (A sleep the person asked for, the lid
    /// or the power button, isn't known here: it comes back as it was.)
    fn panel_back_on(&mut self) {
        if !self.power.panel_off {
            return;
        }
        let internal = self.space.outputs().find(|o| is_internal(&o.name())).cloned();
        match internal {
            Some(o) if in_hdr(&o) => self.step_sdr_white(&o, STEP),
            _ => self.set_brightness(STEP),
        }
    }

    /// Every [`TICK`]: turn displays off or sleep if idle long enough.
    pub fn power_tick(&mut self) {
        self.ensure_locker();
        self.power_displays_tick();
        self.saver_tick();
        let saver = self.settings.screen_saver.clone();
        if saver.never_fade.is_some() && self.pointer_corner() == saver.never_fade {
            self.power.last_activity = Instant::now();
        }
        let before = self.power.inhibitors.len() + self.power.keep_awake.len();
        self.power.inhibitors.retain(|s| s.is_alive());
        self.power.keep_awake.retain(|(c, _)| !c.is_dead());
        let pruned = self.power.inhibitors.len() + self.power.keep_awake.len() != before;
        self.auto_approve_tick();
        if self.power.caffeinated_until.is_some_and(|t| SystemTime::now() >= t) {
            info!("caffeinated time is up");
            self.set_caffeinated(false, None, false);
        }
        let settings = self.settings.power.clone();
        let dims_at_rest = self.dims_at_rest();
        let p = &mut self.power;
        let awake = p.caffeinated || !p.inhibitors.is_empty() || !p.keep_awake.is_empty() || unattended();
        if awake {
            p.last_activity = Instant::now();
        }
        // Programs watching for idle (ext-idle-notify) aren't told while
        // something keeps the machine awake.
        self.idle_notifier_state.set_is_inhibited(awake);
        let p = &mut self.power;
        let idle = p.last_activity.elapsed();
        let mut changed = pruned;
        let mut lock = false;
        let mut went_dark = false;
        let minutes = |m: u32| Duration::from_secs(m as u64 * 60);
        // On the lock screen nobody is working: displays go off after a minute.
        let (display_off, sleep) = settings.idle_minutes(p.policy.on_battery);
        let display_off = if self.lock.locked() { 1 } else { display_off };
        let saver_dim = dims_at_rest && !awake && !p.dimmed && !p.display_asleep && !p.panel_off && idle >= Duration::from_secs(30);
        if saver_dim {
            p.dimmed_at_rest = true;
            p.dimmed = true;
            if let Some(level) = p.brightness {
                p.apply_brightness(level * DIM_TO);
            }
            changed = true;
        }
        if display_off > 0 && idle >= minutes(display_off) && !p.display_asleep {
            p.display_asleep = true;
            p.send(Job::Brightness(0));
            if p.keyboard.is_some() {
                p.send(Job::KeyboardBacklight(0));
            }
            info!(?idle, "displays asleep");
            changed = true;
            went_dark = true;
            lock = settings.lock_on_display_off;
        }
        let display_asleep = p.display_asleep;
        if sleep > 0 && idle >= minutes(sleep) && !p.slept {
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
        if saver_dim {
            self.dim_hdr_panel();
        }
        if went_dark {
            self.backend_data.set_displays_on(false);
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

    /// The built-in display's brightness (kept for it).
    pub fn set_brightness(&mut self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        let internal = self.space.outputs().find(|o| is_internal(&o.name())).cloned();
        // In HDR its backlight does nothing: SDR's white, as the keys
        // move ([`sdr_level`]).
        if let Some(o) = internal.as_ref().filter(|o| in_hdr(o)) {
            self.set_sdr_level(&o.clone(), level);
            return;
        }
        let Some(_) = self.power.brightness else { return };
        self.power.brightness = Some(level);
        if !self.power.display_asleep {
            self.power.apply_brightness(level);
        }
        if let Some(o) = internal {
            self.keep_brightness(&o, level);
        }
        self.set_panel_off(off_at(level));
        self.power_changed();
        // A display's brightness is part of what's said of the displays
        // (`OutputInfo::brightness`): whoever shows it is told. (Untold,
        // Preferences' slider went back to the level it had before.)
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
    }

    /// The brightness of a display in HDR: SDR's white at `level`
    /// ([`sdr_nits_at`]), kept for the display; the built-in panel's
    /// bottom is off here too (in HDR its backlight isn't what's moved,
    /// and the least of SDR's white is still light).
    fn set_sdr_level(&mut self, output: &smithay::output::Output, level: f32) {
        let key = crate::desktop::display_key(output);
        self.settings.display.config_mut(&key, &output.name()).sdr_nits = Some(sdr_nits_at(level));
        if let Err(e) = self.apply_color(output) {
            warn!("SDR's white on {}: {e}", output.name());
        }
        // Saved on the next tick (keys repeat, a slider is dragged).
        self.power.unsaved = true;
        if is_internal(&output.name()) {
            self.set_panel_off(off_at(level));
            self.power_changed();
        }
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        self.remote_displays_changed();
    }

    /// The built-in display switched off (its brightness at the bottom),
    /// or back on.
    fn set_panel_off(&mut self, off: bool) {
        if self.power.panel_off == off {
            return;
        }
        let Some(o) = self.space.outputs().find(|o| is_internal(&o.name())).cloned() else { return };
        self.power.panel_off = off;
        info!(output = o.name(), off, "the built-in display, at the bottom of its brightness");
        self.backend_data.set_output_on(&o, !off);
    }

    /// SDR's white was set for `output` in cd/m² (Preferences'
    /// "Everything else's brightness"): in HDR that is its brightness, so
    /// the built-in panel, switched off at the bottom of it, is back on
    /// with any light above the least, and the power status says the
    /// level.
    pub fn sdr_white_set(&mut self, output: &smithay::output::Output) {
        if !is_internal(&output.name()) || !in_hdr(output) {
            return;
        }
        if !off_at(sdr_level(crate::color::applied(output).sdr_nits)) {
            self.set_panel_off(false);
        }
        self.power_changed();
    }

    /// Any display's brightness, by connector (kept for it).
    pub fn set_output_brightness(&mut self, connector: &str, level: f32) -> Result<(), String> {
        let level = level.clamp(0.0, 1.0);
        // In HDR, any display's is SDR's white: what the keys move.
        let hdr = self.space.outputs().chain(self.mirrors.iter()).find(|o| o.name() == connector && in_hdr(o)).cloned();
        if let Some(o) = hdr {
            self.set_sdr_level(&o, level);
            return Ok(());
        }
        if is_internal(connector) {
            self.set_brightness(level);
            return Ok(());
        }
        let Some(e) = self.power.externals.iter_mut().find(|e| e.output == connector) else {
            return Err(format!("{connector}'s brightness can't be set from here"));
        };
        e.level = level;
        self.power.send(Job::DisplayBrightness(connector.to_owned(), level));
        let output = self.space.outputs().chain(self.mirrors.iter()).find(|o| o.name() == connector).cloned();
        if let Some(o) = output {
            self.keep_brightness(&o, level);
        }
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        self.remote_displays_changed();
        Ok(())
    }

    fn keep_brightness(&mut self, o: &smithay::output::Output, level: f32) {
        let key = crate::desktop::display_key(o);
        self.settings.display.config_mut(&key, &o.name()).brightness = Some(level);
        // Saved on the next tick (keys repeat).
        self.power.unsaved = true;
    }

    /// Every tick: notice displays coming and going, look for their
    /// brightness controls, and give each its kept brightness.
    fn power_displays_tick(&mut self) {
        let mut names: Vec<String> = self.space.outputs().chain(self.mirrors.iter()).map(|o| o.name()).collect();
        names.sort();
        if names != self.power.looked.0 {
            self.power.externals.retain(|e| names.contains(&e.output));
            self.power.looked = (names.clone(), Instant::now());
            self.apply_saver_refresh();
        }
        let internal = self.space.outputs().find(|o| is_internal(&o.name())).cloned();
        if !self.power.internal_set
            && let Some(o) = internal
        {
            self.power.internal_set = true;
            let level = starting_level(self.settings.display.brightness_for(&crate::desktop::display_key(&o), &o.name()));
            if self.power.brightness.is_some() {
                info!(level, "built-in display brightness");
                self.power.brightness = Some(level);
                self.power.apply_brightness(level);
                self.power_changed();
            }
        }
        if self.power.looked.1.elapsed() <= LOOK_FOR + TICK {
            let external: Vec<String> = names.into_iter().filter(|n| !is_internal(n)).collect();
            if self.power.jobs.is_some() {
                self.power.send(Job::FindDisplays(external));
            } else {
                // Not the session: every display pretends to have one.
                self.displays_found(external.into_iter().map(|n| (n, 1.0)).collect());
            }
        }
        if std::mem::take(&mut self.power.unsaved) {
            match &mut self.rescue {
                // In Rescue, into the person's own settings.
                Some(r) => {
                    for (key, c) in &self.settings.display.outputs {
                        if c.brightness.is_some() {
                            r.real.display.outputs.entry(key.clone()).or_default().brightness = c.brightness;
                        }
                    }
                    crate::desktop::save_settings(&r.real);
                }
                None => crate::desktop::save_settings(&self.settings),
            }
        }
    }

    /// The worker found these USB brightness controls (connector, level).
    pub fn displays_found(&mut self, found: Vec<(String, f32)>) {
        let mut changed = false;
        for (name, current) in found {
            if self.power.externals.iter().any(|e| e.output == name) {
                continue;
            }
            let Some(o) = self.space.outputs().chain(self.mirrors.iter()).find(|o| o.name() == name).cloned() else { continue };
            let level = self.settings.display.brightness_for(&crate::desktop::display_key(&o), &name);
            info!(display = name, current, level, "display brightness");
            if (level - current).abs() > 0.005 {
                self.power.send(Job::DisplayBrightness(name.clone(), level));
            }
            self.power.externals.push(External { output: name, level });
            changed = true;
        }
        if changed {
            self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
            self.remote_displays_changed();
        }
    }

    /// Keep the screens awake while `conn` stays open (`KeepAwake`).
    pub fn keep_awake(&mut self, conn: &std::rc::Rc<crate::ipc::Conn>, why: String) {
        info!(why, "kept awake by a program");
        self.power.keep_awake.push((conn.clone(), why));
        self.power_activity();
        self.power_changed();
    }

    /// An IPC connection closed: what it kept awake isn't.
    pub fn ipc_closed(&mut self, conn: &std::rc::Rc<crate::ipc::Conn>) {
        self.grabs_closed(conn);
        self.input_closed(conn);
        let before = self.power.keep_awake.len();
        self.power.keep_awake.retain(|(c, _)| !std::rc::Rc::ptr_eq(c, conn));
        if self.power.keep_awake.len() != before {
            self.power_changed();
        }
    }

    /// A connection that owned its synthetic input ended
    /// (`Request::OwnInput`): what it still had down is let go, the last
    /// pressed first, as its own letting go would have been taken.
    pub fn input_closed(&mut self, conn: &std::rc::Rc<crate::ipc::Conn>) {
        let Some(at) = self.ipc.owned_input.iter().position(|(who, _)| std::rc::Rc::ptr_eq(who, conn)) else { return };
        let (_, held) = self.ipc.owned_input.swap_remove(at);
        if !held.is_empty() {
            info!(held = held.len(), "a connection that owned its input ended with it down: let go");
        }
        for what in held.into_iter().rev() {
            self.synthetic_input(what.release());
        }
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
            keys::MIC_MUTE => |s| s.volume_key(Job::ToggleMicMute),
            keys::RFKILL => |s| s.volume_key(Job::ToggleAirplane),
            keys::SWITCH_VIDEO_MODE => |s| s.spawn_program("olo-preferences", &["display"]),
            keys::NOTIFICATION_CENTER => |s| s.spawn_program("olo-notifications", &["--history"]),
            keys::SCALE => |s| s.toggle_overview(),
            keys::SEARCH => |s| s.spawn_program("olo-palette", &["--toggle"]),
            keys::SLEEP => |s| {
                if s.power.jobs.is_some() {
                    s.power.send(Job::Suspend);
                } else {
                    info!("sleep key (simulated: not the session)");
                }
            },
            keys::KBD_UP => |s| s.step_keyboard(STEP),
            keys::KBD_DOWN => |s| s.step_keyboard(-STEP),
            keys::KBD_TOGGLE => |s| {
                let Some(level) = s.power.keyboard_level else { return };
                let next = if level > 0.0 {
                    s.power.keyboard_was = level;
                    0.0
                } else {
                    s.power.keyboard_was.max(STEP)
                };
                s.set_keyboard_backlight(next);
                s.show_indicator(crate::osd::Indicator::KeyboardBacklight(next));
            },
            _ => return false,
        };
        if pressed {
            action(self);
        }
        true
    }

    /// Do Not Disturb on or off (a Magic Keyboard's key), as Deskbar's
    /// menu does it: in the notifications' settings, which
    /// olo-notifications follows; written off the loop.
    pub fn toggle_do_not_disturb(&mut self) {
        use olo_protocol::notifications::NotificationSettings;
        let mut n = NotificationSettings::load();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        // (By hand, as Deskbar's menu: a schedule's quiet hours stay.)
        let on = !(n.dnd && n.dnd_until.is_none_or(|t| now < t));
        n.dnd = on;
        n.dnd_until = None;
        std::thread::spawn(move || {
            if let Err(e) = n.save() {
                warn!("can't keep Do Not Disturb: {e}");
            }
        });
        self.show_indicator(crate::osd::Indicator::DoNotDisturb { on });
    }

    /// The brightness keys: the display under the pointer if we can set
    /// its brightness, else the built-in one. A display in HDR has its
    /// SDR white moved instead (`step_sdr_white`).
    fn step_brightness(&mut self, delta: f32) {
        // Snap to the step grid so up then down returns exactly.
        let step = |level: f32| (((level + delta) / STEP).round() * STEP).clamp(0.0, 1.0);
        let pointer = self.pointer.current_location();
        let under_output = self.space.output_under(pointer).next().cloned();
        let internal = self.space.outputs().find(|o| is_internal(&o.name())).cloned();
        let hdr = in_hdr;
        if let Some(o) = under_output.as_ref().filter(|o| hdr(o)) {
            return self.step_sdr_white(&o.clone(), delta);
        }
        let under = under_output.map(|o| o.name());
        if under.as_deref().is_none_or(|n| self.power.display_brightness(n).is_none())
            && let Some(o) = internal.filter(|o| hdr(o))
        {
            return self.step_sdr_white(&o, delta);
        }
        if let Some(name) = under
            && let Some(level) = self.power.display_brightness(&name)
        {
            let level = step(level);
            let _ = self.set_output_brightness(&name, level);
            self.show_indicator(crate::osd::Indicator::Brightness(level));
        } else if let Some(level) = self.power.brightness {
            let level = step(level);
            self.set_brightness(level);
            self.show_indicator(crate::osd::Indicator::Brightness(level));
        }
    }

    /// In HDR a display shows light as it's sent, and the built-in
    /// panel's backlight does nothing (the ThinkPad's OLED ignored it):
    /// the brightness keys move SDR's white instead, everything that isn't
    /// HDR, as Preferences' "Everything else's brightness" does, by even
    /// steps of light (a log scale from `MIN_SDR_NITS` to `MAX_SDR_NITS`),
    /// shown on the brightness indicator and kept for the display.
    fn step_sdr_white(&mut self, output: &smithay::output::Output, delta: f32) {
        let level = sdr_level(crate::color::applied(output).sdr_nits);
        let level = (((level + delta) / STEP).round() * STEP).clamp(0.0, 1.0);
        self.set_sdr_level(output, level);
        self.show_indicator(crate::osd::Indicator::Brightness(level));
    }

    fn step_keyboard(&mut self, delta: f32) {
        if let Some(level) = self.power.keyboard_level {
            let level = (((level + delta) / STEP).round() * STEP).clamp(0.0, 1.0);
            self.set_keyboard_backlight(level);
            self.show_indicator(crate::osd::Indicator::KeyboardBacklight(level));
        }
    }

    pub fn set_keyboard_backlight(&mut self, level: f32) {
        let Some(_) = self.power.keyboard_level else { return };
        let level = level.clamp(0.0, 1.0);
        self.power.keyboard_level = Some(level);
        if !self.power.display_asleep {
            self.power.apply_keyboard(level);
        }
        self.power_changed();
    }

    fn volume_key(&mut self, job: Job) {
        if self.power.jobs.is_some() {
            self.power.send(job);
        } else if matches!(job, Job::ToggleMicMute) {
            // Not the session: pretend.
            self.power.mic_muted = !self.power.mic_muted;
            info!(muted = self.power.mic_muted, "microphone key (simulated)");
            self.show_indicator(crate::osd::Indicator::Microphone { muted: self.power.mic_muted });
        } else if matches!(job, Job::ToggleAirplane) {
            // Not the session: the radios are left alone; pretend.
            self.power.airplane = !self.power.airplane;
            info!(on = self.power.airplane, "airplane key (simulated)");
            self.show_indicator(crate::osd::Indicator::Airplane { on: self.power.airplane });
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
        // Full, like every display with no level kept.
        assert_eq!(p.status().brightness, Some(1.0));
        assert!(!p.status().caffeinated);
        assert!(!p.status().panel_off);
    }

    #[test]
    fn the_bottom_of_the_brightness_is_off_and_a_session_never_starts_there() {
        assert!(off_at(0.0));
        assert!(!off_at(STEP));
        // (Sixteen steps down from full land on it exactly.)
        let mut level = 1.0_f32;
        for _ in 0..16 {
            level = (((level - STEP) / STEP).round() * STEP).clamp(0.0, 1.0);
        }
        assert!(off_at(level));
        assert_eq!(starting_level(0.0), STEP);
        assert_eq!(starting_level(0.5), 0.5);
        assert_eq!(starting_level(STEP), STEP);
    }

    /// In HDR a display's brightness is SDR's white, on even steps of
    /// light: the bottom its least, the top its most, and a level and
    /// its light say each other.
    #[test]
    fn brightness_in_hdr_is_sdrs_white() {
        use olo_protocol::color::{MAX_SDR_NITS, MIN_SDR_NITS};
        assert_eq!((sdr_nits_at(0.0), sdr_nits_at(1.0)), (MIN_SDR_NITS, MAX_SDR_NITS));
        assert_eq!((sdr_level(MIN_SDR_NITS), sdr_level(MAX_SDR_NITS)), (0.0, 1.0));
        assert_eq!((sdr_level(1.0), sdr_level(5000.0)), (0.0, 1.0));
        // Each of the keys' sixteen steps is its own light, more than the
        // last by the same share, and comes back as the step it was.
        let mut last = 0.0;
        for k in 0..=16 {
            let level = k as f32 * STEP;
            let nits = sdr_nits_at(level);
            assert!(nits > last, "step {k}: {nits} after {last}");
            assert!((sdr_level(nits) - level).abs() < 0.004, "step {k}: {nits} cd/m² is {}", sdr_level(nits));
            last = nits;
        }
        // A slider's 64 steps come back within half a step (cd/m² are whole).
        for k in 0..=64 {
            let level = k as f32 / 64.0;
            assert!((sdr_level(sdr_nits_at(level)) - level).abs() < 0.008, "{level}");
        }
        assert!(off_at(sdr_level(MIN_SDR_NITS)) && !off_at(sdr_level(sdr_nits_at(STEP))));
    }

    #[test]
    fn built_in_panels() {
        assert!(is_internal("eDP-1") && is_internal("LVDS-1") && is_internal("DSI-1"));
        assert!(!is_internal("DP-1") && !is_internal("HDMI-A-1") && !is_internal("HEADLESS-1"));
    }
}
