//! The clock and time zone, through systemd-timedated
//! (`org.freedesktop.timedate1`). Changing them goes through polkit, which
//! asks for an administrator's password (`interactive`).

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.timedate1";
const PATH: &str = "/org/freedesktop/timedate1";
const IFACE: &str = "org.freedesktop.timedate1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeStatus {
    /// "Europe/London".
    pub timezone: String,
    /// Network time is on.
    pub ntp: bool,
    pub ntp_synchronized: bool,
    /// Network time can be turned on (a time sync service is installed).
    pub can_ntp: bool,
    /// The hardware clock keeps local time rather than UTC.
    pub local_rtc: bool,
}

pub fn status() -> Result<TimeStatus> {
    let c = bus::system()?;
    let get = |name| bus::property::<bool>(c, DEST, PATH, IFACE, name);
    Ok(TimeStatus {
        timezone: bus::property::<String>(c, DEST, PATH, IFACE, "Timezone")?,
        ntp: get("NTP")?,
        ntp_synchronized: get("NTPSynchronized")?,
        can_ntp: get("CanNTP")?,
        local_rtc: get("LocalRTC")?,
    })
}

/// Every zone timedated accepts, sorted ("Africa/Abidjan", …).
pub fn timezones() -> Result<Vec<String>> {
    bus::call(bus::system()?, DEST, PATH, IFACE, "ListTimezones", &())
}

/// Set the clock to `usec` microseconds since the epoch (UTC). Fails while
/// network time is on.
pub fn set_time(usec: i64) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, PATH, IFACE, "SetTime", &(usec, false, true))
}

pub fn set_timezone(zone: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, PATH, IFACE, "SetTimezone", &(zone, true))
}

pub fn set_ntp(on: bool) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, PATH, IFACE, "SetNTP", &(on, true))
}

/// Whether `e` is polkit refusing for want of a password (no
/// authentication agent answered, or the user cancelled).
pub fn needs_auth(e: &crate::Error) -> bool {
    let s = e.to_string();
    s.contains("InteractiveAuthorizationRequired") || s.contains("NotAuthorized") || s.contains("AccessDenied")
}
