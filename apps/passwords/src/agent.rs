//! Passwords is the session's SSH agent (Ben, 2026-11-05: "build and enable
//! by default ssh agent support in olo passwords so that the ssh keys in it
//! are available for use automatically"): the SSH keys kept in it sign for
//! ssh, git and the rest, natively (the agent protocol,
//! draft-miller-ssh-agent; RustCrypto's `ssh-key` to read keys and sign), no
//! `ssh-agent`, no key ever written out.
//!
//! The socket is `olo_xdg::ssh_agent_socket()` (olo-session sets
//! `SSH_AUTH_SOCK` to it as the session starts). Each request of a program
//! of the person's own (its uid checked) goes to Passwords as a keychain
//! request ([`Request::SshKeys`], [`Request::SshSign`]) carrying the
//! program's process, so it's answered as `op`'s are: once unlocked, a
//! signature once the person allowed that terminal session (or the
//! program, outside one), and the keys themselves only in the store's
//! worker ([`keys`], [`sign`]).

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use olo_keychain::{Incoming, Request, Response, SshKey};
use onepassword::{ItemFieldType, ItemState};
use ssh_key::{
    HashAlg, PrivateKey,
    private::{EcdsaKeypair, Ed25519Keypair, KeypairData, RsaKeypair},
};

use crate::vault::Data;

// (The agent protocol's messages are read in `olo_keychain::agent`:
// Rex reads them the same way, for a host's terminals.)
use olo_keychain::agent::{MAX_MESSAGE, RSA_SHA2_256, RSA_SHA2_512};

/// Listen on `path`, each request going to `deliver` (Passwords' UI
/// thread) and its answer back to the program that asked.
pub fn serve(path: &Path, deliver: impl Fn(Incoming) + Send + Sync + 'static) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    // (Left by a Passwords that ended: taken over.)
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "another Passwords is the SSH agent here"));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let deliver = std::sync::Arc::new(deliver);
    std::thread::Builder::new().name("ssh-agent".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let deliver = deliver.clone();
            let _ = std::thread::Builder::new().name("ssh-agent client".into()).spawn(move || client(stream, &*deliver));
        }
    })?;
    Ok(())
}

/// The process at the other end of `stream`, if it's the person's own.
fn peer(stream: &UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED into a ucred of ours.
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    // SAFETY: getuid can't fail.
    (r == 0 && cred.uid == unsafe { libc::getuid() }).then_some(cred.pid)
}

/// One program's conversation: requests answered in turn till it hangs up.
fn client(mut stream: UnixStream, deliver: &(impl Fn(Incoming) + ?Sized)) {
    let Some(pid) = peer(&stream) else { return };
    let from = std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim().to_owned()).unwrap_or_else(|_| "a program".into());
    loop {
        let mut len = [0u8; 4];
        if stream.read_exact(&mut len).is_err() {
            return;
        }
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > MAX_MESSAGE {
            return;
        }
        let mut msg = vec![0u8; len];
        if stream.read_exact(&mut msg).is_err() {
            return;
        }
        let ask = |request: Request| -> Option<Response> {
            let (incoming, stream) = Incoming::local(request, from.clone(), pid).ok()?;
            deliver(incoming);
            olo_keychain::read_reply(&stream)
        };
        if stream.write_all(&olo_keychain::agent::framed(olo_keychain::agent::answer(&msg, None, &ask))).is_err() {
            return;
        }
    }
}

/// What follows from an SSH key, as 1Password shows it beside the private
/// key: its public key (OpenSSH's line), its fingerprint ("SHA256:…") and
/// its kind, in 1Password's words ("Ed25519", "RSA, 4096-bit"; and
/// "ECDSA P-256", which 1Password doesn't make).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub public: String,
    pub fingerprint: String,
    pub kind: String,
}

