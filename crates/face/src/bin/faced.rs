//! olo-faced: face unlock's service, as root, on the system bus as
//! `org.olo.Face1` (systemd starts it when it's first asked).
//!
//!   olo-faced serve [--bus ADDRESS] [--root DIR] [--models DIR] [--camera-dir DIR]
//!   olo-faced install --models DIR --pam FILE     (root)
//!   olo-faced uninstall                           (root)
//!
//! It alone opens the infrared camera, runs the models and keeps each
//! person's enrolled face (`/var/lib/olo-face/UID.json`, root's only), so
//! nothing running as the person can read the faces or skip the look.
//! Who asks is known from the bus (`GetConnectionUnixUser`): a person
//! may ask about themselves, root (sudo's and polkit's PAM module) about
//! anyone. Enrolling and forgetting take the account password (checked
//! through PAM), so something running as the person can't put its own
//! face in. The camera is opened for a look and closed after it.
//!
//! `--bus`, `--root`, `--camera-dir` are for tests (a private bus, a
//! folder for the faces, pictures for frames); `--camera-dir` exists only
//! on the command line, never in the environment.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use olo_face::{camera, emitter, look, model::Models};

const NAME: &str = "org.olo.Face1";
const PATH: &str = "/org/olo/Face1";
const INSTALLED: &str = "/usr/local/libexec/olo-faced";
const MODELS: &str = "/usr/local/share/olo/face";
const PAM_MODULE: &str = "/usr/lib64/security/pam_olo_face.so";
const PAM_LINE: &str = "auth       sufficient   pam_olo_face.so   # Olo: face unlock";
const UNIT_FILE: &str = "/etc/systemd/system/olo-faced.service";
const UNIT: &str = "# Olo: face unlock's service (Preferences ▸ Security).
[Unit]
Description=Olo: face unlock

