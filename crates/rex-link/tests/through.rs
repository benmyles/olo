//! The helper itself, run: between this test (the app's side) and a
//! stand-in for the Rex server that answers as the server does, each
//! request in its own time.
//!
//! The stand-in is this same program, run by the helper with
//! `OLO_REX_LINK_STANDIN` set ([`standin`]): so the test has no harness
//! but its own `main`, which runs each test in turn.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use olo_rex_link::{HERE, Piece, Scanner, TYPED};

fn main() {
    if let Ok(args) = std::env::var("OLO_REX_LINK_STANDIN") {
        return standin(&args.split_whitespace().map(str::to_owned).collect::<Vec<_>>());
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    // (Nothing here is ignored, so a run of the ignored tests is none.)
    if args.iter().any(|a| a == "--ignored" || a == "--list") {
        return println!("\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
    }
    let only = args.iter().find(|a| !a.starts_with('-'));
    let tests: [(&str, fn()); 8] = [
        ("what_is_typed_reaches_the_server_one_at_a_time_in_order_however_it_is_sent", what_is_typed_reaches_the_server_one_at_a_time_in_order_however_it_is_sent),
        ("a_burst_is_passed_as_fast_as_its_answers_come", a_burst_is_passed_as_fast_as_its_answers_come),
        ("a_request_never_answered_holds_the_rest_its_time_and_no_longer", a_request_never_answered_holds_the_rest_its_time_and_no_longer),
        ("an_upgrade_refused_passes_whole_and_nothing_says_a_helper_is_there", an_upgrade_refused_passes_whole_and_nothing_says_a_helper_is_there),
        ("a_frame_too_large_to_keep_passes_either_way_and_typing_goes_on", a_frame_too_large_to_keep_passes_either_way_and_typing_goes_on),
        ("what_the_app_sent_last_reaches_the_server_before_it_ends", what_the_app_sent_last_reaches_the_server_before_it_ends),
        ("the_app_gone_the_helper_ends", the_app_gone_the_helper_ends),
        ("how_a_terminal_reads_is_said_unasked_after_an_enter_and_nothing_else_is_added", how_a_terminal_reads_is_said_unasked_after_an_enter_and_nothing_else_is_added),
    ];
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    println!("\nrunning {} tests", tests.len());
    for (name, test) in tests {
        if only.is_some_and(|only| !name.contains(only.as_str())) {
            skipped += 1;
            continue;
        }
        match std::panic::catch_unwind(test) {
            Ok(()) => {
                println!("test {name} ... ok");
                passed += 1;
            }
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    println!("\ntest result: {}. {passed} passed; {failed} failed; 0 ignored; 0 measured; {skipped} filtered out", if failed == 0 { "ok" } else { "FAILED" });
    if failed > 0 {
        std::process::exit(101);
    }
}

const UPGRADE: &[u8] = b"GET /api/control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\r\n";

/// What the stand-in says when it refuses the upgrade (`--refuse`).
const REFUSAL: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: 27\r\n\r\nthis server is not for you\n";

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % below.max(1)
    }
}

/// A moment after the server has taken an Enter typed into a terminal
/// the app is told how that terminal reads, in one event, having asked
/// nothing; a write with no Enter brings none; the answers to the
/// helper's own questions never reach the app; and what the app sent
/// is all the server was given but those two questions.
fn how_a_terminal_reads_is_said_unasked_after_an_enter_and_nothing_else_is_added() {
    let mut rng = Lcg(7);
    let mut through = Through::new(&["--reads", "--slow-ms", "1"]);
    // A letter: answered, and nothing more comes.
    through.to.write_all(&frame(&write(1), &mut rng)).unwrap();
    let mut seen: Vec<serde_json::Value> = Vec::new();
    let read = |through: &mut Through, within: Duration| {
        let until = Instant::now() + within;
        let mut got = Vec::new();
        while let Ok(piece) = through.from.recv_timeout(until.saturating_duration_since(Instant::now())) {
            if let Piece::Frame { text: Some(text), .. } = piece {
                got.push(serde_json::from_slice::<serde_json::Value>(&text).unwrap());
            }
        }
        got
    };
    seen.extend(read(&mut through, Duration::from_millis(300)));
    assert!(seen.iter().any(|v| v["type"] == "response" && v["id"] == "1"), "{seen:?}");
    assert!(!seen.iter().any(|v| v.to_string().contains("input_mode_changed")), "no Enter, nothing said: {seen:?}");
    // An Enter: its answer, and then the event, in that order, within
    // a moment and a little.
    let sent = Instant::now();
    through.to.write_all(&frame(&enter(2), &mut rng)).unwrap();
    let got = read(&mut through, Duration::from_millis(600));
    let answer = got.iter().position(|v| v["type"] == "response" && v["id"] == "2").expect("the Enter's answer");
    let said =
        got.iter().position(|v| v["payload"]["payload"]["event"]["payload"]["name"] == "input_mode_changed").unwrap_or_else(|| panic!("the event: {got:?} ({:?} after the Enter)", sent.elapsed()));
    assert!(said > answer, "the event after the answer it follows: {got:?}");
    let event = &got[said]["payload"]["payload"];
    assert_eq!((event["session_id"].as_str(), event["event"]["payload"]["block_id"].as_str()), (Some("s"), Some("b")));
    assert_eq!(event["event"]["payload"]["payload"], serde_json::json!({ "echo": false, "canonical": false, "signals": true, "shell": true, "after": "2" }));
    // Nothing of the helper's own questions reaches the app.
    seen.extend(got);
    assert!(!seen.iter().any(|v| v["id"].as_str().is_some_and(|id| id.starts_with("olo-rex-link"))), "{seen:?}");
    assert_eq!(seen.iter().filter(|v| v.to_string().contains("input_mode_changed")).count(), 1);
    // Lines and letters sent all at once, cut anywhere: every one is
    // answered, in order, none of the helper's own answers among them;
    // and the last line's word comes, of after everything typed.
    let mut all = Vec::new();
    for id in 3..203 {
        all.extend(frame(&if id % 3 == 0 { enter(id) } else { write(id) }, &mut rng));
    }
    while !all.is_empty() {
        let n = (1 + rng.next(700) as usize).min(all.len());
        through.to.write_all(&all[..n]).unwrap();
        all.drain(..n);
    }
    let got = read(&mut through, Duration::from_millis(1500));
    let answered: Vec<usize> = got.iter().filter(|v| v["type"] == "response").map(|v| v["id"].as_str().unwrap().parse().expect("only the app's own answers")).collect();
    assert_eq!(answered, (3..203).collect::<Vec<_>>());
    // (A word while the burst passed, if the server was ever done with
    // all that had come: each of after more typed than the last. And
    // one at its end, of after everything.)
    let after: Vec<usize> = got
        .iter()
        .filter(|v| v["payload"]["payload"]["event"]["payload"]["name"] == "input_mode_changed")
        .map(|v| v["payload"]["payload"]["event"]["payload"]["payload"]["after"].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(after.windows(2).all(|pair| pair[0] < pair[1]), "{after:?}");
    assert_eq!(after.last(), Some(&202), "{after:?}");
    // One more line, by itself: said, after it.
    through.to.write_all(&frame(&enter(203), &mut rng)).unwrap();
    let got = read(&mut through, Duration::from_millis(600));
    let words: Vec<&serde_json::Value> = got.iter().filter(|v| v["payload"]["payload"]["event"]["payload"]["name"] == "input_mode_changed").collect();
    assert_eq!(words.len(), 1, "{got:?}");
    assert_eq!(words[0]["payload"]["payload"]["event"]["payload"]["payload"]["after"], "203");
}

/// A text frame as the app sends it (masked).
fn frame(text: &str, rng: &mut Lcg) -> Vec<u8> {
    let payload = text.as_bytes();
    let mut out = vec![0x81];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    let key = [rng.next(256) as u8, rng.next(256) as u8, rng.next(256) as u8, rng.next(256) as u8];
    out.extend_from_slice(&key);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    out
}

struct Through {
    helper: Child,
    to: ChildStdin,
    /// The helper's output cut into pieces, by a thread that reads it.
    from: Receiver<Piece>,
}

impl Through {
    fn new(standin: &[&str]) -> Through {
        let through = Through::begun(standin);
        let Piece::Head(head) = through.from.recv_timeout(Duration::from_secs(10)).expect("the upgrade's answer") else { panic!("an answer to the upgrade first") };
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("HTTP/1.1 101") && head.contains(&format!("\r\n{HERE}\r\n")), "the helper says it is there: {head:?}");
        through
    }

    /// The helper started and the upgrade asked for; its answer not
    /// yet read.
    fn begun(standin: &[&str]) -> Through {
        // (The stand-in is this program again, told so by the variable.
        // A request is held a second and a half for another's answer.)
        let mut helper = Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .env("OLO_REX_LINK_STANDIN", standin.join(" "))
            .env("OLO_REX_LINK_HELD_MS", "1500")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (to, mut out) = (helper.stdin.take().unwrap(), helper.stdout.take().unwrap());
        let (tx, from) = channel();
        std::thread::spawn(move || {
            let mut scanner = Scanner::new();
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut pieces = scanner.push(&buf[..n]);
                // (An upgrade refused is followed by a page, not frames.)
                if pieces.iter().any(|piece| matches!(piece, Piece::Head(head) if !olo_rex_link::upgraded(head))) {
                    pieces.extend(scanner.plain());
                }
                for piece in pieces {
                    if tx.send(piece).is_err() {
                        return;
                    }
                }
            }
        });
        let mut through = Through { helper, to, from };
        through.to.write_all(UPGRADE).unwrap();
        through
    }

    /// The next answer (events passed over): its id, and what the
    /// stand-in said of the request (its place as it arrived, whether
    /// another typed request was unanswered then).
    fn answer(&mut self, within: Duration) -> Option<(String, u64, bool)> {
        let until = Instant::now() + within;
        loop {
            let piece = self.from.recv_timeout(until.saturating_duration_since(Instant::now())).ok()?;
            let Piece::Frame { text: Some(text), .. } = piece else { continue };
            let value: serde_json::Value = serde_json::from_slice(&text).unwrap();
            if value["type"] == "response" {
                return Some((value["id"].as_str().unwrap().to_owned(), value["payload"]["arrived"].as_u64().unwrap(), value["payload"]["overlap"].as_bool().unwrap()));
            }
        }
    }
}

impl Drop for Through {
    fn drop(&mut self) {
        let _ = self.helper.kill();
        let _ = self.helper.wait();
    }
}

fn write(id: usize) -> String {
    format!(r#"{{"type":"request","id":"{id}","method":"com.superlogical.terminal.write","payload":{{"session_id":"s","block_id":"b","args":{{"data":"k"}}}}}}"#)
}

/// A write that ends a line.
fn enter(id: usize) -> String {
    format!(r#"{{"type":"request","id":"{id}","method":"com.superlogical.terminal.write","payload":{{"session_id":"s","block_id":"b","args":{{"data":"ls\r"}}}}}}"#)
}

fn other(id: usize) -> String {
    format!(r#"{{"type":"request","id":"{id}","method":"session.view","payload":{{"session_id":"s"}}}}"#)
}

/// What is typed, sent all at once and in pieces cut anywhere, with
/// other requests among it: every request is answered, the typed ones
/// reached the server in the order sent, and never one while another
/// was unanswered. Many times over, each its own way.
fn what_is_typed_reaches_the_server_one_at_a_time_in_order_however_it_is_sent() {
    let rounds: u64 = std::env::var("OLO_REX_LINK_ROUNDS").ok().and_then(|n| n.parse().ok()).unwrap_or(12);
    for round in 0..rounds {
        let mut rng = Lcg(round * 7 + 1);
        let mut through = Through::new(&["--slow-ms", if round % 3 == 0 { "0" } else { "2" }]);
        let count = 120 + rng.next(200) as usize;
        let mut bytes = Vec::new();
        // The typed requests' ids, in the order sent.
        let mut typed = Vec::new();
        for id in 1..=count {
            if rng.next(4) == 0 {
                bytes.extend(frame(&other(id), &mut rng));
            } else {
                bytes.extend(frame(&write(id), &mut rng));
                typed.push(id.to_string());
            }
        }
        // (Sent from a thread: the answers are read meanwhile.)
        let mut to = through.to.try_clone_or_take();
        let mut cut = Lcg(round + 99);
        let sender = std::thread::spawn(move || {
            let mut at = 0;
            while at < bytes.len() {
                let most = if cut.next(3) == 0 { 4000 } else { 30 };
                let n = (1 + cut.next(most) as usize).min(bytes.len() - at);
                to.write_all(&bytes[at..at + n]).unwrap();
                at += n;
                if cut.next(5) == 0 {
                    std::thread::sleep(Duration::from_micros(cut.next(1500)));
                }
            }
            to
        });
        let mut places = std::collections::HashMap::new();
        for _ in 0..count {
            let (id, arrived, overlap) = through.answer(Duration::from_secs(30)).unwrap_or_else(|| panic!("round {round}: every request is answered ({} of {count} were)", places.len()));
            assert!(!overlap, "round {round}: request {id} reached the server while another typed one was unanswered");
            places.insert(id, arrived);
        }
        for (i, id) in typed.iter().enumerate() {
            assert_eq!(places[id], i as u64 + 1, "round {round}: typed request {id} reached the server in its place");
        }
        drop(sender.join().unwrap());
    }
}

/// How long the helper keeps a burst: printed, and held to a bound (a
/// held request goes on when the last one's answer has come, not at a
/// clock's tick).
fn a_burst_is_passed_as_fast_as_its_answers_come() {
    let mut rng = Lcg(3);
    let mut through = Through::new(&["--slow-ms", "0"]);
    let count = 400;
    let mut bytes = Vec::new();
    for id in 1..=count {
        bytes.extend(frame(&write(id), &mut rng));
    }
    let began = Instant::now();
    through.to.write_all(&bytes).unwrap();
    for _ in 0..count {
        through.answer(Duration::from_secs(30)).expect("an answer");
    }
    let took = began.elapsed();
    println!("{count} typed requests sent at once, each answered at once: all through in {:.1} ms ({:.0} µs each)", took.as_secs_f64() * 1000.0, took.as_secs_f64() * 1e6 / count as f64);
    assert!(took < Duration::from_secs(4), "{took:?}");
}

/// A request the server never answers holds what is typed its time
/// (a second and a half here), then all that waited goes on together.
fn a_request_never_answered_holds_the_rest_its_time_and_no_longer() {
    let mut rng = Lcg(5);
    let mut through = Through::new(&["--slow-ms", "0", "--never", "2"]);
    for id in 1..=4 {
        through.to.write_all(&frame(&write(id), &mut rng)).unwrap();
    }
    let began = Instant::now();
    assert_eq!(through.answer(Duration::from_secs(5)).map(|a| a.0).as_deref(), Some("1"));
    // (The second is never answered; the third and fourth wait for it
    // its time, and go on together.)
    let mut later = vec![through.answer(Duration::from_secs(8)).expect("the third and fourth are let through").0];
    let waited = began.elapsed();
    later.push(through.answer(Duration::from_secs(5)).expect("both").0);
    later.sort();
    assert_eq!(later, ["3", "4"]);
    assert!(waited >= Duration::from_millis(1400) && waited < Duration::from_millis(3000), "{waited:?}");
    assert!(began.elapsed() < Duration::from_millis(3000), "together: {:?}", began.elapsed());
}

/// The server says no to the upgrade (it answers with a page): the
/// page reaches the app whole, as it comes, and nothing says a helper
/// is there.
fn an_upgrade_refused_passes_whole_and_nothing_says_a_helper_is_there() {
    let through = Through::begun(&["--refuse"]);
    let mut got = Vec::new();
    let until = Instant::now() + Duration::from_secs(10);
    while got.len() < REFUSAL.len() && Instant::now() < until {
        if let Ok(piece) = through.from.recv_timeout(Duration::from_millis(200)) {
            got.extend_from_slice(piece.bytes());
        }
    }
    assert_eq!(String::from_utf8_lossy(&got), String::from_utf8_lossy(REFUSAL));
}

/// A frame too large to keep (17 MiB) passes either way, as it comes,
/// and what is typed after it is taken and answered as before.
fn a_frame_too_large_to_keep_passes_either_way_and_typing_goes_on() {
    let mut rng = Lcg(9);
    // From the app: a binary frame of 17 MiB, then two typed requests.
    let mut through = Through::new(&["--slow-ms", "0", "--big"]);
    let big = 17 << 20;
    let mut bytes = vec![0x82, 0x80 | 127];
    bytes.extend_from_slice(&(big as u64).to_be_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes.resize(bytes.len() + big, 5);
    bytes.extend(frame(&write(1), &mut rng));
    bytes.extend(frame(&write(2), &mut rng));
    let mut to = through.to.try_clone_or_take();
    let sender = std::thread::spawn(move || {
        to.write_all(&bytes).unwrap();
        to
    });
    // From the server (the stand-in sends as large a frame before its
    // first answer): every byte of it comes, then the answers.
    let (mut passed, mut answers) = (0usize, Vec::new());
    let until = Instant::now() + Duration::from_secs(60);
    while answers.len() < 2 && Instant::now() < until {
        match through.from.recv_timeout(Duration::from_millis(500)) {
            Ok(Piece::Raw(bytes)) => passed += bytes.len(),
            Ok(Piece::Frame { text: Some(text), .. }) => {
                let value: serde_json::Value = serde_json::from_slice(&text).unwrap();
                if value["type"] == "response" {
                    answers.push((value["id"].as_str().unwrap().to_owned(), value["payload"]["overlap"].as_bool().unwrap()));
                }
            }
            _ => {}
        }
    }
    assert_eq!(passed, 10 + big, "the server's large frame came whole");
    assert_eq!(answers, [("1".to_owned(), false), ("2".to_owned(), false)]);
    drop(sender.join().unwrap());
}

/// What the app sent last, and a piece cut off by its going, reach the
/// server before the server's side ends: the stand-in says how many
/// bytes it was given, as it ends.
fn what_the_app_sent_last_reaches_the_server_before_it_ends() {
    let mut rng = Lcg(13);
    let mut through = Through::new(&["--slow-ms", "0", "--count"]);
    let mut bytes = Vec::new();
    for id in 1..=20 {
        bytes.extend(frame(&write(id), &mut rng));
    }
    // (And the beginning of a frame that never ends.)
    bytes.extend_from_slice(&[0x81, 0x85, 1, 2, 3, 4, 9]);
    let sent = bytes.len() + UPGRADE.len();
    let mut to = through.to.try_clone_or_take();
    to.write_all(&bytes).unwrap();
    drop(to);
    let until = Instant::now() + Duration::from_secs(20);
    let mut counted = None;
    while counted.is_none() && Instant::now() < until {
        if let Ok(Piece::Frame { text: Some(text), .. }) = through.from.recv_timeout(Duration::from_millis(200))
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&text)
            && value["type"] == "counted"
        {
            counted = value["bytes"].as_u64();
        }
    }
    assert_eq!(counted, Some(sent as u64), "every byte the app sent reached the server");
}

/// The app going away ends the helper and the server's side with it.
fn the_app_gone_the_helper_ends() {
    let mut through = Through::new(&[]);
    let to = through.to.try_clone_or_take();
    drop(to);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if through.helper.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < until, "the helper is still there ten seconds after its input closed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

trait TakeStdin {
    fn try_clone_or_take(&mut self) -> ChildStdin;
}

impl TakeStdin for ChildStdin {
    /// The helper's input, handed to another thread (and a closed
    /// stand-in left in its place).
    fn try_clone_or_take(&mut self) -> ChildStdin {
        let null = Command::new("true").stdin(Stdio::piped()).spawn().unwrap().stdin.take().unwrap();
        std::mem::replace(self, null)
    }
}

/// A text frame as the server sends it (not masked).
fn answer_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let mut out = vec![0x81];
    match payload.len() {
        n if n < 126 => out.push(n as u8),
        n if n < 65536 => {
            out.push(126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

/// A stand-in for the Rex server behind the helper: it speaks as much
/// of the control connection as the helper reads, on its input and
/// output, and behaves as the server does where that matters: every
/// request is answered by a thread of its own after a moment of its own
/// choosing, so two that are with it together may be answered either
/// way round.
///
/// Its answer to a request says how the request found it: `arrived`,
/// the request's place among the typed ones as they reached it, and
/// `overlap`, whether another typed request was unanswered then (which
/// the helper is there to prevent).
///
/// `--slow-ms N`: the longest an answer is kept (2 unless given).
/// `--never K`: the K-th typed request is never answered.
/// `--refuse`: the upgrade is refused, with a page.
/// `--big`: a frame of 17 MiB is sent before the first answer.
/// `--count`: as its input ends it says how many bytes it was given.
fn standin(args: &[String]) {
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<u64>().ok());
    let slow = after("--slow-ms").unwrap_or(2);
    let never = after("--never");
    let (refuse, mut big, count) = (args.iter().any(|a| a == "--refuse"), args.iter().any(|a| a == "--big"), args.iter().any(|a| a == "--count"));
    let reads = args.iter().any(|a| a == "--reads");
    let mut given = 0u64;
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let unanswered = Arc::new(AtomicUsize::new(0));
    let arrived = AtomicU64::new(0);
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut scanner = Scanner::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut stdin = std::io::stdin().lock();
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        given += n as u64;
        for piece in scanner.push(&buf[..n]) {
            match piece {
                Piece::Head(_) if refuse => {
                    let mut out = out.lock().unwrap();
                    let _ = out.write_all(REFUSAL);
                    let _ = out.flush();
                }
                Piece::Head(_) => {
                    let mut out = out.lock().unwrap();
                    let _ = out.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n");
                    let _ = out.flush();
                }
                Piece::Frame { text: Some(text), .. } => {
                    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&text) else { continue };
                    let Some(id) = value.get("id").and_then(|id| id.as_str()).map(str::to_owned) else { continue };
                    let typed = value.get("method").and_then(|m| m.as_str()).is_some_and(|m| TYPED.contains(&m));
                    if std::mem::take(&mut big) {
                        let mut frame = vec![0x82, 127];
                        frame.extend_from_slice(&((17u64 << 20).to_be_bytes()));
                        frame.resize(10 + (17 << 20), 6);
                        let mut out = out.lock().unwrap();
                        let _ = out.write_all(&frame);
                    }
                    // (How a terminal reads, asked as the server is: a
                    // shell at its line editor, where the stand-in is
                    // told to say so.)
                    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or_default();
                    if reads && (method.ends_with(".input_mode") || method.ends_with(".process")) {
                        let payload = if method.ends_with(".input_mode") {
                            r#"{"echo":false,"canonical":false,"signals":true}"#
                        } else {
                            r#"{"child":{"pid":24,"name":"bash"},"foreground":{"pid":24,"name":"bash"}}"#
                        };
                        let mut out = out.lock().unwrap();
                        let _ = out.write_all(&answer_frame(&format!(r#"{{"type":"response","id":"{id}","seq":5,"payload":{payload}}}"#)));
                        let _ = out.flush();
                        continue;
                    }
                    let (place, overlap) = if typed { (arrived.fetch_add(1, Ordering::SeqCst) + 1, unanswered.fetch_add(1, Ordering::SeqCst) > 0) } else { (0, false) };
                    if typed && never == Some(place) {
                        continue;
                    }
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let kept = Duration::from_micros((seed >> 33) % (slow * 1000 + 1));
                    let (out, unanswered) = (out.clone(), unanswered.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(kept);
                        let mut out = out.lock().unwrap();
                        // (An event before some answers, as the server's are.)
                        if kept.as_micros() % 3 == 0 {
                            let _ = out.write_all(&answer_frame(r#"{"type":"event","seq":1,"payload":{"payload":{"type":"response"}}}"#));
                        }
                        if typed {
                            unanswered.fetch_sub(1, Ordering::SeqCst);
                        }
                        let _ = out.write_all(&answer_frame(&format!(r#"{{"type":"response","id":"{id}","seq":1,"payload":{{"arrived":{place},"overlap":{overlap}}}}}"#)));
                        let _ = out.flush();
                    });
                }
                _ => {}
            }
        }
    }
    if count {
        // (Once the answers still on their way are out.)
        std::thread::sleep(Duration::from_millis(100));
        let mut out = out.lock().unwrap();
        let _ = out.write_all(&answer_frame(&format!(r#"{{"type":"counted","bytes":{given}}}"#)));
        let _ = out.flush();
    }
}
