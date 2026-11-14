//! What follows pair-verify on a connection is enciphered: frames of at
//! most 1024 bytes, each its length (two bytes, the smaller first, also
//! what the cipher vouches for), the ChaCha20-Poly1305 text and its
//! 16-byte tag; each direction has its key and counts its frames (the
//! count is the nonce).

use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead as _, KeyInit as _, Payload},
};

const FRAME: usize = 1024;
const TAG: usize = 16;

pub struct Cipher {
    out: ChaCha20Poly1305,
    back: ChaCha20Poly1305,
    sent: u64,
    taken: u64,
    /// What has come and isn't a whole frame yet.
    waiting: Vec<u8>,
}

fn nonce(count: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&count.to_le_bytes());
    Nonce::from(n)
}

impl Cipher {
    /// `out_key`: for what this side sends; `in_key`: for what comes.
    pub fn new(out_key: &[u8; 32], in_key: &[u8; 32]) -> Cipher {
        Cipher { out: ChaCha20Poly1305::new(out_key.into()), back: ChaCha20Poly1305::new(in_key.into()), sent: 0, taken: 0, waiting: Vec::new() }
    }

    /// `data`, enciphered for sending.
    pub fn seal(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + (data.len() / FRAME + 1) * (2 + TAG));
        for frame in data.chunks(FRAME) {
            let len = (frame.len() as u16).to_le_bytes();
            let sealed = self.out.encrypt(&nonce(self.sent), Payload { msg: frame, aad: &len }).expect("sealing a frame");
            self.sent += 1;
            out.extend(len);
            out.extend(sealed);
        }
        out
    }

    /// What `came` says, with what came before it: the frames that are
    /// whole by now, deciphered (the rest waits for more). Err: a frame
    /// that isn't the other side's (the connection is no good).
    pub fn open(&mut self, came: &[u8]) -> Result<Vec<u8>, NotTheirs> {
        self.waiting.extend(came);
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(len) = self.waiting.get(at..at + 2) {
            let size = u16::from_le_bytes([len[0], len[1]]) as usize + TAG;
            let Some(frame) = self.waiting.get(at + 2..at + 2 + size) else { break };
            out.extend(self.back.decrypt(&nonce(self.taken), Payload { msg: frame, aad: len }).map_err(|_| NotTheirs)?);
            self.taken += 1;
            at += 2 + size;
        }
        self.waiting.drain(..at);
        Ok(out)
    }
}

/// A frame that the other side's key didn't seal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotTheirs;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_and_come_whole_or_in_pieces() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let (mut here, mut there) = (Cipher::new(&a, &b), Cipher::new(&b, &a));
        let long: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let sealed = here.seal(&long);
        // Three frames: 1024, 1024, 952, each with its length and tag.
        assert_eq!(sealed.len(), 3000 + 3 * 18);
        assert_eq!(&sealed[..2], &[0, 4]);
        // Come a few bytes at a time.
        let mut got = Vec::new();
        for piece in sealed.chunks(7) {
            got.extend(there.open(piece).unwrap());
        }
        assert_eq!(got, long);
        // And back, with the other key and its own count.
        let reply = there.seal(b"ok");
        assert_eq!(here.open(&reply).unwrap(), b"ok");
        // The next frame has the next nonce: the same text isn't the same bytes.
        assert_ne!(here.seal(b"again"), Cipher::new(&a, &b).seal(b"again"));
        // A frame tampered with is refused; and one out of turn.
        let (mut here, mut there) = (Cipher::new(&a, &b), Cipher::new(&b, &a));
        let mut bad = here.seal(b"hello");
        bad[5] ^= 1;
        assert!(there.open(&bad).is_err());
        let (mut here, mut there) = (Cipher::new(&a, &b), Cipher::new(&b, &a));
        let (_first, second) = (here.seal(b"one"), here.seal(b"two"));
        assert!(there.open(&second).is_err());
    }
}
