//! Stand-ins for fprintd and olo-faced on a bus of a test's own, for
//! trying what waits on a finger or a look (Passwords' Allow and its lock,
//! the lock screen) with nobody's reader or camera. Each answers as asked
//! on its command line and prints what it's asked, so a run says which of
//! them answered.
//!
//! `cargo run -p olo-system --example fake-readers -- --bus ADDRESS
//! [--finger match|no-match|none] [--face match|no-face|none] [--after MS]`,
//! then the program with `OLO_FPRINT_BUS=ADDRESS` (a debug build) and
//! `OLO_FACE_BUS=ADDRESS`. `none`: nothing enrolled. `--after`: how long a
//! finger or a look takes to answer (default 600 ms).

use std::time::Duration;

use zbus::object_server::SignalEmitter;

struct Opts {
    finger: String,
    face: String,
    after: Duration,
}

struct Manager;

#[zbus::interface(name = "net.reactivated.Fprint.Manager")]
impl Manager {
    fn get_default_device(&self) -> zbus::zvariant::OwnedObjectPath {
        zbus::zvariant::OwnedObjectPath::try_from("/net/reactivated/Fprint/Device/0").expect("path")
    }
}

struct Device(std::sync::Arc<Opts>);

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl Device {
    fn claim(&self, user: &str) {
        println!("fprintd: Claim {user}");
    }

    fn release(&self) {
        println!("fprintd: Release");
    }

    fn list_enrolled_fingers(&self, user: &str) -> Vec<String> {
        println!("fprintd: ListEnrolledFingers {user}");
        if self.0.finger == "none" { Vec::new() } else { vec!["right-index-finger".into()] }
    }

    fn verify_start(&self, finger: &str, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        println!("fprintd: VerifyStart {finger}");
        let (result, after, emitter) = (if self.0.finger == "match" { "verify-match" } else { "verify-no-match" }, self.0.after, emitter.into_owned());
        std::thread::spawn(move || {
            std::thread::sleep(after);
            println!("fprintd: VerifyStatus {result}");
            let _ = futures_lite::future::block_on(Device::verify_status(&emitter, result, true));
        });
    }

    fn verify_stop(&self) {
        println!("fprintd: VerifyStop");
    }

    #[zbus(property, name = "scan-type")]
    fn scan_type(&self) -> String {
        "press".into()
    }

    #[zbus(signal)]
    async fn verify_status(emitter: &SignalEmitter<'_>, result: &str, done: bool) -> zbus::Result<()>;
}

struct Face(std::sync::Arc<Opts>);

#[zbus::interface(name = "org.olo.Face1")]
impl Face {
    fn status(&self, user: &str) -> String {
        println!("face: Status {user}");
        let enrolled = if self.0.face == "none" { 0 } else { 5 };
        format!("{{\"camera\": true, \"enrolled\": {enrolled}}}")
    }

    fn verify(&self, user: &str, timeout_ms: u32) -> String {
        println!("face: Verify {user} ({timeout_ms} ms)");
        std::thread::sleep(self.0.after);
        let answer = match self.0.face.as_str() {
            "match" => "match",
            "none" => "not-enrolled",
            _ => "no-face",
        };
        println!("face: {answer}");
        answer.into()
    }
}

fn main() -> zbus::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let bus = arg("--bus").expect("--bus ADDRESS");
    let opts = std::sync::Arc::new(Opts {
        finger: arg("--finger").unwrap_or_else(|| "match".into()),
        face: arg("--face").unwrap_or_else(|| "match".into()),
        after: Duration::from_millis(arg("--after").and_then(|a| a.parse().ok()).unwrap_or(600)),
    });
    let _conn = zbus::blocking::connection::Builder::address(bus.as_str())?
        .name("net.reactivated.Fprint")?
        .name("org.olo.Face1")?
        .serve_at("/net/reactivated/Fprint/Manager", Manager)?
        .serve_at("/net/reactivated/Fprint/Device/0", Device(opts.clone()))?
        .serve_at("/org/olo/Face1", Face(opts.clone()))?
        .build()?;
    println!("fake readers on {bus}: finger {}, face {}, after {:?}", opts.finger, opts.face, opts.after);
    loop {
        std::thread::park();
    }
}
