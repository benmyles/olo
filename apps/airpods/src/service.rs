//! The service: follows BlueZ for AirPods paired with this computer,
//! talks to each pair connected over Apple's accessory protocol (one
//! thread each, asleep on its channel until they say something), follows
//! the sound server for how their sound is carried, and publishes all of
//! it (`AirPodsState`) for Deskbar, Preferences and agents. Asked things
//! on its socket (`Request`). What it does for the person, by Olo's
//! settings (`AirPodsSettings`): pauses what plays when a bud comes out
//! and plays it again when it's back, turns the sound down while the
//! wearer speaks, and keeps recording from the computer's microphones
//! (so the AirPods stay at music quality) unless told otherwise.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use olo_protocol::airpods::{self as ap, AirPods, AirPodsSettings, AirPodsState, BudMicrophone, Change, Charge, ListeningMode, MicrophoneChoice, Reply, Request};
use olo_sync::Mutex;
use olo_system::{
    airpods::{
        self as sys, Model,
        link::Link,
        proto::{self, Message, Part, Placement, Setting},
    },
    audio, bluetooth as bt, media,
};

/// How long after a bud comes out what was paused is still played again
/// when it goes back in.
const RESUME_WITHIN: Duration = Duration::from_secs(10 * 60);
/// The sound while the wearer speaks, at the AirPods' lowest level (of 8).
const SPEAKING_FLOOR: f32 = 0.25;

/// One pair, and what we keep of it besides what's published.
#[derive(Default)]
struct Dev {
    info: AirPods,
    link: Option<Arc<Link>>,
    /// A thread is opening the channel (or talking over it).
    linking: bool,
    placement: Option<(Placement, Placement)>,
    /// Players paused when a bud came out, when, and how many buds were in.
    paused: Vec<String>,
    paused_at: Option<(Instant, u8)>,
    /// The output's level before Conversation Awareness turned it down.
    before_speaking: Option<(String, f32, usize)>,
    /// Their microphone stream is running, and Conversation Awareness's
    /// setting to put back when it stops (it's off while recording, as
    /// in a call on an iPhone: speaking would duck the sound).
    mic_on: bool,
    awareness_after: Option<bool>,
    /// Their broadcast: last heard; since when it has said they're
    /// connected to nothing.
    heard_at: Option<Instant>,
    free_since: Option<Instant>,
    /// When they last stopped being connected here (another device took
    /// them, or the person disconnected them): nothing is taken back
    /// automatically until they've been free a minute (`may_take`).
    lost_at: Option<Instant>,
    /// When Olo last asked to connect them by itself.
    tried_at: Option<Instant>,
}

struct Inner {
    devices: BTreeMap<String, Dev>,
    settings: AirPodsSettings,
    published: Option<AirPodsState>,
    /// The inputs there were last time, and the last one chosen that
    /// wasn't AirPods' (to go back to).
    inputs: Vec<String>,
    computer_input: Option<String>,
    /// The sound server's devices have been seen once: what comes after
    /// is new (an output there at the service's start was already there).
    sound_seen: bool,
    fake: bool,
    /// The broadcasts are heard; if not, why.
    listening: bool,
    not_listening: Option<String>,
    /// Their broadcast keys, by address (keys.rs), and when last read.
    keys: BTreeMap<String, sys::proto::Keys>,
    keys_read: Option<Instant>,
    /// Something is playing on this computer.
    playing: bool,
    /// A thread is waiting to forget pairs not heard from in a while.
    expiring: bool,
}

struct Service {
    inner: Mutex<Inner>,
}

fn service() -> &'static Service {
    static S: OnceLock<Service> = OnceLock::new();
    S.get_or_init(|| Service {
        inner: Mutex::new(Inner {
            devices: BTreeMap::new(),
            settings: AirPodsSettings::load(),
            published: None,
            inputs: Vec::new(),
            computer_input: None,
            sound_seen: false,
            fake: std::env::var_os("OLO_FAKE_AIRPODS").is_some(),
            listening: false,
            not_listening: None,
            keys: BTreeMap::new(),
            keys_read: None,
            playing: false,
            expiring: false,
        }),
    })
}

/// Publish the state if it changed (the lock held by the caller).
fn publish(inner: &mut Inner) {
    let state = AirPodsState { pid: std::process::id(), devices: inner.devices.values().map(|d| d.info.clone()).collect(), listening: inner.listening, not_listening: inner.not_listening.clone() };
    if inner.published.as_ref() == Some(&state) {
        return;
    }
    if let Err(e) = state.save() {
        tracing::warn!("airpods: couldn't publish: {e}");
    }
    inner.published = Some(state);
}

/// Run the service; never returns.
pub fn run() -> ! {
    let s = service();
    let fake = s.inner.lock().unwrap().fake;
    if fake {
        fake_pair();
    } else {
        refresh();
        if let Err(e) = bt::watch(refresh) {
            tracing::warn!("airpods: can't follow BlueZ: {e}");
        }
        std::thread::Builder::new().name("airpods-sound".into()).spawn(|| audio::watch_devices(sound_changed)).expect("thread");
        listen();
        std::thread::Builder::new().name("airpods-playing".into()).spawn(|| audio::watch_playing(playing_changed)).expect("thread");
    }
    follow_settings();
    serve()
}

// ---- Their broadcast: connecting and switching by themselves -----------------

