//! A mirrored screen's sound, as the receiver takes it: what the computer
//! plays, 44.1 kHz stereo, in Apple Lossless frames of 352 samples
//! (stored as they are: ALAC's own escape for uncompressed frames, which
//! every decoder of it reads; 1.4 Mbit/s, nothing on a home network), one
//! to an RTP packet over UDP to the port its SETUP named.
//!
//! Each packet is sealed with ChaCha20-Poly1305 under a key of this
//! sender's own making, given to the receiver in the SETUP (`shk`), the
//! RTP header's timestamp and source vouched for, the nonce a count
//! (eight bytes, written after the tag). The receiver learns when each
//! sample plays from a sync packet on its control port, sent with the
//! first packet and every second after (`sync`): "at this time, the
//! sample at this RTP time is playing", the time this side's as
//! [`super::clock`] has it, the samples the lead behind what was just
//! captured. A packet it missed it asks for again there, by number, and
//! is sent it as it was.

use std::{
    collections::VecDeque,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit as _, Nonce,
    aead::{Aead as _, Payload},
};

/// Samples a second, and in a frame (ALAC's for screens).
pub(crate) const RATE: u32 = 44_100;
pub(crate) const FRAME: usize = 352;
/// SETUP's names for it: ALAC (`ct`), and its format bit for 44.1 kHz,
/// 16-bit stereo (`audioFormat`).
pub(crate) const ALAC: i64 = 2;
pub(crate) const ALAC_44100_16_2: i64 = 0x40000;

const DATA: u8 = 0x60;
const SYNC_NTP: u8 = 0xd4;
const SYNC_PTP: u8 = 0xd7;
const ASK_AGAIN: u8 = 0xd5;
const SENT_AGAIN: u8 = 0xd6;
/// Packets kept to be sent again.
const KEPT: usize = 512;

/// Bits written from the top of each byte down.
struct Bits {
    out: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.out.push(0);
            }
            let bit = (value >> i & 1) as u8;
            *self.out.last_mut().expect("a byte") |= bit << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// One ALAC frame of `samples` (left and right in turn), stored as they
/// are: a channel pair's element, its size given, uncompressed, and the
/// end.
pub(crate) fn alac(samples: &[i16]) -> Vec<u8> {
    let mut b = Bits { out: Vec::with_capacity(samples.len() * 2 + 8), used: 0 };
    // (A pair: element 1, instance 0, 12 bits unused; the size is given,
    // nothing shifted, not compressed.)
    b.put(1, 3);
    b.put(0, 4);
    b.put(0, 12);
    b.put(1, 1);
    b.put(0, 2);
    b.put(1, 1);
    b.put((samples.len() / 2) as u32, 32);
    for &s in samples {
        b.put(u32::from(s as u16), 16);
    }
    b.put(7, 3);
    b.out
}

/// Such a frame read back (a test's receiver's): its samples, or None if
/// it isn't one.
pub(crate) fn unalac(frame: &[u8]) -> Option<Vec<i16>> {
    let bit = |i: usize| frame.get(i / 8).map(|b| u32::from(b >> (7 - i % 8) & 1));
    let get = |from: usize, n: usize| (from..from + n).try_fold(0u32, |v, i| Some(v << 1 | bit(i)?));
    let (kind, sized, shifted, stored) = (get(0, 3)?, get(19, 1)?, get(20, 2)?, get(22, 1)?);
    if kind != 1 || sized != 1 || shifted != 0 || stored != 1 {
        return None;
    }
    let n = get(23, 32)? as usize;
    let samples: Option<Vec<i16>> = (0..n * 2).map(|i| get(55 + i * 16, 16).map(|v| v as u16 as i16)).collect();
    (get(55 + n * 32, 3)? == 7).then_some(samples?)
}

fn nonce(n: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_le_bytes());
    Nonce::from(b)
}

