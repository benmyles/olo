//! One-time passwords (RFC 6238) from a TOTP field's value: an
//! `otpauth://totp/…` URI or a bare base32 secret.

use hmac::{Hmac, KeyInit, Mac};

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Totp {
    pub secret: Vec<u8>,
    pub algorithm: Algorithm,
    pub digits: u32,
    /// Seconds each code lasts.
    pub period: u64,
    pub issuer: Option<String>,
    pub account: Option<String>,
}

impl Drop for Totp {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.secret);
    }
}

fn bad(why: &str) -> Error {
    Error::InvalidArgument(format!("not a one-time password secret: {why}"))
}

fn base32(text: &str) -> Result<Vec<u8>, Error> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '=').map(|c| c.to_ascii_uppercase()).collect();
    if clean.is_empty() {
        return Err(bad("the secret is empty"));
    }
    let mut spec = data_encoding::Specification::new();
    spec.symbols.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567");
    spec.check_trailing_bits = false;
    let encoding = spec.encoding().map_err(|e| bad(&e.to_string()))?;
    encoding.decode(clean.as_bytes()).map_err(|_| bad("the secret isn't base32"))
}

impl Totp {
    pub fn parse(value: &str) -> Result<Totp, Error> {
        let value = value.trim();
        if !value.to_ascii_lowercase().starts_with("otpauth://") {
            return Ok(Totp { secret: base32(value)?, algorithm: Algorithm::Sha1, digits: 6, period: 30, issuer: None, account: None });
        }
        let url = url::Url::parse(value).map_err(|_| bad("the URI doesn't parse"))?;
        if url.host_str().map(str::to_ascii_lowercase).as_deref() != Some("totp") {
            return Err(bad("only time-based (totp) URIs are supported"));
        }
        let label = percent_decode(url.path().trim_start_matches('/'));
        let (mut issuer, account) = match label.split_once(':') {
            Some((issuer, account)) => (Some(issuer.trim().to_string()), Some(account.trim().to_string())),
            None if label.is_empty() => (None, None),
            None => (None, Some(label)),
        };
        let mut totp = Totp { secret: Vec::new(), algorithm: Algorithm::Sha1, digits: 6, period: 30, issuer: None, account };
        for (key, value) in url.query_pairs() {
            match key.to_ascii_lowercase().as_str() {
                "secret" => totp.secret = base32(&value)?,
                "algorithm" => {
                    totp.algorithm = match value.to_ascii_uppercase().as_str() {
                        "SHA1" => Algorithm::Sha1,
                        "SHA256" => Algorithm::Sha256,
                        "SHA512" => Algorithm::Sha512,
                        _ => return Err(bad("unknown algorithm")),
                    }
                }
                "digits" => totp.digits = value.parse().ok().filter(|d| (6..=10).contains(d)).ok_or_else(|| bad("digits"))?,
                "period" => totp.period = value.parse().ok().filter(|p| *p > 0).ok_or_else(|| bad("period"))?,
                "issuer" => issuer = Some(value.into_owned()),
                _ => {}
            }
        }
        if totp.secret.is_empty() {
            return Err(bad("the URI has no secret"));
        }
        totp.issuer = issuer;
        Ok(totp)
    }

    /// The code for a moment (seconds since the epoch).
    pub fn code_at(&self, unix: u64) -> String {
        let counter = (unix / self.period).to_be_bytes();
        let digest = mac(self.algorithm, &self.secret, &counter);
        let offset = (digest[digest.len() - 1] & 0x0f) as usize;
        let bin = u32::from_be_bytes([digest[offset] & 0x7f, digest[offset + 1], digest[offset + 2], digest[offset + 3]]);
        let modulus = 10u64.pow(self.digits);
        format!("{:0width$}", bin as u64 % modulus, width = self.digits as usize)
    }

    /// The current code and the seconds it has left.
    pub fn now(&self) -> (String, u64) {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        (self.code_at(unix), self.period - unix % self.period)
    }
}

fn mac(algorithm: Algorithm, key: &[u8], message: &[u8]) -> Vec<u8> {
    macro_rules! run {
        ($d:ty) => {{
            let mut mac = <Hmac<$d> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
            mac.update(message);
            mac.finalize().into_bytes().to_vec()
        }};
    }
    match algorithm {
        Algorithm::Sha1 => run!(sha1::Sha1),
        Algorithm::Sha256 => run!(sha2::Sha256),
        Algorithm::Sha512 => run!(sha2::Sha512),
    }
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", text.replace('+', "%2B")).as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B.
    #[test]
    fn rfc_vectors() {
        let cases = [
            (Algorithm::Sha1, b"12345678901234567890".to_vec()),
            (Algorithm::Sha256, b"12345678901234567890123456789012".to_vec()),
            (Algorithm::Sha512, b"1234567890123456789012345678901234567890123456789012345678901234".to_vec()),
        ];
        let expected = [
            (59u64, ["94287082", "46119246", "90693936"]),
            (1111111109, ["07081804", "68084774", "25091201"]),
            (1234567890, ["89005924", "91819424", "93441116"]),
            (20000000000, ["65353130", "77737706", "47863826"]),
        ];
        for (i, (algorithm, secret)) in cases.into_iter().enumerate() {
            let totp = Totp { secret, algorithm, digits: 8, period: 30, issuer: None, account: None };
            for (t, codes) in expected {
                assert_eq!(totp.code_at(t), codes[i], "{algorithm:?} at {t}");
            }
        }
    }

    #[test]
    fn parses_uris_and_secrets() {
        let t = Totp::parse("otpauth://totp/ACME%20Co:jane@example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=ACME%20Co&period=60&digits=8&algorithm=SHA256").unwrap();
        assert_eq!(t.secret, b"12345678901234567890");
        assert_eq!((t.algorithm, t.digits, t.period), (Algorithm::Sha256, 8, 60));
        assert_eq!(t.issuer.as_deref(), Some("ACME Co"));
        assert_eq!(t.account.as_deref(), Some("jane@example.com"));
        let t = Totp::parse("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap();
        assert_eq!(t.secret, b"12345678901234567890");
        assert_eq!(t.code_at(59), "287082");
        assert!(Totp::parse("otpauth://hotp/x?secret=GEZDGNBV").is_err());
        assert!(Totp::parse("not base32!").is_err());
    }
}
