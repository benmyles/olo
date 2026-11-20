//! `olo_system::dnf` against `dnf-fake`, end to end. Needs a bus to meet
//! on: run with `OLO_TEST_BUS=unix:path=…` (a private bus: `olo-bus
//! unix:path=/var/tmp/…/bus.sock`), e.g.
//! `OLO_TEST_BUS=unix:path=/var/tmp/olo-dev/dnf/bus.sock cargo test -p olo-system --test dnf_fake`.
//! Without it the test does nothing.

use std::{
    process::{Child, Command},
    sync::Arc,
    time::{Duration, Instant},
};

use olo_system::dnf::{self, Event, Open, Session};

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn open() -> Session {
    Session::open(&Open::default()).expect("a session")
}

#[test]
fn against_the_fake() {
    let Ok(address) = std::env::var("OLO_TEST_BUS") else { return };
    // SAFETY: set before any other thread reads the environment.
    unsafe { std::env::set_var("OLO_DNF_BUS", &address) };
    let _fake = Fake(Command::new(env!("CARGO_BIN_EXE_dnf-fake")).args(["--address", &address]).spawn().expect("dnf-fake"));
    let bus = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus).unwrap();
    let start = Instant::now();
    while !dbus.name_has_owner(dnf::DEST.try_into().unwrap()).unwrap_or(false) {
        assert!(start.elapsed() < Duration::from_secs(10), "dnf-fake didn't start");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!dnf::real() && dnf::may_change());

    // What it reads.
    let s = open();
    let u = s.updates().unwrap();
    assert_eq!(u.updates.len(), 21, "{:?}", u.updates.iter().map(|x| &x.new.name).collect::<Vec<_>>());
    let security = u.updates.iter().filter(|x| x.kind(&u.advisories) == "security").count();
    assert_eq!(security, 9);
    let firefox = u.updates.iter().find(|x| x.new.name == "firefox").unwrap();
    assert_eq!((firefox.installed.evr.as_str(), firefox.new.evr.as_str()), ("146.0-1.fc45", "146.0.2-1.fc45"));
    let a = &u.advisories[&firefox.advisories[0]];
    assert_eq!((a.kind.as_str(), a.severity.as_str()), ("security", "Critical"));
    assert_eq!(a.references.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["CVE-2026-41020", "CVE-2026-41021"]);
    let kernel = u.updates.iter().find(|x| x.new.name == "kernel").unwrap();
    assert_eq!(kernel.installed.evr, "6.19.4-200.fc45", "the newest installed kernel is what's updated");
    let d = s.details(&firefox.new.nevra, "upgrades").unwrap();
    assert_eq!(d.changelog.len(), 3);
    assert!(d.changelog[0].text.contains("Update to 146.0.2") && d.changelog[0].when > d.changelog[2].when, "newest first");
    let repos = s.repos().unwrap();
    assert_eq!((repos.len(), repos.iter().filter(|r| r.enabled).count()), (8, 7));
    assert!(repos[0].enabled && !repos.last().unwrap().enabled, "on ones first");
    let h = s.history(10).unwrap();
    assert_eq!(h.iter().map(|t| t.id).collect::<Vec<_>>(), [4, 3, 2, 1]);
    assert_eq!(h[0].upgraded[0].original_evr, "2.8.3-1.fc45");
    assert!(!s.offline().unwrap().pending);

    // A transaction resolved, then run, heard as it goes.
    s.upgrade(&["firefox".into()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(r.ok(), "{:?}", r.problems);
    let did: Vec<(String, String)> = r.items.iter().map(|i| (i.action.clone(), i.package.name.clone())).collect();
    for want in [("Upgrade", "firefox"), ("Replaced", "firefox"), ("Upgrade", "nss"), ("Install", "firefox-langpacks")] {
        assert!(did.contains(&(want.0.into(), want.1.into())), "{did:?}");
    }
    let (download, _) = r.sizes();
    assert!(download > 100_000_000);
    let mut heard = Vec::new();
    s.hearing(&mut |e| heard.push(e), |s| s.run(false, "Updated Firefox")).unwrap();
    assert!(heard.iter().any(|e| matches!(e, Event::DownloadStart { .. })));
    assert!(heard.iter().any(|e| matches!(e, Event::Action { nevra, .. } if nevra.starts_with("firefox-146.0.2"))));
    assert!(matches!(heard.last(), Some(Event::Finished { ok: true })), "{:?}", heard.last());
    drop(s);
    let s = open();
    let u = s.updates().unwrap();
    assert_eq!(u.updates.len(), 19, "firefox and nss done");
    assert_eq!(s.history(1).unwrap()[0].comment, "Updated Firefox");

    // Cancelled while it downloads: nothing done.
    s.reset().unwrap();
    s.upgrade(&["rust".into()]).unwrap();
    assert!(s.resolve(false).unwrap().ok());
    let s = Arc::new(s);
    let canceller = s.clone();
    let mut cancelled = false;
    let result = s.hearing(
        &mut |e| {
            if matches!(e, Event::Download { .. }) && !cancelled {
                cancelled = true;
                canceller.cancel().unwrap();
            }
        },
        |s| s.run(false, ""),
    );
    assert!(result.is_err() && cancelled, "{result:?}");
    assert!(s.updates().unwrap().updates.iter().any(|x| x.new.name == "rust"), "still to do");

    // A third party's key, asked about and trusted.
    s.reset().unwrap();
    s.upgrade(&["olo-tools".into()]).unwrap();
    assert!(s.resolve(false).unwrap().ok());
    let asker = s.clone();
    let mut asked = None;
    s.hearing(
        &mut |e| {
            if let Event::Key(k) = e {
                asker.confirm_key(&k.key_id, true).unwrap();
                asked = Some(k);
            }
        },
        |s| s.run(false, ""),
    )
    .unwrap();
    assert!(asked.is_some_and(|k| k.url.contains("copr") && !k.fingerprint.is_empty()));

    // Put off until the computer starts again, then not.
    s.reset().unwrap();
    s.upgrade(&["glibc".into()]).unwrap();
    assert!(s.resolve(false).unwrap().ok());
    s.run(true, "").unwrap();
    let off = s.offline().unwrap();
    assert!(off.pending && off.status.get("status").is_some_and(|v| v == "ready"), "{off:?}");
    s.offline_then("poweroff").unwrap();
    s.cancel_offline().unwrap();
    assert!(!s.offline().unwrap().pending);

    // A repository turned on.
    s.enable(&["updates-testing"], true).unwrap();
    assert!(s.repos().unwrap().iter().any(|r| r.id == "updates-testing" && r.enabled));
    s.enable(&["updates-testing"], false).unwrap();

    // Undoing history's upgrade: back to the version before.
    s.reset().unwrap();
    s.downgrade(&["expat-2.8.3-1.fc45".into()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(r.items.iter().any(|i| i.action == "Downgrade" && i.package.evr == "2.8.3-1.fc45"), "{:?}", r.items);

    // A package moving architecture (noarch to x86_64; Ben's, 2026-11-15):
    // asked for by its new build's name.arch, dnf refuses it, and with it
    // everything asked for together; asked for as installed
    // (`Update::upgrade_spec`), it's an update like any.
    let u = s.updates().unwrap();
    let w = u.updates.iter().find(|x| x.new.name == "python3-wcwidth").expect("offered");
    let other = u.updates.iter().find(|x| x.new.name == "rust").expect("rust");
    assert_eq!((w.installed.arch.as_str(), w.new.arch.as_str()), ("noarch", "x86_64"));
    s.reset().unwrap();
    s.upgrade(&[w.new.key(), other.new.key()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(!r.ok() && r.items.is_empty() && r.problems.iter().any(|p| p.contains("different architecture")), "{:?}", r.problems);
    assert_eq!(dnf::refused_arguments(&r.problems).first().map(|(a, _)| a.as_str()), Some("python3-wcwidth.x86_64"));
    s.reset().unwrap();
    s.upgrade(&[w.upgrade_spec(), other.upgrade_spec()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(r.ok(), "{:?}", r.problems);
    assert!(r.items.iter().any(|i| i.action == "Upgrade" && i.package.name == "python3-wcwidth" && i.package.arch == "x86_64"), "{:?}", r.items);
    assert!(r.items.iter().any(|i| i.action == "Replaced" && i.package.name == "python3-wcwidth" && i.package.arch == "noarch"));
    assert!(r.items.iter().any(|i| i.action == "Upgrade" && i.package.name == "rust"));

    // Nothing by that name.
    s.reset().unwrap();
    s.install(&["no-such-package".into()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(!r.ok() && r.problems.iter().any(|p| p.contains("no-such-package")));

    // RPM Fusion, by its release package's address.
    s.reset().unwrap();
    s.install(&["https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-45.noarch.rpm".into()]).unwrap();
    let r = s.resolve(false).unwrap();
    assert!(r.items.iter().any(|i| i.package.name == "rpmfusion-free-release"), "{:?}", r.items);
    s.run(false, "").unwrap();
    assert!(s.repos().unwrap().iter().any(|r| r.id == "rpmfusion-free-updates" && r.enabled));

    // Checking again finds what came out since.
    s.expire_cache().unwrap();
    let mut repos_heard = 0;
    assert!(s.hearing(&mut |e| repos_heard += matches!(e, Event::DownloadEnd { ok: true, .. }) as usize, |s| s.read_repos()).unwrap());
    assert!(repos_heard >= 7);
    drop(s);
    let s = open();
    assert!(s.updates().unwrap().updates.iter().any(|x| x.new.name == "bash"));

    // The next release.
    let next = Session::open(&Open { release: Some("46".into()), ..Default::default() }).unwrap();
    next.system_upgrade().unwrap();
    let r = next.resolve(false).unwrap();
    assert!(r.ok() && r.items.iter().any(|i| i.package.release.ends_with("fc46")));
    let none = Session::open(&Open { release: Some("47".into()), ..Default::default() }).unwrap();
    let e = none.read_repos().unwrap_err();
    assert!(dnf::error_text(&e).contains("404"), "{e}");
}
