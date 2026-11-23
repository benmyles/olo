//! What Rex sends a host over SSH, left as it is so that it can be
//! packed ([`PlainMask`]).
//!
//! A WebSocket client hides each frame it sends under four bytes of
//! chance (its mask), so that a page's script can't write bytes of its
//! choosing onto the wire for a proxy between a browser and a server to
//! misread. Between Rex and a host there is no such proxy: the frames
//! travel inside the SSH connection, which hides them itself. But bytes
//! under chance don't pack, and SSH's compression, which makes what a
//! far host sends a third to a tenth of its size, left what Rex sends
//! as large as it was (a letter typed is 320 bytes on the wire, most of
//! them the same as the letter's before: measured 2026-11-23, HACKING,
//! Rex, "Fewer bytes"). So over SSH each frame goes with a mask of
//! nothing, which hides nothing: still a masked frame, as a client's
//! must be, and what it says the same.
//!
//! The WebSocket library has no say in its mask, so this stands behind
//! it and takes the mask off what it writes: it knows where the upgrade
//! request ends and where each frame's header and payload are, keeps
//! nothing but a header not yet whole, and changes no length.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Where in what is written the next byte stands.
enum At {
    /// In the upgrade's request: so many bytes of the blank line that
    /// ends it have passed.
    Head(u8),
    /// In a frame's header, of which this much has come.
    Header(Vec<u8>),
    /// In a frame's payload: so many bytes left of it, its mask, and
    /// how far into the mask the next byte is.
    Payload { left: u64, mask: [u8; 4], at: usize },
}

/// A stream to a host, with the WebSocket frames written to it given a
/// mask of nothing (the module's words say why). What is read from it
/// is as it comes.
pub struct PlainMask<S> {
    inner: S,
    at: At,
    /// What was taken and isn't written yet, and how much of it is.
    out: Vec<u8>,
    written: usize,
}

impl<S> PlainMask<S> {
    pub fn new(inner: S) -> PlainMask<S> {
        PlainMask { inner, at: At::Head(0), out: Vec::new(), written: 0 }
    }

