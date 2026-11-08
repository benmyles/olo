//! CTAPHID (messages over 64-byte reports, on a channel of one's own)
//! and the CTAP2 commands Olo uses: authenticatorGetInfo, ClientPIN's
//! key agreement, MakeCredential and GetAssertion with `hmac-secret`.
//! Every wait can be cancelled (`Wait::cancel`, sent to the key as
//! CTAPHID_CANCEL) and says when a touch is wanted (`Wait::touch`).

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use ciborium::Value;

use crate::{
    Error, Result,
    hid::{REPORT, Transport},
};

const BROADCAST: [u8; 4] = [0xFF; 4];
const CMD_PING: u8 = 0x01;
const CMD_INIT: u8 = 0x06;
const CMD_WINK: u8 = 0x08;
const CMD_CBOR: u8 = 0x10;
const CMD_CANCEL: u8 = 0x11;
const CMD_KEEPALIVE: u8 = 0x3B;
const CMD_ERROR: u8 = 0x3F;
/// KEEPALIVE's status while the key waits for a touch.
const UP_NEEDED: u8 = 2;

pub const GET_INFO: u8 = 0x04;
pub const CLIENT_PIN: u8 = 0x06;
pub const MAKE_CREDENTIAL: u8 = 0x01;
pub const GET_ASSERTION: u8 = 0x02;

/// CTAP2's status codes Olo tells apart.
pub const ERR_NO_CREDENTIALS: u8 = 0x2E;
pub const ERR_KEEPALIVE_CANCEL: u8 = 0x2D;
pub const ERR_USER_ACTION_TIMEOUT: u8 = 0x2F;
pub const ERR_OPERATION_DENIED: u8 = 0x27;
pub const ERR_PIN_REQUIRED: u8 = 0x36;
pub const ERR_PIN_INVALID: u8 = 0x31;
pub const ERR_PIN_BLOCKED: u8 = 0x32;
pub const ERR_PIN_AUTH_INVALID: u8 = 0x33;
pub const ERR_PIN_AUTH_BLOCKED: u8 = 0x34;
pub const ERR_PIN_POLICY_VIOLATION: u8 = 0x37;
pub const ERR_UNSUPPORTED_EXTENSION: u8 = 0x16;
/// CTAPHID's: the key is busy with another channel's request.
pub const HID_ERR_BUSY: u8 = 0x06;

/// How a command waits: cancelled when `cancel` turns true, `touch`
/// called (once) when the key starts waiting for one.
pub struct Wait<'a> {
    pub cancel: &'a AtomicBool,
    pub touch: &'a (dyn Fn() + Sync),
    /// Given up on (the key gives up on a touch by itself after about
    /// 30 seconds; this is a backstop).
    pub limit: Duration,
}

impl Wait<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A channel of one's own on a key (`CTAPHID_INIT`).
pub struct Channel<T: Transport> {
    t: T,
    cid: [u8; 4],
    /// CTAPHID's capabilities: wink (1), CBOR (4), no MSG (8).
    pub capabilities: u8,
    /// The key's firmware, as CTAPHID_INIT says it.
    pub version: (u8, u8, u8),
}

