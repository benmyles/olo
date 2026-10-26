//! Logins from Passwords (tabula-keychain): the toolbar's key button
//! lists the page's saved logins and fills one in; a user script notices
//! a login being used and NetPositive offers to save it.
//!
//! The capture script reports through `console.log`, with a random token
//! per process kept in the script's closure, so a page (or an ad in it)
//! can't make up logins for the page around it without the token. Fills
//! go into the top frame only and check its origin first.

use std::time::{Duration, Instant};

use servo::WebViewId;
use tabula_keychain::{CallError, Request, Response};
use tabula_kit::{Proxy, WindowId};
use zeroize::Zeroize;

/// What a request to Passwords was for.
pub enum Purpose {
    /// The key button's menu.
    Menu,
    Fill,
    /// A login was used: offer to save it unless Passwords knows it.
    Check(Box<Offer>),
    Save,
    /// Unlock, then show the menu.
    Unlock,
}

/// Passwords answered (or couldn't be reached).
pub struct Answer {
    pub win: WindowId,
    pub view: WebViewId,
    pub url: String,
    pub purpose: Purpose,
    pub result: Result<Response, String>,
    /// Passwords was started or unlocked meanwhile (it has the keyboard).
    pub waited: bool,
}

/// Ask Passwords from a thread; with `start`, start it first if needed.
pub fn ask(proxy: Proxy, win: WindowId, view: WebViewId, url: String, purpose: Purpose, request: Request, start: bool) {
    std::thread::Builder::new()
        .name("keychain".into())
        .spawn(move || {
            let mut result = tabula_keychain::call(&request);
            let mut waited = false;
            if start && matches!(result, Err(CallError::NotRunning)) {
                waited = true;
                match tabula_xdg::find_program("tabula-passwords") {
                    Some(program) => {
                        if let Err(e) = tabula_xdg::open::spawn(&[program.into_os_string()]) {
                            tracing::warn!("couldn't start Passwords: {e}");
                        }
                        let until = Instant::now() + Duration::from_secs(10);
                        while matches!(result, Err(CallError::NotRunning)) && Instant::now() < until {
                            std::thread::sleep(Duration::from_millis(200));
                            result = tabula_keychain::call(&request);
                        }
                    }
                    None => tracing::warn!("tabula-passwords isn't installed"),
                }
                // Just started, so locked: wait for the unlock, then ask again.
                if matches!(request, Request::Logins { .. })
                    && matches!(result, Ok(Response::Locked))
                    && let Ok(Response::Unlocked) = tabula_keychain::call(&Request::Unlock)
                {
                    result = tabula_keychain::call(&request);
                }
            }
            if matches!(request, Request::Unlock | Request::Fill { .. } | Request::Save { .. }) {
                waited = true;
            }
            let result = result.map_err(|e| e.to_string());
            proxy.send(Answer { win, view, url, purpose, result, waited });
        })
        .expect("thread");
}

/// A login just used on a page, offered for saving.
pub struct Offer {
    pub url: String,
    pub username: String,
    pub password: String,
    pub kind: OfferKind,
    /// Saving now.
    pub busy: bool,
    pub error: Option<String>,
}

