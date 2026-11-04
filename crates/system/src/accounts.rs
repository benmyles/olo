//! The user accounts' service (AccountsService, on the system bus): what
//! it changes asks polkit, which lets root. And this user's login shell
//! as the password database has it.

use std::path::PathBuf;

use crate::{Result, bus};

/// Where Olo Suggestions puts Olo's build of brush (and makes it the
/// login shell).
pub const BRUSH: &str = "/usr/local/bin/brush";

const DEST: &str = "org.freedesktop.Accounts";

/// This user's name and login shell (the password database's, as it is
/// now: one changed since login included). May ask NSS (SSSD, LDAP):
/// off the UI thread.
pub fn login_shell() -> Option<(String, PathBuf)> {
    use std::ffi::CStr;
    // SAFETY: getuid has no preconditions; getpwuid_r writes into the
    // buffer it's given, and what it returns points into it.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0u8; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        let name = CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned();
        let shell = PathBuf::from(CStr::from_ptr(pw.pw_shell).to_string_lossy().into_owned());
        Some((name, shell))
    }
}

/// Make `shell` `user`'s login shell.
pub fn set_shell(user: &str, shell: &str) -> Result<()> {
    let conn = bus::system()?;
    let path: zvariant::OwnedObjectPath = bus::call(conn, DEST, "/org/freedesktop/Accounts", "org.freedesktop.Accounts", "FindUserByName", &(user,))?;
    bus::call::<_, ()>(conn, DEST, path.as_str(), "org.freedesktop.Accounts.User", "SetShell", &(shell,))
}