/// [`Derived`] of a key kept in a field's value, if it can be read: one
/// under a passphrase too, in OpenSSH's own format, whose public part is
/// kept in the clear.
pub fn derived(text: &str) -> Option<Derived> {
    let public = match private_key(text) {
        Ok(key) => key.public_key().clone(),
        Err(_) => PrivateKey::from_openssh(text.trim()).ok()?.public_key().clone(),
    };
    let line = public.to_openssh().ok()?;
    // (Without its comment: a field's key has the item's name for that.)
    let line = line.split(' ').take(2).collect::<Vec<_>>().join(" ");
    let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
    let kind = match public.key_data() {
        ssh_key::public::KeyData::Ed25519(_) => "Ed25519".to_owned(),
        ssh_key::public::KeyData::Rsa(rsa) => format!("RSA, {}-bit", rsa.key_size()),
        ssh_key::public::KeyData::Ecdsa(e) => match e.curve() {
            ssh_key::EcdsaCurve::NistP256 => "ECDSA P-256".to_owned(),
            ssh_key::EcdsaCurve::NistP384 => "ECDSA P-384".to_owned(),
            ssh_key::EcdsaCurve::NistP521 => "ECDSA P-521".to_owned(),
        },
        other => other.algorithm().to_string(),
    };
    Some(Derived { public: line, fingerprint, kind })
}

/// The fields 1Password lists beside SSH key field `f`, from [`Derived`]
/// `d`: "public key" and "fingerprint" (shown over it) and "key type"
/// (under it), by the ids and labels 1Password gives them (`op item get`
/// lists them; `op://vault/item/public key` reads one), in its section.
pub fn derived_fields_from(f: &onepassword::ItemField, d: &Derived) -> [onepassword::ItemField; 3] {
    let field = |id: &str, label: &str, value: &str| {
        let mut x = onepassword::ItemField::new(id, label, ItemFieldType::Text, value);
        x.section_id = f.section_id.clone();
        x
    };
    [field(PUBLIC_KEY, "public key", &d.public), field(FINGERPRINT, "fingerprint", &d.fingerprint), field(KEY_TYPE, "key type", &d.kind)]
}

/// The ids of [`derived_fields_from`]'s fields.
pub const PUBLIC_KEY: &str = "public_key";
pub const FINGERPRINT: &str = "fingerprint";
pub const KEY_TYPE: &str = "key_type";

/// Item `i`'s fields worked out from SSH key field `f` that it has none
/// of its own for (by id or label: one brought in with them keeps its
/// own).
pub fn derived_fields(i: &onepassword::Item, f: &onepassword::ItemField, d: &Derived) -> Vec<onepassword::ItemField> {
    derived_fields_from(f, d).into_iter().filter(|x| !i.fields.iter().any(|o| o.id == x.id || o.title.eq_ignore_ascii_case(&x.title))).collect()
}

/// Item `i`'s field `key` (an id, else a label) among those worked out
/// from its SSH key, for copying it and its reference and for `op read`.
pub fn derived_field(i: &onepassword::Item, key: &str) -> Option<onepassword::ItemField> {
    i.fields
        .iter()
        .filter(|f| f.field_type == ItemFieldType::SshKey && !f.value.is_empty())
        .filter_map(|f| derived(&f.value).map(|d| derived_fields(i, f, &d)))
        .flatten()
        .find(|x| x.id == key || x.title.eq_ignore_ascii_case(key))
}

/// An Ed25519 key under a passphrase ("secret"), in OpenSSH's format: a
/// throwaway made for tests and fixtures, used nowhere.
#[cfg(test)]
pub(crate) const LOCKED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABCD5l7k+3
0HzSOfi0dB9dlOAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIPF/l1l1Yjf1ZDo7
S9YRDQzknw0RUiTw94W3wGF8Pv31AAAAkAiPC/Gu4ajQTe7uMHuv4XDHy5w6DIdzbLhEMJ
3IqmJgLWnX3361/5TP3moMxF9W/3MSGgqNzsGiZF8Mk/c7lsJmRqv4VOsgoFTnFh8+QQ/X
lhxxC0UkUFGlN3MHwkfNf7Mpb1/aTFQbDSUUpX3mEwq6jV3uYEAJ68z6+TpOCge6AQoH32
hcZz1SEtYqv+wsSw==
-----END OPENSSH PRIVATE KEY-----
";

