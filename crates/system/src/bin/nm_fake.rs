//! `nm-fake [--address ADDRESS] [--network SSID:PASSWORD]… [--enterprise
//! SSID]… [--dump FILE]`: a stand-in NetworkManager for tests, so nothing
//! touches the real one (its saved networks and their passwords). It
//! takes the name org.freedesktop.NetworkManager on the bus at ADDRESS
//! (default: the session bus, `DBUS_SESSION_BUS_ADDRESS`) and answers what
//! Backup asks of `olo_system::network`: the saved connections
//! (`ListConnections`, each one's `GetSettings` and `GetSecrets`), and
//! saving another (`AddConnection`). Each `--network` is a saved WPA one
//! (an empty password: an open one), each `--enterprise` a WPA-Enterprise
//! one. `--dump FILE` is rewritten with every saved connection (as JSON:
//! id, ssid, key-mgmt, psk) whenever one is added. Point programs at it
//! with `OLO_NM_BUS=ADDRESS`.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use zvariant::{OwnedObjectPath, OwnedValue, Value};

const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";

#[derive(Debug, Clone, serde::Serialize)]
struct Saved {
    id: String,
    ssid: String,
    hidden: bool,
    autoconnect: bool,
    key_mgmt: String,
    psk: String,
    enterprise: bool,
}

struct Model {
    saved: Vec<Saved>,
    dump: Option<PathBuf>,
}

type Shared = Arc<Mutex<Model>>;

fn path_of(i: usize) -> String {
    format!("{SETTINGS_PATH}/{}", i + 1)
}

fn value(v: Value<'_>) -> OwnedValue {
    v.try_to_owned().expect("a value")
}

impl Model {
    fn write_dump(&self) {
        if let Some(dump) = &self.dump {
            let _ = std::fs::write(dump, serde_json::to_vec_pretty(&self.saved).unwrap_or_default());
        }
    }
}

struct Settings {
    model: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl Settings {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        let m = self.model.lock().unwrap();
        (0..m.saved.len()).filter_map(|i| OwnedObjectPath::try_from(path_of(i)).ok()).collect()
    }

