//! The pool's certificate authority, for the proxy (`intercept`): made
//! once and kept beside the pool's other files (its key readable only by
//! this user), it vouches only for the providers' own hosts (a name
//! constraint in the certificate itself, so even its key could vouch for
//! nothing else), and the proxy signs a certificate for each of those
//! hosts with it as it starts. Claude Code adds it to what it trusts
//! (`NODE_EXTRA_CA_CERTS`); Antigravity adds it via `AGY_CA_CERT`; Codex
//! takes a bundle of the system's roots and it (`SSL_CERT_FILE`), as its CA
//! setting replaces the roots.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use chrono::Datelike;
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};

/// The hosts the pool answers for: the subscriptions' model APIs.
pub const HOSTS: &[&str] = &["api.anthropic.com", "chatgpt.com", "daily-cloudcode-pa.googleapis.com", "cloudcode-pa.googleapis.com"];

/// Legacy Claude + Codex hosts (covered by pre-Antigravity `pool-ca.pem`).
pub const LEGACY_HOSTS: &[&str] = &["api.anthropic.com", "chatgpt.com"];

/// Antigravity's Cloud Code hosts. On upgraded installs whose existing
/// `pool-ca.pem` only permits Claude + Codex, a companion `pool-agy-ca.pem`
/// covers these hosts without touching `pool-ca.pem` so running Claude Code
/// and Codex sessions keep trusting their existing CA undisturbed.
pub const AGY_HOSTS: &[&str] = &["daily-cloudcode-pa.googleapis.com", "cloudcode-pa.googleapis.com"];

const NAME: &str = "Olo Token Pool";
const AGY_NAME: &str = "Olo Token Pool (Antigravity)";

/// Where the primary authority's certificate is (for Claude Code and fresh
/// installs).
pub fn cert_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-ca.pem")
}

/// Where the certificate Antigravity should trust is: `pool-ca.pem` on fresh
/// 4-host installs (or rotated primaries that cover `AGY_HOSTS`), or the
/// companion `pool-agy-ca.pem` on upgraded installs whose legacy `pool-ca.pem`
/// lacks Antigravity name constraints.
pub fn agy_cert_path() -> PathBuf {
    agy_cert_path_in(&olo_xdg::config_dir())
}

pub(crate) fn agy_cert_path_in(dir: &Path) -> PathBuf {
    let primary_cert = dir.join("pool-ca.pem");
    let primary_key = dir.join("pool-ca.key");
    if primary_cert.is_file() {
        // Always check primary coverage FIRST (cached by `(path, mtime, len)`)
        // so a rotated/fresh 4-host `pool-ca.pem` is never shadowed by a stale
        // leftover `pool-agy-ca.pem`, and repeated `pointed()` checks avoid
        // generating a keypair per call.
        if primary_covers_agy_cached(&primary_cert, &primary_key) {
            return primary_cert;
        }
        return dir.join("pool-agy-ca.pem");
    }
    let companion = dir.join("pool-agy-ca.pem");
    if companion.is_file() {
        return companion;
    }
    primary_cert
}

/// The system's roots and the authority's, for clients whose CA setting
/// replaces the roots (Codex).
pub fn bundle_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-ca-bundle.pem")
}

/// Where distributions keep the system's roots.
const SYSTEM_ROOTS: &[&str] = &["/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/cert.pem", "/etc/pki/tls/cacert.pem"];

fn params_for(now: chrono::DateTime<chrono::Utc>, name: &str, hosts: &[&str]) -> CertificateParams {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn.push(DnType::OrganizationName, "Olo");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.name_constraints = Some(NameConstraints { permitted_subtrees: hosts.iter().map(|h| GeneralSubtree::DnsName((*h).into())).collect(), excluded_subtrees: Vec::new() });
    p.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    p.not_after = rcgen::date_time_ymd(now.year() + 10, now.month() as u8, now.day().min(28) as u8);
    p
}

