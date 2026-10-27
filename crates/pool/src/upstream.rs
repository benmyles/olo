//! What the providers expect and report: where requests go, how a
//! subscription's sign-in is sent and refreshed, and how quota comes back
//! (the usage APIs, and every response's rate-limit headers).

use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use crate::model::{Credential, Provider, ResetCredit, Window, now};

/// The providers' addresses (tests point them at fakes).
#[derive(Debug, Clone)]
pub struct Upstreams {
    /// Claude's API (`/v1/messages` and the usage report).
    pub claude: String,
    /// Codex's Responses API on ChatGPT.
    pub codex: String,
    /// ChatGPT's backend (the usage report).
    pub chatgpt: String,
    pub claude_oauth: String,
    pub codex_oauth: String,
}

impl Default for Upstreams {
    fn default() -> Self {
        Upstreams {
            claude: "https://api.anthropic.com".into(),
            codex: "https://chatgpt.com/backend-api/codex".into(),
            chatgpt: "https://chatgpt.com/backend-api".into(),
            claude_oauth: "https://platform.claude.com/v1/oauth/token".into(),
            codex_oauth: "https://auth.openai.com/oauth/token".into(),
        }
    }
}

impl Upstreams {
    /// The real providers, or one fake standing in for all of them
    /// (`OLO_POOL_UPSTREAM`, for tests): its `/oauth` takes refreshes.
    pub fn from_env() -> Upstreams {
        match std::env::var("OLO_POOL_UPSTREAM") {
            Ok(base) if !base.is_empty() => {
                let base = base.trim_end_matches('/').to_owned();
                Upstreams { claude: base.clone(), codex: base.clone(), chatgpt: base.clone(), claude_oauth: format!("{base}/oauth"), codex_oauth: format!("{base}/oauth") }
            }
            _ => Upstreams::default(),
        }
    }
}

/// The official clients' OAuth client ids, which their sign-ins belong to.
const CLAUDE_CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CODEX_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Claude's API takes subscription (OAuth) sign-ins with this beta.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Send `c` with `req`.
pub fn authorize(req: RequestBuilder, p: Provider, c: &Credential) -> RequestBuilder {
    let req = req.bearer_auth(&c.access);
    match (p, &c.account_id) {
        (Provider::Codex, Some(id)) => req.header("chatgpt-account-id", id),
        _ => req,
    }
}

/// The client's `anthropic-beta` list with the OAuth beta added.
pub fn with_oauth_beta(client: Option<&str>) -> String {
    match client.map(str::trim).filter(|s| !s.is_empty()) {
        Some(list) if list.split(',').any(|b| b.trim() == OAUTH_BETA) => list.to_owned(),
        Some(list) => format!("{list},{OAUTH_BETA}"),
        None => OAUTH_BETA.to_owned(),
    }
}

/// Why talking to a provider failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// It refused the sign-in: it must be signed in again.
    Refused(String),
    /// Anything else (network, a server error): try again later.
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(s) | Failure::Other(s) => f.write_str(s),
        }
    }
}

async fn http_failure(what: &str, r: reqwest::Response) -> Failure {
    let status = r.status();
    // The body can explain, but never holds a secret worth keeping; cut it short.
    let body: String = r.text().await.unwrap_or_default().chars().take(200).collect();
    let msg = format!("{what}: HTTP {}{}", status.as_u16(), if body.is_empty() { String::new() } else { format!(" ({body})") });
    if status == 400 || status == 401 || status == 403 { Failure::Refused(msg) } else { Failure::Other(msg) }
}

