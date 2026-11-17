//! `phone-standin [--address ADDRESS] [--android] [--unpaired]`: a
//! stand-in for BlueZ with one phone paired (`org.bluez` on the bus at
//! ADDRESS; default the session bus), for headless sessions and tests, so
//! nothing touches Bluetooth, a phone or anyone's messages. It answers
//! what Olo's phone link asks of BlueZ (`olo_system::phone::link`: the
//! devices there are, profiles registered, a profile's channel opened)
//! and behind each channel is a stand-in phone
//! (`olo_system::phone::standin`) that speaks the profiles: an iPhone
//! with made-up people in it, or with `--android` a phone with an
//! Android's habits. Point Olo at it with `OLO_PHONE_BLUEZ=ADDRESS`.
//!
//! What a test makes happen, it tells the running stand-in:
//! `phone-standin [--address ADDRESS] --tell receive NAME ADDRESS TEXT…`
//! (a message arrives now, and the phone tells whoever asked to be told),
//! `--tell set messages|contacts|connected|paired yes|no` (the phone's
//! own switches; in reach or not; paired or not), `--tell sent` (prints
//! what was sent through it, a line each).

use std::{
    collections::HashMap,
    os::fd::{FromRawFd as _, OwnedFd},
    sync::{Arc, Mutex, mpsc},
};

use olo_system::phone::{
    link::{Closer, Stream},
    map, pbap,
    standin::Phone,
};
use zvariant::{OwnedObjectPath, OwnedValue, Value};

const NAME: &str = "org.bluez";
const ADAPTER: &str = "/org/bluez/hci0";
const DEVICE: &str = "/org/bluez/hci0/dev_02_00_00_00_F0_01";
const STANDIN: &str = "org.olo.PhoneStandin";
/// The phone's music player, where BlueZ puts a connected phone's.
const PLAYER: &str = "/org/bluez/hci0/dev_02_00_00_00_F0_01/avrcp/player0";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotAvailable(String),
    AlreadyExists(String),
    InvalidArguments(String),
    AuthenticationRejected(String),
}

enum Job {
    /// Open a channel of `uuid` to whoever registered that profile.
    Open(String),
    /// Tell whoever asked that message `handle` came.
    Tell(String),
    /// Say that the device's properties changed.
    Changed,
    /// Pair: the agent is asked whether the code matches; yes pairs.
    /// Told how it went, when someone waits (a `Connect` to the phone
    /// unpaired, which it pairs from its side).
    Pair(Option<async_channel::Sender<bool>>),
    /// An advertisement was registered (who, their object): read what
    /// it solicits.
    Advert(String, String),
    /// The phone closes every channel it has open, and stays connected
    /// (its owner turned a switch off; it had enough).
    HangUp,
    /// What the phone plays changed: said, as BlueZ says it.
    Music,
}

/// What the stand-in phone plays.
#[derive(Clone, Default)]
struct Playing {
    playing: bool,
    title: String,
    artist: String,
}

struct World {
    phone: Phone,
    /// Profiles registered: uuid → (who, their object).
    profiles: HashMap<String, (String, String)>,
    paired: bool,
    connected: bool,
    android: bool,
    discovering: bool,
    /// The pairing agent registered: (who, their object).
    agent: Option<(String, String)>,
    /// How many times it has paired (one pairing, one entry on the
    /// phone: `--tell pairings`).
    pairings: u32,
    /// The profile whose phone-book client record is published (an
    /// iPhone offers Sync Contacts only to a computer that has one).
    pce_record: Option<String>,
    /// BlueZ takes the phone's messages for connected already (`--tell
    /// set stale yes`): asked to connect them, it says yes and opens
    /// nothing, until it's told to let go (`DisconnectProfile`).
    stale: bool,
    /// An advertisement soliciting Apple's notification service is
    /// broadcast: an iPhone shows its switches for the computer only
    /// while one is.
    soliciting: bool,
    /// Who registered the advertisement.
    advertiser: Option<String>,
    /// What it's playing (`--tell music`; none: it has no player), and
    /// what its remote control was pressed (`--tell pressed`).
    music: Option<Playing>,
    pressed: Vec<String>,
    jobs: mpsc::Sender<Job>,
}

