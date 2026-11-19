//! A watch refused because the kernel has no inotify instance to give
//! (the account's all in use: the kernel's 128 were, 2026-11-18) isn't
//! refused for good: the next one, once there's room, follows its files.
//! (A test of its own: it holds this program's open files down, which
//! would trouble any other test running beside it.)

use std::{path::PathBuf, sync::mpsc, time::Duration};

use olo_kit_base::watch::watch;

#[test]
fn a_refusal_is_not_kept() {
    let dir = PathBuf::from(format!("/var/tmp/olo-kit-watch-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("state.json");

    // This program may open no more files: the kernel refuses an inotify
    // instance as it does when the account's are all in use (EMFILE). A
    // new descriptor takes the lowest number free, so the limit goes there.
    let open: Vec<i32> = std::fs::read_dir("/proc/self/fd").unwrap().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    let lowest_free = (0..).find(|n| !open.contains(n) || std::fs::read_link(format!("/proc/self/fd/{n}")).is_err()).unwrap();
    let mut was = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut was) }, 0);
    let held = libc::rlimit { rlim_cur: lowest_free as libc::rlim_t, rlim_max: was.rlim_max };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &held) }, 0);
    let refused = watch([file.clone()], |_| {});
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &was) }, 0);
    assert!(refused.is_none(), "with no file to be opened, no watch");

    // Room again: the next watch follows its file.
    let (tx, rx) = mpsc::channel();
    let _w = watch([file.clone()], move |p| tx.send(p.to_vec()).unwrap()).expect("tried again, and given");
    std::fs::write(&file, "1").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).expect("the file followed"), [file.clone()]);
    let _ = std::fs::remove_dir_all(&dir);
}