/// Whether `cert_pem` + `issuer` can sign valid leaf certificates for every
/// host in `hosts` under WebPKI name constraints.
fn covers_hosts(cert_pem: &str, issuer: &Issuer<'static, KeyPair>, hosts: &[&str]) -> bool {
    use rustls::client::danger::ServerCertVerifier as _;
    use rustls::pki_types::pem::PemObject as _;
    let Ok(ca_der) = CertificateDer::from_pem_slice(cert_pem.as_bytes()) else { return false };
    let mut roots = rustls::RootCertStore::empty();
    if roots.add(ca_der).is_err() {
        return false;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let Ok(verifier) = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider).build() else { return false };
    let Ok(leaf_key) = KeyPair::generate() else { return false };
    let Ok(mut p) = CertificateParams::new(hosts.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>()) else { return false };
    let now = chrono::Utc::now();
    let ago = now - chrono::Duration::days(1);
    let until = now + chrono::Duration::days(2);
    p.not_before = rcgen::date_time_ymd(ago.year(), ago.month() as u8, ago.day() as u8);
    p.not_after = rcgen::date_time_ymd(until.year(), until.month() as u8, until.day() as u8);
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let Ok(leaf) = p.signed_by(&leaf_key, issuer) else { return false };
    let check_time = UnixTime::now();
    hosts.iter().all(|host| ServerName::try_from(*host).ok().is_some_and(|name| verifier.verify_server_cert(leaf.der(), &[], &name, &[], check_time).is_ok()))
}

type CacheKey = (PathBuf, Option<SystemTime>, u64, Option<SystemTime>, u64);

fn primary_covers_agy_cached(cert_path: &Path, key_path: &Path) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, bool>>> = OnceLock::new();
    let cert_meta = std::fs::metadata(cert_path).ok();
    let key_meta = std::fs::metadata(key_path).ok();
    let key = (
        cert_path.to_path_buf(),
        cert_meta.as_ref().and_then(|m| m.modified().ok()),
        cert_meta.as_ref().map(|m| m.len()).unwrap_or(0),
        key_meta.as_ref().and_then(|m| m.modified().ok()),
        key_meta.as_ref().map(|m| m.len()).unwrap_or(0),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&cached) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return cached;
    }
    let covers = std::fs::read_to_string(cert_path).ok().zip(std::fs::read_to_string(key_path).ok()).is_some_and(|(cert_pem, k)| {
        KeyPair::from_pem(&k).ok().is_some_and(|kp| {
            let issuer = Issuer::new(params_for(chrono::Utc::now(), NAME, HOSTS), kp);
            covers_hosts(&cert_pem, &issuer, AGY_HOSTS)
        })
    });
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 64 {
        map.clear();
    }
    map.insert(key, covers);
    covers
}

/// Exclusive cross-process file lock for CA initialization.
struct CaLock(std::fs::File);

impl Drop for CaLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: valid file descriptor held by `self.0`.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn lock_ca_dir(dir: &Path) -> Result<CaLock, String> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn’t prepare {}: {e}", dir.display()))?;
    let path = dir.join("pool-ca.lock");
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| format!("couldn’t open {}: {e}", path.display()))?;
    // SAFETY: valid open fd.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 { Ok(CaLock(file)) } else { Err(format!("couldn’t lock {}: {}", path.display(), std::io::Error::last_os_error())) }
}

/// Guard against unit tests (or test harnesses setting `OLO_POOL_TEST_ISOLATE`)
/// touching real `~/.config/olo`. Never active in production binaries or
/// `cargo run` / `jato run` (which set `CARGO_MANIFEST_DIR`).
fn assert_test_dir_isolated(dir: &Path) {
    let in_test = cfg!(test) || std::env::var_os("OLO_POOL_TEST_ISOLATE").is_some();
    if in_test && std::env::var_os("OLO_ALLOW_REAL_CONFIG").is_none() {
        assert!(dir.starts_with("/var/tmp"), "tests must NEVER open/write CAs outside /var/tmp (got {}); use Authority::open_in() or Authority::ephemeral() with a /var/tmp dir", dir.display());
    }
}

/// The authority: its primary certificate (PEM) and signing issuer, plus an
/// optional companion Antigravity CA on upgraded legacy 2-host installs.
pub struct Authority {
    pub cert_pem: String,
    pub agy_cert_pem: Option<String>,
    /// Set if this is a legacy 2-host CA whose companion Antigravity CA could
    /// not be created (e.g. unwritable directory); Claude + Codex keep serving.
    pub agy_error: Option<String>,
    primary_covers_agy: bool,
    issuer: Issuer<'static, KeyPair>,
    agy_issuer: Option<Issuer<'static, KeyPair>>,
}

impl Authority {
    /// Open or initialize the CA(s) in `olo_xdg::config_dir()`.
    pub fn open() -> Result<Authority, String> {
        Self::open_in(&olo_xdg::config_dir())
    }