pub enum OfferKind {
    New,
    Update { vault: String, id: String, title: String },
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl Offer {
    pub fn host(&self) -> String {
        tabula_keychain::host(&self.url).unwrap_or_else(|| "this site".into())
    }

    pub fn message(&self) -> String {
        let who = if self.username.is_empty() { String::new() } else { format!(" for “{}”", self.username) };
        match &self.kind {
            OfferKind::New => format!("Save the password{who} on {} in Passwords?", self.host()),
            OfferKind::Update { title, .. } => format!("Update the password{who} in “{title}”?"),
        }
    }

    pub fn request(&self) -> Request {
        let title = self.host();
        let replace = match &self.kind {
            OfferKind::Update { vault, id, .. } => Some((vault.clone(), id.clone())),
            OfferKind::New => None,
        };
        Request::Save {
            url: self.url.clone(),
            title,
            username: self.username.clone(),
            password: self.password.clone(),
            replace,
        }
    }
}

/// Whether the page at `url` can have logins.
pub fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// A random token for this process.
pub fn token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The marker a captured login's console message starts with.
pub fn marker(token: &str) -> String {
    format!("\u{1}tabula-login-{token}:")
}

/// Watches the top frame's forms; when one with a filled-in password is
/// sent (submit, a button, or Enter), reports `{u, p}` once.
pub fn capture_script(token: &str) -> String {
    let marker = serde_json::to_string(&marker(token)).unwrap_or_default();
    format!(
        r#"(function() {{
  if (window.top !== window) return;
  var log = console.log.bind(console), K = {marker}, last = '';
  var TEXT = 'input:not([type]),input[type=text],input[type=email],input[type=tel]';
  function filled(scope, sel) {{
    return Array.prototype.filter.call(scope.querySelectorAll(sel), function(e) {{ return e.value; }});
  }}
  function grab(form) {{
    var scope = form || document;
    var pws = filled(scope, 'input[type=password]');
    if (!pws.length) return;
    // A change-password form: current, new, confirm.
    var pass = pws[0];
    if (pws.length == 2 && pws[0].value !== pws[1].value) pass = pws[1];
    if (pws.length >= 3) pass = pws[1];
    var texts = filled(scope, TEXT), user = '';
    for (var i = 0; i < texts.length; i++)
      if (texts[i].compareDocumentPosition(pws[0]) & 4) user = texts[i].value;
    var msg = JSON.stringify({{u: user, p: pass.value}});
    if (msg === last) return;
    last = msg;
    log(K + msg);
  }}
  document.addEventListener('submit', function(e) {{ grab(e.target); }}, true);
  document.addEventListener('click', function(e) {{
    var b = e.target && e.target.closest && e.target.closest('button,input[type=submit],input[type=image]');
    if (b && b.type !== 'button' && b.type !== 'reset') grab(b.form || b.closest('form'));
  }}, true);
  document.addEventListener('keydown', function(e) {{
    var t = e.target;
    if (e.key === 'Enter' && t && t.tagName === 'INPUT') grab(t.form);
  }}, true);
}})();"#
    )
}

/// Fills `username` and `password` into the page if it is still at
/// `origin`; evaluates to "filled", "none" (no field) or "moved".
pub fn fill_script(origin: &str, username: &str, password: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function(origin, u, p) {{
  if (location.origin !== origin) return 'moved';
  function usable(e) {{
    var r = e.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && !e.disabled && !e.readOnly;
  }}
  function all(scope, sel) {{ return Array.prototype.filter.call(scope.querySelectorAll(sel), usable); }}
  function set(e, v) {{
    try {{ Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(e, v); }}
    catch (x) {{ e.value = v; }}
    e.dispatchEvent(new Event('input', {{bubbles: true}}));
    e.dispatchEvent(new Event('change', {{bubbles: true}}));
  }}
  var TEXT = 'input:not([type]),input[type=text],input[type=email],input[type=tel]';
  var pws = all(document, 'input[type=password]');
  var a = document.activeElement;
  var pass = a && a.type === 'password' && usable(a) ? a : pws[0] || null;
  var user = null;
  if (pass) {{
    var texts = all(pass.form || document, TEXT);
    for (var i = 0; i < texts.length; i++)
      if (texts[i].compareDocumentPosition(pass) & 4) user = texts[i];
  }} else {{
    var texts = all(document, TEXT);
    user = texts.filter(function(e) {{
      return /user|mail|login|account|name/i.test((e.name || '') + (e.id || '') + (e.autocomplete || ''));
    }})[0] || null;
  }}
  if (user && u) set(user, u);
  if (pass && p) set(pass, p);
  if (pass) pass.focus(); else if (user) user.focus();
  return user || pass ? 'filled' : 'none';
}})({}, {}, {})"#,
        q(origin),
        q(username),
        q(password)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_quote_values() {
        let s = fill_script("https://a.example", "o'neil\"</script>", "p\\w\n");
        assert!(s.contains(r#""o'neil\"</script>""#));
        assert!(s.contains(r#""p\\w\n""#));
        let c = capture_script("abc");
        assert!(c.contains("tabula-login-abc:"));
        assert_eq!(token().len(), 32);
        assert!(web("https://x") && web("http://x") && !web("about:blank") && !web("file:///"));
    }
}