impl<T: Transport> Channel<T> {
    pub fn open(mut t: T) -> Result<Self> {
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
        send(&mut t, BROADCAST, CMD_INIT, &nonce)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let Some((cmd, data)) = recv(&mut t, BROADCAST, deadline)? else { return Err(Error::Timeout) };
            // (Another program's INIT answered on the broadcast channel.)
            if cmd != CMD_INIT || data.len() < 17 || data[..8] != nonce {
                continue;
            }
            let cid = [data[8], data[9], data[10], data[11]];
            return Ok(Channel { t, cid, capabilities: data[16], version: (data[13], data[14], data[15]) });
        }
    }

    /// Whether the key speaks CTAP2 (CBOR).
    pub fn cbor_capable(&self) -> bool {
        self.capabilities & 4 != 0
    }

    /// Blink the key's light (to say which one, of several).
    pub fn wink(&mut self) -> Result<()> {
        if self.capabilities & 1 == 0 {
            return Ok(());
        }
        send(&mut self.t, self.cid, CMD_WINK, &[])?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while let Some((cmd, data)) = recv(&mut self.t, self.cid, deadline)? {
            match cmd {
                CMD_WINK => return Ok(()),
                CMD_ERROR => return Err(Error::Hid(data.first().copied().unwrap_or(0))),
                _ => {}
            }
        }
        Err(Error::Timeout)
    }

    /// CTAPHID_PING: `data` echoed.
    pub fn ping(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        send(&mut self.t, self.cid, CMD_PING, data)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Some((cmd, back)) = recv(&mut self.t, self.cid, deadline)? {
            if cmd == CMD_PING {
                return Ok(back);
            }
        }
        Err(Error::Timeout)
    }

    /// A CTAP2 command and its parameters (a CBOR map), waiting through
    /// the key's keepalives; its answer, if any.
    pub fn cbor(&mut self, command: u8, params: Option<Value>, wait: &Wait) -> Result<Option<Value>> {
        let mut data = vec![command];
        if let Some(p) = params {
            ciborium::into_writer(&p, &mut data).map_err(|_| Error::Cbor)?;
        }
        send(&mut self.t, self.cid, CMD_CBOR, &data)?;
        let start = Instant::now();
        let mut told = false;
        let mut cancel_sent = false;
        loop {
            if wait.cancelled() && !cancel_sent {
                send(&mut self.t, self.cid, CMD_CANCEL, &[])?;
                cancel_sent = true;
            }
            if start.elapsed() > wait.limit {
                if !cancel_sent {
                    let _ = send(&mut self.t, self.cid, CMD_CANCEL, &[]);
                }
                return Err(Error::Timeout);
            }
            // (A short look, so a cancel goes out promptly.)
            let Some((cmd, back)) = recv(&mut self.t, self.cid, Instant::now() + Duration::from_millis(100))? else {
                if cancel_sent && start.elapsed() > wait.limit.min(Duration::from_secs(2)) + Duration::from_secs(1) {
                    return Err(Error::Cancelled);
                }
                continue;
            };
            match cmd {
                CMD_KEEPALIVE => {
                    if back.first() == Some(&UP_NEEDED) && !told {
                        told = true;
                        (wait.touch)();
                    }
                }
                CMD_CBOR => {
                    let Some(&status) = back.first() else { return Err(Error::Protocol("an empty CBOR answer")) };
                    if status == ERR_KEEPALIVE_CANCEL || (cancel_sent && status != 0) {
                        return Err(Error::Cancelled);
                    }
                    if status != 0 {
                        return Err(Error::Ctap(status));
                    }
                    if back.len() == 1 {
                        return Ok(None);
                    }
                    return ciborium::from_reader(&back[1..]).map(Some).map_err(|_| Error::Cbor);
                }
                CMD_ERROR => return Err(Error::Hid(back.first().copied().unwrap_or(0))),
                _ => {}
            }
        }
    }

    /// authenticatorGetInfo.
    pub fn get_info(&mut self) -> Result<Info> {
        let never = AtomicBool::new(false);
        let wait = Wait { cancel: &never, touch: &|| {}, limit: Duration::from_secs(3) };
        let v = self.cbor(GET_INFO, None, &wait)?.ok_or(Error::Protocol("no info"))?;
        Info::from_cbor(&v)
    }
}

/// One CTAPHID message out, in reports.
fn send(t: &mut impl Transport, cid: [u8; 4], cmd: u8, data: &[u8]) -> Result<()> {
    if data.len() > 7609 {
        return Err(Error::Protocol("a message too long for CTAPHID"));
    }
    let mut r = [0u8; REPORT];
    r[..4].copy_from_slice(&cid);
    r[4] = cmd | 0x80;
    r[5..7].copy_from_slice(&(data.len() as u16).to_be_bytes());
    let first = data.len().min(REPORT - 7);
    r[7..7 + first].copy_from_slice(&data[..first]);
    t.send(&r)?;
    let mut off = first;
    let mut seq = 0u8;
    while off < data.len() {
        let mut r = [0u8; REPORT];
        r[..4].copy_from_slice(&cid);
        r[4] = seq;
        let n = (data.len() - off).min(REPORT - 5);
        r[5..5 + n].copy_from_slice(&data[off..off + n]);
        t.send(&r)?;
        off += n;
        seq += 1;
    }
    Ok(())
}

