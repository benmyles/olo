//! Which account serves a request. The aim is to waste as little quota as
//! possible: quota that resets unused is gone, so drain the account whose
//! usable quota expires soonest first (earliest-expiry-first), then the
//! older account, then by id, so the order is the same after a restart.
//! An account in use keeps serving unless another expires more than
//! five minutes sooner, so rolling windows' small differences don't make
//! it hop back and forth.

use crate::model::{Account, Provider};

/// How much sooner another account's quota must expire to take over.
pub const STICKY: i64 = 5 * 60;

/// When `a` can next serve `family` if it can't now (the latest of what
/// blocks it), else None.
pub fn blocked_until(a: &Account, family: &str, now: i64) -> Option<i64> {
    let mut until: Option<i64> = None;
    let mut block = |t: i64| until = Some(until.map_or(t, |u| u.max(t)));
    for w in a.windows.iter().filter(|w| w.applies_to(family) && w.exhausted(now)) {
        // An exhausted window with no reset time blocks for a minute, when
        // the quota is read again.
        block(w.resets_at.unwrap_or(now + 60));
    }
    if let Some(c) = a.cooldown_until.filter(|&c| c > now) {
        block(c);
    }
    if let Some(&c) = a.model_cooldowns.get(family).filter(|&&c| c > now) {
        block(c);
    }
    until
}

/// When `a`'s soonest usable quota for `family` resets (unknown: never,
/// so it goes last).
pub fn expiry(a: &Account, family: &str, now: i64) -> i64 {
    a.windows.iter().filter(|w| w.applies_to(family) && w.used < 100.0).filter_map(|w| w.resets_at.filter(|&r| r > now)).min().unwrap_or(i64::MAX)
}

/// Whether `a` may serve `provider`'s `family` now (`has_credential`:
/// whether the pool holds its sign-in).
pub fn eligible(a: &Account, provider: Provider, family: &str, now: i64, has_credential: bool) -> bool {
    a.provider == provider && a.enabled && a.signed_out.is_none() && has_credential && blocked_until(a, family, now).is_none()
}

/// The account to use, of those `eligible` and not in `tried`; `sticky`
/// is the one serving this provider and family lately.
pub fn pick<'a>(accounts: &'a [Account], provider: Provider, family: &str, sticky: Option<&str>, now: i64, tried: &[String], has_credential: impl Fn(&Account) -> bool) -> Option<&'a Account> {
    let mut ok: Vec<&Account> = accounts.iter().filter(|a| !tried.contains(&a.id) && has_credential(a) && eligible(a, provider, family, now, true)).collect();
    ok.sort_by(|a, b| (expiry(a, family, now), a.created, &a.id).cmp(&(expiry(b, family, now), b.created, &b.id)));
    let best = *ok.first()?;
    if let Some(s) = sticky.and_then(|id| ok.iter().find(|a| a.id == id)) {
        let (keep, other) = (expiry(s, family, now), expiry(best, family, now));
        // `other` must be STICKY sooner to take over (no overflow at MAX).
        if keep == other || other.saturating_add(STICKY) >= keep {
            return Some(s);
        }
    }
    Some(best)
}

/// When the first of `provider`'s accounts could serve `family` again,
/// for a Retry-After when none can now.
pub fn next_available(accounts: &[Account], provider: Provider, family: &str, now: i64) -> Option<i64> {
    accounts.iter().filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none()).filter_map(|a| blocked_until(a, family, now)).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Window;

    fn acct(id: &str, created: i64, windows: &[(f64, Option<i64>)]) -> Account {
        let mut a = Account::new(id.into(), id.into(), Provider::Claude);
        a.created = created;
        a.windows = windows.iter().enumerate().map(|(i, &(used, resets_at))| Window { name: format!("w{i}"), label: String::new(), used, resets_at, models: vec![] }).collect();
        a
    }

    fn choose<'a>(accounts: &'a [Account], sticky: Option<&str>, now: i64) -> Option<&'a str> {
        pick(accounts, Provider::Claude, "opus", sticky, now, &[], |_| true).map(|a| a.id.as_str())
    }

    #[test]
    fn earliest_expiry_first() {
        let now = 1000;
        let accounts = [acct("late", 1, &[(10.0, Some(9000))]), acct("soon", 2, &[(80.0, Some(3000))])];
        assert_eq!(choose(&accounts, None, now), Some("soon"));
        // Exhausted: skipped until its window resets.
        let accounts = [acct("late", 1, &[(10.0, Some(9000))]), acct("soon", 2, &[(100.0, Some(3000))])];
        assert_eq!(choose(&accounts, None, now), Some("late"));
        // Past its reset it may serve again (its new reset unknown till
        // the quota is read, so after one that's known).
        assert!(eligible(&accounts[1], Provider::Claude, "opus", 3001, true));
        assert_eq!(choose(&accounts[1..], None, 3001), Some("soon"));
    }

    #[test]
    fn ties_and_unknowns() {
        let now = 0;
        // Unknown expiry goes last; equal ones by age, then id.
        let accounts = [acct("b", 5, &[]), acct("a", 5, &[(1.0, Some(100))]), acct("c", 1, &[(1.0, Some(100))])];
        assert_eq!(choose(&accounts, None, now), Some("c"));
        let accounts = [acct("b", 5, &[]), acct("a", 5, &[])];
        assert_eq!(choose(&accounts, None, now), Some("a"));
    }

    #[test]
    fn stickiness() {
        let now = 0;
        let accounts = [acct("x", 1, &[(5.0, Some(10_000))]), acct("y", 2, &[(5.0, Some(10_000 - 200))])];
        // y expires sooner, but by less than five minutes: x keeps serving.
        assert_eq!(choose(&accounts, Some("x"), now), Some("x"));
        let accounts = [acct("x", 1, &[(5.0, Some(10_000))]), acct("y", 2, &[(5.0, Some(10_000 - 400))])];
        assert_eq!(choose(&accounts, Some("x"), now), Some("y"));
    }

    #[test]
    fn model_meters_and_cooldowns() {
        let now = 0;
        let mut opus_out = acct("a", 1, &[(10.0, Some(500))]);
        opus_out.windows.push(Window { name: "model:opus/seven_day".into(), label: String::new(), used: 100.0, resets_at: Some(9000), models: vec!["opus".into()] });
        let other = acct("b", 2, &[(10.0, Some(800))]);
        let accounts = [opus_out, other];
        assert_eq!(choose(&accounts, None, now), Some("b"));
        // A Sonnet request isn't held up by Opus's meter.
        assert_eq!(pick(&accounts, Provider::Claude, "sonnet", None, now, &[], |_| true).map(|a| a.id.as_str()), Some("a"));
        let mut cooled = acct("c", 0, &[(1.0, Some(100))]);
        cooled.cooldown_until = Some(60);
        assert_eq!(blocked_until(&cooled, "opus", now), Some(60));
        assert_eq!(next_available(&[cooled.clone()], Provider::Claude, "opus", now), Some(60));
        cooled.enabled = false;
        assert_eq!(choose(&[cooled], None, 61), None);
    }
}
