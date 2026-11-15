//! Words for what the daemon gives in numbers: dates, "an hour ago",
//! kinds of update, severities, who did something.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

/// The local offset from UTC now, in seconds (from the C library, which
/// reads the zone the computer is set to).
fn offset(t: i64) -> i64 {
    // SAFETY: localtime_r writes into the tm given; the time is valid.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt: libc::time_t = t as libc::time_t;
        if libc::localtime_r(&tt, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

/// Days since 1970-01-01 as (year, month 1–12, day 1–31).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "15 Nov 2026", in the computer's own time.
pub fn date(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    let local = t + offset(t);
    let (y, m, d) = civil(local.div_euclid(86_400));
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// "15 Nov 2026, 14:05".
pub fn date_time(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    let local = t + offset(t);
    let secs = local.rem_euclid(86_400);
    format!("{}, {:02}:{:02}", date(t), secs / 3600, secs % 3600 / 60)
}

/// "just now", "5 minutes ago", "3 hours ago", "on 15 Nov 2026".
pub fn ago(t: i64) -> String {
    let d = (now() - t).max(0);
    match d {
        0..60 => "just now".into(),
        60..120 => "a minute ago".into(),
        120..3600 => format!("{} minutes ago", d / 60),
        3600..7200 => "an hour ago".into(),
        7200..86_400 => format!("{} hours ago", d / 3600),
        _ => format!("on {}", date(t)),
    }
}

/// An advisory's kind as people say it.
pub fn kind(kind: &str) -> &'static str {
    match kind {
        "security" => "Security",
        "bugfix" => "Bug Fix",
        "enhancement" => "Enhancement",
        "newpackage" => "New Package",
        _ => "Other",
    }
}

/// How pressing a severity is (0 most).
pub fn severity_rank(s: &str) -> u8 {
    match s.to_ascii_lowercase().as_str() {
        "critical" => 0,
        "important" => 1,
        "moderate" => 2,
        "low" => 3,
        _ => 4,
    }
}

/// A severity worth saying ("" for none).
pub fn severity(s: &str) -> &str {
    if s.is_empty() || s.eq_ignore_ascii_case("none") { "" } else { s }
}

/// Who a user id is ("root", "ben"; the number when unknown).
pub fn user(uid: i64) -> String {
    if uid < 0 {
        return String::new();
    }
    // SAFETY: getpwuid_r writes into the buffers given.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 1024];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(uid as libc::uid_t, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null() && !pw.pw_name.is_null() {
            return std::ffi::CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned();
        }
    }
    format!("user {uid}")
}

/// "1 package", "3 packages".
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    #[test]
    fn days_are_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(20_407), (2025, 11, 15));
        assert_eq!(super::civil(-1), (1969, 12, 31));
        assert_eq!(super::count(1, "package", "packages"), "1 package");
    }
}
