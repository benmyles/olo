//! Signing in to Google (Drive's store): with a "Desktop app" OAuth client
//! each person makes in their own Google Cloud project (DiskBackup's setup
//! walks them through it), as Google's installed-app flow has it: the
//! browser sent to Google's consent page, the answer to a loopback address
//! of ours (`http://127.0.0.1:PORT`, a port of its own each time), the
//! code made a refresh token with PKCE (S256). The scopes: `drive.file`
//! (only the files DiskBackup makes; non-sensitive, so nothing for Google
//! to review) and `openid email` (whose Drive it is, to say).

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

pub const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.file";

/// Where Google is (a test's own server, else Google's).
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    /// The APIs' root (`/drive/v3/files`, `/upload/drive/v3/files` under it).
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Endpoints {
        Endpoints { auth: "https://accounts.google.com/o/oauth2/v2/auth".into(), token: "https://oauth2.googleapis.com/token".into(), api: "https://www.googleapis.com".into() }
    }
}

/// A person's own OAuth client ("Desktop app"): its id and secret (a
/// desktop client's secret isn't secret to Google, but it's kept as one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

impl Client {
    /// From the file Google Cloud's Download JSON gives (`{"installed":
    /// {"client_id": …, "client_secret": …}}`).
    pub fn from_json(text: &str) -> Result<Client, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "that isn't a client's file from Google Cloud (it isn't JSON)".to_string())?;
        if v.get("web").is_some() {
            return Err("that's a web application's client: make a Desktop app's (Application type: Desktop app)".into());
        }
        let c = v.get("installed").ok_or("that isn't a Desktop app client's file from Google Cloud")?;
        let get = |k: &str| c.get(k).and_then(|s| s.as_str()).map(str::to_owned).filter(|s| !s.is_empty());
        Ok(Client { id: get("client_id").ok_or("the file has no client_id")?, secret: get("client_secret").ok_or("the file has no client_secret")? })
    }

    /// Plausible as typed (an id ends `.apps.googleusercontent.com`).
    pub fn check(&self) -> Result<(), String> {
        if !self.id.trim().ends_with(".apps.googleusercontent.com") {
            return Err("a client ID ends with .apps.googleusercontent.com".into());
        }
        if self.secret.trim().is_empty() {
            return Err("the client secret is missing".into());
        }
        Ok(())
    }
}

/// Signed in: the refresh token (kept), and whose account it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedIn {
    pub refresh_token: String,
    pub email: String,
}

/// An HTTP agent for Google: its TLS the system's roots, statuses as
/// values (an error's body says why), no redirect followed (a resumable
/// upload's "go on" is a 308 with no Location: followed, it's lost). A
/// read or write that makes no progress for two minutes fails (a network
/// that went without saying so: one that says so has its connections cut
/// at once, `olo_net`).
pub fn agent() -> ureq::Agent {
    olo_net::watch();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent("Olo DiskBackup")
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(120)))
        .timeout_per_write(Some(Duration::from_secs(120)))
        .build()
        .into()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random(n: usize) -> String {
    let mut raw = vec![0u8; n];
    getrandom::fill(&mut raw).expect("the system's random source failed");
    b64(&raw)
}

/// `s` for a URL's query.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A sign-in under way: its consent page's address (for the browser), and
/// the loopback address waiting for Google's answer.
pub struct SignIn {
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
    client: Client,
    endpoints: Endpoints,
    url: String,
}

