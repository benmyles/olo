//! What the pool knows: its accounts, each one's credential (kept apart,
//! sealed), quota windows and usage.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

/// Seconds since the epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Claude Code, on a Claude subscription.
    Claude,
    /// Codex, on a ChatGPT subscription.
    Codex,
}

impl Provider {
    pub fn client(self) -> &'static str {
        match self {
            Provider::Claude => "Claude Code",
            Provider::Codex => "Codex",
        }
    }
    pub fn parse(s: &str) -> Option<Provider> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "anthropic" => Some(Provider::Claude),
            "codex" | "chatgpt" | "openai" => Some(Provider::Codex),
            _ => None,
        }
    }
}

/// An account's sign-in: never shown, logged or sent anywhere but its
/// provider.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Credential {
    pub access: String,
    pub refresh: Option<String>,
    /// When `access` stops working (seconds since the epoch), if known.
    pub expires_at: Option<i64>,
    /// ChatGPT's account id, sent with Codex requests.
    #[serde(default)]
    pub account_id: Option<String>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential").field("expires_at", &self.expires_at).finish_non_exhaustive()
    }
}

/// One of a subscription's quota windows, as its provider reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// The provider's name for it ("five_hour", "seven_day_opus",
    /// "primary_window", "model:opus/seven_day").
    pub name: String,
    /// For people: "5-hour", "Weekly", "Weekly Opus".
    pub label: String,
    /// Percent used, 0–100 (it can pass 100).
    pub used: f64,
    pub resets_at: Option<i64>,
    /// The model families it applies to; empty: every request.
    #[serde(default)]
    pub models: Vec<String>,
}

impl Window {
    pub fn applies_to(&self, family: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|m| m == family)
    }
    pub fn exhausted(&self, now: i64) -> bool {
        self.used >= 100.0 && self.resets_at.is_none_or(|r| r > now)
    }

    /// How long it runs, in seconds: by what it's called ("5-hour",
    /// "Weekly", "GPT-5.3-Codex-Spark 5-hour", "3-day": Codex's primary
    /// window is a week long on some plans), else by its name.
    pub fn length(&self) -> Option<i64> {
        let label = self.label.as_str();
        let words: Vec<&str> = label.split_whitespace().collect();
        if words.contains(&"Weekly") {
            return Some(7 * 86400);
        }
        for w in &words {
            if let Some(n) = w.strip_suffix("-hour").and_then(|n| n.parse::<i64>().ok()) {
                return Some(n * 3600);
            }
            if let Some(n) = w.strip_suffix("-day").and_then(|n| n.parse::<i64>().ok()) {
                return Some(n * 86400);
            }
        }
        let n = self.name.as_str();
        if n.contains("five_hour") || n.contains("primary") {
            Some(5 * 3600)
        } else if n.contains("seven_day") || n.contains("secondary") {
            Some(7 * 86400)
        } else {
            None
        }
    }

    /// Percent used per second since it started (None this early on,
    /// or when its length isn't known).
    pub fn rate(&self, now: i64) -> Option<f64> {
        let elapsed = now - (self.resets_at? - self.length()?);
        (elapsed >= 600).then(|| self.used / elapsed as f64)
    }

    /// What claiming a reset of it at `at` gives back, in percent of its
    /// quota, by how each provider's reset works. Claude's fills it up
    /// again and leaves its reset as it was ("Your weekly limits still
    /// reset on their usual day and time"): what's used then, as much of
    /// it as there's time to use before it resets by itself, at the rate
    /// it's used. Codex's starts a new window at the next request and loses
    /// what was left (OpenAI's help: it "pulls your normal weekly allowance
    /// forward"): what's used then, less the part of the window that's
    /// gone. Either way next to nothing just before it resets by itself.
    pub fn reset_worth(&self, provider: Provider, now: i64, at: i64) -> f64 {
        let (Some(reset), Some(len)) = (self.resets_at, self.length()) else { return 0.0 };
        if len <= 0 {
            return 0.0;
        }
        // (Its reset after `at`: a new window runs from each reset.)
        let mut next = reset;
        while next <= at {
            next += len;
        }
        let used = self.used_at(now, at);
        match provider {
            // What a refill lets through that what's left wouldn't: of the
            // use to come before it resets by itself, the part past what's
            // left (Codex's review: "the gain is min(demand, 100) −
            // min(demand, remaining)"). Used up before it had a rate (in its
            // first minutes): all of it wanted.
            Provider::Claude => {
                let demand = match self.rate(now) {
                    Some(rate) => rate * (next - at) as f64,
                    None if used >= 100.0 => 100.0,
                    None => 0.0,
                };
                let left = (100.0 - used).max(0.0);
                (demand.min(100.0) - demand.min(left)).clamp(0.0, 100.0)
            }
            Provider::Codex => {
                let gone = 100.0 * (len - (next - at)) as f64 / len as f64;
                (used - gone).clamp(0.0, 100.0)
            }
        }
    }

    /// How much a reset at `at` would give back (percent): what it is
    /// expected to have used by then, at today's rate, starting afresh
    /// after its own reset.
    pub fn used_at(&self, now: i64, at: i64) -> f64 {
        let rate = self.rate(now).unwrap_or(0.0);
        let used = match (self.resets_at, self.length()) {
            // (A new window from each reset: since the latest before `at`.)
            (Some(r), Some(len)) if at >= r && len > 0 => rate * ((at - r) % len) as f64,
            (Some(r), None) if at >= r => rate * (at - r) as f64,
            _ => self.used + rate * (at - now).max(0) as f64,
        };
        used.min(100.0)
    }
}