/// An RTP packet of one frame, sealed: the header, the frame sealed, and
/// the nonce.
pub(crate) fn packet(aead: &ChaCha20Poly1305, n: u64, seq: u16, rtp: u32, frame: &[u8]) -> Vec<u8> {
    let mut header = [0u8; 12];
    (header[0], header[1]) = (0x80, DATA);
    header[2..4].copy_from_slice(&seq.to_be_bytes());
    header[4..8].copy_from_slice(&rtp.to_be_bytes());
    let sealed = aead.encrypt(&nonce(n), Payload { msg: frame, aad: &header[4..12] }).expect("sealing a frame");
    let mut out = Vec::with_capacity(12 + sealed.len() + 8);
    out.extend(header);
    out.extend(sealed);
    out.extend(n.to_le_bytes());
    out
}

/// Such a packet opened (a test's receiver's): its number, RTP time and
/// frame.
pub(crate) fn unpacket(aead: &ChaCha20Poly1305, packet: &[u8]) -> Option<(u16, u32, Vec<u8>)> {
    if packet.len() < 12 + 16 + 8 || packet[0] != 0x80 || packet[1] & 0x7f != DATA {
        return None;
    }
    let (sealed, n) = packet[12..].split_at(packet.len() - 12 - 8);
    let n = u64::from_le_bytes(n.try_into().ok()?);
    let frame = aead.decrypt(&nonce(n), Payload { msg: sealed, aad: &packet[4..12] }).ok()?;
    Some((u16::from_be_bytes([packet[2], packet[3]]), u32::from_be_bytes(packet[4..8].try_into().ok()?), frame))
}

/// Where the receiver's clock stands, for a sync: the time now as its
/// timing has it (fixed point; NTP's with the epoch), and the timeline
/// for PTP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Now {
    Ntp(u64),
    Ptp(u64, u64),
}

/// A sync packet: at `now`, the sample at RTP time `playing` plays.
pub(crate) fn sync(first: bool, playing: u32, now: Now) -> Vec<u8> {
    let mut p = vec![if first { 0x90 } else { 0x80 }, 0, 0, 4];
    p.extend(playing.to_be_bytes());
    match now {
        Now::Ntp(t) => {
            p[1] = SYNC_NTP;
            p.extend(t.to_be_bytes());
            p.extend(playing.to_be_bytes());
        }
        Now::Ptp(t, timeline) => {
            p[1] = SYNC_PTP;
            p.extend(super::clock::nanoseconds(t).to_be_bytes());
            p.extend(playing.to_be_bytes());
            p.extend(timeline.to_be_bytes());
        }
    }
    p
}

/// The sound going to a receiver.
pub(crate) struct Sound {
    data: UdpSocket,
    to: SocketAddr,
    control: UdpSocket,
    control_to: SocketAddr,
    aead: ChaCha20Poly1305,
    sealed: u64,
    seq: u16,
    /// The first sample's capture and RTP time (the stream's start).
    began: Option<(Instant, u32)>,
    /// How far behind the samples' count the capture runs, in seconds,
    /// smoothed: the sound card's clock isn't quite the computer's, and
    /// syncs follow the samples (else a long film's sound would wander
    /// from its picture), not each callback's jitter.
    skew: f64,
    /// Samples sent so far, and pushed so far.
    sent: u64,
    pushed: u64,
    /// Samples not yet a whole frame.
    waiting: Vec<i16>,
    kept: VecDeque<(u16, Vec<u8>)>,
    /// How far behind capture the receiver plays, in samples.
    pub lead: u32,
    synced: bool,
}

impl Sound {
    pub fn new(data: UdpSocket, to: SocketAddr, control: UdpSocket, control_to: SocketAddr, key: &[u8; 32], lead: u32) -> Sound {
        Sound {
            data,
            to,
            control,
            control_to,
            aead: ChaCha20Poly1305::new(key.into()),
            sealed: 0,
            seq: u16::from_le_bytes(crate::pairing::hap::random::<2>()),
            began: None,
            skew: 0.0,
            sent: 0,
            pushed: 0,
            waiting: Vec::with_capacity(FRAME * 4),
            kept: VecDeque::with_capacity(KEPT),
            lead,
            synced: false,
        }
    }

    /// Seconds from the stream's start to `t`.
    fn since(&self, t: Instant) -> Option<f64> {
        let (t0, _) = self.began?;
        Some(if t >= t0 { (t - t0).as_secs_f64() } else { -((t0 - t).as_secs_f64()) })
    }

