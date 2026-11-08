//! Setting up's two touches, on the first key plugged in, every CTAPHID
//! message to and from it printed with when (milliseconds): a credential
//! made for Olo (not discoverable: nothing is kept on the key), then its
//! secret. Nothing is sealed or saved, and no password is asked for: for
//! seeing what a key does when setting up doesn't go as it should.
//!
//! `cargo run -p olo-fido --example touch`

use std::{
    io,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use olo_fido::{
    cred,
    ctap::{Channel, Wait},
    hid::{Hidraw, REPORT, Transport},
};

/// A key's node, each report out and in printed.
struct Logged {
    key: Hidraw,
    start: Instant,
}

fn describe(r: &[u8; REPORT]) -> String {
    let cid = u32::from_be_bytes([r[0], r[1], r[2], r[3]]);
    if r[4] & 0x80 != 0 {
        let cmd = r[4] & 0x7F;
        let len = u16::from_be_bytes([r[5], r[6]]);
        let name = match cmd {
            0x01 => "PING",
            0x06 => "INIT",
            0x08 => "WINK",
            0x10 => "CBOR",
            0x11 => "CANCEL",
            0x3B => "KEEPALIVE",
            0x3F => "ERROR",
            _ => "?",
        };
        let first = if len > 0 { format!(" first byte {:#04x}", r[7]) } else { String::new() };
        format!("channel {cid:08x} {name} ({len} bytes){first}")
    } else {
        format!("channel {cid:08x} continuation {}", r[4])
    }
}

impl Transport for Logged {
    fn send(&mut self, r: &[u8; REPORT]) -> io::Result<()> {
        println!("{:7} ms  → {}", self.start.elapsed().as_millis(), describe(r));
        self.key.send(r)
    }

    fn recv(&mut self, wait: Duration) -> io::Result<Option<[u8; REPORT]>> {
        let got = self.key.recv(wait)?;
        if let Some(r) = &got {
            println!("{:7} ms  ← {}", self.start.elapsed().as_millis(), describe(r));
        }
        Ok(got)
    }
}

fn main() {
    let Some(d) = olo_fido::devices().into_iter().next() else {
        println!("No FIDO key is plugged in.");
        return;
    };
    println!("{} ({})", d.name, d.path.display());
    let start = Instant::now();
    let key = match Hidraw::open(&d) {
        Ok(k) => k,
        Err(e) => return println!("couldn't open it: {e}"),
    };
    let mut ch = match Channel::open(Logged { key, start }) {
        Ok(ch) => ch,
        Err(e) => return println!("no channel: {e}"),
    };
    let info = match ch.get_info() {
        Ok(i) => i,
        Err(e) => return println!("GetInfo: {e}"),
    };
    let never = AtomicBool::new(false);
    let touch = || println!("{:7} ms  ** the key wants a touch now **", start.elapsed().as_millis());
    let wait = Wait { cancel: &never, touch: &touch, limit: Duration::from_secs(40) };
    println!("{:7} ms  first touch: making a credential", start.elapsed().as_millis());
    // (Asked again when the key gives up waiting, three times in all.)
    let mut tries = 0;
    let made = loop {
        match cred::make(&mut ch, "olo-touch-test", None, &wait) {
            Ok(m) => break m,
            Err(olo_fido::Error::Ctap(0x27 | 0x2F)) if tries < 2 => {
                tries += 1;
                println!("{:7} ms  no touch came: asking again", start.elapsed().as_millis());
            }
            Err(e) => return println!("{:7} ms  making it: {e} ({e:?})", start.elapsed().as_millis()),
        }
    };
    println!("{:7} ms  made: credential of {} bytes", start.elapsed().as_millis(), made.id.len());
    println!("{:7} ms  second touch: its secret", start.elapsed().as_millis());
    match cred::secret(&mut ch, info.pin_protocol(), &made.id, &[7u8; 32], &wait) {
        Ok(_) => println!("{:7} ms  the secret came: setting up would have worked", start.elapsed().as_millis()),
        Err(e) => println!("{:7} ms  the secret: {e} ({e:?})", start.elapsed().as_millis()),
    }
}