/// Look this far ahead for a credit about to expire unused.
pub const EXPIRY_LOOKAHEAD: i64 = 48 * 3600;
/// Redeem a credit no later than this before it expires.
pub const EXPIRY_MARGIN: i64 = 30 * 60;
/// Nor later than this before a window resets by itself.
pub const RESET_MARGIN: i64 = 10 * 60;
/// A reset must give back at least this much (percent) to be worth it.
pub const WORTH_REDEEMING: f64 = 5.0;

/// Tokens and requests, counted from the providers' usage reports (never
/// what was asked or answered).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// How a request through the pool ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    /// Its account was out of quota (the next one took it).
    RateLimited,
    /// Refused, a server error or a broken answer.
    Failed,
    /// The client went away before the answer ended.
    Cancelled,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Completed => "Completed",
            Outcome::RateLimited => "Rate limited",
            Outcome::Failed => "Failed",
            Outcome::Cancelled => "Cancelled",
        }
    }
}

/// A request an account served or turned away: never its content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// When it ended (seconds since the epoch).
    pub at: i64,
    pub model: String,
    /// The program that sent it ("Claude Code", "Codex", …).
    #[serde(default)]
    pub client: String,
    pub outcome: Outcome,
    /// The provider's HTTP status (0: none came).
    pub status: u16,
    /// How long it ran, in milliseconds.
    pub ms: u64,
    #[serde(default)]
    pub usage: Usage,
}

/// How many of an account's latest requests it keeps.
pub const RECENT: usize = 25;

impl ResetCredit {
    /// Whether claiming it resets `w`: a Codex credit, any of the account's
    /// own windows; a Claude grant, the limits it `clears` (its week,
    /// `seven_day`; Fable's week, `seven_day_overage_included`; or by the
    /// window's own name); one that doesn't say, the shared windows.
    pub fn clears(&self, provider: Provider, w: &Window) -> bool {
        if provider == Provider::Codex || self.clears.is_empty() {
            return w.models.is_empty();
        }
        // (Fable's week: `seven_day_overage_included` is a weekly limit,
        // never a 5-hour one.)
        let weekly = w.name.starts_with("seven_day") || w.name.ends_with("/seven_day");
        let fable_week = weekly && (w.models.iter().any(|m| m == "fable") || w.name.contains("fable"));
        (self.gives_back_weekly && w.name == "seven_day") || self.clears.iter().any(|c| *c == w.name || (fable_week && c == "seven_day_overage_included"))
    }

    /// How many limits it clears (giving the week back counts as one).
    pub fn breadth(&self) -> usize {
        self.clears.len() + usize::from(self.gives_back_weekly && !self.clears.iter().any(|c| c == "seven_day"))
    }

    /// Usable at `now`: started, not expired, some left.
    pub fn usable_at(&self, now: i64) -> bool {
        self.usable && self.remaining > 0 && self.starts_at.is_none_or(|s| s <= now) && self.expires_at.is_none_or(|e| e > now)
    }
}