/// How long they must have been free (connected to nothing, by their
/// broadcast) after another device took them, before Olo takes them by
/// itself again: never a tug of war with a phone.
const FREE_FOR: Duration = Duration::from_secs(60);
/// At most one try to connect by itself this often.
const TRY_EVERY: Duration = Duration::from_secs(15);
/// Not heard for this long: not nearby any more.
const NEARBY_FOR: Duration = Duration::from_secs(60);

/// Whether Olo may take them by itself now (see `lost_at`).
fn may_take(d: &mut Dev) -> bool {
    let free = d.free_since.is_some_and(|t| t.elapsed() >= FREE_FOR);
    if d.lost_at.is_some() && free {
        d.lost_at = None;
    }
    d.lost_at.is_none() && d.tried_at.is_none_or(|t| t.elapsed() >= TRY_EVERY)
}

/// Ask BlueZ to connect them, by itself (on a thread: it can take seconds).
fn take(d: &mut Dev, why: &str) {
    d.tried_at = Some(Instant::now());
    let (path, name) = (d.info.path.clone(), d.info.name.clone());
    tracing::info!("airpods: connecting {name}: {why}");
    std::thread::spawn(move || {
        if let Err(e) = bt::connect(&path) {
            tracing::info!("airpods: couldn't connect {name}: {e}");
        }
    });
}

/// Listen for their broadcasts (BlueZ's advertisement monitor).
fn listen() {
    match bt::watch_apple(sys::adv::PROXIMITY, heard_broadcast) {
        Ok(conn) => {
            // For as long as the service runs.
            std::mem::forget(conn);
            let mut inner = service().inner.lock().unwrap();
            (inner.listening, inner.not_listening) = (true, None);
            publish(&mut inner);
        }
        Err(e) => {
            tracing::info!("airpods: not listening for their broadcasts: {e}");
            let mut inner = service().inner.lock().unwrap();
            (inner.listening, inner.not_listening) = (false, Some(e.to_string()));
            publish(&mut inner);
        }
    }
}

/// A proximity broadcast: whose (their private address, resolved with
/// each pair's key), where they are and their levels; then connect or
/// switch by themselves, as the settings say.
fn heard_broadcast(advert: bt::Advert) {
    let Some(p) = sys::adv::parse(&advert.data) else { return };
    let mut inner = service().inner.lock().unwrap();
    let resolve = |keys: &BTreeMap<String, sys::proto::Keys>| keys.iter().find(|(_, k)| k.irk.is_some_and(|irk| sys::adv::resolves(&advert.address, &irk))).map(|(a, k)| (a.clone(), k.clone()));
    let mut found = resolve(&inner.keys);
    if found.is_none() && inner.keys_read.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
        // Keys learnt since (they come over the channel once connected).
        inner.keys = crate::keys::all();
        inner.keys_read = Some(Instant::now());
        found = resolve(&inner.keys);
    }
    let Some((addr, keys)) = found else { return };
    let settings = inner.settings.clone();
    let playing = inner.playing;
    let Some(d) = inner.devices.get_mut(&addr) else { return };
    let now = Instant::now();
    d.heard_at = Some(now);
    if p.connected {
        d.free_since = None;
    } else {
        d.free_since.get_or_insert(now);
    }
    let i = &mut d.info;
    i.nearby = true;
    i.elsewhere = p.connected && !i.connected;
    if !i.connected {
        let [left, right, case] = match keys.encryption {
            Some(k) => sys::adv::decrypt(&p, &k),
            None => [p.left, p.right, p.case],
        };
        let level = |c: sys::adv::Charge| c.percent.map(|percent| Charge { percent, charging: c.charging });
        (i.left, i.right, i.case) = (level(left), level(right), level(case));
        i.in_ear = p.left_in_ear as u8 + p.right_in_ear as u8;
    }
    let in_ear = p.left_in_ear || p.right_in_ear;
    if !d.info.connected && in_ear && may_take(d) {
        if settings.connect_automatically && !p.connected {
            take(d, "put in an ear near this computer, connected to nothing");
        } else if settings.switch_automatically && p.connected && playing {
            take(d, "something plays here while they're in an ear");
        }
    }
    publish(&mut inner);
    expire_later(&mut inner);
}

/// Something started or stopped playing on this computer: in ears and on
/// another device, they're taken (switch automatically).
fn playing_changed(playing: Option<bool>) {
    let mut inner = service().inner.lock().unwrap();
    let started = playing == Some(true) && !inner.playing;
    inner.playing = playing == Some(true);
    if !started || !inner.settings.switch_automatically {
        return;
    }
    for d in inner.devices.values_mut() {
        let fresh = d.heard_at.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
        if fresh && !d.info.connected && d.info.elsewhere && d.info.in_ear > 0 && may_take(d) {
            take(d, "something plays here while they're in an ear");
        }
    }
}

/// While any pair is nearby, a thread waits to mark them gone once their
/// broadcast stops (the case closed, carried away): asleep otherwise.
fn expire_later(inner: &mut Inner) {
    if inner.expiring {
        return;
    }
    inner.expiring = true;
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(NEARBY_FOR / 2);
            let mut inner = service().inner.lock().unwrap();
            let mut any = false;
            for d in inner.devices.values_mut() {
                if d.info.nearby && d.heard_at.is_none_or(|t| t.elapsed() >= NEARBY_FOR) {
                    d.info.nearby = false;
                    d.info.elsewhere = false;
                    if !d.info.connected {
                        (d.info.left, d.info.right, d.info.case, d.info.in_ear) = (None, None, None, 0);
                    }
                }
                any |= d.info.nearby;
            }
            publish(&mut inner);
            if !any {
                inner.expiring = false;
                return;
            }
        }
    });
}

