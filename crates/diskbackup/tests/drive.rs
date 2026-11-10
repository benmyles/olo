//! Google Drive as a store, against a Google of our own (a few hundred
//! lines of HTTP that do what Drive's API does, with its awkward parts: a
//! 429 now and then, a token that runs out, listings two files a page, a
//! resumable upload's first piece cut off part way). A sign-in through
//! the loopback address; then a whole repository: backed up, a big file
//! resumably, restored, tidied.

use std::{
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
};

use olo_diskbackup::{
    Key, Options, Progress, Repo, Store, backup,
    drive::Drive,
    exclude::Excludes,
    oauth::{Client, Endpoints, SignIn},
    prune,
    restore::restore,
};

#[path = "support/google.rs"]
mod google;
use google::{Google, serve, unescape};

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

#[test]
fn sign_in_then_back_up_restore_and_tidy() {
    let google = Arc::new(Mutex::new(Google { slow_down: 1, expire: 1, cut: 1, ..Google::default() }));
    let api = serve(google.clone());
    let endpoints = Endpoints { auth: format!("{api}/auth"), token: format!("{api}/token"), api: api.clone() };
    let client = Client { id: "123.apps.googleusercontent.com".into(), secret: "GOCSPX-secret".into() };

    // Signing in: the browser sent back to our loopback address.
    let sign_in = SignIn::start(&client, &endpoints).unwrap();
    let url = sign_in.url().to_owned();
    assert!(url.contains("code_challenge_method=S256") && url.contains("drive.file") && url.contains("access_type=offline"), "{url}");
    let state = url.split("state=").nth(1).unwrap().split('&').next().unwrap().to_owned();
    let redirect = unescape(url.split("redirect_uri=").nth(1).unwrap().split('&').next().unwrap());
    let browser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut s = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
        s.write_all(format!("GET /?state={state}&code=the-code&scope=x HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
        let mut page = String::new();
        s.read_to_string(&mut page).unwrap();
        page
    });
    let http = olo_diskbackup::oauth::agent();
    let signed = sign_in.wait(&http, &AtomicBool::new(false)).unwrap();
    assert!(browser.join().unwrap().contains("Signed in"));
    assert_eq!((signed.refresh_token.as_str(), signed.email.as_str()), ("the-refresh", "me@example.com"));

    // The folder, found or made (once).
    let folder = Drive::folder(&client, &signed.refresh_token, &endpoints).unwrap();
    assert_eq!(Drive::folder(&client, &signed.refresh_token, &endpoints).unwrap(), folder);

    let limits = (Arc::new(olo_diskbackup::Limit::default()), Arc::new(olo_diskbackup::Limit::default()));
    let drive = Drive::new(client.clone(), signed.refresh_token.clone(), signed.email.clone(), folder.clone(), limits.0.clone(), limits.1.clone(), endpoints.clone());
    assert_eq!(drive.describe(), "Google Drive (me@example.com)");
    // Whose Drive, and its space (Destination's details).
    let about = drive.about().unwrap();
    assert_eq!((about.email.as_str(), about.limit), ("me@example.com", Some(16_106_127_360)));
    let store: Arc<dyn Store> = Arc::new(drive);

    // The store's own: put, replace, get, a range, list by folder, delete.
    store.put("index/a", b"one").unwrap();
    store.put("index/a", b"two").unwrap();
    store.put("index/b", b"three").unwrap();
    store.put("snapshots/c", b"four").unwrap();
    assert_eq!(store.get("index/a").unwrap(), b"two");
    assert_eq!(store.get_range("index/b", 1, 3).unwrap(), b"hre");
    assert_eq!(store.list("index").unwrap(), [("index/a".to_owned(), 3), ("index/b".to_owned(), 5)]);
    assert_eq!(store.list("").unwrap().len(), 3, "every page of them");
    store.delete("index/a").unwrap();
    store.delete("index/a").unwrap();
    assert_eq!(store.get("index/a").unwrap_err().kind(), std::io::ErrorKind::NotFound);
    store.delete("index/b").unwrap();
    store.delete("snapshots/c").unwrap();

    // A whole repository on it.
    let base = PathBuf::from(format!("/var/tmp/olo-diskbackup-drive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/note.txt"), "a note").unwrap();
    std::fs::write(src.join("big.bin"), noise(20 << 20, 3)).unwrap();
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let lock = repo.lock(false).unwrap();
    let (_, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    drop(lock);
    // Checked fast: Google's checksums show every pack arrived as sent;
    // one changed in Drive since is found by its checksum, fetched, and
    // found damaged.
    let c = olo_diskbackup::check(&repo, &s1, false, &cancel, &progress).unwrap();
    assert!(c.problems.is_empty() && c.by_checksum >= 2 && c.sampled == 1, "{c:?}");
    let (hurt, was) = {
        let mut g = google.lock().unwrap();
        let (id, f) = g.files.iter_mut().filter(|(_, f)| f.0.starts_with("data/")).max_by_key(|(_, f)| f.2.len()).unwrap();
        let was = f.2.clone();
        f.2[100] ^= 0x55;
        (id.clone(), was)
    };
    let c = olo_diskbackup::check(&repo, &s1, false, &cancel, &progress).unwrap();
    assert_eq!(c.bad_packs.len(), 1, "{c:?}");
    assert!(c.problems.iter().any(|p| p.contains("damaged")), "{c:?}");
    google.lock().unwrap().files.get_mut(&hurt).unwrap().2 = was;
    // Opened again (another computer, say): restored whole.
    let repo2 = Repo::open(store.clone(), key.clone(), Some(base.join("cache2"))).unwrap();
    let out = base.join("out");
    restore(&repo2, &s1.tree, &src, &out, &cancel, &progress).unwrap();
    assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), noise(20 << 20, 3));
    assert_eq!(std::fs::read_to_string(out.join("docs/note.txt")).unwrap(), "a note");
    // Gone from the files, and tidied away.
    std::fs::remove_file(src.join("big.bin")).unwrap();
    let lock = repo.lock(false).unwrap();
    let parent = repo.snapshots().unwrap().pop().unwrap();
    let (id2, _) = backup(&mut repo, &opts, Some((parent.0, &parent.1)), &cancel, &progress).unwrap();
    drop(lock);
    for (id, _) in repo.snapshots().unwrap() {
        if id != id2 {
            repo.forget(&id).unwrap();
        }
    }
    let before: usize = google.lock().unwrap().files.values().map(|f| f.2.len()).sum();
    let lock = repo.lock(true).unwrap();
    let pruned = prune(&mut repo, &cancel, &progress).unwrap();
    drop(lock);
    let after: usize = google.lock().unwrap().files.values().map(|f| f.2.len()).sum();
    assert!(after + (19 << 20) < before, "{pruned:?}: {before} → {after}");

    // Erased: the folder deleted whole (one request), found no more, and
    // made again, empty, by its name.
    let requests = google.lock().unwrap().requests;
    let drive = Drive::new(client.clone(), signed.refresh_token.clone(), signed.email.clone(), folder.clone(), limits.0.clone(), limits.1.clone(), endpoints.clone());
    drive.delete_folder().unwrap();
    assert!(google.lock().unwrap().requests - requests <= 2, "one request (a token at most besides)");
    assert!(google.lock().unwrap().files.values().all(|f| f.1 != folder), "nothing left in it");
    assert_eq!(Drive::find_folder(&client, &signed.refresh_token, &endpoints).unwrap(), None);
    let again = Drive::folder(&client, &signed.refresh_token, &endpoints).unwrap();
    assert_ne!(again, folder);
    assert_eq!(Drive::find_folder(&client, &signed.refresh_token, &endpoints).unwrap(), Some(again));

    let g = google.lock().unwrap();
    assert_eq!((g.slow_down, g.expire, g.cut, g.misplaced), (0, 0, 0, 0), "every fault met, every piece where it belongs");
    eprintln!("{} requests, {} token fetches, {} chunked bodies", g.requests, g.tokens, g.chunked);
    drop(g);
    let _ = std::fs::remove_dir_all(base);
    let _ = AtomicUsize::new(0).load(Relaxed);
}
