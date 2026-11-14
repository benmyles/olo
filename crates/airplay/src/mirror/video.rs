//! A mirrored screen's pictures as the receiver takes them, on the data
//! connection (TCP, to the port the video stream's SETUP named): each a
//! 128-byte header and what it carries.
//!
//! - *The decoder's configuration* (header byte 4: 1), plain: H.264's
//!   parameter sets as an `avcC` record, and the picture's size. Sent
//!   before the first picture and again whenever the sets change (a
//!   window that changed size: a new encoder).
//! - *A picture* (byte 4: 0; byte 5: 0x10 for a key frame): one access
//!   unit's slices, each its length (4 bytes) and the unit (MP4's form,
//!   not Annex B's start codes), sealed with ChaCha20-Poly1305: the key
//!   from the connection's pairing (HKDF over pair-verify's secret with
//!   "DataStream-Salt" and the stream's id), the nonce a count of the
//!   pictures, the header vouched for (its first four bytes are the
//!   sealed length, tag included).
//! - *A heartbeat* (byte 4: 2), header alone, every second once
//!   pictures flow: a still screen sends no pictures.
//!
//! Each picture's stamp (bytes 8–15, little-endian fixed point) is when
//! it is to be shown on the receiver's clock (`clock`), and bytes 40–47
//! name that clock's timeline where it keeps one (PTP).
//!
//! As doubletake (LGPL, a sender for Linux that shows on today's Apple
//! TVs) and UxPlay (a receiver) have found it; written here from what
//! they say of the wire, not from their code.

use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit as _, Nonce,
    aead::{Aead as _, Payload},
};

pub(crate) const HEADER: usize = 128;

/// The NAL units of an Annex B stream, without their start codes.
pub(crate) fn units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &from) in starts.iter().enumerate() {
        let mut to = starts.get(n + 1).map_or(stream.len(), |next| next - 3);
        // (A four-byte start code's leading zero belongs to it, as do
        // trailing zeros.)
        while to > from && stream[to - 1] == 0 && n + 1 < starts.len() {
            to -= 1;
        }
        if to > from {
            out.push(&stream[from..to]);
        }
    }
    out
}

/// One encoded picture, as the data connection takes it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Picture {
    /// Its slices, each length-prefixed.
    pub units: Vec<u8>,
    pub key: bool,
    /// Parameter sets that came with it.
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

/// An encoder's Annex B output for one picture, sorted: slices (types 1
/// to 5) kept for the picture, parameter sets (7, 8) apart, the rest
/// (SEI, delimiters) left out.
pub(crate) fn picture(annex_b: &[u8]) -> Picture {
    let mut p = Picture::default();
    for unit in units(annex_b) {
        match unit[0] & 0x1f {
            1..=5 => {
                p.key |= unit[0] & 0x1f == 5;
                p.units.extend((unit.len() as u32).to_be_bytes());
                p.units.extend(unit);
            }
            7 => p.sps = Some(unit.to_vec()),
            8 => p.pps = Some(unit.to_vec()),
            _ => {}
        }
    }
    p
}

/// The decoder's configuration (MP4's `avcC`, four-byte lengths), and
/// the four bytes the receiver wants after it.
pub(crate) fn configuration(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut out = vec![1, sps.get(1).copied().unwrap_or(66), sps.get(2).copied().unwrap_or(0), sps.get(3).copied().unwrap_or(31), 0xff, 0xe1];
    out.extend((sps.len() as u16).to_be_bytes());
    out.extend(sps);
    out.push(1);
    out.extend((pps.len() as u16).to_be_bytes());
    out.extend(pps);
    out.extend([2, 0, 0, 0]);
    out
}

fn float(h: &mut [u8; HEADER], at: usize, v: f32) {
    h[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The configuration's header: its length, its stamp, the picture's
/// size (and the screen it's from and the part shown, the whole of it).
pub(crate) fn configuration_header(len: usize, stamp: u64, width: u32, height: u32) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0..4].copy_from_slice(&(len as u32).to_le_bytes());
    (h[4], h[5], h[6], h[7]) = (1, 0, 0x16, 1);
    h[8..16].copy_from_slice(&stamp.to_le_bytes());
    for at in [16, 40, 56] {
        float(&mut h, at, width as f32);
        float(&mut h, at + 4, height as f32);
    }
    h
}

/// A picture's header: its sealed length, whether it's a key frame, when
/// it's shown, and on which timeline (0: NTP's).
pub(crate) fn picture_header(sealed: usize, key: bool, stamp: u64, timeline: u64) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0..4].copy_from_slice(&(sealed as u32).to_le_bytes());
    h[5] = if key { 0x10 } else { 0 };
    h[8..16].copy_from_slice(&stamp.to_le_bytes());
    h[40..48].copy_from_slice(&timeline.to_le_bytes());
    h
}

