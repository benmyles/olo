//! Passkeys from Passwords: a user script gives pages WebAuthn
//! (`navigator.credentials.create/get` with `publicKey`, and
//! `PublicKeyCredential`), sending each request to NetPositive through
//! `console.log` with the per-process token, like the login capture.
//! NetPositive checks the relying party against the page, asks before
//! anything happens (the bar under the toolbar), builds the client data
//! and has Passwords make or sign with the key; the answer goes back into
//! the page through a function only the token names.
//!
//! Top frames only, no conditional (autofill) mediation, ES256 only.

use serde::Deserialize;
use servo::is_reg_domain;
use tabula_keychain::Passkey;
use url::Url;

/// What the page asked for.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PageRequest {
    Create { id: u64, rp: Rp, user: User, challenge: String, exclude: Vec<String> },
    Get { id: u64, rp_id: Option<String>, challenge: String, allow: Vec<String> },
    /// Aborted by the page.
    Cancel { id: u64 },
}

#[derive(Debug, Deserialize)]
pub struct Rp {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

/// A page's request waiting on the bar under the toolbar.
pub struct Ask {
    /// The page's request number.
    pub id: u64,
    pub url: String,
    pub rp_id: String,
    pub challenge: String,
    pub kind: AskKind,
    /// Waiting for Passwords.
    pub busy: bool,
    pub error: Option<String>,
}

pub enum AskKind {
    Create { rp_name: String, user_id: String, user_name: String, display_name: String, exclude: Vec<String> },
    /// `passkeys` is None until Passwords says which there are.
    Get { allow: Vec<String>, passkeys: Option<Vec<Passkey>>, chosen: usize },
}

impl Ask {
    pub fn host(&self) -> String {
        tabula_keychain::host(&self.url).unwrap_or_else(|| self.rp_id.clone())
    }

    pub fn message(&self) -> String {
        let host = self.host();
        match &self.kind {
            AskKind::Create { user_name, display_name, .. } => {
                let who = if user_name.is_empty() { display_name } else { user_name };
                if who.is_empty() {
                    format!("Save a passkey for {host} in Passwords?")
                } else {
                    format!("Save a passkey for “{who}” on {host} in Passwords?")
                }
            }
            AskKind::Get { passkeys: None, .. } => format!("{host} wants to sign in with a passkey…"),
            AskKind::Get { passkeys: Some(p), .. } if p.is_empty() => format!("Passwords has no passkey for {host}."),
            AskKind::Get { passkeys: Some(p), chosen, .. } if p.len() == 1 => {
                format!("Sign in to {host} with your passkey for “{}”?", label(&p[*chosen]))
            }
            AskKind::Get { .. } => format!("Sign in to {host} with a passkey:"),
        }
    }
}

/// How a passkey is named in the bar.
pub fn label(p: &Passkey) -> String {
    if p.user_name.is_empty() { p.title.clone() } else { p.user_name.clone() }
}

/// The relying party a page at `url` may use: `requested`, or its own
/// host; the host itself or a registrable domain above it, on a secure
/// page.
pub fn rp_id(requested: Option<&str>, url: &str) -> Result<String, String> {
    let page = Url::parse(url).map_err(|e| e.to_string())?;
    let host = page.host_str().unwrap_or("").to_ascii_lowercase();
    let rp = requested.map(str::to_ascii_lowercase).unwrap_or_else(|| host.clone());
    if !tabula_keychain::rp_id_fits(&rp, url) {
        return Err(format!("This page can’t use passkeys for {rp}."));
    }
    // Not a public suffix: it, or a domain it's under, is registrable.
    let registrable = rp == host || {
        let mut d = rp.as_str();
        loop {
            if is_reg_domain(d) {
                break true;
            }
            match d.split_once('.') {
                Some((_, rest)) => d = rest,
                None => break false,
            }
        }
    };
    if !registrable {
        return Err(format!("{rp} isn’t a site that can have passkeys."));
    }
    Ok(rp)
}

/// The client data the page and the signature cover.
pub fn client_data(kind: &str, challenge: &str, url: &str) -> String {
    let origin = Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_default();
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    format!(r#"{{"type":{},"challenge":{},"origin":{},"crossOrigin":false}}"#, q(kind), q(challenge), q(&origin))
}

/// Unpadded base64url of SHA-256.
pub fn hash(data: &str) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use sha2::{Digest, Sha256};
    B64.encode(Sha256::digest(data.as_bytes()))
}

pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-passkey-{token}:")
}

