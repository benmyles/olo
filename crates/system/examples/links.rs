//! Prints the links Deskbar's tray shows (`olo_system::network::Links`)
//! now and whenever they change, as NetworkManager's signals say:
//! `OLO_NM_BUS=ADDRESS cargo run --example links` against `nm-fake`.
//! `--time`: how long a read of the first Wi-Fi adapter alone takes, and
//! one of the links (what the watch does on a signal), and nothing else.

fn main() {
    if std::env::args().any(|a| a == "--time") {
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let _ = olo_system::network::wifi();
            let wifi = t.elapsed();
            let t = std::time::Instant::now();
            let _ = olo_system::network::links();
            println!("wifi() {:.2} ms, links() {:.2} ms", wifi.as_secs_f64() * 1e3, t.elapsed().as_secs_f64() * 1e3);
        }
        return;
    }
    let start = std::time::Instant::now();
    if let Err(e) = olo_system::network::watch_links(move |l| {
        let wired = l.wired.as_ref().map(|d| format!("{} {:?} {:?} {:?}", d.interface, d.addresses, d.dns, d.speed));
        let wifi = l.wifi.as_ref().map(|w| format!("{:?} {}%", w.ssid, w.strength));
        let wifi_dev = l.wifi_device.as_ref().map(|d| format!("{} state {} {:?} {:?}", d.interface, d.state, d.addresses, d.dns));
        println!("{:>6} ms  primary {:?}  wired {wired:?}  wifi {wifi:?} {wifi_dev:?}", start.elapsed().as_millis(), l.primary);
    }) {
        eprintln!("links: {e}");
        std::process::exit(1);
    }
    loop {
        std::thread::park();
    }
}