/// An RSA key of 2048 bits, in OpenSSH's format: a throwaway made for
/// tests and fixtures, used nowhere.
#[cfg(test)]
pub(crate) const RSA_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEA22TgGXPNdjBnXlkIc31Jvy7b8FhSLov7wrpDtj/3msqFfgkgG+91
Y5RF0ll26PsPaxGXIeUjrhb4C5mdBrxroLJOutVZgV5wzOCFD5xXDo08xTSAuJM8/jd8W/
fi7EaY3/E2muc2IcKx2Ue27zGKOwu9Jjr/0/csm6RfJ1FaYTmETh+o14+0JzcEUn4YczsA
RWlF/b5//i9Wr2OG+62HOyjtsQirzG+/+61Ya4BoJiRiMHG6nRX+oU67o8GlX3NyL9JcBp
+iP3U+iqAQtbXXfrQzaLTJQNYlLKDH2xVusoSjCH1SToJOtYO5OqX/WuZeHNVF9AphBTR2
DatnCQCIEwAAA7hxlTAWcZUwFgAAAAdzc2gtcnNhAAABAQDbZOAZc812MGdeWQhzfUm/Lt
vwWFIui/vCukO2P/eayoV+CSAb73VjlEXSWXbo+w9rEZch5SOuFvgLmZ0GvGugsk661VmB
XnDM4IUPnFcOjTzFNIC4kzz+N3xb9+LsRpjf8Taa5zYhwrHZR7bvMYo7C70mOv/T9yybpF
8nUVphOYROH6jXj7QnNwRSfhhzOwBFaUX9vn/+L1avY4b7rYc7KO2xCKvMb7/7rVhrgGgm
JGIwcbqdFf6hTrujwaVfc3Iv0lwGn6I/dT6KoBC1tdd+tDNotMlA1iUsoMfbFW6yhKMIfV
JOgk61g7k6pf9a5l4c1UX0CmEFNHYNq2cJAIgTAAAAAwEAAQAAAQAG6x9wrgjZ+Udz34ri
BB7B5fsO5o9w4v6FctTqd7SZ2lkV/bkC0yz4fux/aE1/pgB5Ioze3ML18nN6qirG05pGma
4x5hh0eUNFXVnU1tvynDGWZm7IuJvryFl7l6LoSzGhnjxX64KhfepDLSfE/eLwtGYP6+cK
tXi1T8b7Dm2zMyDW7cM4TQds83WBmAcwvYTbdo7KlqdRQPHeBkWeq4UEk1w7in3QeZTdTJ
pGTbIO2kRzowM27ungjsRP1A+TeuuOIcGS57Oob9ouRudUxDT6olJb7HbdWORWE/foUXbW
be3APXW4cS6HBziGbyWPbsB7hIvPTgsg90fSCuenEIzBAAAAgENtyfxem12vRJ6H3c9e5G
BlEbzr6dQWcMY92gUUKOa1JFqr/M5/Uju5S50+NnnijS8kap+qsw+djRiqV4LdXi0eUN84
ZH+sxwVUGVioj4IRM55Q4SWlwZXNrnd3UAveSXTwfCJwhErXZDSB9i5M3a6YFzHk+V1ugm
OvUriH2AGQAAAAgQD1TteFrkA6nA853GK/V/AF/8eI+bH9ddd4+SMO+b4IK7bUKhyy/hYL
QUkIOgKwaPtGA3xmzdStggmBdJ+siLLifsgj0E9fcTfQLdA0sBd1oH4scqhKpwMt+rUQjm
w1BmZ8by3Y/O3wlE7khbsPPiCVGinTXWk23RA9pWoJKD/XxwAAAIEA5PTidk/tZyFs2q6R
LRQaYzFjsFJSTVy1SoQGio3Z5hvzfG35rXST14z0t438L75GwpZlx4MEb+hbapXRHeRRCC
EFoasW9nvimTS1Zk2FZFLmyrq8ipkxj0VfP4WjDcCrAtIO+7nEZGE80F9noqmd06FDJiMe
DjO8EkGZruSxBVUAAAAAAQID
-----END OPENSSH PRIVATE KEY-----
";

