//! ClientPIN's key agreement (CTAP 2.1 §6.5): a shared secret with the
//! key over P-256 ECDH, in PIN/UV protocol 1 or 2, which `hmac-secret`
//! encrypts its salts and its answer with.

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::NoPadding};
use ciborium::Value;
use hmac::{KeyInit, Mac};
use p256::{PublicKey, SecretKey, elliptic_curve::sec1::ToSec1Point};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    Error, Result,
    ctap::{CLIENT_PIN, Channel, Wait, get, int_map},
    hid::Transport,
};

/// The secret shared with a key, in one protocol.
pub struct Shared {
    pub protocol: u8,
    hmac_key: Zeroizing<[u8; 32]>,
    aes_key: Zeroizing<[u8; 32]>,
    /// Ours, as a COSE key, for the key to agree on its side.
    pub platform_key: Value,
}

/// Agree a secret with the key behind `ch` in PIN protocol `protocol`.
pub fn agree<T: Transport>(ch: &mut Channel<T>, protocol: u8, wait: &Wait) -> Result<Shared> {
    let params = int_map(vec![(1, Value::Integer(protocol.into())), (2, Value::Integer(2.into()))]);
    let answer = ch.cbor(CLIENT_PIN, Some(params), wait)?.ok_or(Error::Protocol("no key agreement"))?;
    let theirs = cose_public(get(&answer, 1).ok_or(Error::Protocol("no key agreement"))?)?;
    shared_with(&theirs, protocol)
}

/// A new key pair of ours, and the secret it shares with `theirs`.
pub fn shared_with(theirs: &PublicKey, protocol: u8) -> Result<Shared> {
    let ours = random_secret()?;
    let z = p256::ecdh::diffie_hellman(ours.to_nonzero_scalar(), theirs.as_affine());
    let platform_key = cose_key(&ours.public_key());
    derive(z.raw_secret_bytes().as_slice(), protocol, platform_key)
}

/// The keys protocol `protocol` derives from an ECDH result's x.
pub fn derive(z: &[u8], protocol: u8, platform_key: Value) -> Result<Shared> {
    let (hmac_key, aes_key) = match protocol {
        1 => {
            let k: [u8; 32] = Sha256::digest(z).into();
            (k, k)
        }
        2 => {
            let hk = hkdf::Hkdf::<Sha256>::new(Some(&[0u8; 32]), z);
            let (mut h, mut a) = ([0u8; 32], [0u8; 32]);
            hk.expand(b"CTAP2 HMAC key", &mut h).map_err(|_| Error::Protocol("HKDF"))?;
            hk.expand(b"CTAP2 AES key", &mut a).map_err(|_| Error::Protocol("HKDF"))?;
            (h, a)
        }
        _ => return Err(Error::Protocol("an unknown PIN protocol")),
    };
    Ok(Shared { protocol, hmac_key: Zeroizing::new(hmac_key), aes_key: Zeroizing::new(aes_key), platform_key })
}

