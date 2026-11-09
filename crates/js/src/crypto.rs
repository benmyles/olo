//! Web Crypto's primitives, native: digests (SHA-1, SHA-256, SHA-384,
//! SHA-512), HMAC over them, AES (GCM, CBC, CTR), PBKDF2 and HKDF, and
//! random bytes from the system's generator (`crypto.getRandomValues` is
//! cryptographically strong, as the web promises).
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
/// AES encryption or decryption of `data` under `key` (16, 24 or 32
/// bytes) in `mode`: GCM (`iv` its nonce, 12 bytes; `aad` its additional
/// data; a 128-bit tag, appended as Web Crypto has it), CBC (`iv` 16
/// bytes, PKCS #7 padding) or CTR (`iv` its counter block; `counter_bits`
/// of it count). None for what it can't do (or a decryption that fails).
pub(crate) fn aes(mode: &str, encrypt: bool, key: &[u8], iv: &[u8], data: &[u8], aad: &[u8], counter_bits: usize) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit as _, Payload};
    use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, StreamCipher, block_padding::Pkcs7};
    match mode.to_ascii_uppercase().as_str() {
        "AES-GCM" => {
            if iv.len() != 12 {
                return None;
            }
            let nonce = aes_gcm::Nonce::from_slice(iv);
            let payload = Payload { msg: data, aad };
            macro_rules! gcm {
                ($cipher:ty) => {{
                    let c = <$cipher>::new_from_slice(key).ok()?;
                    if encrypt { c.encrypt(nonce, payload).ok() } else { c.decrypt(nonce, payload).ok() }
                }};
            }
            match key.len() {
                16 => gcm!(aes_gcm::Aes128Gcm),
                24 => gcm!(aes_gcm::AesGcm<aes::Aes192, aes_gcm::aead::consts::U12>),
                32 => gcm!(aes_gcm::Aes256Gcm),
                _ => None,
            }
        }
        "AES-CBC" => {
            if iv.len() != 16 {
                return None;
            }
            macro_rules! cbc {
                ($aes:ty) => {{
                    if encrypt {
                        Some(cbc::Encryptor::<$aes>::new_from_slices(key, iv).ok()?.encrypt_padded_vec_mut::<Pkcs7>(data))
                    } else {
                        cbc::Decryptor::<$aes>::new_from_slices(key, iv).ok()?.decrypt_padded_vec_mut::<Pkcs7>(data).ok()
                    }
                }};
            }
            match key.len() {
                16 => cbc!(aes::Aes128),
                24 => cbc!(aes::Aes192),
                32 => cbc!(aes::Aes256),
                _ => None,
            }
        }
        // (Encryption and decryption are the same in CTR.)
        "AES-CTR" => {
            if iv.len() != 16 {
                return None;
            }
            let mut out = data.to_vec();
            macro_rules! ctr {
                ($flavour:ident, $aes:ty) => {{
                    ctr::$flavour::<$aes>::new_from_slices(key, iv).ok()?.apply_keystream(&mut out);
                }};
            }
            macro_rules! sized {
                ($flavour:ident) => {
                    match key.len() {
                        16 => ctr!($flavour, aes::Aes128),
                        24 => ctr!($flavour, aes::Aes192),
                        32 => ctr!($flavour, aes::Aes256),
                        _ => return None,
                    }
                };
            }
            match counter_bits {
                32 => sized!(Ctr32BE),
                64 => sized!(Ctr64BE),
                _ => sized!(Ctr128BE),
            }
            Some(out)
        }
        _ => None,
    }
}
/// PBKDF2 over HMAC with the named hash: `bits` of it.
pub(crate) fn pbkdf2(hash: &str, password: &[u8], salt: &[u8], iterations: u32, bits: usize) -> Option<Vec<u8>> {
    if bits == 0 || bits % 8 != 0 || iterations == 0 {
        return None;
    }
    let mut out = vec![0u8; bits / 8];
    match hash.to_ascii_uppercase().as_str() {
        "SHA-1" => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, iterations, &mut out),
        "SHA-256" => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, salt, iterations, &mut out),
        "SHA-384" => pbkdf2::pbkdf2_hmac::<sha2::Sha384>(password, salt, iterations, &mut out),
        "SHA-512" => pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, salt, iterations, &mut out),
        _ => return None,
    }
    Some(out)
}
/// HKDF with the named hash: `bits` of it.
pub(crate) fn hkdf(hash: &str, key: &[u8], salt: &[u8], info: &[u8], bits: usize) -> Option<Vec<u8>> {
    if bits == 0 || bits % 8 != 0 {
        return None;
    }
    let mut out = vec![0u8; bits / 8];
    match hash.to_ascii_uppercase().as_str() {
        "SHA-1" => hkdf::Hkdf::<sha1::Sha1>::new(Some(salt), key).expand(info, &mut out).ok()?,
        "SHA-256" => hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), key).expand(info, &mut out).ok()?,
        "SHA-384" => hkdf::Hkdf::<sha2::Sha384>::new(Some(salt), key).expand(info, &mut out).ok()?,
        "SHA-512" => hkdf::Hkdf::<sha2::Sha512>::new(Some(salt), key).expand(info, &mut out).ok()?,
        _ => return None,
    }
    Some(out)
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
        // NIST's AES-GCM test case 3 (no additional data): its ciphertext
        // and tag; and back.
        let key = (0..16).map(|i| [0xfe, 0xff, 0xe9, 0x92, 0x86, 0x65, 0x73, 0x1c, 0x6d, 0x6a, 0x8f, 0x94, 0x67, 0x30, 0x83, 0x08][i]).collect::<Vec<u8>>();
        let iv = [0xca, 0xfe, 0xba, 0xbe, 0xfa, 0xce, 0xdb, 0xad, 0xde, 0xca, 0xf8, 0x88];
        let plain = (0..64)
            .map(|i| {
                [
                    0xd9, 0x31, 0x32, 0x25, 0xf8, 0x84, 0x06, 0xe5, 0xa5, 0x59, 0x09, 0xc5, 0xaf, 0xf5, 0x26, 0x9a, 0x86, 0xa7, 0xa9, 0x53, 0x15, 0x34, 0xf7, 0xda, 0x2e, 0x4c, 0x30, 0x3d, 0x8a, 0x31,
                    0x8a, 0x72, 0x1c, 0x3c, 0x0c, 0x95, 0x95, 0x68, 0x09, 0x53, 0x2f, 0xcf, 0x0e, 0x24, 0x49, 0xa6, 0xb5, 0x25, 0xb1, 0x6a, 0xed, 0xf5, 0xaa, 0x0d, 0xe6, 0x57, 0xba, 0x63, 0x7b, 0x39,
                    0x1a, 0xaf, 0xd2, 0x55,
                ][i]
            })
            .collect::<Vec<u8>>();
        let sealed = super::aes("AES-GCM", true, &key, &iv, &plain, &[], 0).unwrap();
        assert_eq!(hex(sealed[64..].to_vec()), "4d5c2af327cd64a62cf35abd2ba6fab4");
        assert_eq!(hex(sealed[..4].to_vec()), "42831ec2");
        assert_eq!(super::aes("AES-GCM", false, &key, &iv, &sealed, &[], 0).unwrap(), plain);
        // CBC and CTR come back as they went.
        let iv16 = [7u8; 16];
        for mode in ["AES-CBC", "AES-CTR"] {
            let out = super::aes(mode, true, &key, &iv16, b"attack at dawn!!+", &[], 128).unwrap();
            assert_eq!(super::aes(mode, false, &key, &iv16, &out, &[], 128).unwrap(), b"attack at dawn!!+");
        }
        // RFC 6070's PBKDF2-HMAC-SHA1 test 2, and RFC 5869's HKDF test 1.
        assert_eq!(hex(super::pbkdf2("SHA-1", b"password", b"salt", 2, 160).unwrap()), "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957");
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0..13).collect();
        let info: Vec<u8> = (0xf0..0xfa).collect();
        assert_eq!(hex(super::hkdf("SHA-256", &ikm, &salt, &info, 336).unwrap()), "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
        let r = super::random(32);
        assert_eq!(r.len(), 32);
        assert_ne!(r, vec![0u8; 32]);
    }
}
