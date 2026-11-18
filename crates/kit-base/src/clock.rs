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

/// Why an [`Alarm`]'s wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woke {
    /// Its time came.
    Due,
    /// Someone woke it ([`AlarmWaker::wake`]): what it waits for may have
    /// changed.
    Woken,
    /// The clock was set (by hand, by the network): look at the time again.
    ClockSet,
}

/// A wait for a time on the wall's clock, for a program with a loop of
/// its own (a service that reminds of an event, that starts a job at an
/// hour): it ends when that time comes whatever happened meanwhile, the
/// computer asleep (a wait counted in elapsed time, `sleep` or a
/// condition's timeout, doesn't count the sleep: it ends that much
/// late) or the clock set (`Woke::ClockSet`); or when it's woken. The
/// kernel's own timer for the real-time clock (`timerfd`): no ticking,
/// nothing looked at meanwhile.
pub struct Alarm {
    timer: std::os::fd::OwnedFd,
    wake: std::sync::Arc<std::os::fd::OwnedFd>,
}

/// Wakes an [`Alarm`]'s wait from any thread.
#[derive(Clone)]
pub struct AlarmWaker(std::sync::Arc<std::os::fd::OwnedFd>);

impl AlarmWaker {
    pub fn wake(&self) {
        use std::os::fd::AsRawFd as _;
        let one = 1u64.to_ne_bytes();
        // SAFETY: eight bytes written to an eventfd of ours.
        unsafe { libc::write(self.0.as_raw_fd(), one.as_ptr().cast(), 8) };
    }
}

impl Alarm {
    pub fn new() -> std::io::Result<Alarm> {
        use std::os::fd::FromRawFd as _;
        // SAFETY: two descriptors made, each owned at once.
        unsafe {
            let timer = libc::timerfd_create(libc::CLOCK_REALTIME, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK);
            if timer < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let timer = std::os::fd::OwnedFd::from_raw_fd(timer);
            let wake = libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK);
            if wake < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Alarm { timer, wake: std::sync::Arc::new(std::os::fd::OwnedFd::from_raw_fd(wake)) })
        }
    }

    pub fn waker(&self) -> AlarmWaker {
        AlarmWaker(self.wake.clone())
    }

    /// Wait until `at` (seconds since 1970; None: until woken). A time
    /// already gone is due at once.
    pub fn wait(&self, at: Option<i64>) -> Woke {
        use std::os::fd::AsRawFd as _;
        let (timer, wake) = (self.timer.as_raw_fd(), self.wake.as_raw_fd());
        // (Armed for the time, or disarmed; told if the clock is set.)
        let spec = libc::itimerspec { it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 }, it_value: libc::timespec { tv_sec: at.map_or(0, |t| t.max(1)), tv_nsec: 0 } };
        // SAFETY: our timer set; a spec of ours read.
        unsafe { libc::timerfd_settime(timer, libc::TFD_TIMER_ABSTIME | libc::TFD_TIMER_CANCEL_ON_SET, &spec, std::ptr::null_mut()) };
        loop {
            let mut fds = [libc::pollfd { fd: timer, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 }];
            // SAFETY: two descriptors of ours polled, for as long as it takes.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Woke::Woken;
            }
            let mut count = [0u8; 8];
            if fds[1].revents & libc::POLLIN != 0 {
                // SAFETY: the eventfd's count read (and so cleared).
                unsafe { libc::read(wake, count.as_mut_ptr().cast(), 8) };
                return Woke::Woken;
            }
            if fds[0].revents != 0 {
                // SAFETY: the timer's count read.
                let got = unsafe { libc::read(timer, count.as_mut_ptr().cast(), 8) };
                if got < 0 {
                    return match std::io::Error::last_os_error().raw_os_error() {
                        Some(libc::ECANCELED) => Woke::ClockSet,
                        Some(libc::EAGAIN) => continue,
                        _ => Woke::Woken,
                    };
                }
                return Woke::Due;
            }
        }
    }
}

#[cfg(test)]
mod alarm_tests {
    use super::*;

    fn unix() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// Due when its time comes (one already gone, at once), woken from
    /// another thread before it, and with no time, only woken.
    #[test]
    fn due_at_its_time_or_woken() {
        let alarm = Alarm::new().unwrap();
        let t = std::time::Instant::now();
        assert_eq!(alarm.wait(Some(unix() - 5)), Woke::Due);
        assert!(t.elapsed() < std::time::Duration::from_millis(200));
        let t = std::time::Instant::now();
        assert_eq!(alarm.wait(Some(unix() + 2)), Woke::Due);
        assert!(t.elapsed() >= std::time::Duration::from_millis(900) && t.elapsed() < std::time::Duration::from_secs(4), "{:?}", t.elapsed());
        for at in [Some(unix() + 3600), None] {
            let waker = alarm.waker();
            let woke = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                waker.wake();
            });
            let t = std::time::Instant::now();
            assert_eq!(alarm.wait(at), Woke::Woken);
            assert!(t.elapsed() < std::time::Duration::from_secs(2));
            woke.join().unwrap();
        }
    }
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
