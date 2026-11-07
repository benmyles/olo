//! The module in a real PAM run (private services, as olo-login's
//! tests): a password asked and checked, a session opened, and the key
//! the password gives is in the session's keyring, readable by the
//! login's processes. (The test joins a new session keyring of its own
//! first, as pam_keyinit gives a login, which needs root.)

use std::path::PathBuf;

use olo_greet::{
    PromptKind,
    pam::{Answer, Pam},
};

fn me() -> (String, u32) {
    // SAFETY: getpwuid's static entry, copied at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned(), libc::getuid())
    }
}

/// The key named `name` in this process's session keyring.
fn session_key(name: &str) -> Option<Vec<u8>> {
    let desc = std::ffi::CString::new(name).ok()?;
    // SAFETY: keyctl(KEYCTL_SEARCH, session keyring) and KEYCTL_READ into a buffer.
    unsafe {
        let id = libc::syscall(libc::SYS_keyctl, 10 as libc::c_long, -3 as libc::c_long, c"user".as_ptr(), desc.as_ptr(), 0 as libc::c_long);
        if id <= 0 {
            return None;
        }
        let mut buf = vec![0u8; 64];
        let n = libc::syscall(libc::SYS_keyctl, 11 as libc::c_long, id, buf.as_mut_ptr(), 64 as libc::c_long);
        (n > 0).then(|| {
            buf.truncate(n as usize);
            buf
        })
    }
}

#[test]
fn the_login_password_s_key_reaches_the_session_keyring() {
    let module = std::env::current_exe().unwrap().parent().unwrap().join("libpam_olo_keyring.so");
    if !module.exists() {
        eprintln!("no {}: built with the library", module.display());
        return;
    }
    // SAFETY: KEYCTL_JOIN_SESSION_KEYRING with no name: a new anonymous one.
    assert!(unsafe { libc::syscall(libc::SYS_keyctl, 1 as libc::c_long, std::ptr::null::<libc::c_char>()) } > 0);
    let dir = PathBuf::from(format!("/var/tmp/olo-pam-keyring-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("check.sh");
    std::fs::write(&script, "#!/bin/sh\nread -r p\n[ \"$p\" = \"open sesame\" ]\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(
        dir.join("login"),
        format!(
            "auth required pam_exec.so type=auth expose_authtok quiet {}\nauth optional {m}\nauth required pam_permit.so\naccount required pam_permit.so\nsession required pam_permit.so\nsession optional {m}\n",
            script.display(),
            m = module.display()
        ),
    )
    .unwrap();
    // (pam_permit stands in for pam_unix: a stack whose modules are all
    // optional or ignored is refused.) A login with no password (a face,
    // a fingerprint): nothing kept.
    std::fs::write(
        dir.join("nopassword"),
        format!("auth required pam_permit.so\nauth optional {m}\naccount required pam_permit.so\nsession required pam_permit.so\nsession optional {m}\n", m = module.display()),
    )
    .unwrap();
    let (user, uid) = me();
    let converse = || -> olo_greet::pam::Converse {
        Box::new(|kind, _text: &str| match kind {
            PromptKind::Secret | PromptKind::Visible => Answer::Said(Some("open sesame".to_owned())),
            _ => Answer::Said(None),
        })
    };
    let mut p = Pam::start("nopassword", &user, Some(&dir), converse()).unwrap();
    assert_eq!(p.authenticate(), Ok(()));
    p.open_session().unwrap();
    assert_eq!(session_key(pam_olo_keyring::KEY_NAME), None);
    drop(p);
    let mut p = Pam::start("login", &user, Some(&dir), converse()).unwrap();
    assert_eq!(p.authenticate(), Ok(()));
    p.open_session().unwrap();
    let kept = session_key(pam_olo_keyring::KEY_NAME).expect("the key in the session keyring");
    assert_eq!(kept.as_slice(), pam_olo_keyring::login_key(b"open sesame", uid, &pam_olo_keyring::machine_id()).as_slice());
    drop(p);
    std::fs::remove_dir_all(dir).unwrap();
}
