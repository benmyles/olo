//! `olo-phone --service` end to end: against `phone-standin` (olo-system's
//! stand-in BlueZ with a stand-in phone behind it, found beside olo-phone
//! in the build's folder) and stand-in browsers that say which tabs wait
//! for a code. Needs a bus to meet on: run with `OLO_TEST_BUS=unix:path=…`
//! (a private bus: `olo-bus unix:path=/var/tmp/…/bus.sock`) after `cargo
//! build -p olo-system --bin phone-standin`. Without them the test does
//! nothing.
//!
//! A browser on an agent's screen is known by its process's environment
//! (`OLO_SCREEN`), so the stand-in browsers are processes of their own:
//! this test program again, running only [`a_browser`].

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

struct Ended(Child);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A stand-in browser, when this program is started as one
/// (`OLO_PHONE_TEST_BROWSER`: the socket, its tabs as JSON, how long to
/// stay, and "unasked" to say its tabs wait without being asked). It
/// prints each thing the service says, a line each, and says a code put
/// in at once was put in.
#[test]
fn a_browser() {
    let Ok(plan) = std::env::var("OLO_PHONE_TEST_BROWSER") else { return };
    let plan: Value = serde_json::from_str(&plan).unwrap();
    let stream = UnixStream::connect(plan["socket"].as_str().unwrap()).unwrap();
    let say = |v: Value| (&stream).write_all(format!("{v}\n").as_bytes()).unwrap();
    say(json!({"do": "browser", "screen": null, "pid": 0}));
    if plan["unasked"] == true {
        say(json!({"say": "waiting", "id": null, "tabs": plan["tabs"]}));
    }
    let until = Instant::now() + Duration::from_millis(plan["ms"].as_u64().unwrap());
    let mut lines = BufReader::new(&stream);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        stream.set_read_timeout(Some(left)).unwrap();
        let mut line = String::new();
        match lines.read_line(&mut line) {
            Ok(n) if n > 0 => {}
            _ => return,
        }
        let heard: Value = serde_json::from_str(&line).unwrap();
        println!("heard {heard}");
        match heard["say"].as_str() {
            Some("any") => say(json!({"say": "waiting", "id": heard["id"], "tabs": plan["tabs"]})),
            Some("code") if heard["fill"] == true => say(json!({"say": "outcome", "id": heard["id"], "outcome": "filled"})),
            _ => {}
        }
    }
}

/// Start a stand-in browser (on agent screen `screen`, if given).
fn browser(socket: &Path, screen: Option<&str>, tabs: Value, ms: u64, unasked: bool) -> Child {
    let plan = json!({"socket": socket, "tabs": tabs, "ms": ms, "unasked": unasked});
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "a_browser", "--nocapture"]).env("OLO_PHONE_TEST_BROWSER", plan.to_string()).env_remove("OLO_SCREEN").stdout(Stdio::piped());
    if let Some(s) = screen {
        c.env("OLO_SCREEN", s);
    }
    c.spawn().unwrap()
}