/// An SSH key as kept in a field's value: OpenSSH's own format, or PKCS#8
/// (what 1Password makes: Ed25519, RSA, ECDSA P-256).
pub fn private_key(text: &str) -> Result<PrivateKey, String> {
    let text = text.trim();
    if text.contains("BEGIN OPENSSH PRIVATE KEY") {
        let key = PrivateKey::from_openssh(text).map_err(|e| e.to_string())?;
        if key.is_encrypted() {
            return Err("the key has a passphrase".into());
        }
        return Ok(key);
    }
    // (A key under a passphrase, in either of the older forms: it
    // can't be signed with here, and is said to be so.)
    if text.contains("BEGIN ENCRYPTED PRIVATE KEY") || text.contains("Proc-Type: 4,ENCRYPTED") {
        return Err("the key has a passphrase".into());
    }
    let form = ["BEGIN PRIVATE KEY", "BEGIN RSA PRIVATE KEY", "BEGIN EC PRIVATE KEY"].into_iter().find(|form| text.contains(form)).ok_or("not a private key Passwords can read")?;
    let body: String = text.lines().map(str::trim).filter(|l| !l.starts_with("-----") && !l.contains(':')).collect();
    let der = B64.decode(body.trim()).map_err(|e| e.to_string())?;
    // (A curve's key as SSH's: the point uncompressed, as OpenSSH writes it.)
    macro_rules! ecdsa {
        ($secret:expr, $curve:ident, $variant:ident) => {{
            use $curve::elliptic_curve::sec1::ToSec1Point as _;
            let secret: $curve::SecretKey = $secret;
            KeypairData::Ecdsa(EcdsaKeypair::$variant { public: secret.public_key().to_sec1_point(false), private: secret.into() })
        }};
    }
    let data = match form {
        // The classic forms a key file was written in before OpenSSH's
        // own (an `id_rsa` kept since then is one): PKCS#1 for RSA,
        // SEC1 for a curve's, which names its curve. (They were "not a
        // private key Passwords can read": three keys in Passwords and
        // one offered, Ben, 2026-11-23, "there are 3 keys in passwords
        // it should be offering them".)
        "BEGIN RSA PRIVATE KEY" => {
            let rsa = <rsa::RsaPrivateKey as rsa::pkcs1::DecodeRsaPrivateKey>::from_pkcs1_der(&der).map_err(|e| e.to_string())?;
            KeypairData::Rsa(RsaKeypair::try_from(rsa).map_err(|e| e.to_string())?)
        }
        "BEGIN EC PRIVATE KEY" => {
            if let Ok(secret) = p256::SecretKey::from_sec1_der(&der) {
                ecdsa!(secret, p256, NistP256)
            } else if let Ok(secret) = p384::SecretKey::from_sec1_der(&der) {
                ecdsa!(secret, p384, NistP384)
            } else if let Ok(secret) = p521::SecretKey::from_sec1_der(&der) {
                ecdsa!(secret, p521, NistP521)
            } else {
                return Err("a key on a curve Passwords can't sign with".into());
            }
        }
        _ => {
            // Ed25519's PKCS#8: a 16-byte head (its length, its version,
            // the Ed25519 OID, an octet string of an octet string), then
            // its 32-byte seed. Version 0 ends there (OpenSSL's); version
            // 1 has the public key after it, which is how 1Password
            // writes the keys it makes (and ring, and RustCrypto), with
            // another length in the head. Read by what the head says,
            // not as sixteen fixed bytes: a key 1Password made was "a
            // PKCS#8 key of a kind Passwords can't sign with", the third
            // of Ben's three (2026-11-23: "but it still doesn't work").
            const ED25519: [u8; 16] = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
            let ed25519 = der.len() >= 48 && der[0] == 0x30 && der[1] < 0x80 && der[2..4] == ED25519[2..4] && der[4] <= 1 && der[5..16] == ED25519[5..16];
            if ed25519 {
                let seed: [u8; 32] = der[16..48].try_into().map_err(|_| "a short Ed25519 key")?;
                KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed))
            } else if let Ok(rsa) = <rsa::RsaPrivateKey as rsa::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
                KeypairData::Rsa(RsaKeypair::try_from(rsa).map_err(|e| e.to_string())?)
            } else if let Ok(secret) = <p256::SecretKey as p256::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
                ecdsa!(secret, p256, NistP256)
            } else if let Ok(secret) = <p384::SecretKey as p384::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
                ecdsa!(secret, p384, NistP384)
            } else if let Ok(secret) = <p521::SecretKey as p521::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
                ecdsa!(secret, p521, NistP521)
            } else {
                return Err(unread(&der));
            }
        }
    };
    PrivateKey::new(data, "").map_err(|e| e.to_string())
}

