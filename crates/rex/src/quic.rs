//! The helper's own link over UDP, from the app's side (`olo_rex_link::
//! quic_wire` says what the two agree on; the helper's `quic` mode is
//! the other end). SSH signs in and starts the helper, hands it a
//! token made for this link and takes back the port it listens on and
//! its certificate's print; from then on the streams to the host's Rex
//! server (the control connection, each terminal's) are streams of one
//! QUIC connection, not channels of the SSH connection:
//! - a terminal's flood holds up only itself (each stream has its own
//!   window; over SSH one TCP connection carries all, in order);
//! - a lost packet holds up the stream it was of for a round trip, not
//!   every stream for a retransmission's wait;
//! - the link goes on through a change of the network under it.
//!
//! Where UDP doesn't get through (a firewall, a proxy between), the
//! link isn't made and everything goes over SSH as before: the app's
//! streams never wait on it for longer than [`WITHIN`], once.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use olo_rex_link::quic_wire as wire;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

use crate::transport::AsyncReadWrite;

/// How long the link is given to come up (its handshake): past it,
/// SSH.
pub const WITHIN: Duration = Duration::from_millis(2500);

/// The helper's certificate is the one whose print SSH brought, and no
/// other: nobody else is asked.
#[derive(Debug)]
struct Pinned {
    print: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

pub fn print_of(certificate: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(certificate).iter().map(|b| format!("{b:02x}")).collect()
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if print_of(end_entity.as_ref()) == self.print { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("not the helper's certificate".into())) }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A link that is up.
pub struct QuicLink {
    connection: quinn::Connection,
    token: [u8; wire::TOKEN],
    /// (Its socket lives as long as the link.)
    _endpoint: quinn::Endpoint,
}

impl std::fmt::Debug for QuicLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicLink").field("to", &self.connection.remote_address()).finish_non_exhaustive()
    }
}

/// A token for one link: what the helper is given over SSH, and what
/// every stream then begins with.
pub fn new_token() -> Result<[u8; wire::TOKEN], String> {
    let mut token = [0u8; wire::TOKEN];
    rustls::crypto::aws_lc_rs::default_provider().secure_random.fill(&mut token).map_err(|_| "no randomness".to_owned())?;
    Ok(token)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the helper said as it began to listen ([`wire::ANNOUNCE`]):
/// its port, and its certificate's print.
pub fn announced(line: &str) -> Option<(u16, String)> {
    let mut words = line.split_whitespace();
    (words.next()? == wire::ANNOUNCE && words.next()?.parse::<u32>().ok()? == wire::VERSION).then_some(())?;
    let port = words.next()?.parse().ok()?;
    let print = words.next()?;
    (print.len() == 64 && print.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (port, print.to_owned()))
}

impl QuicLink {
    /// The link to the helper at `to`, whose certificate has `print`:
    /// made (its handshake, which needs UDP both ways) within
    /// [`WITHIN`].
    pub async fn connect(to: SocketAddr, print: &str, token: [u8; wire::TOKEN]) -> Result<QuicLink, String> {
        let up = async {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| e.to_string())?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Pinned { print: print.to_owned(), provider }))
                .with_no_client_auth();
            tls.alpn_protocols = vec![wire::ALPN.to_vec()];
            let mut config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?));
            let mut transport = quinn::TransportConfig::default();
            transport.max_idle_timeout(Some(wire::IDLE.try_into().map_err(|_| "the idle time")?));
            transport.keep_alive_interval(Some(wire::KEEP_ALIVE));
            config.transport_config(Arc::new(transport));
            let from: SocketAddr = if to.is_ipv6() { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() } else { (std::net::Ipv4Addr::UNSPECIFIED, 0).into() };
            let endpoint = quinn::Endpoint::client(from).map_err(|e| e.to_string())?;
            let connection = endpoint.connect_with(config, to, wire::NAME).map_err(|e| e.to_string())?.await.map_err(|e| e.to_string())?;
            // (The handshake has gone both ways over UDP: the path
            // carries it. No word more is waited for: a round trip
            // saved at every connection.)
            Ok::<_, String>(QuicLink { connection, token, _endpoint: endpoint })
        };
        tokio::time::timeout(WITHIN, up).await.map_err(|_| format!("no answer over UDP from {to} in {} ms", WITHIN.as_millis()))?
    }

    async fn begin(&self, kind: u8) -> Result<(quinn::SendStream, quinn::RecvStream), String> {
        let (mut send, recv) = self.connection.open_bi().await.map_err(|e| e.to_string())?;
        let mut head = [0u8; wire::TOKEN + 1];
        head[..wire::TOKEN].copy_from_slice(&self.token);
        head[wire::TOKEN] = kind;
        send.write_all(&head).await.map_err(|e| e.to_string())?;
        Ok((send, recv))
    }

    /// A stream to the host's Rex server: through the helper's gate
    /// (`control`: the control connection, sent ahead of the others'
    /// bytes where both wait), or straight.
    pub async fn open(&self, control: bool) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, String> {
        let (send, recv) = self.begin(if control { wire::CONTROL } else { wire::PLAIN }).await?;
        if control {
            let _ = send.set_priority(1);
        }
        Ok(Box::pin(tokio::io::join(recv, send)))
    }

    /// A word asked of the helper and had back: the helper takes this
    /// link's token (one that doesn't closes the link).
    pub async fn hello(&self) -> Result<(), String> {
        let (mut send, mut recv) = self.begin(wire::HELLO).await?;
        let _ = send.finish();
        let mut word = [0u8; 1];
        recv.read_exact(&mut word).await.map_err(|e| e.to_string())
    }

    /// The link's round trip, as its own packets have measured it.
    pub fn round_trip(&self) -> Duration {
        self.connection.rtt()
    }

    /// Whether it is still up.
    pub fn alive(&self) -> bool {
        self.connection.close_reason().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_word_is_read() {
        let print = "ab".repeat(32);
        assert_eq!(announced(&format!("olo-rex-quic 1 61100 {print}\n")), Some((61100, print.clone())));
        // Another version of the wire, a print that isn't one, another word: no link.
        assert_eq!(announced(&format!("olo-rex-quic 2 61100 {print}")), None);
        assert_eq!(announced("olo-rex-quic 1 61100 abc"), None);
        assert_eq!(announced("rex: command not found"), None);
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_ne!(new_token().unwrap(), new_token().unwrap());
    }
}
