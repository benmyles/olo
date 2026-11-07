//! olo-dconf: Olo's own dconf service (`ca.desrt.dconf`), in place of
//! GNOME's dconf-service, for every program that keeps settings in
//! GSettings (GTK and GNOME apps, Chrome; app_server's toolkit look,
//! `olo_system::dconf`). The same D-Bus interface and the same files, so
//! nothing reading them changes: programs read the database themselves
//! (`$XDG_CONFIG_HOME/dconf/user`, GLib's GVDB format: `gvdb.rs`) and ask
//! this service to change it.
//!
//! `ca.desrt.dconf.Writer` at `/ca/desrt/dconf/Writer/NAME` (NAME the
//! database, "user" unless a dconf profile names another):
//! - `Change(ay) → s`: a change (`changeset.rs`) made in the file as it is
//!   now (read again each time: nothing another writer did is lost),
//!   written whole and renamed into place, the readers' shared flag set
//!   (`$XDG_RUNTIME_DIR/dconf/NAME`: they map it, and open the file again
//!   once it says so), then the answer (a tag), then `Notify(prefix,
//!   paths, tag)` for every program watching.
//! - `Init()`: the database made if there is none.
//!
//! D-Bus starts it on demand while an Olo session runs: olo-session names
//! it for `ca.desrt.dconf` in the runtime folder
//! (`$XDG_RUNTIME_DIR/dbus-1/services`, read before the system's), and
//! stops GNOME's if it was running (session's graphical.rs). It goes
//! after ten minutes with nothing asked (D-Bus starts it again), giving
//! up its name first and answering what came meanwhile.
//!
//! ```text
//! olo-dconf                  serve (as D-Bus starts it)
//! olo-dconf --bus ADDRESS    serve on another bus (a test's own)
//! olo-dconf --rewrite IN OUT write database IN again as olo-dconf
//!                            writes it (to check it reads the same:
//!                            `dconf dump /` with XDG_CONFIG_HOME at OUT's)
//! ```

mod changeset;
mod gvdb;

