//! The keyring's prompts, asked by us. gnome-keyring-daemon asks for the
//! login keyring's password (to unlock it, or to make a new one) through
//! a "system prompter" on the session bus, `org.gnome.keyring.SystemPrompter`,
//! which D-Bus would otherwise start as GTK's gcr-prompter. An Olo
//! session owns that name itself ([`Prompter::serve`]), so the question
//! comes up as an Olo window and nothing else is started.
//!
//! The protocol is gcr's (gcr-system-prompter.c, gcr-secret-exchange.c):
//!
//! 1. The daemon calls `BeginPrompting(callback)`. One prompt is shown at
//!    a time; the rest wait their turn. When it's this one's turn we call
//!    the daemon back, `PromptReady("", {}, exchange)`, the exchange
//!    carrying our Diffie-Hellman public key.
//! 2. The daemon calls `PerformPrompt(callback, "password" | "confirm",
//!    properties, exchange)` with what to show and its public key: we
//!    show it ([`Event::Show`]).
//! 3. The answer goes back in `PromptReady("yes" | "no", properties,
//!    exchange)`, a password encrypted in the exchange (DH over the 1536-bit
//!    IKE group, HKDF-SHA256 to an AES-128 key, CBC with PKCS#7 padding),
//!    so it never crosses the bus in the clear. A wrong password brings
//!    another `PerformPrompt` with a `warning`.
//! 4. `StopPrompting(callback)` ends it: we close the window and call
//!    `PromptDone()`. If the daemon goes away the window closes too.

use olo_sync::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use num_bigint::BigUint;
use zbus::{Connection, names::BusName, zvariant::OwnedValue};

use crate::Result;

pub const SYSTEM_NAME: &str = "org.gnome.keyring.SystemPrompter";
pub const PRIVATE_NAME: &str = "org.gnome.keyring.PrivatePrompter";
const PATH: &str = "/org/gnome/keyring/Prompter";
const IFACE: &str = "org.gnome.keyring.internal.Prompter";
const CALLBACK: &str = "org.gnome.keyring.internal.Prompter.Callback";
const PROTOCOL: &str = "sx-aes-1";

/// What a prompt shows, as the daemon describes it (gcr's GcrPrompt
/// properties).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub title: String,
    /// The question, in bold ("Authentication required").
    pub message: String,
    /// The detail ("An application wants access to the keyring “login”,
    /// but it is locked").
    pub description: String,
    /// Shown as a problem: "The unlock password was incorrect".
    pub warning: String,
    /// Choosing a new password: ask for it twice.
    pub password_new: bool,
    /// A check box's label ("Automatically unlock this keyring whenever
    /// I'm logged in"), if one is shown, and whether it's ticked.
    pub choice_label: String,
    pub choice_chosen: bool,
    pub continue_label: String,
    pub cancel_label: String,
}

impl Props {
    fn update(&mut self, changes: &HashMap<String, OwnedValue>) {
        for (k, v) in changes {
            let s = || String::try_from(v.try_clone().ok()?).ok();
            let b = || bool::try_from(v).ok();
            match k.as_str() {
                "title" => self.title = s().unwrap_or_default(),
                "message" => self.message = s().unwrap_or_default(),
                "description" => self.description = s().unwrap_or_default(),
                "warning" => self.warning = s().unwrap_or_default(),
                "password-new" => self.password_new = b().unwrap_or(false),
                "choice-label" => self.choice_label = s().unwrap_or_default(),
                "choice-chosen" => self.choice_chosen = b().unwrap_or(false),
                "continue-label" => self.continue_label = s().unwrap_or_default(),
                "cancel-label" => self.cancel_label = s().unwrap_or_default(),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Ask for a password.
    Password,
    /// Ask to go ahead or not.
    Confirm,
}

/// What the prompter asks of the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Show prompt `id` (again, if it's already up: a wrong password comes
    /// back with a `warning`), and wait for [`Prompter::answer`].
    Show { id: u64, kind: Kind, props: Props },
    /// Close prompt `id`: it's over, answered or not.
    Close(u64),
}

/// The user's answer to a prompt.
pub enum Answer {
    /// The password (for [`Kind::Password`]).
    Password(String),
    /// Go ahead (for [`Kind::Confirm`]).
    Continue,
    Cancel,
}

type Handler = Arc<dyn Fn(Event) + Send + Sync>;

/// One caller's prompting: the daemon's callback object.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Callback {
    sender: String,
    path: String,
}

struct Active {
    id: u64,
    callback: Callback,
    exchange: Exchange,
    props: Props,
    /// Shown and waiting for the user (else waiting for the daemon).
    asking: Option<Kind>,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Callback>,
    active: Option<Active>,
    next_id: u64,
}

struct Shared {
    state: Mutex<State>,
    on: Handler,
    conn: Mutex<Option<Connection>>,
}

impl Shared {
    fn conn(&self) -> Option<Connection> {
        self.conn.lock().unwrap().clone()
    }

