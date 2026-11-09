//! Which snapshots to keep: every one from the last hour, then the newest
//! of each of the last so many hours, days, weeks and months (in local
//! time). A snapshot any rule keeps stays; the rest are forgotten (and what
//! only they used goes at the next prune). (Every one from the last hour:
//! a backup made a few minutes ago isn't let go of the moment another is
//! made, Back Up Now in the same hour, say.)

use crate::{crypto::Id, repo::Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub hourly: u32,
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
}

/// The local hour, day, ISO week and month `t` (seconds since 1970) is in,
/// each as a number that's the same exactly within that period.
fn periods(t: i64) -> [i64; 4] {
    // SAFETY: localtime_r writes the struct it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    unsafe { libc::localtime_r(&tt, &mut tm) };
    let (year, yday, hour, mon) = (tm.tm_year as i64 + 1900, tm.tm_yday as i64, tm.tm_hour as i64, tm.tm_mon as i64);
    let day = year * 400 + yday;
    // ISO weeks start on Monday: the day's number less its weekday.
    let weekday = (tm.tm_wday as i64 + 6) % 7;
    let days_since = (t + tm.tm_gmtoff as i64).div_euclid(86_400);
    [day * 24 + hour, day, (days_since - weekday).div_euclid(7), year * 12 + mon]
}

/// The snapshots (any order) to forget under `policy`.
pub fn to_forget(snapshots: &[(Id, Snapshot)], policy: Policy) -> Vec<Id> {
    let mut by_time: Vec<&(Id, Snapshot)> = snapshots.iter().collect();
    by_time.sort_by_key(|s| std::cmp::Reverse(s.1.time));
    // Every one within the hour before the newest (the newest too).
    let newest = by_time.first().map_or(0, |s| s.1.time);
    let mut keep: Vec<bool> = by_time.iter().map(|s| newest - s.1.time < 3600).collect();
    for (rule, count) in [policy.hourly, policy.daily, policy.weekly, policy.monthly].into_iter().enumerate() {
        let mut last = None;
        let mut taken = 0;
        for (i, (_, s)) in by_time.iter().enumerate() {
            if taken >= count {
                break;
            }
            let p = periods(s.time)[rule];
            // The newest of each period (newest first: the first seen).
            if last != Some(p) {
                last = Some(p);
                keep[i] = true;
                taken += 1;
            }
        }
    }
    by_time.iter().zip(keep).filter(|(_, k)| !k).map(|((id, _), _)| *id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Summary;

    fn snap(t: i64) -> (Id, Snapshot) {
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&t.to_le_bytes());
        (Id(id), Snapshot { time: t, host: "h".into(), paths: vec![], tree: Id::default(), parent: None, summary: Summary::default(), extra: Vec::new() })
    }

    /// Every 20 minutes for 3 days: the last hour's, 24 hourly, 3 daily
    /// kept (the newest of each), the rest forgotten.
    #[test]
    fn hourly_and_daily() {
        let start = 1_800_000_000 - 1_800_000_000 % 86_400;
        let all: Vec<(Id, Snapshot)> = (0..3 * 72).map(|k| snap(start + k * 1200)).collect();
        let forget = to_forget(&all, Policy { hourly: 24, daily: 7, weekly: 0, monthly: 0 });
        let kept: Vec<i64> = all.iter().filter(|(id, _)| !forget.contains(id)).map(|(_, s)| s.time).collect();
        // 24 hours' newest, the last hour's (up to 2 more), each earlier
        // day's newest (2 more), at most.
        assert!((24..=29).contains(&kept.len()), "{}", kept.len());
        assert!(kept.contains(&all.last().unwrap().1.time));
        // Nothing kept twice for one hour, but the last hour.
        let newest = all.last().unwrap().1.time;
        let mut hours: Vec<i64> = kept.iter().filter(|t| newest - **t >= 3600).map(|t| periods(*t)[0]).collect();
        let n = hours.len();
        hours.dedup();
        assert_eq!(hours.len(), n);
        // Keep nothing at all: still the last hour's (the newest, and the
        // two before it, 20 and 40 minutes before).
        let forget = to_forget(&all, Policy { hourly: 0, daily: 0, weekly: 0, monthly: 0 });
        assert_eq!(forget.len(), all.len() - 3);
    }
}
