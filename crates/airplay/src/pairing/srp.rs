//! SRP-6a as HomeKit's pair-setup uses it: the 3072-bit group of RFC
//! 5054, SHA-512, the name "Pair-Setup", and the code the receiver shows
//! as the password. The sender's side; the receiver's is here too, for
//! the tests' own receiver.
//!
//! The details that differ between implementations are as Apple's
//! receivers take them (as srptools has them, which pyatv pairs with
//! Apple TVs by, and as Apple's open HomeKit ADK has them): k = H(N |
//! PAD(g)); u = H(PAD(A) | PAD(B)); x = H(s | H(I ":" P)); the session
//! key K = H(S), S without its leading zeros; the proofs M1 = H(H(N) xor
//! H(g) | H(I) | s | A | B | K) and M2 = H(A | M1 | K), A and B without
//! their leading zeros.

use num_bigint::BigUint;
use sha2::{Digest as _, Sha512};

/// RFC 5054's 3072-bit group; its generator is 5.
const N_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF";
const G: u32 = 5;
pub const NAME: &str = "Pair-Setup";
/// A public value's length: the group's.
pub const KEY_BYTES: usize = 384;

fn n() -> BigUint {
    BigUint::parse_bytes(N_HEX.as_bytes(), 16).expect("the group")
}

fn h(parts: &[&[u8]]) -> [u8; 64] {
    let mut hash = Sha512::new();
    for p in parts {
        hash.update(p);
    }
    hash.finalize().into()
}

/// A number as its bytes, the group's length (zeros before it).
fn pad(v: &BigUint) -> Vec<u8> {
    let bytes = v.to_bytes_be();
    let mut out = vec![0; KEY_BYTES.saturating_sub(bytes.len())];
    out.extend(bytes);
    out
}

fn strip(bytes: &[u8]) -> &[u8] {
    &bytes[bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len())..]
}

fn k() -> BigUint {
    BigUint::from_bytes_be(&h(&[&n().to_bytes_be(), &pad(&BigUint::from(G))]))
}

fn x(salt: &[u8], code: &str) -> BigUint {
    BigUint::from_bytes_be(&h(&[salt, &h(&[NAME.as_bytes(), b":", code.as_bytes()])]))
}

fn u(a_pub: &BigUint, b_pub: &BigUint) -> BigUint {
    BigUint::from_bytes_be(&h(&[&pad(a_pub), &pad(b_pub)]))
}

fn m1(salt: &[u8], a_pub: &[u8], b_pub: &[u8], key: &[u8]) -> [u8; 64] {
    let (hn, hg) = (h(&[&n().to_bytes_be()]), h(&[&[G as u8]]));
    let both: Vec<u8> = hn.iter().zip(hg).map(|(a, b)| a ^ b).collect();
    h(&[strip(&both), strip(&h(&[NAME.as_bytes()])), salt, strip(a_pub), strip(b_pub), key])
}

fn m2(a_pub: &[u8], m1: &[u8], key: &[u8]) -> [u8; 64] {
    h(&[strip(a_pub), m1, key])
}

/// What the sender works out from the receiver's salt and public value
/// and the code: its own public value, its proof, the session key, and
/// the proof the receiver must answer with.
pub struct Client {
    pub public: Vec<u8>,
    pub proof: [u8; 64],
    pub key: [u8; 64],
    answer: [u8; 64],
}

impl Client {
    /// `secret`: random, kept by nobody after. None if the receiver's
    /// public value is one no honest receiver sends (a multiple of the
    /// group's prime: the key would be the same whatever the code).
    pub fn new(secret: &[u8], salt: &[u8], their_public: &[u8], code: &str) -> Option<Client> {
        let (n, g, a) = (n(), BigUint::from(G), BigUint::from_bytes_be(secret));
        let b_pub = BigUint::from_bytes_be(their_public);
        if (&b_pub % &n) == BigUint::ZERO {
            return None;
        }
        let a_pub = g.modpow(&a, &n);
        let (u, x) = (u(&a_pub, &b_pub), x(salt, code));
        if u == BigUint::ZERO {
            return None;
        }
        // S = (B - k g^x) ^ (a + u x)
        let base = (&b_pub + &n * k() - (k() * g.modpow(&x, &n)) % &n) % &n;
        let s = base.modpow(&(a + u * x), &n);
        let key = h(&[&s.to_bytes_be()]);
        let public = pad(&a_pub);
        let proof = m1(salt, &public, their_public, &key);
        Some(Client { answer: m2(&public, &proof, &key), public, proof, key })
    }

