//! The pool's certificate authority, for the proxy (`intercept`): made
//! once and kept beside the pool's other files (its key readable only by
//! this user), it vouches only for the providers' own hosts (a name
//! constraint in the certificate itself, so even its key could vouch for
//! nothing else), and the proxy signs a certificate for each of those
//! hosts with it as it starts. Claude Code adds it to what it trusts
//! (`NODE_EXTRA_CA_CERTS`); Codex takes a bundle of the system's roots
//! and it (`SSL_CERT_FILE`), as its CA setting replaces the roots.

use std::{path::PathBuf, sync::Arc};

use chrono::Datelike;
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The hosts the pool answers for: the subscriptions' model APIs.
pub const HOSTS: &[&str] = &["api.anthropic.com", "chatgpt.com"];

const NAME: &str = "Olo Token Pool";

/// Where the authority's certificate is (for the clients to trust).
pub fn cert_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-ca.pem")
}

fn key_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-ca.key")
}

/// The system's roots and the authority's, for clients whose CA setting
/// replaces the roots (Codex).
pub fn bundle_path() -> PathBuf {
    olo_xdg::config_dir().join("pool-ca-bundle.pem")
}

/// Where distributions keep the system's roots.
const SYSTEM_ROOTS: &[&str] = &["/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/cert.pem", "/etc/pki/tls/cacert.pem"];

fn params(now: chrono::DateTime<chrono::Utc>) -> CertificateParams {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, NAME);
    dn.push(DnType::OrganizationName, "Olo");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.name_constraints = Some(NameConstraints { permitted_subtrees: HOSTS.iter().map(|h| GeneralSubtree::DnsName((*h).into())).collect(), excluded_subtrees: Vec::new() });
    p.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    p.not_after = rcgen::date_time_ymd(now.year() + 10, now.month() as u8, now.day().min(28) as u8);
    p
}

/// The authority: its certificate (PEM) and what signs with it.
pub struct Authority {
    pub cert_pem: String,
    issuer: Issuer<'static, KeyPair>,
}

impl Authority {
    /// The one kept, else a new one (written: certificate, key, bundle).
    pub fn open() -> Result<Authority, String> {
        let (cert, key) = (cert_path(), key_path());
        let kept = std::fs::read_to_string(&cert).ok().zip(std::fs::read_to_string(&key).ok());
        let authority = match kept.and_then(|(c, k)| Some((c, KeyPair::from_pem(&k).ok()?))) {
            Some((cert_pem, key)) => {
                // Its name and key are all signing needs (the dates and
                // serial are the certificate's own business).
                Authority { cert_pem, issuer: Issuer::new(params(chrono::Utc::now()), key) }
            }
            None => {
                let key = KeyPair::generate().map_err(|e| e.to_string())?;
                let p = params(chrono::Utc::now());
                let cert_pem = p.self_signed(&key).map_err(|e| e.to_string())?.pem();
                crate::store::write_private(&key_path(), key.serialize_pem().as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s certificate key: {e}"))?;
                write_public(&cert_path(), cert_pem.as_bytes()).map_err(|e| format!("Couldn’t keep the pool’s certificate: {e}"))?;
                Authority { cert_pem, issuer: Issuer::new(p, key) }
            }
        };
        authority.write_bundle();
        Ok(authority)
    }

    /// An authority kept nowhere (tests).
    pub fn ephemeral() -> Authority {
        let key = KeyPair::generate().expect("a key");
        let p = params(chrono::Utc::now());
        let cert_pem = p.self_signed(&key).expect("a certificate").pem();
        Authority { cert_pem, issuer: Issuer::new(p, key) }
    }

    /// The system's roots with ours after them.
    fn write_bundle(&self) {
        let mut bundle = SYSTEM_ROOTS.iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| {
            tracing::warn!("pool: found none of the system's root certificates for Codex's bundle");
            String::new()
        });
        if !bundle.ends_with('\n') && !bundle.is_empty() {
            bundle.push('\n');
        }
        bundle.push_str(&format!("# {NAME}\n{}", self.cert_pem));
        if std::fs::read_to_string(bundle_path()).ok().as_deref() != Some(bundle.as_str())
            && let Err(e) = write_public(&bundle_path(), bundle.as_bytes())
        {
            tracing::warn!("pool: couldn’t write the certificate bundle: {e}");
        }
    }

    /// A TLS server configuration presenting a certificate for each of
    /// `HOSTS` (by the name the client asks for), made now.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        use rustls::pki_types::pem::PemObject;
        let ca_der = CertificateDer::from_pem_slice(self.cert_pem.as_bytes()).map_err(|e| e.to_string())?;
        let mut resolver = rustls::server::ResolvesServerCertUsingSni::new();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        for host in HOSTS {
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
            let cert = p.signed_by(&key, &self.issuer).map_err(|e| e.to_string())?;
            let chain: Vec<CertificateDer<'static>> = vec![cert.der().clone(), ca_der.clone()];
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

/// Written readable by anyone (a certificate is public), atomically.
fn write_public(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}
