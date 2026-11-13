//! A test's receiver's side of a mirrored screen (`play::fake::Tv`
//! takes the requests and calls these): the pictures' connection,
//! opened with the key the pairing gives, and the sound's packets,
//! opened with the key the sender gave; what came kept for the test.

use std::{
    io::Read as _,
    net::{TcpListener, UdpSocket},
    sync::{Arc, Mutex},
    time::Duration,
};

use chacha20poly1305::{ChaCha20Poly1305, KeyInit as _};

use super::{sound, video};
use crate::pairing::hap::derive;

/// What a mirrored screen sent.
#[derive(Debug, Default)]
pub struct Shown {
    /// The decoder's configurations (`avcC`, the receiver's four bytes
    /// after it), and the size each said.
    pub configurations: Vec<(Vec<u8>, (f32, f32))>,
    /// Each picture: a key frame, its stamp and timeline, and its slices
    /// (length-prefixed).
    pub pictures: Vec<(bool, u64, u64, Vec<u8>)>,
    pub heartbeats: usize,
    /// A picture that didn't open (not sealed with the pairing's key).
    pub refused: usize,
    /// The sound's samples, as they came (the first ten seconds), and the
    /// packets' RTP times.
    pub samples: Vec<i16>,
    pub rtp: Vec<u32>,
    /// Sync packets on the control port.
    pub syncs: Vec<Vec<u8>>,
    /// It's to close the pictures' connection after this many pictures
    /// (someone there stopped it).
    pub stop_after: Option<usize>,
    /// The data connection was closed by the sender.
    pub closed: bool,
}

/// A port for pictures of stream `id`, on a connection proved with
/// `secret`; each read and kept in `shown`.
pub(crate) fn pictures(secret: [u8; 32], id: u64, shown: Arc<Mutex<Shown>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port for pictures");
    let port = listener.local_addr().expect("its address").port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let mut sealer = video::Sealer::new(&derive(&secret, &format!("DataStream-Salt{id}"), "DataStream-Output-Encryption-Key"));
        let mut header = [0u8; video::HEADER];
        loop {
            if stream.read_exact(&mut header).is_err() {
                shown.lock().unwrap().closed = true;
                return;
            }
            let size = u32::from_le_bytes(header[..4].try_into().expect("four bytes")) as usize;
            let mut body = vec![0u8; size];
            if stream.read_exact(&mut body).is_err() {
                shown.lock().unwrap().closed = true;
                return;
            }
            let mut s = shown.lock().unwrap();
            match header[4] {
                0 => match sealer.open(&header, &body) {
                    Some(units) => {
                        let (stamp, timeline) = (u64::from_le_bytes(header[8..16].try_into().expect("8")), u64::from_le_bytes(header[40..48].try_into().expect("8")));
                        s.pictures.push((header[5] == 0x10, stamp, timeline, units));
                    }
                    None => s.refused += 1,
                },
                1 => {
                    let f = |at: usize| f32::from_le_bytes(header[at..at + 4].try_into().expect("4"));
                    s.configurations.push((body, (f(16), f(20))));
                }
                2 => s.heartbeats += 1,
                _ => {}
            }
            if s.stop_after.is_some_and(|n| s.pictures.len() >= n) {
                return;
            }
        }
    });
    port
}

/// Ports for sound sealed with `key` (data, control); what comes kept in
/// `shown`.
pub(crate) fn sound(key: [u8; 32], shown: Arc<Mutex<Shown>>) -> (u16, u16) {
    let (data, control) = (UdpSocket::bind("127.0.0.1:0").expect("a port for sound"), UdpSocket::bind("127.0.0.1:0").expect("a control port"));
    let ports = (data.local_addr().expect("its address").port(), control.local_addr().expect("its address").port());
    for s in [&data, &control] {
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    }
    let heard = shown.clone();
    std::thread::spawn(move || {
        let aead = ChaCha20Poly1305::new(&key.into());
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = data.recv_from(&mut buf) {
            let Some((_, rtp, frame)) = sound::unpacket(&aead, &buf[..n]) else { continue };
            let Some(samples) = sound::unalac(&frame) else { continue };
            let mut s = heard.lock().unwrap();
            s.rtp.push(rtp);
            if s.samples.len() < sound::RATE as usize * 20 {
                s.samples.extend(samples);
            }
        }
    });
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok((n, _)) = control.recv_from(&mut buf) {
            shown.lock().unwrap().syncs.push(buf[..n].to_vec());
        }
    });
    ports
}