/// Why a PKCS#8 key none of the readers took can't be signed with: its
/// kind, by what the key itself says it is (its algorithm, which is no
/// secret), so the log names what is missing instead of "a kind".
fn unread(der: &[u8]) -> String {
    let Ok(info) = p256::pkcs8::PrivateKeyInfoRef::try_from(der) else {
        return "a PKCS#8 key Passwords can't read (it isn't written as PKCS#8 is)".into();
    };
    let oid = info.algorithm.oid.to_string();
    let curve = info.algorithm.parameters_oid().map(|c| c.to_string()).unwrap_or_default();
    match oid.as_str() {
        "1.2.840.113549.1.1.1" => match <rsa::RsaPrivateKey as rsa::pkcs8::DecodePrivateKey>::from_pkcs8_der(der) {
            Err(why) => format!("an RSA key Passwords can't read ({why})"),
            Ok(_) => "an RSA key Passwords can't read".into(),
        },
        "1.2.840.10045.2.1" => format!("a key on a curve Passwords can't sign with ({})", if curve.is_empty() { "unnamed" } else { &curve }),
        "1.2.840.10040.4.1" => "a DSA key, which SSH hosts no longer take".into(),
        "1.3.101.113" => "an Ed448 key, which SSH has no signature for".into(),
        "1.3.101.112" => "an Ed25519 key written in a way Passwords can't read".into(),
        _ => format!("a PKCS#8 key of a kind Passwords can't sign with (algorithm {oid})"),
    }
}

/// The SSH keys in `data` (active items' SSH key fields), with each one's
/// item title, public key blob and field: the agent's identities.
fn ssh_fields(data: &Data) -> impl Iterator<Item = (&str, &onepassword::ItemField)> {
    data.entries
        .iter()
        .filter(|e| e.state == ItemState::Active)
        .flat_map(|e| e.item.fields.iter().filter(|f| f.field_type == ItemFieldType::SshKey && !f.value.is_empty()).map(move |f| (e.item.title.as_str(), f)))
}

/// What `SshKeys` answers: every SSH key Passwords can sign with.
pub fn keys(data: &Data) -> Vec<SshKey> {
    let mut out: Vec<SshKey> = Vec::new();
    for (title, f) in ssh_fields(data) {
        // (A key that can't be offered is said, by its item's name and
        // why: left out without a word, two of a person's three keys
        // were simply not there for a host, and nothing said so.)
        let key = match private_key(&f.value) {
            Ok(key) => key,
            Err(why) => {
                tracing::warn!(key = title, why, "an SSH key in Passwords can't be offered");
                continue;
            }
        };
        let Ok(blob) = encode_vec(key.public_key().key_data()) else { continue };
        let key = B64.encode(blob);
        if !out.iter().any(|k| k.key == key) {
            out.push(SshKey { key, comment: title.to_owned() });
        }
    }
    out
}

/// What `SshSign` answers: `data` signed with the key whose public key
/// blob is `key` (RSA with the SHA-2 `flags` ask for, else SHA-1 as an
/// old server wants), the signature in SSH's encoding; and the key's item.
pub fn sign(data: &Data, key: &[u8], message: &[u8], flags: u32) -> Result<(Vec<u8>, String), String> {
    use signature::Signer as _;
    for (title, f) in ssh_fields(data) {
        let Ok(private) = private_key(&f.value) else { continue };
        if encode_vec(private.public_key().key_data()).ok().as_deref() != Some(key) {
            continue;
        }
        let sig = match private.key_data() {
            KeypairData::Rsa(rsa) => {
                let hash = if flags & RSA_SHA2_512 != 0 {
                    Some(HashAlg::Sha512)
                } else if flags & RSA_SHA2_256 != 0 {
                    Some(HashAlg::Sha256)
                } else {
                    None
                };
                (rsa, hash).try_sign(message)
            }
            other => other.try_sign(message),
        }
        .map_err(|e| e.to_string())?;
        return Ok((encode_vec(&sig).map_err(|e| e.to_string())?, title.to_owned()));
    }
    Err("Passwords has no such SSH key".into())
}

