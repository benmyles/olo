//! `shape IFACE` prints a link's shaping; `shape IFACE clear` takes
//! Olo's away; `shape IFACE DOWN UP`, each `RATE_KBIT,DELAY_MS,JITTER_MS,LOSS`
//! or `-` (as it is), shapes it (root). For trying `olo_system::shaping`
//! in a network namespace of its own: `sudo unshare -n …`.

fn shape(s: &str) -> Option<olo_system::shaping::Shape> {
    if s == "-" {
        return None;
    }
    let v: Vec<&str> = s.split(',').collect();
    Some(olo_system::shaping::Shape { rate_kbit: v[0].parse().unwrap(), delay_ms: v[1].parse().unwrap(), jitter_ms: v[2].parse().unwrap(), loss: v[3].parse().unwrap() })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let iface = &args[0];
    let r = match args.get(1).map(String::as_str) {
        None => olo_system::shaping::read(iface).map(|s| println!("{s:?}")),
        Some("clear") => olo_system::shaping::clear(iface),
        Some(down) => olo_system::shaping::apply(iface, &olo_system::shaping::Shaping { down: shape(down), up: shape(&args[2]) }),
    };
    if let Err(e) = r {
        eprintln!("shape: {e}");
        std::process::exit(1);
    }
}