impl SignIn {
    pub fn start(client: &Client, endpoints: &Endpoints) -> std::io::Result<SignIn> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let verifier = random(48);
        let challenge = b64(&sha2::Sha256::digest(verifier.as_bytes()));
        let state = random(16);
        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
            endpoints.auth,
            escape(client.id.trim()),
            escape(&redirect),
            escape(SCOPES)
        );
        Ok(SignIn { listener, redirect, verifier, state, client: client.clone(), endpoints: endpoints.clone(), url })
    }

    /// Google's consent page, for the browser.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Google's answer (the browser sent back here), made a refresh token;
    /// stopped by `cancel`, or after 15 minutes.
    pub fn wait(self, http: &ureq::Agent, cancel: &AtomicBool) -> Result<SignedIn, String> {
        self.listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let until = Instant::now() + Duration::from_secs(15 * 60);
        let code = loop {
            if cancel.load(Relaxed) {
                return Err("stopped".into());
            }
            if Instant::now() > until {
                return Err("no answer from Google in 15 minutes".into());
            }
            let (mut conn, _) = match self.listener.accept() {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };
            let _ = conn.set_nonblocking(false);
            let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
            let mut line = String::new();
            let _ = BufReader::new(&conn).read_line(&mut line);
            // GET /?state=…&code=… HTTP/1.1 (a favicon's asked for too).
            let target = line.split_whitespace().nth(1).unwrap_or("");
            let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
            let params: Vec<(String, String)> = query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), unescape(v))).collect();
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            if get("state").is_none() {
                let _ = conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let (page, result) = if get("state").as_deref() != Some(self.state.as_str()) {
                ("That answer wasn't for this sign-in. Go back to DiskBackup and try again.", Err("the answer wasn't for this sign-in".to_string()))
            } else if let Some(e) = get("error") {
                let why = if e == "access_denied" { "you didn't allow it".to_string() } else { e };
                ("DiskBackup wasn't let in. Go back to DiskBackup to try again.", Err(format!("Google said no: {why}")))
            } else if let Some(code) = get("code") {
                ("Signed in. You can close this tab and go back to DiskBackup.", Ok(code))
            } else {
                ("Google's answer had no code. Go back to DiskBackup and try again.", Err("Google's answer had no code".to_string()))
            };
            let body = format!("<!doctype html><meta charset=utf-8><title>DiskBackup</title><body style=\"font: 15px system-ui, sans-serif; margin: 3em; color: #222\"><p>{page}</p></body>");
            let _ = conn.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            break result?;
        };
        let form = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&client_secret={}&code_verifier={}",
            escape(&code),
            escape(&self.redirect),
            escape(self.client.id.trim()),
            escape(self.client.secret.trim()),
            escape(&self.verifier)
        );
        let v = post_form(http, &self.endpoints.token, &form)?;
        let refresh_token = v.get("refresh_token").and_then(|t| t.as_str()).ok_or("Google gave no refresh token (sign in again)")?.to_owned();
        let email = v.get("id_token").and_then(|t| t.as_str()).and_then(email_of).unwrap_or_default();
        Ok(SignedIn { refresh_token, email })
    }
}

/// The email in an ID token (straight from Google's token endpoint over
/// TLS: its signature isn't checked again).
fn email_of(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("email").and_then(|e| e.as_str()).map(str::to_owned)
}

/// A form posted to `url`; its JSON answer (an error's description said).
pub(crate) fn post_form(http: &ureq::Agent, url: &str, form: &str) -> Result<serde_json::Value, String> {
    let resp = http.post(url).header("Content-Type", "application/x-www-form-urlencoded").send(form).map_err(|e| format!("Google can't be reached: {e}"))?;
    let status = resp.status().as_u16();
    let mut text = String::new();
    let _ = resp.into_body().into_reader().read_to_string(&mut text);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if !(200..300).contains(&status) {
        let e = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
        let d = v.get("error_description").and_then(|e| e.as_str()).unwrap_or("");
        return Err(match e {
            "invalid_client" => "Google doesn't know that client: check its ID and secret".into(),
            "invalid_grant" => "Google signed DiskBackup out (the sign-in was revoked or expired): sign in again".into(),
            _ => format!("Google said {status} {e} {d}").trim().to_owned(),
        });
    }
    Ok(v)
}

/// A fresh access token for `refresh`, and how long it lasts.
pub fn access_token(http: &ureq::Agent, endpoints: &Endpoints, client: &Client, refresh: &str) -> Result<(String, Duration), String> {
    let form = format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", escape(refresh), escape(client.id.trim()), escape(client.secret.trim()));
    let v = post_form(http, &endpoints.token, &form)?;
    let token = v.get("access_token").and_then(|t| t.as_str()).ok_or("Google gave no access token")?.to_owned();
    let secs = v.get("expires_in").and_then(|t| t.as_u64()).unwrap_or(3600);
    Ok((token, Duration::from_secs(secs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clients_file() {
        let c = Client::from_json(r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","project_id":"olo","client_secret":"GOCSPX-x","redirect_uris":["http://localhost"]}}"#).unwrap();
        assert_eq!(c, Client { id: "123-abc.apps.googleusercontent.com".into(), secret: "GOCSPX-x".into() });
        assert!(c.check().is_ok());
        assert!(Client::from_json(r#"{"web":{"client_id":"x"}}"#).unwrap_err().contains("Desktop app"));
        assert!(Client { id: "nope".into(), secret: "s".into() }.check().is_err());
        assert_eq!(unescape("a%2Fb+c%3D"), "a/b c=");
        assert_eq!(escape("a b/c"), "a%20b%2Fc");
    }
}