// ---- BlueZ --------------------------------------------------------------------

/// Read BlueZ again: AirPods paired, connected or not; open the channel
/// to those newly connected.
fn refresh() {
    let status = match bt::status() {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("airpods: BlueZ: {e}");
            return;
        }
    };
    let mut start = Vec::new();
    {
        let mut inner = service().inner.lock().unwrap();
        let ours: Vec<&bt::Device> = status.devices.iter().filter(|d| d.paired && d.modalias.as_deref().is_some_and(sys::is_airpods)).collect();
        inner.devices.retain(|addr, d| {
            let keep = ours.iter().any(|o| &o.address == addr);
            if !keep && let Some(l) = &d.link {
                l.close();
            }
            keep
        });
        for o in ours {
            let product = o.modalias.as_deref().and_then(sys::ids).map_or(0, |(_, p)| p);
            let model = Model::of(product);
            let d = inner.devices.entry(o.address.clone()).or_default();
            let i = &mut d.info;
            i.address = o.address.clone();
            i.path = o.path.clone();
            i.name = o.name.clone();
            i.product = product;
            i.model = model.name.to_owned();
            i.abilities = ap::Abilities { noise_control: model.noise_control, adaptive: model.adaptive, buds: model.buds };
            if i.connected && !o.connected {
                d.lost_at = Some(Instant::now());
            }
            i.connected = o.connected;
            if o.connected {
                i.elsewhere = false;
            }
            if o.connected && !d.linking {
                d.linking = true;
                start.push(o.address.clone());
            }
            if !o.connected {
                if let Some(l) = d.link.take() {
                    l.close();
                }
                forget_live(d);
            }
        }
        publish(&mut inner);
    }
    for addr in start {
        std::thread::Builder::new().name("airpods-link".into()).spawn(move || talk(addr)).expect("thread");
    }
}

/// What's only true while connected.
fn forget_live(d: &mut Dev) {
    let i = &mut d.info;
    i.linked = false;
    i.left = None;
    i.right = None;
    i.case = None;
    i.whole = None;
    i.in_ear = 0;
    i.in_case = 0;
    i.speaking = false;
    d.placement = None;
}

fn still_connected(addr: &str) -> bool {
    service().inner.lock().unwrap().devices.get(addr).is_some_and(|d| d.info.connected)
}

