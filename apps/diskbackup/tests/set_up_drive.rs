//! `olo-diskbackup --set-up-drive ID SECRET` against a Google of our own:
//! the consent page's address said, the browser's answer taken at the
//! loopback address, the folder made, a backup made there; then the
//! service backs up to it. All in a sandbox (its own HOME, settings, state,
//! key and sign-in: `OLO_DISKBACKUP_*`), no browser opened.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[path = "../../../crates/diskbackup/tests/support/google.rs"]
mod google;

fn sandboxed(cmd: &mut Command, base: &Path, api: &str) {
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", base.join("home"))
        .env("XDG_CONFIG_HOME", base.join("config"))
        .env("XDG_CACHE_HOME", base.join("cache"))
        .env("XDG_STATE_HOME", base.join("state"))
        .env("XDG_RUNTIME_DIR", base.join("run"))
        .env("OLO_DISKBACKUP_DIR", base.join("run"))
        .env("OLO_DISKBACKUP_KEY", "11".repeat(32))
        .env("OLO_DISKBACKUP_GOOGLE_API", api)
        .env("OLO_DISKBACKUP_GOOGLE", r#"{"client":{"id":"123.apps.googleusercontent.com","secret":"GOCSPX-secret"},"refresh_token":"the-refresh"}"#);
}

#[test]
fn set_up_then_back_up() {
    let base = std::path::PathBuf::from(format!("/var/tmp/olo-diskbackup-setup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["home/docs", "config", "cache", "state", "run"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    std::fs::write(base.join("home/docs/note.txt"), "to Drive").unwrap();
    let g = Arc::new(Mutex::new(google::Google::default()));
    let api = google::serve(g.clone());
    let exe = env!("CARGO_BIN_EXE_olo-diskbackup");

    let mut cmd = Command::new(exe);
    cmd.args(["--set-up-drive", "123.apps.googleusercontent.com", "GOCSPX-secret"]).stdout(Stdio::piped()).stderr(Stdio::null());
    sandboxed(&mut cmd, &base, &api);
    let mut child = cmd.spawn().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    // The consent page's address, said.
    let url = loop {
        let mut line = String::new();
        assert!(out.read_line(&mut line).unwrap() > 0, "no address said");
        if let Some(u) = line.trim().strip_prefix(&format!("{api}/auth?")) {
            break u.to_owned();
        }
    };
    let param = |k: &str| google::unescape(url.split(&format!("{k}=")).nth(1).unwrap().split('&').next().unwrap());
    let (state, redirect) = (param("state"), param("redirect_uri"));
    // The browser, sent back.
    let mut s = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
    s.write_all(format!("GET /?state={state}&code=the-code HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).unwrap();
    assert!(page.contains("Signed in"), "{page}");
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    assert!(child.wait().unwrap().success(), "{rest}");
    assert!(rest.contains("Backing up to Google Drive (me@example.com)"), "{rest}");
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base.join("run/diskbackup-settings.json")).unwrap()).unwrap();
    assert_eq!(settings["target"]["kind"], "google-drive");
    assert_eq!(settings["target"]["account"], "me@example.com");
    assert!(g.lock().unwrap().files.values().any(|f| f.0 == "config"), "the backup made in Drive");

    // The service backs up to it.
    let mut cmd = Command::new(exe);
    cmd.arg("--service").stdout(Stdio::null()).stderr(Stdio::null());
    sandboxed(&mut cmd, &base, &api);
    let mut service = cmd.spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    let state = loop {
        let st: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base.join("run/olo-diskbackup.json")).unwrap_or_default()).unwrap_or_default();
        if st["last_ok"].is_number() || Instant::now() > until {
            break st;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = service.kill();
    let _ = service.wait();
    assert!(state["last_ok"].is_number(), "{state}");
    assert!(state["target"].as_str().unwrap_or("").contains("Google Drive"), "{state}");
    assert!(g.lock().unwrap().files.values().any(|f| f.0.starts_with("snapshots/")), "a snapshot in Drive");
    let _ = std::fs::remove_dir_all(base);
}

/// A Google of our own for trying the app's setup window by hand (headless):
/// served for `OLO_FAKE_GOOGLE_SECONDS` (120), its address written to
/// `OLO_FAKE_GOOGLE_AT`, each request answered after
/// `OLO_FAKE_GOOGLE_LATENCY_MS`.
#[test]
#[ignore]
fn serve_google() {
    // (`OLO_FAKE_GOOGLE_LATENCY_MS`: each request that slow, as Google's.)
    let latency = std::env::var("OLO_FAKE_GOOGLE_LATENCY_MS").ok().and_then(|m| m.parse().ok()).map(Duration::from_millis).unwrap_or_default();
    let api = google::serve(Arc::new(Mutex::new(google::Google { latency, ..Default::default() })));
    std::fs::write(std::env::var("OLO_FAKE_GOOGLE_AT").unwrap_or_else(|_| "/var/tmp/olo-fake-google".into()), &api).unwrap();
    let secs = std::env::var("OLO_FAKE_GOOGLE_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    std::thread::sleep(Duration::from_secs(secs));
}

/// Backing up to the Drive of a Google account connected to Olo
/// (`--set-up-drive-account`, what the Google grid's DiskBackup tick and
/// the Back Up to Google Drive window do): signed in through the
/// account's own app (Olo's list of accounts and its sign-in, here a
/// test's own), the folder made, a backup made there, the account ticked
/// for DiskBackup; one that isn't connected refused, saying where to
/// connect it; then `--disconnect-drive`: no target, unticked, the
/// backups in Drive left as they are.
#[test]
fn set_up_drive_account_then_disconnect() {
    let base = std::path::PathBuf::from(format!("/var/tmp/olo-diskbackup-account-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["home/docs", "config", "cache", "state", "run", "google"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let list = base.join("google/accounts.json");
    std::fs::write(&list, r#"[{"address":"me@example.com","services":["mail"],"scopes":["https://mail.google.com/","https://www.googleapis.com/auth/drive.file"]}]"#).unwrap();
    std::fs::write(base.join("google/me@example.com.json"), r#"{"client":{"id":"123.apps.googleusercontent.com","secret":"GOCSPX-secret"},"refresh_token":"the-refresh"}"#).unwrap();
    let g = Arc::new(Mutex::new(google::Google::default()));
    let api = google::serve(g.clone());
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_olo-diskbackup"));
        cmd.args(args);
        sandboxed(&mut cmd, &base, &api);
        // (Olo's own list and sign-ins, not DiskBackup's from before.)
        cmd.env_remove("OLO_DISKBACKUP_GOOGLE").env("OLO_GOOGLE_ACCOUNTS", &list).env("OLO_GOOGLE_SECRETS", base.join("google"));
        cmd.output().unwrap()
    };
    let out = run(&["--set-up-drive-account", "ME@example.com"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}{}", String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("Backing up to Google Drive (me@example.com)"), "{said}");
    let settings = || -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(base.join("run/diskbackup-settings.json")).unwrap()).unwrap() };
    assert_eq!((settings()["target"]["kind"].as_str(), settings()["target"]["account"].as_str()), (Some("google-drive"), Some("me@example.com")));
    assert!(g.lock().unwrap().files.values().any(|f| f.0 == "config"), "the backup made in Drive");
    let services = || -> serde_json::Value { serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&list).unwrap()).unwrap()[0]["services"].clone() };
    assert_eq!(services(), serde_json::json!(["mail", "diskbackup"]));
    // One that isn't connected: refused, nothing changed.
    let out = run(&["--set-up-drive-account", "someone@example.org"]);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("isn't connected to Olo"), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(settings()["target"]["account"].as_str(), Some("me@example.com"));
    // Disconnected: no target, unticked; what's in Drive stays.
    let out = run(&["--disconnect-drive"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no longer backs up to me@example.com’s Google Drive"));
    assert_eq!(settings()["target"]["kind"].as_str(), Some("none"));
    assert_eq!(services(), serde_json::json!(["mail"]));
    assert!(g.lock().unwrap().files.values().any(|f| f.0 == "config"), "the backups there stay");
    let _ = std::fs::remove_dir_all(base);
}