    /// The RTP time of the sample captured at `t` (where the stream has
    /// begun).
    fn rtp_at(&self, t: Instant) -> Option<u32> {
        let (_, r0) = self.began?;
        Some(r0.wrapping_add(((self.since(t)? - self.skew) * f64::from(RATE)).round() as i64 as u32))
    }

    /// `samples` (stereo, interleaved) captured at `at` (their first),
    /// sent as whole frames; the rest waits for the next. `now` says
    /// where the receiver's clock is for the first sync.
    pub fn push(&mut self, samples: &[i16], at: Instant, now: impl Fn(Instant) -> Option<Now>) -> std::io::Result<()> {
        if self.began.is_none() {
            let r0 = u32::from_le_bytes(crate::pairing::hap::random::<4>());
            self.began = Some((at - Duration::from_secs_f64(self.waiting.len() as f64 / 2.0 / f64::from(RATE)), r0));
        }
        // (Where these samples came, against where their count puts them.)
        if let Some(since) = self.since(at) {
            let off = since - self.pushed as f64 / f64::from(RATE);
            self.skew += (off - self.skew) * 0.01;
        }
        self.pushed += (samples.len() / 2) as u64;
        self.waiting.extend_from_slice(samples);
        while self.waiting.len() >= FRAME * 2 {
            if !self.synced
                && let Some(n) = self.began.and_then(|(t0, r0)| now(t0).map(|n| (n, r0)))
            {
                let (n, r0) = n;
                self.control.send_to(&sync(true, r0.wrapping_sub(self.lead), n), self.control_to)?;
                self.synced = true;
            }
            let frame = alac(&self.waiting[..FRAME * 2]);
            self.waiting.drain(..FRAME * 2);
            let rtp = self.began.map_or(0, |(_, r0)| r0).wrapping_add(self.sent as u32);
            let p = packet(&self.aead, self.sealed, self.seq, rtp, &frame);
            self.data.send_to(&p, self.to)?;
            if self.kept.len() == KEPT {
                self.kept.pop_front();
            }
            self.kept.push_back((self.seq, p));
            (self.sealed, self.seq, self.sent) = (self.sealed + 1, self.seq.wrapping_add(1), self.sent + FRAME as u64);
        }
        Ok(())
    }

    /// Where the stream is now, said again (every second).
    pub fn resync(&mut self, at: Instant, now: Now) -> std::io::Result<()> {
        let Some(rtp) = self.rtp_at(at) else { return Ok(()) };
        self.control.send_to(&sync(false, rtp.wrapping_sub(self.lead), now), self.control_to).map(drop)
    }

    /// What a receiver asking for packets again is sent: each it still
    /// has, as it was.
    pub fn again(&self, asked: &[u8]) -> Vec<Vec<u8>> {
        if asked.len() != 8 || asked[0] != 0x80 || asked[1] != ASK_AGAIN {
            return Vec::new();
        }
        let (first, count) = (u16::from_be_bytes([asked[4], asked[5]]), u16::from_be_bytes([asked[6], asked[7]]));
        (0..count.min(KEPT as u16))
            .filter_map(|i| {
                let seq = first.wrapping_add(i);
                self.kept.iter().find(|(s, _)| *s == seq).map(|(_, p)| [&[0x80, SENT_AGAIN, 0, 1][..], p].concat())
            })
            .collect()
    }

