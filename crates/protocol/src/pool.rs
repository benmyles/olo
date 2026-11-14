//! Token Pool's meters, for Deskbar's widgets and agents (Ben,
//! 2026-11-13: "deskbar widget support for showing token pool usage /
//! quotas etc. i want to be able to have it show the aggregate
//! availability for each provider x quota type and any other
//! options/views you think might be useful and make sense").
//!
//! The copy of olo-pool serving writes where each account's quotas stand
//! (`PoolMeters`) to a small JSON file in the runtime directory as they
//! change (`olo_token_pool::meters`), and Deskbar follows the file
//! (`olo_kit::watch`): nothing asks the pool anything on a timer. What a
//! widget shows of it is worked out here (`availability`): what's left of
//! one provider's kind of quota across every account that can serve it,
//! when it next resets, when an account that's out comes back, and how
//! long what's left lasts at the pace of the last hour.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where each account's quotas stand, as the pool last said.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolMeters {
    /// The olo-pool process that wrote it: a file left by one that has
    /// gone says nothing.
    pub pid: u32,
    /// Its accounts have been read (its vault opened): until then it
    /// knows none.
    pub loaded: bool,
    pub accounts: Vec<PoolAccount>,
    /// The providers ("claude") whose client the person turned the pool
    /// off for: served on the client's own sign-in meanwhile.
    pub paused: Vec<String>,
}

/// One account's quotas.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolAccount {
    pub id: String,
    pub name: String,
    /// "claude" or "codex".
    pub provider: String,
    pub enabled: bool,
    /// Its sign-in no longer works (and why).
    pub signed_out: Option<String>,
    /// Refused every request until then (seconds since the epoch): its
    /// provider said a meter ran out.
    pub blocked_until: Option<i64>,
    /// Refused one model family's requests until then ("fable").
    pub model_blocked: BTreeMap<String, i64>,
    /// Requests it's answering now.
    pub in_flight: u32,
    pub last_used: Option<i64>,
    /// When its quotas were last read from its provider.
    pub synced: Option<i64>,
    pub windows: Vec<PoolWindow>,
}

/// One of an account's quotas.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolWindow {
    /// The provider's name for it ("five_hour").
    pub name: String,
    /// For people: "5-hour", "Weekly", "Weekly Fable".
    pub label: String,
    /// Percent used, 0–100 (it can pass 100).
    pub used: f64,
    pub resets_at: Option<i64>,
    /// The model families it's for; empty: every request.
    pub models: Vec<String>,
    /// Where it stood (when, and percent used) as far back as the pool
    /// has seen it within the hour before it wrote: its pace is worked
    /// out from that to now by whoever reads it (`pace_at`), so it falls
    /// as it stands still with nothing written.
    pub since: Option<(i64, f64)>,
}

impl PoolWindow {
    /// Percent used now: none once its reset has passed (the file may be
    /// older than that).
    pub fn used_at(&self, now: i64) -> f64 {
        match self.resets_at {
            Some(r) if r <= now => 0.0,
            _ => self.used.max(0.0),
        }
    }

    /// How fast it's being used at `now`: percent an hour since `since`
    /// (None: seen for under ten minutes, or reset since).
    pub fn pace_at(&self, now: i64) -> Option<f64> {
        let (t0, u0) = self.since?;
        if now - t0 < 600 || self.resets_at.is_some_and(|r| r <= now) || self.used < u0 {
            return None;
        }
        Some((self.used - u0) / ((now - t0) as f64 / 3600.0))
    }
}

