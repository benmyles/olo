//! The key a login password gives: what `pam_olo_keyring` makes at
//! login and hands to olo-secrets in the login's kernel keyring
//! ([`KEY_NAME`]), and what olo-secrets makes itself when it has to ask
//! for the password (a login without one, or the password changed). Its
//! own crate so olo-secrets shares it without the PAM module.

use zeroize::Zeroizing;

/// What the key is called in the session keyring.
pub const KEY_NAME: &str = "olo-secrets:login-key";

/// The key a login password gives user `uid` on the computer
/// `machine_id` (/etc/machine-id): Argon2id (64 MiB, 3 passes, 1 lane)
/// of the password, salted by both. The same password always gives the
/// same key, so PAM and olo-secrets (asking for it when PAM had none)
/// agree; another user or computer gets another.
pub fn login_key(password: &[u8], uid: u32, machine_id: &str) -> Zeroizing<[u8; 32]> {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"olo-secrets login key\0");
    h.update(uid.to_string().as_bytes());
    h.update(b"\0");
    h.update(machine_id.trim().as_bytes());
    let salt = h.finalize();
    let params = argon2::Params::new(64 * 1024, 3, 1, Some(32)).expect("Argon2's parameters");
    let mut key = Zeroizing::new([0u8; 32]);
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params).hash_password_into(password, &salt, key.as_mut()).expect("Argon2");
    key
}

/// This computer's id.
pub fn machine_id() -> String {
    std::fs::read_to_string("/etc/machine-id").unwrap_or_default().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_password_gives_the_same_key_for_its_user_only() {
        let t = std::time::Instant::now();
        let a = login_key(b"correct horse", 1000, "0123456789abcdef");
        let took = t.elapsed();
        assert_eq!(*a, *login_key(b"correct horse", 1000, "0123456789abcdef\n"));
        assert_ne!(*a, *login_key(b"correct horse!", 1000, "0123456789abcdef"));
        assert_ne!(*a, *login_key(b"correct horse", 1001, "0123456789abcdef"));
        assert_ne!(*a, *login_key(b"correct horse", 1000, "fedcba9876543210"));
        eprintln!("one derivation: {took:?}");
    }
}