pub(crate) fn heartbeat() -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    (h[4], h[6]) = (2, 0x1e);
    h
}

/// The pictures' cipher: the stream's key, a count for the nonce.
pub(crate) struct Sealer {
    aead: ChaCha20Poly1305,
    sent: u64,
}

fn nonce(n: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_le_bytes());
    Nonce::from(b)
}

pub(crate) const TAG: usize = 16;

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Sealer {
        Sealer { aead: ChaCha20Poly1305::new(key.into()), sent: 0 }
    }

    /// A picture's header and its slices sealed, as sent.
    pub fn seal(&mut self, units: &[u8], key: bool, stamp: u64, timeline: u64) -> Vec<u8> {
        let header = picture_header(units.len() + TAG, key, stamp, timeline);
        let sealed = self.aead.encrypt(&nonce(self.sent), Payload { msg: units, aad: &header }).expect("sealing a picture");
        self.sent += 1;
        let mut out = Vec::with_capacity(HEADER + sealed.len());
        out.extend(header);
        out.extend(sealed);
        out
    }

    /// (The receiver's side, for a test's receiver.)
    pub fn open(&mut self, header: &[u8; HEADER], sealed: &[u8]) -> Option<Vec<u8>> {
        let plain = self.aead.decrypt(&nonce(self.sent), Payload { msg: sealed, aad: header }).ok()?;
        self.sent += 1;
        Some(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_sorted_into_a_picture() {
        // An IDR picture as encoders write it: a delimiter, SPS, PPS, SEI
        // and two slices, with three- and four-byte start codes.
        let stream = [
            &[0u8, 0, 0, 1, 9, 0xf0][..],
            &[0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xaa],
            &[0, 0, 1, 0x68, 0xce, 0x3c, 0x80],
            &[0, 0, 1, 6, 5, 1, 0x80],
            &[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00],
            &[0, 0, 1, 0x65, 0x00, 0x11],
        ]
        .concat();
        let p = picture(&stream);
        assert!(p.key);
        assert_eq!(p.sps.as_deref(), Some(&[0x67, 0x42, 0xc0, 0x1f, 0xaa][..]));
        assert_eq!(p.pps.as_deref(), Some(&[0x68, 0xce, 0x3c, 0x80][..]));
        // (A slice's own trailing zero byte before the next start code
        // belongs to that code.)
        assert_eq!(p.units, [&[0, 0, 0, 3, 0x65, 0x88, 0x84][..], &[0, 0, 0, 3, 0x65, 0x00, 0x11]].concat());
        let p = picture(&[0, 0, 0, 1, 0x41, 0x9a, 0x02]);
        assert!(!p.key && p.sps.is_none());
        assert_eq!(p.units, [0, 0, 0, 3, 0x41, 0x9a, 0x02]);
    }

    #[test]
    fn the_configuration_and_its_header() {
        let c = configuration(&[0x67, 0x64, 0x00, 0x28, 1], &[0x68, 2]);
        assert_eq!(c, [1, 0x64, 0x00, 0x28, 0xff, 0xe1, 0, 5, 0x67, 0x64, 0x00, 0x28, 1, 1, 0, 2, 0x68, 2, 2, 0, 0, 0]);
        let h = configuration_header(c.len(), 0x0102_0304_0506_0708, 1920, 1080);
        assert_eq!(&h[..8], &[22, 0, 0, 0, 1, 0, 0x16, 1]);
        assert_eq!(&h[8..16], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(f32::from_le_bytes(h[16..20].try_into().unwrap()), 1920.0);
        assert_eq!(f32::from_le_bytes(h[60..64].try_into().unwrap()), 1080.0);
        assert_eq!(&heartbeat()[..8], &[0, 0, 0, 0, 2, 0, 0x1e, 0]);
    }

    #[test]
    fn pictures_sealed_and_opened_in_turn() {
        let key = [7u8; 32];
        let (mut out, mut back) = (Sealer::new(&key), Sealer::new(&key));
        for (n, units) in [b"first picture".as_slice(), b"second"].iter().enumerate() {
            let sent = out.seal(units, n == 0, 99, 5);
            let header: [u8; HEADER] = sent[..HEADER].try_into().unwrap();
            assert_eq!(u32::from_le_bytes(header[..4].try_into().unwrap()) as usize, units.len() + TAG);
            assert_eq!(header[5], if n == 0 { 0x10 } else { 0 });
            assert_eq!(u64::from_le_bytes(header[40..48].try_into().unwrap()), 5);
            assert_eq!(back.open(&header, &sent[HEADER..]).as_deref(), Some(*units));
        }
        // Out of turn, or with its header changed, it doesn't open.
        let sent = out.seal(b"third", false, 1, 0);
        let mut header: [u8; HEADER] = sent[..HEADER].try_into().unwrap();
        header[8] ^= 1;
        assert_eq!(back.open(&header, &sent[HEADER..]), None);
    }
}