impl Shared {
    /// `data` (whole AES blocks) encrypted: protocol 1 with a zero IV,
    /// 2 with a random one before it.
    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if !data.len().is_multiple_of(16) {
            return Err(Error::Protocol("not whole AES blocks"));
        }
        let mut iv = [0u8; 16];
        if self.protocol == 2 {
            getrandom::fill(&mut iv).map_err(|_| Error::Random)?;
        }
        let mut buf = data.to_vec();
        let n = buf.len();
        cbc::Encryptor::<aes::Aes256>::new((&*self.aes_key).into(), (&iv).into()).encrypt_padded::<NoPadding>(&mut buf, n).map_err(|_| Error::Protocol("AES"))?;
        Ok(if self.protocol == 2 { [&iv[..], &buf].concat() } else { buf })
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (iv, ct) = if self.protocol == 2 {
            if data.len() < 16 {
                return Err(Error::Protocol("too short to decrypt"));
            }
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&data[..16]);
            (iv, &data[16..])
        } else {
            ([0u8; 16], data)
        };
        if !ct.len().is_multiple_of(16) {
            return Err(Error::Protocol("not whole AES blocks"));
        }
        let mut buf = Zeroizing::new(ct.to_vec());
        cbc::Decryptor::<aes::Aes256>::new((&*self.aes_key).into(), (&iv).into()).decrypt_padded::<NoPadding>(&mut buf).map_err(|_| Error::Protocol("AES"))?;
        Ok(buf)
    }

    /// `msg`'s authentication: HMAC-SHA-256, its first 16 bytes in
    /// protocol 1.
    pub fn authenticate(&self, msg: &[u8]) -> Vec<u8> {
        let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&*self.hmac_key).expect("any key length");
        mac.update(msg);
        let full = mac.finalize().into_bytes();
        if self.protocol == 1 { full[..16].to_vec() } else { full.to_vec() }
    }
}

/// A random P-256 secret key.
pub fn random_secret() -> Result<SecretKey> {
    loop {
        let mut b = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *b).map_err(|_| Error::Random)?;
        if let Ok(k) = SecretKey::from_slice(&*b) {
            return Ok(k);
        }
    }
}

/// A P-256 public key as COSE_Key (kty EC2, alg ECDH-ES+HKDF-256, crv
/// P-256, x, y).
pub fn cose_key(k: &PublicKey) -> Value {
    let p = k.to_sec1_point(false);
    let (x, y) = (p.x().expect("not the identity").to_vec(), p.y().expect("uncompressed").to_vec());
    int_map(vec![(1, Value::Integer(2.into())), (3, Value::Integer((-25).into())), (-1, Value::Integer(1.into())), (-2, Value::Bytes(x)), (-3, Value::Bytes(y))])
}

/// The P-256 public key in a COSE_Key.
pub fn cose_public(v: &Value) -> Result<PublicKey> {
    let coord = |k| get(v, k).and_then(|c| c.as_bytes()).filter(|b| b.len() == 32).ok_or(Error::Protocol("a COSE key's coordinates"));
    let (x, y) = (coord(-2)?, coord(-3)?);
    let point = [&[0x04][..], x, y].concat();
    PublicKey::from_sec1_bytes(&point).map_err(|_| Error::Protocol("a COSE key not on P-256"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_agree_and_round_trip() {
        for protocol in [1, 2] {
            // Ours and the key's, each from the other's public key.
            let key_side = random_secret().unwrap();
            let ours = shared_with(&key_side.public_key(), protocol).unwrap();
            let platform = cose_public(&ours.platform_key).unwrap();
            let z = p256::ecdh::diffie_hellman(key_side.to_nonzero_scalar(), platform.as_affine());
            let theirs = derive(z.raw_secret_bytes().as_slice(), protocol, Value::Null).unwrap();
            let salt = [7u8; 32];
            let enc = ours.encrypt(&salt).unwrap();
            assert_eq!(enc.len(), if protocol == 2 { 48 } else { 32 });
            assert_eq!(&**theirs.decrypt(&enc).unwrap(), &salt);
            assert_eq!(ours.authenticate(&enc), theirs.authenticate(&enc));
            assert_eq!(ours.authenticate(&enc).len(), if protocol == 2 { 32 } else { 16 });
        }
    }

    #[test]
    fn protocol_two_keys_are_ctaps() {
        // HKDF-SHA-256 with a zero salt, CTAP 2.1's two labels: the HMAC
        // and AES keys differ, and protocol 1's are SHA-256 of x.
        let z = [1u8; 32];
        let two = derive(&z, 2, Value::Null).unwrap();
        assert_ne!(*two.hmac_key, *two.aes_key);
        let one = derive(&z, 1, Value::Null).unwrap();
        assert_eq!(*one.aes_key, <[u8; 32]>::from(Sha256::digest(z)));
    }
}
