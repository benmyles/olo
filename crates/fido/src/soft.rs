//! A software authenticator: CTAPHID and the CTAP2 Olo uses, answered in
//! the process, for tests and for headless sessions. Debug builds take
//! `OLO_FIDO_SOFT=FILE` as a key plugged in (its master secret kept in
//! FILE, so a credential made in one process opens in another), touched
//! by itself `OLO_FIDO_SOFT_TOUCH_MS` (400) after it starts waiting, or
//! never ("never": till cancelled). Never in release builds.

use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant},
};

use ciborium::Value;
use hmac::{KeyInit, Mac};
use p256::SecretKey;
use sha2::{Digest, Sha256};

use crate::{
    ctap::{self, get, int, int_map, text_map},
    hid::{Device, REPORT, Transport},
    pin,
};

const KEEPALIVE: u8 = 0x3B;

/// A software key.
pub struct Soft {
    master: [u8; 32],
    cid: [u8; 4],
    /// A message coming in: its channel, command, length and data so far.
    incoming: Option<([u8; 4], u8, usize, Vec<u8>)>,
    out: VecDeque<[u8; REPORT]>,
    /// An answer waiting for a touch: when it's touched, and the answer.
    pending: Option<(Option<Instant>, Vec<u8>)>,
    touch_after: Option<Duration>,
    agreement: Option<SecretKey>,
    counter: u32,
    /// Touches it has taken (tests).
    pub touches: usize,
    /// Waits for a touch it gives up on first (tests): each answered "no
    /// touch came" (operation denied, as a YubiKey does when it stops
    /// waiting) at once.
    pub misses: usize,
}

impl Soft {
    /// A key with `master` as its secret, touched `touch_after` after it
    /// starts waiting (never with `None`).
    pub fn new(master: [u8; 32], touch_after: Option<Duration>) -> Soft {
        Soft { master, cid: [0x50, 0x4F, 0x46, 0x54], incoming: None, out: VecDeque::new(), pending: None, touch_after, agreement: None, counter: 0, touches: 0, misses: 0 }
    }

    /// The device it stands for.
    pub fn device() -> Device {
        Device { path: "soft".into(), name: "Olo Soft Key".into(), vendor: 0, product: 0 }
    }

    /// Debug builds' `OLO_FIDO_SOFT` key, if set.
    pub fn from_env() -> Option<Soft> {
        if !cfg!(debug_assertions) {
            return None;
        }
        let file = std::env::var_os("OLO_FIDO_SOFT")?;
        let master = master_in(Path::new(&file))?;
        let touch = match std::env::var("OLO_FIDO_SOFT_TOUCH_MS").as_deref() {
            Ok("never") => None,
            Ok(ms) => Some(Duration::from_millis(ms.parse().unwrap_or(400))),
            Err(_) => Some(Duration::from_millis(400)),
        };
        Some(Soft::new(master, touch))
    }

    fn reply(&mut self, cid: [u8; 4], cmd: u8, data: &[u8]) {
        let mut r = [0u8; REPORT];
        r[..4].copy_from_slice(&cid);
        r[4] = cmd | 0x80;
        r[5..7].copy_from_slice(&(data.len() as u16).to_be_bytes());
        let first = data.len().min(REPORT - 7);
        r[7..7 + first].copy_from_slice(&data[..first]);
        self.out.push_back(r);
        let (mut off, mut seq) = (first, 0u8);
        while off < data.len() {
            let mut r = [0u8; REPORT];
            r[..4].copy_from_slice(&cid);
            r[4] = seq;
            let n = (data.len() - off).min(REPORT - 5);
            r[5..5 + n].copy_from_slice(&data[off..off + n]);
            self.out.push_back(r);
            off += n;
            seq += 1;
        }
    }

    fn message(&mut self, cid: [u8; 4], cmd: u8, data: Vec<u8>) {
        match cmd {
            // INIT: a channel of its own.
            0x06 => {
                let mut a = data[..8.min(data.len())].to_vec();
                a.extend_from_slice(&self.cid);
                a.extend_from_slice(&[2, 5, 8, 0, 0x05]);
                self.reply(cid, 0x06, &a);
            }
            0x01 => self.reply(cid, 0x01, &data),
            0x08 => self.reply(cid, 0x08, &[]),
            // CANCEL: whatever waits for a touch ends.
            0x11 => {
                if self.pending.take().is_some() {
                    self.reply(self.cid, 0x10, &[ctap::ERR_KEEPALIVE_CANCEL]);
                }
            }
            0x10 => {
                let Some((&command, params)) = data.split_first() else { return };
                let params: Option<Value> = if params.is_empty() { None } else { ciborium::from_reader(params).ok() };
                match self.cbor(command, params.as_ref()) {
                    Answer::Now(a) => self.reply(cid, 0x10, &a),
                    Answer::OnTouch(_) if self.misses > 0 => {
                        self.misses -= 1;
                        self.reply(cid, 0x10, &[ctap::ERR_OPERATION_DENIED]);
                    }
                    Answer::OnTouch(a) => self.pending = Some((self.touch_after.map(|d| Instant::now() + d), a)),
                }
            }
            _ => self.reply(cid, 0x3F, &[0x01]),
        }
    }