/// An ssh-key value in SSH's own encoding.
fn encode_vec(v: &impl ssh_key::encoding::Encode) -> Result<Vec<u8>, ssh_key::Error> {
    let mut out = Vec::new();
    v.encode(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys made from fixed numbers (no random generator in tests).
    /// What follows from a key, as 1Password shows it: the public key's
    /// line without the comment, the SHA-256 fingerprint, the kind; for
    /// a key 1Password made (PKCS#8) as for OpenSSH's; nothing for what
    /// isn't a key.
    #[test]
    fn a_keys_public_key_fingerprint_and_kind() {
        let key = ed25519();
        let text = key.to_openssh(ssh_key::LineEnding::LF).unwrap();
        let d = derived(&text).expect("a key");
        assert_eq!(d.kind, "Ed25519");
        assert_eq!(d.public, key.public_key().to_openssh().unwrap().rsplit_once(' ').unwrap().0);
        assert!(d.public.starts_with("ssh-ed25519 AAAA") && !d.public.contains("test"), "{}", d.public);
        assert_eq!(d.fingerprint, key.public_key().fingerprint(HashAlg::Sha256).to_string());
        assert!(d.fingerprint.starts_with("SHA256:"));
        let p256 = derived(&p256_key().to_openssh(ssh_key::LineEnding::LF).unwrap()).expect("a P-256 key");
        assert_eq!(p256.kind, "ECDSA P-256");
        assert_eq!(derived("not a key"), None);
        // A key under a passphrase: its public part all the same, as
        // `ssh-keygen -lf` gives it.
        let locked = LOCKED_KEY;
        assert!(private_key(locked).is_err());
        let d = derived(locked).expect("its public part");
        assert_eq!(d.public, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPF/l1l1Yjf1ZDo7S9YRDQzknw0RUiTw94W3wGF8Pv31");
        assert_eq!(d.fingerprint, "SHA256:D28AeCdq4A9yMQ6UOvCrgZn7Hrtebl3lcaYCA8y4m7s");
        assert_eq!(d.kind, "Ed25519");
        // An RSA key, its size in 1Password's words.
        let rsa = derived(RSA_KEY).expect("an RSA key");
        assert_eq!(rsa.kind, "RSA, 2048-bit");
        assert_eq!(rsa.fingerprint, "SHA256:RX2UHRZIZPCaDQvRiYlrJot8Vl4bML51sAJBn9IRYRg");
        assert_eq!(
            rsa.public,
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDbZOAZc812MGdeWQhzfUm/LtvwWFIui/vCukO2P/eayoV+CSAb73VjlEXSWXbo+w9rEZch5SOuFvgLmZ0GvGugsk661VmBXnDM4IUPnFcOjTzFNIC4kzz+N3xb9+LsRpjf8Taa5zYhwrHZR7bvMYo7C70mOv/T9yybpF8nUVphOYROH6jXj7QnNwRSfhhzOwBFaUX9vn/+L1avY4b7rYc7KO2xCKvMb7/7rVhrgGgmJGIwcbqdFf6hTrujwaVfc3Iv0lwGn6I/dT6KoBC1tdd+tDNotMlA1iUsoMfbFW6yhKMIfVJOgk61g7k6pf9a5l4c1UX0CmEFNHYNq2cJAIgT"
        );
    }

    fn ed25519() -> PrivateKey {
        PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[3u8; 32])), "test").unwrap()
    }

    fn p256_key() -> PrivateKey {
        use p256::elliptic_curve::sec1::ToSec1Point as _;
        let secret = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        PrivateKey::new(KeypairData::Ecdsa(EcdsaKeypair::NistP256 { public: secret.public_key().to_sec1_point(false), private: secret.into() }), "test").unwrap()
    }

    /// A curve's key in the two older forms a key is kept in (SEC1,
    /// "EC PRIVATE KEY"; PKCS#8, "PRIVATE KEY"), on each of SSH's three
    /// curves: read, and signed with. (On P-384 and P-521 they were no
    /// key at all, and SEC1 on any.)
    /// Ed25519's PKCS#8 with the public key after the seed (version 1:
    /// what 1Password writes for a key it made) is the key its seed
    /// makes, in both ways the public key is written; and a kind that
    /// can't be signed with is named.
    #[test]
    fn an_ed25519_key_with_its_public_key_after_it_is_read() {
        let seed: Vec<u8> = (1..=32).collect();
        let head = |len: u8, version: u8| [vec![0x30, len, 0x02, 0x01, version, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20], seed.clone()].concat();
        let pem = |der: Vec<u8>| format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", B64.encode(der));
        let plain = private_key(&pem(head(0x2e, 0))).expect("version 0");
        let public = match plain.key_data() {
            KeypairData::Ed25519(pair) => pair.public.0.to_vec(),
            _ => panic!("not Ed25519"),
        };
        // (RFC 5958's: the public key as [1], a bit string. And ring's
        // and older RustCrypto's: [1] holding a bit string.)
        let implicit = [head(0x51, 1), vec![0x81, 0x21, 0x00], public.clone()].concat();
        let explicit = [head(0x53, 1), vec![0xa1, 0x23, 0x03, 0x21, 0x00], public.clone()].concat();
        for (name, der) in [("implicit", implicit), ("explicit", explicit)] {
            let key = private_key(&pem(der)).unwrap_or_else(|why| panic!("{name}: {why}"));
            assert_eq!(key.public_key(), plain.public_key(), "{name}");
            use signature::Signer as _;
            assert!(key.key_data().try_sign(b"x").is_ok(), "{name}");
        }
        // A DSA key (its algorithm, then nothing a reader would take):
        // said to be one.
        let dsa = vec![0x30, 0x14, 0x02, 0x01, 0x00, 0x30, 0x0b, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x01, 0x05, 0x00, 0x04, 0x02, 0x04, 0x00];
        assert_eq!(private_key(&pem(dsa)).unwrap_err(), "a DSA key, which SSH hosts no longer take");
        let other = vec![0x30, 0x0e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x02, 0x04, 0x00];
        assert_eq!(private_key(&pem(other)).unwrap_err(), "a PKCS#8 key of a kind Passwords can't sign with (algorithm 1.3.101.110)");
    }

    #[test]
    fn a_curves_key_is_read_in_its_older_forms() {
        use signature::Signer as _;
        let pem = |label: &str, der: &[u8]| format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", B64.encode(der));
        macro_rules! both {
            ($curve:ident, $bytes:expr) => {{
                use $curve::pkcs8::EncodePrivateKey as _;
                let secret = $curve::SecretKey::from_slice(&$bytes).unwrap();
                vec![pem("EC PRIVATE KEY", &secret.to_sec1_der().unwrap()), pem("PRIVATE KEY", secret.to_pkcs8_der().unwrap().as_bytes())]
            }};
        }
        let mut p521 = [9u8; 66];
        p521[0] = 0;
        for (curve, forms) in [("nistp256", both!(p256, [9u8; 32])), ("nistp384", both!(p384, [9u8; 48])), ("nistp521", both!(p521, p521))] {
            for text in forms {
                let key = private_key(&text).unwrap_or_else(|why| panic!("{curve}: {why}"));
                assert!(key.algorithm().to_string().ends_with(curve), "{}", key.algorithm());
                assert!(key.key_data().try_sign(b"x").is_ok(), "{curve} signs");
            }
        }
        // (Under a passphrase, in the older forms: said, not taken for something else.)
        assert_eq!(private_key("-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----").unwrap_err(), "the key has a passphrase");
        assert_eq!(private_key("-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----").unwrap_err(), "the key has a passphrase");
    }

    /// Every kind of key in each form a private key comes in, as
    /// ssh-keygen writes them (RSA's classic form too, which the test
    /// above has none of): `OLO_KEYS_PROBE`, a folder of keys made with
    /// `ssh-keygen -t KIND -b BITS -N "" -f NAME`, some rewritten with
    /// `ssh-keygen -p -N "" -m PKCS8|PEM -f NAME`. Read, and signed
    /// with, every one (21 of 21 on 2026-11-23: RSA of 2048, 3072 and
    /// 4096 bits, ECDSA on the three curves and Ed25519, each as
    /// OpenSSH's own, PKCS#8 and PEM).
    #[test]
    fn every_kind_of_key_in_every_form_is_read() {
        let Some(dir) = std::env::var_os("OLO_KEYS_PROBE") else { return };
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).filter(|n| !n.ends_with(".pub")).collect();
        names.sort();
        let mut refused = Vec::new();
        for name in names {
            let text = std::fs::read_to_string(std::path::Path::new(&dir).join(&name)).unwrap();
            let read = private_key(&text);
            let signs = read.as_ref().ok().map(|key| {
                use signature::Signer as _;
                match key.key_data() {
                    KeypairData::Rsa(rsa) => (rsa, Some(HashAlg::Sha512)).try_sign(b"x").is_ok(),
                    other => other.try_sign(b"x").is_ok(),
                }
            });
            eprintln!(
                "{name}: {}",
                match (&read, signs) {
                    (Ok(_), Some(true)) => "read, signs".to_owned(),
                    (Ok(_), _) => "read, CAN'T SIGN".to_owned(),
                    (Err(why), _) => format!("REFUSED: {why}"),
                }
            );
            if !matches!(signs, Some(true)) {
                refused.push(name);
            }
        }
        assert!(refused.is_empty(), "refused: {refused:?}");
    }

    #[test]
    fn reads_openssh_keys() {
        for key in [ed25519(), p256_key()] {
            let text = key.to_openssh(ssh_key::LineEnding::LF).unwrap();
            assert_eq!(private_key(&text).unwrap().public_key().key_data(), key.public_key().key_data());
        }
        assert!(private_key("hunter2").is_err());
    }

    #[test]
    fn lists_and_signs_with_a_vaults_keys() {
        use crate::vault::Entry;
        use onepassword::{Item, ItemCategory, ItemField};
        let mut data = Data::default();
        for (title, key) in [("GitHub", ed25519()), ("Server", p256_key())] {
            let now = chrono::Utc::now();
            let item = Item {
                id: title.to_lowercase(),
                title: title.into(),
                category: ItemCategory::SshKey,
                vault_id: "v1".into(),
                fields: vec![ItemField::new("private_key", "private key", ItemFieldType::SshKey, key.to_openssh(ssh_key::LineEnding::LF).unwrap().as_str())],
                sections: vec![],
                notes: String::new(),
                tags: vec![],
                websites: vec![],
                version: 1,
                files: vec![],
                document: None,
                created_at: now,
                updated_at: now,
            };
            data.entries.push(Entry { item, state: ItemState::Active, remote: None, dirty: false, rev: 0 });
        }
        let listed = keys(&data);
        assert_eq!(listed.iter().map(|k| k.comment.as_str()).collect::<Vec<_>>(), ["GitHub", "Server"]);
        for (k, key) in listed.iter().zip([ed25519(), p256_key()]) {
            let blob = B64.decode(&k.key).unwrap();
            let (sig, _) = sign(&data, &blob, b"session to sign", 0).unwrap();
            let sig = ssh_key::Signature::try_from(&sig[..]).unwrap();
            signature::Verifier::verify(key.public_key().key_data(), b"session to sign", &sig).unwrap();
        }
        assert!(sign(&data, b"no such key", b"x", 0).is_err());
    }

    #[test]
    fn reads_an_ed25519_pkcs8_key() {
        // A 1Password-style key: PKCS#8, its seed after the fixed head.
        let seed = [7u8; 32];
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
        der.extend_from_slice(&seed);
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", B64.encode(&der));
        let key = private_key(&pem).unwrap();
        assert_eq!(key.public_key().key_data(), &ssh_key::public::KeyData::from(Ed25519Keypair::from_seed(&seed).public));
    }
}
