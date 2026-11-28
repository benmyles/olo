//! Token Pool as a separate Antigravity sign-in's `xdg-open` (agy opens
//! its sign-in page with it: `olo_token_pool::import::prepare_sign_in`):
//! the built program, started by that name from the sign-in (agy's home
//! and XDG directories the sign-in's own), opens a web page as the person
//! opens one: in the browser their Default Apps choice names, found with
//! their own environment, which that browser is given (never the
//! sign-in's: a new, empty browser profile in it). Anything but a web
//! page, or a start that isn't a sign-in's, is refused, with nothing
//! opened. No window or pool starts. The browser is a stand-in that
//! writes down what it was given; every system directory is left out, so
//! nothing else could be chosen, and nothing here reaches the session
//! (no display, a runtime directory and ports of its own).

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{Duration, Instant},
};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the stand-in browser was given: its environment (`env -0`).
fn env_in(file: &Path) -> HashMap<String, String> {
    std::fs::read(file).unwrap().split(|b| *b == 0).filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).collect()
}

/// The person, a sign-in's directory with Token Pool as its `xdg-open`,
/// and a browser of the person's that writes down what it opened.
struct Fixture {
    _scratch: Scratch,
    root: PathBuf,
    person: PathBuf,
    signin: PathBuf,
    opened: PathBuf,
    carrier: String,
}

/// A directory of the test's own under /var/tmp: made new (never one
/// that was there, so never another's removed with it), named afresh
/// until one is.
fn scratch(name: &str) -> Scratch {
    use std::os::unix::fs::DirBuilderExt as _;
    for n in 0u32.. {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let root = PathBuf::from(format!("/var/tmp/olo-pool-opener-{name}-{}-{nanos}-{n}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => return Scratch(root),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", root.display()),
        }
    }
    unreachable!()
}