    /// Open or initialize the CA(s) in `dir`:
    /// - Fresh install (no existing `pool-ca.{pem,key}`): creates a single
    ///   4-host `pool-ca.{pem,key}` + `pool-ca-bundle.pem`.
    /// - Upgraded install (existing `pool-ca.{pem,key}` lacking Antigravity
    ///   hosts): leaves `pool-ca.{pem,key}` 100% byte-for-byte untouched
    ///   (and refreshes `pool-ca-bundle.pem` only if system roots changed),
    ///   and opens/creates a companion `pool-agy-ca.{pem,key}` constrained
    ///   strictly to [`AGY_HOSTS`]. If companion creation fails (e.g. a
    ///   read-only directory), the legacy primary CA still opens cleanly and
    ///   `agy_error` records why Antigravity could not be enabled.
    pub fn open_in(dir: &Path) -> Result<Authority, String> {
        assert_test_dir_isolated(dir);
        let lock = lock_ca_dir(dir);
        let (cert_file, key_file, bundle_file) = (dir.join("pool-ca.pem"), dir.join("pool-ca.key"), dir.join("pool-ca-bundle.pem"));
        let kept = std::fs::read_to_string(&cert_file).ok().zip(std::fs::read_to_string(&key_file).ok());
        let existing = kept.and_then(|(cert_pem, k)| {
            let key = KeyPair::from_pem(&k).ok()?;
            // Its name and key are all signing needs (the dates and
            // serial are the certificate's own business).
            let issuer = Issuer::new(params_for(chrono::Utc::now(), NAME, HOSTS), key);
            Some((cert_pem, issuer))
        });
        let authority = match existing {
            Some((cert_pem, issuer)) => {
                if covers_hosts(&cert_pem, &issuer, AGY_HOSTS) {
                    // Primary covers all 4 hosts: remove any stale leftover companion CA
                    // (when writable/locked) so client path selection and server SNI
                    // signing never diverge.
                    if lock.is_ok() {
                        let _ = std::fs::remove_file(dir.join("pool-agy-ca.pem"));
                        let _ = std::fs::remove_file(dir.join("pool-agy-ca.key"));
                    }
                    let a = Authority { cert_pem, agy_cert_pem: None, agy_error: None, primary_covers_agy: true, issuer, agy_issuer: None };
                    if lock.is_ok()
                        && let Err(e) = a.write_bundle_to(&bundle_file)
                    {
                        tracing::warn!("pool: {e}");
                    }
                    a
                } else {
                    // Legacy 2-host CA: keep pool-ca.pem and pool-ca.key 100%
                    // byte-for-byte untouched so already-running Claude Code and
                    // Codex sessions keep working. Refresh Codex's bundle only
                    // when its content actually differs (e.g. OS root update).
                    let (agy_cert_pem, agy_issuer, agy_error) = match open_existing_agy_companion(dir) {
                        Some((pem, iss)) => (Some(pem), Some(iss), None),
                        None => match &lock {
                            Ok(_) => match create_agy_companion(dir) {
                                Ok((pem, iss)) => (Some(pem), Some(iss), None),
                                Err(e) => {
                                    tracing::warn!("pool: Antigravity companion CA unavailable (keeping Claude/Codex proxy active): {e}");
                                    (None, None, Some(e))
                                }
                            },
                            Err(lock_err) => {
                                let msg = format!("Couldn’t lock CA directory to create Antigravity companion CA: {lock_err}");
                                tracing::warn!("pool: {msg} (keeping Claude/Codex proxy active)");
                                (None, None, Some(msg))
                            }
                        },
                    };
                    let a = Authority { cert_pem, agy_cert_pem, agy_error, primary_covers_agy: false, issuer, agy_issuer };
                    if lock.is_ok()
                        && let Err(e) = a.write_bundle_to(&bundle_file)
                    {
                        tracing::warn!("pool: {e}");
                    }
                    a
                }
            }
            None => {
                let _lock = lock.map_err(|e| format!("Couldn’t lock the pool’s certificate directory: {e}"))?;
                let key = KeyPair::generate().map_err(|e| e.to_string())?;
                let p = params_for(chrono::Utc::now(), NAME, HOSTS);
                let cert_pem = p.self_signed(&key).map_err(|e| e.to_string())?.pem();
                crate::store::write_private(&key_file, key.serialize_pem().as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s certificate key: {e}"))?;
                write_public(&cert_file, cert_pem.as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s certificate: {e}"))?;
                let _ = std::fs::remove_file(dir.join("pool-agy-ca.pem"));
                let _ = std::fs::remove_file(dir.join("pool-agy-ca.key"));
                let a = Authority { cert_pem, agy_cert_pem: None, agy_error: None, primary_covers_agy: true, issuer: Issuer::new(p, key), agy_issuer: None };
                if let Err(e) = a.write_bundle_to(&bundle_file) {
                    tracing::warn!("pool: {e}");
                }
                a
            }
        };
        Ok(authority)
    }

    /// An authority kept nowhere (tests).
    pub fn ephemeral() -> Authority {
        let key = KeyPair::generate().expect("a key");
        let p = params_for(chrono::Utc::now(), NAME, HOSTS);
        let cert_pem = p.self_signed(&key).expect("a certificate").pem();
        Authority { cert_pem, agy_cert_pem: None, agy_error: None, primary_covers_agy: true, issuer: Issuer::new(p, key), agy_issuer: None }
    }

    /// Whether this authority can sign Antigravity Cloud Code hosts.
    pub fn has_agy(&self) -> bool {
        self.primary_covers_agy || self.agy_issuer.is_some()
    }

    /// The CA PEM that Antigravity clients should trust for this authority.
    pub fn agy_pem(&self) -> &str {
        self.agy_cert_pem.as_deref().unwrap_or(&self.cert_pem)
    }

    /// The system's roots with ours after them.
    fn write_bundle_to(&self, path: &Path) -> Result<(), String> {
        self.write_bundle_with_roots(path, SYSTEM_ROOTS)
    }

    fn write_bundle_with_roots(&self, path: &Path, root_candidates: &[&str]) -> Result<(), String> {
        let Some(mut bundle) = root_candidates.iter().find_map(|p| std::fs::read_to_string(p).ok()) else {
            if path.exists() {
                tracing::warn!("pool: found none of the system's root certificates; keeping existing Codex bundle untouched");
                return Ok(());
            }
            return Err("found none of the system's root certificates for Codex's bundle".into());
        };
        if !bundle.ends_with('\n') && !bundle.is_empty() {
            bundle.push('\n');
        }
        bundle.push_str(&format!("# {NAME}\n{}", self.cert_pem));
        if std::fs::read_to_string(path).ok().as_deref() != Some(bundle.as_str()) {
            write_public(path, bundle.as_bytes()).map_err(|e| format!("couldn’t write the certificate bundle: {e}"))?;
        }
        Ok(())
    }

    /// A TLS server configuration presenting a certificate for each supported
    /// host (by the name the client asks for), made now.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        use rustls::pki_types::pem::PemObject;
        let ca_der = CertificateDer::from_pem_slice(self.cert_pem.as_bytes()).map_err(|e| e.to_string())?;
        let agy_ca_der = match &self.agy_cert_pem {
            Some(pem) => Some(CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|e| e.to_string())?),
            None => None,
        };
        let mut resolver = rustls::server::ResolvesServerCertUsingSni::new();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let hosts = if self.has_agy() { HOSTS } else { LEGACY_HOSTS };
        for host in hosts {
            let key = KeyPair::generate().map_err(|e| e.to_string())?;
            let mut p = CertificateParams::new(vec![(*host).to_owned()]).map_err(|e| e.to_string())?;
            let now = chrono::Utc::now();
            let ago = now - chrono::Duration::days(1);
            p.not_before = rcgen::date_time_ymd(ago.year(), ago.month() as u8, ago.day() as u8);
            let until = now + chrono::Duration::days(365);
            p.not_after = rcgen::date_time_ymd(until.year(), until.month() as u8, until.day() as u8);
            p.distinguished_name.push(DnType::CommonName, *host);
            p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            p.use_authority_key_identifier_extension = true;
            let (issuer, parent_der) = match (AGY_HOSTS.contains(host), &self.agy_issuer, &agy_ca_der) {
                (true, Some(agy_iss), Some(agy_der)) => (agy_iss, agy_der),
                _ => (&self.issuer, &ca_der),
            };
            let cert = p.signed_by(&key, issuer).map_err(|e| e.to_string())?;
            let chain: Vec<CertificateDer<'static>> = vec![cert.der().clone(), parent_der.clone()];
            let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
            let signing = provider.key_provider.load_private_key(private).map_err(|e| e.to_string())?;
            resolver.add(host, rustls::sign::CertifiedKey::new(chain, signing)).map_err(|e| e.to_string())?;
        }
        let mut config =
            rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.with_no_client_auth().with_cert_resolver(Arc::new(resolver));
        // One request at a time on a connection, as the clients' HTTP/1.1.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

fn open_existing_agy_companion(dir: &Path) -> Option<(String, Issuer<'static, KeyPair>)> {
    let (cert_file, key_file) = (dir.join("pool-agy-ca.pem"), dir.join("pool-agy-ca.key"));
    let (cert_pem, k) = (std::fs::read_to_string(&cert_file).ok()?, std::fs::read_to_string(&key_file).ok()?);
    let key = KeyPair::from_pem(&k).ok()?;
    let issuer = Issuer::new(params_for(chrono::Utc::now(), AGY_NAME, AGY_HOSTS), key);
    covers_hosts(&cert_pem, &issuer, AGY_HOSTS).then_some((cert_pem, issuer))
}

fn create_agy_companion(dir: &Path) -> Result<(String, Issuer<'static, KeyPair>), String> {
    let (cert_file, key_file) = (dir.join("pool-agy-ca.pem"), dir.join("pool-agy-ca.key"));
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let p = params_for(chrono::Utc::now(), AGY_NAME, AGY_HOSTS);
    let cert_pem = p.self_signed(&key).map_err(|e| e.to_string())?.pem();
    crate::store::write_private(&key_file, key.serialize_pem().as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s Antigravity certificate key: {e}"))?;
    write_public(&cert_file, cert_pem.as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s Antigravity certificate: {e}"))?;
    Ok((cert_pem, Issuer::new(p, key)))
}

/// Written readable by anyone (a certificate is public), atomically via a
/// unique temporary file so concurrent writers never collide.
fn write_public(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("pool-ca");
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{name}.{pid}.{seq}.tmp"));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    if let Err(e) = f.write_all(bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A legacy 2-host CA (permitting only `api.anthropic.com` and `chatgpt.com`)
    /// is left 100% byte-for-byte untouched on upgrade (so running Claude Code
    /// and Codex sessions keep working undisturbed), while a companion
    /// `pool-agy-ca.{pem,key}` is created strictly for Antigravity hosts.
    #[test]
    fn legacy_two_host_ca_stays_untouched_and_creates_companion_agy_ca() {
        use rustls::client::danger::ServerCertVerifier as _;
        use rustls::pki_types::pem::PemObject as _;

        let dir = tempfile::Builder::new().prefix("olo-pool-ca-legacy-").tempdir_in("/var/tmp").unwrap();
        let legacy_key = KeyPair::generate().unwrap();
        let legacy_params = params_for(chrono::Utc::now(), NAME, LEGACY_HOSTS);
        let legacy_pem = legacy_params.self_signed(&legacy_key).unwrap().pem();
        let legacy_key_pem = legacy_key.serialize_pem();
        let mut expected_bundle = SYSTEM_ROOTS.iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        if !expected_bundle.ends_with('\n') && !expected_bundle.is_empty() {
            expected_bundle.push('\n');
        }
        expected_bundle.push_str(&format!("# {NAME}\n{legacy_pem}"));
        std::fs::write(dir.path().join("pool-ca.pem"), &legacy_pem).unwrap();
        std::fs::write(dir.path().join("pool-ca.key"), &legacy_key_pem).unwrap();
        std::fs::write(dir.path().join("pool-ca-bundle.pem"), &expected_bundle).unwrap();
        let bundle_mtime_before = std::fs::metadata(dir.path().join("pool-ca-bundle.pem")).unwrap().modified().unwrap();

        let opened = Authority::open_in(dir.path()).unwrap();
        // Legacy files MUST be 100% byte-for-byte untouched (and unchanged bundle not rewritten).
        assert_eq!(std::fs::read_to_string(dir.path().join("pool-ca.pem")).unwrap(), legacy_pem);
        assert_eq!(std::fs::read_to_string(dir.path().join("pool-ca.key")).unwrap(), legacy_key_pem);
        assert_eq!(std::fs::read_to_string(dir.path().join("pool-ca-bundle.pem")).unwrap(), expected_bundle);
        assert_eq!(std::fs::metadata(dir.path().join("pool-ca-bundle.pem")).unwrap().modified().unwrap(), bundle_mtime_before);

        // Companion CA created and selected by `agy_cert_path_in`.
        let companion_pem = opened.agy_cert_pem.as_deref().expect("companion CA created for legacy 2-host install");
        assert!(opened.has_agy() && opened.agy_error.is_none());
        assert_eq!(std::fs::read_to_string(dir.path().join("pool-agy-ca.pem")).unwrap(), companion_pem);
        assert!(dir.path().join("pool-agy-ca.key").is_file());
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-agy-ca.pem"));

        // Verify primary CA signs Claude/Codex and companion CA signs Antigravity (and rejects non-AGY hosts).
        let primary_issuer = Issuer::new(params_for(chrono::Utc::now(), NAME, HOSTS), KeyPair::from_pem(&legacy_key_pem).unwrap());
        assert!(covers_hosts(&legacy_pem, &primary_issuer, LEGACY_HOSTS));
        let agy_issuer = opened.agy_issuer.as_ref().unwrap();
        assert!(covers_hosts(companion_pem, agy_issuer, AGY_HOSTS));

        let agy_der = CertificateDer::from_pem_slice(companion_pem.as_bytes()).unwrap();
        let mut agy_roots = rustls::RootCertStore::empty();
        agy_roots.add(agy_der).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(agy_roots), provider).build().unwrap();
        for forbidden in ["api.anthropic.com", "example.com"] {
            let k = KeyPair::generate().unwrap();
            let mut p = CertificateParams::new(vec![forbidden.to_owned()]).unwrap();
            p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let leaf = p.signed_by(&k, agy_issuer).unwrap();
            assert!(verifier.verify_server_cert(leaf.der(), &[], &ServerName::try_from(forbidden).unwrap(), &[], UnixTime::now()).is_err(), "companion CA must reject {forbidden}");
        }

        // ServerConfig builds cleanly with both issuers wired per-host.
        assert!(opened.server_config().is_ok());

        // Codex bundle still picks up OS system-root updates on legacy installs.
        let fake_roots = dir.path().join("system-roots.pem");
        std::fs::write(&fake_roots, "# updated-os-roots\n").unwrap();
        let fake_roots_s = fake_roots.display().to_string();
        opened.write_bundle_with_roots(&dir.path().join("pool-ca-bundle.pem"), &[&fake_roots_s]).unwrap();
        let updated_bundle = std::fs::read_to_string(dir.path().join("pool-ca-bundle.pem")).unwrap();
        assert!(updated_bundle.starts_with("# updated-os-roots\n# Olo Token Pool\n"), "{updated_bundle}");

        // Opening a second time reuses BOTH CAs byte-for-byte.
        let again = Authority::open_in(dir.path()).unwrap();
        assert_eq!(again.cert_pem, legacy_pem);
        assert_eq!(again.agy_cert_pem.as_deref(), Some(companion_pem));
    }

    /// If companion CA creation fails on an upgraded legacy install (e.g.
    /// unwritable companion path), the established 2-host Claude/Codex proxy
    /// STILL opens and builds a valid ServerConfig while surfacing `agy_error`.
    #[test]
    fn companion_ca_failure_preserves_legacy_claude_and_codex_proxy() {
        let dir = tempfile::Builder::new().prefix("olo-pool-ca-ro-").tempdir_in("/var/tmp").unwrap();
        let legacy_key = KeyPair::generate().unwrap();
        let legacy_pem = params_for(chrono::Utc::now(), NAME, LEGACY_HOSTS).self_signed(&legacy_key).unwrap().pem();
        std::fs::write(dir.path().join("pool-ca.pem"), &legacy_pem).unwrap();
        std::fs::write(dir.path().join("pool-ca.key"), legacy_key.serialize_pem()).unwrap();
        // Make `pool-agy-ca.key` an unremovable non-empty directory so companion creation fails.
        let blocker = dir.path().join("pool-agy-ca.key");
        std::fs::create_dir_all(&blocker).unwrap();
        std::fs::write(blocker.join("keep"), b"x").unwrap();

        let opened = Authority::open_in(dir.path()).unwrap();
        assert_eq!(opened.cert_pem, legacy_pem);
        assert!(!opened.has_agy());
        assert!(opened.agy_error.is_some());
        assert!(opened.server_config().is_ok(), "legacy Claude/Codex ServerConfig must still build");
    }

    /// When a 4-host primary CA exists alongside a stale leftover companion CA,
    /// `agy_cert_path_in` selects `pool-ca.pem` and `open_in` cleans up the
    /// stale companion files so client and server always agree on the CA.
    #[test]
    fn stale_companion_ca_is_ignored_and_cleaned_when_primary_covers_agy() {
        let dir = tempfile::Builder::new().prefix("olo-pool-ca-stale-").tempdir_in("/var/tmp").unwrap();
        let fresh = Authority::open_in(dir.path()).unwrap();
        // Plant a stale companion CA left over from before primary rotation.
        let stale_key = KeyPair::generate().unwrap();
        let stale_pem = params_for(chrono::Utc::now(), AGY_NAME, AGY_HOSTS).self_signed(&stale_key).unwrap().pem();
        std::fs::write(dir.path().join("pool-agy-ca.pem"), &stale_pem).unwrap();
        std::fs::write(dir.path().join("pool-agy-ca.key"), stale_key.serialize_pem()).unwrap();

        // Even BEFORE `open_in` cleans up, `agy_cert_path_in` selects the 4-host primary!
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-ca.pem"));

        let reopened = Authority::open_in(dir.path()).unwrap();
        assert_eq!(reopened.cert_pem, fresh.cert_pem);
        assert!(reopened.agy_cert_pem.is_none());
        assert!(!dir.path().join("pool-agy-ca.pem").exists());
        assert!(!dir.path().join("pool-agy-ca.key").exists());
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-ca.pem"));
    }

    /// A fresh install creates ONE 4-host `pool-ca.pem` covering Claude, Codex,
    /// and Antigravity with no redundant companion file.
    #[test]
    fn fresh_install_creates_single_four_host_ca() {
        let dir = tempfile::Builder::new().prefix("olo-pool-ca-fresh-").tempdir_in("/var/tmp").unwrap();
        let opened = Authority::open_in(dir.path()).unwrap();
        assert!(opened.agy_cert_pem.is_none());
        assert!(opened.has_agy() && opened.agy_error.is_none());
        assert!(!dir.path().join("pool-agy-ca.pem").exists());
        assert!(covers_hosts(&opened.cert_pem, &opened.issuer, HOSTS));
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-ca.pem"));
        assert_eq!(opened.agy_pem(), opened.cert_pem);
        assert!(opened.server_config().is_ok());
    }

    /// Restoring `pool-ca.key` after a negative cache hit invalidates
    /// `primary_covers_agy_cached` (key mtime/len included in CacheKey), and
    /// missing OS root bundles keep an existing bundle untouched while returning
    /// an error on fresh installs.
    #[test]
    fn key_restore_invalidates_cache_and_missing_os_roots_preserve_bundle() {
        let dir = tempfile::Builder::new().prefix("olo-pool-ca-cache-").tempdir_in("/var/tmp").unwrap();
        let opened = Authority::open_in(dir.path()).unwrap();
        let key_path = dir.path().join("pool-ca.key");
        let saved_key = std::fs::read(&key_path).unwrap();

        // Remove key -> negative cache hit -> falls back to companion path.
        std::fs::remove_file(&key_path).unwrap();
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-agy-ca.pem"));

        // Restore key -> CacheKey changes -> immediately resolves back to 4-host pool-ca.pem!
        std::fs::write(&key_path, &saved_key).unwrap();
        assert_eq!(agy_cert_path_in(dir.path()), dir.path().join("pool-ca.pem"));

        // Missing OS roots on an existing bundle keeps the existing bundle intact.
        let bundle = dir.path().join("pool-ca-bundle.pem");
        std::fs::write(&bundle, "# existing-valid-bundle\n").unwrap();
        opened.write_bundle_with_roots(&bundle, &["/var/tmp/definitely-missing-roots-12345.crt"]).unwrap();
        assert_eq!(std::fs::read_to_string(&bundle).unwrap(), "# existing-valid-bundle\n");

        // Missing OS roots when no bundle exists returns an error instead of writing an empty-roots file.
        let fresh_bundle = dir.path().join("fresh-bundle.pem");
        assert!(opened.write_bundle_with_roots(&fresh_bundle, &["/var/tmp/definitely-missing-roots-12345.crt"]).is_err());
        assert!(!fresh_bundle.exists());
    }
}
