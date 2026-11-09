use alloc::boxed::Box;
use alloc::vec::Vec;

use super::ResolvesClientCert;
use crate::log::{debug, trace};
use crate::msgs::enums::ExtensionType;
use crate::msgs::handshake::{CertificateChain, DistinguishedName, ProtocolName, ServerExtensions};
use crate::sync::Arc;
use crate::{CipherSuite, SignatureScheme, compress, sign};

#[derive(Debug)]
pub(super) struct ServerCertDetails<'a> {
    pub(super) cert_chain: CertificateChain<'a>,
    pub(super) ocsp_response: Vec<u8>,
}

impl<'a> ServerCertDetails<'a> {
    pub(super) fn new(cert_chain: CertificateChain<'a>, ocsp_response: Vec<u8>) -> Self {
        Self {
            cert_chain,
            ocsp_response,
        }
    }

    pub(super) fn into_owned(self) -> ServerCertDetails<'static> {
        let Self {
            cert_chain,
            ocsp_response,
        } = self;
        ServerCertDetails {
            cert_chain: cert_chain.into_owned(),
            ocsp_response,
        }
    }
}

pub(super) struct ClientHelloDetails {
    pub(super) alpn_protocols: Vec<ProtocolName>,
    pub(super) sent_extensions: Vec<ExtensionType>,
    pub(super) extension_order_seed: u16,
    pub(super) offered_cert_compression: bool,
    pub(super) offered_cipher_suites: Vec<CipherSuite>,
    /// Olo: what a Chrome hello chose at random, kept for a second hello
    /// (after a HelloRetryRequest) to say the same.
    pub(super) chrome: Option<ChromeHello>,
}

/// Olo: a Chrome hello's random parts (BoringSSL's): GREASE values (a
/// random high nibble, `0x?A?A`) for its cipher suites, groups, first and
/// last extensions, versions and signature algorithms, and its GREASE ECH
/// extension (a random config id, X25519 share and payload of 128, 160,
/// 192 or 224 bytes and the AEAD's 16).
#[derive(Clone, Debug)]
pub(super) struct ChromeHello {
    pub(super) grease: [u16; 6],
    pub(super) ech: Vec<u8>,
}

impl ChromeHello {
    pub(super) const CIPHER: usize = 0;
    pub(super) const GROUP: usize = 1;
    pub(super) const FIRST: usize = 2;
    pub(super) const LAST: usize = 3;
    pub(super) const VERSION: usize = 4;
    pub(super) const SIGNATURE: usize = 5;

    pub(super) fn new(random: &dyn crate::crypto::SecureRandom) -> Result<Self, crate::rand::GetRandomFailed> {
        let mut seed = [0u8; 8];
        random.fill(&mut seed)?;
        let mut grease = [0u16; 6];
        for (g, s) in grease.iter_mut().zip(seed) {
            let v = u16::from((s & 0xf0) | 0x0a);
            *g = (v << 8) | v;
        }
        // The two extensions differ (as BoringSSL makes them).
        if grease[Self::LAST] == grease[Self::FIRST] {
            grease[Self::LAST] ^= 0x1010;
        }
        let payload = 128 + 32 * usize::from(seed[6] % 4) + 16;
        let mut random_part = alloc::vec![0u8; 1 + 32 + payload];
        random.fill(&mut random_part)?;
        let mut ech = Vec::with_capacity(8 + random_part.len());
        // Outer; HKDF-SHA256, AES-128-GCM; its config id; its share; its payload.
        ech.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x01, random_part[0], 0x00, 0x20]);
        ech.extend_from_slice(&random_part[1..33]);
        ech.extend_from_slice(&(payload as u16).to_be_bytes());
        ech.extend_from_slice(&random_part[33..]);
        Ok(Self { grease, ech })
    }
}

impl ClientHelloDetails {
    pub(super) fn new(alpn_protocols: Vec<ProtocolName>, extension_order_seed: u16) -> Self {
        Self {
            alpn_protocols,
            sent_extensions: Vec::new(),
            extension_order_seed,
            offered_cert_compression: false,
            offered_cipher_suites: Vec::new(),
            chrome: None,
        }
    }

    pub(super) fn server_sent_unsolicited_extensions(
        &self,
        received_exts: &ServerExtensions<'_>,
        allowed_unsolicited: &[ExtensionType],
    ) -> bool {
        let mut extensions = received_exts.collect_used();
        extensions.extend(
            received_exts
                .unknown_extensions
                .iter()
                .map(|ext| ExtensionType::from(*ext)),
        );
        for ext_type in extensions {
            if !self.sent_extensions.contains(&ext_type) && !allowed_unsolicited.contains(&ext_type)
            {
                trace!("Unsolicited extension {ext_type:?}");
                return true;
            }
        }

        false
    }
}

pub(super) enum ClientAuthDetails {
    /// Send an empty `Certificate` and no `CertificateVerify`.
    Empty { auth_context_tls13: Option<Vec<u8>> },
    /// Send a non-empty `Certificate` and a `CertificateVerify`.
    Verify {
        certkey: Arc<sign::CertifiedKey>,
        signer: Box<dyn sign::Signer>,
        auth_context_tls13: Option<Vec<u8>>,
        compressor: Option<&'static dyn compress::CertCompressor>,
    },
}

impl ClientAuthDetails {
    pub(super) fn resolve(
        resolver: &dyn ResolvesClientCert,
        canames: Option<&[DistinguishedName]>,
        sigschemes: &[SignatureScheme],
        auth_context_tls13: Option<Vec<u8>>,
        compressor: Option<&'static dyn compress::CertCompressor>,
    ) -> Self {
        let acceptable_issuers = canames
            .unwrap_or_default()
            .iter()
            .map(|p| p.as_ref())
            .collect::<Vec<&[u8]>>();

        if let Some(certkey) = resolver.resolve(&acceptable_issuers, sigschemes) {
            if let Some(signer) = certkey.key.choose_scheme(sigschemes) {
                debug!("Attempting client auth");
                return Self::Verify {
                    certkey,
                    signer,
                    auth_context_tls13,
                    compressor,
                };
            }
        }

        debug!("Client auth requested but no cert/sigscheme available");
        Self::Empty { auth_context_tls13 }
    }
}