    /// Start the next waiting prompt, if none is active: send our half of
    /// the key exchange.
    fn next(self: &Arc<Self>) {
        let (callback, sent) = {
            let mut st = self.state.lock().unwrap();
            if st.active.is_some() {
                return;
            }
            let Some(callback) = st.waiting.pop_front() else { return };
            st.next_id += 1;
            let exchange = Exchange::new();
            let sent = exchange.begin();
            st.active = Some(Active { id: st.next_id, callback: callback.clone(), exchange, props: Props::default(), asking: None });
            (callback, sent)
        };
        self.ready(callback, "", HashMap::new(), sent);
    }

    /// Call the daemon's `PromptReady`, off the caller's thread. If the
    /// daemon can't take it, that prompting is over.
    fn ready(self: &Arc<Self>, callback: Callback, reply: &'static str, props: HashMap<&'static str, zvariant::Value<'static>>, exchange: String) {
        let Some(conn) = self.conn() else { return };
        let me = self.clone();
        conn.executor()
            .spawn(
                async move {
                    let Ok(conn) = me.conn().ok_or(()) else { return };
                    let r = conn
                        .call_method(Some(callback.sender.as_str()), callback.path.as_str(), Some(CALLBACK), "PromptReady", &(reply, props, exchange))
                        .await;
                    if let Err(e) = r {
                        tracing::debug!("keyring prompt: the caller didn't take the answer: {e}");
                        me.stop(&callback, false);
                    }
                },
                "keyring-prompt-ready",
            )
            .detach();
    }

    /// End `callback`'s prompting (closing its window), telling the daemon
    /// if `done`; then the next one may start.
    fn stop(self: &Arc<Self>, callback: &Callback, done: bool) {
        let closed = {
            let mut st = self.state.lock().unwrap();
            st.waiting.retain(|c| c != callback);
            match &st.active {
                Some(a) if a.callback == *callback => st.active.take().map(|a| a.id),
                _ => None,
            }
        };
        if let Some(id) = closed {
            (self.on)(Event::Close(id));
        }
        if done && let Some(conn) = self.conn() {
            let callback = callback.clone();
            let c = conn.clone();
            conn.executor()
                .spawn(
                    async move {
                        let _ = c.call_method(Some(callback.sender.as_str()), callback.path.as_str(), Some(CALLBACK), "PromptDone", &()).await;
                    },
                    "keyring-prompt-done",
                )
                .detach();
        }
        self.next();
    }

