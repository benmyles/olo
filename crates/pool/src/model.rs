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
}

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

/// A provider-issued credit that resets an account's quota early (some
/// subscriptions get a few).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
