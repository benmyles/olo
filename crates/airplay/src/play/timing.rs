//! The time, for a receiver that asks it: AirPlay's SETUP names a port
//! where the sender answers timing questions (NTP's times in RTP's
//! dress), and a receiver may ask before it plays. A film played from
//! its address keeps its own time, so the answers only have to be there;
//! a mirrored screen's pictures are stamped on the same clock (the
//! computer's monotonic one, `mirror::clock`), so the receiver learns
//! from them how this computer's clock stands to its own. A receiver
//! that names a timing port of its own is asked the time too, from the
//! same socket ([`Timing::probe`]): its answers aren't answered.

use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

pub(crate) struct Timing {
    pub port: u16,
    at: SocketAddr,
    socket: Option<UdpSocket>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Timing {
    /// Answering on a port of this address until dropped. It sleeps
    /// until asked.
    pub fn start(ip: IpAddr) -> std::io::Result<Timing> {
        let socket = UdpSocket::bind((ip, 0))?;
        let at = socket.local_addr()?;
        let asking = socket.try_clone().ok();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new().name("airplay-timing".into()).spawn(move || {
            let mut buf = [0u8; 128];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(reply) = answer(&buf[..n], now()) {
                    let _ = socket.send_to(&reply, from);
                }
            }
        })?;
        Ok(Timing { port: at.port(), at, socket: asking, stop, thread: Some(thread) })
    }

    /// Ask the receiver's timing port the time, three times a tenth of a
    /// second apart (on a thread of its own: it's soon done).
    pub fn probe(&self, to: SocketAddr) {
        let Some(socket) = self.socket.as_ref().and_then(|s| s.try_clone().ok()) else { return };
        let _ = std::thread::Builder::new().name("airplay-probe".into()).spawn(move || {
            for seq in 1u16..=3 {
                let mut question = [0u8; 32];
                (question[0], question[1]) = (0x80, 0xd2);
                question[2..4].copy_from_slice(&seq.to_be_bytes());
                let (secs, frac) = now();
                question[24..28].copy_from_slice(&secs.to_be_bytes());
                question[28..32].copy_from_slice(&frac.to_be_bytes());
                let _ = socket.send_to(&question, to);
                if seq < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
    }
}

impl Drop for Timing {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // (Woken by a word from ourselves: it waits on nothing else.)
        if let Ok(waker) = UdpSocket::bind((self.at.ip(), 0)) {
            let _ = waker.send_to(&[], self.at);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The time as NTP has it: seconds and the fraction, on the computer's
/// monotonic clock with NTP's epoch added (see `mirror::clock`).
pub(crate) fn now() -> (u32, u32) {
    let t = crate::mirror::clock::ntp(crate::mirror::clock::Clock::new().now());
    ((t >> 32) as u32, t as u32)
}

/// The answer to a timing question: its sending time given back as the
/// reference, and `now` as when it came and when this left. None for
/// what isn't a question (an answer to ours).
pub(crate) fn answer(question: &[u8], now: (u32, u32)) -> Option<[u8; 32]> {
    let question: &[u8; 32] = question.try_into().ok()?;
    if question[1] & 0x7f != 0x52 {
        return None;
    }
    let mut reply = [0u8; 32];
    (reply[0], reply[1]) = (question[0], 0x53 | 0x80);
    reply[2..4].copy_from_slice(&7u16.to_be_bytes());
    reply[8..16].copy_from_slice(&question[24..32]);
    for at in [16, 24] {
        reply[at..at + 4].copy_from_slice(&now.0.to_be_bytes());
        reply[at + 4..at + 8].copy_from_slice(&now.1.to_be_bytes());
    }
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_answered_with_its_own_time_and_ours() {
        let mut question = [0u8; 32];
        (question[0], question[1]) = (0x80, 0xd2);
        question[24..32].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let reply = answer(&question, (0xaabb_ccdd, 0x1122_3344)).unwrap();
        assert_eq!(&reply[..4], &[0x80, 0xd3, 0, 7]);
        assert_eq!(&reply[8..16], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&reply[16..24], &[0xaa, 0xbb, 0xcc, 0xdd, 0x11, 0x22, 0x33, 0x44]);
        assert_eq!(&reply[24..32], &reply[16..24]);
        assert_eq!(answer(&question[..20], (0, 0)), None);
    }

    #[test]
    fn asked_over_the_loopback_and_gone_when_dropped() {
        let timing = Timing::start("127.0.0.1".parse().unwrap()).unwrap();
        let asker = UdpSocket::bind("127.0.0.1:0").unwrap();
        asker.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut question = [0u8; 32];
        (question[0], question[1], question[28]) = (0x80, 0xd2, 9);
        asker.send_to(&question, ("127.0.0.1", timing.port)).unwrap();
        // (An answer isn't answered.)
        let mut answer = question;
        answer[1] = 0xd3;
        assert_eq!(super::answer(&answer, (0, 0)), None);
        // (Rubbish is passed over.)
        asker.send_to(b"what time is it", ("127.0.0.1", timing.port)).unwrap();
        let mut reply = [0u8; 64];
        let (n, _) = asker.recv_from(&mut reply).unwrap();
        assert_eq!((n, reply[1], reply[12]), (32, 0xd3, 9));
        let secs = u32::from_be_bytes([reply[16], reply[17], reply[18], reply[19]]);
        assert!(secs.abs_diff(now().0) < 5);
        drop(timing);
    }
}
