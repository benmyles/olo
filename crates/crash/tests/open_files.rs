//! A program held to a login's 1,024 open files raises itself to what
//! its account may have (`olo_crash::raise_open_files`, which `install`
//! does first). In a process of its own: the limit is the process's.

#[test]
fn the_limit_is_raised_to_the_most_allowed() {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes `limit`.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
    let hard = limit.rlim_max;
    if hard == libc::RLIM_INFINITY || hard <= 256 {
        eprintln!("skipped: this account's hard limit is {hard}");
        return;
    }
    // As a login gives it.
    limit.rlim_cur = 256;
    // SAFETY: setrlimit reads `limit`; lowering a soft limit needs no privilege.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    assert_eq!(olo_crash::raise_open_files(), Some((256, hard)));
    // SAFETY: getrlimit writes `limit`.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
    assert_eq!((limit.rlim_cur, limit.rlim_max), (hard, hard));
    // Nothing to do the second time; and `install` does it too.
    assert_eq!(olo_crash::raise_open_files(), Some((hard, hard)));
    limit.rlim_cur = 300;
    // SAFETY: as above.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    olo_crash::install();
    // SAFETY: getrlimit writes `limit`.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
    assert_eq!(limit.rlim_cur, hard);
}
