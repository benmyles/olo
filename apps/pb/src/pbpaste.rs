//! `pbpaste`, as macOS's: the clipboard's text on its output (nothing,
//! if what's copied isn't text). The session puts it on PATH as
//! `pbpaste`. macOS's `-pboard` and `-Prefer` are taken and ignored.

use std::io::Write as _;

/// Text's types, the best first (as Olo's apps copy it).
const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-help" || a == "--help") {
        println!("usage: pbpaste [-pboard {{general | ruler | find | font}}] [-Prefer {{txt | rtf | ps}}]\n  Writes the clipboard's text to its output.");
        return;
    }
    match olo_kit_base::clipboard::read(&TEXT) {
        Ok(Some(data)) => {
            let mut out = std::io::stdout().lock();
            // (A pipe closed early, `pbpaste | head`, is no error.)
            let _ = out.write_all(&data).and_then(|()| out.flush());
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("pbpaste: {e}");
            std::process::exit(1);
        }
    }
}
