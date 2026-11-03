//! The user accounts' service (AccountsService, on the system bus): what
//! it changes asks polkit, which lets root.

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.Accounts";

/// Make `shell` `user`'s login shell.
pub fn set_shell(user: &str, shell: &str) -> Result<()> {
    let conn = bus::system()?;
    let path: zvariant::OwnedObjectPath = bus::call(conn, DEST, "/org/freedesktop/Accounts", "org.freedesktop.Accounts", "FindUserByName", &(user,))?;
    bus::call::<_, ()>(conn, DEST, path.as_str(), "org.freedesktop.Accounts.User", "SetShell", &(shell,))
}
