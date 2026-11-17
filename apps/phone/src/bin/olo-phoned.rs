//! olo-phoned: the session's phone service (service.rs: it speaks to the
//! phone over Bluetooth, tells of messages, finds one-time codes and
//! gives them to the browser tab waiting, and answers Phone, Deskbar,
//! Preferences and agents on its socket). The session starts it, and
//! starts it again if it ends.

fn main() {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        println!("usage: olo-phoned\n  the session's phone service (started by the session); olo-phone asks it things");
        return;
    }
    olo_phone::upgrade_on_signal();
    olo_phone::service::run();
}