/// A fresh sign-in from `c`'s refresh token (providers rotate it: keep
/// the new one).
pub async fn refresh(client: &Client, up: &Upstreams, p: Provider, c: &Credential) -> Result<Credential, Failure> {
    let Some(token) = c.refresh.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Err(Failure::Refused("No refresh token: sign in again.".into()));
    };
    let (url, id) = match p {
        Provider::Claude => (&up.claude_oauth, CLAUDE_CLIENT),
        Provider::Codex => (&up.codex_oauth, CODEX_CLIENT),
    };
    let r = client
        .post(url)
        .timeout(Duration::from_secs(30))
        .json(&json!({"grant_type": "refresh_token", "refresh_token": token, "client_id": id}))
        .send()
        .await
        .map_err(|e| Failure::Other(format!("Refreshing the sign-in: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Refreshing the sign-in", r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Refreshing the sign-in: {e}")))?;
    let access = d["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Failure::Other("Refreshing the sign-in: no token came back".into()))?;
    let expires_at = d["expires_in"].as_i64().map(|s| now() + s).or_else(|| jwt_exp(access));
    Ok(Credential {
        access: access.to_owned(),
        refresh: d["refresh_token"].as_str().filter(|t| !t.is_empty()).map(str::to_owned).or_else(|| c.refresh.clone()),
        expires_at,
        account_id: c.account_id.clone(),
    })
}

/// A JWT's claims (unverified: only to read our own token's expiry and
/// email).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let b = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&b).ok()
}

pub fn jwt_exp(token: &str) -> Option<i64> {
    jwt_claims(token)?["exp"].as_i64()
}

/// A time as the providers write it: seconds, milliseconds, or RFC 3339.
pub fn timestamp(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| if f > 1e11 { (f / 1000.0) as i64 } else { f as i64 }),
        Value::String(s) => s.parse::<f64>().ok().map(|f| f as i64).or_else(|| chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp())),
        _ => None,
    }
}

/// "seven_day_opus" for people.
fn claude_label(name: &str) -> String {
    match name {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        _ => {
            let (base, rest) = if let Some(r) = name.strip_prefix("seven_day_") {
                ("Weekly", r)
            } else if let Some(r) = name.strip_prefix("five_hour_") {
                ("5-hour", r)
            } else {
                return name.replace('_', " ");
            };
            let mut c = rest.chars();
            let model: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
            format!("{base} {}", model.replace('_', " "))
        }
    }
}

/// The model families a Claude window is for ("seven_day_opus": Opus).
fn claude_models(name: &str) -> Vec<String> {
    let rest = name.strip_prefix("seven_day_").or_else(|| name.strip_prefix("five_hour_"));
    match rest {
        Some(m) if !m.is_empty() && !m.contains("oauth") && !m.contains("overage") && !m.contains("apps") => vec![m.to_owned()],
        _ => Vec::new(),
    }
}

/// Codex's windows are named by their length.
fn codex_label(kind: &str, seconds: Option<i64>) -> String {
    match seconds {
        Some(18_000) => "5-hour".into(),
        Some(604_800) => "Weekly".into(),
        Some(s) if s % 86_400 == 0 => format!("{}-day", s / 86_400),
        Some(s) if s % 3_600 == 0 => format!("{}-hour", s / 3_600),
        _ => if kind == "primary" { "Primary".into() } else { "Secondary".into() },
    }
}

