//! The seat's terminals: the one the login screen and the sessions it
//! starts are on, made the one showing.

/// `VT_ACTIVATE` and `VT_WAITACTIVE` (linux/vt.h).
const VT_ACTIVATE: libc::c_ulong = 0x5606;
const VT_WAITACTIVE: libc::c_ulong = 0x5607;

/// Terminal `n` shown (and waited for).
pub fn activate(n: u32) {
    // SAFETY: /dev/tty0 opened and closed here; the ioctls take the
    // terminal's number.
    unsafe {
        let fd = libc::open(c"/dev/tty0".as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if fd < 0 {
            eprintln!("olo-login: /dev/tty0: {}", std::io::Error::last_os_error());
            return;
        }
        libc::ioctl(fd, VT_ACTIVATE, n as libc::c_ulong);
        libc::ioctl(fd, VT_WAITACTIVE, n as libc::c_ulong);
        libc::close(fd);
    }
}