type Shared = Arc<Mutex<World>>;

struct Manager(Shared);

#[zbus::interface(name = "org.bluez.ProfileManager1")]
impl Manager {
    /// As BlueZ: kept by the phone's service it connects to (the phone
    /// book's client connects to its server); the client's service record
    /// published unless it's registered as a client only.
    fn register_profile(&self, profile: OwnedObjectPath, uuid: String, options: HashMap<String, OwnedValue>, #[zbus(header)] header: zbus::message::Header<'_>) -> Result<(), Error> {
        let who = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let mut w = self.0.lock().expect("world");
        let uuid = uuid.to_ascii_lowercase();
        let remote = if uuid == pbap::PCE_UUID { pbap::PSE_UUID.to_owned() } else { uuid.clone() };
        if w.profiles.contains_key(&remote) {
            return Err(Error::AlreadyExists("UUID already registered".into()));
        }
        let role = options.get("Role").and_then(|v| <&str>::try_from(v).ok().map(str::to_owned));
        if uuid == pbap::PCE_UUID && role.as_deref() != Some("client") {
            w.pce_record = Some(profile.as_str().to_owned());
        }
        w.profiles.insert(remote, (who, profile.as_str().to_owned()));
        Ok(())
    }

    fn unregister_profile(&self, profile: OwnedObjectPath) {
        let mut w = self.0.lock().expect("world");
        w.profiles.retain(|_, (_, path)| path != profile.as_str());
        if w.pce_record.as_deref() == Some(profile.as_str()) {
            w.pce_record = None;
        }
    }
}

struct Adverts(Shared);

#[zbus::interface(name = "org.bluez.LEAdvertisingManager1")]
impl Adverts {
    fn register_advertisement(&self, advert: OwnedObjectPath, _options: HashMap<String, OwnedValue>, #[zbus(header)] header: zbus::message::Header<'_>) {
        let who = header.sender().map(|s| s.to_string()).unwrap_or_default();
        // (Read once this has answered: asking from in here would wait
        // on the thread that hears the answer.)
        let mut w = self.0.lock().expect("world");
        w.advertiser = Some(who.clone());
        let _ = w.jobs.send(Job::Advert(who, advert.as_str().to_owned()));
    }

    fn unregister_advertisement(&self, _advert: OwnedObjectPath) {
        self.0.lock().expect("world").soliciting = false;
    }
}

struct Agents(Shared);

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl Agents {
    fn register_agent(&self, agent: OwnedObjectPath, _capability: String, #[zbus(header)] header: zbus::message::Header<'_>) {
        let who = header.sender().map(|s| s.to_string()).unwrap_or_default();
        self.0.lock().expect("world").agent = Some((who, agent.as_str().to_owned()));
    }

    fn unregister_agent(&self, _agent: OwnedObjectPath) {
        self.0.lock().expect("world").agent = None;
    }

    fn request_default_agent(&self, _agent: OwnedObjectPath) {}
}

struct Adapter(Shared);

