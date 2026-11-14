//! A receiver of a test's own to pair with: the other side of pair-setup
//! and pair-verify, as HomeKit says an accessory does them, in process
//! ([`Receiver`] takes the messages directly) or on the loopback
//! ([`serve`]). Not a receiver of anything: it pairs, proves itself, and
//! answers one question under the cipher.

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener},
    sync::{Arc, Mutex},
};

use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{Signer as _, SigningKey};

use super::{
    Cipher, Error, Post,
    hap::{NAME, derive, open, random, seal, signed_by},
    srp,
    tlv8::{self, Tlv},
};

pub struct Receiver {
    /// The code on its screen.
    pub code: String,
    pub id: Vec<u8>,
    signing: SigningKey,
    /// It was asked to show its code.
    pub shown: bool,
    /// Who it has paired with: their names for themselves, their keys,
    /// and what each said it's called.
    pub paired: Vec<(Vec<u8>, [u8; 32], Vec<u8>)>,
    srp: Option<srp::Server>,
    session: Option<[u8; 64]>,
    verifying: Option<([u8; 32], [u8; 32], [u8; 32])>,
    /// The keys of the last connection proved, to compare.
    pub proved: Option<[u8; 32]>,
    /// Signs for its key in pair-setup as HomeKit says (a test may have
    /// it not).
    pub signs: bool,
}

impl Receiver {
    pub fn new(code: &str) -> Receiver {
        Receiver {
            code: code.into(),
            id: b"AA:BB:CC:DD:EE:FF".to_vec(),
            signing: SigningKey::from_bytes(&random::<32>()),
            shown: false,
            paired: Vec::new(),
            srp: None,
            session: None,
            verifying: None,
            proved: None,
            signs: true,
        }
    }

    /// As it would be after being reset: a new key, nobody paired.
    pub fn reset(&mut self) {
        self.signing = SigningKey::from_bytes(&random::<32>());
        self.paired.clear();
    }

    fn refuse(state: u8, error: u8) -> Vec<u8> {
        Tlv::default().with(tlv8::STATE, [state]).with(tlv8::ERROR, [error]).write()
    }

    fn setup(&mut self, said: &Tlv) -> Vec<u8> {
        match said.byte(tlv8::STATE) {
            Some(1) => {
                let server = srp::Server::new(&random::<32>(), &random::<16>(), &self.code);
                let answer = Tlv::default().with(tlv8::STATE, [2]).with(tlv8::SALT, server.salt.clone()).with(tlv8::PUBLIC_KEY, server.public.clone());
                self.srp = Some(server);
                answer.write()
            }
            Some(3) => {
                let checked = self.srp.as_ref().zip(said.get(tlv8::PUBLIC_KEY)).zip(said.get(tlv8::PROOF)).and_then(|((srp, key), proof)| srp.check(key, proof));
                match checked {
                    Some((key, proof)) => {
                        self.session = Some(key);
                        Tlv::default().with(tlv8::STATE, [4]).with(tlv8::PROOF, proof).write()
                    }
                    None => Self::refuse(4, 2),
                }
            }
            Some(5) => {
                let Some(key) = self.session.take() else { return Self::refuse(6, 2) };
                let session = derive(&key, "Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info");
                let Some(theirs) = said.get(tlv8::ENCRYPTED_DATA).and_then(|sealed| open(&session, b"PS-Msg05", sealed)).and_then(|text| Tlv::read(&text)) else { return Self::refuse(6, 2) };
                let (Some(id), Some(public)) = (theirs.get(tlv8::IDENTIFIER), theirs.get(tlv8::PUBLIC_KEY).and_then(|k| <[u8; 32]>::try_from(k).ok())) else { return Self::refuse(6, 2) };
                let x = derive(&key, "Pair-Setup-Controller-Sign-Salt", "Pair-Setup-Controller-Sign-Info");
                if !signed_by(&public, &[&x[..], id, &public].concat(), theirs.get(tlv8::SIGNATURE)) {
                    return Self::refuse(6, 2);
                }
                self.paired.push((id.to_vec(), public, theirs.get(NAME).unwrap_or_default().to_vec()));
                let ours = self.signing.verifying_key().to_bytes();
                let y = derive(&key, "Pair-Setup-Accessory-Sign-Salt", "Pair-Setup-Accessory-Sign-Info");
                let signature = if self.signs { self.signing.sign(&[&y[..], &self.id, &ours].concat()).to_bytes().to_vec() } else { vec![0; 64] };
                let inner = Tlv::default().with(tlv8::IDENTIFIER, self.id.clone()).with(tlv8::PUBLIC_KEY, ours).with(tlv8::SIGNATURE, signature);
                Tlv::default().with(tlv8::STATE, [6]).with(tlv8::ENCRYPTED_DATA, seal(&session, b"PS-Msg06", &inner.write())).write()
            }
            _ => Self::refuse(0, 1),
        }
    }

