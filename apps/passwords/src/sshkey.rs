//! SSH keys made and brought in here, kept as 1Password keeps them:
//! PKCS#8, Ed25519's with its public key after its seed (how 1Password
//! writes the keys it makes), so a key made or imported in Passwords
//! shows in 1Password as one made there does, its public key,
//! fingerprint and type worked out from it (Ben, 2026-11-22: "olo
//! passwords should show/have/sync all the same stuff as 1p for ssh
//! keys"). 1Password makes Ed25519 keys and RSA ones of 2048, 3072 and
//! 4096 bits, and reads a key file, one under a passphrase too: so does
//! this. (A curve's key, which 1Password doesn't take, is kept as
//! OpenSSH's own text: the agent signs with it here all the same.)

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use ssh_key::{LineEnding, PrivateKey, private::KeypairData};

/// The kinds of key Passwords makes, as 1Password's Generate offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ed25519,
    /// Its size in bits.
    Rsa(usize),
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Ed25519, Kind::Rsa(2048), Kind::Rsa(3072), Kind::Rsa(4096)];

    /// As 1Password names it, and as the key's type row says it.
    pub fn label(self) -> String {
        match self {
            Kind::Ed25519 => "Ed25519".into(),
            Kind::Rsa(bits) => format!("RSA, {bits}-bit"),
        }
    }
}

/// A new key of `kind`, as Passwords keeps it. An RSA key takes a while
/// (a second or more at 4096 bits): on a worker thread.
pub fn generate(kind: Kind) -> Result<String, String> {
    match kind {
        Kind::Ed25519 => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
            let text = ed25519_pkcs8(&seed);
            seed.fill(0);
            Ok(text)
        }
        Kind::Rsa(bits) => {
            let key = rsa::RsaPrivateKey::new(&mut SystemRandom, bits).map_err(|e| e.to_string())?;
            rsa_pkcs8(&key)
        }
    }
}

/// Why a key file wasn't brought in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// It's under a passphrase: to ask for (again, when `wrong`).
    Passphrase { wrong: bool },
    /// It isn't a key Passwords reads, and why.
    Unread(String),
}

/// Key file `text` as Passwords keeps it, opened with `passphrase` when
/// it's under one (OpenSSH's own format; the older forms under one are
/// refused with how to open them).
pub fn import(text: &str, passphrase: Option<&str>) -> Result<String, Refused> {
    let text = text.trim();
    let key = match crate::agent::private_key(text) {
        Ok(key) => key,
        Err(why) if text.contains("BEGIN OPENSSH PRIVATE KEY") => {
            let locked = PrivateKey::from_openssh(text).map_err(|e| Refused::Unread(e.to_string()))?;
            if !locked.is_encrypted() {
                return Err(Refused::Unread(why));
            }
            let Some(passphrase) = passphrase else { return Err(Refused::Passphrase { wrong: false }) };
            locked.decrypt(passphrase).map_err(|_| Refused::Passphrase { wrong: true })?
        }
        Err(why) => return Err(Refused::Unread(why)),
    };
    kept(&key).map_err(Refused::Unread)
}

/// A key typed or pasted into a field, kept as one made or imported is
/// (PKCS#8 for Ed25519 and RSA); anything else as it was (one under a
/// passphrase says so where it is).
pub fn normalized(value: &str) -> String {
    match crate::agent::private_key(value) {
        Ok(key) => kept(&key).unwrap_or_else(|_| value.to_owned()),
        Err(_) => value.to_owned(),
    }
}

/// Key `key` as Passwords keeps it: PKCS#8 for Ed25519 and RSA (as
/// 1Password does), OpenSSH's own text for a curve's.
pub fn kept(key: &PrivateKey) -> Result<String, String> {
    match key.key_data() {
        KeypairData::Ed25519(pair) => Ok(ed25519_pkcs8(&pair.private.to_bytes())),
        KeypairData::Rsa(pair) => rsa_pkcs8(&rsa::RsaPrivateKey::try_from(pair).map_err(|e| e.to_string())?),
        _ => key.to_openssh(LineEnding::LF).map(|t| t.to_string()).map_err(|e| e.to_string()),
    }
}

