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
    /// The logins for the page's context menu in a field (its number).
    Field(u64),
    /// Learn a page's logins as it loads, so a field's menu has them at once.
    Prefetch,
    /// One value of a login for the field the context menu was for.
    FillField(Value),
    /// A login was used: offer to save it unless Passwords knows it.
    Check(Box<Offer>),
    Save,
    /// Unlock, then show the menu.
    Unlock,
    /// The passkeys for the page's passkey request (its number).
    Passkeys(u64),
    PasskeyCreate(u64),
    /// Signing for the request, over this client data.
    PasskeySign(u64, String),
}

/// Which of a login's values goes into a field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Value {
    Username,
    Password,
}

/// The context menu for a field waits this long for the page's logins
/// before opening without them.
pub const FIELD_MENU_WAIT: Duration = Duration::from_millis(400);

/// How long the logins learnt for a site are used without asking again
/// (a field's menu asks anyway, for next time).
pub const KNOWN_FOR: Duration = Duration::from_secs(600);

/// The field's context menu waited long enough (its number).
pub struct FieldMenuTimeout {
    pub win: WindowId,
    pub id: u64,
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
            if !matches!(request, Request::Logins { .. } | Request::Check { .. }) {
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
/// `origin`, into the fields `learnt` for it if they are there;
/// evaluates to "filled", "none" (no field) or "moved".
pub fn fill_script(origin: &str, username: &str, password: &str, learnt: &Fields) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let qs = |s: &Option<String>| q(s.as_deref().unwrap_or(""));
    format!(
        r#"(function(origin, u, p, us, ps) {{
  if (location.origin !== origin) return 'moved';
  // Shown and editable: not the hidden decoys some sign-ins keep (tiny,
  // off the page, transparent or hidden from assistive technology).
  function usable(e) {{
    var r = e.getBoundingClientRect(), st = getComputedStyle(e);
    return r.width >= 4 && r.height >= 4 && r.right > 0 && r.bottom > 0
      && st.visibility !== 'hidden' && st.opacity !== '0'
      && e.getAttribute('aria-hidden') !== 'true' && !e.disabled && !e.readOnly;
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
    // A sign-in that asks for the account first (Google, Microsoft,
    // Apple): the field with the focus if it takes text, else one that
    // says it is for the account, else an email field, else the only one.
    var texts = all(document, TEXT);
    var named = function(e) {{
      var a = (e.getAttribute('autocomplete') || '') + ' ' + (e.name || '') + ' ' + (e.id || '') + ' ' + (e.getAttribute('aria-label') || '');
      return /user|mail|login|account|ident|phone/i.test(a);
    }};
    user = (a && texts.indexOf(a) >= 0 ? a : null)
      || texts.filter(function(e) {{ return /username/i.test(e.getAttribute('autocomplete') || ''); }})[0]
      || texts.filter(named)[0]
      || texts.filter(function(e) {{ return e.type === 'email'; }})[0]
      || (texts.length === 1 ? texts[0] : null);
  }}
  // Fields learnt from filling them by hand come first.
  function learnt(sel) {{
    try {{ var e = sel && document.querySelector(sel); return e && usable(e) ? e : null; }} catch (x) {{ return null; }}
  }}
  user = learnt(us) || user;
  pass = learnt(ps) || pass;
  if (user && u) set(user, u);
  if (pass && p) set(pass, p);
  if (pass) pass.focus(); else if (user) user.focus();
  return user || pass ? 'filled' : 'none';
}})({}, {}, {}, {}, {})"#,
        q(origin),
        q(username),
        q(password),
        qs(&learnt.user),
        qs(&learnt.pass)
    )
}

/// Which fields of a page take the username and the password, learnt
/// when they are filled from a field's context menu.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<String>,
}

/// The fields learnt for each page (origin and path), in
/// `netpositive-fields.json`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Learnt {
    pages: std::collections::BTreeMap<String, Fields>,
}

/// The page a URL's fields are learnt for: its origin and path, without
/// the query (Google's sign-in carries a different one each time).
pub fn page_key(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(format!("{}{}", u.origin().ascii_serialization(), u.path()))
}

impl Learnt {
    fn path() -> std::path::PathBuf {
        tabula_kit::config_dir().join("netpositive-fields.json")
    }