fn fixture(name: &str) -> Fixture {
    let scratch = scratch(name);
    let root = scratch.0.clone();
    let person = root.join("person");
    let apps = person.join(".local/share/applications");
    for d in [apps.as_path(), &person.join(".config/olo"), &person.join("bin"), &root.join("none"), &root.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    // Their browser (Default Apps' choice): a stand-in. (What it opened is
    // put in place whole, last: there once it's all written.)
    let opened = root.join("opened");
    let browser = root.join("browser");
    std::fs::write(&browser, format!("#!/bin/sh\n/usr/bin/env -0 > '{0}.env'\nprintf '%s' \"$1\" > '{0}.part' && /usr/bin/mv '{0}.part' '{0}'\n", opened.display())).unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        apps.join("olo-test-browser.desktop"),
        format!("[Desktop Entry]\nType=Application\nName=Test Browser\nExec={} %u\nMimeType=x-scheme-handler/http;x-scheme-handler/https;\n", browser.display()),
    )
    .unwrap();
    std::fs::write(person.join(".config/olo/default-apps.list"), "[Default Applications]\nx-scheme-handler/https=olo-test-browser.desktop;\nx-scheme-handler/http=olo-test-browser.desktop;\n")
        .unwrap();
    // The sign-in's directory, Token Pool its `xdg-open`.
    let signin = root.join("run/olo-pool-signin-test");
    std::fs::create_dir_all(signin.join("bin")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_olo-pool"), signin.join("bin/xdg-open")).unwrap();
    // What the sign-in's command gives it of the person's (as
    // `import::sign_in_command` makes it).
    let carrier = serde_json::json!({
        "PATH": person.join("bin"),
        "HOME": person,
        "XDG_CONFIG_HOME": person.join(".config"),
        "XDG_DATA_HOME": null,
        "XDG_CACHE_HOME": null,
        "XDG_STATE_HOME": person.join(".local/state"),
        "DBUS_SESSION_BUS_ADDRESS": format!("unix:path={}", root.join("person-bus").display()),
        "BROWSER": null,
    })
    .to_string();
    Fixture { _scratch: scratch, root, person, signin, opened, carrier }
}

impl Fixture {
    /// Token Pool started as agy starts its `xdg-open`: by that name, in
    /// the sign-in's environment (`carrier`: what it was given, if
    /// anything), with `args`: how it ended, and what it said.
    fn open(&self, carrier: Option<&str>, args: &[&str]) -> (ExitStatus, String) {
        self.open_os(carrier, &args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>())
    }

    /// [`Self::open`], its arguments as bytes (one needn't be text).
    fn open_os(&self, carrier: Option<&str>, args: &[std::ffi::OsString]) -> (ExitStatus, String) {
        let d = &self.signin;
        let mut c = Command::new(d.join("bin/xdg-open"));
        // (As `import::sign_in_command` gives it: the sign-in's home and
        // XDG directories, state's too, and its opener as `BROWSER`. All of
        // them are to be put back as they were, or taken away.)
        c.args(args)
            .env_clear()
            .env("PATH", format!("{}:{}", d.join("bin").display(), self.person.join("bin").display()))
            .env("HOME", d)
            .env("XDG_CONFIG_HOME", d.join(".config"))
            .env("XDG_DATA_HOME", d.join(".local/share"))
            .env("XDG_CACHE_HOME", d.join(".cache"))
            .env("XDG_STATE_HOME", d.join(".local/state"))
            .env("BROWSER", d.join("bin/xdg-open"))
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", d.join("no-dbus").display()))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            // (No system's apps or lists: only the person's could open it.)
            .env("XDG_DATA_DIRS", self.root.join("none"))
            .env("XDG_CONFIG_DIRS", self.root.join("none"))
            // (Were it ever the pool, never this computer's ports.)
            .env("OLO_POOL_PORT", "0")
            .env("OLO_POOL_PROXY_PORT", "0");
        if let Some(carrier) = carrier {
            c.env("OLO_POOL_OPENER_ENV", carrier);
        }
        let said = self.root.join("said");
        c.stdout(std::process::Stdio::null()).stderr(std::fs::File::create(&said).unwrap());
        let mut child = c.spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return (status, std::fs::read_to_string(&said).unwrap());
            }
            if Instant::now() > end {
                let _ = child.kill();
                panic!("Token Pool as xdg-open didn't end within 20 s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// What the person's browser opened, once it has (it's started on its
    /// own, apart from the opener).
    fn browser_opened(&self) -> (String, HashMap<String, String>) {
        let end = Instant::now() + Duration::from_secs(10);
        while !self.opened.exists() || !self.opened.with_extension("env").exists() {
            assert!(Instant::now() < end, "the person's browser never opened the page");
            std::thread::sleep(Duration::from_millis(20));
        }
        // (Written before the address: whole once that's there.)
        (std::fs::read_to_string(&self.opened).unwrap(), env_in(&self.opened.with_extension("env")))
    }
}

#[test]
fn the_page_opens_in_the_persons_browser_with_their_environment() {
    let f = fixture("page");
    let page = "https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";
    let (status, said) = f.open(Some(&f.carrier), &[page]);
    assert!(status.success(), "{said}");
    let (url, env) = f.browser_opened();
    assert_eq!(url, page);
    // The person's own, as they were: not the sign-in's, and nothing of
    // the opener's.
    assert_eq!(env["HOME"], f.person.display().to_string());
    assert_eq!(env["XDG_CONFIG_HOME"], f.person.join(".config").display().to_string());
    assert_eq!(env["XDG_STATE_HOME"], f.person.join(".local/state").display().to_string());
    assert_eq!(env["PATH"], f.person.join("bin").display().to_string());
    assert_eq!(env["DBUS_SESSION_BUS_ADDRESS"], format!("unix:path={}", f.root.join("person-bus").display()));
    for gone in ["XDG_DATA_HOME", "XDG_CACHE_HOME", "BROWSER", "OLO_POOL_OPENER_ENV"] {
        assert!(!env.contains_key(gone), "{gone} reached the browser");
    }
    assert!(!env.values().any(|v| v.contains("olo-pool-signin-test")), "the sign-in's directory reached the browser: {env:?}");
}

/// Refused, and said so, before anything is opened: the refusal is the
/// opener's first answer, ahead of any browser it could start (so that
/// it's said is what shows none was), and it names a page's scheme and
/// host at most.
#[test]
fn nothing_but_a_web_page_and_only_for_a_sign_in() {
    let f = fixture("refused");
    let refused = |carrier: Option<&str>, args: &[&str], why: &str| {
        let (status, said) = f.open(carrier, args);
        assert_eq!(status.code(), Some(1), "{args:?}: {said}");
        assert_eq!(said.trim_end(), format!("xdg-open (Token Pool’s): {why}"), "{args:?}");
    };
    refused(Some(&f.carrier), &["file:///etc/passwd"], "only a web page (http or https) is opened, not a file: address");
    refused(Some(&f.carrier), &["javascript:alert(1)"], "only a web page (http or https) is opened, not a javascript: address");
    refused(Some(&f.carrier), &["--version"], "only a web page (http or https) is opened, not something else");
    refused(Some(&f.carrier), &["https://?state=s3cret"], "only a web page (http or https) is opened, not something else");
    // Not a sign-in's (nothing of the person's to give back), or not
    // Token Pool's: refused, even a web page.
    refused(None, &["https://example.com/"], "only the page a sign-in in Token Pool asks for is opened");
    refused(Some(r#"{"PATH":"/usr/bin"}"#), &["https://example.com/"], "OLO_POOL_OPENER_ENV isn’t what Token Pool gave");
    // A web page beside something else: none of them.
    refused(Some(&f.carrier), &["https://example.com/", "file:///etc/passwd"], "only a web page (http or https) is opened, not a file: address");
    // Nothing the URL parser makes a host of (a path's first part, after
    // three slashes or a backslash) is said, nor opened.
    refused(Some(&f.carrier), &["https:///s3cret.example/o?state=s3cret"], "only a web page (http or https) is opened, not a https: address");
    refused(Some(&f.carrier), &["https://\\s3cret.example/o?state=s3cret"], "only a web page (http or https) is opened, not a https: address");
    // An argument that isn't text: nothing of it said (Astra's and Neon's
    // reviews, 2026-11-28: it was printed whole, its query and all).
    use std::os::unix::ffi::OsStringExt as _;
    let bytes = std::ffi::OsString::from_vec(b"https://accounts.google.com/o/oauth2/auth?state=s3cret&login_hint=sam\xff".to_vec());
    let (status, said) = f.open_os(Some(&f.carrier), std::slice::from_ref(&bytes));
    assert_eq!(status.code(), Some(1), "{said}");
    assert_eq!(said.trim_end(), "xdg-open (Token Pool’s): only a web page (http or https) is opened, not something that isn’t text");
    let (status, said) = f.open_os(None, &[bytes]);
    assert_eq!((status.code(), said.trim_end()), (Some(1), "xdg-open (Token Pool’s): only the page a sign-in in Token Pool asks for is opened"));
    assert!(!f.opened.with_extension("env").exists(), "something was opened");
}