    async fn add_connection(&self, settings: HashMap<String, HashMap<String, OwnedValue>>, #[zbus(object_server)] server: &zbus::ObjectServer) -> zbus::fdo::Result<OwnedObjectPath> {
        let text = |g: &str, k: &str| settings.get(g).and_then(|s| s.get(k)).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
        let flag = |g: &str, k: &str, or: bool| settings.get(g).and_then(|s| s.get(k)).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(or);
        let ssid = settings.get("802-11-wireless").and_then(|s| s.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.clone()).ok()).ok_or_else(|| zbus::fdo::Error::InvalidArgs("no ssid".into()))?;
        if text("connection", "type") != "802-11-wireless" || text("connection", "uuid").len() != 36 {
            return Err(zbus::fdo::Error::InvalidArgs("not a Wi-Fi connection with a uuid".into()));
        }
        let saved = Saved {
            id: text("connection", "id"),
            ssid: String::from_utf8_lossy(&ssid).into_owned(),
            hidden: flag("802-11-wireless", "hidden", false),
            autoconnect: flag("connection", "autoconnect", true),
            key_mgmt: text("802-11-wireless-security", "key-mgmt"),
            psk: text("802-11-wireless-security", "psk"),
            enterprise: settings.contains_key("802-1x"),
        };
        let path = {
            let mut m = self.model.lock().unwrap();
            m.saved.push(saved);
            m.write_dump();
            path_of(m.saved.len() - 1)
        };
        server.at(path.as_str(), Connection { model: self.model.clone(), path: path.clone() }).await?;
        OwnedObjectPath::try_from(path).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

struct Connection {
    model: Shared,
    path: String,
}

impl Connection {
    fn saved(&self) -> zbus::fdo::Result<Saved> {
        let m = self.model.lock().unwrap();
        (0..m.saved.len()).find(|i| path_of(*i) == self.path).map(|i| m.saved[i].clone()).ok_or_else(|| zbus::fdo::Error::UnknownObject(self.path.clone()))
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Connection {
    fn get_settings(&self) -> zbus::fdo::Result<HashMap<String, HashMap<String, OwnedValue>>> {
        let s = self.saved()?;
        let mut out: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
        out.insert(
            "connection".into(),
            HashMap::from([("id".into(), value(Value::from(s.id.as_str()))), ("type".into(), value(Value::from("802-11-wireless"))), ("autoconnect".into(), value(Value::from(s.autoconnect)))]),
        );
        out.insert(
            "802-11-wireless".into(),
            HashMap::from([("ssid".into(), value(Value::from(s.ssid.as_bytes().to_vec()))), ("mode".into(), value(Value::from("infrastructure"))), ("hidden".into(), value(Value::from(s.hidden)))]),
        );
        if s.enterprise {
            out.insert("802-11-wireless-security".into(), HashMap::from([("key-mgmt".into(), value(Value::from("wpa-eap")))]));
            out.insert("802-1x".into(), HashMap::from([("eap".into(), value(Value::from(vec!["peap"])))]));
        } else if !s.key_mgmt.is_empty() {
            out.insert("802-11-wireless-security".into(), HashMap::from([("key-mgmt".into(), value(Value::from(s.key_mgmt.as_str())))]));
        }
        Ok(out)
    }

    fn get_secrets(&self, setting: String) -> zbus::fdo::Result<HashMap<String, HashMap<String, OwnedValue>>> {
        let s = self.saved()?;
        let mut out = HashMap::new();
        if setting == "802-11-wireless-security" && !s.psk.is_empty() {
            out.insert(setting, HashMap::from([("psk".into(), value(Value::from(s.psk.as_str())))]));
        }
        Ok(out)
    }
}

fn main() {
    olo_crash::install();
    let mut address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let (mut saved, mut dump) = (Vec::new(), None);
    let mut args = std::env::args().skip(1);
    let usage = || -> ! {
        eprintln!("usage: nm-fake [--address ADDRESS] [--network SSID:PASSWORD]… [--enterprise SSID]… [--dump FILE]");
        std::process::exit(2)
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().unwrap_or_else(|| usage()),
            "--network" => {
                let n = args.next().unwrap_or_else(|| usage());
                let (ssid, psk) = n.split_once(':').unwrap_or((n.as_str(), ""));
                let key_mgmt = if psk.is_empty() { "" } else { "wpa-psk" };
                saved.push(Saved { id: ssid.into(), ssid: ssid.into(), hidden: false, autoconnect: true, key_mgmt: key_mgmt.into(), psk: psk.into(), enterprise: false });
            }
            "--enterprise" => {
                let ssid = args.next().unwrap_or_else(|| usage());
                saved.push(Saved { id: ssid.clone(), ssid, hidden: false, autoconnect: true, key_mgmt: "wpa-eap".into(), psk: String::new(), enterprise: true });
            }
            "--dump" => dump = Some(PathBuf::from(args.next().unwrap_or_else(|| usage()))),
            _ => usage(),
        }
    }
    let n = saved.len();
    let model: Shared = Arc::new(Mutex::new(Model { saved, dump }));
    model.lock().unwrap().write_dump();
    let conn = match zbus::blocking::connection::Builder::address(address.as_str()).and_then(|b| b.name("org.freedesktop.NetworkManager")).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nm-fake: can't take org.freedesktop.NetworkManager on {address}: {e}");
            std::process::exit(1);
        }
    };
    let server = conn.object_server();
    server.at(SETTINGS_PATH, Settings { model: model.clone() }).expect("settings");
    for i in 0..n {
        server.at(path_of(i).as_str(), Connection { model: model.clone(), path: path_of(i) }).expect("a connection");
    }
    eprintln!("nm-fake: serving {n} saved networks on {address}");
    loop {
        std::thread::park();
    }
}
