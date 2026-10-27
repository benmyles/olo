//! Tabula Passwords in Chrome. The extension (`apps/chrome/extension`) is
//! packed here into a CRX3 signed with a key of this user's own, and
//! listed in the Chrome install's `extensions` folder, from which Chrome
//! installs it at its next start (and updates it when its version grows;
//! on Linux, without asking). Beside it goes the native messaging host's
//! manifest, which lets the extension start `tabula-chrome` as its host
//! (`host.rs`). Nothing needs root; everything is redone at each start, so
//! a newly installed Chrome version and a moved `tabula-chrome` are
//! picked up.

use std::{
    io::Write,
    path::Path,
};

use p256::{
    ecdsa::{Signature, SigningKey, signature::Signer},
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tabula_theme::{Palette, Rgba};

/// The native messaging host's name.
pub const HOST: &str = "org.tabula.passwords";

const FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../extension/manifest.json")),
    ("background.js", include_bytes!("../extension/background.js")),
    ("content.js", include_bytes!("../extension/content.js")),
    ("tree.js", include_bytes!("../extension/tree.js")),
    ("popup.html", include_bytes!("../extension/popup.html")),
    ("popup.css", include_bytes!("../extension/popup.css")),
    ("popup.js", include_bytes!("../extension/popup.js")),
    ("icon-16.png", include_bytes!("../extension/icon-16.png")),
    ("icon-32.png", include_bytes!("../extension/icon-32.png")),
    ("icon-48.png", include_bytes!("../extension/icon-48.png")),
    ("icon-128.png", include_bytes!("../extension/icon-128.png")),
];

/// The extension's version, from its manifest (raise it there whenever
/// the extension changes, or Chrome keeps the old one).
pub fn version() -> String {
    let manifest: serde_json::Value = serde_json::from_slice(FILES[0].1).expect("the extension's manifest is JSON");
    manifest["version"].as_str().expect("the manifest has a version").to_owned()
}

/// Chrome's id for a key: the first 16 bytes of the SHA-256 of its
/// SubjectPublicKeyInfo, in hex written with the letters a–p.
pub fn id_of(spki: &[u8]) -> String {
    Sha256::digest(spki)[..16].iter().flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect()
}

/// This user's signing key (made once, kept beside Chrome, readable only
/// by them).
fn key(root: &Path) -> std::io::Result<SigningKey> {
    let file = root.join("extension-key.der");
    if let Ok(der) = std::fs::read(&file)
        && let Ok(key) = SigningKey::from_pkcs8_der(&der)
    {
        return Ok(key);
    }
    let key = loop {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        if let Ok(key) = SigningKey::from_slice(&bytes) {
            break key;
        }
    };
    let der = key.to_pkcs8_der().map_err(std::io::Error::other)?;
    std::fs::create_dir_all(root)?;
    let tmp = root.join("extension-key.der.tmp");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(der.as_bytes())?;
    }
    std::fs::rename(&tmp, &file)?;
    Ok(key)
}

/// CSS custom properties for the popup from Tabula's palette, light and
/// dark.
fn palette_css() -> String {
    fn vars(p: &Palette) -> String {
        let hex = |c: Rgba| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        format!(
            "--panel: {}; --text: {}; --doc: {}; --border: {}; --shine: {}; --shadow: {}; --select: {}; --dim: {}; --fail: {};",
            hex(p.panel_background),
            hex(p.panel_text),
            hex(p.document_background),
            hex(p.control_border),
            hex(p.shine),
            hex(p.shadow),
            hex(p.list_selected_background),
            hex(tabula_theme::mix(p.panel_text, p.panel_background, 0.45)),
            hex(p.failure),
        )
    }
    let light = Palette::for_mode(tabula_theme::ThemeMode::Light);
    let dark = Palette::for_mode(tabula_theme::ThemeMode::Dark);
    format!(":root {{ {} }}\n@media (prefers-color-scheme: dark) {{ :root {{ {} }} }}\n", vars(&light), vars(&dark))
}

/// The extension's files as a zip (stored, dated 1980, so the same files
/// make the same bytes).
fn zip() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).last_modified_time(zip::DateTime::default());
    let palette = palette_css();
    let files = FILES.iter().map(|(n, b)| (*n, *b)).chain(std::iter::once(("palette.css", palette.as_bytes())));
    for (name, bytes) in files {
        w.start_file(name, opts).expect("zip entry");
        w.write_all(bytes).expect("zip write");
    }
    w.finish().expect("zip").into_inner()
}

fn varint(mut n: u64, out: &mut Vec<u8>) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

