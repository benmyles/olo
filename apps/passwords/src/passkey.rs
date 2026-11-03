//! Passkeys (WebAuthn credentials) that Passwords makes, keeps and signs
//! with. Each lives in a login item's "Passkey" section: the credential
//! id, the relying party, the user handle and name, and the P-256 private
//! key (concealed). 1Password syncs them like any other field, but its
//! own apps don't know them as passkeys.
//!
//! Attestation is "none" with a zero AAGUID; the backup flags are set
//! because 1Password syncs the key; the signature counter stays 0
//! (unsupported, as with other synced passkeys).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use onepassword::{Item, ItemField, ItemFieldType, ItemSection};
use p256::{
    ecdsa::{Signature, SigningKey, VerifyingKey, signature::Signer},
    pkcs8::EncodePublicKey,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const SECTION: &str = "passkey";
const CREDENTIAL: &str = "passkey-credential";
const RP: &str = "passkey-rp";
const USER: &str = "passkey-user";
const NAME: &str = "passkey-name";
const KEY: &str = "passkey-key";

/// Flags: user present, user verified, backup eligible, backed up.
const FLAGS: u8 = 0x01 | 0x04 | 0x08 | 0x10;
/// Attested credential data follows.
const ATTESTED: u8 = 0x40;

/// A passkey as kept in an item.
pub struct Stored {
    pub credential_id: String,
    pub rp_id: String,
    pub user_handle: String,
    pub user_name: String,
    key: String,
}

impl Drop for Stored {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl Stored {
    pub fn signing_key(&self) -> Option<SigningKey> {
        let mut bytes = B64.decode(&self.key).ok()?;
        let key = SigningKey::from_slice(&bytes).ok();
        bytes.zeroize();
        key
    }
}

/// The item's passkey, if it has one.
pub fn stored(item: &Item) -> Option<Stored> {
    let get = |id: &str| item.field(id).filter(|f| f.section_id.as_deref() == Some(SECTION)).map(|f| f.value.clone());
    Some(Stored { credential_id: get(CREDENTIAL).filter(|c| !c.is_empty())?, rp_id: get(RP)?, user_handle: get(USER).unwrap_or_default(), user_name: get(NAME).unwrap_or_default(), key: get(KEY)? })
}

/// Whether a website saved with an item is near enough to `rp_id` for
/// the item to hold its passkey (the same host, or one above or below).
pub fn near(website: &str, rp_id: &str) -> bool {
    let Some(host) = olo_keychain::host(website) else { return false };
    let rp_id = rp_id.to_ascii_lowercase();
    let rp_id = rp_id.strip_prefix("www.").unwrap_or(&rp_id);
    host == rp_id || host.ends_with(&format!(".{rp_id}")) || rp_id.ends_with(&format!(".{host}"))
}

/// A new passkey's key and credential id.
pub struct New {
    pub key: SigningKey,
    pub credential_id: Vec<u8>,
}

impl New {
    pub fn generate() -> New {
        let mut credential_id = vec![0u8; 16];
        getrandom::fill(&mut credential_id).expect("the system's random source failed");
        loop {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("the system's random source failed");
            let key = SigningKey::from_slice(&bytes);
            bytes.zeroize();
            if let Ok(key) = key {
                return New { key, credential_id };
            }
        }
    }

    /// Keep it in `item`'s Passkey section, replacing any there.
    pub fn store(&self, item: &mut Item, rp_id: &str, user_handle: &str, user_name: &str) {
        item.fields.retain(|f| f.section_id.as_deref() != Some(SECTION));
        if !item.sections.iter().any(|s| s.id == SECTION) {
            item.sections.push(section());
        }
        item.fields.extend(self.fields(rp_id, user_handle, user_name));
    }

    /// The Passkey section's fields.
    pub fn fields(&self, rp_id: &str, user_handle: &str, user_name: &str) -> Vec<ItemField> {
        [
            (CREDENTIAL, "credential ID", ItemFieldType::Text, B64.encode(&self.credential_id)),
            (RP, "relying party", ItemFieldType::Text, rp_id.to_string()),
            (USER, "user handle", ItemFieldType::Text, user_handle.to_string()),
            (NAME, "user name", ItemFieldType::Text, user_name.to_string()),
            (KEY, "private key", ItemFieldType::Concealed, B64.encode(self.key.to_bytes())),
        ]
        .into_iter()
        .map(|(id, title, kind, value)| ItemField::new(id, title, kind, value).in_section(SECTION))
        .collect()
    }

    /// The public key as a SubjectPublicKeyInfo, for `getPublicKey()`.
    pub fn public_key_der(&self) -> Vec<u8> {
        self.key.verifying_key().to_public_key_der().map(|d| d.as_bytes().to_vec()).unwrap_or_default()
    }

    /// Authenticator data with the attested credential.
    pub fn auth_data(&self, rp_id: &str) -> Vec<u8> {
        let mut data = auth_data(rp_id, ATTESTED);
        data.extend_from_slice(&[0; 16]);
        data.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
        data.extend_from_slice(&self.credential_id);
        data.extend_from_slice(&cose_key(self.key.verifying_key()));
        data
    }
}

/// What an item's view shows of its passkey: the site and the account
/// (its key and ids stay out of sight).
pub fn shown(fields: &[&ItemField]) -> Vec<ItemField> {
    [(RP, "site"), (NAME, "user name")].into_iter().filter_map(|(id, title)| fields.iter().find(|f| f.id == id).map(|f| ItemField { title: title.into(), ..(*f).clone() })).collect()
}

pub fn section() -> ItemSection {
    ItemSection { id: SECTION.into(), title: "Passkey".into() }
}

/// rpIdHash, flags and a zero counter.
pub fn auth_data(rp_id: &str, extra_flags: u8) -> Vec<u8> {
    let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
    data.push(FLAGS | extra_flags);
    data.extend_from_slice(&0u32.to_be_bytes());
    data
}

/// The "none" attestation object for `auth_data`.
pub fn attestation_object(auth_data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    head(&mut out, 5, 3);
    text(&mut out, "fmt");
    text(&mut out, "none");
    text(&mut out, "attStmt");
    head(&mut out, 5, 0);
    text(&mut out, "authData");
    bytes(&mut out, auth_data);
    out
}

/// A DER ECDSA signature over `auth_data` and the client data's hash.
pub fn sign(key: &SigningKey, auth_data: &[u8], client_data_hash: &[u8]) -> Vec<u8> {
    let mut message = auth_data.to_vec();
    message.extend_from_slice(client_data_hash);
    let signature: Signature = key.sign(&message);
    signature.to_der().to_bytes().to_vec()
}

/// The public key as a COSE_Key: EC2, ES256, P-256, x, y (in CTAP2's
/// canonical key order).
fn cose_key(key: &VerifyingKey) -> Vec<u8> {
    let point = key.to_sec1_point(false);
    let raw = point.as_bytes();
    let mut out = Vec::new();
    head(&mut out, 5, 5);
    int(&mut out, 1);
    int(&mut out, 2);
    int(&mut out, 3);
    int(&mut out, -7);
    int(&mut out, -1);
    int(&mut out, 1);
    int(&mut out, -2);
    bytes(&mut out, &raw[1..33]);
    int(&mut out, -3);
    bytes(&mut out, &raw[33..65]);
    out
}

// The little CBOR WebAuthn needs.

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..24 => out.push(m | n as u8),
        24..256 => out.extend_from_slice(&[m | 24, n as u8]),
        256..65536 => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        _ => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
}

fn int(out: &mut Vec<u8>, v: i64) {
    if v >= 0 { head(out, 0, v as u64) } else { head(out, 1, (-1 - v) as u64) }
}

fn bytes(out: &mut Vec<u8>, b: &[u8]) {
    head(out, 2, b.len() as u64);
    out.extend_from_slice(b);
}

fn text(out: &mut Vec<u8>, s: &str) {
    head(out, 3, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;

    #[test]
    fn create_and_sign() {
        let new = New::generate();
        let data = new.auth_data("example.com");
        assert_eq!(&data[..32], Sha256::digest(b"example.com").as_slice());
        assert_eq!(data[32], 0x5d);
        assert_eq!(&data[37..53], &[0; 16]);
        assert_eq!(u16::from_be_bytes([data[53], data[54]]), 16);
        // COSE key: a 5-entry map, kty EC2, alg ES256.
        assert_eq!(&data[71..77], &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20]);
        assert_eq!(data.len(), 71 + 77);
        let att = attestation_object(&data);
        assert_eq!(&att[..5], &[0xa3, 0x63, b'f', b'm', b't']);
        assert!(att.ends_with(&data));

        let hash = Sha256::digest(b"{}");
        let assertion = auth_data("example.com", 0);
        let der = sign(&new.key, &assertion, &hash);
        let signature = Signature::from_der(&der).unwrap();
        let mut message = assertion.clone();
        message.extend_from_slice(&hash);
        new.key.verifying_key().verify(&message, &signature).unwrap();
        assert!(!new.public_key_der().is_empty());
    }

    #[test]
    fn kept_in_an_item() {
        let mut item: Item = serde_json::from_str(
            r#"{"id":"i","title":"T","category":"Login","vaultId":"v","version":1,
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "fields":[{"id":"username","title":"username","fieldType":"Text","value":"jane"}]}"#,
        )
        .unwrap();
        assert!(stored(&item).is_none());
        let new = New::generate();
        new.store(&mut item, "example.com", "dXNlcg", "jane");
        new.store(&mut item, "example.com", "dXNlcg", "jane");
        assert_eq!(item.sections.len(), 1);
        assert_eq!(item.fields.len(), 6);
        let s = stored(&item).unwrap();
        assert_eq!(s.rp_id, "example.com");
        assert_eq!(s.user_handle, "dXNlcg");
        assert_eq!(s.signing_key().unwrap().to_bytes(), new.key.to_bytes());
        assert!(near("https://www.example.com", "example.com"));
        assert!(near("https://login.example.com", "example.com"));
        assert!(near("example.com", "login.example.com"));
        assert!(!near("https://example.com.evil.net", "example.com"));
    }
}
