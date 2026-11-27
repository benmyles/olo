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
    /// Whether this window is an account-wide plan allowance meter (not
    /// model-scoped, not a surface meter, and not extra-usage spend).
    pub fn is_shared(&self) -> bool {
        self.models.is_empty() && self.name != "overage" && self.name != "seven_day_overage_included"
    }

    /// Model families this window is scoped to (empty for shared plan
    /// meters and extra-usage spend). Claude's `seven_day_overage_included`
    /// (`7d_oi` header) is Fable's own weekly meter.
    pub fn scoped_models(&self) -> Vec<String> {
        if !self.models.is_empty() {
            return self.models.clone();
        }
        if self.name == "seven_day_overage_included" {
            return vec!["fable".to_owned()];
        }
        Vec::new()
    }

    pub fn applies_to(&self, family: &str) -> bool {
        if self.name == "overage" {
            return false;
        }
        if self.name == "seven_day_overage_included" {
            return family == "fable";
        }
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
        (elapsed >= 600).then_some(self.used / elapsed as f64)
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
            return w.is_shared();
        }
        // (Fable's week: `seven_day_overage_included` is a weekly limit,
        // never a 5-hour one.)
        let weekly = w.name.starts_with("seven_day") || w.name.ends_with("/seven_day");
        let fable_week = weekly && (w.models.iter().any(|m| m == "fable") || w.name.contains("fable"));
        (self.gives_back_weekly && w.name == "seven_day") || self.clears.iter().any(|c| *c == w.name || (fable_week && c == "seven_day_overage_included"))
    }

    /// Whether this credit can restore allowance for requests of `family`
    /// (empty `family`: any model). A Fable-only credit does not apply to Opus.
    pub fn applies_to_family(&self, provider: Provider, family: &str) -> bool {
        if family.is_empty() || provider == Provider::Codex || self.clears.is_empty() || self.gives_back_weekly {
            return true;
        }
        self.clears.iter().any(|c| {
            c == "five_hour"
                || c == "seven_day"
                || (c == "seven_day_overage_included" && family == "fable")
                || c.strip_prefix("seven_day_").or_else(|| c.strip_prefix("five_hour_")).is_some_and(|m| m == family)
        })
    }

    /// How many limits it clears (giving the week back counts as one).
    pub fn breadth(&self) -> usize {
        self.clears.len() + usize::from(self.gives_back_weekly && !self.clears.iter().any(|c| c == "seven_day"))
    }

    /// Usable at `now`: started, not expired, some left.
    pub fn usable_at(&self, now: i64) -> bool {
        self.usable && self.remaining > 0 && self.starts_at.is_none_or(|s| s <= now) && self.expires_at.is_none_or(|e| e > now)
    }

    /// Usable once a limit it clears is reached: already usable now, or
    /// waiting for a limit (`use_requires_limit`, with `unavailable_reason`
    /// either `"needs_limit"` or absent — matching Claude Code's `Gr()`,
    /// while rejecting explicit blockers like `"cooldown"` or `"unavailable"`).
    pub fn usable_at_limit(&self, now: i64) -> bool {
        let ready = self.usable || (self.needs_limit && matches!(self.unavailable_reason.as_deref(), None | Some("needs_limit")));
        ready && self.remaining > 0 && self.starts_at.is_none_or(|s| s <= now) && self.expires_at.is_none_or(|e| e > now)
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
    /// Why the provider says it cannot be used right now (Claude's
    /// `unavailable_reason`: e.g. `"needs_limit"` before a limit is hit).
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    /// It gives back the week's use too (Claude's `gives_back_weekly`): a
    /// Fable reset claimed on 2026-11-19 took its account's week from 50%
    /// to 1% with its Fable week, though it `clears` Fable's alone.
    #[serde(default)]
    pub gives_back_weekly: bool,
}

/// An account answered from usage credits (`Account::paid`): until
/// when, and the meter that ran out (its window's name: what the
/// provider's usage report is looked in for, to see it has come back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaidFor {
    pub until: i64,
    pub meter: String,
}

