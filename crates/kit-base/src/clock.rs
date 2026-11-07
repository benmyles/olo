//! The local time, as the C library knows the zone (no tz database of
//! our own), and as people read it: "9:41" and "Friday, November 6", as
//! the lock screen and the login screen show it.

/// A moment, local.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    /// 0–11.
    pub month: i32,
    /// 1–31.
    pub day: i32,
    /// 0 (Sunday)–6.
    pub weekday: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
}

#[repr(C)]
struct CTm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const std::ffi::c_char,
}

unsafe extern "C" {
    fn localtime_r(t: *const i64, out: *mut CTm) -> *mut CTm;
}

/// `secs` since the epoch, local.
pub fn local(secs: i64) -> Local {
    let mut c = CTm { tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70, tm_wday: 4, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: std::ptr::null() };
    // SAFETY: both pointers valid for the call; localtime_r is reentrant.
    if unsafe { localtime_r(&secs, &mut c) }.is_null() {
        return Local::default();
    }
    Local { year: c.tm_year + 1900, month: c.tm_mon, day: c.tm_mday, weekday: c.tm_wday, hour: c.tm_hour, minute: c.tm_min, second: c.tm_sec }
}

/// Now, local.
pub fn now() -> Local {
    local(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64))
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

impl Local {
    /// "9:41" (twelve hours).
    pub fn time(&self) -> String {
        let h = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h}:{:02}", self.minute)
    }

    /// "Friday, November 6".
    pub fn date(&self) -> String {
        format!("{}, {} {}", DAYS[self.weekday.clamp(0, 6) as usize], MONTHS[self.month.clamp(0, 11) as usize], self.day)
    }
}

/// How long until the next minute begins (a clock's tick).
pub fn to_next_minute() -> std::time::Duration {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    std::time::Duration::from_secs(60 - secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_people_read_it() {
        let l = Local { year: 2026, month: 10, day: 6, weekday: 5, hour: 0, minute: 7, second: 0 };
        assert_eq!((l.time(), l.date()), ("12:07".to_owned(), "Friday, November 6".to_owned()));
        assert_eq!(Local { hour: 21, minute: 41, ..l }.time(), "9:41");
        assert!(now().year >= 2026);
    }
}
