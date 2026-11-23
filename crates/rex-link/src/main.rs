//! `olo-rex-link [--] [COMMAND…]`: run on a Rex host, over SSH, in the
//! place of `rex __internal dial-stdio` for the app's control
//! connection. It runs COMMAND (that command, unless another is given)
//! and stands between: see the library.
//!
//! Four threads, each waiting on one thing and none on a clock unless
//! something is held: what the app sends (to the gate), what the gate
//! lets go (to the server), what the server sends (to the app, and an
//! answer to the gate), and the time after which a held request waits
//! no longer. Nothing is written while the gate is locked: a server
//! that stops reading stops only what goes to it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use olo_rex_link::{Gate, Piece, Scanner, announced, upgraded};

/// The most that waits to be written to the server before no more is
/// read from the app (which SSH then holds back in its turn).
const MOST_WAITING: usize = 4 << 20;

/// The gate, and the way on to the server's writer (one lock for both:
/// what passes the gate is queued in the order it passed).
struct Onward {
    gate: Gate,
    to_server: Option<Sender<Vec<u8>>>,
}

struct Shared {
    onward: Mutex<Onward>,
    /// Something of the gate changed: the timer looks again.
    changed: Condvar,
    /// The bytes queued for the server and not yet written.
    waiting: Mutex<usize>,
    written: Condvar,
}

impl Shared {
    fn send(&self, onward: &mut Onward, pieces: Vec<Vec<u8>>) {
        for bytes in pieces {
            let Some(to_server) = &onward.to_server else { return };
            *self.waiting.lock().unwrap_or_else(|e| e.into_inner()) += bytes.len();
            if to_server.send(bytes).is_err() {
                onward.to_server = None;
            }
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version") {
        println!("olo-rex-link {}", olo_rex_link::VERSION);
        return;
    }
    if args.first().is_some_and(|a| a == "--") {
        args.remove(0);
    }
    if args.is_empty() {
        args = ["rex", "__internal", "dial-stdio"].map(str::to_owned).to_vec();
    }
    let mut child = match Command::new(&args[0]).args(&args[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("olo-rex-link: couldn't run {}: {e}", args[0]);
            std::process::exit(127);
        }
    };
    // (`OLO_REX_LINK_HELD_MS`: how long a request is held for another's
    // answer, for the tests of that; `HELD_AT_MOST` otherwise.)
    let gate = match std::env::var("OLO_REX_LINK_HELD_MS").ok().and_then(|ms| ms.parse().ok()) {
        Some(ms) => Gate::holding(std::time::Duration::from_millis(ms)),
        None => Gate::new(),
    };
    let (to_server, queued) = channel::<Vec<u8>>();
    let shared = Arc::new(Shared { onward: Mutex::new(Onward { gate, to_server: Some(to_server) }), changed: Condvar::new(), waiting: Mutex::new(0), written: Condvar::new() });
    let mut server_in = child.stdin.take().expect("the server's side");
    let mut from_server = child.stdout.take().expect("the server's side");

    // To the server, in the order the gate let things go. (Its end
    // closes the server's input: the app has gone, or the server has.)
    let writer = shared.clone();
    std::thread::spawn(move || {
        for bytes in queued {
            let done = server_in.write_all(&bytes).is_err();
            *writer.waiting.lock().unwrap_or_else(|e| e.into_inner()) -= bytes.len();
            writer.written.notify_all();
            if done {
                break;
            }
        }
    });

    // From the app, through the gate.
    let up = shared.clone();
    std::thread::spawn(move || {
        let mut scanner = Scanner::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut stdin = std::io::stdin().lock();
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            {
                let mut onward = up.onward.lock().unwrap_or_else(|e| e.into_inner());
                for piece in scanner.push(&buf[..n]) {
                    let gone = onward.gate.from_app(piece, Instant::now());
                    up.send(&mut onward, gone);
                }
                if onward.to_server.is_none() {
                    return;
                }
            }
            up.changed.notify_all();
            // (A server that isn't reading: no more is read for it.)
            let mut waiting = up.waiting.lock().unwrap_or_else(|e| e.into_inner());
            while *waiting > MOST_WAITING {
                waiting = up.written.wait(waiting).unwrap_or_else(|e| e.into_inner());
            }
        }
        // The app has gone: what it sent goes on, to the last byte
        // (a piece cut off too, and what was held), and then the
        // server's side of it ends.
        let mut onward = up.onward.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut gone = Vec::new();
        if let Some(piece) = scanner.rest() {
            gone.extend(onward.gate.from_app(piece, now));
        }
        gone.extend(onward.gate.open(now));
        up.send(&mut onward, gone);
        onward.to_server = None;
        drop(onward);
        up.changed.notify_all();
    });

    // What a request's answer never coming lets go, at its time (one
    // wait while something is held, none otherwise).
    let timer = shared.clone();
    std::thread::spawn(move || {
        let mut onward = timer.onward.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            onward = match onward.gate.deadline() {
                Some(at) => {
                    let (mut onward, _) = timer.changed.wait_timeout(onward, at.saturating_duration_since(Instant::now())).unwrap_or_else(|e| e.into_inner());
                    let gone = onward.gate.tick(Instant::now());
                    timer.send(&mut onward, gone);
                    onward
                }
                None => timer.changed.wait(onward).unwrap_or_else(|e| e.into_inner()),
            };
        }
    });

    // From the server to the app, as it comes; an answer lets the next
    // typed request through.
    let mut scanner = Scanner::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut stdout = std::io::stdout().lock();
    loop {
        let n = match from_server.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut pieces = scanner.push(&buf[..n]);
        // (An upgrade refused: what follows is a page, not a connection.
        // It passes as it comes, the app isn't told a helper is there,
        // and nothing is held for answers that can't be read.)
        let refused = pieces.iter().any(|piece| matches!(piece, Piece::Head(head) if !upgraded(head)));
        if refused {
            pieces.extend(scanner.plain());
        }
        for piece in &pieces {
            let sent = match piece {
                Piece::Head(head) => stdout.write_all(&announced(head)),
                other => stdout.write_all(other.bytes()),
            };
            if sent.is_err() {
                std::process::exit(0);
            }
        }
        if stdout.flush().is_err() {
            std::process::exit(0);
        }
        let mut onward = shared.onward.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if refused {
            let gone = onward.gate.open(now);
            shared.send(&mut onward, gone);
        }
        for piece in &pieces {
            let gone = onward.gate.from_server(piece, now);
            shared.send(&mut onward, gone);
        }
        drop(onward);
        shared.changed.notify_all();
    }
    // (The server's last bytes, a piece cut off among them.)
    if let Some(piece) = scanner.rest() {
        let _ = stdout.write_all(piece.bytes());
        let _ = stdout.flush();
    }
    let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(0);
    std::process::exit(status);
}
