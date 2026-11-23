//! Against a real host, where one is given (never in an ordinary run:
//! every test here is ignored): `OLO_REX_TEST_HOST=ssh://USER@HOST`, with
//! an agent (`SSH_AUTH_SOCK`) whose keys let USER and root in, and a
//! `HOME` whose `.ssh/known_hosts` knows the host. It runs commands
//! there, asks how the host stands as the account and as root, and reads
//! the login server's accounts; it changes nothing on the host. What it
//! prints names neither the host nor the account.
//!
//!   HOME=SCRATCH OLO_REX_TEST_HOST=ssh://USER@HOST cargo test -p olo-rex-client --test host -- --ignored --nocapture

use std::sync::Arc;

use olo_rex_client::{
    SshAskpass, SshSessionPool, SshTarget,
    provision::{self, OverSsh, Runner as _},
};
use zeroize::Zeroizing;

/// Trusts no host it doesn't know and answers no question.
#[derive(Debug)]
struct Nobody;

impl SshAskpass for Nobody {
    fn confirm_host_key(&self, _: &str, _: u16, _: &str, _: &str, _: bool) -> bool {
        false
    }

    fn prompt_secret(&self, _: &str, _: &str, _: bool) -> Option<Zeroizing<String>> {
        None
    }
}

fn host() -> OverSsh {
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    let target = SshTarget::parse(&address).expect("an ssh:// address");
    assert!(target.user.as_deref().is_some_and(|u| u != "root"), "the address names an account that isn't root");
    OverSsh { pool: Arc::new(SshSessionPool::default()), target, root_key: None, askpass: Some(Arc::new(Nobody)) }
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(f)
}