fn answer_name(token: &str) -> String {
    format!("__tabulaPasskey{token}")
}

/// Settles the page's request `id`: with `Ok(credential)` (the fields the
/// script turns into a PublicKeyCredential) or `Err((DOMException name,
/// message))`.
pub fn answer_script(token: &str, id: u64, result: Result<serde_json::Value, (&str, &str)>) -> String {
    let (error, value) = match result {
        Ok(v) => (serde_json::Value::Null, v),
        Err((name, message)) => (serde_json::json!([name, message]), serde_json::Value::Null),
    };
    format!("window[{}]({id}, {error}, {value})", serde_json::to_string(&answer_name(token)).unwrap_or_default())
}

/// Settle the page's request `id` (see [`answer_script`]).
pub fn settle(view: &servo::WebView, token: &str, id: u64, result: Result<serde_json::Value, (&str, &str)>) {
    view.evaluate_javascript(answer_script(token, id, result), |_| {});
}

/// The page side.
pub fn script(token: &str) -> String {
    let q = |s: String| serde_json::to_string(&s).unwrap_or_default();
    format!(
        r#"(function() {{
  if (window.top !== window || !window.isSecureContext) return;
  var log = console.log.bind(console), K = {marker}, pending = {{}}, next = 1;
  var hidden = new WeakMap();
  function b64(v) {{
    var u = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
    var s = '';
    for (var i = 0; i < u.length; i++) s += String.fromCharCode(u[i]);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}
  function buf(s) {{
    s = s.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    var b = atob(s), u = new Uint8Array(b.length);
    for (var i = 0; i < b.length; i++) u[i] = b.charCodeAt(i);
    return u.buffer;
  }}
  function fail(name, message) {{ return new DOMException(message, name); }}
  function illegal() {{ throw new TypeError('Illegal constructor'); }}
  function cls(name, parent) {{
    var C = function() {{ illegal(); }};
    Object.defineProperty(C, 'name', {{value: name}});
    if (parent) C.prototype = Object.create(parent.prototype, {{constructor: {{value: C}}}});
    Object.defineProperty(C.prototype, Symbol.toStringTag, {{value: name}});
    Object.defineProperty(window, name, {{value: C, writable: true, configurable: true}});
    return C;
  }}
  function getter(C, name) {{
    Object.defineProperty(C.prototype, name, {{get: function() {{ return hidden.get(this)[name]; }}, enumerable: true}});
  }}
  var Credential = window.Credential || cls('Credential');
  var PKC = cls('PublicKeyCredential', Credential);
  var Resp = cls('AuthenticatorResponse');
  var Att = cls('AuthenticatorAttestationResponse', Resp);
  var Asr = cls('AuthenticatorAssertionResponse', Resp);
  ['id', 'type'].forEach(function(n) {{ getter(PKC, n); }});
  ['rawId', 'response', 'authenticatorAttachment'].forEach(function(n) {{ getter(PKC, n); }});
  getter(Resp, 'clientDataJSON');
  getter(Att, 'attestationObject');
  ['authenticatorData', 'signature', 'userHandle'].forEach(function(n) {{ getter(Asr, n); }});
  PKC.prototype.getClientExtensionResults = function() {{ return {{}}; }};
  PKC.prototype.toJSON = function() {{
    var h = hidden.get(this), r = hidden.get(h.response), out = {{clientDataJSON: b64(r.clientDataJSON)}};
    if (r.attestationObject) {{
      out.attestationObject = b64(r.attestationObject);
      out.authenticatorData = r.data.authenticatorData;
      out.transports = ['internal', 'hybrid'];
      out.publicKey = r.data.publicKey;
      out.publicKeyAlgorithm = -7;
    }} else {{
      out.authenticatorData = b64(r.authenticatorData);
      out.signature = b64(r.signature);
      out.userHandle = r.userHandle ? b64(r.userHandle) : null;
    }}
    return {{id: h.id, rawId: h.id, type: 'public-key', authenticatorAttachment: 'platform', response: out,
             clientExtensionResults: {{}}}};
  }};
  Att.prototype.getTransports = function() {{ return ['internal', 'hybrid']; }};
  Att.prototype.getAuthenticatorData = function() {{ return buf(hidden.get(this).data.authenticatorData); }};
  Att.prototype.getPublicKey = function() {{ return buf(hidden.get(this).data.publicKey); }};
  Att.prototype.getPublicKeyAlgorithm = function() {{ return -7; }};
  PKC.isUserVerifyingPlatformAuthenticatorAvailable = function() {{ return Promise.resolve(true); }};
  PKC.isConditionalMediationAvailable = function() {{ return Promise.resolve(false); }};
  PKC.getClientCapabilities = function() {{
    return Promise.resolve({{conditionalCreate: false, conditionalGet: false, hybridTransport: false,
      passkeyPlatformAuthenticator: true, userVerifyingPlatformAuthenticator: true, relatedOrigins: false,
      signalAllAcceptedCredentials: false, signalCurrentUserDetails: false, signalUnknownCredential: false}});
  }};
  function ids(list) {{
    return (list || []).map(function(c) {{ return Object.assign({{}}, c, {{id: buf(c.id)}}); }});
  }}
  PKC.parseCreationOptionsFromJSON = function(o) {{
    return Object.assign({{}}, o, {{challenge: buf(o.challenge), user: Object.assign({{}}, o.user, {{id: buf(o.user.id)}}),
      excludeCredentials: ids(o.excludeCredentials)}});
  }};
  PKC.parseRequestOptionsFromJSON = function(o) {{
    return Object.assign({{}}, o, {{challenge: buf(o.challenge), allowCredentials: ids(o.allowCredentials)}});
  }};
  function make(C, values) {{
    var o = Object.create(C.prototype);
    hidden.set(o, values);
    return o;
  }}
  Object.defineProperty(window, {answer}, {{value: function(id, error, r) {{
    var p = pending[id];
    if (!p) return;
    delete pending[id];
    if (error) return p.reject(fail(error[0], error[1]));
    var cdj = new TextEncoder().encode(r.clientDataJSON).buffer, response;
    if (p.op === 'create') {{
      response = make(Att, {{clientDataJSON: cdj, attestationObject: buf(r.attestationObject), data: r}});
    }} else {{
      response = make(Asr, {{clientDataJSON: cdj, authenticatorData: buf(r.authenticatorData),
        signature: buf(r.signature), userHandle: r.userHandle ? buf(r.userHandle) : null}});
    }}
    p.resolve(make(PKC, {{id: r.id, rawId: buf(r.id), type: 'public-key', response: response,
      authenticatorAttachment: 'platform'}}));
  }}}});
  function ask(op, options) {{
    if (!options || !options.publicKey) return Promise.reject(fail('NotSupportedError', 'Only passkeys are supported.'));
    var signal = options.signal;
    if (signal && signal.aborted) return Promise.reject(signal.reason || fail('AbortError', 'The request was aborted.'));
    var pk = options.publicKey, msg;
    // No passkey autofill: such a request just waits (until aborted).
    if (op === 'get' && options.mediation === 'conditional') {{
      return new Promise(function(_, reject) {{
        if (signal) signal.addEventListener('abort', function() {{ reject(signal.reason || fail('AbortError', 'Aborted.')); }});
      }});
    }}
    try {{
      if (op === 'create') {{
        var algs = (pk.pubKeyCredParams || []).map(function(p) {{ return p.alg; }});
        if (algs.length && algs.indexOf(-7) < 0)
          return Promise.reject(fail('NotSupportedError', 'Passwords makes only ES256 passkeys.'));
        msg = {{op: op, rp: {{id: pk.rp.id || null, name: String(pk.rp.name || '')}},
          user: {{id: b64(pk.user.id), name: String(pk.user.name || ''), displayName: String(pk.user.displayName || '')}},
          challenge: b64(pk.challenge), exclude: (pk.excludeCredentials || []).map(function(c) {{ return b64(c.id); }})}};
      }} else {{
        msg = {{op: op, rp_id: pk.rpId || null, challenge: b64(pk.challenge),
          allow: (pk.allowCredentials || []).map(function(c) {{ return b64(c.id); }})}};
      }}
    }} catch (e) {{
      return Promise.reject(new TypeError('The passkey options are malformed.'));
    }}
    var id = msg.id = next++;
    return new Promise(function(resolve, reject) {{
      pending[id] = {{op: op, resolve: resolve, reject: reject}};
      if (signal) signal.addEventListener('abort', function() {{
        if (!pending[id]) return;
        delete pending[id];
        log(K + JSON.stringify({{op: 'cancel', id: id}}));
        reject(signal.reason || fail('AbortError', 'The request was aborted.'));
      }});
      log(K + JSON.stringify(msg));
    }});
  }}
  var container = {{
    create: function(o) {{ return ask('create', o); }},
    get: function(o) {{ return ask('get', o); }},
    store: function() {{ return Promise.reject(fail('NotSupportedError', 'Storing credentials isn’t supported.')); }},
    preventSilentAccess: function() {{ return Promise.resolve(); }}
  }};
  Object.defineProperty(Navigator.prototype, 'credentials', {{get: function() {{ return container; }}, configurable: true}});
}})();"#,
        marker = q(marker(token)),
        answer = q(answer_name(token)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests() {
        let r: PageRequest = serde_json::from_str(
            r#"{"op":"create","id":1,"rp":{"id":null,"name":"Ex"},"user":{"id":"dQ","name":"jane","displayName":"Jane"},"challenge":"Y2g","exclude":[]}"#,
        )
        .unwrap();
        assert!(matches!(r, PageRequest::Create { id: 1, ref rp, .. } if rp.id.is_none()));
        let r: PageRequest = serde_json::from_str(r#"{"op":"cancel","id":3}"#).unwrap();
        assert!(matches!(r, PageRequest::Cancel { id: 3 }));
    }

    #[test]
    fn relying_parties() {
        assert_eq!(rp_id(None, "https://login.example.com/x").unwrap(), "login.example.com");
        assert_eq!(rp_id(Some("example.com"), "https://login.example.com/").unwrap(), "example.com");
        assert!(rp_id(Some("com"), "https://example.com/").is_err());
        assert!(rp_id(Some("co.uk"), "https://example.co.uk/").is_err());
        assert!(rp_id(Some("other.com"), "https://example.com/").is_err());
        assert!(rp_id(None, "http://example.com/").is_err());
        assert_eq!(rp_id(None, "http://localhost:8765/").unwrap(), "localhost");
    }

    #[test]
    fn client_data_is_json() {
        let c = client_data("webauthn.get", "Y2g", "https://example.com:8443/login");
        let v: serde_json::Value = serde_json::from_str(&c).unwrap();
        assert_eq!(v["origin"], "https://example.com:8443");
        assert!(c.starts_with(r#"{"type":"webauthn.get","challenge":"Y2g""#));
        assert_eq!(hash("abc"), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
        let s = answer_script("t", 4, Err(("NotAllowedError", "No.")));
        assert_eq!(s, r#"window["__tabulaPasskeyt"](4, ["NotAllowedError","No."], null)"#);
        assert!(script("t").contains("tabula-passkey-t:"));
    }
}
