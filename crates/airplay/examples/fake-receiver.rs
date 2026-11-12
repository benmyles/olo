//! A receiver of a test's own, run by hand, for looking at what shows
//! while a film plays (Deskbar's tray, `olo-airplay playing`) without a
//! real one: `cargo run --release -p olo-airplay --example fake-receiver
//! -- [SECONDS]`. It listens on the loopback only, shows nothing, and
//! "plays" each film for SECONDS (600 if not said). It prints where to
//! pair with it (its code is 1234) and where to play to it: each
//! address takes one connection, so there are several.
//!
//! With `OLO_AIRPLAY_DIR` and `XDG_STATE_HOME` set to folders of the
//! test's own for the commands (and `OLO_AIRPLAY_DIR` for the Deskbar
//! looked at), nothing of it reaches the person's pairings or tray.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use olo_airplay::{pairing, play};

fn main() {
    let seconds = std::env::args().nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(600);
    let receiver = Arc::new(Mutex::new(pairing::fake::Receiver::new("1234")));
    println!("pair --code 1234 --at {}", pairing::fake::serve(receiver.clone()));
    let tv = Arc::new(Mutex::new(play::fake::Tv::new(receiver)));
    tv.lock().unwrap().film = Duration::from_secs(seconds);
    for _ in 0..16 {
        println!("play --at {}", play::fake::serve(tv.clone()));
    }
    loop {
        std::thread::park();
    }
}