[Service]
Type=dbus
BusName=org.olo.Face1
ExecStart=/usr/local/libexec/olo-faced serve
";
const ACTIVATION_FILE: &str = "/usr/share/dbus-1/system-services/org.olo.Face1.service";
const ACTIVATION: &str = "# Olo: face unlock's service, started when first asked.
[D-BUS Service]
Name=org.olo.Face1
Exec=/usr/local/libexec/olo-faced serve
User=root
SystemdService=olo-faced.service
";
const POLICY_FILE: &str = "/etc/dbus-1/system.d/org.olo.Face1.conf";
const POLICY: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<!-- Olo: face unlock. Root owns the name; anyone may ask (the service
     checks who's asking, and about whom). -->
<busconfig>
  <policy user="root">
    <allow own="org.olo.Face1"/>
  </policy>
  <policy context="default">
    <allow send_destination="org.olo.Face1"/>
  </policy>
</busconfig>
"#;
/// The PAM services a look stands for the password in.
const PAM_SERVICES: [(&str, &str); 2] = [("/etc/pam.d/sudo", "/usr/lib/pam.d/sudo"), ("/etc/pam.d/polkit-1", "/usr/lib/pam.d/polkit-1")];

struct Face {
    root: PathBuf,
    models_dir: PathBuf,
    fake: Option<PathBuf>,
    models: Mutex<Option<Arc<Models>>>,
    /// One look at a time (the camera is one).
    camera: Mutex<()>,
}

impl Face {
    fn models(&self) -> Result<Arc<Models>, String> {
        let mut m = self.models.lock().unwrap();
        if m.is_none() {
            *m = Some(Arc::new(Models::load(&self.models_dir)?));
        }
        Ok(m.clone().expect("loaded"))
    }

    fn file(&self, uid: u32) -> PathBuf {
        self.root.join(format!("{uid}.json"))
    }

    fn enrolled(&self, uid: u32) -> Option<look::Enrolled> {
        serde_json::from_slice(&std::fs::read(self.file(uid)).ok()?).ok()
    }

    fn emitters(&self) -> Vec<emitter::Emitter> {
        std::fs::read(self.root.join("emitters.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// The camera, open, its emitter lit if one was found for it.
    fn open(&self) -> Result<Box<dyn camera::Frames + Send>, String> {
        if let Some(dir) = &self.fake {
            return camera::open(Some(dir)).map_err(|e| e.to_string());
        }
        let dev = camera::infrared().into_iter().next().ok_or("no infrared camera")?;
        let cam = camera::Camera::open(&dev).map_err(|e| format!("{}: {e}", dev.display()))?;
        let id = camera_id(&dev);
        if let Some(e) = self.emitters().into_iter().find(|e| e.camera == id) {
            let _ = emitter::set(cam.fd(), e.unit, e.selector, &e.value);
        }
        Ok(Box::new(cam))
    }

    /// Find the camera's emitter if it isn't lit and none is known.
    fn light(&self) -> Result<(), String> {
        if self.fake.is_some() {
            return Ok(());
        }
        let dev = camera::infrared().into_iter().next().ok_or("no infrared camera")?;
        let id = camera_id(&dev);
        let mut known = self.emitters();
        if known.iter().any(|e| e.camera == id) {
            return Ok(());
        }
        let mut cam = camera::Camera::open(&dev).map_err(|e| e.to_string())?;
        let iface = camera::usb_interface(&dev).ok_or("the camera's interface")?;
        let fd = cam.fd().try_clone().map_err(|e| e.to_string())?;
        if let Some(e) = emitter::find(&fd, &mut cam, &iface, &id, &|s| tracing::info!("emitter: {s}")) {
            known.push(e);
            write_private(&self.root.join("emitters.json"), &serde_json::to_vec_pretty(&known).unwrap_or_default())?;
        }
        Ok(())
    }
}

/// A camera node's USB vendor and product ("04f2:b8b3").
fn camera_id(dev: &Path) -> String {
    let Some(iface) = camera::usb_interface(dev) else { return String::new() };
    let Some(usb) = iface.parent() else { return String::new() };
    let read = |f: &str| std::fs::read_to_string(usb.join(f)).map(|s| s.trim().to_owned()).unwrap_or_default();
    format!("{}:{}", read("idVendor"), read("idProduct"))
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// A user's uid, by name.
fn uid_of(user: &str) -> Option<u32> {
    let c = std::ffi::CString::new(user).ok()?;
    // SAFETY: getpwnam's entry is read at once.
    let p = unsafe { libc::getpwnam(c.as_ptr()) };
    (!p.is_null()).then(|| unsafe { (*p).pw_uid })
}

/// Whether the message's sender may ask about `user`: root, or `user`.
async fn allowed(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>, user: &str) -> Result<u32, zbus::fdo::Error> {
    let sender = hdr.sender().ok_or_else(|| zbus::fdo::Error::AccessDenied("who's asking?".into()))?;
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let caller = dbus.get_connection_unix_user(zbus::names::BusName::Unique(sender.clone())).await?;
    let uid = uid_of(user).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no user {user}")))?;
    if caller != 0 && caller != uid {
        return Err(zbus::fdo::Error::AccessDenied("only about yourself".into()));
    }
    Ok(uid)
}

/// `work` on a thread of its own (the camera and the models block).
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    rx.recv().await.expect("the work's answer")
}

struct Service {
    face: Arc<Face>,
}

#[zbus::interface(name = "org.olo.Face1")]
impl Service {
    /// What there is for `user`, as JSON: a camera, a face enrolled.
    async fn status(&self, user: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<String> {
        let uid = allowed(conn, &hdr, &user).await?;
        let camera = self.face.fake.is_some() || !camera::infrared().is_empty();
        let enrolled = self.face.enrolled(uid).map_or(0, |e| e.faces.len());
        Ok(serde_json::json!({ "camera": camera, "enrolled": enrolled }).to_string())
    }

    /// Look for `user`'s face for up to `timeout_ms`: "match",
    /// "no-match", "no-face", "flat", "not-enrolled", "no-camera".
    async fn verify(&self, user: String, timeout_ms: u32, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<String> {
        let uid = allowed(conn, &hdr, &user).await?;
        let face = self.face.clone();
        let limit = Duration::from_millis(timeout_ms.clamp(500, 10_000) as u64);
        Ok(blocking(move || {
            let Some(enrolled) = face.enrolled(uid) else { return "not-enrolled".to_owned() };
            let _one = face.camera.lock().unwrap();
            let models = match face.models() {
                Ok(m) => m,
                Err(e) => return format!("error: {e}"),
            };
            let mut frames = match face.open() {
                Ok(f) => f,
                Err(e) => {
                    tracing::info!("verify: {e}");
                    return "no-camera".to_owned();
                }
            };
            let outcome = look::verify(&mut *frames, &models, &enrolled, limit);
            tracing::info!(uid, ?outcome, "verify");
            match outcome {
                look::Outcome::Match(_) => "match",
                look::Outcome::NoMatch(_) => "no-match",
                look::Outcome::NoFace => "no-face",
                look::Outcome::Flat => "flat",
            }
            .to_owned()
        })
        .await)
    }

    /// Enrol `user`'s face (their password first): "enrolled",
    /// "wrong-password", "no-face", "no-camera". `Progress` says how many
    /// looks so far.
    async fn enroll(
        &self,
        user: String,
        password: String,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::fdo::Result<String> {
        let uid = allowed(conn, &hdr, &user).await?;
        let face = self.face.clone();
        let user_name = user.clone();
        let (tx, rx) = async_channel::unbounded::<usize>();
        let work = blocking(move || {
            if olo_system::auth::check_password(&user, &password).is_err() {
                return "wrong-password".to_owned();
            }
            drop(password);
            let _one = face.camera.lock().unwrap();
            let models = match face.models() {
                Ok(m) => m,
                Err(e) => return format!("error: {e}"),
            };
            if let Err(e) = face.light() {
                tracing::info!("enroll: the emitter: {e}");
            }
            let mut frames = match face.open() {
                Ok(f) => f,
                Err(e) => {
                    tracing::info!("enroll: {e}");
                    return "no-camera".to_owned();
                }
            };
            match look::enrol(&mut *frames, &models, Duration::from_secs(20), &|n| {
                let _ = tx.send_blocking(n);
            }) {
                Some(e) => match write_private(&face.file(uid), &serde_json::to_vec(&e).unwrap_or_default()) {
                    Ok(()) => "enrolled".to_owned(),
                    Err(e) => format!("error: {e}"),
                },
                None => "no-face".to_owned(),
            }
        });
        let progress = async {
            while let Ok(n) = rx.recv().await {
                let _ = Service::progress(&emitter, &user_name, n as u32, look::ENROL as u32).await;
            }
        };
        let (result, ()) = futures_join(work, progress).await;
        Ok(result)
    }

    /// Forget `user`'s face (their password first).
    async fn forget(&self, user: String, password: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<String> {
        let uid = allowed(conn, &hdr, &user).await?;
        let face = self.face.clone();
        Ok(blocking(move || {
            if olo_system::auth::check_password(&user, &password).is_err() {
                return "wrong-password".to_owned();
            }
            match std::fs::remove_file(face.file(uid)) {
                Ok(()) => "forgotten".to_owned(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "forgotten".to_owned(),
                Err(e) => format!("error: {e}"),
            }
        })
        .await)
    }

    #[zbus(signal)]
    async fn progress(emitter: &zbus::object_server::SignalEmitter<'_>, user: &str, got: u32, of: u32) -> zbus::Result<()>;
}

/// Both futures, to their ends.
async fn futures_join<A: std::future::Future, B: std::future::Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(|cx| {
        if ra.is_none()
            && let std::task::Poll::Ready(v) = a.as_mut().poll(cx)
        {
            ra = Some(v);
        }
        if rb.is_none()
            && let std::task::Poll::Ready(v) = b.as_mut().poll(cx)
        {
            rb = Some(v);
        }
        if ra.is_some() && rb.is_some() { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
    })
    .await;
    (ra.expect("a"), rb.expect("b"))
}

fn serve(args: &[String]) -> i32 {
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let face = Arc::new(Face {
        root: arg("--root").map_or_else(|| PathBuf::from("/var/lib/olo-face"), PathBuf::from),
        models_dir: arg("--models").map_or_else(|| PathBuf::from(MODELS), PathBuf::from),
        fake: arg("--camera-dir").map(PathBuf::from),
        models: Mutex::new(None),
        camera: Mutex::new(()),
    });
    let builder = match arg("--bus") {
        Some(a) => zbus::blocking::connection::Builder::address(a.as_str()),
        None => zbus::blocking::connection::Builder::system(),
    };
    let conn = match builder.and_then(|b| b.name(NAME)).and_then(|b| b.serve_at(PATH, Service { face })).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("olo-faced: {e}");
            return 1;
        }
    };
    tracing::info!("serving {NAME}");
    let _keep = conn;
    loop {
        std::thread::park();
    }
}

/// `text` with `line` as its first `auth` line (once).
fn with_line(text: &str, line: &str) -> String {
    if text.lines().any(|l| l.contains("pam_olo_face.so")) {
        return text.to_owned();
    }
    let mut out = Vec::new();
    let mut put = false;
    for l in text.lines() {
        if !put && l.trim_start().starts_with("auth") {
            out.push(line.to_owned());
            put = true;
        }
        out.push(l.to_owned());
    }
    if !put {
        out.push(line.to_owned());
    }
    out.join("\n") + "\n"
}

fn without_line(text: &str) -> String {
    text.lines().filter(|l| !l.contains("pam_olo_face.so")).collect::<Vec<_>>().join("\n") + "\n"
}

fn install(args: &[String]) -> Result<(), String> {
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let models = PathBuf::from(arg("--models").ok_or("--models DIR")?);
    let pam = PathBuf::from(arg("--pam").ok_or("--pam FILE")?);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let copy = |from: &Path, to: &str, mode: u32| -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(d) = Path::new(to).parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = format!("{to}.olo-new");
        std::fs::copy(from, &tmp).map_err(|e| format!("{} → {to}: {e}", from.display()))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, to).map_err(|e| format!("{to}: {e}"))
    };
    let write = |to: &str, text: &str| -> Result<(), String> {
        if let Some(d) = Path::new(to).parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(to, text).map_err(|e| format!("{to}: {e}"))
    };
    copy(&exe, INSTALLED, 0o755)?;
    for m in ["yunet.onnx", "sface.onnx"] {
        copy(&models.join(m), &format!("{MODELS}/{m}"), 0o644)?;
    }
    copy(&pam, PAM_MODULE, 0o755)?;
    write(UNIT_FILE, UNIT)?;
    write(ACTIVATION_FILE, ACTIVATION)?;
    write(POLICY_FILE, POLICY)?;
    // The look before the password: a service's own file (or, where only
    // the distribution's is, a copy of it in /etc, which wins).
    for (own, vendor) in PAM_SERVICES {
        let text = std::fs::read_to_string(own).or_else(|_| std::fs::read_to_string(vendor));
        if let Ok(text) = text {
            write(own, &with_line(&text, PAM_LINE))?;
        }
    }
    olo_system::systemd::reload().map_err(|e| format!("systemd: {e}"))?;
    reload_bus();
    Ok(())
}

fn uninstall() -> Result<(), String> {
    for (own, _) in PAM_SERVICES {
        if let Ok(text) = std::fs::read_to_string(own) {
            std::fs::write(own, without_line(&text)).map_err(|e| format!("{own}: {e}"))?;
        }
    }
    let _ = olo_system::systemd::start_stop("olo-faced.service", false);
    for f in [UNIT_FILE, ACTIVATION_FILE, POLICY_FILE, PAM_MODULE, INSTALLED] {
        match std::fs::remove_file(f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{f}: {e}")),
            _ => {}
        }
    }
    let _ = std::fs::remove_dir_all(MODELS);
    olo_system::systemd::reload().map_err(|e| format!("systemd: {e}"))?;
    reload_bus();
    Ok(())
}

/// The bus reads its policy again (the new name's).
fn reload_bus() {
    if let Ok(c) = zbus::blocking::Connection::system() {
        let _ = c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ReloadConfig", &());
    }
}

fn main() {
    olo_crash::install();
    tracing_subscriber_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some("install") => match install(&args[1..]) {
            Ok(()) => 0,
            Err(e) => {
                println!("error: {e}");
                1
            }
        },
        Some("uninstall") => match uninstall() {
            Ok(()) => 0,
            Err(e) => {
                println!("error: {e}");
                1
            }
        },
        _ => {
            eprintln!("usage: olo-faced serve [--bus ADDRESS] [--root DIR] [--models DIR] [--camera-dir DIR] | install --models DIR --pam FILE | uninstall");
            2
        }
    };
    std::process::exit(code);
}

/// Its log to stderr (the journal, under systemd).
fn tracing_subscriber_init() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).with_target(false).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pam_lines_in_and_out() {
        let sudo = "#%PAM-1.0\nauth       include      system-auth\naccount    include      system-auth\n";
        let with = with_line(sudo, PAM_LINE);
        assert_eq!(with.lines().nth(1), Some(PAM_LINE));
        assert_eq!(with_line(&with, PAM_LINE), with);
        assert_eq!(without_line(&with), sudo);
    }
}