/// (With the account's name out of it.)
fn shown(steps: &[provision::Step], user: &str) -> Vec<String> {
    steps.iter().map(|s| format!("{}: {}", if s.root { "root" } else { "user" }, s.shown().replace(user, "USER"))).collect()
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn a_command_runs_there_and_what_it_writes_comes_line_by_line() {
    let mut host = host();
    let mut lines = Vec::new();
    let ran = block(host.run(false, "printf 'one\\ntwo\\n'; echo three >&2; printf 'no end'; exit 3", &mut |l| lines.push(l.to_owned()))).unwrap();
    assert_eq!(ran.status, Some(3));
    assert!(!ran.ok());
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(sorted, ["no end", "one", "three", "two"], "{lines:?}");
    assert_eq!(ran.output.lines().count(), 4);
    // As root, with the agent's keys; and who it is there.
    let ran = block(host.run(true, "id -u", &mut |_| {})).unwrap();
    assert_eq!((ran.status, ran.output.trim()), (Some(0), "0"));
    let ran = block(host.run(false, "id -u", &mut |_| {})).unwrap();
    assert!(ran.ok() && ran.output.trim() != "0");
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_host_says_how_it_stands_and_what_it_needs() {
    let mut host = host();
    let user = host.target.user.clone().unwrap();
    let facts = block(provision::probe(&mut host, false)).unwrap();
    assert_eq!((facts.os.as_str(), facts.user.as_str()), ("Linux", user.as_str()));
    assert!(facts.uid >= 1000 && facts.rex.is_some());
    println!(
        "as the account: distro {}, rex {:?} ({:?}), packaged {}, a bare copy {}, systemd {}, mode {:?}, listed {:?}, its own server stays {}",
        facts.distro,
        facts.rex,
        facts.version,
        facts.packaged,
        facts.bare,
        facts.systemd,
        facts.mode(),
        facts.listed(),
        facts.own_server_stays()
    );
    for mode in provision::ServerMode::ALL {
        match provision::plan(mode, &facts) {
            Ok(steps) => println!("  for {}: {:?}{}", mode.name(), shown(&steps, &user), if provision::ends_sessions(&steps) { " (ends sessions)" } else { "" }),
            Err(why) => println!("  for {}: can't ({})", mode.name(), why.replace(&user, "USER")),
        }
    }
    // As root: who it is, and the accounts as root reads them.
    let root = block(provision::probe(&mut host, true)).unwrap();
    assert_eq!((root.uid, root.user.as_str()), (0, "root"));
    let accounts = block(provision::read_accounts(&mut host)).unwrap();
    println!("as root: {} accounts listed, {} defaults; the account is among them: {}", accounts.accounts().len(), accounts.defaults().len(), accounts.accounts().iter().any(|a| a.name == user));
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn root_refused_is_said_to_be_refused_and_no_question_is_asked() {
    // A key nobody has: the agent offers none, and no password is asked for.
    let mut host = OverSsh { root_key: Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()), ..host() };
    let failed = block(host.run(true, "id -u", &mut |_| {})).unwrap_err();
    assert!(failed.refused, "{}", failed.why.len());
    // The account is still let in beside it (another connection).
    assert!(block(host.run(false, "true", &mut |_| {})).unwrap().ok());
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_agents_keys_are_told_apart() {
    let status = block(olo_rex_client::inspect_ssh_agent(None));
    assert!(status.reachable && status.key_count > 0 && status.keys.len() == status.key_count);
    assert!(status.keys.iter().all(|k| k.fingerprint.starts_with("SHA256:") && !k.kind.is_empty()));
    let kinds: std::collections::BTreeSet<&str> = status.keys.iter().map(|k| k.kind.as_str()).collect();
    println!("the agent has {} keys, of kinds {kinds:?}; each has a fingerprint", status.key_count);
    // The one of them that lets root in is found by its fingerprint.
    let mut let_in = 0;
    for key in &status.keys {
        let mut host = OverSsh { root_key: Some(key.fingerprint.clone()), ..host() };
        if block(host.run(true, "true", &mut |_| {})).is_ok_and(|ran| ran.ok()) {
            let_in += 1;
        }
    }
    println!("{let_in} of them let root in there");
    assert!(let_in >= 1);
}

/// The host's server as root sees it ([`olo_rex_client::manage::look`]):
/// read, nothing changed; and its login server's log. (What it prints
/// names neither the host nor anybody on it: counts and states only.)
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_server_is_read_as_root_and_nothing_is_changed() {
    use olo_rex_client::manage::{self, Service};
    let mut host = host();
    let before = block(provision::probe(&mut host, true)).expect("root is let in with the agent's keys");
    let server = block(manage::look(&mut host, true)).unwrap();
    println!(
        "mode {:?}; login {:?}, how long {}; shared {:?}; {} people, {} in the rex group; installed {}, newer {}; accounts {}, {} not listed",
        server.mode(),
        server.state(Service::Login),
        if server.up(Service::Login).is_some() { "known" } else { "unknown" },
        server.state(Service::Shared),
        server.people.len(),
        server.members.len(),
        server.installed.is_some(),
        server.newer.is_some(),
        server.accounts.as_ref().map_or("unread".to_owned(), |a| a.accounts().len().to_string()),
        server.not_listed().len(),
    );
    assert!(server.facts.packaged && server.facts.systemd);
    assert!(server.mode().is_some(), "a server runs there for everybody");
    assert!(!server.people.is_empty(), "somebody has an account there");
    assert!(server.installed.is_some());
    // Every action says what it would run, and none was run.
    for action in [manage::Action::Restart(Service::Login), manage::Action::Stop(Service::Shared), manage::Action::Update] {
        assert!(!action.shown(&server).is_empty(), "{action:?}");
    }
    println!(
        "the repository was asked: {}; a newer version: {}; sessions through the login server: {:?}; failed: {}",
        server.checked,
        server.newer.is_some(),
        server.sessions_on(Service::Login),
        server.failed(Service::Login)
    );
    assert!(server.checked, "apt read Rex's own source again");
    assert!(server.sessions_on(Service::Login).is_some(), "the account's own server said how many sessions it has");
    let log = block(manage::journal(&mut host, Service::Login, manage::LogRange::SinceStart)).unwrap();
    assert!(log.first().is_some_and(|day| day.len() == 10 && day.as_bytes()[4] == b'-'), "the log begins with its day");
    assert!(log.iter().skip(1).all(|l| l.len() == 10 || l.as_bytes().get(2) == Some(&b':')), "each line begins with its time");
    println!("the login server's log: {} lines", log.len());
    let after = block(provision::probe(&mut host, true)).unwrap();
    assert_eq!((before.login, before.shared, before.own), (after.login, after.shared, after.own));
}

/// Typing over the link, timed: a measurement that prints, with nothing
/// of the host's in it. A session there runs `cat` (the terminal itself
/// echoes each letter as it comes), and letters are typed a tenth of a
/// second apart, as a quick typist's: for each, how long from its being
/// handed to the client until its echo is back. Twice over: one write
/// on its way at a time, what is typed meanwhile gathered into the next
/// (as Rex typed until 2026-11-23: a letter waits for the answer to the
/// write before it, then makes its own trip), and each sent as it is
/// typed ([`ControlClient::send_terminal_write`]).
///
///   HOME=SCRATCH OLO_REX_TEST_HOST=ssh://USER@HOST cargo test -p olo-rex-client --test host typing -- --ignored --nocapture
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn typing_over_the_link_is_timed() {
    use olo_rex_client::{ClientIdentity, ControlClient, DATA_TERMINAL_SUBPROTOCOL, DataTicketRequest, RexEndpoint, TerminalDataStream, TerminalStreamFrame};
    use std::time::{Duration, Instant};
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    let letters: usize = std::env::var("OLO_REX_TEST_LETTERS").ok().and_then(|n| n.parse().ok()).unwrap_or(40);
    let apart = Duration::from_millis(std::env::var("OLO_REX_TEST_APART_MS").ok().and_then(|n| n.parse().ok()).unwrap_or(100));
    block(async move {
        // (`OLO_REX_TEST_HELPER_FILE`: a build of Olo's helper for the
        // host, `olo-rex-link` built for its kind. It is put there as the
        // app puts it, under the name of a build called "test", and the
        // control connection goes through it. A measurement only: the
        // file is left there, and is the host's to keep or remove.)
        let pool = Arc::new(SshSessionPool::default());
        if let Ok(file) = std::env::var("OLO_REX_TEST_HELPER_FILE") {
            use olo_rex_client::helper::{self, Build, Stands};
            let bytes: &'static [u8] = Box::leak(std::fs::read(&file).expect("the helper's file").into_boxed_slice());
            let builds = [Build { system: "Linux", machines: &["x86_64", "amd64", "aarch64", "arm64"], bytes }];
            let target = SshTarget::parse(&address).expect("an ssh:// address");
            let stands = helper::ensure(&pool, &target, Some(Arc::new(Nobody)), "test", &builds).await.expect("the helper is put on the host");
            match stands {
                Stands::Already => println!("the helper was on the host already"),
                Stands::Installed { bytes, took } => println!("the helper was put on the host: {bytes} bytes in {} ms", took.as_millis()),
                Stands::NoBuild(_) => println!("the host is of a kind the helper isn't built for"),
            }
            pool.set_helper(Some("test"));
        }
        let endpoint = RexEndpoint::parse(&address, pool, Some(Arc::new(Nobody))).expect("an address");
        let (stream, host) = endpoint.connect_control_stream().await.expect("the host's Rex server is reached");
        let (ctrl, _events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.expect("the control connection");
        let in_order = ctrl.keeps_typing_in_order();
        println!(
            "{}",
            if in_order { "Olo's helper is on the host: it gives the server what is typed one at a time" } else { "no helper on the host: the server takes what reaches it together at once" }
        );
        let created = ctrl.session_create(Some("olo-typing-test"), Some(serde_json::json!({"shell": "none", "command": ["cat"], "exit": {"on_completion": false}}))).await.expect("a session");
        let sid = created.payload["session_id"].as_str().unwrap().to_owned();
        let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();
        let boot = ctrl
            .session_bootstrap(
                "all",
                Some(&sid),
                None,
                &[DataTicketRequest { creator_name: "com.superlogical.terminal".into(), data_ref_id: "terminal".into(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.into()) }],
            )
            .await
            .expect("the session's terminal");
        let ticket = boot.payload.data_tickets.as_ref().and_then(|t| t.first()).map(|t| t.ticket.clone()).expect("a ticket for its bytes");
        let (dstream, dhost) = endpoint.connect_stream().await.expect("a second stream");
        let mut term = TerminalDataStream::connect_hello(dstream, &dhost, &ticket).await.expect("its bytes");
        // What it shows to begin with (its snapshot) is let go by; then quiet.
        while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(1500), term.next_frame()).await {}

        // With `OLO_REX_TEST_BUSY`: another terminal there prints all the
        // while, and its bytes are read here (a build scrolling in the
        // next pane): the connection carries more than what is typed.
        let busy = if std::env::var_os("OLO_REX_TEST_BUSY").is_some() {
            let made = ctrl
                .session_create(Some("olo-typing-test-busy"), Some(serde_json::json!({"shell": "none", "command": ["sh", "-c", "while :; do echo the quick brown fox jumps over the lazy dog; sleep 0.02; done"], "exit": {"on_completion": false}})))
                .await
                .expect("a second session");
            let busy_sid = made.payload["session_id"].as_str().unwrap().to_owned();
            let boot = ctrl
                .session_bootstrap(
                    "all",
                    Some(&busy_sid),
                    None,
                    &[DataTicketRequest { creator_name: "com.superlogical.terminal".into(), data_ref_id: "terminal".into(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.into()) }],
                )
                .await
                .expect("its terminal");
            let ticket = boot.payload.data_tickets.as_ref().and_then(|t| t.first()).map(|t| t.ticket.clone()).expect("a ticket");
            let (stream, host) = endpoint.connect_stream().await.expect("a third stream");
            let mut printing = TerminalDataStream::connect_hello(stream, &host, &ticket).await.expect("its bytes");
            let reader = tokio::spawn(async move { while printing.next_frame().await.is_ok() {} });
            println!("another terminal there prints all the while");
            Some((busy_sid, reader))
        } else {
            None
        };

        // The link's own round trip: a write of nothing much, answered.
        let count: usize = std::env::var("OLO_REX_TEST_TRIPS").ok().and_then(|n| n.parse().ok()).unwrap_or(5);
        let mut trips = Vec::new();
        for _ in 0..count {
            let at = Instant::now();
            ctrl.terminal_write(&sid, &bid, "\u{15}").await.expect("a write");
            trips.push(at.elapsed());
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(800), term.next_frame()).await {}
        trips.sort();
        println!(
            "a request and its answer: {} ms at the middle of {count}, {} ms at the least, {} ms at nineteen in twenty, {} ms at the most",
            trips[count / 2].as_millis(),
            trips[0].as_millis(),
            trips[count * 19 / 20].as_millis(),
            trips[count - 1].as_millis()
        );

        // What one write costs the server itself: writes sent together,
        // each answered before the next is given to it (by Olo's helper
        // there; without one they are given together, and the time is
        // the link's). `OLO_REX_TEST_TOGETHER` of them (none unless set).
        let together: usize = std::env::var("OLO_REX_TEST_TOGETHER").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
        if together > 0 {
            let at = Instant::now();
            let answers: Vec<_> = (0..together).filter_map(|_| ctrl.send_terminal_write(&sid, &bid, "x").ok()).collect();
            let mut times = Vec::new();
            for answer in answers {
                let _ = answer.await;
                times.push(at.elapsed());
            }
            // (Evenly apart: each waited for the one before it, there. In
            // a step: they were answered together and came in two
            // packets, the second kept for the first's acknowledgement.)
            let at = |i: usize| times[i.min(together - 1)].as_millis();
            println!(
                "{together} writes sent together: answered after {} ms the first, {} the second, {} a quarter of them, {} half, {} three quarters, {} the last",
                at(0),
                at(1),
                at(together / 4),
                at(together / 2),
                at(together * 3 / 4),
                at(together - 1)
            );
            let _ = ctrl.terminal_write(&sid, &bid, "\u{15}").await;
            while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(1200), term.next_frame()).await {}
        }

        for piped in [false, true] {
            let (typed_tx, mut typed_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Instant)>();
            let (sent_at_tx, mut sent_at_rx) = tokio::sync::mpsc::unbounded_channel::<Instant>();
            // The typist: a letter every `apart`, whatever became of the last.
            let typist = tokio::spawn(async move {
                for i in 0..letters {
                    let letter = char::from(b'a' + (i % 26) as u8).to_string();
                    let now = Instant::now();
                    let _ = sent_at_tx.send(now);
                    let _ = typed_tx.send((letter, now));
                    tokio::time::sleep(apart).await;
                }
            });
            // The sender, as Rex's own: one write waited for at a time, the
            // letters typed meanwhile gathered into the next; or each sent
            // as it comes.
            let (writer_ctrl, s, b) = (ctrl.clone(), sid.clone(), bid.clone());
            let writer = tokio::spawn(async move {
                let mut answers = Vec::new();
                while let Some((letter, _)) = typed_rx.recv().await {
                    // (As the app does at each key it takes: is the link there?)
                    writer_ctrl.probe();
                    if piped {
                        if let Ok(answer) = writer_ctrl.send_terminal_write(&s, &b, &letter) {
                            answers.push(tokio::spawn(answer));
                        }
                    } else {
                        let mut text = letter;
                        while let Ok((more, _)) = typed_rx.try_recv() {
                            text.push_str(&more);
                        }
                        let _ = writer_ctrl.terminal_write(&s, &b, &text).await;
                    }
                }
                for answer in answers {
                    let _ = answer.await;
                }
            });
            // The echoes, each letter's time back.
            let mut waits: Vec<Duration> = Vec::new();
            let mut echoed = String::new();
            let deadline = Instant::now() + apart * letters as u32 + Duration::from_secs(60);
            while waits.len() < letters && Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_secs(20), term.next_frame()).await {
                    Ok(Ok(TerminalStreamFrame::Bytes(bytes))) => {
                        for byte in bytes.iter().filter(|b| b.is_ascii_lowercase()) {
                            echoed.push(char::from(*byte));
                            if let Ok(at) = sent_at_rx.try_recv() {
                                waits.push(at.elapsed());
                            }
                        }
                    }
                    Ok(Ok(_)) => {}
                    _ => break,
                }
            }
            let _ = typist.await;
            let _ = writer.await;
            let expected: String = (0..letters).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
            if piped && !in_order {
                // (The server takes two requests at once and may write the
                // second first: why Rex doesn't type this way without its
                // helper there.)
                let mut letters_back: Vec<char> = echoed.chars().collect();
                letters_back.sort_unstable();
                let mut all: Vec<char> = expected.chars().collect();
                all.sort_unstable();
                assert_eq!(letters_back, all, "every letter came back");
                println!("each sent as typed: in the order typed: {}", echoed == expected);
            } else {
                assert_eq!(echoed, expected, "every letter came back, in the order typed");
            }
            let mut sorted = waits.clone();
            sorted.sort();
            println!(
                "{}: {letters} letters, {} ms apart: a letter's echo after {} ms at the middle, {} ms at nine in ten, {} ms at worst; the first after {} ms, the last after {} ms",
                if piped { "each sent as typed" } else { "one write at a time, gathered" },
                apart.as_millis(),
                sorted[sorted.len() / 2].as_millis(),
                sorted[sorted.len() * 9 / 10].as_millis(),
                sorted[sorted.len() - 1].as_millis(),
                waits[0].as_millis(),
                waits[waits.len() - 1].as_millis(),
            );
            // (A new line, and quiet, before the next.)
            let _ = ctrl.terminal_write(&sid, &bid, "\u{15}").await;
            while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(1200), term.next_frame()).await {}
        }
        // The sessions made for it are ended: the host is left as it was.
        let _ = ctrl.session_destroy(&sid).await;
        if let Some((busy_sid, reader)) = busy {
            reader.abort();
            let _ = ctrl.session_destroy(&busy_sid).await;
        }
    });
}

