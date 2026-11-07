//! `pam_olo_keyring.so`: the password typed at login unlocks Olo's
//! keyring (olo-secrets), as `pam_gnome_keyring` does GNOME Keyring's,
//! without the password leaving PAM. In the login's stack after the
//! password (`auth optional`), it derives a key from the password
//! ([`login_key`]: Argon2id, salted by the user and the computer) and
//! keeps it with the PAM handle; once the session is opened (`session
//! optional`, after `pam_keyinit`, which gives the login a kernel keyring
//! of its own) it puts that key in the login's session keyring
//! ([`KEY_NAME`], the user's, gone in ten minutes, and with the login,
//! which `pam_keyinit revoke` takes away). olo-secrets, a process of
//! that login, takes it from there once and opens its store with it
//! (the store's own key is kept sealed by it: apps/secrets
//! `login_key.rs`).
//!
//! It never fails a login and never reads the person's files: no
//! password (a fingerprint, a face, a login screen's own session), or
//! anything going wrong, and it does nothing. olo-secrets then asks for
//! the password itself when it needs it.

use std::ffi::{CStr, c_char, c_int, c_void};

use zeroize::Zeroizing;

const PAM_SUCCESS: c_int = 0;
const PAM_IGNORE: c_int = 25;
const PAM_AUTHTOK: c_int = 6;

/// The handle's name for it between authentication and the session.
const DATA_NAME: &CStr = c"olo-keyring-login-key";
/// How long it waits there for olo-secrets.
const KEY_SECONDS: u32 = 600;

pub use olo_login_key::{KEY_NAME, login_key, machine_id};

// libpam by its file name (as PAM's own modules need it: a host that
// opened libpam privately, a test's, still gives them to the module),
// no development link needed to build.
#[link(name = "libpam.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn pam_get_user(pamh: *mut c_void, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: *const c_void, item: c_int, out: *mut *const c_void) -> c_int;
    fn pam_set_data(pamh: *mut c_void, name: *const c_char, data: *mut c_void, cleanup: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int)>) -> c_int;
    fn pam_get_data(pamh: *const c_void, name: *const c_char, data: *mut *const c_void) -> c_int;
}

/// The key kept with the handle, wiped and freed when PAM ends.
unsafe extern "C" fn forget(_pamh: *mut c_void, data: *mut c_void, _status: c_int) {
    if !data.is_null() {
        // SAFETY: made by Box::into_raw in pam_sm_authenticate.
        drop(unsafe { Box::from_raw(data as *mut Zeroizing<[u8; 32]>) });
    }
}

/// The user's uid and gid.
fn ids(pamh: *mut c_void) -> Option<(libc::uid_t, libc::gid_t)> {
    let mut user: *const c_char = std::ptr::null();
    // SAFETY: PAM gives a NUL-terminated name it keeps.
    if unsafe { pam_get_user(pamh, &mut user, std::ptr::null()) } != PAM_SUCCESS || user.is_null() {
        return None;
    }
    // SAFETY: getpwnam with that name; its answer read at once.
    let pw = unsafe { libc::getpwnam(user) };
    (!pw.is_null()).then(|| unsafe { ((*pw).pw_uid, (*pw).pw_gid) })
}

/// # Safety
/// Called by PAM with its handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(pamh: *mut c_void, _flags: c_int, _argc: c_int, _argv: *const *const c_char) -> c_int {
    let mut token: *const c_void = std::ptr::null();
    // SAFETY: the password the stack took, NUL-terminated, PAM's.
    if unsafe { pam_get_item(pamh, PAM_AUTHTOK, &mut token) } != PAM_SUCCESS || token.is_null() {
        return PAM_IGNORE;
    }
    let password = Zeroizing::new(unsafe { CStr::from_ptr(token as *const c_char) }.to_bytes().to_vec());
    let Some((uid, _)) = ids(pamh) else { return PAM_IGNORE };
    let machine = machine_id();
    if password.is_empty() || machine.is_empty() {
        return PAM_IGNORE;
    }
    let key = Box::new(login_key(&password, uid, &machine));
    let data = Box::into_raw(key) as *mut c_void;
    // SAFETY: PAM keeps the pointer and calls `forget` with it at the end.
    if unsafe { pam_set_data(pamh, DATA_NAME.as_ptr(), data, Some(forget)) } != PAM_SUCCESS {
        unsafe { forget(pamh, data, 0) };
    }
    PAM_IGNORE
}

/// # Safety
/// Called by PAM with its handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_setcred(_pamh: *mut c_void, _flags: c_int, _argc: c_int, _argv: *const *const c_char) -> c_int {
    PAM_SUCCESS
}

const KEY_SPEC_SESSION_KEYRING: libc::c_long = -3;
const KEYCTL_CHOWN: libc::c_long = 4;
const KEYCTL_SETPERM: libc::c_long = 5;
const KEYCTL_SET_TIMEOUT: libc::c_long = 15;
/// Its possessor (the login's processes) everything; its user may view,
/// read, find and set its timeout.
const PERM: u32 = 0x3f00_0000 | 0x0001_0000 | 0x0002_0000 | 0x0008_0000 | 0x0020_0000;

/// # Safety
/// Called by PAM with its handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_open_session(pamh: *mut c_void, _flags: c_int, _argc: c_int, _argv: *const *const c_char) -> c_int {
    let mut data: *const c_void = std::ptr::null();
    // SAFETY: what pam_sm_authenticate kept, if it did.
    if unsafe { pam_get_data(pamh, DATA_NAME.as_ptr(), &mut data) } != PAM_SUCCESS || data.is_null() {
        return PAM_IGNORE;
    }
    let key: &Zeroizing<[u8; 32]> = unsafe { &*(data as *const Zeroizing<[u8; 32]>) };
    let Some((uid, gid)) = ids(pamh) else { return PAM_IGNORE };
    let Ok(name) = std::ffi::CString::new(KEY_NAME) else { return PAM_IGNORE };
    // SAFETY: add_key with NUL-terminated strings and a 32-byte payload,
    // then keyctl on the key it made.
    unsafe {
        let id = libc::syscall(libc::SYS_add_key, c"user".as_ptr(), name.as_ptr(), key.as_ptr(), 32 as libc::size_t, KEY_SPEC_SESSION_KEYRING);
        if id > 0 {
            libc::syscall(libc::SYS_keyctl, KEYCTL_CHOWN, id, uid as libc::c_long, gid as libc::c_long);
            libc::syscall(libc::SYS_keyctl, KEYCTL_SETPERM, id, PERM as libc::c_long);
            libc::syscall(libc::SYS_keyctl, KEYCTL_SET_TIMEOUT, id, KEY_SECONDS as libc::c_long);
        }
    }
    PAM_IGNORE
}

/// # Safety
/// Called by PAM with its handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_close_session(_pamh: *mut c_void, _flags: c_int, _argc: c_int, _argv: *const *const c_char) -> c_int {
    PAM_IGNORE
}