/// The next whole message on `cid`, if one starts before `deadline`
/// (its continuations may come up to a second after).
fn recv(t: &mut impl Transport, cid: [u8; 4], deadline: Instant) -> Result<Option<(u8, Vec<u8>)>> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(r) = t.recv(left)? else { return Ok(None) };
        if r[..4] != cid || r[4] & 0x80 == 0 {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            continue;
        }
        let cmd = r[4] & 0x7F;
        let len = u16::from_be_bytes([r[5], r[6]]) as usize;
        let mut data = r[7..7 + len.min(REPORT - 7)].to_vec();
        let mut seq = 0u8;
        let more = Instant::now() + Duration::from_secs(1);
        while data.len() < len {
            let Some(c) = t.recv(more.saturating_duration_since(Instant::now()))? else { return Err(Error::Timeout) };
            if c[..4] != cid {
                continue;
            }
            if c[4] != seq {
                return Err(Error::Protocol("a CTAPHID continuation out of order"));
            }
            let n = (len - data.len()).min(REPORT - 5);
            data.extend_from_slice(&c[5..5 + n]);
            seq += 1;
        }
        return Ok(Some((cmd, data)));
    }
}

/// What authenticatorGetInfo says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// "FIDO_2_0", "FIDO_2_1", "U2F_V2"…
    pub versions: Vec<String>,
    /// "hmac-secret", "credProtect"…
    pub extensions: Vec<String>,
    pub aaguid: [u8; 16],
    /// "rk", "up", "clientPin", "makeCredUvNotRqd"…
    pub options: BTreeMap<String, bool>,
    pub pin_protocols: Vec<u8>,
    /// As a number (YubiKey: 0x050800 for 5.8.0).
    pub firmware: Option<u64>,
}

impl Info {
    pub fn from_cbor(v: &Value) -> Result<Info> {
        let mut info = Info::default();
        for (k, v) in map(v)? {
            match int(k) {
                Some(1) => info.versions = texts(v),
                Some(2) => info.extensions = texts(v),
                Some(3) => {
                    if let Some(b) = v.as_bytes().filter(|b| b.len() == 16) {
                        info.aaguid.copy_from_slice(b);
                    }
                }
                Some(4) => {
                    for (o, on) in map(v)? {
                        if let (Some(o), Some(on)) = (o.as_text(), on.as_bool()) {
                            info.options.insert(o.to_owned(), on);
                        }
                    }
                }
                Some(6) => info.pin_protocols = v.as_array().map(|a| a.iter().filter_map(|p| int(p).map(|p| p as u8)).collect()).unwrap_or_default(),
                Some(14) => info.firmware = int(v).map(|f| f as u64),
                _ => {}
            }
        }
        Ok(info)
    }

    pub fn option(&self, name: &str) -> Option<bool> {
        self.options.get(name).copied()
    }

    /// Whether it has `hmac-secret`, which Olo's sealing needs.
    pub fn hmac_secret(&self) -> bool {
        self.extensions.iter().any(|e| e == "hmac-secret")
    }

    /// Whether a PIN is set on it.
    pub fn pin_set(&self) -> bool {
        self.option("clientPin") == Some(true)
    }

    /// Whether making a credential needs the PIN (one set, and the key
    /// not saying a non-discoverable credential needs none).
    pub fn make_credential_needs_pin(&self) -> bool {
        self.pin_set() && self.option("makeCredUvNotRqd") != Some(true)
    }

