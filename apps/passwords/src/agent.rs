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

// The agent protocol's messages (draft-miller-ssh-agent, section 6).
const FAILURE: u8 = 5;
const REQUEST_IDENTITIES: u8 = 11;
const IDENTITIES_ANSWER: u8 = 12;
const SIGN_REQUEST: u8 = 13;
const SIGN_RESPONSE: u8 = 14;
/// Sign-request flags: RSA with SHA-2.
const RSA_SHA2_256: u32 = 2;
const RSA_SHA2_512: u32 = 4;
/// The most a message may be (OpenSSH's own limit).
const MAX_MESSAGE: usize = 256 * 1024;

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
        let answer = answer(&msg, pid, &from, deliver).unwrap_or_else(|| vec![FAILURE]);
        let mut out = (answer.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&answer);
        if stream.write_all(&out).is_err() {
            return;
        }
    }
}

/// A string as the agent protocol encodes it: its length, then it.
fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The next string in `msg` from `at`.
fn take<'a>(msg: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    let len = u32::from_be_bytes(msg.get(*at..*at + 4)?.try_into().ok()?) as usize;
    let s = msg.get(*at + 4..*at + 4 + len)?;
    *at += 4 + len;
    Some(s)
}

/// The answer to one message (None: a failure).
fn answer(msg: &[u8], pid: i32, from: &str, deliver: &(impl Fn(Incoming) + ?Sized)) -> Option<Vec<u8>> {
    let ask = |request: Request| -> Option<Response> {
        let (incoming, stream) = Incoming::local(request, from.to_owned(), pid).ok()?;
        deliver(incoming);
        olo_keychain::read_reply(&stream)
    };
    match msg[0] {
        REQUEST_IDENTITIES => {
            let Some(Response::SshKeys { ref keys }) = ask(Request::SshKeys) else {
                // (Locked and not unlocked, turned off: no keys, so ssh goes
                // on to its others.)
                return Some(vec![IDENTITIES_ANSWER, 0, 0, 0, 0]);
            };
            let mut out = vec![IDENTITIES_ANSWER];
            let keys: Vec<(Vec<u8>, &str)> = keys.iter().filter_map(|k| Some((B64.decode(&k.key).ok()?, k.comment.as_str()))).collect();
            out.extend_from_slice(&(keys.len() as u32).to_be_bytes());
            for (blob, comment) in keys {
                put(&mut out, &blob);
                put(&mut out, comment.as_bytes());
            }
            Some(out)
        }
        SIGN_REQUEST => {
            let mut at = 1;
            let key = take(msg, &mut at)?;
            let data = take(msg, &mut at)?;
            let flags = msg.get(at..at + 4).and_then(|f| f.try_into().ok()).map_or(0, u32::from_be_bytes);
            let Some(Response::SshSignature { ref signature }) = ask(Request::SshSign { key: B64.encode(key), data: B64.encode(data), flags }) else { return None };
            let mut out = vec![SIGN_RESPONSE];
            put(&mut out, &B64.decode(signature).ok()?);
            Some(out)
        }
        // Adding and removing keys, locking, extensions: Passwords keeps
        // the keys; ssh goes on without.
        _ => None,
    }
}

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
    if !text.contains("BEGIN PRIVATE KEY") {
        return Err("not a private key Passwords can read".into());
    }
    let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = B64.decode(body.trim()).map_err(|e| e.to_string())?;
    // Ed25519's PKCS#8: a fixed 16-byte head (version 0, the Ed25519 OID,
    // an octet string of an octet string), then its 32-byte seed.
    const ED25519: [u8; 16] = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
    let data = if der.len() >= 48 && der[..16] == ED25519 {
        let seed: [u8; 32] = der[16..48].try_into().map_err(|_| "a short Ed25519 key")?;
        KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed))
    } else if let Ok(rsa) = <rsa::RsaPrivateKey as rsa::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
        KeypairData::Rsa(RsaKeypair::try_from(rsa).map_err(|e| e.to_string())?)
    } else if let Ok(secret) = <p256::SecretKey as p256::pkcs8::DecodePrivateKey>::from_pkcs8_der(&der) {
        use p256::elliptic_curve::sec1::ToSec1Point as _;
        KeypairData::Ecdsa(EcdsaKeypair::NistP256 { public: secret.public_key().to_sec1_point(false), private: secret.into() })
    } else {
        return Err("a PKCS#8 key of a kind Passwords can't sign with".into());
    };
    PrivateKey::new(data, "").map_err(|e| e.to_string())
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
        let Ok(key) = private_key(&f.value) else { continue };
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
    fn ed25519() -> PrivateKey {
        PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[3u8; 32])), "test").unwrap()
    }

    fn p256_key() -> PrivateKey {
        use p256::elliptic_curve::sec1::ToSec1Point as _;
        let secret = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        PrivateKey::new(KeypairData::Ecdsa(EcdsaKeypair::NistP256 { public: secret.public_key().to_sec1_point(false), private: secret.into() }), "test").unwrap()
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
