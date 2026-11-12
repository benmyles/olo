//! Pair-setup and pair-verify (HomeKit Accessory Protocol, as AirPlay 2
//! carries it).

use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead as _, KeyInit as _},
};
use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use sha2::Sha512;
use zeroize::Zeroize as _;

use super::{
    srp,
    tlv8::{self, Tlv},
};

/// Where the pairing's messages go: the receiver's AirPlay port, by
/// HTTP ([`super::Http`]); a test's own receiver.
pub trait Post {
    fn post(&mut self, path: &str, body: &[u8]) -> Result<Vec<u8>, Error>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The receiver couldn't be reached, or went away.
    Io(String),
    /// The code typed isn't the one it shows.
    WrongCode,
    /// It wants to be left alone for this many seconds (too many tries).
    BackOff(u16),
    /// It refused, with HomeKit's error number (4: it has as many
    /// senders paired as it keeps; 6: not now; 7: busy with another).
    Refused(u8),
    /// What answered isn't what was paired with (its proof of its key
    /// didn't hold): nothing is sent to it.
    Imposter,
    /// It doesn't know us any more (it was reset, or we were removed
    /// from it): to be paired with again.
    Forgotten,
    /// An answer that isn't pairing's.
    Unexpected(&'static str),
    /// It answered the request with an HTTP error.
    Status(u16),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "the receiver couldn't be reached ({e})"),
            Error::WrongCode => write!(f, "that isn't the code it shows"),
            Error::BackOff(s) => write!(f, "too many tries: it takes no more for {s} seconds"),
            Error::Refused(4) => write!(f, "it has as many senders paired as it keeps"),
            Error::Refused(n) => write!(f, "it refused (HomeKit's error {n})"),
            Error::Imposter => write!(f, "what answered isn't the receiver that was paired with"),
            Error::Forgotten => write!(f, "it doesn't know this computer any more: pair with it again"),
            Error::Unexpected(what) => write!(f, "it answered with something that isn't pairing's ({what})"),
            Error::Status(code) => write!(f, "it answered with an error ({code})"),
        }
    }
}

impl std::error::Error for Error {}

/// What a pairing leaves this side with: who the receiver is and its
/// long-term key, and our name to it and our own key.
#[derive(Clone, PartialEq)]
pub struct Credentials {
    pub receiver_id: Vec<u8>,
    pub receiver_key: [u8; 32],
    pub our_id: Vec<u8>,
    /// Ed25519's seed: who holds it is us, to that receiver.
    pub our_secret: [u8; 32],
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials for {}", String::from_utf8_lossy(&self.receiver_id))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>()).flatten()
}

impl Credentials {
    /// In a line, to keep: the receiver's key, ours, its id, ours, each
    /// in hexadecimal with colons between (the form pyatv keeps them
    /// in, so a pairing made there can be brought here).
    pub fn to_line(&self) -> String {
        format!("{}:{}:{}:{}", hex(&self.receiver_key), hex(&self.our_secret), hex(&self.receiver_id), hex(&self.our_id))
    }

    pub fn from_line(line: &str) -> Option<Credentials> {
        let mut parts = line.trim().split(':').map(unhex);
        let (receiver_key, our_secret, receiver_id, our_id) = (parts.next()??, parts.next()??, parts.next()??, parts.next()??);
        Some(Credentials { receiver_id, receiver_key: receiver_key.try_into().ok()?, our_id, our_secret: our_secret.try_into().ok()? })
    }
}

pub(crate) fn derive(secret: &[u8], salt: &str, info: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha512>::new(Some(salt.as_bytes()), secret).expand(info.as_bytes(), &mut key).expect("32 bytes of HKDF");
    key
}

/// The nonce of a pairing message: its eight letters ("PS-Msg05").
fn named(label: &[u8; 8]) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(label);
    Nonce::from(n)
}

pub(super) fn seal(key: &[u8; 32], label: &[u8; 8], text: &[u8]) -> Vec<u8> {
    ChaCha20Poly1305::new(key.into()).encrypt(&named(label), text).expect("sealing a pairing message")
}

pub(super) fn open(key: &[u8; 32], label: &[u8; 8], sealed: &[u8]) -> Option<Vec<u8>> {
    ChaCha20Poly1305::new(key.into()).decrypt(&named(label), sealed).ok()
}

pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("the system's randomness");
    out
}

