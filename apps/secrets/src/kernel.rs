//! The store's key kept for the login in the kernel's keyring (the
//! session's: keyrings(7)), so olo-secrets started again (an upgrade, a
//! crash) opens the store without GNOME Keyring, which has quit by then
//! and would ask for the password to start again. Only the person's own
//! processes of this login can read it, as they could read olo-secrets'
//! memory. Named by the store's path; invalidated on handing back.

use std::ffi::CString;

use zeroize::Zeroizing;

const KEY_SPEC_SESSION_KEYRING: libc::c_long = -3;
const KEYCTL_SEARCH: libc::c_long = 10;
const KEYCTL_READ: libc::c_long = 11;
const KEYCTL_INVALIDATE: libc::c_long = 21;

fn description(store: &std::path::Path) -> Option<CString> {
    CString::new(format!("olo-secrets:{}", store.display())).ok()
}

fn find(desc: &CString) -> Option<libc::c_long> {
    // SAFETY: keyctl(KEYCTL_SEARCH) with NUL-terminated strings.
    let id = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_SEARCH, KEY_SPEC_SESSION_KEYRING, c"user".as_ptr(), desc.as_ptr(), 0 as libc::c_long) };
    (id > 0).then_some(id)
}

/// The key kept for the store at `store`, if there is one.
pub fn get(store: &std::path::Path) -> Option<Zeroizing<[u8; 32]>> {
    let id = find(&description(store)?)?;
    let mut key = Zeroizing::new([0u8; 32]);
    // SAFETY: keyctl(KEYCTL_READ) into a 32-byte buffer.
    let n = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_READ, id, key.as_mut_ptr(), 32 as libc::c_long) };
    (n == 32).then_some(key)
}

/// Keep `key` for the store at `store` (replacing one kept before).
pub fn put(store: &std::path::Path, key: &[u8; 32]) -> bool {
    let Some(desc) = description(store) else { return false };
    // SAFETY: add_key with NUL-terminated strings and a 32-byte payload.
    let id = unsafe { libc::syscall(libc::SYS_add_key, c"user".as_ptr(), desc.as_ptr(), key.as_ptr(), 32 as libc::size_t, KEY_SPEC_SESSION_KEYRING) };
    id > 0
}

/// The key the login password gave (pam_olo_keyring put it in this
/// login's session keyring), taken: read once, then gone from there.
pub fn take_login_key() -> Option<Zeroizing<[u8; 32]>> {
    let desc = CString::new(olo_login_key::KEY_NAME).ok()?;
    let id = find(&desc)?;
    let mut key = Zeroizing::new([0u8; 32]);
    // SAFETY: keyctl(KEYCTL_READ) into a 32-byte buffer, then
    // KEYCTL_INVALIDATE on the key found.
    let n = unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_READ, id, key.as_mut_ptr(), 32 as libc::c_long) };
    unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_INVALIDATE, id) };
    (n == 32).then_some(key)
}

/// Forget the key kept for the store at `store`.
pub fn forget(store: &std::path::Path) {
    if let Some(id) = description(store).and_then(|d| find(&d)) {
        // SAFETY: keyctl(KEYCTL_INVALIDATE) on a key we found.
        unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_INVALIDATE, id) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn kept_and_forgotten() {
        let store = std::path::PathBuf::from(format!("/var/tmp/olo-secrets-kernel-test-{}/secrets.store", std::process::id()));
        assert!(super::get(&store).is_none());
        if !super::put(&store, &[7; 32]) {
            eprintln!("no kernel keyring here");
            return;
        }
        assert_eq!(super::get(&store).as_deref(), Some(&[7u8; 32]));
        assert!(super::put(&store, &[8; 32]));
        assert_eq!(super::get(&store).as_deref(), Some(&[8u8; 32]));
        super::forget(&store);
        assert!(super::get(&store).is_none());
    }
}