/// A provider-issued credit that resets an account's quota early (some
/// subscriptions get a few).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    pub expires_at: Option<i64>,
    /// Redemptions left in it.
    pub remaining: u32,
    /// The provider says it can be used now.
    pub usable: bool,
    /// Claude's program name for it.
    #[serde(default)]
    pub program: Option<String>,
    /// What the provider calls it ("One full limit reset (November
    /// 2026)").
    #[serde(default)]
    pub label: Option<String>,
    /// Its kind: Claude's "session", "weekly" or "full"; ChatGPT's
    /// `reset_type`.
    #[serde(default)]
    pub kind: Option<String>,
    /// The limits it clears, as the rate-limit headers name them
    /// ("seven_day", "seven_day_overage_included": Fable's week).
    #[serde(default)]
    pub clears: Vec<String>,
    /// When it can first be used.
    #[serde(default)]
    pub starts_at: Option<i64>,
    /// Only at a limit it clears (Claude's `use_requires_limit`).
    #[serde(default)]
    pub needs_limit: bool,
    /// It gives back the week's use too (Claude's `gives_back_weekly`): a
    /// Fable reset claimed on 2026-11-19 took its account's week from 50%
    /// to 1% with its Fable week, though it `clears` Fable's alone.
    #[serde(default)]
    pub gives_back_weekly: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Random, stable.
    pub id: String,
    pub name: String,
    pub provider: Provider,
    /// When it joined the pool: ties go to the older.
    pub created: i64,
    /// Paused ones are never chosen.
    pub enabled: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<Window>,
    /// Not chosen before this (a rejection without a known reset).
    #[serde(default)]
    pub cooldown_until: Option<i64>,
    /// The same, for one model family (its own meter ran out).
    #[serde(default)]
    pub model_cooldowns: BTreeMap<String, i64>,
    /// Why it needs signing in again, when its provider refused it.
    #[serde(default)]
    pub signed_out: Option<String>,
    #[serde(default)]
    pub total: Usage,
    /// Today's (local date "YYYY-MM-DD", usage).
    #[serde(default)]
    pub today: (String, Usage),
    pub last_used: Option<i64>,
    /// The last time its quota was read from the provider.
    pub synced: Option<i64>,
    /// Signed in on its own, apart from the client's own sign-in.
    #[serde(default)]
    pub separate: bool,
    /// Its Claude organization (reset credits are redeemed for it).
    #[serde(default)]
    pub org: Option<String>,
    /// Its reset credits, as last read.
    #[serde(default)]
    pub credits: Vec<ResetCredit>,
    /// A redemption under way: (credit, request id), kept before asking so
    /// a retry after an unclear outcome can't redeem twice.
    #[serde(default)]
    pub redeeming: Option<(String, String)>,
    /// The last time a credit was redeemed for it automatically, or tried.
    #[serde(default)]
    pub redeem_tried: Option<i64>,
    /// Its latest requests, newest last (at most [`RECENT`]).
    #[serde(default)]
    pub recent: VecDeque<Request>,
}

impl Account {
    /// The credit to use next: usable, the soonest to expire.
    pub fn credit(&self) -> Option<&ResetCredit> {
        self.credits.iter().filter(|c| c.usable && c.remaining > 0).min_by_key(|c| c.expires_at.unwrap_or(i64::MAX))
    }

    /// What claiming `c` at `at` gives back: the most it gives back of a
    /// window it resets (`Window::reset_worth`).
    pub fn claim_worth(&self, c: &ResetCredit, now: i64, at: i64) -> f64 {
        self.windows.iter().filter(|w| c.clears(self.provider, w)).map(|w| w.reset_worth(self.provider, now, at)).fold(0.0, f64::max)
    }

    /// The credit to claim now for a request of `family` this account
    /// can't serve, and what it gives back: one that resets a window that's
    /// out for it, the narrowest that does (a Fable reset before one that
    /// clears the whole week too, which a used-up week will want), then the
    /// soonest to expire; none that gives back under `WORTH_REDEEMING` (a
    /// window about to reset by itself is better waited for).
    pub fn claim_for(&self, family: &str, now: i64) -> Option<(&ResetCredit, f64)> {
        let out: Vec<&Window> = self.windows.iter().filter(|w| w.applies_to(family) && w.exhausted(now)).collect();
        if out.is_empty() {
            return None;
        }
        // (One that clears every limit that's out for it: one that leaves
        // another standing lets the request through no more than before.)
        self.credits
            .iter()
            .filter(|c| c.usable_at(now) && out.iter().all(|w| c.clears(self.provider, w)))
            .map(|c| (c, out.iter().filter(|w| c.clears(self.provider, w)).map(|w| w.reset_worth(self.provider, now, now)).fold(0.0, f64::max)))
            .filter(|(_, worth)| *worth >= WORTH_REDEEMING)
            .min_by_key(|(c, _)| (c.breadth(), c.expires_at.unwrap_or(i64::MAX)))
    }

