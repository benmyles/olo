//! `olo-airplay`'s pairing commands, run as a person runs them, against
//! a receiver of the test's own on the loopback (`--at`): never the
//! network, never a real receiver. Its pairings go to a folder of the
//! test's own.

use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use olo_airplay::pairing::fake;

fn run(state: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_olo-airplay")).args(args).env("XDG_STATE_HOME", state).env_remove("WAYLAND_DISPLAY").env_remove("OLO_SOCKET").output().expect("olo-airplay runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn paired_checked_listed_and_forgotten() {
    let state = std::path::PathBuf::from(format!("/var/tmp/olo-airplay-cli-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let tv = Arc::new(Mutex::new(fake::Receiver::new("4321")));
    let at = |tv: &Arc<Mutex<fake::Receiver>>| fake::serve(tv.clone()).to_string();

    // Nothing paired yet.
    assert_eq!(run(&state, &["paired"]), (true, "No receiver is paired with.\n".into()));
    let (ok, said) = run(&state, &["check", "Front Room", "--at", &at(&tv)]);
    assert!(!ok && said.contains("isn't paired with"), "{said}");

    // The wrong code: said, and nothing kept.
    let (ok, said) = run(&state, &["pair", "Front", "Room", "--code", "0000", "--at", &at(&tv)]);
    assert!(!ok && said.contains("that isn't the code it shows"), "{said}");
    assert!(tv.lock().unwrap().shown);
    assert_eq!(run(&state, &["paired"]).1, "No receiver is paired with.\n");

    // The code it shows.
    let (ok, said) = run(&state, &["pair", "Front Room", "--code", "4321", "--at", &at(&tv)]);
    assert!(ok && said == "Paired with Front Room.\n", "{said}");
    assert_eq!(tv.lock().unwrap().paired.len(), 1);
    assert!(String::from_utf8_lossy(&tv.lock().unwrap().paired[0].2).contains("Olo"));
    assert_eq!(run(&state, &["paired"]).1, "Front Room  (AA:BB:CC:DD:EE:FF)\n");
    // Kept where only the person reads it.
    use std::os::unix::fs::PermissionsExt as _;
    let kept = state.join("olo/airplay/pairings.json");
    assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o600);

    // It knows us at a new connection, by what was kept.
    assert_eq!(run(&state, &["check", "front room", "--at", &at(&tv)]), (true, "front room knows this computer.\n".into()));

    // The receiver reset: what answers isn't what was paired with.
    tv.lock().unwrap().reset();
    let (ok, said) = run(&state, &["check", "Front Room", "--at", &at(&tv)]);
    assert!(!ok && said.contains("isn't the receiver that was paired with"), "{said}");

    // Forgotten here.
    let (ok, said) = run(&state, &["unpair", "front", "room"]);
    assert!(ok && said.starts_with("Forgotten here."), "{said}");
    assert_eq!(run(&state, &["paired"]).1, "No receiver is paired with.\n");
    assert!(!run(&state, &["unpair", "Front Room"]).0);

    // Nothing there to pair with; and no name given.
    let (ok, said) = run(&state, &["pair", "Gone", "--code", "1", "--at", "127.0.0.1:9"]);
    assert!(!ok && said.contains("couldn't be reached"), "{said}");
    assert!(!run(&state, &["pair"]).0);
    let _ = std::fs::remove_dir_all(&state);
}