/// Talk to one pair for as long as they're connected.
fn talk(addr: String) {
    // Their channel opens once their profiles are up, a moment after the
    // connection: tried again a few times.
    const WAITS: [u64; 8] = [300, 500, 1000, 1500, 2000, 3000, 5000, 8000];
    let mut failures = 0;
    'again: while still_connected(&addr) {
        let mut link = None;
        for wait in WAITS {
            match Link::connect(&addr, proto::PSM, Duration::from_secs(5)) {
                Ok(l) => {
                    link = Some(Arc::new(l));
                    break;
                }
                Err(e) => {
                    tracing::debug!("airpods {addr}: channel: {e}");
                    std::thread::sleep(Duration::from_millis(wait));
                    if !still_connected(&addr) {
                        break 'again;
                    }
                }
            }
        }
        let Some(link) = link else {
            tracing::warn!("airpods {addr}: they don't answer on their accessory channel");
            break;
        };
        let opened = Instant::now();
        // The hello first, and the rest only once they've answered it:
        // asked for notifications before then, they never send their
        // levels or where the buds are (seen on a reconnection).
        let mut rest = vec![proto::host_features(), proto::ask_notifications(), proto::ask_keys()];
        // What they don't say unasked and forget for other hosts: the
        // modes press and hold goes through, as last chosen here.
        if let Some(modes) = crate::keys::press_and_hold(&addr)
            && let Ok(m) = messages(&Change::PressAndHoldModes(modes))
        {
            rest.extend(m);
        }
        let hello = [proto::hello()];
        if let Err(e) = hello.iter().try_for_each(|m| link.send(m)) {
            tracing::warn!("airpods {addr}: {e}");
        } else {
            {
                let mut inner = service().inner.lock().unwrap();
                if let Some(d) = inner.devices.get_mut(&addr) {
                    d.link = Some(link.clone());
                    d.info.linked = true;
                    // Apple's own until they say otherwise (or Olo sent its).
                    d.info.settings.press_and_hold_modes = crate::keys::press_and_hold(&addr).unwrap_or(vec![ListeningMode::Transparency, ListeningMode::NoiseCancellation]);
                }
                publish(&mut inner);
            }
            tracing::info!("airpods {addr}: talking");
            let name = service().inner.lock().unwrap().devices.get(&addr).map(|d| d.info.name.clone()).unwrap_or_default();
            crate::mic::linked(&addr, &name);
            loop {
                match link.recv() {
                    Ok(m) if m.is_empty() => break,
                    Ok(m) => {
                        tracing::trace!("airpods {addr}: heard {}", hex(&m));
                        if proto::is_hello_answer(&m) && !rest.is_empty() {
                            if let Err(e) = rest.drain(..).try_for_each(|r| link.send(&r)) {
                                tracing::warn!("airpods {addr}: {e}");
                                break;
                            }
                            continue;
                        }
                        if let Some(msg) = proto::parse(&m) {
                            heard(&addr, msg);
                        } else {
                            tracing::debug!("airpods {addr}: {}", hex(&m));
                        }
                    }
                    Err(e) => {
                        tracing::debug!("airpods {addr}: {e}");
                        break;
                    }
                }
            }
        }
        crate::mic::unlinked(&addr);
        {
            let mut inner = service().inner.lock().unwrap();
            if let Some(d) = inner.devices.get_mut(&addr) {
                d.link = None;
                d.mic_on = false;
                d.awareness_after = None;
                forget_live(d);
            }
            publish(&mut inner);
        }
        // Closed under us while still connected: open it again, unless it
        // keeps closing at once.
        failures = if opened.elapsed() < Duration::from_secs(5) { failures + 1 } else { 0 };
        if failures >= 3 {
            tracing::warn!("airpods {addr}: their channel keeps closing");
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if let Some(d) = service().inner.lock().unwrap().devices.get_mut(&addr) {
        d.linking = false;
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

fn charge(l: &proto::Level) -> Option<Charge> {
    Some(Charge { percent: l.percent, charging: l.charging })
}

fn modes_of(mask: u8) -> Vec<ListeningMode> {
    ListeningMode::ALL.into_iter().filter(|m| mask & to_proto(*m).bit() != 0).collect()
}

fn to_proto(m: ListeningMode) -> proto::ListeningMode {
    match m {
        ListeningMode::Off => proto::ListeningMode::Off,
        ListeningMode::Transparency => proto::ListeningMode::Transparency,
        ListeningMode::Adaptive => proto::ListeningMode::Adaptive,
        ListeningMode::NoiseCancellation => proto::ListeningMode::NoiseCancellation,
    }
}

fn from_proto(m: proto::ListeningMode) -> ListeningMode {
    match m {
        proto::ListeningMode::Off => ListeningMode::Off,
        proto::ListeningMode::Transparency => ListeningMode::Transparency,
        proto::ListeningMode::Adaptive => ListeningMode::Adaptive,
        proto::ListeningMode::NoiseCancellation => ListeningMode::NoiseCancellation,
    }
}

/// What a message from the AirPods changes.
fn heard(addr: &str, msg: Message) {
    let mut inner = service().inner.lock().unwrap();
    let settings = inner.settings.clone();
    let Some(d) = inner.devices.get_mut(addr) else { return };
    let mut act: Option<Box<dyn FnOnce() + Send>> = None;
    match msg {
        Message::Battery(levels) => {
            let find = |p: Part| levels.iter().find(|l| l.part == p).and_then(charge);
            let i = &mut d.info;
            (i.left, i.right, i.case, i.whole) = (find(Part::Left), find(Part::Right), find(Part::Case), find(Part::Whole));
        }
        Message::Ear { primary, secondary } => {
            let before = d.info.in_ear;
            let count = |p: Placement| [primary, secondary].iter().filter(|x| **x == p).count() as u8;
            d.info.in_ear = count(Placement::InEar);
            d.info.in_case = count(Placement::InCase);
            d.placement = Some((primary, secondary));
            if settings.pause_when_removed {
                act = ear_changed(d, before);
            }
        }
        Message::Setting { setting: Some(s), value, .. } => {
            let on = |v: u8| Some(v == 1);
            let st = &mut d.info.settings;
            match s {
                Setting::ListeningMode => st.listening_mode = proto::ListeningMode::from_byte(value[0]).map(from_proto),
                Setting::ListeningModes => st.press_and_hold_modes = modes_of(value[0]),
                Setting::Conversation => st.conversation_awareness = on(value[0]),
                Setting::AdaptiveVolume => st.personalized_volume = on(value[0]),
                Setting::AdaptiveStrength => st.adaptive_strength = Some(value[0].min(100)),
                Setting::EarDetection => st.ear_detection = on(value[0]),
                Setting::OneBudAnc => st.one_bud_noise_cancellation = on(value[0]),
                Setting::VolumeSwipe => st.volume_swipe = on(value[0]),
                Setting::CaseSounds => st.case_sounds = Some(value[0] == 0),
                Setting::Microphone => {
                    st.microphone = Some(match value[0] {
                        1 => BudMicrophone::Right,
                        2 => BudMicrophone::Left,
                        _ => BudMicrophone::Automatic,
                    })
                }
                Setting::PressSpeed => st.press_speed = Some(value[0]),
                Setting::HoldDuration => st.hold_duration = Some(value[0]),
                Setting::OwnsConnection | Setting::PressAndHold | Setting::AllowOff => {}
            }
        }
        Message::Setting { id, value, .. } => tracing::debug!("airpods {addr}: setting {id:#04x} = {value:02x?}"),
        Message::Conversation { level } => {
            let speaking = level < 8;
            d.info.speaking = speaking;
            if settings.lower_when_speaking && !d.mic_on {
                act = speaking_changed(d, level);
            }
        }
        Message::Info(info) => {
            d.info.model_number = info.model;
            d.info.serial = info.serial;
            d.info.firmware = info.firmware;
            tracing::info!("airpods {addr}: {} {} firmware {}", d.info.model, d.info.model_number, d.info.firmware);
        }
        Message::Keys(keys) => crate::keys::remember(addr, &keys),
        Message::Audio(frames) => crate::mic::frames(addr, frames),
        Message::StemPress { press, left } => tracing::debug!("airpods {addr}: stem press {press:?} (left: {left})"),
        Message::Other { opcode, data } => tracing::debug!("airpods {addr}: opcode {opcode:#06x}: {}", hex(&data)),
    }
    publish(&mut inner);
    drop(inner);
    if let Some(f) = act {
        std::thread::spawn(f);
    }
}

/// Whether sound goes to these AirPods now.
fn playing_here(d: &Dev) -> bool {
    !d.info.sound.output.is_empty() && audio::devices().ok().and_then(|ds| ds.default_device(audio::Kind::Output).map(|o| o.name.clone())).is_some_and(|o| o == d.info.sound.output)
}

/// A bud out pauses what plays (as on an iPhone: one out of two is
/// enough); back in as before plays it again.
fn ear_changed(d: &mut Dev, before: u8) -> Option<Box<dyn FnOnce() + Send>> {
    let now = d.info.in_ear;
    if now < before && before > 0 {
        let output = d.info.sound.output.clone();
        // Who was paused is only known once it's done: set from the thread.
        d.paused_at = Some((Instant::now(), before));
        let addr = d.info.address.clone();
        let here = !output.is_empty();
        return Some(Box::new(move || {
            if !here || !service().inner.lock().unwrap().devices.get(&addr).is_some_and(playing_here) {
                return;
            }
            let playing = media::playing().unwrap_or_default();
            for p in &playing {
                if let Err(e) = media::pause(p) {
                    tracing::debug!("airpods: pausing {p}: {e}");
                }
            }
            if !playing.is_empty() {
                tracing::info!("airpods {addr}: a bud came out: paused {}", playing.join(", "));
            }
            if let Some(d) = service().inner.lock().unwrap().devices.get_mut(&addr) {
                d.paused = playing;
            }
        }));
    }
    if let Some((at, had)) = d.paused_at
        && now >= had
    {
        d.paused_at = None;
        let paused = std::mem::take(&mut d.paused);
        if at.elapsed() < RESUME_WITHIN && !paused.is_empty() {
            let addr = d.info.address.clone();
            return Some(Box::new(move || {
                tracing::info!("airpods {addr}: back in: playing {}", paused.join(", "));
                for p in paused {
                    let _ = media::play(&p);
                }
            }));
        }
    }
    None
}

/// Conversation Awareness: the AirPods say how far down the sound should
/// go (1 lowest … 8 and up normal) as the wearer starts and stops
/// speaking; the AirPods' output follows, and goes back to what it was.
fn speaking_changed(d: &mut Dev, level: u8) -> Option<Box<dyn FnOnce() + Send>> {
    let output = d.info.sound.output.clone();
    if output.is_empty() {
        return None;
    }
    if level >= 8 {
        let (name, volume, channels) = d.before_speaking.take()?;
        return Some(Box::new(move || {
            let _ = audio::set_device_volume(audio::Kind::Output, &name, volume, channels);
        }));
    }
    if d.before_speaking.is_none() {
        let dev = audio::devices().ok()?.outputs.into_iter().find(|o| o.name == output)?;
        d.before_speaking = Some((dev.name, dev.volume.level, dev.channels));
    }
    let (name, volume, channels) = d.before_speaking.clone()?;
    let share = SPEAKING_FLOOR + (1.0 - SPEAKING_FLOOR) * (level.saturating_sub(1) as f32 / 7.0);
    Some(Box::new(move || {
        let _ = audio::set_device_volume(audio::Kind::Output, &name, volume * share, channels);
    }))
}

// ---- the sound server -----------------------------------------------------------

/// The sound server's devices changed: how each pair's sound is carried,
/// and the microphone kept where Olo's settings say.
fn sound_changed(devices: Option<audio::Devices>) {
    let Some(devices) = devices else { return };
    let cards = if devices.outputs.iter().chain(&devices.inputs).any(|d| d.bluetooth.is_some()) { audio::cards().unwrap_or_default() } else { Vec::new() };
    let (mut fix_input, mut make_default, mut use_input) = (None, None, None);
    {
        let mut inner = service().inner.lock().unwrap();
        let seen = std::mem::replace(&mut inner.sound_seen, true);
        let addrs: Vec<String> = inner.devices.keys().cloned().collect();
        for addr in &addrs {
            let ours = |d: &&audio::Device| d.bluetooth.as_ref().is_some_and(|b| b.address.eq_ignore_ascii_case(addr));
            let out = devices.outputs.iter().find(ours);
            // Their headset input (BlueZ's), and their own microphone's
            // source (ours).
            let mic_node = crate::mic::node(addr);
            let input = devices.inputs.iter().filter(ours).find(|d| Some(&d.name) != mic_node.as_ref());
            let microphone = devices.inputs.iter().find(|d| Some(&d.name) == mic_node.as_ref());
            let bt = out.or(input).and_then(|d| d.bluetooth.clone()).unwrap_or_default();
            if inner.settings.microphone == MicrophoneChoice::AirPods
                && let Some(m) = microphone
                && !m.is_default
                && inner.devices.get(addr).is_some_and(|d| d.info.sound.microphone != m.name)
            {
                use_input = Some(m.name.clone());
            }
            let sound = ap::Sound {
                output: out.map(|d| d.name.clone()).unwrap_or_default(),
                input: input.map(|d| d.name.clone()).unwrap_or_default(),
                microphone: microphone.map(|d| d.name.clone()).unwrap_or_default(),
                codec: bt.codec.clone(),
                high_quality: bt.high_quality(),
                card: cards.iter().find(|c| c.address.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(addr))).map(|c| c.name.clone()).unwrap_or_default(),
            };
            if let Some(d) = inner.devices.get_mut(addr) {
                // Their output has just come: sound goes to them, as when
                // headphones are put on (once; the person may choose
                // otherwise after).
                if seen && !sound.output.is_empty() && d.info.sound.output != sound.output && !out.is_some_and(|o| o.is_default) {
                    make_default = Some(sound.output.clone());
                }
                d.info.sound = sound;
            }
        }
        // The computer's microphones stay the input while AirPods are
        // connected, unless the person chose theirs: an input of theirs
        // that has just come and been made the default is undone.
        let is_airpods_input = |name: &str| inner.devices.values().any(|d| d.info.sound.input == name || d.info.sound.microphone == name);
        let default_in = devices.default_device(audio::Kind::Input).map(|d| d.name.clone());
        if let Some(name) = &default_in {
            let new = !inner.inputs.contains(name);
            if !is_airpods_input(name) {
                inner.computer_input = Some(name.clone());
            } else if new && inner.settings.microphone == MicrophoneChoice::Computer {
                fix_input = inner.computer_input.clone().or_else(|| devices.inputs.iter().find(|d| d.bluetooth.is_none()).map(|d| d.name.clone()));
            }
        }
        inner.inputs = devices.inputs.iter().map(|d| d.name.clone()).collect();
        publish(&mut inner);
    }
    if let Some(name) = use_input {
        tracing::info!("airpods: recording from their own microphone ({name})");
        if let Err(e) = audio::set_default(audio::Kind::Input, &name) {
            tracing::warn!("airpods: {e}");
        }
    }
    if let Some(name) = make_default {
        tracing::info!("airpods: playing through them ({name})");
        if let Err(e) = audio::set_default(audio::Kind::Output, &name) {
            tracing::warn!("airpods: {e}");
        }
    }
    if let Some(name) = fix_input {
        tracing::info!("airpods: keeping the computer's microphone ({name}) as the input");
        if let Err(e) = audio::set_default(audio::Kind::Input, &name) {
            tracing::warn!("airpods: {e}");
        }
    }
}

/// Start or stop a pair's own microphone stream (the microphone's source
/// streaming or not, or asked on the command line). Conversation
/// Awareness is off while it runs. False if they aren't talking to us.
pub fn microphone_wanted(addr: &str, on: bool) -> bool {
    let mut inner = service().inner.lock().unwrap();
    let Some(d) = inner.devices.get_mut(addr) else { return false };
    let Some(link) = d.link.clone() else { return false };
    if d.mic_on == on {
        return true;
    }
    d.mic_on = on;
    let mut msgs = vec![if on { proto::start_microphone() } else { proto::stop_microphone() }];
    if on {
        if d.info.settings.conversation_awareness == Some(true) {
            d.awareness_after = Some(true);
            msgs.push(proto::set_on(Setting::Conversation, false));
        }
    } else if let Some(back) = d.awareness_after.take() {
        msgs.push(proto::set_on(Setting::Conversation, back));
    }
    drop(inner);
    crate::mic::switched(addr, on);
    if let Err(e) = msgs.iter().try_for_each(|m| link.send(m)) {
        tracing::warn!("airpods {addr}: {e}");
    }
    true
}

/// Switch a pair between music quality and the headset profile.
fn set_quality(addr: &str, high: bool) -> Result<(), String> {
    let card = audio::cards().map_err(|e| e.to_string())?.into_iter().find(|c| c.address.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(addr))).ok_or("Their sound isn't connected.")?;
    let want = |p: &str| if high { p.starts_with("a2dp") } else { p.starts_with("headset") };
    let profile = card.profiles.iter().filter(|(n, _, available)| *available && want(n)).map(|(n, ..)| n.clone()).next().ok_or(if high {
        "They can't play at music quality now."
    } else {
        "Their microphone isn't available."
    })?;
    audio::set_card_profile(&card.name, &profile).map_err(|e| e.to_string())
}

// ---- settings and requests ------------------------------------------------------

fn follow_settings() {
    let w = olo_kit_base::watch::watch([AirPodsSettings::path()], |_| {
        let new = AirPodsSettings::load();
        let mut inner = service().inner.lock().unwrap();
        if inner.settings != new {
            tracing::info!("airpods: settings changed");
            inner.settings = new;
        }
    });
    // For as long as the service runs.
    std::mem::forget(w);
}

/// The pair a request is for: the one named, else the one connected.
fn pick(inner: &Inner, address: &Option<String>) -> Result<String, String> {
    match address {
        Some(a) => inner.devices.keys().find(|k| k.eq_ignore_ascii_case(a)).cloned().ok_or_else(|| format!("No AirPods paired at {a}.")),
        None => inner
            .devices
            .values()
            .find(|d| d.info.connected)
            .or_else(|| inner.devices.values().next())
            .map(|d| d.info.address.clone())
            .ok_or_else(|| "No AirPods are paired with this computer.".to_owned()),
    }
}

/// The messages that make a change.
fn messages(change: &Change) -> Result<Vec<Vec<u8>>, String> {
    Ok(vec![match change {
        Change::ListeningMode(m) => proto::set_listening_mode(to_proto(*m)),
        Change::PressAndHoldModes(modes) => {
            if modes.len() < 2 {
                return Err("Press and hold needs at least two modes to switch between.".into());
            }
            let mask = modes.iter().fold(0, |m, x| m | to_proto(*x).bit());
            let mut out = Vec::new();
            if modes.contains(&ListeningMode::Off) {
                out.push(proto::set_on(Setting::AllowOff, true));
            }
            out.push(proto::set(Setting::ListeningModes, &[mask]));
            return Ok(out);
        }
        Change::ConversationAwareness(on) => proto::set_on(Setting::Conversation, *on),
        Change::PersonalizedVolume(on) => proto::set_on(Setting::AdaptiveVolume, *on),
        Change::AdaptiveStrength(v) => proto::set(Setting::AdaptiveStrength, &[(*v).min(100)]),
        Change::EarDetection(on) => proto::set_on(Setting::EarDetection, *on),
        Change::OneBudNoiseCancellation(on) => proto::set_on(Setting::OneBudAnc, *on),
        Change::VolumeSwipe(on) => proto::set_on(Setting::VolumeSwipe, *on),
        Change::CaseSounds(on) => proto::set(Setting::CaseSounds, &[if *on { 0 } else { 1 }]),
        Change::Microphone(m) => proto::set(
            Setting::Microphone,
            &[match m {
                BudMicrophone::Automatic => 0,
                BudMicrophone::Right => 1,
                BudMicrophone::Left => 2,
            }],
        ),
        Change::PressSpeed(v) => proto::set(Setting::PressSpeed, &[(*v).min(2)]),
        Change::HoldDuration(v) => proto::set(Setting::HoldDuration, &[(*v).min(2)]),
        Change::Name(name) => {
            if name.trim().is_empty() {
                return Err("A name can't be empty.".into());
            }
            proto::rename(name.trim())
        }
    }])
}

/// What a change looks like at once (the AirPods confirm it after).
fn apply_locally(d: &mut Dev, change: &Change) {
    let st = &mut d.info.settings;
    match change {
        Change::ListeningMode(m) => st.listening_mode = Some(*m),
        Change::PressAndHoldModes(m) => st.press_and_hold_modes = ListeningMode::ALL.into_iter().filter(|x| m.contains(x)).collect(),
        Change::ConversationAwareness(on) => st.conversation_awareness = Some(*on),
        Change::PersonalizedVolume(on) => st.personalized_volume = Some(*on),
        Change::AdaptiveStrength(v) => st.adaptive_strength = Some(*v),
        Change::EarDetection(on) => st.ear_detection = Some(*on),
        Change::OneBudNoiseCancellation(on) => st.one_bud_noise_cancellation = Some(*on),
        Change::VolumeSwipe(on) => st.volume_swipe = Some(*on),
        Change::CaseSounds(on) => st.case_sounds = Some(*on),
        Change::Microphone(m) => st.microphone = Some(*m),
        Change::PressSpeed(v) => st.press_speed = Some(*v),
        Change::HoldDuration(v) => st.hold_duration = Some(*v),
        Change::Name(n) => d.info.name = n.trim().to_owned(),
    }
}

fn answer(request: Request) -> Reply {
    let ok = |state: Option<AirPodsState>| Reply { ok: true, error: None, state };
    let fail = |e: String| Reply { ok: false, error: Some(e), state: None };
    let mut inner = service().inner.lock().unwrap();
    let fake = inner.fake;
    match request {
        Request::Status => {
            publish(&mut inner);
            ok(inner.published.clone())
        }
        Request::Set { address, change } => {
            let addr = match pick(&inner, &address) {
                Ok(a) => a,
                Err(e) => return fail(e),
            };
            let msgs = match messages(&change) {
                Ok(m) => m,
                Err(e) => return fail(e),
            };
            let d = inner.devices.get_mut(&addr).expect("picked");
            if !fake {
                let Some(link) = d.link.clone() else { return fail(format!("{} aren't connected.", d.info.name)) };
                if let Err(e) = msgs.iter().try_for_each(|m| link.send(m)) {
                    return fail(e.to_string());
                }
            }
            apply_locally(d, &change);
            if let Change::PressAndHoldModes(modes) = &change {
                crate::keys::remember_press_and_hold(&addr, modes);
            }
            let path = d.info.path.clone();
            publish(&mut inner);
            drop(inner);
            // What this computer calls them too.
            if let Change::Name(name) = change
                && !fake
                && let Err(e) = bt::set_alias(&path, name.trim())
            {
                tracing::warn!("airpods: renaming in BlueZ: {e}");
            }
            ok(None)
        }
        Request::Connect { address } | Request::Disconnect { address } if fake => {
            let _ = address;
            ok(None)
        }
        Request::Connect { address } => {
            let path = match pick(&inner, &address) {
                Ok(a) if inner.devices[&a].info.connected => return ok(None),
                Ok(a) => inner.devices[&a].info.path.clone(),
                Err(e) => return fail(e),
            };
            drop(inner);
            bt::connect(&path).map(|_| ok(None)).unwrap_or_else(|e| fail(e.to_string()))
        }
        Request::Disconnect { address } => {
            let path = match pick(&inner, &address) {
                Ok(a) if !inner.devices[&a].info.connected => return ok(None),
                Ok(a) => inner.devices[&a].info.path.clone(),
                Err(e) => return fail(e),
            };
            drop(inner);
            bt::disconnect(&path).map(|_| ok(None)).unwrap_or_else(|e| fail(e.to_string()))
        }
        Request::Microphone { address, on } => {
            let addr = match pick(&inner, &address) {
                Ok(a) => a,
                Err(e) => return fail(e),
            };
            drop(inner);
            if microphone_wanted(&addr, on) { ok(None) } else { fail("They aren't connected.".into()) }
        }
        Request::Quality { address, high } => {
            let addr = match pick(&inner, &address) {
                Ok(a) => a,
                Err(e) => return fail(e),
            };
            if fake {
                let d = inner.devices.get_mut(&addr).expect("picked");
                d.info.sound.high_quality = high;
                d.info.sound.codec = if high { "aac" } else { "msbc" }.into();
                publish(&mut inner);
                return ok(None);
            }
            drop(inner);
            set_quality(&addr, high).map(|_| ok(None)).unwrap_or_else(fail)
        }
    }
}

/// Answer requests on the socket, a thread each (connecting and
/// disconnecting wait on BlueZ).
fn serve() -> ! {
    let path = AirPodsState::socket();
    // One service at a time: a socket someone answers on is theirs.
    if UnixStream::connect(&path).is_ok() {
        tracing::warn!("airpods: another service is running");
        std::process::exit(0);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("airpods: can't listen on {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                return;
            }
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(r) => answer(r),
                Err(e) => Reply { ok: false, error: Some(format!("not understood: {e}")), state: None },
            };
            let mut out = serde_json::to_vec(&reply).unwrap_or_default();
            out.push(b'\n');
            let _ = (&stream).write_all(&out);
        });
    }
    std::process::exit(1)
}