/// Unexpired exhaustion evidence observed by a borrower pool for one of a
/// keeper's accounts, carried in `Ask::Redeem` so the keeper never mistakes an
/// account refused on a sparse/unlisted meter (e.g. `five_hour`) for ready.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObservedLimit {
    pub account: String,
    /// Last keeper `reset_id` known to the borrower for this account before
    /// observing the limit (causal token, immune to cross-machine clock skew).
    #[serde(default)]
    pub last_reset_id: Option<String>,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub cooldown_until: Option<i64>,
    #[serde(default)]
    pub model_cooldown: Option<(String, i64)>,
    #[serde(default)]
    pub paid: Option<(String, bool, i64, String)>,
}

/// Proven scope of a completed reset on a keeper, attached to `AlreadyReady`
/// so a borrower catching up on a concurrent keeper reset clears ONLY the
/// windows/models/cooldowns that reset actually cleared (never unrelated ones).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClearedScope {
    pub reset_id: String,
    #[serde(default)]
    pub cleared_shared: bool,
    #[serde(default)]
    pub cleared_models: Vec<String>,
    #[serde(default)]
    pub cleared_windows: Vec<String>,
}

/// Distinct result returned by a keeper pool for a delegated reset request:
/// never conflates an actual credit redemption with an already-ready account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RemoteClaim {
    Redeemed {
        account: String,
        credit_id: String,
        #[serde(default)]
        reset_id: Option<String>,
        #[serde(default)]
        cleared_shared: bool,
        #[serde(default)]
        cleared_models: Vec<String>,
        #[serde(default)]
        cleared_windows: Vec<String>,
        #[serde(default)]
        windows: Vec<Window>,
        #[serde(default)]
        credits: Vec<ResetCredit>,
    },
    AlreadyReady {
        account: String,
        #[serde(default)]
        last_reset: Option<ClearedScope>,
        #[serde(default)]
        windows: Vec<Window>,
        #[serde(default)]
        credits: Vec<ResetCredit>,
    },
}

impl RemoteClaim {
    pub fn account(&self) -> &str {
        match self {
            RemoteClaim::Redeemed { account, .. } | RemoteClaim::AlreadyReady { account, .. } => account,
        }
    }
}

/// Durable keeper-side record of a delegated claim operation (`op_id`),
/// committed atomically with `a.redeeming` before upstream dispatch so a
/// borrower retrying after a lost SSH reply or keeper crash/500 receives the
/// original outcome (or triggers reconciliation) without spending a second
/// reset credit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteClaimRecord {
    pub op_id: String,
    pub at: i64,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub result: Option<RemoteClaim>,
    #[serde(default)]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Durable borrower-side record of an in-flight/unclear delegated claim
/// operation (`op_id`), persisted so the borrower reconciles that exact op with
/// its keeper before spending any other reset credit or falling back to paid
/// usage credits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRemoteClaim {
    pub provider: Provider,
    pub keeper: String,
    pub op_id: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub credit: Option<String>,
    #[serde(default)]
    pub family: String,
    pub at: i64,
    pub tried_at: i64,
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
    /// Its own allowance is out and its provider goes on answering it
    /// from usage credits (Claude's "overage": paid for beyond the
    /// plan), until the meter that ran out resets. Not chosen while
    /// another account can serve or a reset credit can be claimed: the
    /// last the pool turns to (`schedule::pick_paid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid: Option<PaidFor>,
    /// The same, for one model family (its own meter ran out).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_paid: BTreeMap<String, PaidFor>,
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
    /// Where its sign-in is kept, if not here: the pool that renews it
    /// (a Rex host's, by the host's id; or, on a host, the name of the
    /// pool that lent it). This pool then never renews it itself, and
    /// asks the keeper for a newer one (`gateway::hosted`): a
    /// provider's refresh token is spent once, by one pool.
    #[serde(default)]
    pub kept_on: Option<String>,
    /// Kept here for a while for one host alone (by its id), and since when
    /// (seconds): the sign-ins of a host whose account is moving, taken
    /// back from it, for the host it moves to, never handed to another
    /// meanwhile (Rex's move; a day at most, or until that host's pool is
    /// gone).
    #[serde(default)]
    pub kept_for: Option<(String, i64)>,
    /// Set when a host-kept account is re-signed-in on the desktop; blocks
    /// local claiming until that host explicitly confirms handover or is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reauth_pending_host: Option<String>,
}