/// An Ed25519 key from its seed as PKCS#8 version 1 (RFC 5958's
/// OneAsymmetricKey), its public key after the seed: how 1Password, ring
/// and RustCrypto write one.
fn ed25519_pkcs8(seed: &[u8; 32]) -> String {
    let public = ssh_key::private::Ed25519Keypair::from_seed(seed).public;
    let mut der = vec![0x30, 0x51, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
    der.extend_from_slice(seed);
    der.extend_from_slice(&[0x81, 0x21, 0x00]);
    der.extend_from_slice(public.as_ref());
    let text = pem("PRIVATE KEY", &der);
    der.fill(0);
    text
}

fn rsa_pkcs8(key: &rsa::RsaPrivateKey) -> Result<String, String> {
    <rsa::RsaPrivateKey as rsa::pkcs8::EncodePrivateKey>::to_pkcs8_pem(key, rsa::pkcs8::LineEnding::LF).map(|t| t.to_string()).map_err(|e| e.to_string())
}

/// `der` as PEM's text, in lines of 64.
fn pem(label: &str, der: &[u8]) -> String {
    let body = B64.encode(der);
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// The system's random source, as the RSA crate asks for one.
struct SystemRandom;

impl rand_core::TryRng for SystemRandom {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst).expect("the system's random source failed");
        Ok(())
    }
}

impl rand_core::TryCryptoRng for SystemRandom {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key made here is one the agent signs with and 1Password reads:
    /// PKCS#8, Ed25519's with its public key, its public parts as they
    /// should be; two made are two keys.
    #[test]
    fn a_key_made_here() {
        let text = generate(Kind::Ed25519).unwrap();
        assert!(text.starts_with("-----BEGIN PRIVATE KEY-----\n"), "{text}");
        let key = crate::agent::private_key(&text).unwrap();
        let der = B64.decode(text.lines().filter(|l| !l.starts_with("-----")).collect::<String>()).unwrap();
        assert_eq!(der.len(), 83);
        assert_eq!(&der[51..], key.public_key().key_data().ed25519().unwrap().as_ref(), "its public key after its seed");
        assert_eq!(crate::agent::derived(&text).unwrap().kind, "Ed25519");
        assert_ne!(generate(Kind::Ed25519).unwrap(), text);
        let rsa = generate(Kind::Rsa(2048)).unwrap();
        assert!(rsa.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        assert_eq!(crate::agent::derived(&rsa).unwrap().kind, "RSA, 2048-bit");
    }

    /// What making a key of each kind and opening one under a passphrase
    /// take, each on a worker thread (three of each: an RSA key's primes
    /// are found by chance). `cargo test --release -p olo-passwords
    /// key_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn key_cost() {
        for kind in Kind::ALL {
            let took: Vec<_> = (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    generate(kind).unwrap();
                    t.elapsed()
                })
                .collect();
            println!("{}: {took:?}", kind.label());
        }
        let t = std::time::Instant::now();
        import(crate::agent::LOCKED_KEY, Some("secret")).unwrap();
        println!("opened under a passphrase: {:?}", t.elapsed());
    }

    /// A key file brought in: kept as PKCS#8, the same key; one under a
    /// passphrase asked for it, refused a wrong one, opened with the
    /// right one; what isn't a key refused, saying why.
    #[test]
    fn a_key_file_brought_in() {
        let ed = ssh_key::PrivateKey::new(KeypairData::Ed25519(ssh_key::private::Ed25519Keypair::from_seed(&[4u8; 32])), "me@there").unwrap();
        let file = ed.to_openssh(LineEnding::LF).unwrap();
        let kept = import(&file, None).unwrap();
        assert!(kept.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(crate::agent::private_key(&kept).unwrap().public_key().key_data(), ed.public_key().key_data());
        let rsa = import(crate::agent::RSA_KEY, None).unwrap();
        assert!(rsa.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(crate::agent::derived(&rsa), crate::agent::derived(crate::agent::RSA_KEY));
        assert_eq!(import(crate::agent::LOCKED_KEY, None), Err(Refused::Passphrase { wrong: false }));
        assert_eq!(import(crate::agent::LOCKED_KEY, Some("wrong")), Err(Refused::Passphrase { wrong: true }));
        let opened = import(crate::agent::LOCKED_KEY, Some("secret")).unwrap();
        assert_eq!(crate::agent::derived(&opened), crate::agent::derived(crate::agent::LOCKED_KEY));
        assert!(matches!(import("hello", None), Err(Refused::Unread(_))));
    }
}