    /// More of what is written: into `out`, its frames' masks taken off.
    fn take(&mut self, mut bytes: &[u8]) {
        self.out.reserve(bytes.len());
        while !bytes.is_empty() {
            match &mut self.at {
                At::Head(ended) => {
                    // (Up to the blank line that ends the request.)
                    let mut n = 0;
                    while n < bytes.len() && *ended < 4 {
                        *ended = match (bytes[n], *ended) {
                            (b'\r', 0 | 2) | (b'\n', 1 | 3) => *ended + 1,
                            (b'\r', _) => 1,
                            _ => 0,
                        };
                        n += 1;
                    }
                    self.out.extend_from_slice(&bytes[..n]);
                    bytes = &bytes[n..];
                    if *ended == 4 {
                        self.at = At::Header(Vec::with_capacity(14));
                    }
                }
                At::Header(header) => {
                    // Two bytes, a longer length where the second says
                    // so, and the mask where its bit is set.
                    let whole = |header: &[u8]| {
                        header.get(1).map(|second| {
                            2 + match second & 0x7f {
                                126 => 2,
                                127 => 8,
                                _ => 0,
                            } + if second & 0x80 != 0 { 4 } else { 0 }
                        })
                    };
                    let want = whole(header).unwrap_or(2);
                    let n = (want - header.len()).min(bytes.len());
                    header.extend_from_slice(&bytes[..n]);
                    bytes = &bytes[n..];
                    if whole(header) != Some(header.len()) {
                        continue;
                    }
                    let second = header[1];
                    let left = match second & 0x7f {
                        126 => u64::from(u16::from_be_bytes([header[2], header[3]])),
                        127 => u64::from_be_bytes(header[2..10].try_into().unwrap()),
                        n => u64::from(n),
                    };
                    let mut mask = [0; 4];
                    if second & 0x80 != 0 {
                        let key = header.len() - 4;
                        mask.copy_from_slice(&header[key..]);
                        header[key..].fill(0);
                    }
                    self.out.extend_from_slice(header);
                    self.at = if left == 0 { At::Header(Vec::with_capacity(14)) } else { At::Payload { left, mask, at: 0 } };
                }
                At::Payload { left, mask, at } => {
                    let n = (*left).min(bytes.len() as u64) as usize;
                    if *mask == [0; 4] {
                        self.out.extend_from_slice(&bytes[..n]);
                    } else {
                        self.out.extend(bytes[..n].iter().enumerate().map(|(i, byte)| byte ^ mask[(*at + i) % 4]));
                    }
                    (*left, *at) = (*left - n as u64, (*at + n) % 4);
                    bytes = &bytes[n..];
                    if *left == 0 {
                        self.at = At::Header(Vec::with_capacity(14));
                    }
                }
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> PlainMask<S> {
    /// What was taken is written, as far as the stream takes it now.
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.written < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.written += n;
        }
        self.out.clear();
        self.written = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PlainMask<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PlainMask<S> {
    /// All of `buf` is taken once what was taken before is written
    /// (never more kept than one write's worth); what the stream doesn't
    /// take of it now goes with the next write or the flush, as with
    /// any writer that keeps a buffer.
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        this.take(buf);
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use olo_rex_link::{Piece, Scanner};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, below: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % below.max(1)
        }
    }

    /// A frame as a client writes it: under a mask of chance.
    fn frame(first: u8, payload: &[u8], rng: &mut Lcg) -> Vec<u8> {
        let mut out = vec![first];
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n < 65536 => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask: Vec<u8> = (0..4).map(|_| 1 + rng.next(255) as u8).collect();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, byte)| byte ^ mask[i % 4]));
        out
    }

    /// Whatever a client writes, cut anywhere: the request passes as it
    /// is, every frame comes out as long as it went in, still masked,
    /// its mask nothing and its payload therefore what it says; frames
    /// of every length, with nothing in them, and of other kinds than
    /// text among them.
    #[test]
    fn every_frame_goes_out_under_a_mask_of_nothing_however_it_is_cut() {
        let mut rng = Lcg(0x5eed);
        for round in 0..200 {
            let head = b"GET /api/control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\r\r\n\r\n".to_vec();
            let (mut written, mut said) = (head.clone(), Vec::new());
            for _ in 0..(1 + rng.next(12)) {
                let len = match rng.next(6) {
                    0 => 0,
                    1 => 125 + rng.next(3),
                    2 => 65_534 + rng.next(4),
                    _ => rng.next(400),
                };
                let payload: Vec<u8> = (0..len).map(|_| b' ' + rng.next(90) as u8).collect();
                let first = [0x81, 0x82, 0x89, 0x8a, 0x01, 0x80][rng.next(6) as usize];
                written.extend(frame(first, &payload, &mut rng));
                said.push((first, payload));
            }
            let mut plain = PlainMask::new(());
            let mut at = 0;
            while at < written.len() {
                let n = (1 + rng.next(if round % 2 == 0 { 9 } else { 70_000 }) as usize).min(written.len() - at);
                plain.take(&written[at..at + n]);
                at += n;
            }
            assert_eq!(plain.out.len(), written.len(), "no length changes");
            assert_eq!(&plain.out[..head.len()], &head[..], "the request is as it was");
            // (Cut into its frames by the helper's own reader of them.)
            let mut scanner = Scanner::new();
            let frames: Vec<Vec<u8>> = scanner
                .push(&plain.out)
                .into_iter()
                .filter_map(|piece| match piece {
                    Piece::Frame { bytes, .. } => Some(bytes),
                    _ => None,
                })
                .collect();
            assert_eq!(frames.len(), said.len());
            for (bytes, (first, payload)) in frames.iter().zip(&said) {
                let header = bytes.len() - payload.len();
                assert_eq!(bytes[0], *first);
                assert!(bytes[1] & 0x80 != 0, "still a masked frame");
                assert_eq!(&bytes[header - 4..header], [0; 4], "its mask is nothing");
                assert_eq!(&bytes[header..], &payload[..], "and what it says stands as it is");
            }
        }
    }

    /// A reader that takes its time doesn't lose or reorder a byte: what
    /// the stream doesn't take at once is kept and goes with the next
    /// write or the flush.
    #[tokio::test]
    async fn what_a_slow_stream_does_not_take_at_once_is_kept_and_sent_in_order() {
        let mut rng = Lcg(7);
        // (A pipe that holds 64 bytes: nearly every write is taken in parts.)
        let (near, mut far) = tokio::io::duplex(64);
        let mut plain = PlainMask::new(near);
        let mut written = b"GET / HTTP/1.1\r\n\r\n".to_vec();
        let mut expected = written.clone();
        for _ in 0..40 {
            let payload: Vec<u8> = (0..rng.next(900)).map(|_| b'a' + rng.next(26) as u8).collect();
            written.extend(frame(0x81, &payload, &mut rng));
            let mut zeroed = frame(0x81, &payload, &mut Lcg(1));
            let header = zeroed.len() - payload.len();
            zeroed[header - 4..header].fill(0);
            zeroed[header..].copy_from_slice(&payload);
            expected.extend(zeroed);
        }
        let reader = tokio::spawn(async move {
            let mut got = Vec::new();
            let mut buf = [0u8; 37];
            loop {
                tokio::time::sleep(std::time::Duration::from_micros(200)).await;
                match far.read(&mut buf).await {
                    Ok(0) | Err(_) => return got,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
        });
        for chunk in written.chunks(333) {
            plain.write_all(chunk).await.unwrap();
        }
        plain.flush().await.unwrap();
        plain.shutdown().await.unwrap();
        drop(plain);
        assert_eq!(reader.await.unwrap(), expected);
    }

    /// A WebSocket server takes what comes through it as it takes any
    /// client's: the library's own client on one side, its own server
    /// on the other, messages of both kinds and both ways.
    #[tokio::test]
    async fn a_websocket_server_takes_frames_under_a_mask_of_nothing() {
        let (near, far) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut ws = tokio_tungstenite::accept_async(far).await.unwrap();
            let mut got = Vec::new();
            while let Some(Ok(message)) = ws.next().await {
                if message.is_close() {
                    break;
                }
                ws.send(message.clone()).await.unwrap();
                got.push(message);
            }
            got
        });
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/api/control", PlainMask::new(near)).await.unwrap();
        use tokio_tungstenite::tungstenite::Message;
        let sent = [Message::text(r#"{"type":"request","id":"1"}"#), Message::binary(vec![0u8, 1, 2, 255]), Message::text("x".repeat(70_000))];
        for message in &sent {
            ws.send(message.clone()).await.unwrap();
            assert_eq!(&ws.next().await.unwrap().unwrap(), message, "and what comes back is read as it comes");
        }
        ws.close(None).await.unwrap();
        assert_eq!(server.await.unwrap(), sent);
    }
}
