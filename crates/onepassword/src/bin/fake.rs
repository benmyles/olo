//! `onepassword-fake [--account NAME] [--socket NAME] [--data FILE]`: a
//! stand-in 1Password app for headless sessions (see `fake`). It starts
//! with sample data, or FILE's (which it keeps up to date).

fn main() {
    let mut account = "Tabula Test".to_string();
    let mut socket = onepassword::desktop::SOCKET.to_string();
    let mut data = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--account" => account = args.next().expect("--account NAME"),
            "--socket" => socket = args.next().expect("--socket NAME"),
            "--data" => data = Some(std::path::PathBuf::from(args.next().expect("--data FILE"))),
            _ => {
                eprintln!("usage: onepassword-fake [--account NAME] [--socket NAME] [--data FILE]");
                std::process::exit(2);
            }
        }
    }
    let contents = data
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| onepassword::fake::sample(&account));
    let _fake = onepassword::fake::Fake::start_saving(&socket, contents, data).unwrap_or_else(|e| {
        eprintln!("onepassword-fake: can't listen on @{socket}: {e}");
        std::process::exit(1);
    });
    eprintln!("onepassword-fake: account '{account}' on @{socket}");
    loop {
        std::thread::park();
    }
}
