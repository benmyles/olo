//! A stand-in for Passwords at a test's own keychain socket, for tests
//! of what *asks* Passwords (Rex carrying a host's SSH agent here): it
//! answers the SSH agent's two requests for a host with one made-up key
//! and no signature, and writes who asked into a file, one line each.
//! Never the person's Passwords: it wants `OLO_KEYCHAIN_SOCKET`.
//!
//! ```sh
//! OLO_KEYCHAIN_SOCKET=$sock target/debug/examples/olo-keychain-stand-in ASKED.txt
//! ```

use std::io::Write as _;

use base64::Engine as _;
use olo_keychain::{Request, Response, SshKey};

fn main() {
    if std::env::var_os("OLO_KEYCHAIN_SOCKET").is_none() {
        eprintln!("olo-keychain-stand-in: set OLO_KEYCHAIN_SOCKET to a test's own socket (never the person's)");
        std::process::exit(2);
    }
    let Some(asked) = std::env::args().nth(1) else {
        eprintln!("usage: olo-keychain-stand-in FILE-TO-WRITE-WHO-ASKED-INTO");
        std::process::exit(2);
    };
    // (An ed25519 public key's blob: its kind, and 32 bytes.)
    let mut blob = Vec::new();
    for part in [&b"ssh-ed25519"[..], &[7u8; 32][..]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    let key = base64::engine::general_purpose::STANDARD.encode(blob);
    let note = move |line: String| {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&asked) {
            let _ = writeln!(file, "{line}");
        }
    };
    let served = olo_keychain::serve(move |incoming| {
        let response = match &incoming.request {
            Request::SshKeysFor { origin } => {
                note(format!("keys for {} at {} (asked by {})", origin.host, origin.address, incoming.from));
                Response::SshKeys { keys: vec![SshKey { key: key.clone(), comment: "olo-test-key".into() }] }
            }
            Request::SshSignFor { origin, .. } => {
                note(format!("a signature for {} at {} (asked by {})", origin.host, origin.address, incoming.from));
                Response::Error { message: "the stand-in signs nothing".into() }
            }
            other => {
                note(format!("something else: {}", serde_json::to_string(other).unwrap_or_default().chars().take(60).collect::<String>()));
                Response::Locked
            }
        };
        incoming.reply(response);
    });
    if let Err(e) = served {
        eprintln!("olo-keychain-stand-in: {e}");
        std::process::exit(1);
    }
    loop {
        std::thread::park();
    }
}