    /// A credit about to expire unused (within `EXPIRY_LOOKAHEAD`) to
    /// claim now: now is its best moment of those left (each hour until 30
    /// minutes before it expires, and that last chance), by what a claim
    /// then gives back (`claim_worth`), and that's at least
    /// `WORTH_REDEEMING`. A later moment as good wins (it keeps the
    /// choice open).
    pub fn expiring_claim(&self, now: i64) -> Option<&ResetCredit> {
        // (Each about to expire weighed, the soonest first: one worth
        // nothing never hides another worth claiming.)
        let mut expiring: Vec<&ResetCredit> = self.credits.iter().filter(|c| c.usable_at(now) && c.expires_at.is_some_and(|e| e - now <= EXPIRY_LOOKAHEAD)).collect();
        expiring.sort_by_key(|c| c.expires_at);
        expiring.into_iter().find(|c| {
            // (One that only works at a limit, only when one it clears is out.)
            if c.needs_limit && !self.windows.iter().any(|w| c.clears(self.provider, w) && w.exhausted(now)) {
                return false;
            }
            let Some(expires) = c.expires_at else { return false };
            let last = expires - EXPIRY_MARGIN;
            let here = self.claim_worth(c, now, now);
            let later = (1..).map(|h| now + h * 3600).take_while(|&t| t < last).chain(std::iter::once(last)).filter(|&t| t > now).map(|t| self.claim_worth(c, now, t)).fold(0.0, f64::max);
            here >= WORTH_REDEEMING && (here > later || now >= last)
        })
    }

    /// Whether to claim its next credit now so that it isn't lost
    /// (`expiring_claim`).
    pub fn spend_before_expiry(&self, now: i64) -> bool {
        self.expiring_claim(now).is_some()
    }
}

impl Account {
    pub fn new(id: String, name: String, provider: Provider) -> Account {
        Account {
            id,
            name,
            provider,
            created: now(),
            enabled: true,
            email: None,
            plan: None,
            windows: Vec::new(),
            cooldown_until: None,
            model_cooldowns: BTreeMap::new(),
            signed_out: None,
            total: Usage::default(),
            today: (String::new(), Usage::default()),
            last_used: None,
            synced: None,
            separate: false,
            org: None,
            credits: Vec::new(),
            redeeming: None,
            redeem_tried: None,
            recent: VecDeque::new(),
        }
    }

    /// Count a finished request.
    pub fn record(&mut self, usage: &Usage, today: &str, at: i64) {
        self.total.add(usage);
        if self.today.0 != today {
            self.today = (today.to_owned(), Usage::default());
        }
        self.today.1.add(usage);
        self.last_used = Some(at);
    }

    /// Keep a finished request in its strip.
    pub fn finished(&mut self, r: Request) {
        self.recent.push_back(r);
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
    }

    /// Fold in windows newly reported (by name), keeping a known reset
    /// time a sparser report leaves out.
    pub fn merge_windows(&mut self, new: Vec<Window>) {
        for mut w in new {
            match self.windows.iter_mut().find(|o| o.name == w.name) {
                Some(old) => {
                    if w.resets_at.is_none() {
                        w.resets_at = old.resets_at;
                    }
                    if w.label.is_empty() {
                        w.label = old.label.clone();
                    }
                    *old = w;
                }
                None => self.windows.push(w),
            }
        }
    }
}