    fn verify(&mut self, said: &Tlv) -> Vec<u8> {
        match said.byte(tlv8::STATE) {
            Some(1) => {
                let Some(theirs) = said.get(tlv8::PUBLIC_KEY).and_then(|k| <[u8; 32]>::try_from(k).ok()) else { return Self::refuse(2, 1) };
                let secret = random::<32>();
                let ours = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
                let shared = MontgomeryPoint(theirs).mul_clamped(secret).to_bytes();
                let key = derive(&shared, "Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info");
                let signature = self.signing.sign(&[&ours[..], &self.id, &theirs].concat());
                let inner = Tlv::default().with(tlv8::IDENTIFIER, self.id.clone()).with(tlv8::SIGNATURE, signature.to_bytes());
                self.verifying = Some((shared, theirs, ours));
                // (A proving begun: the last connection's is no more this one's.)
                self.proved = None;
                Tlv::default().with(tlv8::STATE, [2]).with(tlv8::PUBLIC_KEY, ours).with(tlv8::ENCRYPTED_DATA, seal(&key, b"PV-Msg02", &inner.write())).write()
            }
            Some(3) => {
                let Some((shared, theirs, ours)) = self.verifying.take() else { return Self::refuse(4, 2) };
                let key = derive(&shared, "Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info");
                let Some(inner) = said.get(tlv8::ENCRYPTED_DATA).and_then(|sealed| open(&key, b"PV-Msg03", sealed)).and_then(|text| Tlv::read(&text)) else { return Self::refuse(4, 2) };
                let known = inner.get(tlv8::IDENTIFIER).and_then(|id| self.paired.iter().find(|(known, ..)| known == id));
                match known {
                    Some((id, public, _)) if signed_by(public, &[&theirs[..], id, &ours].concat(), inner.get(tlv8::SIGNATURE)) => {
                        self.proved = Some(shared);
                        Tlv::default().with(tlv8::STATE, [4]).write()
                    }
                    _ => Self::refuse(4, 2),
                }
            }
            _ => Self::refuse(0, 1),
        }
    }

    /// The cipher of the connection last proved, from its side.
    pub fn control(&self) -> Option<Cipher> {
        let shared = self.proved?;
        Some(Cipher::new(&derive(&shared, "Control-Salt", "Control-Read-Encryption-Key"), &derive(&shared, "Control-Salt", "Control-Write-Encryption-Key")))
    }
}

impl Post for Receiver {
    fn post(&mut self, path: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        let said = Tlv::read(body).ok_or(Error::Unexpected("not TLV8"))?;
        Ok(match path {
            "/pair-pin-start" => {
                self.shown = true;
                Vec::new()
            }
            "/pair-setup" => self.setup(&said),
            "/pair-verify" => self.verify(&said),
            _ => return Err(Error::Status(404)),
        })
    }
}

/// The receiver on the loopback, for one connection: HTTP as a real one
/// speaks it for pairing, enciphered once a connection is proved, and
/// then answering `GET /info` with "a receiver of the test's own".
pub fn serve(receiver: Arc<Mutex<Receiver>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port on the loopback");
    let at = listener.local_addr().expect("its address");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        receiver.lock().unwrap().proved = None;
        let (mut came, mut cipher): (Vec<u8>, Option<Cipher>) = (Vec::new(), None);
        let mut buf = [0u8; 4096];
        loop {
            // A whole request: its head, and the body its length says.
            let request = loop {
                if let Some(end) = came.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&came[..end]).into_owned();
                    let length = head.lines().filter_map(|l| l.split_once(':')).find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    if came.len() >= end + 4 + length {
                        let body = came[end + 4..end + 4 + length].to_vec();
                        came.drain(..end + 4 + length);
                        break (head, body);
                    }
                }
                let Ok(n @ 1..) = stream.read(&mut buf) else { return };
                match &mut cipher {
                    Some(c) => match c.open(&buf[..n]) {
                        Ok(plain) => came.extend(plain),
                        Err(_) => return,
                    },
                    None => came.extend(&buf[..n]),
                }
            };
            let (head, body) = request;
            let mut words = head.split(' ');
            let (method, path) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
            let mut receiver = receiver.lock().unwrap();
            let (status, answer) = match (method, path) {
                ("GET", "/info") if cipher.is_some() => (200, b"a receiver of the test's own".to_vec()),
                ("POST", path) => match receiver.post(path, &body) {
                    Ok(answer) => (200, answer),
                    Err(_) => (404, Vec::new()),
                },
                _ => (470, Vec::new()),
            };
            let mut message = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n", answer.len()).into_bytes();
            message.extend(answer);
            let message = match &mut cipher {
                Some(c) => c.seal(&message),
                None => message,
            };
            if stream.write_all(&message).is_err() {
                return;
            }
            // Proved: enciphered from the next message on.
            if cipher.is_none() && receiver.proved.is_some() {
                cipher = receiver.control();
            }
        }
    });
    at
}