    pub fn load() -> Learnt {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn fields(&self, url: &str) -> Fields {
        page_key(url).and_then(|k| self.pages.get(&k).cloned()).unwrap_or_default()
    }

    /// The field `selector` on the page at `url` takes `value`; true if
    /// that is news (then save).
    pub fn learn(&mut self, url: &str, value: Value, selector: &str) -> bool {
        let Some(key) = page_key(url) else { return false };
        let fields = self.pages.entry(key).or_default();
        let slot = if value == Value::Username { &mut fields.user } else { &mut fields.pass };
        if slot.as_deref() == Some(selector) {
            return false;
        }
        *slot = Some(selector.to_owned());
        true
    }

    /// Written off the UI thread.
    pub fn save(&self) {
        let Ok(text) = serde_json::to_string_pretty(self) else { return };
        std::thread::spawn(move || {
            let path = Self::path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        });
    }
}

/// A field filled from its context menu, to learn (from the page script).
pub struct Filled {
    pub url: String,
    pub value: Value,
    pub selector: String,
}

/// Put `value` into the field with the keyboard focus (the one the
/// context menu was for), in the top frame if it's still at `origin`.
/// Evaluates to "filled", a newline and a selector for the field (to
/// learn it), or "none" or "moved".
pub fn field_script(origin: &str, value: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function(origin, v) {{
  if (location.origin !== origin) return 'moved';
  var e = document.activeElement;
  if (!e || e.disabled || e.readOnly) return 'none';
  var proto = e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : e instanceof HTMLInputElement ? HTMLInputElement.prototype : null;
  if (!proto) return 'none';
  try {{ Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, v); }}
  catch (x) {{ e.value = v; }}
  e.dispatchEvent(new Event('input', {{bubbles: true}}));
  e.dispatchEvent(new Event('change', {{bubbles: true}}));
  return 'filled\n' + {SELECTOR}(e);
}})({}, {})"#,
        q(origin),
        q(value),
        SELECTOR = SELECTOR
    )
}

/// A script function naming an element by a CSS selector that finds it
/// again on the page's next visit: its id, else its name, else its path.
const SELECTOR: &str = r#"(function(e) {
  var esc = (window.CSS && CSS.escape) || function(s) { return s.replace(/[^\w-]/g, '\\$&'); };
  var one = function(sel) { try { return document.querySelectorAll(sel).length === 1; } catch (x) { return false; } };
  if (e.id && one('#' + esc(e.id))) return '#' + esc(e.id);
  var tag = e.tagName.toLowerCase();
  if (e.name) {
    var s = tag + '[name="' + e.name.replace(/["\\]/g, '\\$&') + '"]';
    if (one(s)) return s;
  }
  var path = [];
  for (var n = e; n && n.nodeType === 1 && n !== document.body; n = n.parentElement) {
    var i = 1;
    for (var p = n.previousElementSibling; p; p = p.previousElementSibling) if (p.tagName === n.tagName) i++;
    path.unshift(n.tagName.toLowerCase() + ':nth-of-type(' + i + ')');
  }
  return 'body > ' + path.join(' > ');
})"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_quote_values() {
        let s = fill_script("https://a.example", "o'neil\"</script>", "p\\w\n", &Fields { user: Some("#id\"x".into()), pass: None });
        assert!(s.contains(r##""#id\"x""##), "learnt selectors quoted");
        assert!(s.contains(r#""o'neil\"</script>""#));
        assert!(s.contains(r#""p\\w\n""#));
        let f = field_script("https://a.example", "x\"</script>");
        assert!(f.contains(r#""x\"</script>""#) && f.contains("activeElement"));
        let c = capture_script("abc");
        assert!(c.contains("tabula-login-abc:"));
        assert_eq!(token().len(), 32);
        assert!(web("https://x") && web("http://x") && !web("about:blank") && !web("file:///"));
    }

    #[test]
    fn learns_fields_per_page() {
        let mut l = Learnt::default();
        let url = "https://accounts.google.com/v3/signin/identifier?continue=x&flowName=GlifWebSignIn";
        assert!(l.learn(url, Value::Username, "#identifierId"));
        assert!(!l.learn(url, Value::Username, "#identifierId"), "nothing new");
        // Another visit, another query: the same page.
        let again = "https://accounts.google.com/v3/signin/identifier?continue=y";
        assert_eq!(l.fields(again), Fields { user: Some("#identifierId".into()), pass: None });
        assert_eq!(l.fields("https://accounts.google.com/v3/signin/challenge/pwd"), Fields::default());
        assert!(l.learn(again, Value::Password, "input[name=\"Passwd\"]"));
        let saved = serde_json::to_string(&l).unwrap();
        let back: Learnt = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.fields(url).pass.as_deref(), Some("input[name=\"Passwd\"]"));
    }
}
