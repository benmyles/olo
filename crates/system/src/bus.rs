//! Small helpers over zbus's blocking connection.

use std::sync::OnceLock;

use zbus::blocking::Connection;
use zvariant::{DynamicType, OwnedValue};

use crate::Result;

/// The system bus, connected once and shared.
pub fn system() -> Result<&'static Connection> {
    static SYSTEM: OnceLock<Connection> = OnceLock::new();
    if let Some(c) = SYSTEM.get() {
        return Ok(c);
    }
    let c = Connection::system()?;
    Ok(SYSTEM.get_or_init(|| c))
}

/// The session bus, connected once and shared.
pub fn session() -> Result<&'static Connection> {
    static SESSION: OnceLock<Connection> = OnceLock::new();
    if let Some(c) = SESSION.get() {
        return Ok(c);
    }
    let c = Connection::session()?;
    Ok(SESSION.get_or_init(|| c))
}

/// Call `method` and deserialize the reply.
pub fn call<B, R>(conn: &Connection, dest: &str, path: &str, iface: &str, method: &str, body: &B) -> Result<R>
where
    B: serde::Serialize + DynamicType,
    R: for<'d> serde::Deserialize<'d> + zvariant::Type,
{
    let reply = conn.call_method(Some(dest), path, Some(iface), method, body)?;
    reply.body().deserialize()
}

/// Read property `name` of `iface` on `path`.
pub fn property<T>(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str) -> Result<T>
where
    T: TryFrom<OwnedValue>,
    <T as TryFrom<OwnedValue>>::Error: Into<zbus::Error>,
{
    let v: OwnedValue = call(conn, dest, path, "org.freedesktop.DBus.Properties", "Get", &(iface, name))?;
    T::try_from(v).map_err(Into::into)
}

/// Set property `name` of `iface` on `path`.
pub fn set_property<'a>(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str, value: impl Into<zvariant::Value<'a>>) -> Result<()> {
    let v: zvariant::Value = value.into();
    call::<_, ()>(conn, dest, path, "org.freedesktop.DBus.Properties", "Set", &(iface, name, v))
}