/// What a stand-in browser heard, once it has ended.
fn heard(c: Child) -> Vec<Value> {
    let out = c.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.strip_prefix("heard ")).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn messages_and_a_code_to_the_tab_waiting_for_it() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    let phone = PathBuf::from(env!("CARGO_BIN_EXE_olo-phone"));
    let standin = phone.with_file_name("phone-standin");
    if !standin.exists() {
        eprintln!("no {}: build it (cargo build -p olo-system --bin phone-standin)", standin.display());
        return;
    }
    // (On disk: /tmp is memory here.)
    let dir = Path::new("/var/tmp").join(format!("olo-phone-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let (run, state, socket) = (dir.join("run"), dir.join("state"), dir.join("run/olo-phone.sock"));
    let tell = |words: &[&str]| {
        let out = Command::new(&standin).args(["--address", &address, "--tell"]).args(words).output().unwrap();
        assert!(out.status.success(), "{words:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let there = || {
        wait_for("the stand-in never took its name", || {
            zbus::blocking::connection::Builder::address(address.as_str())
                .and_then(|b| b.build())
                .and_then(|c| zbus::blocking::fdo::DBusProxy::new(&c).and_then(|d| Ok(d.name_has_owner("org.bluez".try_into().unwrap())?)))
                .unwrap_or(false)
        })
    };
    #[allow(unused_assignments)]
    let mut bluez = Some(Ended(Command::new(&standin).args(["--address", &address]).spawn().unwrap()));
    there();
    // The phone's own switch is off as the service starts.
    tell(&["set", "messages", "no"]);
    let _service = Ended(
        Command::new(&phone)
            .arg("--service")
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("OLO_PHONE_BLUEZ", &address)
            .env("OLO_PHONE_DIR", &run)
            .env("OLO_PHONE_SETTINGS", run.join("phone.json"))
            .env("XDG_STATE_HOME", &state)
            .env_remove("OLO_SCREEN")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let published = || std::fs::read_to_string(run.join("olo-phone.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    let olo_phone = |args: &[&str], screen: Option<&str>| {
        let mut c = Command::new(&phone);
        c.args(args).env("OLO_PHONE_DIR", &run).env_remove("OLO_SCREEN").stderr(Stdio::piped());
        if let Some(s) = screen {
            c.env("OLO_SCREEN", s);
        }
        let out = c.output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_owned(), String::from_utf8_lossy(&out.stderr).trim().to_owned())
    };

    // Paired, in reach, and not allowed yet: said so. Allowed on the
    // phone: found within its ten seconds, the inbox read, the contacts.
    wait_for("the service never said the phone must allow it", || published()["link"] == "allow");
    assert_eq!((published()["name"].as_str(), published()["kind"].as_str()), (Some("Mira’s iPhone"), Some("i-phone")));
    assert_eq!(published()["profiles"], true);
    // An iPhone has the switch to turn on only while the computer
    // advertises for it (the stand-in refuses otherwise): the service
    // does, while it waits.
    wait_for("the service never advertised for the iPhone's switches", || tell(&["soliciting"]).trim() == "yes");
    tell(&["set", "messages", "yes"]);
    let start = Instant::now();
    while published()["link"] != "ready" || published()["contacts"] != true {
        assert!(start.elapsed() < Duration::from_secs(25), "the service never connected: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = published();
    assert_eq!((s["conversations"].as_u64(), s["unread"].as_u64(), s["codes"].as_u64()), (Some(2), Some(0), Some(0)));
    let (ok, said, _) = olo_phone(&["--conversations"], None);
    assert!(ok && said.contains("Mira Kovač") && said.contains("Are you still coming on Saturday?"), "a contact's name on her conversation: {said}");

    // The person's tab has the focus, on some forum; the tab on the
    // agent's screen is the bank's, its field just come up. The bank's
    // code is the agent's tab's, put in at once; the person's browser
    // hears only that a code came, then that it went.
    let mine = browser(&socket, None, json!([{"tab": 11, "host": "forum.example", "title": "Two-step sign in", "focused": true, "active": true, "front": true, "age": 4000}]), 2500, false);
    let agents =
        browser(&socket, Some("agent"), json!([{"tab": 7, "frame": 3, "host": "login.acmebank.example", "title": "Sign in", "declared": true, "digits": 6, "by_text": true, "age": 20}]), 2500, false);
    std::thread::sleep(Duration::from_millis(500));
    tell(&["receive", "", "22395", "Your Acme Bank verification code is 482913. It expires in 10 minutes."]);
    let (mine, agents) = (heard(mine), heard(agents));
    assert_eq!(agents, [json!({"say": "any", "id": 1}), json!({"say": "code", "id": 1, "tab": 7, "frame": 3, "code": "482913", "from": "Acme Bank", "fill": true})]);
    assert_eq!(mine, [json!({"say": "any", "id": 1}), json!({"say": "gone", "id": 1})]);
    // It's in the agent screen's history, without the code; and the
    // state says a code came, and nothing of it.
    let history = std::fs::read_to_string(state.join("olo/screens/agent/actions.jsonl")).unwrap();
    let action: Value = serde_json::from_str(history.trim()).unwrap();
    assert_eq!((action["kind"].as_str(), action["args"][0].as_str(), action["result"]["from"].as_str()), (Some("code"), Some("login.acmebank.example"), Some("Acme Bank")));
    assert!(!history.contains("482913"));
    let s = published();
    assert_eq!((s["codes"].as_u64(), s["code_at"].is_null(), s["unread"].as_u64(), s["conversations"].as_u64()), (Some(1), true, Some(1), Some(3)));
    assert!(!s.to_string().contains("482913") && !s.to_string().contains("Acme"));

    // The person's tab alone: offered there, for them to take; and a
    // code field that comes up after the code did gets it too.
    let mine = browser(&socket, None, json!([{"tab": 11, "host": "login.acmebank.example", "title": "Sign in", "focused": true, "active": true, "front": true, "age": 15}]), 2000, false);
    std::thread::sleep(Duration::from_millis(500));
    tell(&["receive", "", "22395", "Your Acme Bank verification code is 771204."]);
    assert_eq!(heard(mine), [json!({"say": "any", "id": 2}), json!({"say": "code", "id": 2, "tab": 11, "frame": 0, "code": "771204", "from": "Acme Bank", "fill": false})]);
    let later = browser(&socket, None, json!([{"tab": 12, "host": "acmebank.example", "title": "Verify", "focused": true, "active": true, "front": true}]), 1200, true);
    assert_eq!(heard(later), [json!({"say": "code", "id": 2, "tab": 12, "frame": 0, "code": "771204", "from": "Acme Bank", "fill": false})]);
    // Nothing that isn't a code is offered anywhere.
    let mine = browser(&socket, None, json!([{"tab": 11, "host": "shop.example", "title": "Checkout", "focused": true, "active": true, "front": true, "age": 5}]), 1500, false);
    std::thread::sleep(Duration::from_millis(500));
    tell(&["receive", "", "+15550100190", "Your parcel arrives tomorrow between 10:00 and 12:00. Tracking 48291375."]);
    assert_eq!(heard(mine), [] as [Value; 0]);

    // What a message says is the person's: the latest code and the
    // conversations are given to what asks here, never on an agent's
    // screen.
    assert_eq!(olo_phone(&["--code"], None), (true, "771204".into(), String::new()));
    assert_eq!(olo_phone(&["--code"], Some("agent")), (false, String::new(), "olo-phone: not from an agent's screen".into()));
    assert_eq!(olo_phone(&["--conversations"], Some("agent")).2, "olo-phone: not from an agent's screen");

    // An answer, by the conversation's name; a new message to a number;
    // what isn't one is refused plainly. The phone has what was sent,
    // and the conversation keeps it.
    assert_eq!(olo_phone(&["--send", "Mira Kovač", "See", "you", "at", "ten."], None), (true, String::new(), String::new()));
    // (Conversations are ordered by the second.)
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(olo_phone(&["--send", "+15550100117", "Thanks Tomas"], None).0, true);
    assert_eq!(olo_phone(&["--send", "nobody at all", "x"], None), (false, String::new(), "olo-phone: That isn't a telephone number or an address.".into()));
    assert_eq!(tell(&["sent"]), "+15550100142: See you at ten.\n+15550100117: Thanks Tomas\n");
    let (_, read, _) = olo_phone(&["--read", "Mira Kovač"], None);
    assert!(read.ends_with("you: See you at ten."), "{read}");
    let (_, all, _) = olo_phone(&["--conversations"], None);
    assert!(all.starts_with("Tomas Lind"), "the latest first, a contact's name on a new one: {all}");
    assert_eq!(olo_phone(&["--mark-read", "22395"], None).0, true);
    assert_eq!(published()["unread"].as_u64(), Some(1), "the parcel's is still unread");
    assert_eq!(olo_phone(&["--contacts", "tomas"], None).1, "Tomas Lind  +15550100117, tomas@example.org");

    // The phone goes out of reach: said; back: connected again by itself.
    tell(&["set", "connected", "no"]);
    assert_eq!(olo_phone(&["--send", "Mira Kovač", "still there?"], None).0, false);
    wait_for("the state never said the phone had gone", || published()["link"] == "away");
    tell(&["set", "connected", "yes"]);
    let start = Instant::now();
    while published()["link"] != "ready" {
        assert!(start.elapsed() < Duration::from_secs(25), "the service never came back: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(published()["conversations"].as_u64(), Some(4), "what it had is kept");
    // The phone closes the channel to its messages and stays connected
    // (its owner turned the switch off and on): known of at once, not
    // when next something is asked, and asked for again. So a message
    // sent a few seconds on, the first thing asked since, goes.
    tell(&["hang-up"]);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(published()["link"], "ready", "it never came back from the phone hanging up: {}", published());
    assert_eq!(olo_phone(&["--send", "Mira Kovač", "back again"], None).0, true);
    // BlueZ takes the phone's messages for connected and, asked for
    // them, says yes and opens nothing (as it did with Ben's iPhone,
    // where the service then waited for good): given up after a few
    // seconds, BlueZ told to let go, and asked again.
    tell(&["set", "stale", "yes"]);
    tell(&["hang-up"]);
    wait_for("the state never said the messages had gone", || published()["link"] != "ready");
    let start = Instant::now();
    while published()["link"] != "ready" {
        assert!(start.elapsed() < Duration::from_secs(45), "the service waited for good on a channel that never came: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(olo_phone(&["--send", "Mira Kovač", "and again"], None).0, true);
    // Its contacts came: nothing is advertised any more.
    wait_for("the service went on advertising with nothing to wait for", || tell(&["soliciting"]).trim() == "no");
    // BlueZ is started again (an update; Olo Doctor changing how it's
    // started): it has forgotten Olo's profiles and says nothing of it.
    // The service registers them with the new one and has the phone's
    // messages open again, by itself.
    drop(bluez.take());
    wait_for("the state never said BlueZ had gone", || published()["profiles"] == false);
    bluez = Some(Ended(Command::new(&standin).args(["--address", &address]).spawn().unwrap()));
    there();
    let start = Instant::now();
    while published()["link"] != "ready" || published()["profiles"] != true {
        assert!(start.elapsed() < Duration::from_secs(25), "the service never came back to a BlueZ started again: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }
    let n = olo_phone(&["--conversations"], None).1.lines().count();
    assert!(n >= 4, "what it had is kept through it: {n} lines");
    // Unpaired: no phone, and nothing of it kept.
    tell(&["set", "paired", "no"]);
    wait_for("the state never said no phone is paired", || published()["link"] == "not-paired" && published()["conversations"] == 0);
    drop(_service);
    drop(bluez);
    let _ = std::fs::remove_dir_all(&dir);
}