/// A protobuf length-delimited field.
fn field(number: u64, bytes: &[u8], out: &mut Vec<u8>) {
    varint(number << 3 | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// A CRX3: "Cr24", version 3, the header (a `CrxFileHeader` protobuf with
/// one `sha256_with_ecdsa` proof and the signed `crx_id`), then the zip.
/// The signature covers "CRX3 SignedData\0", the signed data's length
/// (little-endian u32), the signed data and the zip.
pub fn crx(key: &SigningKey, archive: &[u8]) -> Vec<u8> {
    let spki = key.verifying_key().to_public_key_der().expect("public key DER");
    let id = Sha256::digest(spki.as_bytes());
    let mut signed = Vec::new();
    field(1, &id[..16], &mut signed);
    let mut message = b"CRX3 SignedData\0".to_vec();
    message.extend_from_slice(&(signed.len() as u32).to_le_bytes());
    message.extend_from_slice(&signed);
    message.extend_from_slice(archive);
    let signature: Signature = key.sign(&message);
    let mut proof = Vec::new();
    field(1, spki.as_bytes(), &mut proof);
    field(2, signature.to_der().as_bytes(), &mut proof);
    let mut header = Vec::new();
    field(3, &proof, &mut header);
    field(10000, &signed, &mut header);
    let mut out = b"Cr24".to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(archive);
    out
}

/// Write `bytes` to `path` unless it already holds them.
fn put(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tabula-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// Offer the extension to the Chrome in `chrome_dir` (its install folder)
/// and register `exe` as its host for the profiles in `user_data`. `root`
/// keeps the key and the packed extension. The extension's id.
pub fn set_up(root: &Path, chrome_dir: &Path, user_data: &Path, exe: &Path) -> std::io::Result<String> {
    let key = key(root)?;
    let spki = key.verifying_key().to_public_key_der().map_err(std::io::Error::other)?;
    let id = id_of(spki.as_bytes());
    let version = version();
    let crx_path = root.join("tabula-passwords.crx");
    put(&crx_path, &crx(&key, &zip()))?;
    let external = json!({ "external_crx": crx_path, "external_version": version });
    put(&chrome_dir.join("extensions").join(format!("{id}.json")), &serde_json::to_vec_pretty(&external)?)?;
    let manifest = json!({
        "name": HOST,
        "description": "Tabula Passwords",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{id}/")],
    });
    put(&user_data.join("NativeMessagingHosts").join(format!("{HOST}.json")), &serde_json::to_vec_pretty(&manifest)?)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_crx() {
        assert_eq!(id_of(b"").len(), 32);
        assert!(id_of(b"x").chars().all(|c| ('a'..='p').contains(&c)));
        let key = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let archive = zip();
        let a = crx(&key, &archive);
        assert_eq!(&a[..4], b"Cr24");
        assert_eq!(u32::from_le_bytes(a[4..8].try_into().unwrap()), 3);
        let header_len = u32::from_le_bytes(a[8..12].try_into().unwrap()) as usize;
        assert_eq!(&a[12 + header_len..], &archive[..]);
        // The same files and key make the same bytes (RFC 6979 signing).
        assert_eq!(a, crx(&key, &zip()));
        // The signature checks out against the header's key.
        let spki = key.verifying_key().to_public_key_der().unwrap();
        let mut signed = Vec::new();
        field(1, &Sha256::digest(spki.as_bytes())[..16], &mut signed);
        let mut message = b"CRX3 SignedData\0".to_vec();
        message.extend_from_slice(&(signed.len() as u32).to_le_bytes());
        message.extend_from_slice(&signed);
        message.extend_from_slice(&archive);
        let sig: Signature = key.sign(&message);
        use p256::ecdsa::signature::Verifier;
        assert!(key.verifying_key().verify(&message, &sig).is_ok());
        let header = &a[12..12 + header_len];
        assert!(header.windows(sig.to_der().as_bytes().len()).any(|w| w == sig.to_der().as_bytes()));
        assert_eq!(version(), "1.1.1");
    }

    #[test]
    fn set_up_writes_everything_once() {
        let dir = std::env::temp_dir().join(format!("tabula-chrome-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, chrome, data) = (dir.join("root"), dir.join("chrome"), dir.join("data"));
        let id = set_up(&root, &chrome, &data, Path::new("/usr/bin/tabula-chrome")).unwrap();
        assert_eq!(id, set_up(&root, &chrome, &data, Path::new("/usr/bin/tabula-chrome")).unwrap(), "the same key");
        let ext: serde_json::Value = serde_json::from_slice(&std::fs::read(chrome.join(format!("extensions/{id}.json"))).unwrap()).unwrap();
        assert_eq!(ext["external_version"], "1.1.1");
        let host: serde_json::Value = serde_json::from_slice(&std::fs::read(data.join("NativeMessagingHosts/org.tabula.passwords.json")).unwrap()).unwrap();
        assert_eq!(host["allowed_origins"][0], format!("chrome-extension://{id}/"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.join("extension-key.der")).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