impl Account {
    /// Whether `c` can be redeemed manually right now: either already usable
    /// now, or a `needs_limit` credit whose cleared window is currently exhausted.
    pub fn credit_claimable_now(&self, c: &ResetCredit, now: i64) -> bool {
        c.usable_at(now) || (c.usable_at_limit(now) && self.windows.iter().any(|w| c.clears(self.provider, w) && w.exhausted(now)))
    }

    /// The best credit to redeem manually at `now`: one that is claimable now
    /// (including `needs_limit` credits when a cleared window is exhausted),
    /// giving back most now, then narrowest, then soonest to expire. Shared by
    /// the UI menu/confirmation and `redeem_locked(Why::Asked)`.
    pub fn manual_credit(&self, now: i64) -> Option<&ResetCredit> {
        self.credits
            .iter()
            .filter(|c| self.credit_claimable_now(c, now))
            .map(|c| (c, self.claim_worth(c, now, now)))
            .max_by(|x, y| x.1.total_cmp(&y.1).then_with(|| (y.0.breadth(), y.0.expires_at.unwrap_or(i64::MAX)).cmp(&(x.0.breadth(), x.0.expires_at.unwrap_or(i64::MAX)))))
            .map(|(c, _)| c)
    }

    /// The credit to use next by hand (for the UI menu and confirmation).
    pub fn credit(&self) -> Option<&ResetCredit> {
        self.manual_credit(now())
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
        self.claims_for(family, now).into_iter().next()
    }

    /// Every credit that would let a request of `family` through now, as
    /// `claim_for` ranks them (the first is its choice): one refused, the
    /// next.
    pub fn claims_for(&self, family: &str, now: i64) -> Vec<(&ResetCredit, f64)> {
        let out: Vec<&Window> = self.windows.iter().filter(|w| w.applies_to(family) && w.exhausted(now)).collect();
        if out.is_empty() {
            return Vec::new();
        }
        // (One that clears every limit that's out for it: one that leaves
        // another standing lets the request through no more than before.)
        let mut all: Vec<(&ResetCredit, f64)> = self
            .credits
            .iter()
            .filter(|c| c.usable_at_limit(now) && out.iter().all(|w| c.clears(self.provider, w)))
            .map(|c| (c, out.iter().filter(|w| c.clears(self.provider, w)).map(|w| w.reset_worth(self.provider, now, now)).fold(0.0, f64::max)))
            .filter(|(_, worth)| *worth >= WORTH_REDEEMING)
            .collect();
        all.sort_by_key(|(c, _)| (c.breadth(), c.expires_at.unwrap_or(i64::MAX)));
        all
    }

