//! Web Crypto's primitives, native: digests (SHA-1, SHA-256, SHA-384,
//! SHA-512), HMAC over them, and random bytes from the system's generator
//! (`crypto.getRandomValues` is cryptographically strong, as the web
//! promises).
use hmac::{KeyInit, Mac};
use sha2::Digest;

/// The digest of `data` by its Web Crypto name, or None for a name it
/// doesn't know.
pub(crate) fn digest(algorithm: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match algorithm.to_ascii_uppercase().as_str() {
        "SHA-1" => sha1::Sha1::digest(data).to_vec(),
        "SHA-256" => sha2::Sha256::digest(data).to_vec(),
        "SHA-384" => sha2::Sha384::digest(data).to_vec(),
        "SHA-512" => sha2::Sha512::digest(data).to_vec(),
        _ => return None,
    })
}
/// HMAC of `data` under `key` with the named hash.
pub(crate) fn hmac(algorithm: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    macro_rules! sign {
        ($hash:ty) => {{
            let mut mac = <hmac::Hmac<$hash> as KeyInit>::new_from_slice(key).ok()?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }};
    }
    Some(match algorithm.to_ascii_uppercase().as_str() {
        "SHA-1" => sign!(sha1::Sha1),
        "SHA-256" => sign!(sha2::Sha256),
        "SHA-384" => sign!(sha2::Sha384),
        "SHA-512" => sign!(sha2::Sha512),
        _ => return None,
    })
}
/// `n` bytes from the system's random generator (at most 64 KiB, as
/// getRandomValues allows).
pub(crate) fn random(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n.min(65536)];
    if getrandom::fill(&mut bytes).is_err() {
        bytes.clear();
    }
    bytes
}
#[cfg(test)]
mod tests {
    #[test]
    fn digests_and_hmac_match_their_known_answers() {
        let hex = |b: Vec<u8>| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(super::digest("SHA-256", b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(super::digest("sha-1", b"abc").unwrap()), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert!(super::digest("MD5", b"abc").is_none());
        // RFC 4231 test case 2.
        assert_eq!(hex(super::hmac("SHA-256", b"Jefe", b"what do ya want for nothing?").unwrap()), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let r = super::random(32);
        assert_eq!(r.len(), 32);
        assert_ne!(r, vec![0u8; 32]);
    }
}