impl PoolMeters {
    /// The file, in the runtime directory (one a display: a test
    /// session's Deskbar doesn't show the person's pool).
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("olo-pool-meters-{}.json", olo_xdg::display_name()))
    }

    /// What the pool says now: None if it isn't running (no file, or one
    /// a process that has gone left).
    pub fn load() -> Option<PoolMeters> {
        let m: PoolMeters = serde_json::from_str(&std::fs::read_to_string(Self::path()).ok()?).ok()?;
        (m.pid != 0 && std::path::Path::new(&format!("/proc/{}", m.pid)).exists()).then_some(m)
    }

    /// Written whole, at once (another reading it never sees half).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// The kinds of quota `provider`'s accounts have, as people call
    /// them: those for every request first, shortest first ("5-hour",
    /// "Weekly"), then a model's own ("Weekly Fable").
    pub fn quotas(&self, provider: &str) -> Vec<String> {
        let mut found: Vec<(bool, u8, String)> = Vec::new();
        for a in self.accounts.iter().filter(|a| a.provider == provider) {
            for w in &a.windows {
                if !found.iter().any(|(_, _, l)| *l == w.label) {
                    found.push((!w.models.is_empty(), length_rank(&w.label), w.label.clone()));
                }
            }
        }
        found.sort();
        found.into_iter().map(|(_, _, l)| l).collect()
    }

    /// The account of `provider`'s answering now, else the last that did
    /// (of those that can).
    pub fn serving(&self, provider: &str) -> Option<&PoolAccount> {
        let mine = || self.accounts.iter().filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none());
        mine().filter(|a| a.in_flight > 0).max_by_key(|a| a.last_used).or_else(|| mine().filter(|a| a.last_used.is_some()).max_by_key(|a| a.last_used))
    }

    pub fn paused(&self, provider: &str) -> bool {
        self.paused.iter().any(|p| p == provider)
    }
}

/// How a quota's name sorts: hours, then days, then weeks, then the rest.
fn length_rank(label: &str) -> u8 {
    let l = label.to_lowercase();
    if l.contains("hour") {
        0
    } else if l.contains("day") {
        1
    } else if l.contains("week") {
        2
    } else {
        3
    }
}

/// "Claude", "Codex": a provider as people call it.
pub fn provider_name(provider: &str) -> String {
    match provider {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        other => {
            let mut c = other.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        }
    }
}

/// The providers the pool knows.
pub const PROVIDERS: [&str; 2] = ["claude", "codex"];

/// One account, as a widget counts it.
#[derive(Debug, Clone, PartialEq)]
pub struct Counted {
    pub name: String,
    /// What it has left of the quota, percent (0–100); None: it's out of
    /// it (`out` says why).
    pub left: Option<f64>,
    /// Why it's out: "turned off", "signed out", "refused until …".
    pub out: Option<String>,
    /// When its quota resets.
    pub resets_at: Option<i64>,
    /// When it can serve again, if it's out (or used up) now.
    pub back_at: Option<i64>,
    /// Counted in the whole (an account turned off or signed out is
    /// only when asked to be).
    pub counted: bool,
    /// Answering a request now.
    pub serving: bool,
}

/// What's left of one provider's kind of quota across its accounts.
#[derive(Debug, Clone, PartialEq)]
pub struct Availability {
    pub provider: String,
    /// The quota ("5-hour"); None: whichever leaves each account least,
    /// of those for every request (what holds it back now).
    pub quota: Option<String>,
    /// Each account that has it, in the pool's order.
    pub accounts: Vec<Counted>,
    /// What's left in all, in accounts' worth (1.0: one account's whole
    /// quota).
    pub left: f64,
    /// How many accounts' worth there is in all.
    pub whole: f64,
    /// The soonest an account's quota resets, and whose (one with some
    /// of it used).
    pub next_reset: Option<(i64, String)>,
    /// The first of those used up or out to come back, and when.
    pub back: Option<(i64, String)>,
    /// Accounts' worth used an hour, at the last hour's pace (None: not
    /// known).
    pub pace: Option<f64>,
    /// The pool serves this provider's client (not turned off for it).
    pub serves: bool,
}

