//! File descriptors: one kept in reserve, for the day none is free
//! (app_server's and the Wayland proxy's listening sockets).
//!
//! A listening socket with a connection waiting is readable until the
//! connection is taken, and it can't be taken with no descriptor free
//! (`accept` fails with EMFILE): the loop wakes at once, again and
//! again, saying so each time (a test app_server held to 67 files wrote
//! "accept failed" 171,619 times in a second and a half, and went down:
//! 2026-11-18). With one descriptor held back, the connection is taken
//! with it and closed (its program sees the end, and tries again or
//! says so), and the reserve is taken back. Every Olo program raises
//! its limit as it starts (`olo_crash::raise_open_files`), so this is
//! for what should no longer happen.

use std::{
    fs::File,
    io,
    time::{Duration, Instant},
};

pub struct Reserve {
    spare: Option<File>,
    said: Option<Instant>,
}

impl Default for Reserve {
    fn default() -> Self {
        Reserve { spare: File::open("/dev/null").ok(), said: None }
    }
}

impl Reserve {
    /// `accept` failed with `error`: if for want of a descriptor, the
    /// waiting connection is taken with the reserve (`take`: the
    /// listener's accept) and let go, and true is answered: there may be
    /// more waiting. False: another error, or nothing could be done; the
    /// caller stops for this turn.
    pub fn refuse<T>(&mut self, error: &io::Error, take: impl FnOnce() -> io::Result<T>) -> bool {
        if !matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) {
            return false;
        }
        // (Said once a second at most.)
        if self.said.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            self.said = Some(Instant::now());
            tracing::error!("no file descriptor is free: a program's connection is refused (the limit on open files: /proc/self/limits)");
        }
        let had = self.spare.take().is_some();
        let taken = take().is_ok();
        self.spare = File::open("/dev/null").ok();
        had && taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a want of descriptors is its business; then the connection
    /// is taken and dropped, and it's ready for the next.
    #[test]
    fn refuses_only_for_want_of_a_descriptor() {
        let mut reserve = Reserve::default();
        let mut asked = 0;
        assert!(!reserve.refuse(&io::Error::from_raw_os_error(libc::ECONNABORTED), || {
            asked += 1;
            Ok(())
        }));
        assert_eq!(asked, 0);
        for _ in 0..3 {
            assert!(reserve.refuse(&io::Error::from_raw_os_error(libc::EMFILE), || {
                asked += 1;
                Ok(())
            }));
            assert!(reserve.spare.is_some());
        }
        assert_eq!(asked, 3);
        // The connection couldn't be taken even so: no spinning on it.
        assert!(!reserve.refuse(&io::Error::from_raw_os_error(libc::ENFILE), || Err::<(), _>(io::Error::from_raw_os_error(libc::ENFILE))));
    }
}
