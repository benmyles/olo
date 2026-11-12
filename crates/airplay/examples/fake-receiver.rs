//! A receiver of a test's own, run by hand, for looking at what shows
//! around AirPlay (Preferences ▸ AirPlay, "Play On" in the apps'
//! menus, Deskbar's tray, `olo-airplay`) without a real one:
//! `cargo run --release -p olo-airplay --example fake-receiver --
//! [SECONDS [NAME…]]`. It listens on the loopback only, shows nothing,
//! pairs with the code 1234 and "plays" each film for SECONDS (600 if
//! not said). Each NAME is a receiver ("Front Room" if none is given).
//!
//! It prints the line that points Olo's programs at it in place of the
//! network: `OLO_AIRPLAY_RECEIVERS=…`. With that, `OLO_AIRPLAY_DIR`
//! (where plays are said) and `XDG_STATE_HOME` (the pairings) set to
//! folders of the test's own for what is looked at, nothing of it
//! reaches the person's pairings or tray, and the network isn't asked.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use olo_airplay::{pairing, play};

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds = args.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(600);
    let mut names: Vec<String> = args.collect();
    if names.is_empty() {
        names.push("Front Room".into());
    }
    let receivers: Vec<String> = names
        .iter()
        .map(|name| {
            let tv = Arc::new(Mutex::new(play::fake::Tv::new(Arc::new(Mutex::new(pairing::fake::Receiver::new("1234"))))));
            tv.lock().unwrap().film = Duration::from_secs(seconds);
            format!("{name}={}", play::fake::serve_many(tv))
        })
        .collect();
    println!("{}={}", olo_airplay::RECEIVERS_ENV, receivers.join(";"));
    loop {
        std::thread::park();
    }
}