impl Availability {
    /// What's left of the whole, 0 to 1.
    pub fn fraction(&self) -> f64 {
        if self.whole > 0.0 { (self.left / self.whole).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// How long what's left lasts at this pace, in seconds (None: no pace
    /// to go by).
    pub fn lasts(&self) -> Option<i64> {
        let pace = self.pace.filter(|p| *p > 0.001)?;
        Some((self.left / pace * 3600.0) as i64)
    }

    /// Whether it's out on every account.
    pub fn exhausted(&self) -> bool {
        self.whole > 0.0 && self.left <= 0.0005
    }

    /// The quota, as a widget's name says it: "Claude 5-hour", "Codex".
    pub fn title(&self) -> String {
        match &self.quota {
            Some(q) => format!("{} {q}", provider_name(&self.provider)),
            None => provider_name(&self.provider),
        }
    }

    /// Its tip: what it is, what's left of how much, the next reset, the
    /// pace, then each account.
    pub fn detail(&self, now: i64) -> Vec<String> {
        let mut out = Vec::new();
        let n = self.accounts.iter().filter(|a| a.counted).count();
        out.push(format!("{}% left: {} of {} {}' worth", (self.fraction() * 100.0).round(), worth(self.left), worth(self.whole), if n == 1 { "account" } else { "accounts" }));
        if let Some((at, who)) = &self.next_reset {
            out.push(format!("Next reset in {} ({who})", duration_text(at - now)));
        }
        if let Some((at, who)) = &self.back
            && self.accounts.iter().any(|a| a.left.is_none_or(|l| l <= 0.0))
        {
            out.push(format!("{who} back in {}", duration_text(at - now)));
        }
        match self.lasts() {
            Some(s) if self.next_reset.as_ref().is_none_or(|(r, _)| s < r - now) => out.push(format!("At this pace, what's left lasts {}", duration_text(s))),
            Some(_) => out.push("At this pace, it lasts until the next reset".into()),
            None => {}
        }
        if !self.serves {
            out.push(format!("Turned off for {}: it uses its own sign-in", client_name(&self.provider)));
        }
        for a in &self.accounts {
            let mut line = match (&a.out, a.left) {
                (Some(why), _) => format!("{}: {why}", a.name),
                (None, Some(l)) => format!("{}: {}% left", a.name, l.round()),
                (None, None) => format!("{}: not read yet", a.name),
            };
            if a.out.is_none()
                && let Some(r) = a.resets_at.filter(|r| *r > now)
            {
                line.push_str(&format!(", resets in {}", duration_text(r - now)));
            }
            if a.serving {
                line.push_str(" (answering now)");
            }
            out.push(line);
        }
        out
    }
}

/// "Claude Code", "Codex": a provider's client.
pub fn client_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude Code",
        "codex" => "Codex",
        _ => "its client",
    }
}

/// "1.9", "3": accounts' worth.
fn worth(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{:.0}", v.round()) } else { format!("{v:.1}") }
}

/// A stretch of time, short: "under a minute", "12 min", "1 h 20",
/// "3 h", "2 days 5 h".
pub fn duration_text(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    match (days, hours, minutes) {
        (0, 0, 0) => "under a minute".into(),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m:02}"),
        (1, 0, _) => "1 day".into(),
        (d, 0, _) => format!("{d} days"),
        (1, h, _) => format!("1 day {h} h"),
        (d, h, _) => format!("{d} days {h} h"),
    }
}