#[zbus::interface(name = "org.bluez.Adapter1")]
impl Adapter {
    #[zbus(property)]
    fn address(&self) -> String {
        "02:00:00:00:00:01".into()
    }
    #[zbus(property)]
    fn powered(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn power_state(&self) -> String {
        "on".into()
    }
    #[zbus(property)]
    fn discoverable(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn discovering(&self) -> bool {
        self.0.lock().expect("world").discovering
    }
    #[zbus(property)]
    fn alias(&self) -> String {
        "studio-laptop".into()
    }
    /// A laptop's (Computer, Laptop), as BlueZ's default is.
    #[zbus(property)]
    fn class(&self) -> u32 {
        0x6c010c
    }

    fn start_discovery(&self) {
        self.0.lock().expect("world").discovering = true;
    }

    fn stop_discovery(&self) {
        self.0.lock().expect("world").discovering = false;
    }
}

struct Device(Shared);

#[zbus::interface(name = "org.bluez.Device1")]
impl Device {
    #[zbus(property)]
    fn address(&self) -> String {
        "02:00:00:00:F0:01".into()
    }
    #[zbus(property)]
    fn name(&self) -> String {
        if self.0.lock().expect("world").android { "Pixel 11".into() } else { "Mira’s iPhone".into() }
    }
    #[zbus(property)]
    fn alias(&self) -> String {
        self.name()
    }
    #[zbus(property)]
    fn icon(&self) -> String {
        "phone".into()
    }
    #[zbus(property)]
    fn modalias(&self) -> String {
        if self.0.lock().expect("world").android { "bluetooth:v00E0p1200d1436".into() } else { "bluetooth:v004Cp7A1Ad1A40".into() }
    }
    #[zbus(property)]
    fn paired(&self) -> bool {
        self.0.lock().expect("world").paired
    }
    #[zbus(property)]
    fn trusted(&self) -> bool {
        self.0.lock().expect("world").paired
    }
    #[zbus(property)]
    fn set_trusted(&self, _trusted: bool) {}
    #[zbus(property)]
    fn connected(&self) -> bool {
        self.0.lock().expect("world").connected
    }
    #[zbus(property, name = "UUIDs")]
    fn uuids(&self) -> Vec<String> {
        vec![map::MAS_UUID.into(), pbap::PSE_UUID.into(), "0000110a-0000-1000-8000-00805f9b34fb".into()]
    }

    fn connect_profile(&self, uuid: String) -> Result<(), Error> {
        let w = self.0.lock().expect("world");
        if !w.paired || !w.connected {
            return Err(Error::NotAvailable("br-connection-page-timeout".into()));
        }
        let uuid = uuid.to_ascii_lowercase();
        if !w.profiles.contains_key(&uuid) {
            return Err(Error::InvalidArguments("no such profile registered".into()));
        }
        // (As BlueZ with a profile it takes for connected: yes, and
        // nothing opened.)
        if w.stale && uuid == map::MAS_UUID {
            return Ok(());
        }
        let _ = w.jobs.send(Job::Open(uuid));
        Ok(())
    }

    fn disconnect_profile(&self, uuid: String) -> Result<(), Error> {
        let mut w = self.0.lock().expect("world");
        if uuid.eq_ignore_ascii_case(map::MAS_UUID) && w.stale {
            w.stale = false;
            return Ok(());
        }
        Err(Error::NotAvailable("not connected".into()))
    }

    /// Pairing: answered once the agent has (the code confirmed).
    fn pair(&self) -> Result<(), Error> {
        let w = self.0.lock().expect("world");
        if w.agent.is_none() {
            return Err(Error::NotAvailable("no agent".into()));
        }
        let _ = w.jobs.send(Job::Pair(None));
        Ok(())
    }

    fn cancel_pairing(&self) {}

    /// Connected to unpaired, a phone refuses without security and
    /// starts pairing itself (the agent asked to confirm the code): the
    /// connection is answered once that's done, as BlueZ's is.
    async fn connect(&self) -> Result<(), Error> {
        let waited = {
            let mut w = self.0.lock().expect("world");
            if w.paired {
                w.connected = true;
                let _ = w.jobs.send(Job::Changed);
                return Ok(());
            }
            if w.agent.is_none() {
                return Err(Error::NotAvailable("no agent".into()));
            }
            let (tx, rx) = async_channel::bounded(1);
            let _ = w.jobs.send(Job::Pair(Some(tx)));
            rx
        };
        match waited.recv().await {
            Ok(true) => Ok(()),
            _ => Err(Error::AuthenticationRejected("the code was said not to match".into())),
        }
    }

    fn disconnect(&self) {
        let mut w = self.0.lock().expect("world");
        w.connected = false;
        let _ = w.jobs.send(Job::Changed);
    }
}

/// What a test tells the stand-in to play.
/// The phone as a player this computer is the remote control of, as
/// BlueZ shows a connected phone's (AVRCP).
struct Control(Shared);

#[zbus::interface(name = "org.bluez.MediaControl1")]
impl Control {
    #[zbus(property)]
    fn connected(&self) -> bool {
        let w = self.0.lock().expect("world");
        w.connected && w.music.is_some()
    }

    #[zbus(property)]
    fn player(&self) -> zbus::fdo::Result<OwnedObjectPath> {
        let w = self.0.lock().expect("world");
        if !w.connected || w.music.is_none() {
            return Err(zbus::fdo::Error::UnknownProperty("no player".into()));
        }
        Ok(zvariant::ObjectPath::try_from(PLAYER).expect("a path").into())
    }
}

struct Player(Shared);

impl Player {
    /// A press of the remote control: kept, and done as a phone does it.
    fn press(&self, what: &str) {
        let mut w = self.0.lock().expect("world");
        w.pressed.push(what.to_owned());
        if let Some(m) = &mut w.music {
            match what {
                "play" => m.playing = true,
                "pause" => m.playing = false,
                "next" => m.title = format!("After {}", m.title),
                _ => m.title = format!("Before {}", m.title),
            }
        }
        let _ = w.jobs.send(Job::Music);
    }
}

#[zbus::interface(name = "org.bluez.MediaPlayer1")]
impl Player {
    #[zbus(property)]
    fn status(&self) -> String {
        match &self.0.lock().expect("world").music {
            Some(m) if m.playing => "playing".into(),
            Some(_) => "paused".into(),
            None => "stopped".into(),
        }
    }

    #[zbus(property)]
    fn track(&self) -> HashMap<String, OwnedValue> {
        track(self.0.lock().expect("world").music.as_ref())
    }

    /// (Moves all the while on a real phone: never what's followed.)
    #[zbus(property)]
    fn position(&self) -> u32 {
        41_000
    }

    fn play(&self) {
        self.press("play");
    }

    fn pause(&self) {
        self.press("pause");
    }

    fn next(&self) {
        self.press("next");
    }

    fn previous(&self) {
        self.press("previous");
    }
}

/// A track as BlueZ gives it.
fn track(music: Option<&Playing>) -> HashMap<String, OwnedValue> {
    let mut t = HashMap::new();
    if let Some(m) = music {
        let text = |s: &str| OwnedValue::try_from(Value::from(s)).expect("a string");
        t.insert("Title".to_owned(), text(&m.title));
        if !m.artist.is_empty() {
            t.insert("Artist".to_owned(), text(&m.artist));
        }
        t.insert("Duration".to_owned(), OwnedValue::from(545_000u32));
    }
    t
}

struct Told(Shared);

#[zbus::interface(name = "org.olo.PhoneStandin")]
impl Told {
    /// What the phone plays: `status` playing or paused, or none (it has
    /// no player).
    fn music(&self, status: &str, title: &str, artist: &str) -> Result<(), Error> {
        let mut w = self.0.lock().expect("world");
        w.music = match status {
            "playing" | "paused" => Some(Playing { playing: status == "playing", title: title.to_owned(), artist: artist.to_owned() }),
            "none" => None,
            _ => return Err(Error::InvalidArguments(format!("music is playing, paused or none, not {status}"))),
        };
        let _ = w.jobs.send(Job::Music);
        Ok(())
    }

    /// What its remote control was pressed, in order.
    fn pressed(&self) -> Vec<String> {
        self.0.lock().expect("world").pressed.clone()
    }

    fn receive(&self, name: &str, from: &str, text: &str) -> String {
        let w = self.0.lock().expect("world");
        let handle = w.phone.receive(name, from, text, &now());
        let _ = w.jobs.send(Job::Tell(handle.clone()));
        handle
    }

    fn set(&self, key: &str, value: &str) -> Result<(), Error> {
        let on = value == "yes";
        let mut w = self.0.lock().expect("world");
        // An iPhone's switches for a computer are there to turn on only
        // while the computer advertises for them (BlueFerry's notes, iOS
        // 26); an Android phone asks as it pairs.
        if on && !w.android && !w.soliciting && matches!(key, "messages" | "contacts") {
            return Err(Error::NotAvailable("an iPhone shows this switch only while the computer advertises for it, and it isn't".into()));
        }
        if on && !w.android && key == "contacts" && w.pce_record.is_none() {
            return Err(Error::NotAvailable("an iPhone shows Sync Contacts only to a computer with a phone book client's service record, and this one has none".into()));
        }
        match key {
            "messages" => w.phone.0.lock().expect("phone").messages_allowed = on,
            "contacts" => w.phone.0.lock().expect("phone").contacts_allowed = on,
            "stale" => w.stale = on,
            "connected" => w.connected = on,
            "paired" => w.paired = on,
            _ => return Err(Error::InvalidArguments(format!("nothing called {key}"))),
        }
        let _ = w.jobs.send(Job::Changed);
        Ok(())
    }

    /// Whether an advertisement soliciting Apple's notification service
    /// is registered.
    fn soliciting(&self) -> bool {
        self.0.lock().expect("world").soliciting
    }

    fn hang_up(&self) {
        let _ = self.0.lock().expect("world").jobs.send(Job::HangUp);
    }

    fn pairings(&self) -> u32 {
        self.0.lock().expect("world").pairings
    }

    fn sent(&self) -> Vec<String> {
        let w = self.0.lock().expect("world");
        w.phone.0.lock().expect("phone").sent.iter().map(|(to, text)| format!("{}: {}", to.join(", "), text.replace('\n', " / "))).collect()
    }
}

/// Now, as a listing says it (UTC, with its Z).
fn now() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

const USAGE: &str = "usage: phone-standin [--address ADDRESS] [--android] [--unpaired]
       phone-standin [--address ADDRESS] --tell receive NAME ADDRESS TEXT…
       phone-standin [--address ADDRESS] --tell set messages|contacts|connected|paired|stale yes|no
       phone-standin [--address ADDRESS] --tell hang-up | soliciting | pairings | sent
       phone-standin [--address ADDRESS] --tell music playing|paused|none [TITLE [ARTIST]]
       phone-standin [--address ADDRESS] --tell pressed";

fn main() {
    olo_crash::install();
    let mut address = std::env::var("OLO_PHONE_BLUEZ").or_else(|_| std::env::var("DBUS_SESSION_BUS_ADDRESS")).unwrap_or_default();
    let (mut android, mut unpaired) = (false, false);
    let mut tell: Option<Vec<String>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().expect("--address ADDRESS"),
            "--android" => android = true,
            "--unpaired" => unpaired = true,
            "--tell" => tell = Some(args.by_ref().collect()),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if let Some(words) = tell {
        std::process::exit(told(&address, &words));
    }
    let (tx, jobs) = mpsc::channel::<Job>();
    let phone = Phone::sample();
    phone.0.lock().expect("phone").android = android;
    let world: Shared = Arc::new(Mutex::new(World {
        phone: phone.clone(),
        profiles: HashMap::new(),
        paired: !unpaired,
        connected: !unpaired,
        android,
        discovering: false,
        agent: None,
        soliciting: false,
        advertiser: None,
        music: None,
        pressed: Vec::new(),
        pairings: 0,
        pce_record: None,
        stale: false,
        jobs: tx,
    }));
    let built = zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(|b| b.serve_at("/", zbus::fdo::ObjectManager))
        .and_then(|b| b.serve_at("/org/bluez", Manager(world.clone())))
        .and_then(|b| b.serve_at("/org/bluez", Told(world.clone())))
        .and_then(|b| b.serve_at("/org/bluez", Agents(world.clone())))
        .and_then(|b| b.serve_at(ADAPTER, Adapter(world.clone())))
        .and_then(|b| b.serve_at(ADAPTER, Adverts(world.clone())))
        .and_then(|b| b.serve_at(DEVICE, Device(world.clone())))
        .and_then(|b| b.serve_at(DEVICE, Control(world.clone())))
        .and_then(|b| b.serve_at(PLAYER, Player(world.clone())))
        .and_then(|b| b.name(NAME))
        .and_then(|b| b.build());
    let conn = match built {
        Ok(c) => c,
        Err(e) => {
            eprintln!("phone-standin: can't take {NAME} on {address}: {e}");
            std::process::exit(1);
        }
    };
    // Who registered a profile, the agent or an advertisement and has
    // left the bus: BlueZ forgets what was theirs (a service killed
    // leaves nothing registered for the next one to trip over).
    {
        let world = world.clone();
        let conn = conn.clone();
        std::thread::spawn(move || {
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
            let Ok(changes) = dbus.receive_name_owner_changed() else { return };
            for change in changes {
                let Ok(args) = change.args() else { continue };
                if args.new_owner().is_some() {
                    continue;
                }
                let gone = args.name().to_string();
                let mut w = world.lock().expect("world");
                w.profiles.retain(|_, (who, _)| *who != gone);
                if w.agent.as_ref().is_some_and(|(who, _)| *who == gone) {
                    w.agent = None;
                }
                if w.advertiser.as_deref() == Some(gone.as_str()) {
                    (w.advertiser, w.soliciting) = (None, false);
                }
            }
        });
    }
    // What the methods ask for, done once they've answered: a channel
    // opened is a socket pair, one end the stand-in phone's, the other
    // handed to whoever registered the profile.
    // (The channels open, to end them when the phone goes out of reach.)
    let open: Mutex<Vec<Closer>> = Mutex::new(Vec::new());
    // A channel's two ends, as BlueZ's are: the one handed over
    // non-blocking (BlueZ reads its sockets from its own loop), and of
    // packets where the phone offers OBEX over L2CAP, which BlueZ then
    // connects by (an Android's messages and contacts; never the
    // Notification Server's, whose record offers RFCOMM alone).
    let pair = |packets: bool| -> Option<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as libc::c_int; 2];
        let kind = if packets { libc::SOCK_SEQPACKET } else { libc::SOCK_STREAM };
        // SAFETY: two descriptors of ours, owned from here on.
        unsafe {
            if libc::socketpair(libc::AF_UNIX, kind | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) != 0 {
                return None;
            }
            let flags = libc::fcntl(fds[1], libc::F_GETFL);
            libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
            Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
        }
    };
    for job in jobs {
        let hand_over = |uuid: &str, serve: Box<dyn FnOnce(Stream) + Send>| {
            let (found, android) = {
                let w = world.lock().expect("world");
                (w.profiles.get(uuid).cloned(), w.android)
            };
            let Some((who, path)) = found else { return };
            let Some((ours, theirs)) = pair(android && uuid != map::MNS_UUID) else { return };
            // (The phone's end: it waits for what's asked of it as long
            // as it takes.)
            let ours = Stream::adopt(ours);
            ours.patience(std::time::Duration::ZERO);
            open.lock().expect("open").push(ours.closer());
            // (Served to its end, then closed, as a phone closes a
            // channel it's done with.)
            let done = ours.closer();
            std::thread::spawn(move || {
                serve(ours);
                done.close();
            });
            let device = zvariant::ObjectPath::try_from(DEVICE).expect("a path");
            let fd = zvariant::OwnedFd::from(theirs);
            let none: HashMap<&str, Value> = HashMap::new();
            if let Err(e) = conn.call_method(Some(who.as_str()), path.as_str(), Some("org.bluez.Profile1"), "NewConnection", &(device, fd, none)) {
                eprintln!("phone-standin: handing over a channel: {e}");
            }
        };
        match job {
            Job::Open(uuid) => {
                let p = phone.clone();
                if uuid == map::MAS_UUID {
                    hand_over(&uuid, Box::new(move |link| drop(p.serve_messages(link))));
                } else if uuid == pbap::PSE_UUID {
                    hand_over(&uuid, Box::new(move |link| drop(p.serve_contacts(link))));
                }
            }
            Job::Tell(handle) => {
                if phone.notifying() && world.lock().expect("world").connected {
                    let p = phone.clone();
                    hand_over(map::MNS_UUID, Box::new(move |link| drop(p.tell(link, &handle))));
                }
            }
            // The code on both: the agent is asked; its yes pairs the
            // phone and brings it in reach.
            Job::Pair(told) => {
                let Some((who, path)) = world.lock().expect("world").agent.clone() else { continue };
                let device = zvariant::ObjectPath::try_from(DEVICE).expect("a path");
                let asked = conn.call_method(Some(who.as_str()), path.as_str(), Some("org.bluez.Agent1"), "RequestConfirmation", &(device, 482_913u32));
                if asked.is_ok() {
                    let mut w = world.lock().expect("world");
                    (w.paired, w.connected) = (true, true);
                    w.pairings += 1;
                    let _ = w.jobs.send(Job::Changed);
                }
                if let Some(told) = told {
                    let _ = told.send_blocking(asked.is_ok());
                }
            }
            Job::Advert(who, path) => {
                let solicits = conn
                    .call_method(Some(who.as_str()), path.as_str(), Some("org.freedesktop.DBus.Properties"), "GetAll", &("org.bluez.LEAdvertisement1",))
                    .ok()
                    .and_then(|m| m.body().deserialize::<HashMap<String, OwnedValue>>().ok())
                    .and_then(|all| all.get("SolicitUUIDs").and_then(|v| <Vec<String>>::try_from(v.try_clone().ok()?).ok()))
                    .unwrap_or_default();
                world.lock().expect("world").soliciting = solicits.iter().any(|u| u.eq_ignore_ascii_case("7905f431-b5ce-4e99-a40f-4b1e122d00d0"));
            }
            Job::HangUp => {
                for channel in open.lock().expect("open").drain(..) {
                    channel.close();
                }
            }
            Job::Music => {
                let music = world.lock().expect("world").music.clone();
                let none: Vec<&str> = Vec::new();
                let mut control: HashMap<&str, Value> = HashMap::new();
                control.insert("Connected", music.is_some().into());
                let _ = conn.emit_signal(None::<&str>, DEVICE, "org.freedesktop.DBus.Properties", "PropertiesChanged", &("org.bluez.MediaControl1", control, &none));
                let mut player: HashMap<&str, Value> = HashMap::new();
                let status = match &music {
                    Some(m) if m.playing => "playing",
                    Some(_) => "paused",
                    None => "stopped",
                };
                player.insert("Status", status.into());
                player.insert("Track", Value::from(track(music.as_ref())));
                let _ = conn.emit_signal(None::<&str>, PLAYER, "org.freedesktop.DBus.Properties", "PropertiesChanged", &("org.bluez.MediaPlayer1", player, &none));
            }
            Job::Changed => {
                let w = world.lock().expect("world");
                let mut changed: HashMap<&str, Value> = HashMap::new();
                changed.insert("Connected", w.connected.into());
                changed.insert("Paired", w.paired.into());
                if !w.connected || !w.paired {
                    for channel in open.lock().expect("open").drain(..) {
                        channel.close();
                    }
                }
                drop(w);
                let none: Vec<&str> = Vec::new();
                let _ = conn.emit_signal(None::<&str>, DEVICE, "org.freedesktop.DBus.Properties", "PropertiesChanged", &("org.bluez.Device1", changed, none));
            }
        }
    }
}

