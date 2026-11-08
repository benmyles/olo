//! The key, ids and sealing: two keys from the one kept by Passwords
//! (HKDF-SHA-256), one for ids (HMAC-SHA-256 of what a blob holds: equal
//! content, equal id, and nothing to learn from it without the key), one
//! for sealing (XChaCha20-Poly1305, a random nonce each).

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use hmac::Mac;
use sha2::Sha256;

/// A blob's id: what it holds, hashed under the repository's key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub [u8; 32]);

impl Id {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Id> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Id(out))
    }
}

impl std::fmt::Debug for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.hex()[..12])
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Id, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Id;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("32 bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Id, E> {
                v.try_into().map(Id).map_err(|_| E::invalid_length(v.len(), &self))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Id, E> {
                self.visit_bytes(&v)
            }
        }
        d.deserialize_bytes(V)
    }
}

/// The repository's key: 32 random bytes, and what's made from them.
#[derive(Clone)]
pub struct Key {
    master: [u8; 32],
    seal: [u8; 32],
    ids: [u8; 32],
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

impl Key {
    /// A new key, from the system's random source.
    pub fn generate() -> Key {
        let mut m = [0u8; 32];
        getrandom::fill(&mut m).expect("the system's random source failed");
        Key::from_bytes(m)
    }

    pub fn from_bytes(master: [u8; 32]) -> Key {
        let hk = hkdf::Hkdf::<Sha256>::new(None, &master);
        let (mut seal, mut ids) = ([0u8; 32], [0u8; 32]);
        hk.expand(b"olo diskbackup: sealing", &mut seal).expect("32 bytes");
        hk.expand(b"olo diskbackup: ids", &mut ids).expect("32 bytes");
        Key { master, seal, ids }
    }

    /// The key as Passwords keeps it: 64 hex digits.
    pub fn to_hex(&self) -> String {
        Id(self.master).hex()
    }

    pub fn from_hex(s: &str) -> Option<Key> {
        Id::from_hex(s.trim()).map(|id| Key::from_bytes(id.0))
    }

    /// The id of a blob holding `data`.
    pub fn id(&self, data: &[u8]) -> Id {
        let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&self.ids).expect("any key length");
        mac.update(data);
        Id(mac.finalize().into_bytes().into())
    }

    /// `plain` sealed: a random nonce, then the sealed bytes.
    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).expect("the system's random source failed");
        let sealed = XChaCha20Poly1305::new((&self.seal).into()).encrypt(&XNonce::from(nonce), plain).expect("sealing");
        let mut out = Vec::with_capacity(24 + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// What `sealed` holds, or None if it wasn't sealed with this key (or
    /// was changed since).
    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < 24 {
            return None;
        }
        let nonce = XNonce::try_from(&sealed[..24]).ok()?;
        XChaCha20Poly1305::new((&self.seal).into()).decrypt(&nonce, &sealed[24..]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_and_ids() {
        let k = Key::generate();
        let sealed = k.seal(b"hello");
        assert_eq!(k.open(&sealed).as_deref(), Some(&b"hello"[..]));
        assert!(!sealed.windows(5).any(|w| w == b"hello"));
        // Changed, or another key: nothing.
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(k.open(&bad).is_none());
        assert!(Key::generate().open(&sealed).is_none());
        // Ids: equal for equal content, under one key only.
        assert_eq!(k.id(b"x"), k.id(b"x"));
        assert_ne!(k.id(b"x"), Key::generate().id(b"x"));
        // The key as kept, and back.
        let again = Key::from_hex(&k.to_hex()).unwrap();
        assert_eq!(again.id(b"x"), k.id(b"x"));
        assert_eq!(Id::from_hex(&k.id(b"x").hex()), Some(k.id(b"x")));
    }
}
