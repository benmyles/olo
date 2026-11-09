//! A backup to Google Drive while the network changes under it (Ben,
//! 2026-11-09: "make sure that if the active network changes mid backup
//! that diskbackup will seamlessly start using the new network and not
//! hang or error"), against the Google of our own: its connections cut
//! part way through a piece again and again, more often than a request is
//! tried, and the backup finishes, each piece going on from where Google
//! got to; then no network for a while: it waits (and says so), and goes
//! on when one comes back; and Stop ends a wait for the network at once.
//! (The changes are `olo_net::pretend_change`, which does what a real one
//! does: olo-net's own test makes real ones, in a network of its own.)

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use olo_diskbackup::{
    Key, Limit, Options, Progress, Repo, Store, backup,
    drive::Drive,
    exclude::Excludes,
    oauth::{Client, Endpoints},
    restore::restore,
};

#[path = "support/google.rs"]
mod google;
use google::{Google, serve};

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "{what}: not in {secs} s");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_backup_goes_on_across_network_changes() {
    let google = Arc::new(Mutex::new(Google { latency: Duration::from_millis(20), ..Google::default() }));
    let api = serve(google.clone());
    let port: u16 = api.rsplit(':').next().unwrap().parse().unwrap();
    let endpoints = Endpoints { auth: format!("{api}/auth"), token: format!("{api}/token"), api: api.clone() };
    let client = Client { id: "123.apps.googleusercontent.com".into(), secret: "GOCSPX-secret".into() };
    let folder = Drive::folder(&client, "the-refresh", &endpoints).unwrap();
    // 16 MB/s up: a piece (8 MiB) takes half a second.
    let up = Arc::new(Limit::new(16 << 20));
    let stop = Arc::new(AtomicBool::new(false));
    let drive = Drive::new(client.clone(), "the-refresh".into(), "me@example.com".into(), folder.clone(), up.clone(), Arc::default(), endpoints.clone()).stopped_by(Some(stop.clone()));
    let store: Arc<dyn Store> = Arc::new(drive);

    let base = PathBuf::from(format!("/var/tmp/olo-diskbackup-network-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/note.txt"), "a note").unwrap();
    let big = noise(40 << 20, 5);
    std::fs::write(src.join("big.bin"), &big).unwrap();
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let sent_before = up.passed();

    let (snapshot, cuts) = std::thread::scope(|s| {
        let backing_up = s.spawn(|| {
            let lock = repo.lock(false).unwrap();
            let r = backup(&mut repo, &opts, None, &cancel, &progress);
            drop(lock);
            r
        });
        // A piece of the big pack in Google, then the network changes
        // twelve times in a second (more than the eight tries a request
        // has), each cutting what's under way.
        until("a piece sent", 30, || google.lock().unwrap().sessions.values().any(|s| s.3.len() >= 8 << 20));
        let mut cuts = 0;
        for _ in 0..12 {
            cuts += olo_net::pretend_change(true, Some(port));
            std::thread::sleep(Duration::from_millis(80));
        }
        // No network: it waits, and says so.
        olo_net::pretend_change(false, Some(port));
        until("waiting for the network", 10, || olo_net::waiting() > 0);
        std::thread::sleep(Duration::from_millis(500));
        assert!(!backing_up.is_finished(), "it waits for the network, not fails");
        // Back: on with it.
        let back = Instant::now();
        olo_net::pretend_change(true, None);
        until("not waiting", 2, || olo_net::waiting() == 0);
        println!("went on {:?} after the network came back", back.elapsed());
        let (_, snapshot) = backing_up.join().unwrap().expect("the backup finished");
        (snapshot, cuts)
    });
    let sent = up.passed() - sent_before;
    println!("{cuts} connections cut; {} MB sent for {} MB", sent >> 20, big.len() >> 20);
    assert!(cuts >= 6, "connections were cut under way: {cuts}");
    let g = google.lock().unwrap();
    assert_eq!(g.misplaced, 0, "every piece went on from where Google got to");
    drop(g);
    // (What a cut piece had sent goes again, nothing before it.)
    assert!(sent < big.len() as u64 + (12 + 2) * (8 << 20), "{sent}");

    // All there, restored from another copy of the repository.
    let repo2 = Repo::open(store.clone(), key.clone(), Some(base.join("cache2"))).unwrap();
    let out = base.join("out");
    restore(&repo2, &snapshot.tree, &src, &out, &cancel, &progress).unwrap();
    assert!(std::fs::read(out.join("big.bin")).unwrap() == big);
    assert_eq!(std::fs::read_to_string(out.join("docs/note.txt")).unwrap(), "a note");

    // Stop ends a wait for the network at once.
    olo_net::pretend_change(false, None);
    let started = Instant::now();
    let putting = std::thread::spawn({
        let store = store.clone();
        move || store.put("index/stopped", b"x")
    });
    until("waiting for the network", 5, || olo_net::waiting() > 0);
    stop.store(true, Relaxed);
    olo_net::nudge();
    let r = putting.join().unwrap();
    assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    olo_net::pretend_change(true, None);
    let _ = std::fs::remove_dir_all(&base);
}
