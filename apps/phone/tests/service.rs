//! `olo-phoned` (the phone service) end to end: against `phone-standin` (olo-system's
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
    // (The person's "Check for Codes".)
    if plan["now"] == true {
        say(json!({"say": "waiting", "id": null, "tabs": plan["tabs"], "now": true}));
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

/// A stand-in browser whose person chooses "Check for Codes" at once.
fn browser_checking(socket: &Path, tabs: Value, ms: u64) -> Child {
    let plan = json!({"socket": socket, "tabs": tabs, "ms": ms, "unasked": false, "now": true});
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "a_browser", "--nocapture"]).env("OLO_PHONE_TEST_BROWSER", plan.to_string()).env_remove("OLO_SCREEN").stdout(Stdio::piped());
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

/// Codes sent by mail (Ben, 2026-11-17): a page's code field comes up,
/// and the mail account set up (a stand-in IMAP server here) is looked
/// in: what came in the last minutes, then each message as it comes, the
/// server telling (IDLE). Nothing older is read; with nothing come, the
/// watch ends after its minute (a thirtieth of it here) and signs out; a
/// code in a message that comes goes to the tab as a texted one does; an
/// account left out (Security ▸ Passwords) isn't looked in.
#[test]
fn a_code_from_mail_to_the_tab_waiting_for_it() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    use olo_mail::standin;
    // (On disk: /tmp is memory here.)
    let dir = Path::new("/var/tmp").join(format!("olo-phone-mail-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let (run, socket) = (dir.join("run"), dir.join("run/olo-phone.sock"));
    let old = "From: Acme <no-reply@acme.example>\r\nSubject: Your Acme code\r\n\r\nYour Acme verification code is 111111.\r\n";
    let server = standin::Server::start(vec![(standin::ago(3600), old.into())]);
    std::fs::write(run.join("mail.json"), r#"[{"provider": "gmail", "address": "me@example.com"}]"#).unwrap();
    let _service = Ended(
        Command::new(env!("CARGO_BIN_EXE_olo-phoned"))
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            // (No BlueZ: the other tests' stand-in, on the same bus, isn't
            // this one's, and mail wants none.)
            .env("OLO_PHONE_BLUEZ", format!("unix:path={}", dir.join("no-bluez.sock").display()))
            .env("OLO_PHONE_DIR", &run)
            .env("OLO_PHONE_SETTINGS", run.join("phone.json"))
            .env("OLO_MAIL_ACCOUNTS", run.join("mail.json"))
            .env("OLO_MAIL_IMAP", server.address())
            .env("OLO_PHONE_PACE", "30")
            .env("XDG_STATE_HOME", dir.join("state"))
            .env_remove("OLO_SCREEN")
            // (Its log in the test's folder, to read when it fails.)
            .stderr(std::fs::File::create(dir.join("phoned.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_for("the service never listened", || socket.exists());
    let said = |what: &str| server.said().iter().filter(|c| c.as_str() == what).count();

    // A page waits; no mail comes. Signed in, the old message passed
    // over, waited with (IDLE); after its minute, signed out.
    let page = |tab: u32, digits: Option<u32>| json!([{"tab": tab, "host": "github.com", "title": "Device verification", "focused": true, "active": true, "front": true, "by_mail": true, "age": 2, "digits": digits}]);
    let first = browser(&socket, None, page(20, None), 300, true);
    wait_for("the mail was never looked in", || said("IDLE") == 1);
    let idle_at = Instant::now();
    assert_eq!(heard(first), [] as [Value; 0]);
    wait_for("the watch never ended", || said("LOGOUT") == 1);
    let lasted = idle_at.elapsed();
    assert!(lasted >= Duration::from_millis(1500) && lasted < Duration::from_secs(4), "a minute, paced: {lasted:?}");
    assert!(!server.said().iter().any(|c| c.starts_with("UID FETCH")), "nothing from before the last minutes was read: {:?}", server.said());
    assert_eq!(server.logins(), ["me@example.com standin"]);

    // Another waits, and the code's message comes: offered in its tab.
    let mine = browser(&socket, None, page(21, None), 2500, true);
    wait_for("the mail was never looked in again", || said("IDLE") == 2);
    server.deliver("From: GitHub <noreply@github.com>\r\nSubject: [GitHub] Please verify your device\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hey there!</p><p>Verification code: <b>482913</b></p>\r\n");
    assert_eq!(
        heard(mine),
        [json!({"say": "any", "id": 1}), json!({"say": "code", "id": 1, "tab": 21, "frame": 0, "code": "482913", "from": "GitHub", "fill": false, "mail": true, "account": "me@example.com"})]
    );
    assert_eq!(said("UID FETCH 2 (UID BODY.PEEK[]<0.65536>)"), 1);

    // A page that waits for what a link gives (as Claude's sign-in: "click
    // the link sent to …, or enter the code it shows"): the message that
    // comes has no code, only a button; its address is offered to that tab.
    // (Its field takes eight characters: GitHub's six digits, still good
    // and never taken above, aren't for it.)
    let waits = json!([{"tab": 23, "host": "acme.example", "title": "Sign in - Acme", "focused": true, "active": true, "front": true, "by_mail": true, "age": 2, "digits": 8}]);
    let theirs = browser(&socket, None, waits, 2500, true);
    wait_for("the mail was never looked in for the link", || said("IDLE") == 3);
    server.deliver("From: Acme <login@mail.acme.example>\r\nSubject: Your secure link to Acme is here\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSign in to Acme\r\nClick the button below to finish signing in. This link expires in 10 minutes.\r\nSign in\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>Click the button below to finish signing in.</p><a href=\"https://acme.example/magic-link#t0k3n\">Sign in</a> <a href=\"https://acme.example/help\">Help</a>\r\n--b--\r\n");
    assert_eq!(
        heard(theirs),
        [
            json!({"say": "any", "id": 2}),
            json!({"say": "link", "id": 2, "tab": 23, "frame": 0, "url": "https://acme.example/magic-link#t0k3n", "to": "acme.example", "from": "Acme", "account": "me@example.com"})
        ]
    );

    // A page that says its code is an authenticator app's has the mail
    // left alone; "Check for Codes" there looks in it all the same, at
    // once (signed in anew: what came in the last minutes read first).
    // (The link given, its watch was shut at once.)
    std::thread::sleep(Duration::from_millis(300));
    let app = json!([{"tab": 24, "host": "vault.example", "title": "Two-step", "focused": true, "active": true, "front": true, "by_app": true, "age": 2, "digits": 9}]);
    let before = server.logins().len();
    assert_eq!(heard(browser(&socket, None, app.clone(), 500, true)), [] as [Value; 0]);
    assert_eq!(server.logins().len(), before, "an authenticator's page had the mail looked in");
    let checking = browser_checking(&socket, app, 1200);
    wait_for("Check for Codes never looked in the mail", || server.logins().len() > before);
    assert_eq!(heard(checking), [] as [Value; 0]);

    // Left out: a page waiting (for four digits: not the code had) has
    // the mail looked in no more.
    std::fs::write(run.join("phone.json"), r#"{"unwatched_mail": ["me@example.com"]}"#).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let logins = server.logins().len();
    let later = browser(&socket, None, page(22, Some(4)), 800, true);
    assert_eq!(heard(later), [] as [Value; 0]);
    assert_eq!(server.logins().len(), logins);
    let _ = std::fs::remove_dir_all(&dir);
}

/// One stand-in BlueZ at a time on the test's bus.
static BUS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A phone that misbehaves as Ben's iPhone did (2026-11-17), and the
/// service with it: asleep (the channel taken, nothing said), its
/// characteristics shown over the classic link (a subscription closed
/// at once), a link dropped as a message is sent, a phone that doesn't
/// tell of its messages. The service's waits are a thirtieth of
/// themselves here (`OLO_PHONE_PACE`).
#[test]
fn a_flaky_phone() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    let _one = BUS.lock().unwrap_or_else(|e| e.into_inner());
    let phone = PathBuf::from(env!("CARGO_BIN_EXE_olo-phone"));
    let standin = phone.with_file_name("phone-standin");
    if !standin.exists() {
        return;
    }
    let dir = Path::new("/var/tmp").join(format!("olo-phone-flaky-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let (run, state) = (dir.join("run"), dir.join("state"));
    let tell = |words: &[&str]| {
        let out = Command::new(&standin).args(["--address", &address, "--tell"]).args(words).output().unwrap();
        assert!(out.status.success(), "{words:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let count = |what: &str| tell(&[what]).parse::<u32>().unwrap();
    let _bluez = Ended(Command::new(&standin).args(["--address", &address]).spawn().unwrap());
    wait_for("the stand-in never took its name", || {
        zbus::blocking::connection::Builder::address(address.as_str())
            .and_then(|b| b.build())
            .and_then(|c| zbus::blocking::fdo::DBusProxy::new(&c).and_then(|d| Ok(d.name_has_owner("org.bluez".try_into().unwrap())?)))
            .unwrap_or(false)
    });
    tell(&["set", "messages", "no"]);
    let _service = Ended(
        Command::new(env!("CARGO_BIN_EXE_olo-phoned"))
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("OLO_PHONE_BLUEZ", &address)
            .env("OLO_PHONE_DIR", &run)
            .env("OLO_PHONE_SETTINGS", run.join("phone.json"))
            .env("OLO_PHONE_PACE", "30")
            .env("XDG_STATE_HOME", &state)
            .env_remove("OLO_SCREEN")
            // (Its log in the test's folder, to read when it fails.)
            .stderr(std::fs::File::create(dir.join("phoned.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let published = || std::fs::read_to_string(run.join("olo-phone.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    let olo_phone = |args: &[&str]| {
        let out = Command::new(&phone).args(args).env("OLO_PHONE_DIR", &run).env_remove("OLO_SCREEN").stderr(Stdio::piped()).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let ready_within = |secs: u64, what: &str| {
        let start = Instant::now();
        while published()["link"] != "ready" {
            assert!(start.elapsed() < Duration::from_secs(secs), "{what}: {}", published());
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    wait_for("the service never said the phone must allow it", || published()["link"] == "allow");
    wait_for("the service never advertised for the iPhone's switches", || tell(&["soliciting"]) == "yes");
    tell(&["set", "messages", "yes"]);
    ready_within(25, "the service never connected");

    // Asleep a while: the channel is taken and nothing said on it. The
    // service waits (it doesn't give the channel up and ask again), and
    // has its messages the moment the phone wakes and answers.
    tell(&["set", "asleep", "yes"]);
    tell(&["hang-up"]);
    wait_for("the state never said the messages had gone", || published()["link"] != "ready");
    std::thread::sleep(Duration::from_secs(4));
    let asked = count("asked");
    assert_ne!(published()["link"], "ready");
    tell(&["set", "asleep", "no"]);
    ready_within(3, "a phone that woke and answered wasn't heard");
    assert_eq!(count("asked"), asked, "the channel it was waited on, not another");

    // Asleep for long: given up on after its first wait (6 s here), and
    // asked again further and further apart (1 s, 4 s, 10 s after each
    // silence here; a minute a round all night, before). Said plainly
    // meanwhile; and there again once it wakes.
    tell(&["set", "asleep", "yes"]);
    tell(&["hang-up"]);
    wait_for("the state never said the messages had gone", || published()["link"] != "ready");
    let asked = count("asked");
    std::thread::sleep(Duration::from_secs(16));
    let more = count("asked") - asked;
    assert!((1..=3).contains(&more), "asked for {more} times in 16 s of a phone asleep");
    assert!(published()["trouble"].as_str().is_some_and(|t| t.contains("isn’t answering")), "{}", published());
    tell(&["set", "asleep", "no"]);
    ready_within(20, "it never came back to a phone that woke");
    assert_eq!(published()["trouble"], "");

    // A message sent as the link drops: held for the link, which is
    // asked for at once, and sent; not "the phone isn't connected".
    tell(&["hang-up"]);
    wait_for("the state never said the messages had gone", || published()["link"] != "ready");
    assert_eq!(olo_phone(&["--send", "+15550100117", "held a moment"]).0, true);
    assert!(tell(&["sent"]).contains("held a moment"));
    // Out of reach for good, it fails, said so, after its wait.
    tell(&["set", "connected", "no"]);
    wait_for("the state never said the phone had gone", || published()["link"] == "away");
    let start = Instant::now();
    assert_eq!(olo_phone(&["--send", "+15550100117", "nobody there"]).0, false);
    assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
    tell(&["set", "connected", "yes"]);
    ready_within(25, "the service never came back");

    // Out of reach, and BlueZ stuck: its hands-free side's connecting
    // failed and never said so (BlueZ 5.87's), so every asking after is
    // refused "br-connection-busy" (Ben's phone, 2026-11-17: away for good,
    // though back in the room). Back in reach, not connected until it's
    // paged: BlueZ, busy with it three times running, is told to let go
    // (`Disconnect`, the phone not connected), and the asking after pages
    // it. Said in the log.
    tell(&["set", "connected", "no"]);
    wait_for("the state never said the phone had gone", || published()["link"] == "away");
    tell(&["set", "stuck", "yes"]);
    tell(&["set", "in-reach", "yes"]);
    ready_within(30, "a phone in reach, BlueZ stuck busy with it, was never connected again");
    assert_eq!(count("unstuck"), 1);
    let log = std::fs::read_to_string(dir.join("phoned.log")).unwrap_or_default();
    assert!(log.contains("told to let go of what it's stuck on"), "the log never said so");
    tell(&["set", "in-reach", "no"]);

    // Connected, and BlueZ wedged with `org.bluez.Error.Failed: br-connection-busy`
    // (Ben's iPhone, 2026-11-25: classic ACL/profile stuck while Device1.Connected
    // remained true). Self-heals via Disconnect + settle + Connect automatically.
    tell(&["set", "stuck-failed", "yes"]);
    tell(&["hang-up"]);
    wait_for("the state never left ready after hang-up", || published()["link"] != "ready");
    ready_within(30, "a connected phone wedged on Failed: br-connection-busy never self-healed");
    assert_eq!(count("unstuck"), 2);

    // Persistent `br-connection-busy` requiring two Disconnect + Connect cycles
    // with backoff before BlueZ clears: retries automatically instead of giving up
    // after one attempt.
    tell(&["set", "stuck-cycles", "2"]);
    tell(&["hang-up"]);
    wait_for("the state never left ready for two-cycle recovery", || published()["link"] != "ready");
    ready_within(35, "a phone requiring two br-connection-busy recovery cycles never self-healed");
    assert_eq!(count("unstuck"), 4);

    // A phone that doesn't tell of the messages that come: its inbox is
    // looked in on (every 2 s here), and the message found and told of.
    tell(&["set", "quiet", "yes"]);
    let unread = published()["unread"].as_u64().unwrap();
    tell(&["receive", "", "+15550100190", "Nobody told you of this one."]);
    wait_for("a message the phone didn't tell of was never found", || published()["unread"].as_u64() == Some(unread + 1));
    assert!(olo_phone(&["--read", "+15550100190"]).1.contains("Nobody told you of this one."));
    tell(&["set", "quiet", "no"]);

    // Its notification service shows over the classic link, the
    // low-energy one down (as BlueZ showed Ben's iPhone's): nothing is
    // subscribed to there. (It was, the socket closed at once, and it
    // was subscribed to again: 4667 times in twelve minutes.)
    tell(&["set", "classic-gatt", "yes"]);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!((count("acquired"), published()["others"].as_bool()), (0, Some(false)));
    // Over low energy it's taken; and a link that goes after holding is
    // asked for again, not subscribed to over the classic one.
    tell(&["set", "notifications", "yes"]);
    wait_for("its other notifications never came", || published()["others"] == true);
    assert_eq!(count("acquired"), 2);
    // The low-energy link drops (out of its reach a moment): the phone
    // is asked to connect that way again, and they're back.
    std::thread::sleep(Duration::from_millis(1200));
    tell(&["set", "low-energy", "no"]);
    wait_for("its other notifications never came back from a link dropped", || count("acquired") == 4 && published()["others"] == true);
    // Taken back on the phone: gone, and not subscribed to over the
    // classic link, where they still show.
    tell(&["set", "notifications", "no"]);
    wait_for("the state never said its other notifications had gone", || published()["others"] == false);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(count("acquired"), 4);
    // Subscriptions that close at once over low energy too (a phone
    // that won't tell; a BlueZ that doesn't say which link is up):
    // tried again further and further apart (0.2, 1, 4, 20 s here), not
    // at once for ever.
    tell(&["set", "deaf", "yes"]);
    tell(&["set", "notifications", "yes"]);
    let start = Instant::now();
    while count("acquired") == 4 {
        assert!(start.elapsed() < Duration::from_secs(20), "the phone was never asked to connect over low energy again");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_secs(4));
    let tried = (count("acquired") - 4) / 2;
    assert!((2..=4).contains(&tried), "its notifications were subscribed to {tried} times in 4 s of closing at once");
    assert_eq!(published()["others"], false);
    tell(&["set", "deaf", "no"]);
    let start = Instant::now();
    while published()["others"] != true {
        assert!(start.elapsed() < Duration::from_secs(30), "its other notifications never came back: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }
    // And all the while the service slept between: nothing spins.
    drop(_service);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn messages_and_a_code_to_the_tab_waiting_for_it() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    let _one = BUS.lock().unwrap_or_else(|e| e.into_inner());
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
        Command::new(env!("CARGO_BIN_EXE_olo-phoned"))
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("OLO_PHONE_BLUEZ", &address)
            .env("OLO_PHONE_DIR", &run)
            .env("OLO_PHONE_SETTINGS", run.join("phone.json"))
            .env("XDG_STATE_HOME", &state)
            .env_remove("OLO_SCREEN")
            // (Its log in the test's folder, to read when it fails.)
            .stderr(std::fs::File::create(dir.join("phoned.log")).unwrap())
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
    // (She is in the phone's book twice, a card from each account: one
    // person all the same.)
    assert!(ok && said.contains("Mira Kovač") && said.contains("Are you still coming on Saturday?"), "a contact's name on her conversation: {said}");
    // A long message from before: the listing has only its first 128
    // bytes; it's asked for whole, and shows whole.
    let long = olo_system::phone::standin::Phone::LONG;
    wait_for("the long message never came whole", || olo_phone(&["--read", "Mira Kovač"], None).1.contains("Tell me by Thursday?"));
    let (_, read, _) = olo_phone(&["--read", "Mira Kovač", "--json"], None);
    let read: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(read["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(read["messages"][0]["text"].as_str(), Some(long));
    assert_eq!(read["messages"][0]["read"], true, "asking for it whole doesn't change whether it was read");

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
    assert_eq!(agents, [json!({"say": "any", "id": 1}), json!({"say": "code", "id": 1, "tab": 7, "frame": 3, "code": "482913", "from": "Acme Bank", "fill": true, "mail": false, "account": ""})]);
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
    assert_eq!(
        heard(mine),
        [json!({"say": "any", "id": 2}), json!({"say": "code", "id": 2, "tab": 11, "frame": 0, "code": "771204", "from": "Acme Bank", "fill": false, "mail": false, "account": ""})]
    );
    let later = browser(&socket, None, json!([{"tab": 12, "host": "acmebank.example", "title": "Verify", "focused": true, "active": true, "front": true}]), 1200, true);
    assert_eq!(heard(later), [json!({"say": "code", "id": 2, "tab": 12, "frame": 0, "code": "771204", "from": "Acme Bank", "fill": false, "mail": false, "account": ""})]);
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
    // (A message sent meanwhile waits for it, 25 s, before it fails:
    // `a_flaky_phone`.)
    tell(&["set", "connected", "no"]);
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
    // Its contacts came, and its other notifications aren't allowed
    // yet: the service advertises on, for those (an iPhone connects over
    // low energy to a computer advertising for them). Allowed on the
    // phone, they come, and nothing is advertised any more. (This step
    // waited for "no" with them still to come, from before there were
    // any: it failed since they were added, unseen, as the test runs
    // only with OLO_TEST_BUS. Found 2026-11-17.)
    assert_eq!((tell(&["soliciting"]).trim(), published()["others"].as_bool()), ("yes", Some(false)));
    tell(&["set", "notifications", "yes"]);
    wait_for("its other notifications never came", || published()["others"] == true);
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

/// Replies sent directly on the phone sync into Olo's conversations (both live
/// via MAP Notification Server events in `TELECOM/MSG/SENT` and on connect/sync
/// from the MAP `sent` folder), appear as outgoing & read without firing unread
/// badges or OTP detection, fetch long bodies whole, and never duplicate replies
/// sent from Olo itself.
#[test]
fn outgoing_replies_sent_from_the_phone() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    let _one = BUS.lock().unwrap_or_else(|e| e.into_inner());
    let phone = PathBuf::from(env!("CARGO_BIN_EXE_olo-phone"));
    let standin = phone.with_file_name("phone-standin");
    if !standin.exists() {
        return;
    }
    let dir = Path::new("/var/tmp").join(format!("olo-phone-sent-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let (run, state, socket) = (dir.join("run"), dir.join("state"), dir.join("run/olo-phone.sock"));
    let tell = |words: &[&str]| {
        let out = Command::new(&standin).args(["--address", &address, "--tell"]).args(words).output().unwrap();
        assert!(out.status.success(), "{words:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let _bluez = Ended(Command::new(&standin).args(["--address", &address, "--android"]).spawn().unwrap());
    wait_for("the android stand-in never took its name", || {
        zbus::blocking::connection::Builder::address(address.as_str())
            .and_then(|b| b.build())
            .and_then(|c| zbus::blocking::fdo::DBusProxy::new(&c).and_then(|d| Ok(d.name_has_owner("org.bluez".try_into().unwrap())?)))
            .unwrap_or(false)
    });
    let _service = Ended(
        Command::new(env!("CARGO_BIN_EXE_olo-phoned"))
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("OLO_PHONE_BLUEZ", &address)
            .env("OLO_PHONE_DIR", &run)
            .env("OLO_PHONE_SETTINGS", run.join("phone.json"))
            .env("OLO_PHONE_PACE", "30")
            .env("XDG_STATE_HOME", &state)
            .env_remove("OLO_SCREEN")
            .stderr(std::fs::File::create(dir.join("phoned.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let published = || std::fs::read_to_string(run.join("olo-phone.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    let olo_phone = |args: &[&str]| {
        let out = Command::new(&phone).args(args).env("OLO_PHONE_DIR", &run).env_remove("OLO_SCREEN").stderr(Stdio::piped()).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let start = Instant::now();
    while published()["link"] != "ready" || published()["contacts"] != true {
        assert!(start.elapsed() < Duration::from_secs(25), "android stand-in never connected: {}", published());
        std::thread::sleep(Duration::from_millis(50));
    }

    // 1. Live reply typed on the phone (including text that resembles an OTP
    //    code): delivered via MNS TELECOM/MSG/SENT, appears as outgoing & read,
    //    never increments unread, never fires browser OTP offers.
    let mine = browser(&socket, None, json!([{"tab": 11, "host": "login.acmebank.example", "title": "Sign in", "focused": true, "active": true, "front": true, "age": 10}]), 1500, false);
    std::thread::sleep(Duration::from_millis(400));
    tell(&["reply", "Mira Kovač", "+15550100142", "On my way! My confirmation code is 654321."]);
    wait_for("live reply from phone never appeared in conversation", || olo_phone(&["--read", "Mira Kovač"]).1.contains("you: On my way! My confirmation code is 654321."));
    assert_eq!(heard(mine), [] as [Value; 0], "outgoing reply must never trigger OTP extraction");
    assert_eq!(published()["unread"].as_u64(), Some(0), "outgoing reply must stay read");
    let (_, read_json) = olo_phone(&["--read", "Mira Kovač", "--json"]);
    let read_val: Value = serde_json::from_str(&read_json).unwrap();
    let last = read_val["messages"].as_array().unwrap().last().unwrap();
    assert_eq!((last["outgoing"].as_bool(), last["read"].as_bool()), (Some(true), Some(true)));

    // 2. Long reply sent on the phone while `quiet` (no MNS event): synced from
    //    `sent` on reconnect and fetched whole (beyond Android's 24-char subject).
    tell(&["set", "quiet", "yes"]);
    let long_reply = "Yes, definitely coming on Saturday — I will bring extra jackets, coffee, and the board games we talked about last weekend.";
    tell(&["reply", "Mira Kovač", "+15550100142", long_reply]);
    tell(&["hang-up"]);
    wait_for("long sent reply never synced whole on reconnect", || olo_phone(&["--read", "Mira Kovač"]).1.contains(long_reply));
    assert_eq!(published()["unread"].as_u64(), Some(0));

    // 3. Reply sent from Olo on Android (where Put outbox also adds to `sent`):
    //    must appear ONCE even after a reconnect/re-sync of `sent`.
    assert_eq!(olo_phone(&["--send", "Mira Kovač", "Sent from Olo desktop"]).0, true);
    tell(&["hang-up"]);
    let start = Instant::now();
    while published()["link"] != "ready" {
        assert!(start.elapsed() < Duration::from_secs(20), "never reconnected after hang-up");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Wait briefly for `sent` listing to settle.
    std::thread::sleep(Duration::from_millis(400));
    let (_, read_json) = olo_phone(&["--read", "Mira Kovač", "--json"]);
    let read_val: Value = serde_json::from_str(&read_json).unwrap();
    let matches = read_val["messages"].as_array().unwrap().iter().filter(|m| m["text"].as_str() == Some("Sent from Olo desktop")).count();
    assert_eq!(matches, 1, "message sent from Olo must not be duplicated by sent sync: {read_json}");

    drop(_service);
    drop(_bluez);
    let _ = std::fs::remove_dir_all(&dir);
}
