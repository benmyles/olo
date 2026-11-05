//! A credential of Olo's on a key (MakeCredential, not discoverable, with
//! `hmac-secret`) and its secret (GetAssertion's `hmac-secret`: 32 bytes
//! only that key gives, for a salt, after a touch).

use std::io::Cursor;

use ciborium::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    Error, Result,
    ctap::{Channel, GET_ASSERTION, MAKE_CREDENTIAL, Wait, get, int_map, map, text_map},
    hid::Transport,
    pin,
};

/// The relying party Olo's credentials are for.
pub const RP: &str = "olo.local";

/// authenticatorData's flags.
const FLAG_UP: u8 = 0x01;
const FLAG_AT: u8 = 0x40;
const FLAG_ED: u8 = 0x80;

/// A credential just made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub id: Vec<u8>,
    pub aaguid: [u8; 16],
}

/// Make a credential for `user` (a touch); `hmac-secret` on it.
pub fn make<T: Transport>(ch: &mut Channel<T>, user: &str, wait: &Wait) -> Result<Made> {
    let mut user_id = [0u8; 16];
    getrandom::fill(&mut user_id).map_err(|_| Error::Random)?;
    let params = int_map(vec![
        (1, Value::Bytes(client_data_hash("make")?)),
        (2, text_map(vec![("id", Value::Text(RP.into())), ("name", Value::Text("Olo".into()))])),
        (3, text_map(vec![("id", Value::Bytes(user_id.to_vec())), ("name", Value::Text(user.into())), ("displayName", Value::Text(user.into()))])),
        (4, Value::Array(vec![text_map(vec![("alg", Value::Integer((-7).into())), ("type", Value::Text("public-key".into()))])])),
        (6, text_map(vec![("hmac-secret", Value::Bool(true))])),
    ]);
    let answer = ch.cbor(MAKE_CREDENTIAL, Some(params), wait)?.ok_or(Error::Protocol("no credential"))?;
    let auth = get(&answer, 2).and_then(|a| a.as_bytes()).ok_or(Error::Protocol("no authenticator data"))?;
    let parsed = AuthData::parse(auth, true)?;
    let (id, aaguid) = parsed.credential.ok_or(Error::Protocol("no credential in the authenticator data"))?;
    let hmac = parsed.extensions.as_ref().and_then(|e| e.as_map()).is_some_and(|m| m.iter().any(|(k, v)| k.as_text() == Some("hmac-secret") && v.as_bool() == Some(true)));
    if !hmac {
        return Err(Error::NoHmacSecret);
    }
    Ok(Made { id, aaguid })
}