    /// Everything from a caller that has left the bus.
    fn vanished(self: &Arc<Self>, name: &str) {
        let gone: Vec<Callback> = {
            let st = self.state.lock().unwrap();
            st.waiting.iter().chain(st.active.as_ref().map(|a| &a.callback)).filter(|c| c.sender == name).cloned().collect()
        };
        for c in gone {
            self.stop(&c, false);
        }
    }
}

const FAILED: &str = "org.freedesktop.DBus.Error.Failed";
const INVALID: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// What a call leads to once its reply has gone: the daemon must have its
/// reply before it hears from us again (gcr-system-prompt.c gets confused
/// otherwise), so nothing happens from inside the call.
enum Then {
    Nothing,
    Next,
    Show(Event),
    Stop(Callback),
}

type Reply = std::result::Result<(), (&'static str, String)>;

impl Shared {
    /// One of the daemon's `org.gnome.keyring.internal.Prompter` calls.
    fn call(&self, member: &str, sender: &str, body: &zbus::message::Body) -> (Reply, Then) {
        let invalid = || (Err((INVALID, "Invalid arguments".to_string())), Then::Nothing);
        let failed = |m: &str| (Err((FAILED, m.to_string())), Then::Nothing);
        let callback = |path: &zvariant::ObjectPath<'_>| Callback { sender: sender.to_string(), path: path.to_string() };
        match member {
            "BeginPrompting" => {
                let Ok((path,)) = body.deserialize::<(zvariant::ObjectPath<'_>,)>() else { return invalid() };
                let c = callback(&path);
                let mut st = self.state.lock().unwrap();
                if st.waiting.contains(&c) || st.active.as_ref().is_some_and(|a| a.callback == c) {
                    return failed("Already begun prompting for this prompt callback");
                }
                st.waiting.push_back(c);
                (Ok(()), Then::Next)
            }
            "PerformPrompt" => {
                let Ok((path, kind, properties, exchange)) = body.deserialize::<(zvariant::ObjectPath<'_>, String, HashMap<String, OwnedValue>, String)>() else {
                    return invalid();
                };
                let c = callback(&path);
                let kind = match kind.as_str() {
                    "password" => Kind::Password,
                    "confirm" => Kind::Confirm,
                    _ => return (Err((INVALID, "Invalid type argument".into())), Then::Nothing),
                };
                let mut st = self.state.lock().unwrap();
                let Some(a) = st.active.as_mut().filter(|a| a.callback == c) else {
                    return failed("Not begun prompting for this prompt callback");
                };
                if a.asking.is_some() {
                    return failed("Already performing a prompt for this prompt callback");
                }
                if !a.exchange.receive(&exchange) {
                    return (Err((INVALID, "Invalid secret exchange received".into())), Then::Nothing);
                }
                a.props.update(&properties);
                a.asking = Some(kind);
                (Ok(()), Then::Show(Event::Show { id: a.id, kind, props: a.props.clone() }))
            }
            "StopPrompting" => {
                let Ok((path,)) = body.deserialize::<(zvariant::ObjectPath<'_>,)>() else { return invalid() };
                (Ok(()), Then::Stop(callback(&path)))
            }
            _ => (Err(("org.freedesktop.DBus.Error.UnknownMethod", format!("No method {member}"))), Then::Nothing),
        }
    }

    /// Answer the calls made to us, in order.
    async fn serve(self: Arc<Self>, conn: Connection, mut calls: zbus::MessageStream) {
        use futures_lite::StreamExt;
        while let Some(Ok(msg)) = calls.next().await {
            let header = msg.header();
            if header.message_type() != zbus::message::Type::MethodCall || header.path().is_none_or(|p| p.as_str() != PATH) {
                continue;
            }
            let member = header.member().map(|m| m.to_string()).unwrap_or_default();
            let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
            let (reply, then) = if header.interface().is_some_and(|i| i.as_str() == IFACE) {
                self.call(&member, &sender, &msg.body())
            } else {
                (Err(("org.freedesktop.DBus.Error.UnknownInterface", "No such interface".into())), Then::Nothing)
            };
            let sent = match reply {
                Ok(()) => conn.reply(&header, &()).await,
                Err((name, text)) => conn.reply_error(&header, name, &text).await,
            };
            if let Err(e) = sent {
                tracing::debug!("keyring prompt: couldn't reply to {member}: {e}");
            }
            match then {
                Then::Nothing => {}
                Then::Next => self.next(),
                Then::Show(e) => (self.on)(e),
                Then::Stop(c) => self.stop(&c, true),
            }
        }
    }
}

/// The session's keyring prompter. Dropping it gives the names up.
pub struct Prompter {
    shared: Arc<Shared>,
    _conn: zbus::blocking::Connection,
}

impl Prompter {
    /// Serve the keyring's prompts on the session bus. `on` is called from
    /// zbus's thread; hand the event to the UI.
    pub fn serve(on: impl Fn(Event) + Send + Sync + 'static) -> Result<Prompter> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), on: Arc::new(on), conn: Mutex::new(None) });
        let conn = zbus::blocking::connection::Builder::session()?.build()?;
        let inner = conn.inner().clone();
        *shared.conn.lock().unwrap() = Some(inner.clone());
        // Listening before the names are ours, so no call is missed.
        let calls = zbus::MessageStream::from(&inner);
        inner.executor().spawn(shared.clone().serve(inner.clone(), calls), "keyring-prompt-calls").detach();
        // Callers that go away take their prompts with them.
        let watch = shared.clone();
        let c = inner.clone();
        inner
            .executor()
            .spawn(
                async move {
                    use futures_lite::StreamExt;
                    let Ok(dbus) = zbus::fdo::DBusProxy::new(&c).await else { return };
                    let Ok(mut changes) = dbus.receive_name_owner_changed().await else { return };
                    while let Some(change) = changes.next().await {
                        let Ok(args) = change.args() else { continue };
                        if args.new_owner().is_none() && matches!(args.name(), BusName::Unique(_)) {
                            watch.vanished(args.name().as_str());
                        }
                    }
                },
                "keyring-prompt-watch",
            )
            .detach();
        // Ours from now on, even if gcr-prompter had it (it's started
        // only on demand, and then for one prompt).
        // (Through the bus directly: zbus's own request_name expects an
        // object server, which this doesn't use, so that calls are answered
        // before anything follows from them.)
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        for name in [SYSTEM_NAME, PRIVATE_NAME] {
            let name = zbus::names::WellKnownName::try_from(name)?;
            let reply = dbus.request_name(name.clone(), zbus::fdo::RequestNameFlags::ReplaceExisting.into())?;
            tracing::debug!("keyring prompt: {name}: {reply:?}");
        }
        Ok(Prompter { shared, _conn: conn })
    }

    /// Answer prompt `id` (`choice` is the check box, if one was shown).
    pub fn answer(&self, id: u64, answer: Answer, choice: bool) {
        let sent = {
            let mut st = self.shared.state.lock().unwrap();
            let Some(a) = st.active.as_mut().filter(|a| a.id == id && a.asking.is_some()) else { return };
            a.asking = None;
            a.props.choice_chosen = choice;
            let mut props: HashMap<&'static str, zvariant::Value<'static>> = HashMap::new();
            // gcr wraps each value in a variant of its own (v holding v).
            let wrap = |v: zvariant::Value<'static>| zvariant::Value::Value(Box::new(v));
            if !a.props.choice_label.is_empty() {
                props.insert("choice-chosen", wrap(choice.into()));
            }
            let (reply, exchange) = match answer {
                Answer::Password(mut p) => {
                    if a.props.password_new {
                        props.insert("password-strength", wrap(strength(&p).into()));
                    }
                    let sent = a.exchange.send(Some(p.as_bytes()));
                    // SAFETY: zero bytes keep the string valid UTF-8.
                    unsafe { p.as_bytes_mut().fill(0) };
                    ("yes", sent)
                }
                Answer::Continue => ("yes", a.exchange.send(None)),
                Answer::Cancel => ("no", a.exchange.send(None)),
            };
            (a.callback.clone(), reply, props, exchange)
        };
        let (callback, reply, props, exchange) = sent;
        self.shared.ready(callback, reply, props, exchange);
    }
}

/// gcr's password strength (gcr-prompt-dialog.c): 0 empty, else 1 plus
/// one for each kind of character beyond the first (up to 4) once it's
/// eight or more long.
fn strength(p: &str) -> i32 {
    if p.is_empty() {
        return 0;
    }
    let kinds = [p.chars().any(|c| c.is_lowercase()), p.chars().any(|c| c.is_uppercase()), p.chars().any(|c| c.is_ascii_digit()), p.chars().any(|c| !c.is_alphanumeric())];
    let n = kinds.iter().filter(|k| **k).count() as i32;
    if p.chars().count() >= 8 { n.max(1) } else { 1 }
}

/// RFC 3526's 1536-bit MODP group (generator 2), egg-dh.c's
/// "ietf-ike-grp-modp-1536".
const PRIME_1536: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF";

fn prime() -> BigUint {
    BigUint::parse_bytes(PRIME_1536.as_bytes(), 16).expect("prime")
}

/// One side of gcr's secret exchange, "sx-aes-1".
struct Exchange {
    private: BigUint,
    public: Vec<u8>,
    key: Option<[u8; 16]>,
}

impl Exchange {
    fn new() -> Exchange {
        let p = prime();
        let bits = p.bits();
        // A random exponent below 2^(bits - 1), not zero (egg_dh_gen_pair).
        let mut private = BigUint::ZERO;
        while private == BigUint::ZERO {
            let mut bytes = vec![0u8; bits.div_ceil(8) as usize];
            getrandom::fill(&mut bytes).expect("random");
            private = BigUint::from_bytes_be(&bytes);
            for b in (bits - 1)..(bytes.len() as u64 * 8) {
                private.set_bit(b, false);
            }
        }
        let public = BigUint::from(2u32).modpow(&private, &p).to_bytes_be();
        Exchange { private, public, key: None }
    }

    /// The first message: our public key.
    fn begin(&self) -> String {
        format_exchange(&[("public", &self.public)])
    }

    /// Take the other side's message: derive the shared key from its
    /// public key (once).
    fn receive(&mut self, text: &str) -> bool {
        let fields = parse_exchange(text);
        if self.key.is_some() {
            return true;
        }
        let Some(peer) = fields.get("public") else { return false };
        let p = prime();
        let peer = BigUint::from_bytes_be(peer);
        if peer <= BigUint::from(1u32) || peer >= p {
            return false;
        }
        let shared = peer.modpow(&self.private, &p).to_bytes_be();
        let mut key = [0u8; 16];
        if hkdf::Hkdf::<sha2::Sha256>::new(None, &shared).expand(&[], &mut key).is_err() {
            return false;
        }
        self.key = Some(key);
        true
    }

    /// Our reply, with `secret` encrypted under the shared key.
    fn send(&self, secret: Option<&[u8]>) -> String {
        let (Some(secret), Some(key)) = (secret, self.key) else { return format_exchange(&[("public", &self.public)]) };
        let mut iv = [0u8; 16];
        getrandom::fill(&mut iv).expect("random");
        let sealed = encrypt(&key, &iv, secret);
        format_exchange(&[("public", &self.public), ("secret", &sealed), ("iv", &iv)])
    }
}

fn encrypt(key: &[u8; 16], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    let mut buf = vec![0u8; (plain.len() / 16 + 1) * 16];
    buf[..plain.len()].copy_from_slice(plain);
    let n = cbc::Encryptor::<aes::Aes128>::new(key.into(), iv.into()).encrypt_padded::<Pkcs7>(&mut buf, plain.len()).expect("room for the padding").len();
    buf.truncate(n);
    buf
}

#[cfg(test)]
fn decrypt(key: &[u8; 16], iv: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
    let mut buf = sealed.to_vec();
    let iv: &[u8; 16] = iv.try_into().ok()?;
    let n = cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into()).decrypt_padded::<Pkcs7>(&mut buf).ok()?.len();
    buf.truncate(n);
    Some(buf)
}

/// GKeyFile text, as gcr writes it: `[sx-aes-1]` then `name=base64` lines.
fn format_exchange(fields: &[(&str, &[u8])]) -> String {
    use base64::Engine as _;
    let mut out = format!("[{PROTOCOL}]\n");
    for (k, v) in fields {
        out.push_str(&format!("{k}={}\n", base64::engine::general_purpose::STANDARD.encode(v)));
    }
    out
}

fn parse_exchange(text: &str) -> HashMap<String, Vec<u8>> {
    use base64::Engine as _;
    let mut out = HashMap::new();
    let mut ours = false;
    for line in text.lines().map(str::trim) {
        if let Some(group) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            ours = group == PROTOCOL;
        } else if ours
            && let Some((k, v)) = line.split_once('=')
            && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(v.trim())
        {
            out.insert(k.trim().to_string(), bytes);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both ends of the exchange, as the daemon and we would run it.
    #[test]
    fn exchanges_a_secret() {
        let mut ours = Exchange::new();
        let mut theirs = Exchange::new();
        assert!(theirs.receive(&ours.begin()));
        assert!(ours.receive(&theirs.begin()));
        assert_eq!(ours.key, theirs.key);
        let sent = parse_exchange(&ours.send(Some("hunter2 ünïcode".as_bytes())));
        assert_eq!(sent["public"], ours.public);
        let back = decrypt(&theirs.key.unwrap(), &sent["iv"], &sent["secret"]).unwrap();
        assert_eq!(back, "hunter2 ünïcode".as_bytes());
        // Block-sized secrets get a whole block of padding.
        let sent = parse_exchange(&ours.send(Some(&[7u8; 16])));
        assert_eq!(sent["secret"].len(), 32);
        assert!(!parse_exchange(&ours.send(None)).contains_key("secret"));
        // Nonsense keys are refused.
        let mut fresh = Exchange::new();
        assert!(!fresh.receive("[sx-aes-1]\npublic=AQ==\n"));
        assert!(!fresh.receive("[other]\npublic=AAAA\n"));
    }

    /// With gcr itself: GCR_PEER is gcr's `begin()`; prints our reply
    /// carrying "olo-secret" for gcr to read back.
    #[test]
    #[ignore]
    fn with_gcr() {
        let mut ours = Exchange::new();
        assert!(ours.receive(&std::env::var("GCR_PEER").unwrap().replace("\\n", "\n")));
        println!("REPLY:{}", ours.send(Some(b"olo-secret")).replace('\n', "\\n"));
    }

    /// HKDF with no salt and no info (egg_hkdf_perform), against RFC 5869's
    /// test case 3.
    #[test]
    fn hkdf_matches_rfc_5869() {
        let ikm = [0x0bu8; 22];
        let mut okm = [0u8; 42];
        hkdf::Hkdf::<sha2::Sha256>::new(None, &ikm).expand(&[], &mut okm).unwrap();
        let hex: String = okm.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8");
    }

    /// Against a real gnome-keyring-daemon on a private bus (never the
    /// session's): `KEYRING_TEST_BUS=unix:path=… cargo test -p
    /// olo-system real_daemon -- --ignored --nocapture`. Makes a
    /// keyring (a new password), locks it, unlocks it with a wrong then the
    /// right password, and cancels an unlock.
    #[test]
    #[ignore]
    fn real_daemon() {
        use std::{sync::mpsc, time::Duration};
        use zvariant::{OwnedObjectPath, Value};
        let bus = std::env::var("KEYRING_TEST_BUS").expect("KEYRING_TEST_BUS");
        // SAFETY: set before any thread reads it.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus) };
        let (tx, rx) = mpsc::channel();
        let prompter = Prompter::serve(move |e| tx.send(e).unwrap()).unwrap();
        let conn = zbus::blocking::Connection::session().unwrap();
        const SS: &str = "org.freedesktop.secrets";
        fn call<B: serde::Serialize + zvariant::DynamicType>(conn: &zbus::blocking::Connection, method: &str, body: &B) -> zbus::Message {
            conn.call_method(Some("org.freedesktop.secrets"), "/org/freedesktop/secrets", Some("org.freedesktop.Secret.Service"), method, body).unwrap()
        }
        // Run a prompt, answering each Show from `answers`; (dismissed, shows seen).
        let run = |prompt: OwnedObjectPath, answers: &mut Vec<Answer>| -> (bool, Vec<Props>, OwnedValue) {
            let proxy = zbus::blocking::Proxy::new(&conn, SS, prompt.as_str(), "org.freedesktop.Secret.Prompt").unwrap();
            let mut done = proxy.receive_signal("Completed").unwrap();
            proxy.call_method("Prompt", &("",)).unwrap();
            let mut seen = Vec::new();
            let (wtx, wrx) = mpsc::channel();
            std::thread::spawn(move || {
                let m = done.next().unwrap();
                let (dismissed, result): (bool, OwnedValue) = m.body().deserialize().unwrap();
                wtx.send((dismissed, result)).unwrap();
            });
            loop {
                if let Ok((d, result)) = wrx.try_recv() {
                    // The window closes too.
                    while let Ok(e) = rx.recv_timeout(Duration::from_millis(300)) {
                        assert!(matches!(e, Event::Close(_)), "{e:?}");
                    }
                    return (d, seen, result);
                }
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Event::Show { id, kind, props }) => {
                        println!("{kind:?}: {props:?}");
                        let answer = match kind {
                            Kind::Confirm => Answer::Continue,
                            Kind::Password => {
                                seen.push(props);
                                answers.remove(0)
                            }
                        };
                        prompter.answer(id, answer, false);
                    }
                    Ok(Event::Close(_)) | Err(_) => {}
                }
            }
        };
        let label: HashMap<&str, Value> = [("org.freedesktop.Secret.Collection.Label", Value::from("Olo test"))].into_iter().collect();
        let (collection, prompt): (OwnedObjectPath, OwnedObjectPath) = call(&conn, "CreateCollection", &(label, "")).body().deserialize().unwrap();
        let collection = if prompt.as_str() != "/" {
            let (dismissed, seen, result) = run(prompt, &mut vec![Answer::Password("olo-test-pass".into())]);
            assert!(!dismissed);
            assert!(seen[0].password_new, "{seen:?}");
            println!("new keyring: {:?}", seen[0]);
            // The new collection's path is the prompt's result.
            OwnedObjectPath::try_from(result).expect("the new keyring")
        } else {
            collection
        };
        let locked = || -> bool {
            let v: OwnedValue = conn.call_method(Some(SS), collection.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.freedesktop.Secret.Collection", "Locked")).unwrap().body().deserialize().unwrap();
            bool::try_from(v).unwrap()
        };
        assert!(!locked());
        let _: (Vec<OwnedObjectPath>, OwnedObjectPath) = call(&conn, "Lock", &(vec![collection.clone()],)).body().deserialize().unwrap();
        assert!(locked());
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = call(&conn, "Unlock", &(vec![collection.clone()],)).body().deserialize().unwrap();
        let (dismissed, seen, _) = run(prompt, &mut vec![Answer::Password("wrong".into()), Answer::Password("olo-test-pass".into())]);
        println!("unlock: {seen:#?}");
        assert!(!dismissed);
        assert_eq!(seen.len(), 2);
        assert!(seen[0].warning.is_empty() && !seen[1].warning.is_empty(), "{seen:?}");
        assert!(!locked());
        let _: (Vec<OwnedObjectPath>, OwnedObjectPath) = call(&conn, "Lock", &(vec![collection.clone()],)).body().deserialize().unwrap();
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = call(&conn, "Unlock", &(vec![collection.clone()],)).body().deserialize().unwrap();
        let (dismissed, _, _) = run(prompt, &mut vec![Answer::Cancel]);
        assert!(dismissed);
        assert!(locked());
    }

    #[test]
    fn reads_properties() {
        let mut p = Props::default();
        let mut m = HashMap::new();
        m.insert("message".to_string(), OwnedValue::try_from(zvariant::Value::from("Unlock")).unwrap());
        m.insert("password-new".to_string(), OwnedValue::from(true));
        m.insert("choice-label".to_string(), OwnedValue::try_from(zvariant::Value::from("Auto")).unwrap());
        p.update(&m);
        assert_eq!((p.message.as_str(), p.password_new, p.choice_label.as_str()), ("Unlock", true, "Auto"));
        assert_eq!(strength(""), 0);
        assert_eq!(strength("abc"), 1);
        assert_eq!(strength("abcDEF12!"), 4);
    }
}