/// Tell the stand-in running on `address` what to play.
fn told(address: &str, words: &[String]) -> i32 {
    let conn = match zbus::blocking::connection::Builder::address(address).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("phone-standin: {address}: {e}");
            return 1;
        }
    };
    let call = |done: zbus::Result<zbus::Message>| match done {
        Ok(m) => {
            if let Ok(lines) = m.body().deserialize::<Vec<String>>() {
                for l in lines {
                    println!("{l}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("phone-standin: {e}");
            1
        }
    };
    match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["receive", name, from, text @ ..] if !text.is_empty() => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Receive", &(*name, *from, text.join(" ").as_str()))),
        ["set", key, value] => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Set", &(*key, *value))),
        ["soliciting"] => match conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Soliciting", &()).and_then(|m| m.body().deserialize::<bool>()) {
            Ok(on) => {
                println!("{}", if on { "yes" } else { "no" });
                0
            }
            Err(e) => {
                eprintln!("phone-standin: {e}");
                1
            }
        },
        ["pairings"] => match conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Pairings", &()).and_then(|m| m.body().deserialize::<u32>()) {
            Ok(n) => {
                println!("{n}");
                0
            }
            Err(e) => {
                eprintln!("phone-standin: {e}");
                1
            }
        },
        ["hang-up"] => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "HangUp", &())),
        ["music", status, words @ ..] => {
            let (title, artist) = (words.first().copied().unwrap_or_default(), words.get(1).copied().unwrap_or_default());
            call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Music", &(*status, title, artist)))
        }
        ["pressed"] => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Pressed", &())),
        ["sent"] => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Sent", &())),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}