/// Olo's helper on a real host says how a terminal reads unasked
/// (`olo_rex_link::Teller`): a shell of this test's own there, a line
/// that is over at once, a command of two seconds, and what the helper
/// says of each and when, as the host's own server makes it say it
/// (its messages are the server's own, whatever its version). With
/// `OLO_REX_TEST_HELPER_FILE`, the build of the helper to put there, as
/// `typing_over_the_link_is_timed` takes it. The session is ended.
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_helper_says_a_prompt_on_a_real_host() {
    use olo_rex_client::{ClientIdentity, ControlClient, ControlEvent, RexEndpoint};
    use std::time::{Duration, Instant};
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    block(async move {
        let pool = Arc::new(SshSessionPool::default());
        let file = std::env::var("OLO_REX_TEST_HELPER_FILE").expect("OLO_REX_TEST_HELPER_FILE: the helper, built for the host");
        {
            use olo_rex_client::helper::{self, Build};
            let bytes: &'static [u8] = Box::leak(std::fs::read(&file).expect("the helper's file").into_boxed_slice());
            let builds = [Build { system: "Linux", machines: &["x86_64", "amd64", "aarch64", "arm64"], bytes }];
            let target = SshTarget::parse(&address).expect("an ssh:// address");
            // (Named for what it is: a build put there under the same
            // name before would be taken for this one.)
            let name = format!("test{:08x}", bytes.iter().fold(0x811c_9dc5u32, |hash, byte| (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)));
            helper::ensure(&pool, &target, Some(Arc::new(Nobody)), &name, &builds).await.expect("the helper is put on the host");
            pool.set_helper(Some(&name));
        }
        let endpoint = RexEndpoint::parse(&address, pool, Some(Arc::new(Nobody))).expect("an address");
        let (stream, host) = endpoint.connect_control_stream().await.expect("the host's Rex server is reached");
        let (ctrl, mut events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.expect("the control connection");
        assert!(ctrl.keeps_typing_in_order(), "the connection goes through the helper");
        let created = ctrl.session_create(Some("olo-prompt-test"), Some(serde_json::json!({"command": ["bash", "--norc", "-i"]}))).await.expect("a session");
        let sid = created.payload["session_id"].as_str().unwrap().to_owned();
        let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();
        ctrl.session_bootstrap("all", Some(&sid), None, &[]).await.expect("the session's events");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mut failed = None;
        for (line, what) in [("true\r", "a line over at once"), ("sleep 2\r", "a command of two seconds")] {
            let sent = Instant::now();
            ctrl.send_terminal_write(&sid, &bid, line).expect("sent").await.expect("the line is taken");
            let taken = sent.elapsed();
            let mut reads = None;
            while let Ok(Some(envelope)) = tokio::time::timeout_at((sent + Duration::from_secs(6)).into(), events.recv()).await {
                if let ControlEvent::BlockEvent { block_id, name, payload, .. } = envelope.event
                    && block_id == bid
                    && name == "input_mode_changed"
                    && payload.get("shell").is_some()
                {
                    let at = sent.elapsed();
                    println!("{what}: after {} ms the helper says echo {} canonical {} shell {} after {}", at.as_millis(), payload["echo"], payload["canonical"], payload["shell"], payload["after"]);
                    if payload["shell"] == true && payload["echo"] == false && payload["canonical"] == false && payload["after"] == ctrl.last_typed().to_string() {
                        reads = Some(at);
                        break;
                    }
                }
            }
            match reads {
                Some(at) => println!("{what}: the line was taken in {} ms (a round trip), and the shell said to read {} ms after it was sent", taken.as_millis(), at.as_millis()),
                None => failed = Some(what),
            }
        }
        let _ = ctrl.session_destroy(&sid).await;
        assert_eq!(failed, None, "the helper never said the shell reads again");
    });
}

/// How long a terminal on a real host takes to show once the host is
/// signed in to, over its own link: connecting as Rex did (each step
/// waited for before the next: the control connection's upgrade, its
/// hello, five requests one by one, then the terminal's stream, its
/// upgrade and its hello) and as it does (the hello and the five in
/// one flight with the upgrade, the terminal's stream opened as far as
/// its upgrade meanwhile). A session of this test's own is made there
/// and ended.
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn connecting_is_timed() {
    use olo_rex_client::{BootstrapResponse, ClientIdentity, ControlClient, DATA_TERMINAL_SUBPROTOCOL, DataTicketRequest, RexEndpoint, TerminalDataStream, TerminalStreamFrame, answered};
    use std::time::{Duration, Instant};
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    block(async move {
        let endpoint = RexEndpoint::parse(&address, Arc::new(SshSessionPool::default()), Some(Arc::new(Nobody))).expect("an address");
        endpoint.reach().await.expect("the host is signed in to");
        let (stream, host) = endpoint.connect_control_stream().await.expect("the host's Rex server is reached");
        let (setup, _events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.expect("a control connection");
        // (A run of this that failed left its session: those are ended first.)
        for left in setup.session_list().await.expect("the sessions").payload.into_iter().filter(|s| s.label == "olo-connect-test") {
            let _ = setup.session_destroy(&left.session_id).await;
        }
        let created = setup.session_create(Some("olo-connect-test"), Some(serde_json::json!({"shell": "none", "command": ["cat"], "exit": {"on_completion": false}}))).await.expect("a session");
        let sid = created.payload["session_id"].as_str().unwrap().to_owned();
        let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();
        // (Whatever happens from here, the session is ended: the rest
        // runs as a task of its own, whose end is waited for first.)
        let (endpoint, sid_measured) = (endpoint.clone(), sid.clone());
        let measured = tokio::spawn(async move {
            let sid = sid_measured;
            let mut trips = Vec::new();
            let (stream, host) = endpoint.connect_control_stream().await.unwrap();
            let (timed, _events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.unwrap();
            for _ in 0..5 {
                let at = Instant::now();
                timed.server_version().await.expect("a request");
                trips.push(at.elapsed());
            }
            drop(timed);
            trips.sort();
            let trip = trips[2];
            println!("a request and its answer: {} ms", trip.as_millis());
            let tickets = [DataTicketRequest { creator_name: "com.superlogical.terminal".into(), data_ref_id: "terminal".into(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.into()) }];
            let ticket_of = |boot: BootstrapResponse| boot.data_tickets.unwrap_or_default().into_iter().find(|t| t.block_id == bid).expect("a ticket for the terminal").ticket;
            let first_bytes = |mut terminal: TerminalDataStream<olo_rex_client::RexStream>| async move {
                assert!(matches!(tokio::time::timeout(Duration::from_secs(10), terminal.next_frame()).await.expect("in time").expect("a frame"), TerminalStreamFrame::Bytes(_)));
            };
            for turn in 0..2 {
                // As it was: one thing after another. (Each step's own time is said, in round trips.)
                let began = Instant::now();
                let mut steps = Vec::new();
                let mut step = |what: &'static str, since: &mut Instant| {
                    steps.push(format!("{what} {:.1}", since.elapsed().as_secs_f64() / trip.as_secs_f64()));
                    *since = Instant::now();
                };
                let mut since = began;
                let (stream, host) = endpoint.connect_control_stream().await.unwrap();
                step("a stream", &mut since);
                let (ctrl, _events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.unwrap();
                step("upgrade and hello", &mut since);
                ctrl.server_status().await.unwrap();
                ctrl.server_version().await.unwrap();
                ctrl.hosts_list().await.unwrap();
                ctrl.client_keymap(&[], &[], &[]).await.unwrap();
                let boot = ctrl.session_bootstrap("all", Some(&sid), None, &tickets).await.unwrap().payload;
                step("five requests", &mut since);
                let (stream, host) = endpoint.connect_stream().await.unwrap();
                step("a stream", &mut since);
                let upgrade = TerminalDataStream::upgrade(stream, &host).await.unwrap();
                step("its upgrade", &mut since);
                let terminal = upgrade.hello(&ticket_of(boot)).await.unwrap();
                step("its hello", &mut since);
                first_bytes(terminal).await;
                step("first bytes", &mut since);
                if turn == 0 {
                    println!("as it was, step by step, in round trips: {}", steps.join(", "));
                }
                let was = began.elapsed();
                drop(ctrl);

                // As it is: side by side, and in one flight.
                let began = Instant::now();
                let spare = {
                    let endpoint = endpoint.clone();
                    tokio::spawn(async move {
                        let (stream, host) = endpoint.connect_stream().await.unwrap();
                        TerminalDataStream::upgrade(stream, &host).await.unwrap()
                    })
                };
                let asked = vec![
                    ("server.status", None),
                    ("server.version", None),
                    ("hosts.list", None),
                    ("client.keymap", Some(ControlClient::keymap_request(&[], &[], &[]).unwrap())),
                    ("session.bootstrap", Some(ControlClient::bootstrap_request("all", Some(&sid), None, &tickets).unwrap())),
                ];
                let (stream, host) = endpoint.connect_control_stream().await.unwrap();
                let (ctrl, _events, answers) = ControlClient::connect_asking(stream, &host, ClientIdentity::olo_rex(None), None, asked).await.expect("the server takes the flight");
                let mut boot = None;
                for (i, answer) in answers.into_iter().enumerate() {
                    let answer = answer.await;
                    if i == 4 {
                        boot = Some(answered::<BootstrapResponse>("session.bootstrap", answer).unwrap().payload);
                    } else {
                        answer.unwrap();
                    }
                }
                first_bytes(spare.await.unwrap().hello(&ticket_of(boot.unwrap())).await.unwrap()).await;
                let is = began.elapsed();
                drop(ctrl);
                println!(
                    "turn {turn}: a terminal's first bytes after signing in: {} ms as it was ({:.1} round trips), {} ms as it is ({:.1})",
                    was.as_millis(),
                    was.as_secs_f64() / trip.as_secs_f64(),
                    is.as_millis(),
                    is.as_secs_f64() / trip.as_secs_f64()
                );
            }
        });
        let measured = measured.await;
        let _ = setup.session_destroy(&sid).await;
        measured.expect("measured");
    });
}

/// A flood on a narrow link, and a request beside it: a terminal on
/// the host writes three million lines, its stream is read as it comes
/// (as the app reads it), and once a second a small request is made on
/// the control connection and timed. How long the request waits behind
/// the flood is how long anything else on the connection does (another
/// terminal, a key's answer, the word that a command has ended).
/// `OLO_REX_SSH_WINDOW`: how much the host may send on a channel
/// before it is told to go on (russh's own: 2 MiB). The session is
/// ended.
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn a_request_beside_a_flood_is_timed() {
    use olo_rex_client::{ClientIdentity, ControlClient, DATA_TERMINAL_SUBPROTOCOL, RexEndpoint, TerminalDataStream, TerminalStreamFrame};
    use std::time::{Duration, Instant};
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    block(async move {
        let endpoint = RexEndpoint::parse(&address, Arc::new(SshSessionPool::default()), Some(Arc::new(Nobody))).expect("an address");
        let (stream, host) = endpoint.connect_control_stream().await.expect("the host's Rex server is reached");
        let (ctrl, _events) = ControlClient::connect(stream, &host, ClientIdentity::olo_rex(None), None).await.expect("a control connection");
        for left in ctrl.session_list().await.expect("the sessions").payload.into_iter().filter(|s| s.label == "olo-flood-test") {
            let _ = ctrl.session_destroy(&left.session_id).await;
        }
        let created = ctrl.session_create(Some("olo-flood-test"), Some(serde_json::json!({"command": ["bash", "--norc", "-i"]}))).await.expect("a session");
        let sid = created.payload["session_id"].as_str().unwrap().to_owned();
        let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();
        let (endpoint, ctrl_measured, sid_measured) = (endpoint.clone(), ctrl.clone(), sid.clone());
        let measured = tokio::spawn(async move {
            let (ctrl, sid) = (ctrl_measured, sid_measured);
            ctrl.session_bootstrap("all", Some(&sid), None, &[]).await.unwrap();
            let ticket = ctrl.data_connect(&sid, &bid, "terminal", DATA_TERMINAL_SUBPROTOCOL).await.unwrap().payload.ticket;
            let (stream, host) = endpoint.connect_stream().await.unwrap();
            let mut terminal = TerminalDataStream::connect_hello(stream, &host, &ticket).await.unwrap();
            let got = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let counted = got.clone();
            let reader = tokio::spawn(async move {
                while let Ok(TerminalStreamFrame::Bytes(chunk)) = terminal.next_frame().await {
                    counted.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
                }
            });
            tokio::time::sleep(Duration::from_millis(800)).await;
            let quiet = Instant::now();
            ctrl.server_version().await.unwrap();
            let quiet = quiet.elapsed();
            ctrl.terminal_write(&sid, &bid, "seq 3000000\n").await.unwrap();
            let began = Instant::now();
            let mut waits = Vec::new();
            for _ in 0..12 {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                let at = Instant::now();
                let answered = tokio::time::timeout(Duration::from_secs(40), ctrl.server_version()).await;
                waits.push(if answered.is_ok() { at.elapsed().as_millis().to_string() } else { "more than 40000".to_owned() });
            }
            println!(
                "a request beside nothing: {} ms; beside the flood, one a second: {} ms; the stream had brought {} kB in {:.0} s",
                quiet.as_millis(),
                waits.join(", "),
                got.load(std::sync::atomic::Ordering::Relaxed) / 1000,
                began.elapsed().as_secs_f64()
            );
            reader.abort();
        });
        let measured = measured.await;
        let _ = ctrl.session_destroy(&sid).await;
        measured.expect("measured");
    });
}