/// Today's date here, as the usage is kept by.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// A model's family, which per-model meters and stickiness go by:
/// "claude-opus-5-5" is "opus", "gpt-6-astra[1m]" is "gpt-6-astra".
pub fn model_family(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    let m = m.split('[').next().unwrap_or("").trim();
    for family in ["opus", "sonnet", "haiku", "fable"] {
        if m.contains(family) {
            return family.to_owned();
        }
    }
    // Drop a trailing date ("-20250929").
    let parts: Vec<&str> = m.split('-').collect();
    match parts.split_last() {
        Some((last, rest)) if !rest.is_empty() && last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()) => rest.join("-"),
        _ => m.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    /// An account with a credit expiring at `expires` and a weekly window
    /// `used` percent used, resetting at `resets` (so its start, and its
    /// rate, follow from its week).
    fn holding(expires: i64, used: f64, resets: i64) -> Account {
        let mut a = Account::new("a".into(), "a".into(), Provider::Claude);
        a.credits = vec![ResetCredit { id: "c".into(), expires_at: Some(expires), remaining: 1, usable: true, ..Default::default() }];
        a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used, resets_at: Some(resets), models: Vec::new() }];
        a
    }

    #[test]
    fn credits_are_spent_before_they_expire() {
        let t = 1_000_000;
        // Claude's: its week 4 days in, 90% used (22.5% a day), resetting
        // in 3 days; the credit goes in 20 hours. A refill lets 57.5% more
        // through (the reset day stays) from now until it runs out (what's
        // to come and what's left shrink alike), less after: it waits for
        // that. Used up already, 4 days in: now.
        let a = holding(t + 20 * H, 90.0, t + 3 * 24 * H);
        assert!(!a.spend_before_expiry(t));
        let out = holding(t + 20 * H, 100.0, t + 3 * 24 * H);
        assert!(out.spend_before_expiry(t));
        // 65% used (16.25% a day): it wants 48.75% more where 35% is left, so
        // a refill lets 13.75% more through, now or at any time before it
        // runs out: it waits, to its last chance (by then 78.2% used).
        let e = holding(t + 20 * H, 65.0, t + 3 * 24 * H);
        assert!(!e.spend_before_expiry(t));
        let last = holding(t + 20 * H, 78.2, t + 3 * 24 * H);
        assert!(last.spend_before_expiry(t + 20 * H - EXPIRY_MARGIN));
        // Its week resets by itself in 5 hours, and the next has room for
        // all it will use: a refill gives nothing, then or after.
        let b = holding(t + 20 * H, 90.0, t + 5 * H);
        assert!(!b.spend_before_expiry(t));
        assert!(!b.spend_before_expiry(t + 20 * H - EXPIRY_MARGIN));
        // Hardly used: not worth it even at the end.
        let c = holding(t + 20 * H, 2.0, t + 24 * H);
        assert!(!c.spend_before_expiry(t + 20 * H - EXPIRY_MARGIN));
        // Plenty of time left.
        let d = holding(t + 4 * 24 * H, 90.0, t + 3 * 24 * H);
        assert!(!d.spend_before_expiry(t));
    }

    /// What a reset gives back, by each provider's rule: Claude's what can
    /// be used before the window's own reset; Codex's what's used less the
    /// part of the week gone; next to nothing just before a reset.
    #[test]
    fn a_reset_is_worth_what_it_gives_back() {
        let t = 1_000_000;
        let week = |used: f64, resets: i64| Window { name: "seven_day".into(), label: "Weekly".into(), used, resets_at: Some(resets), models: vec![] };
        // Used up a day into its week: a Claude refill has 6 days to use at
        // 100% a day (all of it); Codex's new week loses the day gone.
        let w = week(100.0, t + 6 * 24 * H);
        assert!((w.reset_worth(Provider::Claude, t, t) - 100.0).abs() < 0.01);
        assert!((w.reset_worth(Provider::Codex, t, t) - 100.0 * 6.0 / 7.0).abs() < 0.01);
        // Used up with half an hour of its week left: next to nothing.
        let late = week(100.0, t + H / 2);
        assert!(late.reset_worth(Provider::Claude, t, t) < WORTH_REDEEMING);
        assert!(late.reset_worth(Provider::Codex, t, t) < WORTH_REDEEMING);
    }

    /// Claimed where it gives back most: a Fable-only reset for a used-up
    /// Fable week before the November one that clears the whole week too;
    /// that one for a used-up week; none for a week about to reset itself.
    #[test]
    fn the_narrowest_credit_that_clears_what_is_out() {
        let t = 1_000_000;
        let mut a = Account::new("a".into(), "a".into(), Provider::Claude);
        let fable = Window { name: "seven_day_fable".into(), label: "Weekly Fable".into(), used: 100.0, resets_at: Some(t + 3 * 24 * H), models: vec!["fable".into()] };
        let week = Window { name: "seven_day".into(), label: "Weekly".into(), used: 50.0, resets_at: Some(t + 3 * 24 * H), models: vec![] };
        a.windows = vec![week.clone(), fable.clone()];
        let grant = |id: &str, clears: &[&str], days: i64| ResetCredit {
            id: id.into(),
            expires_at: Some(t + days * 24 * H),
            remaining: 1,
            usable: true,
            clears: clears.iter().map(|c| (*c).to_owned()).collect(),
            needs_limit: false,
            ..Default::default()
        };
        a.credits = vec![grant("november", &["seven_day", "seven_day_overage_included"], 21), grant("fable", &["seven_day_overage_included"], 109)];
        assert_eq!(a.claim_for("fable", t).map(|(c, _)| c.id.as_str()), Some("fable"));
        // One that gives the week back too (as a real one did) clears as
        // much as November's: the sooner to expire of the two, then.
        a.credits[1].gives_back_weekly = true;
        assert!(a.credits[1].clears(Provider::Claude, &week));
        assert_eq!(a.claim_for("fable", t).map(|(c, _)| c.id.as_str()), Some("november"));
        a.credits[1].gives_back_weekly = false;
        // Nothing out for Opus: no claim.
        assert!(a.claim_for("opus", t).is_none());
        // The week used up: only November's clears it; and for Fable too,
        // which both weeks now stop (a Fable-only reset would leave it
        // stopped: Codex's review reproduced it).
        a.windows[0].used = 100.0;
        assert_eq!(a.claim_for("opus", t).map(|(c, _)| c.id.as_str()), Some("november"));
        assert_eq!(a.claim_for("fable", t).map(|(c, _)| c.id.as_str()), Some("november"));
        // Its week resets in 20 minutes: better waited for.
        a.windows[0].resets_at = Some(t + 20 * 60);
        assert!(a.claim_for("opus", t).is_none());
        // Codex: any credit resets its week; the soonest to expire first.
        let mut c = Account::new("c".into(), "c".into(), Provider::Codex);
        c.windows = vec![Window { name: "secondary_window".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(t + 5 * 24 * H), models: vec![] }];
        c.credits = vec![grant("later", &[], 24), grant("sooner", &[], 12)];
        assert_eq!(c.claim_for("gpt-6-astra", t).map(|(c, _)| c.id.as_str()), Some("sooner"));
    }

    #[test]
    fn families() {
        assert_eq!(model_family("claude-opus-5-5"), "opus");
        assert_eq!(model_family("claude-sonnet-4-5-20250929"), "sonnet");
        assert_eq!(model_family("gpt-6-astra[1m]"), "gpt-6-astra");
        assert_eq!(model_family("gpt-5-codex-20250915"), "gpt-5-codex");
        assert_eq!(model_family(""), "");
    }

    #[test]
    fn merging_keeps_known_resets() {
        let mut a = Account::new("a".into(), "a".into(), Provider::Claude);
        // A window's length by what it's called (Codex's primary can be
        // a week), else by its name.
        let w = |name: &str, label: &str| Window { name: name.into(), label: label.into(), used: 0.0, resets_at: None, models: vec![] };
        assert_eq!(w("primary_window", "Weekly").length(), Some(7 * 86400));
        assert_eq!(w("x/primary_window", "GPT-5.3-Codex-Spark 5-hour").length(), Some(5 * 3600));
        assert_eq!(w("primary_window", "3-day").length(), Some(3 * 86400));
        assert_eq!(w("seven_day_fable", "").length(), Some(7 * 86400));
        a.merge_windows(vec![Window { name: "five_hour".into(), label: "5-hour".into(), used: 10.0, resets_at: Some(500), models: vec![] }]);
        a.merge_windows(vec![Window { name: "five_hour".into(), label: String::new(), used: 30.0, resets_at: None, models: vec![] }]);
        assert_eq!(a.windows, vec![Window { name: "five_hour".into(), label: "5-hour".into(), used: 30.0, resets_at: Some(500), models: vec![] }]);
    }

    #[test]
    fn credentials_never_print() {
        let c = Credential { access: "sekrit".into(), refresh: Some("sekrit2".into()), expires_at: Some(5), account_id: None };
        assert!(!format!("{c:?}").contains("sekrit"));
    }
}
