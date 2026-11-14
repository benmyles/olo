//! `olo-airplay`'s pairing and playing commands, run as a person runs
//! them, against a receiver of the test's own on the loopback (`--at`):
//! never the network, never a real receiver. Its pairings go to a folder
//! of the test's own.

use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use olo_airplay::{pairing::fake, play};

/// The command, with its pairings and its plays said in folders of the
/// test's own (never the person's: a film of theirs may be playing).
fn run(state: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_olo-airplay"))
        .args(args)
        .env("XDG_STATE_HOME", state)
        .env("XDG_RUNTIME_DIR", state.join("run"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .output()
        .expect("olo-airplay runs");
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

/// A film played from the command line: paired with, asked, fetched by
/// the receiver, said as it goes (in words and as JSON), and stopped
/// there when the command is told to end.
#[test]
fn a_film_played_and_one_stopped() {
    use std::io::{BufRead as _, BufReader};
    let state = std::path::PathBuf::from(format!("/var/tmp/olo-airplay-cli-play-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut bytes = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41".to_vec();
    bytes.extend((0..30_000u32).map(|i| (i % 249) as u8));
    let film = state.join("A film.mp4");
    std::fs::write(&film, &bytes).unwrap();
    let film = film.to_str().unwrap();

    let pairing = Arc::new(Mutex::new(fake::Receiver::new("4321")));
    let (ok, said) = run(&state, &["pair", "Front Room", "--code", "4321", "--at", &fake::serve(pairing.clone()).to_string()]);
    assert!(ok, "{said}");
    let tv = Arc::new(Mutex::new(play::fake::Tv::new(pairing)));
    let at = |tv: &Arc<Mutex<play::fake::Tv>>| play::fake::serve(tv.clone()).to_string();

    // In words.
    let (ok, said) = run(&state, &["play", "front", "room", film, "--at", &at(&tv)]);
    assert!(ok && said.ends_with("Playing on front room (it is 10:00 long).\nIt played to its end.\n"), "{said}");
    assert!(tv.lock().unwrap().fetched == bytes && tv.lock().unwrap().sender.starts_with("Olo"));
    assert_eq!(tv.lock().unwrap().asked.last().map(String::as_str), Some("TEARDOWN"));

    // As JSON, from a place, with each step said beside it.
    tv.lock().unwrap().asked.clear();
    let (ok, said) = run(&state, &["play", "Front Room", film, "--from", "1:30", "--json", "--trace", "--at", &at(&tv)]);
    assert!(ok, "{said}");
    let lines: Vec<serde_json::Value> = said.lines().filter(|l| l.starts_with('{')).map(|l| serde_json::from_str(l).unwrap()).collect();
    let states: Vec<&str> = lines.iter().map(|l| l["state"].as_str().unwrap()).filter(|s| *s != "loading").collect();
    assert_eq!(states, ["playing", "ended"], "{said}");
    assert_eq!((lines.iter().find(|l| l["state"] == "playing").unwrap()["duration"].as_f64(), tv.lock().unwrap().from), (Some(600.0), 90.0));
    assert!(said.contains("  SETUP: 200") && said.contains("  insertPlayQueueItem: 200") && said.contains("  it says: "), "{said}");
    // (What the trace says doesn't say where the film is.)
    assert!(!said.contains("/film.mp4"), "{said}");

    // Not paired with; not a film; no film named.
    let (ok, said) = run(&state, &["play", "Den", film, "--at", "127.0.0.1:9"]);
    assert!(!ok && said.contains("Den isn't paired with"), "{said}");
    std::fs::write(state.join("film.mkv"), [0x1a, 0x45, 0xdf, 0xa3, 1, 2, 3, 4]).unwrap();
    let (ok, said) = run(&state, &["play", "Front Room", state.join("film.mkv").to_str().unwrap(), "--at", "127.0.0.1:9"]);
    assert!(!ok && said.contains("Matroska"), "{said}");
    let (ok, said) = run(&state, &["play", "Front Room", "/var/tmp/olo-airplay-no-such-film.mp4", "--at", "127.0.0.1:9"]);
    assert!(!ok && said.contains("No such file"), "{said}");
    assert!(!run(&state, &["play", "Front Room"]).0);
    assert!(!run(&state, &["play", "Front Room", film, "--from", "later", "--at", "127.0.0.1:9"]).0);

    // Nothing is playing now, and nothing is left saying one is.
    assert_eq!(run(&state, &["playing"]), (true, "Nothing is playing.\n".into()));
    assert_eq!(std::fs::read_dir(state.join("run/olo-airplay")).unwrap().count(), 0);
    assert!(!run(&state, &["stop"]).0 && !run(&state, &["pause", "Front Room"]).0);

    // A long one: said to be playing, paused and gone on with from
    // another command, and stopped by one.
    tv.lock().unwrap().film = std::time::Duration::from_secs(120);
    tv.lock().unwrap().asked.clear();
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-airplay"))
        .args(["play", "Front Room", film, "--from", "2:00", "--at", &at(&tv)])
        .env("XDG_STATE_HOME", &state)
        .env("XDG_RUNTIME_DIR", state.join("run"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("olo-airplay runs");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut said = String::new();
    // (Read on until it has said this, this many times.)
    let mut until = |said: &mut String, what: &str, times: usize| {
        while said.matches(what).count() < times {
            assert!(out.read_line(said).unwrap() > 0, "it ended before saying {what:?}: {said}");
        }
    };
    until(&mut said, "Playing on Front Room, from 2:00 (it is 10:00 long).", 1);
    let (ok, listed) = run(&state, &["playing"]);
    assert!(ok && listed.starts_with("A film.mp4 on Front Room: playing, 2:0") && listed.ends_with(" of 10:00\n"), "{listed}");
    let (_, listed) = run(&state, &["playing", "--json"]);
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!((listed[0]["pid"].as_u64(), listed[0]["receiver"].as_str(), listed[0]["path"].as_str()), (Some(u64::from(child.id())), Some("Front Room"), Some(film)));
    // (Another receiver's plays: none. This one's, by the start of its name.)
    assert!(!run(&state, &["pause", "Den"]).0);
    assert!(!run(&state, &["pause", "--pid", "1"]).0 && !run(&state, &["pause", "--pid", "that one"]).0);
    assert_eq!(run(&state, &["pause", "--pid", &child.id().to_string()]), (true, "Paused A film.mp4 on Front Room.\n".into()));
    until(&mut said, "Paused at 2:0", 1);
    assert!(run(&state, &["playing"]).1.contains(": paused, 2:0"));
    assert_eq!(run(&state, &["resume", "front"]), (true, "Going on with A film.mp4 on Front Room.\n".into()));
    until(&mut said, "Playing on Front Room, from 2:0", 2);
    assert!(run(&state, &["playing"]).1.contains(": playing, 2:0"));
    assert_eq!(run(&state, &["stop", "Front Room"]), (true, "Stopped A film.mp4 on Front Room.\n".into()));
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(rest.ends_with("Stopped.\n"), "{rest}");
    assert_eq!(tv.lock().unwrap().asked.last().map(String::as_str), Some("TEARDOWN"));
    assert_eq!(run(&state, &["playing"]).1, "Nothing is playing.\n");
    let _ = std::fs::remove_dir_all(&state);
}

/// As an app asks for it: the receivers a test's own in place of the
/// network's (one address for pairing, proving and playing, as a real
/// one has), the receiver named by its id with `--on`, a film whose name
/// has spaces.
#[test]
fn asked_as_an_apps_menu_asks() {
    let state = std::path::PathBuf::from(format!("/var/tmp/olo-airplay-cli-menu-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut bytes = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41".to_vec();
    bytes.extend((0..20_000u32).map(|i| (i % 241) as u8));
    let film = state.join("A film of ours.mp4");
    std::fs::write(&film, &bytes).unwrap();
    let tv = Arc::new(Mutex::new(play::fake::Tv::new(Arc::new(Mutex::new(fake::Receiver::new("4321"))))));
    let receivers = format!("Front Room={};Den=127.0.0.1:9", play::fake::serve_many(tv.clone()));
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_olo-airplay"))
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_RUNTIME_DIR", state.join("run"))
            .env("OLO_AIRPLAY_RECEIVERS", &receivers)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("OLO_SOCKET")
            .output()
            .expect("olo-airplay runs");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };

    // Found without the network; paired, checked and listed as paired.
    let (ok, said) = run(&["list"]);
    assert!(ok && said.contains("Front Room  Apple TV") && said.contains("pairs first") && said.lines().count() == 2, "{said}");
    assert_eq!(run(&["pair", "front", "--code", "4321"]), (true, "Paired with Front Room.\n".into()));
    assert_eq!(run(&["check", "Front Room"]), (true, "Front Room knows this computer.\n".into()));
    assert!(run(&["list"]).1.contains("; paired"));
    let listed: serde_json::Value = serde_json::from_str(&run(&["list", "--json"]).1).unwrap();
    let id = listed.as_array().unwrap().iter().find(|r| r["name"] == "Front Room").unwrap()["device_id"].as_str().unwrap().to_owned();
    assert_eq!(run(&["paired"]).1, format!("Front Room  ({id})\n"));

    // Played as a menu asks: by its id, apart from the film.
    let (ok, said) = run(&["play", "--on", &id, film.to_str().unwrap()]);
    assert!(ok && said.ends_with("Playing on Front Room (it is 10:00 long).\nIt played to its end.\n"), "{said}");
    assert!(tv.lock().unwrap().fetched == bytes);
    // One that isn't there, and one not paired with: said by its name.
    let (ok, said) = run(&["play", "--on", "Den", film.to_str().unwrap()]);
    assert!(!ok && said.contains("Den isn't paired with"), "{said}");
    let (ok, said) = run(&["play", "--on", "02:00:00:00:00:99", film.to_str().unwrap()]);
    assert!(!ok && said.contains("didn't answer"), "{said}");
    assert!(!run(&["play", "--on", &id]).0);
    let _ = std::fs::remove_dir_all(&state);
}