    /// A credit about to expire unused (within `EXPIRY_LOOKAHEAD`) to
    /// claim now: now is its best moment of those left (each hour until 30
    /// minutes before it expires, and that last chance), by what a claim
    /// then gives back (`claim_worth`), and that's at least
    /// `WORTH_REDEEMING`. A later moment as good wins (it keeps the
    /// choice open).
    pub fn expiring_claim(&self, now: i64) -> Option<&ResetCredit> {
        // (Each about to expire weighed: one worth nothing never hides
        // another worth claiming; of those due now, the one that gives back
        // most, the soonest to expire of those as good.)
        self.credits
            .iter()
            .filter(|c| c.expires_at.is_some_and(|e| e - now <= EXPIRY_LOOKAHEAD))
            .filter_map(|c| {
                // (One that only works at a limit, only when one it clears is out.)
                let ready = if c.needs_limit { self.windows.iter().any(|w| c.clears(self.provider, w) && w.exhausted(now)) && c.usable_at_limit(now) } else { c.usable_at(now) };
                if !ready {
                    return None;
                }
                let last = c.expires_at? - EXPIRY_MARGIN;
                let here = self.claim_worth(c, now, now);
                let later = (1..).map(|h| now + h * 3600).take_while(|&t| t < last).chain(std::iter::once(last)).filter(|&t| t > now).map(|t| self.claim_worth(c, now, t)).fold(0.0, f64::max);
                (here >= WORTH_REDEEMING && (here > later || now >= last)).then_some((c, here))
            })
            .max_by(|(x, a), (y, b)| a.total_cmp(b).then_with(|| y.expires_at.cmp(&x.expires_at)))
            .map(|(c, _)| c)
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
            paid: None,
            model_paid: BTreeMap::new(),
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
            kept_on: None,
            kept_for: None,
            reauth_pending_host: None,
        }
    }

    /// The host this account is kept here for, if it still is (a day at
    /// most).
    pub fn kept_for_host(&self, now: i64) -> Option<&str> {
        self.kept_for.as_ref().filter(|(_, since)| now - since < 24 * 3600).map(|(h, _)| h.as_str())
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

    /// Until when its provider answers `family`'s requests from usage
    /// credits (`paid`, the account's or the family's own), if it does
    /// now.
    pub fn paid_until(&self, family: &str, now: i64) -> Option<i64> {
        let live = |p: &PaidFor| (p.until > now).then_some(p.until);
        self.paid.as_ref().and_then(live).into_iter().chain(self.model_paid.get(family).and_then(live)).max()
    }

    /// It is answered from usage credits from now (what an answer's
    /// headers said: `upstream::Paid`), the account or `family` alone.
    pub fn now_paid(&mut self, family: &str, model_only: bool, until: i64, meter: &str) {
        let mark = PaidFor { until, meter: meter.to_owned() };
        if model_only {
            self.model_paid.insert(family.to_owned(), mark);
        } else {
            self.paid = Some(mark);
        }
    }

    /// The marks of being answered from usage credits that no longer
    /// hold, put away: those run out (`now`), and those whose meter the
    /// provider's own report (`reported`: the account's windows as just
    /// read) lists as not used up, an allowance come back (a reset
    /// credit claimed by another pool of the same accounts, or by the
    /// person). A meter the report doesn't list is left to its time.
    pub fn paid_settled(&mut self, reported: Option<&[Window]>, now: i64) {
        let holds = |p: &PaidFor| p.until > now && !reported.is_some_and(|ws| ws.iter().any(|w| w.name == p.meter && !w.exhausted(now)));
        if self.paid.as_ref().is_some_and(|p| !holds(p)) {
            self.paid = None;
        }
        self.model_paid.retain(|_, p| holds(p));
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

    /// Of two credits due before they expire, the one that gives back most
    /// (Codex's review: the first due won, one worth less): a reset of the
    /// used-up week over one of the 5-hour window, half used.
    #[test]
    fn the_expiring_credit_worth_most() {
        let t = 1_000_000;
        let mut a = holding(t + 20 * H, 100.0, t + 3 * 24 * H);
        a.windows.push(Window { name: "five_hour".into(), label: "5-hour".into(), used: 50.0, resets_at: Some(t + 4 * H), models: Vec::new() });
        a.credits = vec![
            ResetCredit { id: "hour".into(), expires_at: Some(t + EXPIRY_MARGIN), remaining: 1, usable: true, clears: vec!["five_hour".into()], ..Default::default() },
            ResetCredit { id: "week".into(), expires_at: Some(t + EXPIRY_MARGIN + H), remaining: 1, usable: true, clears: vec!["seven_day".into()], ..Default::default() },
        ];
        assert_eq!(a.expiring_claim(t).map(|c| c.id.as_str()), Some("week"));
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

    /// Claude grants with `use_requires_limit: true` report `usable_now: false`
    /// and `unavailable_reason: "needs_limit"` before 100% is reached. Once a
    /// cleared window is exhausted they must be eligible for automatic claiming
    /// (and counted as more-to-draw / expiring), while genuinely blocked grants
    /// (`cooldown`, `unavailable`) must stay ineligible.
    #[test]
    fn needs_limit_credits_are_ready_at_limit_and_not_while_on_cooldown() {
        let t = now();
        let mut a = Account::new("a".into(), "a".into(), Provider::Claude);
        a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(t + 3 * 24 * H), models: vec![] }];
        let mk = |id: &str, reason: Option<&str>, expires: i64| ResetCredit {
            id: id.into(),
            expires_at: Some(expires),
            remaining: 1,
            usable: false,
            needs_limit: true,
            unavailable_reason: reason.map(str::to_owned),
            clears: vec!["seven_day".into()],
            ..Default::default()
        };
        let waiting = mk("waiting", Some("needs_limit"), t + 20 * H);
        let legacy = mk("legacy", None, t + 30 * H);
        let cooling = mk("cooling", Some("cooldown"), t + 10 * H);
        let unavail = mk("unavail", Some("unavailable"), t + 10 * H);
        assert!(!waiting.usable_at(t));
        assert!(waiting.usable_at_limit(t));
        assert!(legacy.usable_at_limit(t), "omitted reason with needs_limit matches Claude Code Gr()");
        assert!(!cooling.usable_at_limit(t));
        assert!(!unavail.usable_at_limit(t));

        a.credits = vec![cooling, unavail, legacy, waiting];
        assert_eq!(a.claim_for("opus", t).map(|(c, _)| c.id.as_str()), Some("waiting"));
        assert_eq!(a.manual_credit(t).map(|c| c.id.as_str()), Some("waiting"));
        assert_eq!(a.expiring_claim(t).map(|c| c.id.as_str()), Some("waiting"));
        // Not at limit yet: `needs_limit` credit is neither auto-claimed nor manually claimable.
        a.windows[0].used = 80.0;
        assert!(a.claim_for("opus", t).is_none());
        assert!(a.manual_credit(t).is_none());
        assert!(a.expiring_claim(t).is_none());
        // Still counted as available headroom for Opus / meters while waiting for limit.
        assert!(crate::told::more(std::slice::from_ref(&a), Provider::Claude, "opus", Some("a"), t, |_| true));
        // A Fable-only credit must NOT report headroom for Opus in `told::more`.
        let mut fable_only = a.clone();
        fable_only.credits =
            vec![ResetCredit { id: "fable".into(), expires_at: Some(t + 20 * H), remaining: 1, usable: true, clears: vec!["seven_day_overage_included".into()], ..Default::default() }];
        assert!(!crate::told::more(std::slice::from_ref(&fable_only), Provider::Claude, "opus", Some("a"), t, |_| true));
        assert!(crate::told::more(std::slice::from_ref(&fable_only), Provider::Claude, "fable", Some("a"), t, |_| true));
        let mut pool = crate::gateway::Pool::default();
        pool.loaded = true;
        pool.saved.accounts.push(a);
        let m = crate::meters::of(&pool, &[], &crate::meters::Paces::default(), 1);
        assert_eq!(m.accounts[0].windows[0].resets, 2);
    }

    /// `overage` (extra-usage spend) never blocks plan allowance;
    /// `seven_day_overage_included` (`7d_oi` header) only applies to Fable,
    /// never Opus, and never prevents a `seven_day`-only reset from clearing Opus.
    #[test]
    fn overage_and_seven_day_oi_window_scopes() {
        let t = 1_000_000;
        let overage = Window { name: "overage".into(), label: "Overage".into(), used: 100.0, resets_at: Some(t + 3 * 24 * H), models: vec![] };
        let oi = Window { name: "seven_day_overage_included".into(), label: "Weekly Overage Included".into(), used: 100.0, resets_at: Some(t + 3 * 24 * H), models: vec![] };
        let week = Window { name: "seven_day".into(), label: "Weekly".into(), used: 100.0, resets_at: Some(t + 3 * 24 * H), models: vec![] };
        assert!(!overage.is_shared() && !overage.applies_to("opus") && !overage.applies_to("fable"));
        assert!(!oi.is_shared() && !oi.applies_to("opus") && oi.applies_to("fable") && oi.scoped_models() == ["fable"]);
        assert!(week.is_shared() && week.applies_to("opus") && week.applies_to("fable"));

        let mut a = Account::new("a".into(), "a".into(), Provider::Claude);
        a.windows = vec![overage, oi, week];
        a.credits = vec![ResetCredit { id: "weekly-only".into(), expires_at: Some(t + 10 * 24 * H), remaining: 1, usable: true, clears: vec!["seven_day".into()], ..Default::default() }];
        // Opus is blocked only by `seven_day` (not `overage` or `7d_oi`), so `weekly-only` clears it.
        assert_eq!(a.claim_for("opus", t).map(|(c, _)| c.id.as_str()), Some("weekly-only"));
        // Fable is also blocked by `seven_day_overage_included`, which `weekly-only` does not clear.
        assert!(a.claim_for("fable", t).is_none());
    }
}