    /// The socket the receiver asks on, to listen on.
    pub fn control(&self) -> std::io::Result<UdpSocket> {
        self.control.try_clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_stored_as_it_is_and_read_back() {
        let samples: Vec<i16> = (0..FRAME as i32 * 2).map(|i| ((i * 977) % 65536 - 32768) as i16).collect();
        let frame = alac(&samples);
        // 55 bits of header, 16 a sample, 3 to end: whole bytes.
        assert_eq!(frame.len(), (55 + samples.len() * 16 + 3).div_ceil(8));
        assert_eq!(&frame[..3], &[0x20, 0x00, 0x12]);
        assert_eq!(unalac(&frame).as_deref(), Some(&samples[..]));
        assert_eq!(unalac(&[0u8; 20]), None);
    }

    #[test]
    fn packets_sealed_and_opened() {
        let aead = ChaCha20Poly1305::new(&[9u8; 32].into());
        let p = packet(&aead, 41, 7, 0xdead_beef, b"a frame");
        assert_eq!(&p[..12], &[0x80, 0x60, 0, 7, 0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0]);
        assert_eq!(&p[p.len() - 8..], &41u64.to_le_bytes());
        assert_eq!(unpacket(&aead, &p), Some((7, 0xdead_beef, b"a frame".to_vec())));
        let mut bad = p.clone();
        bad[5] ^= 1;
        assert_eq!(unpacket(&aead, &bad), None);
    }

    #[test]
    fn syncs_either_way() {
        let ntp = sync(true, 1000, Now::Ntp(0x0102_0304_0506_0708));
        assert_eq!(ntp, [0x90, 0xd4, 0, 4, 0, 0, 3, 0xe8, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 3, 0xe8]);
        let ptp = sync(false, 5, Now::Ptp(3 << 32, 0xaa));
        assert_eq!(ptp.len(), 28);
        assert_eq!((ptp[0], ptp[1]), (0x80, 0xd7));
        assert_eq!(u64::from_be_bytes(ptp[8..16].try_into().unwrap()), 3_000_000_000);
        assert_eq!(u64::from_be_bytes(ptp[20..28].try_into().unwrap()), 0xaa);
    }

    /// Sound pushed in pieces goes as whole frames, numbered in turn, its
    /// RTP time counting samples, with a sync first; a packet asked for
    /// again is sent as it was.
    #[test]
    fn pushed_in_pieces_sent_in_frames() {
        let (theirs, their_control, ours, our_control) =
            (UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap());
        for s in [&theirs, &their_control] {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }
        let key = [3u8; 32];
        let mut sound = Sound::new(ours, theirs.local_addr().unwrap(), our_control, their_control.local_addr().unwrap(), &key, 3748);
        let samples: Vec<i16> = (0..1000i16).collect();
        let at = Instant::now();
        sound.push(&samples[..300], at, |_| Some(Now::Ntp(5 << 32))).unwrap();
        sound.push(&samples[300..], at + Duration::from_micros(150 * 1_000_000 / 44_100), |_| Some(Now::Ntp(5 << 32))).unwrap();
        let aead = ChaCha20Poly1305::new(&key.into());
        let mut buf = [0u8; 2048];
        let (n, _) = their_control.recv_from(&mut buf).unwrap();
        assert_eq!((n, buf[0], buf[1]), (20, 0x90, 0xd4));
        let first_rtp = u32::from_be_bytes(buf[4..8].try_into().unwrap()).wrapping_add(3748);
        // 1000 samples: one frame of 704, 296 waiting.
        let (n, _) = theirs.recv_from(&mut buf).unwrap();
        let (seq, rtp, frame) = unpacket(&aead, &buf[..n]).unwrap();
        assert_eq!((rtp, unalac(&frame).unwrap()), (first_rtp, samples[..704].to_vec()));
        sound.push(&[0; 500], at + Duration::from_micros(500 * 1_000_000 / 44_100), |_| None).unwrap();
        let (n, _) = theirs.recv_from(&mut buf).unwrap();
        let (seq2, rtp2, _) = unpacket(&aead, &buf[..n]).unwrap();
        assert_eq!((seq2, rtp2), (seq.wrapping_add(1), first_rtp.wrapping_add(352)));
        // Where it stands, by the samples captured: a second on, a second
        // of samples on (the captures came when their count said).
        assert_eq!(sound.rtp_at(at + Duration::from_secs(1)), Some(first_rtp.wrapping_add(44_100)));
        // Asked again for both and one it never sent.
        let mut ask = vec![0x80, 0xd5, 0, 1];
        ask.extend(seq.to_be_bytes());
        ask.extend(3u16.to_be_bytes());
        let again = sound.again(&ask);
        assert_eq!(again.len(), 2);
        assert_eq!(&again[0][..4], &[0x80, 0xd6, 0, 1]);
        assert_eq!(unpacket(&aead, &again[1][4..]).map(|(s, ..)| s), Some(seq2));
    }
}