    fn cbor(&mut self, command: u8, p: Option<&Value>) -> Answer {
        let ok = |v: Value| {
            let mut a = vec![0u8];
            ciborium::into_writer(&v, &mut a).expect("encodes");
            a
        };
        let err = |code: u8| Answer::Now(vec![code]);
        match command {
            ctap::GET_INFO => Answer::Now(ok(int_map(vec![
                (1, Value::Array(vec![Value::Text("FIDO_2_0".into()), Value::Text("FIDO_2_1".into())])),
                (2, Value::Array(vec![Value::Text("hmac-secret".into())])),
                (3, Value::Bytes(vec![0x01; 16])),
                (4, text_map(vec![("rk", Value::Bool(true)), ("up", Value::Bool(true)), ("clientPin", Value::Bool(false)), ("makeCredUvNotRqd", Value::Bool(true))])),
                (6, Value::Array(vec![Value::Integer(2.into()), Value::Integer(1.into())])),
                (14, Value::Integer(0x050800.into())),
            ]))),
            ctap::CLIENT_PIN => {
                let Some(p) = p else { return err(0x14) };
                if get(p, 2).and_then(int) != Some(2) {
                    return err(0x01);
                }
                let k = pin::random_secret().expect("random");
                let public = pin::cose_key(&k.public_key());
                self.agreement = Some(k);
                Answer::Now(ok(int_map(vec![(1, public)])))
            }
            ctap::MAKE_CREDENTIAL => {
                let Some(p) = p else { return err(0x14) };
                let Some(rp) = get(p, 2).and_then(|r| r.as_map()).and_then(|m| m.iter().find(|(k, _)| k.as_text() == Some("id"))).and_then(|(_, v)| v.as_text()) else { return err(0x14) };
                let mut nonce = [0u8; 16];
                getrandom::fill(&mut nonce).expect("random");
                let id = [&nonce[..], &self.tag(&nonce)].concat();
                let mut auth = Sha256::digest(rp.as_bytes()).to_vec();
                self.counter += 1;
                auth.push(0x01 | 0x40 | 0x80);
                auth.extend_from_slice(&self.counter.to_be_bytes());
                auth.extend_from_slice(&[0x01; 16]);
                auth.extend_from_slice(&(id.len() as u16).to_be_bytes());
                auth.extend_from_slice(&id);
                ciborium::into_writer(&pin::cose_key(&pin::random_secret().expect("random").public_key()), &mut auth).expect("encodes");
                ciborium::into_writer(&text_map(vec![("hmac-secret", Value::Bool(true))]), &mut auth).expect("encodes");
                Answer::OnTouch(ok(int_map(vec![(1, Value::Text("none".into())), (2, Value::Bytes(auth)), (3, Value::Map(vec![]))])))
            }
            ctap::GET_ASSERTION => {
                let Some(p) = p else { return err(0x14) };
                let Some(rp) = get(p, 1).and_then(|r| r.as_text()) else { return err(0x14) };
                let ids: Vec<Vec<u8>> = get(p, 3)
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|d| d.as_map()?.iter().find(|(k, _)| k.as_text() == Some("id"))?.1.as_bytes().cloned()).collect())
                    .unwrap_or_default();
                let Some(id) = ids.into_iter().find(|id| id.len() == 32 && id[16..] == self.tag(&id[..16])) else { return err(ctap::ERR_NO_CREDENTIALS) };
                let up = get(p, 5).and_then(|o| o.as_map()).and_then(|m| m.iter().find(|(k, _)| k.as_text() == Some("up"))).and_then(|(_, v)| v.as_bool()).unwrap_or(true);
                let mut ext = None;
                if let Some(h) = get(p, 4).and_then(|e| e.as_map()).and_then(|m| m.iter().find(|(k, _)| k.as_text() == Some("hmac-secret"))).map(|(_, v)| v) {
                    let protocol = get(h, 4).and_then(int).unwrap_or(1) as u8;
                    let (Some(platform), Some(salt_enc), Some(salt_auth)) = (get(h, 1), get(h, 2).and_then(|s| s.as_bytes()), get(h, 3).and_then(|s| s.as_bytes())) else { return err(0x14) };
                    let (Some(ours), Ok(theirs)) = (self.agreement.as_ref(), pin::cose_public(platform)) else { return err(0x14) };
                    let z = p256::ecdh::diffie_hellman(ours.to_nonzero_scalar(), theirs.as_affine());
                    let shared = pin::derive(z.raw_secret_bytes().as_slice(), protocol, Value::Null).expect("protocol");
                    if shared.authenticate(salt_enc) != *salt_auth {
                        return err(0x33);
                    }
                    let Ok(salt) = shared.decrypt(salt_enc) else { return err(0x14) };
                    let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&self.cred_random(&id)).expect("any key length");
                    mac.update(&salt[..32]);
                    let output = mac.finalize().into_bytes();
                    ext = Some(text_map(vec![("hmac-secret", Value::Bytes(shared.encrypt(&output).expect("whole blocks")))]));
                }
                let mut auth = Sha256::digest(rp.as_bytes()).to_vec();
                self.counter += 1;
                auth.push(if up { 0x01 } else { 0 } | if ext.is_some() { 0x80 } else { 0 });
                auth.extend_from_slice(&self.counter.to_be_bytes());
                if let Some(e) = &ext {
                    ciborium::into_writer(e, &mut auth).expect("encodes");
                }
                let a = ok(int_map(vec![(1, text_map(vec![("id", Value::Bytes(id)), ("type", Value::Text("public-key".into()))])), (2, Value::Bytes(auth)), (3, Value::Bytes(vec![0; 64]))]));
                if up { Answer::OnTouch(a) } else { Answer::Now(a) }
            }
            _ => err(0x01),
        }
    }

    fn tag(&self, nonce: &[u8]) -> Vec<u8> {
        let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&self.master).expect("any key length");
        mac.update(b"credential");
        mac.update(nonce);
        mac.finalize().into_bytes()[..16].to_vec()
    }

    fn cred_random(&self, id: &[u8]) -> [u8; 32] {
        let mut mac = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&self.master).expect("any key length");
        mac.update(b"cred random");
        mac.update(id);
        mac.finalize().into_bytes().into()
    }
}