use std::{
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const NAME: &str = "ca.desrt.dconf";
const PREFIX: &str = "/ca/desrt/dconf/Writer/";
const INTERFACE: &str = "ca.desrt.dconf.Writer";
/// Gone after this long with nothing asked.
const IDLE: Duration = Duration::from_secs(600);

const INTROSPECTION: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="ca.desrt.dconf.Writer">
    <method name="Init"/>
    <method name="Change"><arg name="blob" type="ay" direction="in"/><arg name="tag" type="s" direction="out"/></method>
    <signal name="Notify"><arg name="prefix" type="s"/><arg name="changes" type="as"/><arg name="tag" type="s"/></signal>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
    <method name="GetMachineId"><arg name="machine_uuid" type="s" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg name="xml_data" type="s" direction="out"/></method>
  </interface>
</node>
"#;

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// The database a writer object is for (its path's last part), if it's
/// one dconf could name.
fn database(path: &str) -> Option<&str> {
    let name = path.strip_prefix(PREFIX)?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')).then_some(name)
}

/// Where database `name` is kept.
fn file_of(name: &str) -> PathBuf {
    config_home().join("dconf").join(name)
}

/// Read database `name` as it is now (none: empty).
fn load(name: &str) -> Result<gvdb::Db, String> {
    match std::fs::read(file_of(name)) {
        Ok(b) => gvdb::read(&b).map_err(|e| format!("{}: {e}", file_of(name).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(gvdb::Db::new()),
        Err(e) => Err(format!("{}: {e}", file_of(name).display())),
    }
}

/// Write database `name` whole, renamed into place (its mode kept), then
/// tell the programs reading it to open it again.
fn save(name: &str, db: &gvdb::Db) -> Result<(), String> {
    let path = file_of(name);
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mode = std::fs::metadata(&path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o644);
    let tmp = dir.join(format!(".{name}.olo-{}", std::process::id()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        f.write_all(&gvdb::write(db))?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    flag(name);
    Ok(())
}

/// Set the shared flag readers of database `name` map
/// (`$XDG_RUNTIME_DIR/dconf/NAME`, its first byte), then take the file
/// away: each opens the database again, and a new flag, next time.
fn flag(name: &str) {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else { return };
    let path = Path::new(&runtime).join("dconf").join(name);
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
        use std::os::unix::fs::FileExt as _;
        let _ = f.write_at(&[1], 0);
        let _ = std::fs::remove_file(&path);
    }
}

/// What a call is answered with.
enum Answer {
    Tag(String, Option<(String, Vec<String>)>),
    Nothing,
    Text(String),
    Error(&'static str, String),
}

struct Service {
    /// Changes made so far (tags are this process's id and a count).
    count: u64,
}

impl Service {
    fn call(&mut self, msg: &zbus::Message) -> Answer {
        let header = msg.header();
        let (Some(path), Some(member)) = (header.path(), header.member()) else {
            return Answer::Error("org.freedesktop.DBus.Error.UnknownMethod", "no method".into());
        };
        let iface = header.interface().map(|i| i.as_str().to_owned()).unwrap_or_default();
        match (iface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus.Peer" | "", "Ping") => Answer::Nothing,
            ("org.freedesktop.DBus.Peer" | "", "GetMachineId") => Answer::Text(std::fs::read_to_string("/etc/machine-id").unwrap_or_default().trim().to_owned()),
            ("org.freedesktop.DBus.Introspectable" | "", "Introspect") => Answer::Text(if database(path.as_str()).is_some() { INTROSPECTION.to_owned() } else { "<node/>".to_owned() }),
            (INTERFACE | "", "Init" | "Change") => {
                let Some(name) = database(path.as_str()) else {
                    return Answer::Error("org.freedesktop.DBus.Error.UnknownObject", format!("no database at {}", path.as_str()));
                };
                if member.as_str() == "Init" {
                    return match load(name).and_then(|db| if file_of(name).exists() { Ok(()) } else { save(name, &db) }) {
                        Ok(()) => Answer::Nothing,
                        Err(e) => Answer::Error("org.freedesktop.DBus.Error.Failed", e),
                    };
                }
                let blob: Vec<u8> = match msg.body().deserialize() {
                    Ok(b) => b,
                    Err(e) => return Answer::Error("org.freedesktop.DBus.Error.InvalidArgs", e.to_string()),
                };
                let changes = match changeset::parse(&blob) {
                    Ok(c) => c,
                    Err(e) => return Answer::Error("org.freedesktop.DBus.Error.InvalidArgs", e),
                };
                self.count += 1;
                let tag = format!("olo-dconf:{}:{}", std::process::id(), self.count);
                if changes.is_empty() {
                    return Answer::Tag(tag, None);
                }
                let made = load(name).and_then(|mut db| {
                    changeset::apply(&mut db, &changes);
                    save(name, &db)
                });
                match made {
                    Ok(()) => Answer::Tag(tag, Some(changeset::notify_args(&changes))),
                    Err(e) => {
                        tracing::warn!("dconf: {e}");
                        Answer::Error("org.freedesktop.DBus.Error.Failed", e)
                    }
                }
            }
            _ => Answer::Error("org.freedesktop.DBus.Error.UnknownMethod", format!("no {iface}.{}", member.as_str())),
        }
    }
}

/// Answer a method call, then (for a change) tell everyone, in that
/// order: the program that changed it knows its tag before the signal.
fn answer(conn: &zbus::blocking::Connection, service: &mut Service, msg: &zbus::Message) {
    let header = msg.header();
    if header.message_type() != zbus::message::Type::MethodCall {
        return;
    }
    let path = header.path().map(|p| p.as_str().to_owned()).unwrap_or_default();
    let quiet = header.primary().flags().contains(zbus::message::Flags::NoReplyExpected);
    match service.call(msg) {
        Answer::Tag(tag, notify) => {
            if !quiet {
                let _ = conn.reply(&header, &(tag.as_str(),));
            }
            if let Some((prefix, paths)) = notify {
                let _ = conn.emit_signal(None::<()>, path.as_str(), INTERFACE, "Notify", &(prefix, paths, tag));
            }
        }
        Answer::Nothing if !quiet => {
            let _ = conn.reply(&header, &());
        }
        Answer::Text(t) if !quiet => {
            let _ = conn.reply(&header, &(t,));
        }
        Answer::Error(name, text) if !quiet => {
            if let Ok(name) = zbus::names::ErrorName::try_from(name) {
                let _ = conn.reply_error(&header, name, &(text,));
            }
        }
        _ => {}
    }
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, from, to] = args.as_slice()
        && flag == "--rewrite"
    {
        return match std::fs::read(from).map_err(|e| e.to_string()).and_then(|b| gvdb::read(&b)).and_then(|db| std::fs::write(to, gvdb::write(&db)).map(|()| db.len()).map_err(|e| e.to_string())) {
            Ok(n) => {
                println!("{n} keys");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-dconf: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    let builder = match args.as_slice() {
        [] => zbus::blocking::connection::Builder::session(),
        [flag, address] if flag == "--bus" => zbus::blocking::connection::Builder::address(address.as_str()),
        _ => {
            eprintln!("usage: olo-dconf [--bus ADDRESS | --rewrite IN OUT]");
            return std::process::ExitCode::from(2);
        }
    };
    let conn = match builder.and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("olo-dconf: the bus: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Messages read on a thread of their own, so being idle can end it.
    let (tx, rx) = mpsc::channel::<zbus::Message>();
    let reader = conn.clone();
    std::thread::Builder::new()
        .name("bus".into())
        .spawn(move || {
            for msg in zbus::blocking::MessageIterator::from(&reader).flatten() {
                if tx.send(msg).is_err() {
                    return;
                }
            }
        })
        .ok();
    // (Through the bus itself: zbus's own would start its object server,
    // which answers calls this loop answers.)
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return std::process::ExitCode::FAILURE };
    let Ok(name) = zbus::names::WellKnownName::try_from(NAME) else { return std::process::ExitCode::FAILURE };
    match dbus.request_name(name.clone(), zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner) => {}
        Ok(_) => {
            eprintln!("olo-dconf: {NAME} is someone else's");
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("olo-dconf: asking for {NAME}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }
    tracing::info!("dconf: serving {NAME} ({})", config_home().join("dconf").display());
    let mut service = Service { count: 0 };
    loop {
        match rx.recv_timeout(IDLE) {
            Ok(msg) => answer(&conn, &mut service, &msg),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => return std::process::ExitCode::FAILURE,
        }
    }
    // Idle: the name given up (calls from now start another), then what
    // came before that answered.
    let _ = dbus.release_name(name);
    while let Ok(msg) = rx.recv_timeout(Duration::from_millis(200)) {
        answer(&conn, &mut service, &msg);
    }
    tracing::info!("dconf: idle, gone ({} changes)", service.count);
    std::process::ExitCode::SUCCESS
}
