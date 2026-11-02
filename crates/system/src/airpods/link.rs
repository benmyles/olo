//! The L2CAP channel to a pair of AirPods (a sequenced-packet socket, each
//! read one whole message), opened once BlueZ has connected them.

use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::Duration,
};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_L2CAP: libc::c_int = 0;
const SOL_BLUETOOTH: libc::c_int = 274;
const BT_SECURITY: libc::c_int = 4;
const BT_SECURITY_MEDIUM: u8 = 2;
/// A classic (BR/EDR) address.
const BDADDR_BREDR: u8 = 0;

#[repr(C)]
struct SockaddrL2 {
    family: libc::sa_family_t,
    psm: u16,
    bdaddr: [u8; 6],
    cid: u16,
    bdaddr_type: u8,
}

#[repr(C)]
struct BtSecurity {
    level: u8,
    key_size: u8,
}

/// "AA:BB:CC:DD:EE:FF" as the kernel takes it (least significant first).
fn bdaddr(addr: &str) -> io::Result<[u8; 6]> {
    let bytes: Vec<u8> = addr.split(':').map(|b| u8::from_str_radix(b, 16)).collect::<Result<_, _>>().map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut out: [u8; 6] = bytes.try_into().map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    out.reverse();
    Ok(out)
}

/// An open channel. Reading blocks until a message comes; [`Link::close`]
/// (from any thread) ends a read waiting.
pub struct Link {
    fd: OwnedFd,
}

impl Link {
    /// Open channel `psm` to the device at `addr` (already connected by
    /// BlueZ: this rides on its link, which is encrypted). Gives up after
    /// `timeout`.
    pub fn connect(addr: &str, psm: u16, timeout: Duration) -> io::Result<Link> {
        let to = SockaddrL2 { family: AF_BLUETOOTH as libc::sa_family_t, psm: psm.to_le(), bdaddr: bdaddr(addr)?, cid: 0, bdaddr_type: BDADDR_BREDR };
        // SAFETY: plain socket calls on a descriptor we own; the structs
        // are the kernel's layout.
        unsafe {
            let fd = libc::socket(AF_BLUETOOTH, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, BTPROTO_L2CAP);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let sec = BtSecurity { level: BT_SECURITY_MEDIUM, key_size: 0 };
            libc::setsockopt(fd.as_raw_fd(), SOL_BLUETOOTH, BT_SECURITY, (&sec as *const BtSecurity).cast(), size_of::<BtSecurity>() as libc::socklen_t);
            let tv = libc::timeval { tv_sec: timeout.as_secs() as libc::time_t, tv_usec: timeout.subsec_micros() as libc::suseconds_t };
            libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDTIMEO, (&tv as *const libc::timeval).cast(), size_of::<libc::timeval>() as libc::socklen_t);
            if libc::connect(fd.as_raw_fd(), (&to as *const SockaddrL2).cast(), size_of::<SockaddrL2>() as libc::socklen_t) < 0 {
                return Err(io::Error::last_os_error());
            }
            let none = libc::timeval { tv_sec: 0, tv_usec: 0 };
            libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDTIMEO, (&none as *const libc::timeval).cast(), size_of::<libc::timeval>() as libc::socklen_t);
            Ok(Link { fd })
        }
    }

    pub fn send(&self, message: &[u8]) -> io::Result<()> {
        // SAFETY: writing from a slice we hold.
        let n = unsafe { libc::send(self.fd.as_raw_fd(), message.as_ptr().cast(), message.len(), libc::MSG_NOSIGNAL) };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// The next message (empty when the channel has closed).
    pub fn recv(&self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; 4096];
        loop {
            // SAFETY: reading into a buffer we own, at most its length.
            let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n >= 0 {
                buf.truncate(n as usize);
                return Ok(buf);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// End the channel; a read waiting on it returns.
    pub fn close(&self) {
        // SAFETY: shutting down our own descriptor (it stays open until drop).
        unsafe { libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn addresses_go_least_significant_first() {
        assert_eq!(super::bdaddr("02:0A:0B:0C:0D:0E").unwrap(), [0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x02]);
        assert!(super::bdaddr("nope").is_err());
    }
}
