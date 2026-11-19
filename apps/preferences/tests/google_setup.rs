//! `olo-preferences --google-setup-done`, as the Google account set-up's
//! agent runs it once the account's app is made (`olo_google::setup`),
//! against a Google of our own (DiskBackup's stand-in): the consent
//! page's address said, the browser's answer taken at the loopback
//! address, the account listed with what it's used for (Mail and
//! Calendar; DiskBackup where it backs up to that account's Drive
//! already), what signs it in kept (a test's own keyring), and each
//! service tried and said. Then the safety check of bringing an older
//! DiskBackup app in: a client that doesn't see the backups DiskBackup
//! wrote is said plainly. All in a sandbox; no browser opened, no real
//! server asked.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
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
        .env("OLO_GOOGLE_ACCOUNTS", base.join("google/accounts.json"))
        .env("OLO_GOOGLE_SECRETS", base.join("google"))
        .env("OLO_GOOGLE_API", api)
        // (The consent page fetched here, not opened in a browser.)
        .env("OLO_GOOGLE_CONSENT", "self")
        // (Mail's and Calendar's servers: nowhere, said as that.)
        .env("OLO_MAIL_IMAP", "127.0.0.1:9")
        .env("OLO_CALENDAR_SERVER", "http://127.0.0.1:9/");
}

/// Run `args`, playing the browser: the consent page's address read from
/// what it says, the loopback address sent Google's answer. What it said
/// in all, and whether it ended well.
fn signed_in(base: &Path, api: &str, args: &[&str]) -> (String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_olo-preferences"));
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    sandboxed(&mut cmd, base, api);
    let mut child = cmd.spawn().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut said = String::new();
    let url = loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            return (format!("{said}{err}"), child.wait().unwrap().success());
        }
        said.push_str(&line);
        if let Some(u) = line.trim().strip_prefix(&format!("{api}/auth?")) {
            break u.to_owned();
        }
    };
    let param = |k: &str| google::unescape(url.split(&format!("{k}=")).nth(1).unwrap().split('&').next().unwrap());
    let (state, redirect, scope) = (param("state"), param("redirect_uri"), param("scope"));
    for wanted in ["https://mail.google.com/", "https://www.googleapis.com/auth/calendar", "https://www.googleapis.com/auth/drive.file", "email"] {
        assert!(scope.split(' ').any(|s| s == wanted), "{wanted} asked for: {scope}");
    }
    let mut s = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
    s.write_all(format!("GET /?state={state}&code=the-code HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).unwrap();
    assert!(page.contains("Signed in"), "{page}");
    out.read_to_string(&mut said).unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    (format!("{said}{err}"), child.wait().unwrap().success())
}

#[test]
fn an_account_connected_and_each_service_tried() {
    let base = std::path::PathBuf::from(format!("/var/tmp/olo-preferences-google-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["home/Downloads", "config", "cache", "state", "run", "google"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let g = Arc::new(Mutex::new(google::Google::default()));
    let api = google::serve(g.clone());
    // DiskBackup backs up to this account's Drive already (its folder
    // there, made by the same app).
    g.lock().unwrap().files.insert("backups".into(), ("Olo DiskBackup".into(), "root".into(), Vec::new()));
    std::fs::write(base.join("run/diskbackup-settings.json"), r#"{"target":{"kind":"google-drive","account":"me@example.com","folder":"backups"}}"#).unwrap();
    let file = base.join("home/Downloads/client_secret_123.json");
    std::fs::write(&file, r#"{"installed":{"client_id":"123.apps.googleusercontent.com","project_id":"olo","client_secret":"GOCSPX-secret","redirect_uris":["http://localhost"]}}"#).unwrap();

    let (said, ok) = signed_in(&base, &api, &["--google-setup-done", "~/Downloads/client_secret_123.json"]);
    assert!(ok, "{said}");
    for wanted in ["Connected me@example.com.", "Mail: ", "Calendar: ", "DiskBackup: its backups in Drive are seen by this app", "Preferences ▸ Integrations ▸ Google lists it"] {
        assert!(said.contains(wanted), "{wanted}\n{said}");
    }
    // Listed, used for Mail and Calendar, and DiskBackup's account already.
    let list: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base.join("google/accounts.json")).unwrap()).unwrap();
    assert_eq!(list[0]["address"], "me@example.com");
    assert_eq!(list[0]["services"], serde_json::json!(["mail", "calendar", "diskbackup"]));
    // What signs it in, kept (nothing of it in the list).
    let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base.join("google/me@example.com.json")).unwrap()).unwrap();
    assert_eq!((kept["client"]["id"].as_str(), kept["refresh_token"].as_str()), (Some("123.apps.googleusercontent.com"), Some("the-refresh")));
    assert!(!std::fs::read_to_string(base.join("google/accounts.json")).unwrap().contains("GOCSPX"));

    // Signed in again with the app it has (no file): the same account,
    // its ticks kept. DiskBackup's folder now another's (as another
    // project's app would find it: not there): said plainly.
    std::fs::write(base.join("run/diskbackup-settings.json"), r#"{"target":{"kind":"google-drive","account":"me@example.com","folder":"made-by-another-app"}}"#).unwrap();
    let (said, ok) = signed_in(&base, &api, &["--google-setup-done", "--account", "me@example.com"]);
    assert!(ok, "{said}");
    assert!(said.contains("this app doesn't see the backups DiskBackup wrote to Drive"), "{said}");
    let list: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base.join("google/accounts.json")).unwrap()).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["services"], serde_json::json!(["mail", "calendar", "diskbackup"]));

    // An account Olo knows no app for: said, nothing asked of Google.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_olo-preferences"));
    cmd.args(["--google-setup-done", "--account", "someone@example.org"]).stdout(Stdio::piped()).stderr(Stdio::piped());
    sandboxed(&mut cmd, &base, &api);
    let out = cmd.output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("no app is known for someone@example.org"), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(base);
}