/// A name of our own for a receiver to know us by: a UUID's form.
fn new_id() -> Vec<u8> {
    let mut b = random::<16>();
    (b[6], b[8]) = (b[6] & 0x0f | 0x40, b[8] & 0x3f | 0x80);
    let h = hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]).into_bytes()
}

/// An answer's TLV, with its refusals read: the state it must be at.
fn answer(body: &[u8], state: u8) -> Result<Tlv, Error> {
    let tlv = Tlv::read(body).ok_or(Error::Unexpected("not TLV8"))?;
    if let Some(error) = tlv.byte(tlv8::ERROR) {
        return Err(match error {
            2 => Error::WrongCode,
            3 => Error::BackOff(tlv.get(tlv8::BACK_OFF).map_or(0, |b| u16::from_le_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]))),
            other => Error::Refused(other),
        });
    }
    if tlv.byte(tlv8::STATE) != Some(state) {
        return Err(Error::Unexpected("out of step"));
    }
    Ok(tlv)
}

/// {"name": name} as OPACK has it (Apple's packing of such things): what
/// the receiver lists this sender as.
fn packed_name(name: &str) -> Vec<u8> {
    let mut end = name.len().min(32);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name.as_bytes()[..end];
    let mut out = vec![0xe1, 0x44];
    out.extend(b"name");
    out.push(0x40 + name.len() as u8);
    out.extend(name);
    out
}

/// A pairing begun: the receiver is showing its code.
pub struct Setup {
    salt: Vec<u8>,
    their_public: Vec<u8>,
}

/// A pairing made.
#[derive(Debug, Clone, PartialEq)]
pub struct Paired {
    pub credentials: Credentials,
    /// The receiver signed for its long-term key as HomeKit says it
    /// does. (If not, the pairing still stands on the code: both sides
    /// proved they knew it. Said, to be looked into.)
    pub signed: bool,
}

impl Setup {
    /// Ask the receiver to show its code, and begin.
    pub fn begin(to: &mut dyn Post) -> Result<Setup, Error> {
        to.post("/pair-pin-start", &[])?;
        let m1 = Tlv::default().with(tlv8::METHOD, [0]).with(tlv8::STATE, [1]);
        let m2 = answer(&to.post("/pair-setup", &m1.write())?, 2)?;
        let salt = m2.get(tlv8::SALT).ok_or(Error::Unexpected("no salt"))?.to_vec();
        let their_public = m2.get(tlv8::PUBLIC_KEY).ok_or(Error::Unexpected("no key"))?.to_vec();
        Ok(Setup { salt, their_public })
    }

    /// With the code the receiver shows (`code`) and the name it is to
    /// know this computer by: the pairing.
    pub fn finish(self, to: &mut dyn Post, code: &str, name: &str) -> Result<Paired, Error> {
        let mut secret = random::<32>();
        let client = srp::Client::new(&secret, &self.salt, &self.their_public, code.trim());
        secret.zeroize();
        let client = client.ok_or(Error::Unexpected("a key no receiver sends"))?;
        let m3 = Tlv::default().with(tlv8::STATE, [3]).with(tlv8::PUBLIC_KEY, client.public.clone()).with(tlv8::PROOF, client.proof);
        let m4 = answer(&to.post("/pair-setup", &m3.write())?, 4)?;
        if !m4.get(tlv8::PROOF).is_some_and(|p| client.answered(p)) {
            // (It took our proof and can't prove it knew the code itself.)
            return Err(Error::Imposter);
        }
        // Our long-term key and name, signed, under the session's key.
        let our_secret = random::<32>();
        let signing = SigningKey::from_bytes(&our_secret);
        let (our_id, our_public) = (new_id(), signing.verifying_key().to_bytes());
        let x = derive(&client.key, "Pair-Setup-Controller-Sign-Salt", "Pair-Setup-Controller-Sign-Info");
        let signature = signing.sign(&[&x[..], &our_id, &our_public].concat());
        let inner = Tlv::default().with(tlv8::IDENTIFIER, our_id.clone()).with(tlv8::PUBLIC_KEY, our_public).with(tlv8::SIGNATURE, signature.to_bytes()).with(NAME, packed_name(name));
        let session = derive(&client.key, "Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info");
        let m5 = Tlv::default().with(tlv8::STATE, [5]).with(tlv8::ENCRYPTED_DATA, seal(&session, b"PS-Msg05", &inner.write()));
        let m6 = answer(&to.post("/pair-setup", &m5.write())?, 6)?;
        // Its long-term key and name, the same way.
        let theirs = m6.get(tlv8::ENCRYPTED_DATA).and_then(|sealed| open(&session, b"PS-Msg06", sealed)).and_then(|text| Tlv::read(&text)).ok_or(Error::Unexpected("its key couldn't be read"))?;
        let receiver_id = theirs.get(tlv8::IDENTIFIER).ok_or(Error::Unexpected("no name of its own"))?.to_vec();
        let receiver_key: [u8; 32] = theirs.get(tlv8::PUBLIC_KEY).and_then(|k| k.try_into().ok()).ok_or(Error::Unexpected("no key of its own"))?;
        let y = derive(&client.key, "Pair-Setup-Accessory-Sign-Salt", "Pair-Setup-Accessory-Sign-Info");
        let signed = signed_by(&receiver_key, &[&y[..], &receiver_id, &receiver_key].concat(), theirs.get(tlv8::SIGNATURE));
        Ok(Paired { credentials: Credentials { receiver_id, receiver_key, our_id, our_secret }, signed })
    }
}

