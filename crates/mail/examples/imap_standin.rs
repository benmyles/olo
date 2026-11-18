//! A stand-in IMAP server by itself, for trying mailed codes and sign-in
//! links with no account of anyone's (the phone service's tests use the
//! same one in process):
//!
//!     cargo run --release -p olo-mail --example imap_standin < FIFO
//!
//! It prints its address (for `OLO_MAIL_IMAP`; any address and the
//! password "standin" sign in), then delivers, as a new message in the
//! inbox, each file whose path it reads on its input (a message as it
//! travels: headers, an empty line, the body), until its input ends.

use std::io::BufRead as _;

fn main() {
    let server = olo_mail::standin::Server::start(Vec::new());
    println!("{}", server.address());
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                server.deliver(&raw.replace("\r\n", "\n").replace('\n', "\r\n"));
                println!("delivered {path}");
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