/// Whether the key behind `ch` holds credential `id` (asked without a
/// touch: GetAssertion with `up` off, as platforms do before asking for
/// one).
pub fn holds<T: Transport>(ch: &mut Channel<T>, id: &[u8], wait: &Wait) -> Result<bool> {
    let params = int_map(vec![(1, Value::Text(RP.into())), (2, Value::Bytes(client_data_hash("look")?)), (3, Value::Array(vec![descriptor(id)])), (5, text_map(vec![("up", Value::Bool(false))]))]);
    match ch.cbor(GET_ASSERTION, Some(params), wait) {
        Ok(_) => Ok(true),
        Err(Error::Ctap(crate::ctap::ERR_NO_CREDENTIALS)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Credential `id`'s secret for `salt` (a touch).
pub fn secret<T: Transport>(ch: &mut Channel<T>, protocol: u8, id: &[u8], salt: &[u8; 32], wait: &Wait) -> Result<Zeroizing<[u8; 32]>> {
    let shared = pin::agree(ch, protocol, wait)?;
    let salt_enc = shared.encrypt(salt)?;
    let salt_auth = shared.authenticate(&salt_enc);
    let mut hmac = vec![(1, shared.platform_key.clone()), (2, Value::Bytes(salt_enc)), (3, Value::Bytes(salt_auth))];
    if protocol != 1 {
        hmac.push((4, Value::Integer(protocol.into())));
    }
    let params = int_map(vec![
        (1, Value::Text(RP.into())),
        (2, Value::Bytes(client_data_hash("secret")?)),
        (3, Value::Array(vec![descriptor(id)])),
        (4, text_map(vec![("hmac-secret", int_map(hmac))])),
        (5, text_map(vec![("up", Value::Bool(true))])),
    ]);
    let answer = ch.cbor(GET_ASSERTION, Some(params), wait)?.ok_or(Error::Protocol("no assertion"))?;
    let auth = get(&answer, 2).and_then(|a| a.as_bytes()).ok_or(Error::Protocol("no authenticator data"))?;
    let parsed = AuthData::parse(auth, false)?;
    if parsed.flags & FLAG_UP == 0 {
        return Err(Error::Protocol("the key answered without a touch"));
    }
    let ext = parsed.extensions.ok_or(Error::NoHmacSecret)?;
    let out = map(&ext)?.iter().find(|(k, _)| k.as_text() == Some("hmac-secret")).and_then(|(_, v)| v.as_bytes()).ok_or(Error::NoHmacSecret)?;
    let plain = shared.decrypt(out)?;
    if plain.len() != 32 {
        return Err(Error::Protocol("an hmac-secret of the wrong length"));
    }
    let mut s = Zeroizing::new([0u8; 32]);
    s.copy_from_slice(&plain);
    Ok(s)
}

fn descriptor(id: &[u8]) -> Value {
    text_map(vec![("id", Value::Bytes(id.to_vec())), ("type", Value::Text("public-key".into()))])
}

/// A client data hash: nothing is signed for anyone to check, so a
/// random one, named for what it's for.
fn client_data_hash(what: &str) -> Result<Vec<u8>> {
    let mut r = [0u8; 32];
    getrandom::fill(&mut r).map_err(|_| Error::Random)?;
    Ok(Sha256::new().chain_update(what).chain_update(r).finalize().to_vec())
}

/// authenticatorData, as far as Olo reads it.
pub struct AuthData {
    pub rp_hash: [u8; 32],
    pub flags: u8,
    /// The credential made (its id and the key's AAGUID).
    pub credential: Option<(Vec<u8>, [u8; 16])>,
    pub extensions: Option<Value>,
}

impl AuthData {
    pub fn parse(b: &[u8], made: bool) -> Result<AuthData> {
        if b.len() < 37 {
            return Err(Error::Protocol("authenticator data too short"));
        }
        let mut rp_hash = [0u8; 32];
        rp_hash.copy_from_slice(&b[..32]);
        if rp_hash[..] != Sha256::digest(RP.as_bytes())[..] {
            return Err(Error::Protocol("authenticator data for another relying party"));
        }
        let flags = b[32];
        let mut at = 37;
        let mut credential = None;
        if flags & FLAG_AT != 0 {
            if b.len() < at + 18 {
                return Err(Error::Protocol("attested credential data too short"));
            }
            let mut aaguid = [0u8; 16];
            aaguid.copy_from_slice(&b[at..at + 16]);
            let len = u16::from_be_bytes([b[at + 16], b[at + 17]]) as usize;
            at += 18;
            let id = b.get(at..at + len).ok_or(Error::Protocol("a credential id too long"))?.to_vec();
            at += len;
            // Its public key: a CBOR value, skipped.
            let mut c = Cursor::new(&b[at..]);
            let _: Value = ciborium::from_reader(&mut c).map_err(|_| Error::Cbor)?;
            at += c.position() as usize;
            credential = Some((id, aaguid));
        } else if made {
            return Err(Error::Protocol("no attested credential"));
        }
        let extensions = if flags & FLAG_ED != 0 { Some(ciborium::from_reader::<Value, _>(&b[at..]).map_err(|_| Error::Cbor)?) } else { None };
        Ok(AuthData { rp_hash, flags, credential, extensions })
    }
}