// ---- a pretend pair, for tests ----------------------------------------------------

/// `OLO_FAKE_AIRPODS=1`: a pair of AirPods Pro, connected, that take
/// every change, so Deskbar and Preferences can be driven headless.
fn fake_pair() {
    let model = Model::of(0x2024);
    let mut inner = service().inner.lock().unwrap();
    let d = inner.devices.entry("AA:BB:CC:DD:EE:FF".into()).or_default();
    d.info = AirPods {
        address: "AA:BB:CC:DD:EE:FF".into(),
        path: "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF".into(),
        name: "AirPods Pro".into(),
        model: model.name.into(),
        product: model.product,
        model_number: "A3048".into(),
        serial: "FAKE000000".into(),
        firmware: "9A348".into(),
        abilities: ap::Abilities { noise_control: true, adaptive: true, buds: true },
        connected: true,
        linked: true,
        left: Some(Charge { percent: 82, charging: false }),
        right: Some(Charge { percent: 79, charging: false }),
        case: Some(Charge { percent: 54, charging: true }),
        whole: None,
        in_ear: 2,
        in_case: 0,
        nearby: true,
        elsewhere: false,
        speaking: false,
        settings: ap::BudSettings {
            listening_mode: Some(ListeningMode::NoiseCancellation),
            press_and_hold_modes: vec![ListeningMode::Transparency, ListeningMode::NoiseCancellation],
            conversation_awareness: Some(true),
            personalized_volume: Some(false),
            adaptive_strength: Some(50),
            ear_detection: Some(true),
            one_bud_noise_cancellation: Some(false),
            volume_swipe: Some(true),
            case_sounds: Some(true),
            microphone: Some(BudMicrophone::Automatic),
            press_speed: Some(0),
            hold_duration: Some(0),
        },
        sound: ap::Sound {
            output: "bluez_output.AA_BB_CC_DD_EE_FF.1".into(),
            input: String::new(),
            microphone: String::new(),
            codec: "aac".into(),
            high_quality: true,
            card: "bluez_card.AA_BB_CC_DD_EE_FF".into(),
        },
    };
    publish(&mut inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ago(s: u64) -> Option<Instant> {
        Instant::now().checked_sub(Duration::from_secs(s))
    }

    #[test]
    fn taken_by_another_device_they_are_left_alone_until_free_a_minute() {
        let mut d = Dev::default();
        // Never lost: Olo may.
        assert!(may_take(&mut d));
        // Taken by the phone (lost here), still connected there: not again.
        d.lost_at = ago(5);
        d.free_since = None;
        assert!(!may_take(&mut d));
        // Free (connected to nothing) for 30 s: still not.
        d.free_since = ago(30);
        assert!(!may_take(&mut d));
        // Free for a minute: yes, and the loss is forgotten.
        d.free_since = ago(61);
        assert!(may_take(&mut d));
        assert!(d.lost_at.is_none());
    }

    #[test]
    fn one_try_at_a_time() {
        let mut d = Dev { tried_at: ago(3), ..Dev::default() };
        assert!(!may_take(&mut d));
        d.tried_at = ago(16);
        assert!(may_take(&mut d));
    }
}