/// What's left of `provider`'s `quota` (None: what holds each account
/// back now) across its accounts at `now`: each account that has it,
/// those turned off or signed out counted in the whole only if
/// `count_off`. None if no account of `provider`'s has it.
pub fn availability(m: &PoolMeters, provider: &str, quota: Option<&str>, count_off: bool, now: i64) -> Option<Availability> {
    let mut accounts = Vec::new();
    let (mut left, mut whole) = (0.0, 0.0);
    let mut next_reset: Option<(i64, String)> = None;
    let mut back: Option<(i64, String)> = None;
    let mut pace: Option<f64> = None;
    let sooner = |slot: &mut Option<(i64, String)>, at: i64, who: &str| {
        if slot.as_ref().is_none_or(|(t, _)| at < *t) {
            *slot = Some((at, who.to_owned()));
        }
    };
    for a in m.accounts.iter().filter(|a| a.provider == provider) {
        // Its window for the quota: the one by that name, or the one for
        // every request with least left.
        let window = match quota {
            Some(q) => a.windows.iter().find(|w| w.label == q),
            None => a.windows.iter().filter(|w| w.models.is_empty()).min_by(|x, y| (100.0 - x.used_at(now)).total_cmp(&(100.0 - y.used_at(now)))),
        };
        let Some(w) = window else {
            // (Not read yet: listed, not counted. One without this quota
            // isn't listed.)
            if quota.is_none() && a.windows.is_empty() && a.enabled && a.signed_out.is_none() {
                accounts.push(Counted { name: a.name.clone(), left: None, out: Some("not read yet".into()), resets_at: None, back_at: None, counted: false, serving: a.in_flight > 0 });
            }
            continue;
        };
        let blocked = a.blocked_until.filter(|t| *t > now).into_iter().chain(w.models.iter().filter_map(|f| a.model_blocked.get(f).copied()).filter(|t| *t > now)).max();
        let resets_at = w.resets_at.filter(|r| *r > now);
        let have = (100.0 - w.used_at(now)).clamp(0.0, 100.0);
        let (out, counted) = if !a.enabled {
            (Some("turned off".to_owned()), count_off)
        } else if a.signed_out.is_some() {
            (Some("signed out".to_owned()), count_off)
        } else if let Some(t) = blocked {
            (Some(format!("refused until it resets, in {}", duration_text(t - now))), true)
        } else {
            (None, true)
        };
        let usable = out.is_none();
        let back_at = match (usable, blocked) {
            (false, Some(t)) => Some(t),
            (true, _) if have <= 0.0 => resets_at,
            _ => None,
        };
        if counted {
            whole += 1.0;
            if usable {
                left += have / 100.0;
                if let Some(p) = w.pace_at(now) {
                    *pace.get_or_insert(0.0) += p.max(0.0) / 100.0;
                }
            }
            if let Some(r) = resets_at
                && w.used_at(now) > 0.0
            {
                sooner(&mut next_reset, r, &a.name);
            }
            if let Some(t) = back_at {
                sooner(&mut back, t, &a.name);
            }
        }
        accounts.push(Counted { name: a.name.clone(), left: usable.then_some(have), out, resets_at, back_at, counted, serving: a.in_flight > 0 });
    }
    if accounts.is_empty() {
        return None;
    }
    Some(Availability { provider: provider.to_owned(), quota: quota.map(str::to_owned), accounts, left, whole, next_reset, back, pace, serves: !m.paused(provider) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f64, resets_at: Option<i64>) -> PoolWindow {
        PoolWindow { name: label.to_lowercase(), label: label.into(), used, resets_at, models: Vec::new(), since: None }
    }

    fn account(name: &str, provider: &str, windows: Vec<PoolWindow>) -> PoolAccount {
        PoolAccount { id: name.to_lowercase(), name: name.into(), provider: provider.into(), enabled: true, windows, ..Default::default() }
    }

    fn pool() -> PoolMeters {
        let mut fable = window("Weekly Fable", 100.0, Some(9_000));
        fable.models = vec!["fable".into()];
        let mut home = account("Home", "claude", vec![window("5-hour", 40.0, Some(20_000)), window("Weekly", 40.0, Some(50_000))]);
        // (10% used an hour before: 30% an hour.)
        home.windows[0].since = Some((1_000 - 3_600, 10.0));
        let mut old = account("Old", "claude", vec![window("5-hour", 0.0, None)]);
        old.signed_out = Some("refresh refused".into());
        PoolMeters {
            pid: 1,
            loaded: true,
            accounts: vec![
                account("Work", "claude", vec![window("5-hour", 80.0, Some(1_600)), window("Weekly", 20.0, Some(40_000)), fable]),
                home,
                old,
                account("Chat", "codex", vec![window("5-hour", 50.0, Some(3_000)), window("Weekly", 5.0, Some(90_000))]),
            ],
            paused: Vec::new(),
        }
    }

    /// Two accounts that can serve, one signed out: what's left of the
    /// two, the soonest reset with something used, the pace.
    #[test]
    fn a_quota_across_accounts() {
        let a = availability(&pool(), "claude", Some("5-hour"), false, 1_000).unwrap();
        assert_eq!(a.accounts.len(), 3);
        assert!((a.left - 0.8).abs() < 1e-9, "{}", a.left);
        assert_eq!(a.whole, 2.0);
        assert!((a.fraction() - 0.4).abs() < 1e-9);
        assert_eq!(a.next_reset, Some((1_600, "Work".into())));
        assert!((a.pace.unwrap() - 0.3).abs() < 1e-9);
        // 0.8 accounts' worth at 0.3 an hour: 2 h 40.
        assert_eq!(a.lasts(), Some(9_600));
        // Nothing used for another hour: half the pace.
        let still = availability(&pool(), "claude", Some("5-hour"), false, 1_000 + 3_600).unwrap();
        assert!((still.pace.unwrap() - 0.15).abs() < 1e-9);
        assert_eq!(a.title(), "Claude 5-hour");
        // Counting the one signed out: a third account's worth, none left.
        let all = availability(&pool(), "claude", Some("5-hour"), true, 1_000).unwrap();
        assert_eq!(all.whole, 3.0);
        assert!((all.left - 0.8).abs() < 1e-9);
    }

    /// A reset that has passed since the pool wrote counts as reset; a
    /// model's own quota counts the accounts that have it; one used up
    /// says when it's back.
    #[test]
    fn resets_models_and_coming_back() {
        let later = availability(&pool(), "claude", Some("5-hour"), false, 1_700).unwrap();
        // (Work's reset at 1,600 has passed: all of it is left.)
        assert!((later.left - 1.6).abs() < 1e-9);
        let fable = availability(&pool(), "claude", Some("Weekly Fable"), false, 1_000).unwrap();
        assert_eq!((fable.accounts.len(), fable.whole, fable.left), (1, 1.0, 0.0));
        assert!(fable.exhausted());
        assert_eq!(fable.back, Some((9_000, "Work".into())));
        // Refused until a time: out, and back then.
        let mut m = pool();
        m.accounts[1].blocked_until = Some(5_000);
        let a = availability(&m, "claude", Some("5-hour"), false, 1_000).unwrap();
        assert!((a.left - 0.2).abs() < 1e-9);
        assert_eq!(a.pace, None);
        assert_eq!(a.back, Some((5_000, "Home".into())));
        assert!(a.accounts[1].out.as_deref().unwrap().starts_with("refused"));
    }

    /// No quota named: each account's tightest for every request.
    #[test]
    fn what_holds_each_back() {
        let a = availability(&pool(), "claude", None, false, 1_000).unwrap();
        // Work: 5-hour 20% left (not its Fable week, which is for one
        // model); Home: 60% left of both.
        assert!((a.left - 0.8).abs() < 1e-9, "{}", a.left);
        assert_eq!(a.title(), "Claude");
        assert_eq!(availability(&pool(), "codex", Some("Weekly Fable"), false, 0), None);
    }

    #[test]
    fn quotas_in_order() {
        assert_eq!(pool().quotas("claude"), vec!["5-hour", "Weekly", "Weekly Fable"]);
        assert_eq!(pool().quotas("codex"), vec!["5-hour", "Weekly"]);
    }

    #[test]
    fn the_one_answering() {
        let mut m = pool();
        m.accounts[0].last_used = Some(10);
        m.accounts[1].last_used = Some(20);
        assert_eq!(m.serving("claude").map(|a| a.name.as_str()), Some("Home"));
        m.accounts[0].in_flight = 1;
        assert_eq!(m.serving("claude").map(|a| a.name.as_str()), Some("Work"));
        assert_eq!(m.serving("codex"), None);
    }

    #[test]
    fn a_tip() {
        let a = availability(&pool(), "claude", Some("5-hour"), false, 1_000).unwrap();
        let tip = a.detail(1_000);
        assert_eq!(tip[0], "40% left: 0.8 of 2 accounts' worth");
        assert_eq!(tip[1], "Next reset in 10 min (Work)");
        assert_eq!(tip[2], "At this pace, it lasts until the next reset");
        assert_eq!(tip[3], "Work: 20% left, resets in 10 min");
        assert_eq!(tip[5], "Old: signed out");
        assert_eq!(duration_text(4_800), "1 h 20");
        assert_eq!(duration_text(90_000), "1 day 1 h");
    }
}
