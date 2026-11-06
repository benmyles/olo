//! The Secret Service's sessions: how secrets cross the bus. "plain" as
//! they are; "dh-ietf1024-sha256-aes128-cbc-pkcs7" (libsecret's choice,
//! so Chrome's and Slack's) encrypted: Diffie-Hellman over RFC 2409's
//! second Oakley group (1024 bits, generator 2), the shared secret padded
//! to the prime's length (as libsecret's egg_dh_gen_secret writes it),
//! HKDF-SHA256 with no salt and no info to a 16-byte key, each secret
//! AES-128-CBC with PKCS#7 padding and a random IV as its parameters.

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use num_bigint::BigUint;
use zeroize::Zeroizing;

pub const PLAIN: &str = "plain";
pub const DH: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";

/// RFC 2409's second Oakley group (egg-dh.c's "ietf-ike-grp-modp-1024").
const PRIME_1024: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF";

fn prime() -> BigUint {
    BigUint::parse_bytes(PRIME_1024.as_bytes(), 16).expect("prime")
}

/// One client's session.
pub enum Crypt {
    Plain,
    Aes(Zeroizing<[u8; 16]>),
}

/// A random exponent below 2^(bits − 1), not zero, and 2 to it mod p
/// (egg_dh_gen_pair).
fn pair(p: &BigUint) -> (BigUint, BigUint) {
    let bits = p.bits();
    loop {
        let mut bytes = Zeroizing::new(vec![0u8; bits.div_ceil(8) as usize]);
        getrandom::fill(&mut bytes).expect("random");
        let mut private = BigUint::from_bytes_be(&bytes);
        for b in (bits - 1)..(bytes.len() as u64 * 8) {
            private.set_bit(b, false);
        }
        if private != BigUint::ZERO {
            let public = BigUint::from(2u32).modpow(&private, p);
            return (private, public);
        }
    }
}

/// `n` as big-endian bytes as long as `p`'s.
fn padded(n: &BigUint, p: &BigUint) -> Zeroizing<Vec<u8>> {
    let bytes = n.to_bytes_be();
    let len = p.bits().div_ceil(8) as usize;
    let mut out = Zeroizing::new(vec![0u8; len.saturating_sub(bytes.len())]);
    out.extend_from_slice(&bytes);
    out
}

/// The key from a shared secret.
fn derive(shared: &[u8]) -> Zeroizing<[u8; 16]> {
    let mut key = Zeroizing::new([0u8; 16]);
    hkdf::Hkdf::<sha2::Sha256>::new(None, shared).expand(&[], key.as_mut()).expect("16 bytes of HKDF");
    key
}

impl Crypt {
    /// The session `OpenSession(algorithm, input)` asks for, and its
    /// output (our public key for DH); None if not one we know.
    pub fn open(algorithm: &str, input: &[u8]) -> Option<Result<(Crypt, Vec<u8>), &'static str>> {
        match algorithm {
            PLAIN => Some(Ok((Crypt::Plain, Vec::new()))),
            DH => {
                let p = prime();
                let peer = BigUint::from_bytes_be(input);
                if peer <= BigUint::from(1u32) || peer >= &p - 1u32 {
                    return Some(Err("the client's public key is out of range"));
                }
                let (private, public) = pair(&p);
                let shared = padded(&peer.modpow(&private, &p), &p);
                Some(Ok((Crypt::Aes(derive(&shared)), padded(&public, &p).to_vec())))
            }
            _ => None,
        }
    }

    /// A secret for the client: (parameters, value).
    pub fn seal(&self, secret: &[u8]) -> (Vec<u8>, Vec<u8>) {
        match self {
            Crypt::Plain => (Vec::new(), secret.to_vec()),
            Crypt::Aes(key) => {
                let mut iv = [0u8; 16];
                getrandom::fill(&mut iv).expect("random");
                let mut buf = vec![0u8; (secret.len() / 16 + 1) * 16];
                buf[..secret.len()].copy_from_slice(secret);
                let key: &[u8; 16] = key;
                let n = cbc::Encryptor::<aes::Aes128>::new(key.into(), (&iv).into()).encrypt_padded::<Pkcs7>(&mut buf, secret.len()).expect("room for the padding").len();
                buf.truncate(n);
                (iv.to_vec(), buf)
            }
        }
    }

    /// A secret from the client.
    pub fn open_secret(&self, parameters: &[u8], value: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        match self {
            Crypt::Plain => Some(Zeroizing::new(value.to_vec())),
            Crypt::Aes(key) => {
                let iv: &[u8; 16] = parameters.try_into().ok()?;
                let mut buf = Zeroizing::new(value.to_vec());
                let key: &[u8; 16] = key;
                let n = cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into()).decrypt_padded::<Pkcs7>(&mut buf).ok()?.len();
                buf.truncate(n);
                Some(buf)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client as libsecret is: its own pair, the same derivation.
    #[test]
    fn a_client_and_the_service_agree() {
        let p = prime();
        for _ in 0..64 {
            let (cpriv, cpub) = pair(&p);
            let Some(Ok((service, out))) = Crypt::open(DH, &padded(&cpub, &p)) else { panic!() };
            assert_eq!(out.len(), 128);
            let theirs = BigUint::from_bytes_be(&out);
            let client = Crypt::Aes(derive(&padded(&theirs.modpow(&cpriv, &p), &p)));
            for secret in [&b""[..], b"x", b"sixteen bytes!!!", b"a longer secret than one block"] {
                let (iv, sealed) = service.seal(secret);
                assert_eq!(client.open_secret(&iv, &sealed).unwrap().as_slice(), secret);
                let (iv, sealed) = client.seal(secret);
                assert_eq!(service.open_secret(&iv, &sealed).unwrap().as_slice(), secret);
            }
        }
    }

    #[test]
    fn nonsense_refused() {
        assert!(matches!(Crypt::open(DH, &[1]), Some(Err(_))));
        assert!(Crypt::open("dh-ietf9999", &[]).is_none());
        let Some(Ok((plain, out))) = Crypt::open(PLAIN, &[]) else { panic!() };
        assert!(out.is_empty());
        assert_eq!(plain.seal(b"s"), (Vec::new(), b"s".to_vec()));
        let key = Crypt::Aes(Zeroizing::new([7; 16]));
        assert!(key.open_secret(&[0; 15], &[0; 16]).is_none(), "an IV of the wrong length");
    }
}
