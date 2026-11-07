//! Plymouth's boot splash handed to the login screen as GDM hands it:
//! told to let go of the screen (deactivate) before the login screen's
//! session takes it, then, once the login screen is up, to quit leaving
//! its last picture there (so the computer goes from its boot splash
//! straight to the login screen, no console between); told to quit
//! outright when the login screen doesn't come (its console back, to say
//! why). Plymouth's own protocol on its socket (`ply-boot-protocol.h`):
//! a request is its letter, its argument (`\x02`, the argument's length
//! with its NUL, the argument) and a NUL; the answer one byte, ACK or NAK.

use std::{
    io::{Read, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixStream},
    },
    time::Duration,
};

/// plymouthd's socket (abstract, its name not padded).
const SOCKET: &[u8] = b"/org/freedesktop/plymouthd";
const ACK: u8 = 0x06;
/// The longest an answer is waited for (deactivating waits for its
/// drawing to stop).
const WAIT: Duration = Duration::from_secs(10);

/// Where Plymouth listens.
pub struct Splash {
    name: Vec<u8>,
}

impl Splash {
    /// The system's plymouthd.
    pub fn system() -> Splash {
        Splash { name: SOCKET.to_vec() }
    }

    /// One listening on the abstract socket `name` (tests).
    #[cfg(test)]
    fn at(name: &[u8]) -> Splash {
        Splash { name: name.to_vec() }
    }

    /// `request` sent; whether Plymouth said yes (`None`: it isn't there).
    fn ask(&self, request: &[u8]) -> Option<bool> {
        let addr = SocketAddr::from_abstract_name(&self.name).ok()?;
        let mut s = UnixStream::connect_addr(&addr).ok()?;
        s.set_read_timeout(Some(WAIT)).ok()?;
        s.set_write_timeout(Some(WAIT)).ok()?;
        s.write_all(request).ok()?;
        let mut answer = [0u8; 1];
        // (Quitting, it may go before it answers.)
        Some(s.read_exact(&mut answer).map_or(request[0] == b'Q', |()| answer[0] == ACK))
    }

    /// Whether it's showing.
    pub fn running(&self) -> bool {
        self.ask(b"P\0") == Some(true)
    }

    /// It lets go of the screen and the keyboard (its picture stays).
    pub fn deactivate(&self) -> bool {
        self.ask(b"D\0") == Some(true)
    }

    /// It quits, leaving its picture on the screen (`retain`) for what
    /// takes the screen next, or giving the console back.
    pub fn quit(&self, retain: bool) -> bool {
        self.ask(if retain { b"Q\x02\x02\x01\0" } else { b"Q\x02\x01\0" }) == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A plymouthd of our own that says yes, and what it was asked.
    fn fake(name: &[u8], requests: usize) -> std::thread::JoinHandle<Vec<Vec<u8>>> {
        let listener = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name).unwrap()).unwrap();
        std::thread::spawn(move || {
            let mut heard = Vec::new();
            for _ in 0..requests {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 16];
                let n = s.read(&mut buf).unwrap();
                heard.push(buf[..n].to_vec());
                s.write_all(&[ACK]).unwrap();
            }
            heard
        })
    }

    #[test]
    fn plymouth_hears_its_own_protocol() {
        let name = format!("/olo/plymouth-test-{}", std::process::id());
        let splash = Splash::at(name.as_bytes());
        // Nobody there: no.
        assert!(!splash.running());
        let server = fake(name.as_bytes(), 4);
        assert!(splash.running());
        assert!(splash.deactivate());
        assert!(splash.quit(true));
        assert!(splash.quit(false));
        let heard = server.join().unwrap();
        assert_eq!(heard, [b"P\0".to_vec(), b"D\0".to_vec(), b"Q\x02\x02\x01\0".to_vec(), b"Q\x02\x01\0".to_vec()]);
    }
}
