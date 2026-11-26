//! A bus of a test's own.

use std::{
    io::{BufRead as _, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// A running `olo-bus` and its socket's file: ended and removed with it.
pub struct Bus(pub Child, pub String);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// A bus on a socket of its own and its address, once it says it is
/// ready; None if it hasn't `within` (it is ended then). Never a wait
/// without an end: a bus that didn't say it once held a whole run of the
/// workspace's tests for 25 minutes (2026-11-26).
pub fn bus(name: &str, within: Duration) -> Option<(Bus, String)> {
    let socket = format!("{}/olo-bus-{name}-{}.sock", env!("CARGO_TARGET_TMPDIR"), std::process::id());
    let _ = std::fs::remove_file(&socket);
    let address = format!("unix:path={socket}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-bus")).args([address.as_str(), "--ready"]).stdout(Stdio::piped()).spawn().expect("olo-bus");
    let out = child.stdout.take().unwrap();
    let bus = Bus(child, socket);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(out).read_line(&mut line);
        let _ = tx.send(line);
    });
    (rx.recv_timeout(within).ok()?.trim() == "ready").then_some((bus, address))
}