/// The quota windows in a usage report.
pub fn parse_usage(p: Provider, d: &Value) -> (Vec<Window>, Option<String>) {
    let mut out = Vec::new();
    match p {
        Provider::Claude => {
            if let Some(map) = d.as_object() {
                for (name, w) in map {
                    if !(name.starts_with("five_hour") || name.starts_with("seven_day")) {
                        continue;
                    }
                    let Some(used) = w["utilization"].as_f64() else { continue };
                    out.push(Window { name: name.clone(), label: claude_label(name), used, resets_at: timestamp(&w["resets_at"]), models: claude_models(name) });
                }
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            (out, None)
        }
        Provider::Codex => {
            for kind in ["primary", "secondary"] {
                let w = &d["rate_limit"][format!("{kind}_window")];
                let Some(used) = w["used_percent"].as_f64() else { continue };
                let resets_at = timestamp(&w["reset_at"]).or_else(|| w["reset_after_seconds"].as_i64().map(|s| now() + s));
                out.push(Window { name: format!("{kind}_window"), label: codex_label(kind, w["limit_window_seconds"].as_i64()), used, resets_at, models: vec![] });
            }
            (out, d["plan_type"].as_str().map(str::to_owned))
        }
    }
}

/// Read a subscription's quota from its provider.
pub async fn usage(client: &Client, up: &Upstreams, p: Provider, c: &Credential) -> Result<(Vec<Window>, Option<String>), Failure> {
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage", up.claude),
        Provider::Codex => format!("{}/wham/usage", up.chatgpt),
    };
    let mut req = authorize(client.get(url), p, c).timeout(Duration::from_secs(20));
    if p == Provider::Claude {
        req = req.header("anthropic-beta", OAUTH_BETA);
    }
    let r = req.send().await.map_err(|e| Failure::Other(format!("Reading the quota: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading the quota", r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Reading the quota: {e}")))?;
    Ok(parse_usage(p, &d))
}

/// The reset credits in a provider's report (only ones it says the
/// account may use).
pub fn parse_credits(p: Provider, d: &Value) -> Vec<ResetCredit> {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    match p {
        Provider::Codex => list(&d["credits"])
            .iter()
            .filter(|r| r["status"] == "available" && r["is_supported_by_plan"] == true)
            .filter_map(|r| Some(ResetCredit { id: r["id"].as_str()?.to_owned(), expires_at: timestamp(&r["expires_at"]), remaining: 1, usable: true, program: None }))
            .collect(),
        Provider::Claude => {
            let program = &d["cedar_ember"];
            if program["eligible"] != true {
                return Vec::new();
            }
            list(&program["grants"])
                .iter()
                .filter(|r| r["resets_left"].as_u64().unwrap_or(0) > 0 && r["paused"] != true)
                .filter_map(|r| {
                    Some(ResetCredit {
                        id: r["id"].as_str()?.to_owned(),
                        expires_at: timestamp(&r["ends_at"]),
                        remaining: r["resets_left"].as_u64()?.min(u32::MAX as u64) as u32,
                        usable: r["usable_now"] == true,
                        program: Some("cedar_ember".into()),
                    })
                })
                .collect()
        }
    }
}

/// Read an account's reset credits.
pub async fn credits(client: &Client, up: &Upstreams, p: Provider, c: &Credential) -> Result<Vec<ResetCredit>, Failure> {
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage?cedar_ember=1&skip_spend=1", up.claude),
        Provider::Codex => format!("{}/wham/rate-limit-reset-credits", up.chatgpt),
    };
    let r = authorize(client.get(url), p, c).header("anthropic-beta", OAUTH_BETA).timeout(Duration::from_secs(20)).send().await.map_err(|e| Failure::Other(format!("Reading reset credits: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading reset credits", r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Reading reset credits: {e}")))?;
    Ok(parse_credits(p, &d))
}

/// Redeem `credit` (`request` identifies this redemption, so asking
/// again after an unclear outcome redeems it once).
pub async fn redeem(client: &Client, up: &Upstreams, p: Provider, c: &Credential, org: Option<&str>, credit: &ResetCredit, request: &str) -> Result<(), Failure> {
    let (url, body) = match p {
        Provider::Codex => (format!("{}/wham/rate-limit-reset-credits/consume", up.chatgpt), json!({"credit_id": credit.id, "redeem_request_id": request})),
        Provider::Claude => {
            let org = org.filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
            let org = org.ok_or_else(|| Failure::Other("This account's Claude organization isn't known: sign in again.".into()))?;
            let program = credit.program.as_deref().unwrap_or("cedar_ember");
            (format!("{}/api/organizations/{org}/reset_rate_limits", up.claude), json!({"program": program, "grant_id": credit.id, "request_id": request}))
        }
    };
    let r = authorize(client.post(url), p, c).header("anthropic-beta", OAUTH_BETA).timeout(Duration::from_secs(30)).json(&body).send().await.map_err(|e| Failure::Other(format!("Redeeming a reset credit: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Redeeming a reset credit", r).await);
    }
    if p == Provider::Claude {
        let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Redeeming a reset credit: {e}")))?;
        if d["result"] != "reset" && d["result"] != "already_used" {
            return Err(Failure::Other("Claude didn't confirm the reset.".into()));
        }
    }
    Ok(())
}

/// What a rejection (429) says: until when, and whether only this
/// model's own meter ran out.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    pub until: i64,
    pub model_only: bool,
}

fn header_num(h: &axum::http::HeaderMap, name: &str) -> Option<f64> {
    h.get(name)?.to_str().ok()?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// When to try again, from `retry-after` (seconds or an HTTP date).
pub fn retry_after(h: &axum::http::HeaderMap, now: i64) -> Option<i64> {
    let v = h.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(s) = v.parse::<i64>() {
        return Some(now + s.max(0));
    }
    chrono::DateTime::parse_from_rfc2822(v).ok().map(|t| t.timestamp())
}

/// The quota a response's headers report, and a rejection if it was one.
pub fn header_windows(h: &axum::http::HeaderMap, p: Provider, family: &str, status: u16, now: i64) -> (Vec<Window>, Option<Rejection>) {
    let mut out = Vec::new();
    match p {
        Provider::Claude => {
            for (name, kind) in [("five_hour", "5h"), ("seven_day", "7d"), ("seven_day_opus", "7d-opus"), ("seven_day_sonnet", "7d-sonnet"), ("seven_day_fable", "7d-fable")] {
                if let Some(fraction) = header_num(h, &format!("anthropic-ratelimit-unified-{kind}-utilization")) {
                    let resets_at = header_num(h, &format!("anthropic-ratelimit-unified-{kind}-reset")).map(|v| v as i64);
                    out.push(Window { name: name.into(), label: claude_label(name), used: fraction * 100.0, resets_at, models: claude_models(name) });
                }
            }
        }
        Provider::Codex => {
            for kind in ["primary", "secondary"] {
                if let Some(used) = header_num(h, &format!("x-codex-{kind}-used-percent")) {
                    let resets_at = header_num(h, &format!("x-codex-{kind}-reset-at"))
                        .map(|v| v as i64)
                        .or_else(|| header_num(h, &format!("x-codex-{kind}-reset-after-seconds")).map(|v| now + v as i64));
                    let minutes = header_num(h, &format!("x-codex-{kind}-window-minutes")).map(|m| m as i64 * 60);
                    // Without the length, keep the name the usage report gave.
                    let label = if minutes.is_some() { codex_label(kind, minutes) } else { String::new() };
                    out.push(Window { name: format!("{kind}_window"), label, used, resets_at, models: vec![] });
                }
            }
        }
    }
    if status != 429 {
        return (out, None);
    }
    // Claude says when one of the account's meters ran out
    // (`…-unified-status: rejected`). Any other 429 (a burst limit, or a
    // request it won't serve) belongs to the request, not the account:
    // it goes back to the client as it came, and the account stays in use.
    if p == Provider::Claude && h.get("anthropic-ratelimit-unified-status").and_then(|v| v.to_str().ok()) != Some("rejected") {
        return (out, None);
    }
    let reset = header_num(h, "anthropic-ratelimit-unified-reset").map(|v| v as i64).filter(|&v| v > now);
    let until = reset.or_else(|| retry_after(h, now)).unwrap_or(now + 60);
    // Claude names the meter that ran out; a per-model one leaves the
    // account's other models usable.
    let claim = h.get("anthropic-ratelimit-unified-representative-claim").and_then(|v| v.to_str().ok()).unwrap_or("");
    let model_only = p == Provider::Claude && !family.is_empty() && (claim == "seven_day_overage_included" || claude_models(claim).iter().any(|m| m == family));
    if model_only && claim == "seven_day_overage_included" {
        out.push(Window { name: format!("model:{family}/seven_day"), label: format!("Weekly {family}"), used: 100.0, resets_at: Some(until), models: vec![family.to_owned()] });
    }
    (out, Some(Rejection { until, model_only }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_reports() {
        let d = json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-27T22:00:00Z"},
            "seven_day": {"utilization": 18, "resets_at": 1793500000},
            "seven_day_opus": {"utilization": 100, "resets_at": 1793600000},
            "seven_day_oauth_apps": null,
            "extra_usage": {"utilization": 3},
        });
        let (w, _) = parse_usage(Provider::Claude, &d);
        let names: Vec<_> = w.iter().map(|w| (w.name.as_str(), w.label.as_str(), w.used)).collect();
        assert_eq!(names, [("five_hour", "5-hour", 42.0), ("seven_day", "Weekly", 18.0), ("seven_day_opus", "Weekly Opus", 100.0)]);
        assert_eq!(w[0].resets_at, Some(1793138400));
        assert_eq!(w[2].models, ["opus"]);
        let d = json!({"plan_type": "pro", "rate_limit": {
            "primary_window": {"used_percent": 25, "limit_window_seconds": 18000, "reset_at": 1793140000},
            "secondary_window": {"used_percent": 60, "limit_window_seconds": 604800, "reset_after_seconds": 100}}});
        let (w, plan) = parse_usage(Provider::Codex, &d);
        assert_eq!(plan.as_deref(), Some("pro"));
        assert_eq!((w[0].label.as_str(), w[0].resets_at), ("5-hour", Some(1793140000)));
        assert_eq!(w[1].label, "Weekly");
    }

    #[test]
    fn headers_and_rejections() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.5".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "5000".parse().unwrap());
        let (w, r) = header_windows(&h, Provider::Claude, "opus", 200, 1000);
        assert_eq!((w[0].used, w[0].resets_at, r), (50.0, Some(5000), None));
        h.insert("anthropic-ratelimit-unified-status", "rejected".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-representative-claim", "seven_day_overage_included".parse().unwrap());
        h.insert("retry-after", "600".parse().unwrap());
        let (w, r) = header_windows(&h, Provider::Claude, "fable", 429, 1000);
        assert_eq!(r, Some(Rejection { until: 1600, model_only: true }));
        assert!(w.iter().any(|w| w.models == ["fable"] && w.used >= 100.0));
        // A shared meter: the whole account waits.
        h.insert("anthropic-ratelimit-unified-representative-claim", "five_hour".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-reset", "7000".parse().unwrap());
        assert_eq!(header_windows(&h, Provider::Claude, "fable", 429, 1000).1, Some(Rejection { until: 7000, model_only: false }));
        // A 429 that isn't a meter running out: not the account's.
        let mut h = axum::http::HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
        h.insert("retry-after", "5".parse().unwrap());
        assert_eq!(header_windows(&h, Provider::Claude, "opus", 429, 1000).1, None);
        h.remove("anthropic-ratelimit-unified-status");
        assert_eq!(header_windows(&h, Provider::Claude, "opus", 429, 1000).1, None);
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-codex-primary-used-percent", "75".parse().unwrap());
        h.insert("x-codex-primary-reset-after-seconds", "30".parse().unwrap());
        let (w, _) = header_windows(&h, Provider::Codex, "gpt-5-codex", 200, 1000);
        assert_eq!((w[0].used, w[0].resets_at), (75.0, Some(1030)));
    }

    #[test]
    fn betas_and_tokens() {
        assert_eq!(with_oauth_beta(None), OAUTH_BETA);
        assert_eq!(with_oauth_beta(Some("a,b")), format!("a,b,{OAUTH_BETA}"));
        assert_eq!(with_oauth_beta(Some(&format!("{OAUTH_BETA},x"))), format!("{OAUTH_BETA},x"));
        let token = format!("h.{}.s", URL_SAFE_NO_PAD.encode(br#"{"exp":1793000000,"email":"a@example.com"}"#));
        assert_eq!(jwt_exp(&token), Some(1793000000));
        assert_eq!(timestamp(&json!(1793000000123u64)), Some(1793000000));
    }
}
