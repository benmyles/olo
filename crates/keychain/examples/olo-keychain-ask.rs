//! Asks a test's Passwords one thing over its keychain socket, as an Olo
//! program would (Chrome's extension, the palette), and says which answer
//! came: for tests of what Passwords shows when it's asked (a passkey's
//! question, the unlock). Only ever a test's own Passwords: it wants
//! `OLO_KEYCHAIN_SOCKET` (never the person's, the default), and prints
//! the answer's kind alone, nothing an answer holds.
//!
//! ```sh
//! OLO_KEYCHAIN_SOCKET=$sock target/debug/examples/olo-keychain-ask \
//!     '{"type":"passkey_sign","url":"https://github.com/login","rp_id":"github.com","allow":[],"chosen":null,"client_data_hash":""}'
//! ```

fn main() {
    if std::env::var_os("OLO_KEYCHAIN_SOCKET").is_none() {
        eprintln!("olo-keychain-ask: set OLO_KEYCHAIN_SOCKET to a test's own Passwords (never the person's)");
        std::process::exit(2);
    }
    let Some(json) = std::env::args().nth(1) else {
        eprintln!("usage: olo-keychain-ask REQUEST-AS-JSON");
        std::process::exit(2);
    };
    let request: olo_keychain::Request = match serde_json::from_str(&json) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("olo-keychain-ask: {e}");
            std::process::exit(2);
        }
    };
    match olo_keychain::call(&request) {
        Ok(response) => {
            // (Its kind alone: `{"type":"declined"}` says "declined".)
            let kind = serde_json::to_value(&response).ok().and_then(|v| match v {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Object(o) => o.get("type").and_then(|t| t.as_str().map(str::to_owned)).or_else(|| o.keys().next().cloned()),
                _ => None,
            });
            println!("{}", kind.unwrap_or_else(|| "?".into()));
        }
        Err(e) => {
            eprintln!("olo-keychain-ask: {e}");
            std::process::exit(1);
        }
    }
}