    /// The PIN protocol to use: 2 where it has it, else 1.
    pub fn pin_protocol(&self) -> u8 {
        if self.pin_protocols.contains(&2) { 2 } else { 1 }
    }

    /// The firmware as "5.8.0" (YubiKeys give it so; others as a number).
    pub fn firmware_text(&self) -> Option<String> {
        self.firmware.map(|f| if f > 0xFFFF { format!("{}.{}.{}", f >> 16, (f >> 8) & 0xFF, f & 0xFF) } else { f.to_string() })
    }
}

pub(crate) fn map(v: &Value) -> Result<&Vec<(Value, Value)>> {
    v.as_map().ok_or(Error::Protocol("not a CBOR map"))
}

pub(crate) fn int(v: &Value) -> Option<i128> {
    v.as_integer().map(i128::from)
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|t| t.as_text().map(str::to_owned)).collect()).unwrap_or_default()
}

/// The value at integer key `k` of a CBOR map.
pub(crate) fn get(v: &Value, k: i128) -> Option<&Value> {
    v.as_map()?.iter().find(|(key, _)| int(key) == Some(k)).map(|(_, v)| v)
}

/// A CBOR map with integer keys, in canonical order (CTAP2 wants it).
pub(crate) fn int_map(mut entries: Vec<(i64, Value)>) -> Value {
    // Canonical CBOR: shorter encodings first, then bytewise; for
    // integers that's the positive ones ascending, then the negative
    // ones descending (-1, -2, -3).
    entries.sort_by_key(|(k, _)| if *k >= 0 { (0, *k) } else { (1, -*k) });
    Value::Map(entries.into_iter().map(|(k, v)| (Value::Integer(k.into()), v)).collect())
}

/// A CBOR map with text keys, in canonical order (by length, then bytes).
pub(crate) fn text_map(mut entries: Vec<(&str, Value)>) -> Value {
    entries.sort_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then(a.cmp(b)));
    Value::Map(entries.into_iter().map(|(k, v)| (Value::Text(k.to_owned()), v)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_maps() {
        let m = int_map(vec![(-3, Value::Null), (3, Value::Null), (-1, Value::Null), (1, Value::Null), (-2, Value::Null)]);
        let keys: Vec<i128> = m.as_map().unwrap().iter().map(|(k, _)| int(k).unwrap()).collect();
        assert_eq!(keys, [1, 3, -1, -2, -3]);
        let t = text_map(vec![("type", Value::Null), ("id", Value::Null), ("hmac-secret", Value::Null), ("up", Value::Null)]);
        let keys: Vec<&str> = t.as_map().unwrap().iter().map(|(k, _)| k.as_text().unwrap()).collect();
        assert_eq!(keys, ["id", "up", "type", "hmac-secret"]);
    }

    /// Reports kept in memory: what was sent, and answers queued.
    struct Loop {
        sent: Vec<[u8; REPORT]>,
        answers: std::collections::VecDeque<[u8; REPORT]>,
    }

    impl Transport for Loop {
        fn send(&mut self, r: &[u8; REPORT]) -> std::io::Result<()> {
            self.sent.push(*r);
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> std::io::Result<Option<[u8; REPORT]>> {
            Ok(self.answers.pop_front())
        }
    }

    #[test]
    fn long_messages_in_continuations() {
        let data: Vec<u8> = (0..200u8).collect();
        let mut l = Loop { sent: vec![], answers: Default::default() };
        send(&mut l, [1, 2, 3, 4], CMD_CBOR, &data).unwrap();
        // 57 in the first report, then 59 a continuation: four reports.
        assert_eq!(l.sent.len(), 4);
        assert_eq!(l.sent[0][4], 0x90);
        assert_eq!(&l.sent[0][5..7], &[0, 200]);
        assert_eq!(l.sent[1][4], 0);
        assert_eq!(l.sent[3][4], 2);
        // Read back as they were sent.
        l.answers = l.sent.drain(..).collect();
        let (cmd, back) = recv(&mut l, [1, 2, 3, 4], Instant::now() + Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!((cmd, back), (CMD_CBOR, data));
    }
}