/// The name a sender gives itself in pair-setup (Apple's own type).
pub(super) const NAME: u8 = 0x11;

pub(super) fn signed_by(key: &[u8; 32], what: &[u8], signature: Option<&[u8]>) -> bool {
    let (Ok(key), Some(Ok(signature))) = (VerifyingKey::from_bytes(key), signature.map(Signature::from_slice)) else { return false };
    key.verify_strict(what, &signature).is_ok()
}

/// A connection each side has proved itself on: what keys come of it.
pub struct Session {
    shared: [u8; 32],
}

impl Session {
    /// A key of this connection's, by HomeKit's salt and purpose
    /// ("Control-Salt", "Control-Write-Encryption-Key").
    pub fn key(&self, salt: &str, info: &str) -> [u8; 32] {
        derive(&self.shared, salt, info)
    }

    /// The cipher for the connection it was proved on: what this side
    /// writes and reads from here on.
    pub fn control(&self) -> super::Cipher {
        super::Cipher::new(&self.key("Control-Salt", "Control-Write-Encryption-Key"), &self.key("Control-Salt", "Control-Read-Encryption-Key"))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.zeroize();
    }
}

/// Prove to the receiver that we are who it paired with, and have it
/// prove it is the one we paired with (`c`).
pub fn verify(to: &mut dyn Post, c: &Credentials) -> Result<Session, Error> {
    let mut secret = random::<32>();
    let our_public = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
    let m1 = Tlv::default().with(tlv8::STATE, [1]).with(tlv8::PUBLIC_KEY, our_public);
    let m2 = answer(&to.post("/pair-verify", &m1.write())?, 2)?;
    let their_public: [u8; 32] = m2.get(tlv8::PUBLIC_KEY).and_then(|k| k.try_into().ok()).ok_or(Error::Unexpected("no key for the connection"))?;
    let shared = MontgomeryPoint(their_public).mul_clamped(secret).to_bytes();
    secret.zeroize();
    let key = derive(&shared, "Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info");
    let theirs = m2.get(tlv8::ENCRYPTED_DATA).and_then(|sealed| open(&key, b"PV-Msg02", sealed)).and_then(|text| Tlv::read(&text)).ok_or(Error::Imposter)?;
    // It is the receiver we paired with: its name, and its long-term
    // key's signature over both of this connection's keys.
    if theirs.get(tlv8::IDENTIFIER) != Some(&c.receiver_id) || !signed_by(&c.receiver_key, &[&their_public[..], &c.receiver_id, &our_public].concat(), theirs.get(tlv8::SIGNATURE)) {
        return Err(Error::Imposter);
    }
    let signature = SigningKey::from_bytes(&c.our_secret).sign(&[&our_public[..], &c.our_id, &their_public].concat());
    let inner = Tlv::default().with(tlv8::IDENTIFIER, c.our_id.clone()).with(tlv8::SIGNATURE, signature.to_bytes());
    let m3 = Tlv::default().with(tlv8::STATE, [3]).with(tlv8::ENCRYPTED_DATA, seal(&key, b"PV-Msg03", &inner.write()));
    match answer(&to.post("/pair-verify", &m3.write())?, 4) {
        Ok(_) => Ok(Session { shared }),
        // (It read our proof and doesn't have us: removed, or reset.)
        Err(Error::WrongCode) => Err(Error::Forgotten),
        Err(e) => Err(e),
    }
}
