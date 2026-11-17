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
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::{Arc, Mutex, mpsc},
};

use olo_system::phone::{map, pbap, standin::Phone};
use zvariant::{OwnedObjectPath, OwnedValue, Value};

const NAME: &str = "org.bluez";
const ADAPTER: &str = "/org/bluez/hci0";
const DEVICE: &str = "/org/bluez/hci0/dev_02_00_00_00_F0_01";
const STANDIN: &str = "org.olo.PhoneStandin";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotAvailable(String),
    AlreadyExists(String),
    InvalidArguments(String),
}

enum Job {
    /// Open a channel of `uuid` to whoever registered that profile.
    Open(String),
    /// Tell whoever asked that message `handle` came.
    Tell(String),
    /// Say that the device's properties changed.
    Changed,
}

struct World {
    phone: Phone,
    /// Profiles registered: uuid → (who, their object).
    profiles: HashMap<String, (String, String)>,
    paired: bool,
    connected: bool,
    android: bool,
    jobs: mpsc::Sender<Job>,
}

type Shared = Arc<Mutex<World>>;

struct Manager(Shared);

#[zbus::interface(name = "org.bluez.ProfileManager1")]
impl Manager {
    fn register_profile(&self, profile: OwnedObjectPath, uuid: String, _options: HashMap<String, OwnedValue>, #[zbus(header)] header: zbus::message::Header<'_>) -> Result<(), Error> {
        let who = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let mut w = self.0.lock().expect("world");
        let uuid = uuid.to_ascii_lowercase();
        if w.profiles.contains_key(&uuid) {
            return Err(Error::AlreadyExists("UUID already registered".into()));
        }
        w.profiles.insert(uuid, (who, profile.as_str().to_owned()));
        Ok(())
    }

    fn unregister_profile(&self, profile: OwnedObjectPath) {
        self.0.lock().expect("world").profiles.retain(|_, (_, path)| path != profile.as_str());
    }
}

struct Adapter;

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
    fn alias(&self) -> String {
        "olo".into()
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
        let _ = w.jobs.send(Job::Open(uuid));
        Ok(())
    }

    fn connect(&self) -> Result<(), Error> {
        let mut w = self.0.lock().expect("world");
        w.connected = true;
        let _ = w.jobs.send(Job::Changed);
        Ok(())
    }

    fn disconnect(&self) {
        let mut w = self.0.lock().expect("world");
        w.connected = false;
        let _ = w.jobs.send(Job::Changed);
    }
}

/// What a test tells the stand-in to play.
struct Told(Shared);

#[zbus::interface(name = "org.olo.PhoneStandin")]
impl Told {
    fn receive(&self, name: &str, from: &str, text: &str) -> String {
        let w = self.0.lock().expect("world");
        let handle = w.phone.receive(name, from, text, &now());
        let _ = w.jobs.send(Job::Tell(handle.clone()));
        handle
    }

    fn set(&self, key: &str, value: &str) -> Result<(), Error> {
        let on = value == "yes";
        let mut w = self.0.lock().expect("world");
        match key {
            "messages" => w.phone.0.lock().expect("phone").messages_allowed = on,
            "contacts" => w.phone.0.lock().expect("phone").contacts_allowed = on,
            "connected" => w.connected = on,
            "paired" => w.paired = on,
            _ => return Err(Error::InvalidArguments(format!("nothing called {key}"))),
        }
        let _ = w.jobs.send(Job::Changed);
        Ok(())
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
       phone-standin [--address ADDRESS] --tell set messages|contacts|connected|paired yes|no
       phone-standin [--address ADDRESS] --tell sent";

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
    let world: Shared = Arc::new(Mutex::new(World { phone: phone.clone(), profiles: HashMap::new(), paired: !unpaired, connected: !unpaired, android, jobs: tx }));
    let built = zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(|b| b.serve_at("/", zbus::fdo::ObjectManager))
        .and_then(|b| b.serve_at("/org/bluez", Manager(world.clone())))
        .and_then(|b| b.serve_at("/org/bluez", Told(world.clone())))
        .and_then(|b| b.serve_at(ADAPTER, Adapter))
        .and_then(|b| b.serve_at(DEVICE, Device(world.clone())))
        .and_then(|b| b.name(NAME))
        .and_then(|b| b.build());
    let conn = match built {
        Ok(c) => c,
        Err(e) => {
            eprintln!("phone-standin: can't take {NAME} on {address}: {e}");
            std::process::exit(1);
        }
    };
    // What the methods ask for, done once they've answered: a channel
    // opened is a socket pair, one end the stand-in phone's, the other
    // handed to whoever registered the profile.
    for job in jobs {
        let hand_over = |uuid: &str, serve: Box<dyn FnOnce(UnixStream) + Send>| {
            let Some((who, path)) = world.lock().expect("world").profiles.get(uuid).cloned() else { return };
            let Ok((ours, theirs)) = UnixStream::pair() else { return };
            std::thread::spawn(move || serve(ours));
            let device = zvariant::ObjectPath::try_from(DEVICE).expect("a path");
            let fd = zvariant::OwnedFd::from(OwnedFd::from(theirs));
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
            Job::Changed => {
                let w = world.lock().expect("world");
                let mut changed: HashMap<&str, Value> = HashMap::new();
                changed.insert("Connected", w.connected.into());
                changed.insert("Paired", w.paired.into());
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
        ["sent"] => call(conn.call_method(Some(NAME), "/org/bluez", Some(STANDIN), "Sent", &())),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}