enum Answer {
    Now(Vec<u8>),
    OnTouch(Vec<u8>),
}

impl Transport for Soft {
    fn send(&mut self, r: &[u8; REPORT]) -> std::io::Result<()> {
        let cid = [r[0], r[1], r[2], r[3]];
        if r[4] & 0x80 != 0 {
            let len = u16::from_be_bytes([r[5], r[6]]) as usize;
            let data = r[7..7 + len.min(REPORT - 7)].to_vec();
            self.incoming = Some((cid, r[4] & 0x7F, len, data));
        } else if let Some((c, _, len, data)) = self.incoming.as_mut().filter(|(c, ..)| *c == cid) {
            let _ = c;
            let n = (*len - data.len()).min(REPORT - 5);
            data.extend_from_slice(&r[5..5 + n]);
        }
        if let Some((c, cmd, len, data)) = self.incoming.take() {
            if data.len() >= len {
                self.message(c, cmd, data);
            } else {
                self.incoming = Some((c, cmd, len, data));
            }
        }
        Ok(())
    }

    fn recv(&mut self, wait: Duration) -> std::io::Result<Option<[u8; REPORT]>> {
        if let Some(r) = self.out.pop_front() {
            return Ok(Some(r));
        }
        let Some((at, _)) = &self.pending else {
            std::thread::sleep(wait.min(Duration::from_millis(50)));
            return Ok(None);
        };
        let now = Instant::now();
        if at.is_some_and(|at| now >= at) {
            let (_, answer) = self.pending.take().expect("pending");
            self.touches += 1;
            self.reply(self.cid, 0x10, &answer);
            return Ok(self.out.pop_front());
        }
        // Still waiting for a touch: a keepalive saying so, every 100 ms.
        let nap = at.map(|at| at - now).unwrap_or(wait).min(wait).min(Duration::from_millis(100));
        std::thread::sleep(nap);
        let mut r = [0u8; REPORT];
        r[..4].copy_from_slice(&self.cid);
        r[4] = KEEPALIVE | 0x80;
        r[6] = 1;
        r[7] = 2;
        Ok(Some(r))
    }
}

/// The master secret kept in `file`, made there (readable by the person
/// alone) the first time.
fn master_in(file: &Path) -> Option<[u8; 32]> {
    if let Ok(b) = std::fs::read(file)
        && let Ok(m) = <[u8; 32]>::try_from(b.as_slice())
    {
        return Some(m);
    }
    let mut m = [0u8; 32];
    getrandom::fill(&mut m).ok()?;
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file).ok()?;
    f.write_all(&m).ok()?;
    Some(m)
}