    /// Whether the receiver's proof is right: it knew the code too.
    pub fn answered(&self, their_proof: &[u8]) -> bool {
        their_proof == self.answer
    }
}

/// The receiver's side, for a receiver of a test's own.
#[doc(hidden)]
pub struct Server {
    pub salt: Vec<u8>,
    pub public: Vec<u8>,
    secret: BigUint,
    verifier: BigUint,
}

#[doc(hidden)]
impl Server {
    pub fn new(secret: &[u8], salt: &[u8], code: &str) -> Server {
        let (n, g, b) = (n(), BigUint::from(G), BigUint::from_bytes_be(secret));
        let verifier = g.modpow(&x(salt, code), &n);
        let public = pad(&((k() * &verifier + g.modpow(&b, &n)) % &n));
        Server { salt: salt.to_vec(), public, secret: b, verifier }
    }

    /// The session key and the proof to answer with, if the sender's
    /// proof is right.
    pub fn check(&self, their_public: &[u8], their_proof: &[u8]) -> Option<([u8; 64], [u8; 64])> {
        let n = n();
        let a_pub = BigUint::from_bytes_be(their_public);
        if (&a_pub % &n) == BigUint::ZERO {
            return None;
        }
        let u = u(&a_pub, &BigUint::from_bytes_be(&self.public));
        let s = (a_pub * self.verifier.modpow(&u, &n)).modpow(&self.secret, &n);
        let key = h(&[&s.to_bytes_be()]);
        let expected = m1(&self.salt, their_public, &self.public, &key);
        (their_proof == expected).then(|| (key, m2(their_public, &expected, &key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_come_to_the_same_key_with_the_same_code() {
        let salt = [0x5a; 16];
        let server = Server::new(&[0x21; 32], &salt, "3939");
        assert_eq!(server.public.len(), KEY_BYTES);
        let client = Client::new(&[0x42; 32], &salt, &server.public, "3939").unwrap();
        assert_eq!(client.public.len(), KEY_BYTES);
        let (key, answer) = server.check(&client.public, &client.proof).expect("the right code");
        assert_eq!(key, client.key);
        assert!(client.answered(&answer));
        assert!(!client.answered(&[0; 64]));
        // Another code: the receiver refuses the proof.
        let wrong = Client::new(&[0x42; 32], &salt, &server.public, "3940").unwrap();
        assert_ne!(wrong.key, client.key);
        assert!(server.check(&wrong.public, &wrong.proof).is_none());
    }

    /// A salt that begins with a zero is hashed whole (srptools drops the
    /// zero; a receiver doesn't), and public values that begin with zeros
    /// agree between the two sides.
    #[test]
    fn leading_zeros() {
        let salt = [0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7];
        let server = Server::new(&[9; 32], &salt, "0000");
        let client = Client::new(&[3; 32], &salt, &server.public, "0000").unwrap();
        assert!(server.check(&client.public, &client.proof).is_some());
        assert_eq!(strip(&[0, 0, 5, 0]), &[5, 0]);
        assert_eq!(strip(&[0, 0]), &[] as &[u8]);
    }

    /// The two hashes that are the same for every pairing begin with no
    /// zero byte: whether an implementation drops leading zeros of them
    /// or not (they differ in that), the proof is the same.
    #[test]
    fn the_constants_need_no_stripping() {
        let (hn, hg) = (h(&[&n().to_bytes_be()]), h(&[&[G as u8]]));
        assert_ne!(hn[0] ^ hg[0], 0);
        assert_ne!(h(&[NAME.as_bytes()])[0], 0);
        assert_eq!(n().to_bytes_be().len(), KEY_BYTES);
    }

    #[test]
    fn a_public_value_no_honest_side_sends_is_refused() {
        assert!(Client::new(&[1; 32], &[1; 16], &[0; KEY_BYTES], "1111").is_none());
        assert!(Client::new(&[1; 32], &[1; 16], &n().to_bytes_be(), "1111").is_none());
        let server = Server::new(&[9; 32], &[1; 16], "1111");
        assert!(server.check(&[0; KEY_BYTES], &[0; 64]).is_none());
    }
}
