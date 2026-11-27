//! What the providers expect and report: where requests go, how a
//! subscription's sign-in is sent and refreshed, and how quota comes back
//! (the usage APIs, and every response's rate-limit headers).

use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use crate::likeness::{self, Like, What};
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
    /// Where the proxy sends what it passes through untouched, for every
    /// host (a fake in tests); None: the host itself, over HTTPS.
    pub origin: Option<String>,
}

impl Default for Upstreams {
    fn default() -> Self {
        Upstreams {
            claude: "https://api.anthropic.com".into(),
            codex: "https://chatgpt.com/backend-api/codex".into(),
            chatgpt: "https://chatgpt.com/backend-api".into(),
            claude_oauth: "https://platform.claude.com/v1/oauth/token".into(),
            codex_oauth: "https://auth.openai.com/oauth/token".into(),
            origin: None,
        }
    }
}

impl Upstreams {
    /// The real providers, or one fake standing in for all of them
    /// (`OLO_POOL_UPSTREAM`, for tests): its `/oauth` takes refreshes.
    pub fn from_env() -> Upstreams {
        match std::env::var("OLO_POOL_UPSTREAM") {
            Ok(base) if !base.is_empty() => Upstreams::fake(&base),
            _ => Upstreams::default(),
        }
    }

    /// One fake standing in for every provider and host.
    pub fn fake(base: &str) -> Upstreams {
        let base = base.trim_end_matches('/').to_owned();
        Upstreams {
            claude: base.clone(),
            codex: format!("{base}/backend-api/codex"),
            chatgpt: format!("{base}/backend-api"),
            claude_oauth: format!("{base}/oauth"),
            codex_oauth: format!("{base}/oauth"),
            origin: Some(base),
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
    /// It refused the sign-in (or refused a claim outright without spending it).
    Refused(String),
    /// Could not be built or validated locally: nothing was sent over the network.
    NotSent(String),
    /// Anything else (network, a server error): try again later.
    Other(String),
}

impl Failure {
    /// The provider said to slow down (HTTP 429).
    pub fn rate_limited(&self) -> bool {
        matches!(self, Failure::Other(m) if m.contains("HTTP 429"))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(s) | Failure::NotSent(s) | Failure::Other(s) => f.write_str(s),
        }
    }
}

/// Cached installed Claude Code version so fallback requests never run
/// synchronous PATH scans / `stat` calls on Tokio worker threads.
static CLAUDE_VERSION_CACHE: std::sync::OnceLock<olo_sync::Mutex<(std::time::Instant, String)>> = std::sync::OnceLock::new();

fn detect_claude_version() -> String {
    crate::capture::installed(Provider::Claude).map(|i| i.version).filter(|v| !v.is_empty()).unwrap_or_else(|| "2.1.332".into())
}

/// Refresh the cached Claude Code version (called by `capture::follow` on
/// client installs/updates, off the Tokio runtime).
pub fn refresh_claude_version() {
    let ver = detect_claude_version();
    if let Some(cell) = CLAUDE_VERSION_CACHE.get() {
        *cell.lock().unwrap() = (std::time::Instant::now(), ver);
    } else {
        let _ = CLAUDE_VERSION_CACHE.set(olo_sync::Mutex::new((std::time::Instant::now(), ver)));
    }
}

fn claude_version() -> String {
    let cell = CLAUDE_VERSION_CACHE.get_or_init(|| olo_sync::Mutex::new((std::time::Instant::now(), detect_claude_version())));
    cell.lock().unwrap().1.clone()
}

/// Claude Code's CLI User-Agent when no per-account capture exists yet
/// (before the first capture finishes, or where Claude Code isn't installed):
/// Anthropic rejects `cedar_ember` reads without a Claude Code CLI User-Agent
/// (`eligible: false, ineligible_reason: "surface"`).
pub(crate) fn claude_cli_ua() -> String {
    format!("claude-cli/{} (external, cli)", claude_version())
}

/// Claude Code's `/api/oauth/profile` User-Agent (matches Claude Code 2.1.332:
/// `claude-code/<ver>`, with `Content-Type: application/json`,
/// `Cache-Control: no-cache`, and no `anthropic-beta`).
pub(crate) fn claude_profile_ua() -> String {
    format!("claude-code/{}", claude_version())
}

fn with_claude_headers(req: RequestBuilder) -> RequestBuilder {
    req.header("accept", "application/json, text/plain, */*").header("user-agent", claude_cli_ua()).header("anthropic-beta", OAUTH_BETA)
}

/// A request the pool makes for itself, sent as the account's client sends
/// it when its capture has it (`likeness`): its answer; None when none was
/// captured (the pool's own way, then).
async fn as_client(like: Option<&Like>, what: What, up: &Upstreams, c: &Credential, target: Option<&str>, body: Option<Vec<u8>>, doing: &str) -> Option<Result<likeness::Answer, Failure>> {
    let like = like?;
    let sent = like.request(what)?;
    let built = match likeness::build(&sent, &like.fill(c, &sent.host, target.map_or(sent.path(), |t| t.split('?').next().unwrap_or(t))), target, body.as_deref()) {
        Ok(b) => b,
        Err(e) => return Some(Err(Failure::Other(format!("{doing}: {e}")))),
    };
    let answer = likeness::send(&built, up).await.map_err(|e| Failure::Other(format!("{doing}: {e}")));
    if let Ok(a) = &answer {
        like.remember(&sent.host, target.map_or(sent.path(), |t| t.split('?').next().unwrap_or(t)), &a.headers);
    }
    Some(answer)
}

/// An answer's JSON, when it says it went well.
fn ok_json(doing: &str, a: likeness::Answer) -> Result<Value, Failure> {
    if !(200..300).contains(&a.status) {
        let msg = format!("{doing}: HTTP {}{}", a.status, if a.body.is_empty() { String::new() } else { format!(" ({})", a.text()) });
        return Err(if matches!(a.status, 400 | 401 | 403) { Failure::Refused(msg) } else { Failure::Other(msg) });
    }
    a.json().map_err(|e| Failure::Other(format!("{doing}: {e}")))
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
pub async fn refresh(client: &Client, up: &Upstreams, p: Provider, c: &Credential, like: Option<&Like>) -> Result<Credential, Failure> {
    let Some(token) = c.refresh.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Err(Failure::Refused("No refresh token: sign in again.".into()));
    };
    let d: Value = match as_client(like, What::Renewal, up, c, None, None, "Refreshing the sign-in").await {
        Some(answer) => ok_json("Refreshing the sign-in", answer?)?,
        None => {
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
            r.json().await.map_err(|e| Failure::Other(format!("Refreshing the sign-in: {e}")))?
        }
    };
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

/// "seven_day_opus" for people: "Weekly Opus"; one this doesn't know
/// by its words, each capitalised ("overage": "Overage").
fn claude_label(name: &str) -> String {
    match name {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        _ => {
            let (base, rest) = if let Some(r) = name.strip_prefix("seven_day_") {
                ("Weekly ", r)
            } else if let Some(r) = name.strip_prefix("five_hour_") {
                ("5-hour ", r)
            } else {
                ("", name)
            };
            format!("{base}{}", words(rest))
        }
    }
}

/// "overage_included" as words, each capitalised: "Overage Included".
fn words(s: &str) -> String {
    s.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The model families a Claude window is for ("seven_day_opus": Opus).
fn claude_models(name: &str) -> Vec<String> {
    let rest = name.strip_prefix("seven_day_").or_else(|| name.strip_prefix("five_hour_"));
    match rest {
        Some(m) if !m.is_empty() && !m.contains("oauth") && !m.contains("overage") && !m.contains("apps") => vec![m.to_owned()],
        _ => Vec::new(),
    }
}

/// A header meter's own words as the usage report has them: "oi" is
/// "overage_included".
fn header_words(rest: &str) -> String {
    match rest.replace('-', "_").as_str() {
        "oi" => "overage_included".to_owned(),
        r => r.to_owned(),
    }
}

/// Codex's windows are named by their length.
fn codex_label(kind: &str, seconds: Option<i64>) -> String {
    match seconds {
        Some(18_000) => "5-hour".into(),
        Some(604_800) => "Weekly".into(),
        // (A length of nothing, as ChatGPT gives a window not started:
        // named by its place, not "0-day".)
        Some(s) if s > 0 && s % 86_400 == 0 => format!("{}-day", s / 86_400),
        Some(s) if s > 0 && s % 3_600 == 0 => format!("{}-hour", s / 3_600),
        _ => {
            if kind == "primary" {
                "Primary".into()
            } else {
                "Secondary".into()
            }
        }
    }
}

/// "Fable" as part of a window's name: "fable"; "Claude Code":
/// "claude_code".
fn slug(s: &str) -> String {
    s.trim().to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '.' && c != '-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

/// One of Claude's usage rows (the usage report's `limits`, the server's
/// list of every meter the account has: its kind, its scope, how much is
/// used, when it resets), as a window: named as the older fields name
/// the same meters ("five_hour", "seven_day", "seven_day_fable"), so the
/// two never show twice, and a meter for one model limits only that
/// model's requests (one for a surface, Cowork say, none here).
fn claude_row(row: &Value) -> Option<Window> {
    let used = row["percent"].as_f64()?;
    let kind = row["kind"].as_str()?;
    let resets_at = timestamp(&row["resets_at"]);
    let model = row["scope"]["model"]["display_name"].as_str().filter(|m| !m.trim().is_empty());
    let surface = row["scope"]["surface"]["display_name"].as_str().filter(|m| !m.trim().is_empty());
    let (name, label, models) = match (kind, model, surface) {
        ("session", None, None) => ("five_hour".to_owned(), "5-hour".to_owned(), vec![]),
        ("weekly_all", None, None) => ("seven_day".to_owned(), "Weekly".to_owned(), vec![]),
        (_, Some(m), _) => {
            let base = if row["group"] == "session" { ("five_hour", "5-hour") } else { ("seven_day", "Weekly") };
            (format!("{}_{}", base.0, slug(m)), format!("{} {}", base.1, m.trim()), vec![slug(m)])
        }
        (_, None, Some(s)) => {
            let base = if row["group"] == "session" { ("five_hour", "5-hour") } else { ("seven_day", "Weekly") };
            (format!("{}_surface_{}", base.0, slug(s)), format!("{} {}", base.1, s.trim()), vec![format!("surface:{}", slug(s))])
        }
        // (A meter this doesn't know yet: shown by its own words.)
        (k, None, None) => {
            let mut c = k.replace('_', " ");
            if let Some(f) = c.get(..1) {
                c.replace_range(..1, &f.to_uppercase());
            }
            (k.to_owned(), c, vec![])
        }
    };
    Some(Window { name, label, used, resets_at, models })
}

/// Codex's windows of one limit (`rate_limit`, an additional limit's, the
/// code review's): its primary and secondary, named under `under`
/// ("" for the account's own) and said after `title`.
fn codex_windows(out: &mut Vec<Window>, limit: &Value, under: &str, title: &str, models: &[String]) {
    for kind in ["primary", "secondary"] {
        let w = &limit[format!("{kind}_window")];
        let Some(used) = w["used_percent"].as_f64() else { continue };
        let resets_at = timestamp(&w["reset_at"]).or_else(|| w["reset_after_seconds"].as_i64().map(|s| now() + s));
        let length = codex_label(kind, w["limit_window_seconds"].as_i64());
        let (name, label) = if under.is_empty() { (format!("{kind}_window"), length) } else { (format!("{under}/{kind}_window"), format!("{title} {length}")) };
        out.push(Window { name, label, used, resets_at, models: models.to_vec() });
    }
}

/// The quota windows in a usage report: every meter it has (Ben,
/// 2026-11-08: "make all the quotas for an account show up e.g. … a
/// special fable quota and codex might have a 5 hour quota"), one not
/// started yet (nothing used in it) at 0%.
pub fn parse_usage(p: Provider, d: &Value) -> (Vec<Window>, Option<String>) {
    let mut out: Vec<Window> = Vec::new();
    match p {
        Provider::Claude => {
            // The server's rows first: every meter, as it lists them.
            for row in d["limits"].as_array().into_iter().flatten() {
                if let Some(w) = claude_row(row)
                    && !out.iter().any(|o| o.name == w.name)
                {
                    out.push(w);
                }
            }
            // Then the older fields, for what the rows didn't say (and a
            // server without rows). A meter with nothing used yet in it
            // ("utilization": null) is at 0%; one that doesn't apply
            // (null) isn't there.
            if let Some(map) = d.as_object() {
                for (name, w) in map {
                    // (Not "seven_day_breakdown": what the week's usage was
                    // spent on, not a meter.)
                    if !(name.starts_with("five_hour") || name.starts_with("seven_day")) || name.ends_with("_breakdown") || !w.is_object() || out.iter().any(|o| o.name == *name) {
                        continue;
                    }
                    let used = w["utilization"].as_f64().unwrap_or(0.0);
                    out.push(Window { name: name.clone(), label: claude_label(name), used, resets_at: timestamp(&w["resets_at"]), models: claude_models(name) });
                }
            }
            // (The account's own two first, then by name.)
            out.sort_by_key(|w| (!matches!(w.name.as_str(), "five_hour" | "seven_day"), w.name.clone()));
            (out, None)
        }
        Provider::Codex => {
            codex_windows(&mut out, &d["rate_limit"], "", "", &[]);
            // Limits of their own: a model's ("GPT-5.3-Codex-Spark"), for
            // its requests only.
            for (i, extra) in d["additional_rate_limits"].as_array().into_iter().flatten().enumerate() {
                let title = extra["limit_name"].as_str().filter(|n| !n.trim().is_empty()).map(str::trim).unwrap_or("Other");
                let under = extra["metered_feature"].as_str().filter(|f| !f.is_empty()).map(str::to_owned).unwrap_or_else(|| format!("additional_{i}"));
                codex_windows(&mut out, &extra["rate_limit"], &under, title, &[crate::model::model_family(&title.to_lowercase())]);
            }
            // Code review's, which no request here uses.
            codex_windows(&mut out, &d["code_review_rate_limit"], "code_review", "Code review", &["code-review".to_owned()]);
            (out, d["plan_type"].as_str().map(str::to_owned))
        }
    }
}

/// Read a subscription's quota from its provider.
pub async fn usage(client: &Client, up: &Upstreams, p: Provider, c: &Credential, like: Option<&Like>) -> Result<(Vec<Window>, Option<String>), Failure> {
    // (Claude Code's own read: no query; Codex's as its session sends it.)
    let target = (p == Provider::Claude).then_some("/api/oauth/usage");
    if let Some(answer) = as_client(like, What::Usage, up, c, target, None, "Reading the quota").await {
        return Ok(parse_usage(p, &ok_json("Reading the quota", answer?)?));
    }
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage", up.claude),
        Provider::Codex => format!("{}/wham/usage", up.chatgpt),
    };
    let mut req = authorize(client.get(url), p, c).timeout(Duration::from_secs(20));
    if p == Provider::Claude {
        req = with_claude_headers(req).header("content-type", "application/json");
    }
    let r = req.send().await.map_err(|e| Failure::Other(format!("Reading the quota: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading the quota", r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Reading the quota: {e}")))?;
    Ok(parse_usage(p, &d))
}

/// Read a Claude account's organization UUID from `/api/oauth/profile`
/// (used to heal accounts whose `org` wasn't recorded at import, so reset
/// claims to `/api/organizations/{org}/reset_rate_limits` can succeed).
pub async fn profile_org(client: &Client, up: &Upstreams, c: &Credential, like: Option<&Like>) -> Result<Option<String>, Failure> {
    let d: Value = match as_client(like, What::Profile, up, c, Some("/api/oauth/profile"), None, "Reading the profile").await {
        Some(answer) => ok_json("Reading the profile", answer?)?,
        None => {
            let url = format!("{}/api/oauth/profile", up.claude);
            // Match Claude Code 2.1.332's `/api/oauth/profile` call:
            // `User-Agent: claude-code/<ver>`, axios `Accept`,
            // `Content-Type: application/json`, `Cache-Control: no-cache`,
            // and no `anthropic-beta`.
            let r = authorize(client.get(url), Provider::Claude, c)
                .header("accept", "application/json, text/plain, */*")
                .header("user-agent", claude_profile_ua())
                .header("content-type", "application/json")
                .header("cache-control", "no-cache")
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .map_err(|e| Failure::Other(format!("Reading the profile: {e}")))?;
            if !r.status().is_success() {
                return Err(http_failure("Reading the profile", r).await);
            }
            r.json().await.map_err(|e| Failure::Other(format!("Reading the profile: {e}")))?
        }
    };
    Ok(d["organization"]["uuid"].as_str().map(str::trim).filter(|o| !o.is_empty()).map(str::to_owned))
}

/// The reset credits in a provider's report (only ones it says the
/// account may use).
pub fn parse_credits(p: Provider, d: &Value) -> Vec<ResetCredit> {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    match p {
        // ChatGPT's (Codex's `RateLimitResetCreditDetails`): `status`,
        // `reset_type`, `title`.
        Provider::Codex => {
            let mut out: Vec<ResetCredit> = list(&d["credits"])
                .iter()
                .filter(|r| r["status"] == "available" && r["is_supported_by_plan"] != false)
                .filter_map(|r| {
                    Some(ResetCredit {
                        id: r["id"].as_str()?.to_owned(),
                        expires_at: timestamp(&r["expires_at"]),
                        remaining: 1,
                        usable: true,
                        label: text(&r["title"]),
                        kind: text(&r["reset_type"]),
                        ..Default::default()
                    })
                })
                .collect();
            // What ChatGPT counts (`available_count`) is the number, either
            // way: the details can stop short of it (the rest, as ones it
            // chooses itself) or list more than it counts (the soonest to
            // expire of them, which it spends first). Without a count, the
            // details.
            let Some(counted) = d["available_count"].as_u64() else { return out };
            let counted = u32::try_from(counted).unwrap_or(u32::MAX);
            let listed = u32::try_from(out.len()).unwrap_or(u32::MAX);
            if counted > listed {
                out.push(ResetCredit { id: String::new(), expires_at: None, remaining: counted - listed, usable: true, label: None, kind: None, ..Default::default() });
            } else if counted < listed {
                out.sort_by_key(|c| c.expires_at.unwrap_or(i64::MAX));
                out.truncate(counted as usize);
            }
            out
        }
        // Claude's grants (`cedar_ember`), read whole: what each clears,
        // its kind, when it starts and ends, and whether it waits for a
        // limit; none where the account may not use them (`eligible`).
        Provider::Claude => {
            let program = &d["cedar_ember"];
            if program["eligible"] != true {
                if let Some(why) = program["ineligible_reason"].as_str() {
                    tracing::debug!("pool: Claude reset credits ineligible ({why})");
                }
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
                        label: text(&r["label"]),
                        kind: text(&r["type"]),
                        clears: r["clears"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_owned)).collect(),
                        starts_at: timestamp(&r["starts_at"]),
                        // (Claude Code's own default: true.)
                        needs_limit: r["use_requires_limit"].as_bool().unwrap_or(true),
                        unavailable_reason: text(&r["unavailable_reason"]),
                        gives_back_weekly: r["gives_back_weekly"] == true,
                    })
                })
                .collect()
        }
    }
}

/// Read an account's reset credits.
pub async fn credits(client: &Client, up: &Upstreams, p: Provider, c: &Credential, like: Option<&Like>) -> Result<Vec<ResetCredit>, Failure> {
    // (Claude Code's read of its resets: its quota read with this query.)
    let target = (p == Provider::Claude).then_some("/api/oauth/usage?cedar_ember=1&skip_spend=1");
    if let Some(answer) = as_client(like, What::Credits, up, c, target, None, "Reading reset credits").await {
        return Ok(parse_credits(p, &ok_json("Reading reset credits", answer?)?));
    }
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage?cedar_ember=1&skip_spend=1", up.claude),
        Provider::Codex => format!("{}/wham/rate-limit-reset-credits", up.chatgpt),
    };
    let mut req = authorize(client.get(url), p, c).timeout(Duration::from_secs(20));
    if p == Provider::Claude {
        req = with_claude_headers(req).header("content-type", "application/json");
    }
    let r = req.send().await.map_err(|e| Failure::Other(format!("Reading reset credits: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading reset credits", r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("Reading reset credits: {e}")))?;
    Ok(parse_credits(p, &d))
}

/// Redeem `credit` (`request` identifies this redemption, so asking
/// again after an unclear outcome redeems it once).
pub async fn redeem(client: &Client, up: &Upstreams, p: Provider, c: &Credential, org: Option<&str>, credit: &ResetCredit, request: &str, like: Option<&Like>) -> Result<(), Failure> {
    const DOING: &str = "Redeeming a reset credit";
    if let Some(l) = like
        && let Some(sent) = l.request(What::Claim)
    {
        // The client's own claim: Claude Code names the grant; Codex lets
        // ChatGPT choose the next credit, as its own session does.
        let body = match p {
            Provider::Claude => {
                let program = credit.program.as_deref().unwrap_or("cedar_ember");
                likeness::with_fields(&sent.body, &[("program", json!(program)), ("grant_id", json!(credit.id)), ("request_id", json!(request))])
            }
            Provider::Codex => likeness::with_fields(&sent.body, &[("redeem_request_id", json!(request))]),
        }
        .map_err(|e| Failure::NotSent(format!("{DOING}: {e}")))?;
        let effective_org = l.org.as_deref().or(org);
        let concrete_path = match (p, effective_org) {
            (Provider::Claude, Some(o)) if sent.path().contains(crate::capture::ORG) => sent.path().replace(crate::capture::ORG, o),
            _ => sent.path().to_owned(),
        };
        let mut fill = l.fill(c, &sent.host, &concrete_path);
        if fill.org.is_none() {
            fill.org = org.map(str::to_owned);
        }
        let built = likeness::build(&sent, &fill, None, Some(&body)).map_err(|e| Failure::NotSent(format!("{DOING}: {e}")))?;
        let answer = likeness::send(&built, up).await.map_err(|e| match e {
            likeness::SendError::BeforeSend(s) => Failure::NotSent(format!("{DOING}: {s}")),
            likeness::SendError::AfterSend(s) => Failure::Other(format!("{DOING}: {s}")),
        })?;
        l.remember(&sent.host, &concrete_path, &answer.headers);
        return claimed(p, &ok_json(DOING, answer)?);
    }
    let (url, body) = match p {
        // (In Codex's order: its request id, then the credit.)
        // (One ChatGPT chooses, `available_count`'s, named by none.)
        Provider::Codex if credit.id.is_empty() => (format!("{}/wham/rate-limit-reset-credits/consume", up.chatgpt), json!({"redeem_request_id": request})),
        Provider::Codex => (format!("{}/wham/rate-limit-reset-credits/consume", up.chatgpt), json!({"redeem_request_id": request, "credit_id": credit.id})),
        Provider::Claude => {
            let org = org.filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
            let org = org.ok_or_else(|| Failure::NotSent("This account's Claude organization isn't known: sign in again.".into()))?;
            let program = credit.program.as_deref().unwrap_or("cedar_ember");
            (format!("{}/api/organizations/{org}/reset_rate_limits", up.claude), json!({"program": program, "grant_id": credit.id, "request_id": request}))
        }
    };
    let mut req = authorize(client.post(url), p, c).timeout(Duration::from_secs(30)).json(&body);
    if p == Provider::Claude {
        req = with_claude_headers(req);
    }
    let r = req.send().await.map_err(|e| if e.is_connect() || e.is_builder() { Failure::NotSent(format!("{DOING}: {e}")) } else { Failure::Other(format!("{DOING}: {e}")) })?;
    if !r.status().is_success() {
        return Err(http_failure(DOING, r).await);
    }
    let d: Value = r.json().await.map_err(|e| Failure::Other(format!("{DOING}: {e}")))?;
    claimed(p, &d)
}

/// Whether a claim's answer says a reset was used: Claude's `result`;
/// ChatGPT's `code` (Codex's `ConsumeRateLimitResetCreditResponse`), whose
/// `nothing_to_reset` and `no_credit` spent nothing.
fn claimed(p: Provider, d: &Value) -> Result<(), Failure> {
    match p {
        // (Refused: nothing was spent, so no claim is left unclear.)
        Provider::Claude => match d["result"].as_str() {
            Some("reset" | "already_used") => Ok(()),
            Some("not_limited") => Err(Failure::Refused("Claude found nothing to reset: no reset was used.".into())),
            Some(other @ ("ineligible" | "cooldown" | "unavailable")) => Err(Failure::Refused(format!("Claude refused the reset ({other}): none was used."))),
            _ => Err(Failure::Other("Claude didn't confirm the reset.".into())),
        },
        Provider::Codex => match d["code"].as_str() {
            Some("reset" | "already_redeemed") => Ok(()),
            Some("nothing_to_reset") => Err(Failure::Refused("ChatGPT found nothing to reset: no credit was used.".into())),
            Some("no_credit") => Err(Failure::Refused("ChatGPT says this account has no reset credit left.".into())),
            _ => Err(Failure::Other("ChatGPT didn't confirm the reset.".into())),
        },
    }
}

/// What a rejection (429) says: until when, and whether only this
/// model's own meter ran out.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    pub until: i64,
    pub model_only: bool,
}

/// An answer given although the account's own allowance had run out:
/// paid for from its usage credits (what Claude's headers call overage:
/// money beyond the plan), from then until the meter that ran out
/// (`meter`) resets. Whether only this model's own meter ran out.
#[derive(Debug, Clone, PartialEq)]
pub struct Paid {
    pub until: i64,
    pub model_only: bool,
    pub meter: String,
}

/// How long an account its provider serves from usage credits is taken
/// to go on so, where its answer names no reset.
pub const PAID_UNKNOWN: i64 = 3600;

fn header_num(h: &axum::http::HeaderMap, name: &str) -> Option<f64> {
    h.get(name)?.to_str().ok()?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// When to try again, from `retry-after` (seconds or an HTTP date).
pub fn retry_after(h: &axum::http::HeaderMap, now: i64) -> Option<i64> {
    let v = h.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(s) = v.parse::<i64>() {
        return now.checked_add(s.max(0));
    }
    chrono::DateTime::parse_from_rfc2822(v).ok().map(|t| t.timestamp())
}

/// The quota a response's headers report, and a rejection if it was one.
pub fn header_windows(h: &axum::http::HeaderMap, p: Provider, family: &str, status: u16, now: i64) -> (Vec<Window>, Option<Rejection>) {
    let mut out = Vec::new();
    match p {
        Provider::Claude => {
            // Every meter the headers carry ("5h", "7d", "7d-fable", and
            // any new one), named as the usage report names it.
            for (key, _) in h.iter() {
                let Some(kind) = key.as_str().strip_prefix("anthropic-ratelimit-unified-").and_then(|k| k.strip_suffix("-utilization")) else { continue };
                let Some(fraction) = header_num(h, key.as_str()) else { continue };
                // ("7d_oi" as "7d-fable": its own words after the length;
                // "oi" is what the rejections call "overage_included".)
                let name = match kind.split_once(['-', '_']) {
                    None if kind == "5h" => "five_hour".to_owned(),
                    None if kind == "7d" => "seven_day".to_owned(),
                    Some(("5h", rest)) => format!("five_hour_{}", header_words(rest)),
                    Some(("7d", rest)) => format!("seven_day_{}", header_words(rest)),
                    _ => kind.replace('-', "_"),
                };
                let resets_at = header_num(h, &format!("anthropic-ratelimit-unified-{kind}-reset")).map(|v| v as i64);
                out.push(Window { label: claude_label(&name), models: claude_models(&name), name, used: fraction * 100.0, resets_at });
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Provider::Codex => {
            for kind in ["primary", "secondary"] {
                if let Some(used) = header_num(h, &format!("x-codex-{kind}-used-percent")) {
                    let resets_at = header_num(h, &format!("x-codex-{kind}-reset-at"))
                        .map(|v| v as i64)
                        .or_else(|| header_num(h, &format!("x-codex-{kind}-reset-after-seconds")).and_then(|v| now.checked_add(v as i64)));
                    let minutes = header_num(h, &format!("x-codex-{kind}-window-minutes")).and_then(|m| (m as i64).checked_mul(60));
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
    let (model_only, _) = meter_out(h, p, family, until, &mut out);
    (out, Some(Rejection { until, model_only }))
}

/// The meter Claude says ran out (`representative-claim`), marked used up
/// until `until` in `out`: whether it is `family`'s own alone (a
/// per-model one leaves the account's other models usable), and its name
/// among the windows.
fn meter_out(h: &axum::http::HeaderMap, p: Provider, family: &str, until: i64, out: &mut Vec<Window>) -> (bool, String) {
    let claim = h.get("anthropic-ratelimit-unified-representative-claim").and_then(|v| v.to_str().ok()).unwrap_or("");
    let model_only = p == Provider::Claude && !family.is_empty() && ((claim == "seven_day_overage_included" && family == "fable") || claude_models(claim).iter().any(|m| m == family));
    if model_only && claim == "seven_day_overage_included" {
        let name = format!("model:{family}/seven_day");
        out.push(Window { name: name.clone(), label: format!("Weekly {}", words(family)), used: 100.0, resets_at: Some(until), models: vec![family.to_owned()] });
        return (true, name);
    }
    if p == Provider::Claude && (claim.starts_with("five_hour") || claim.starts_with("seven_day")) {
        // (The meter it names is used up until then, whatever its row said
        // a moment before: so the card says what stopped it, and a reset
        // credit that clears it can be used.)
        match out.iter_mut().find(|w| w.name == claim) {
            Some(w) => {
                w.used = w.used.max(100.0);
                w.resets_at = Some(until);
            }
            None => out.push(Window { name: claim.to_owned(), label: claude_label(claim), used: 100.0, resets_at: Some(until), models: claude_models(claim) }),
        }
    }
    (model_only, claim.to_owned())
}

/// Whether an answer that was given (`status`: not a refusal) was paid
/// for from the account's usage credits, its own allowance being out:
/// Claude says a meter ran out (`…-unified-status: rejected`) and answers
/// all the same. It is how Claude Code itself tells (2.1.332:
/// `isUsingOverage`, status "rejected" with `…-overage-status` "allowed"
/// or "allowed_warning"; its "Now using usage credits"). The meter is
/// marked used up in `out` as a refusal marks it, so that the account is
/// left for another, or for a reset credit that clears it.
///
/// The pool took only a 429 for a meter running out: an account whose
/// provider went on answering from usage credits stayed the one chosen,
/// every request after paid for, with other accounts unused and reset
/// credits unclaimed (Ben, 2026-11-26: "instead of switching to another
/// account or using a reset token pool is letting the clients use usage
/// credits first").
pub fn paid_by_credits(h: &axum::http::HeaderMap, p: Provider, family: &str, status: u16, now: i64, out: &mut Vec<Window>) -> Option<Paid> {
    if p != Provider::Claude || !(200..400).contains(&status) {
        return None;
    }
    if h.get("anthropic-ratelimit-unified-status").and_then(|v| v.to_str().ok()).map(str::trim) != Some("rejected") {
        return None;
    }
    let claim = h.get("anthropic-ratelimit-unified-representative-claim").and_then(|v| v.to_str().ok()).unwrap_or("");
    // Until the meter it names resets: as the answer says, else as that
    // meter's own row does, else the soonest of those used up.
    let until = header_num(h, "anthropic-ratelimit-unified-reset")
        .map(|v| v as i64)
        .filter(|&v| v > now)
        .or_else(|| out.iter().find(|w| w.name == claim).and_then(|w| w.resets_at).filter(|&r| r > now))
        .or_else(|| out.iter().filter(|w| w.used >= 99.5).filter_map(|w| w.resets_at).filter(|&r| r > now).min())
        .unwrap_or(now + PAID_UNKNOWN);
    let (model_only, meter) = meter_out(h, p, family, until, out);
    Some(Paid { until, model_only, meter })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ChatGPT counts (`available_count`) is the number either way
    /// (Codex's review: two rows and a count of 0 made two credits): more
    /// than its details list, the rest as its own choice; fewer, the
    /// soonest to expire of those listed; no count, the details.
    #[test]
    fn chatgpts_count_is_the_number() {
        let row = |id: &str, at: &str| json!({"id": id, "status": "available", "expires_at": at});
        let rows = json!([row("late", "2026-12-20T00:00:00Z"), row("soon", "2026-12-01T00:00:00Z")]);
        let count = |n: Option<u64>| {
            let mut d = json!({"credits": rows});
            if let Some(n) = n {
                d["available_count"] = json!(n);
            }
            let got = parse_credits(Provider::Codex, &d);
            (got.iter().map(|c| c.remaining).sum::<u32>(), got.iter().map(|c| c.id.clone()).collect::<Vec<_>>())
        };
        assert_eq!(count(Some(0)), (0, vec![]));
        assert_eq!(count(Some(1)), (1, vec!["soon".to_owned()]));
        assert_eq!(count(Some(3)).0, 3);
        assert_eq!(count(None).0, 2);
        assert_eq!(count(Some(u64::MAX)).0, u32::MAX);
    }

    /// A window of no length is named by its place ("Primary"), never
    /// "0-day" (Ben's Token Pool showed one, 2026-11-19).
    #[test]
    fn a_window_of_no_length_has_a_name() {
        assert_eq!(codex_label("primary", Some(0)), "Primary");
        assert_eq!(codex_label("secondary", Some(0)), "Secondary");
        assert_eq!(codex_label("primary", Some(18_000)), "5-hour");
        assert_eq!(codex_label("secondary", Some(604_800)), "Weekly");
        assert_eq!(codex_label("secondary", Some(2 * 86_400)), "2-day");
    }

    #[test]
    fn usage_reports() {
        let d = json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-27T22:00:00Z"},
            "seven_day": {"utilization": 18, "resets_at": 1793500000},
            "seven_day_opus": {"utilization": 100, "resets_at": 1793600000},
            "seven_day_oauth_apps": null,
            "seven_day_breakdown": {"by_model": {}},
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

    /// Every meter there is: Claude's rows (Fable's own weekly, a
    /// surface's, one not known yet), the older fields for what they
    /// don't say, one not started at 0%; Codex's extra limits and code
    /// review's.
    #[test]
    fn every_meter_reported() {
        let d = json!({
            "five_hour": {"utilization": null, "resets_at": null},
            "seven_day": {"utilization": 26.0, "resets_at": "2026-11-12T00:00:00Z"},
            "seven_day_sonnet": {"utilization": 4.0, "resets_at": "2026-11-12T00:00:00Z"},
            "seven_day_oauth_apps": null,
            "limits": [
                {"kind": "weekly_all", "group": "weekly", "percent": 26.0, "resets_at": "2026-11-12T00:00:00Z", "scope": null, "severity": "normal", "is_active": true},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 61.5, "resets_at": "2026-11-12T00:00:00Z", "scope": {"model": {"display_name": "Fable"}}, "severity": "warning", "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 3.0, "resets_at": null, "scope": {"surface": {"display_name": "Cowork"}}, "severity": "normal", "is_active": false},
                {"kind": "monthly_all", "group": "monthly", "percent": 9.0, "resets_at": null, "severity": "normal", "is_active": false}
            ]
        });
        let (w, _) = parse_usage(Provider::Claude, &d);
        let rows: Vec<_> = w.iter().map(|w| (w.name.as_str(), w.label.as_str(), w.used, w.models.clone())).collect();
        assert_eq!(
            rows,
            [
                ("five_hour", "5-hour", 0.0, vec![]),
                ("seven_day", "Weekly", 26.0, vec![]),
                ("monthly_all", "Monthly all", 9.0, vec![]),
                ("seven_day_fable", "Weekly Fable", 61.5, vec!["fable".to_owned()]),
                ("seven_day_sonnet", "Weekly Sonnet", 4.0, vec!["sonnet".to_owned()]),
                ("seven_day_surface_cowork", "Weekly Cowork", 3.0, vec!["surface:cowork".to_owned()]),
            ]
        );
        // Fable's limits only Fable; Cowork's, nothing here.
        assert!(w[3].applies_to("fable") && !w[3].applies_to("opus"));
        assert!(!w[5].applies_to("opus") && !w[5].applies_to("fable"));
        let d = json!({"plan_type": "pro",
            "rate_limit": {"primary_window": {"used_percent": 12, "limit_window_seconds": 604800, "reset_after_seconds": 100}, "secondary_window": null},
            "additional_rate_limits": [{"limit_name": "GPT-5.3-Codex-Spark", "metered_feature": "codex_bengalfox",
                "rate_limit": {"primary_window": {"used_percent": 40, "limit_window_seconds": 18000, "reset_after_seconds": 50},
                               "secondary_window": {"used_percent": 5, "limit_window_seconds": 604800, "reset_after_seconds": 500}}}],
            "code_review_rate_limit": {"primary_window": {"used_percent": 0, "limit_window_seconds": 604800, "reset_after_seconds": 900}}});
        let (w, _) = parse_usage(Provider::Codex, &d);
        let rows: Vec<_> = w.iter().map(|w| (w.name.as_str(), w.label.as_str(), w.used)).collect();
        assert_eq!(
            rows,
            [
                ("primary_window", "Weekly", 12.0),
                ("codex_bengalfox/primary_window", "GPT-5.3-Codex-Spark 5-hour", 40.0),
                ("codex_bengalfox/secondary_window", "GPT-5.3-Codex-Spark Weekly", 5.0),
                ("code_review/primary_window", "Code review Weekly", 0.0),
            ]
        );
        assert!(w[1].applies_to("gpt-5.3-codex-spark") && !w[1].applies_to("gpt-6-astra"));
        assert!(!w[3].applies_to("gpt-6-astra"));
    }

    #[test]
    fn header_meters_named_as_people_say_them() {
        // (Ben's pool, 2026-11-13: "7d oi", "overage" and "Weekly fable"
        // beside "Weekly Fable" among his quotas.)
        let mut h = axum::http::HeaderMap::new();
        for (k, v) in [("7d", "0.2"), ("7d-fable", "0.4"), ("7d_oi", "0.4"), ("overage", "0")] {
            h.insert(axum::http::HeaderName::try_from(format!("anthropic-ratelimit-unified-{k}-utilization")).unwrap(), v.parse().unwrap());
        }
        let (w, _) = header_windows(&h, Provider::Claude, "", 200, 0);
        let names: Vec<_> = w.iter().map(|w| (w.name.as_str(), w.label.as_str())).collect();
        assert_eq!(names, [("overage", "Overage"), ("seven_day", "Weekly"), ("seven_day_fable", "Weekly Fable"), ("seven_day_overage_included", "Weekly Overage Included")]);
        assert!(w[3].models.is_empty(), "overage isn't a model");
        assert_eq!(claude_label("seven_day_claude_code"), "Weekly Claude Code");
    }

    #[test]
    fn headers_and_rejections() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.5".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "5000".parse().unwrap());
        let (w, r) = header_windows(&h, Provider::Claude, "opus", 200, 1000);
        assert_eq!((w[0].used, w[0].resets_at, r), (50.0, Some(5000), None));
        // Any meter the headers carry, Fable's own and a new one.
        h.insert("anthropic-ratelimit-unified-7d-fable-utilization", "0.25".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-7d-omelette-utilization", "0.1".parse().unwrap());
        let (w, _) = header_windows(&h, Provider::Claude, "opus", 200, 1000);
        let names: Vec<_> = w.iter().map(|w| (w.name.as_str(), w.label.as_str())).collect();
        assert_eq!(names, [("five_hour", "5-hour"), ("seven_day_fable", "Weekly Fable"), ("seven_day_omelette", "Weekly Omelette")]);
        h.remove("anthropic-ratelimit-unified-7d-fable-utilization");
        h.remove("anthropic-ratelimit-unified-7d-omelette-utilization");
        h.insert("anthropic-ratelimit-unified-status", "rejected".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-representative-claim", "seven_day_overage_included".parse().unwrap());
        h.insert("retry-after", "600".parse().unwrap());
        let (w, r) = header_windows(&h, Provider::Claude, "fable", 429, 1000);
        assert_eq!(r, Some(Rejection { until: 1600, model_only: true }));
        assert!(w.iter().any(|w| w.models == ["fable"] && w.used >= 100.0 && w.label == "Weekly Fable"));
        // A shared meter: the whole account waits.
        h.insert("anthropic-ratelimit-unified-representative-claim", "five_hour".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-reset", "7000".parse().unwrap());
        assert_eq!(header_windows(&h, Provider::Claude, "fable", 429, 1000).1, Some(Rejection { until: 7000, model_only: false }));
        // An answer given all the same (200), the week used up: paid for
        // from usage credits. No refusal (it was served), and the week is
        // marked used up until its reset, as a refusal marks it.
        let mut paid = axum::http::HeaderMap::new();
        for (k, v) in [
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
            ("anthropic-ratelimit-unified-representative-claim", "seven_day"),
            ("anthropic-ratelimit-unified-reset", "9000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.97"),
            ("anthropic-ratelimit-unified-7d-reset", "9000"),
        ] {
            paid.insert(k, v.parse().unwrap());
        }
        let (mut w, r) = header_windows(&paid, Provider::Claude, "opus", 200, 1000);
        assert_eq!((r, w[0].used), (None, 97.0));
        assert_eq!(paid_by_credits(&paid, Provider::Claude, "opus", 200, 1000, &mut w), Some(Paid { until: 9000, model_only: false, meter: "seven_day".into() }));
        assert_eq!((w.len(), w[0].name.as_str(), w[0].used, w[0].resets_at), (1, "seven_day", 100.0, Some(9000)));
        // (A meter the answer's rows don't carry: the 5-hour one, named
        // by the claim alone. Its reset from the answer's own.)
        paid.insert("anthropic-ratelimit-unified-representative-claim", "five_hour".parse().unwrap());
        paid.insert("anthropic-ratelimit-unified-reset", "4000".parse().unwrap());
        let (mut w, _) = header_windows(&paid, Provider::Claude, "opus", 200, 1000);
        assert_eq!(paid_by_credits(&paid, Provider::Claude, "opus", 200, 1000, &mut w), Some(Paid { until: 4000, model_only: false, meter: "five_hour".into() }));
        assert!(w.iter().any(|w| w.name == "five_hour" && w.used == 100.0 && w.resets_at == Some(4000)));
        // (Fable's own week: the account's other models go on.)
        paid.insert("anthropic-ratelimit-unified-representative-claim", "seven_day_overage_included".parse().unwrap());
        let (mut w, _) = header_windows(&paid, Provider::Claude, "fable", 200, 1000);
        assert_eq!(paid_by_credits(&paid, Provider::Claude, "fable", 200, 1000, &mut w), Some(Paid { until: 4000, model_only: true, meter: "model:fable/seven_day".into() }));
        // (No reset named anywhere: an hour, then looked at again.)
        paid.remove("anthropic-ratelimit-unified-reset");
        paid.remove("anthropic-ratelimit-unified-7d-reset");
        paid.insert("anthropic-ratelimit-unified-representative-claim", "five_hour".parse().unwrap());
        let (mut w, _) = header_windows(&paid, Provider::Claude, "opus", 200, 1000);
        assert_eq!(paid_by_credits(&paid, Provider::Claude, "opus", 200, 1000, &mut w).map(|p| p.until), Some(1000 + PAID_UNKNOWN));
        // Within its allowance, a warning near its end, a refusal, another
        // provider's: not paid for from credits.
        let mut within = axum::http::HeaderMap::new();
        for status in ["allowed", "allowed_warning"] {
            within.insert("anthropic-ratelimit-unified-status", status.parse().unwrap());
            within.insert("anthropic-ratelimit-unified-overage-status", "allowed".parse().unwrap());
            assert_eq!(paid_by_credits(&within, Provider::Claude, "opus", 200, 1000, &mut Vec::new()), None, "{status}");
        }
        within.insert("anthropic-ratelimit-unified-status", "rejected".parse().unwrap());
        assert_eq!(paid_by_credits(&within, Provider::Claude, "opus", 429, 1000, &mut Vec::new()), None, "a refusal is a rejection, not an answer");
        assert_eq!(paid_by_credits(&within, Provider::Codex, "gpt-5-codex", 200, 1000, &mut Vec::new()), None);
        assert_eq!(paid_by_credits(&axum::http::HeaderMap::new(), Provider::Claude, "opus", 200, 1000, &mut Vec::new()), None);
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

    #[test]
    fn claude_credits_parse_needs_limit_and_cli_ua() {
        let d = json!({
            "cedar_ember": {
                "eligible": true,
                "at_limit": false,
                "grants": [{
                    "id": "full-reset-2611",
                    "type": "full",
                    "label": "Full Reset",
                    "resets_left": 1,
                    "paused": false,
                    "usable_now": false,
                    "use_requires_limit": true,
                    "unavailable_reason": "needs_limit",
                    "clears": ["five_hour", "seven_day"],
                    "ends_at": "2026-12-01T00:00:00Z"
                }]
            }
        });
        let got = parse_credits(Provider::Claude, &d);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].id.as_str(), got[0].usable, got[0].needs_limit, got[0].unavailable_reason.as_deref()), ("full-reset-2611", false, true, Some("needs_limit")));
        let bad = json!({"cedar_ember": {"eligible": false, "ineligible_reason": "surface", "grants": []}});
        assert!(parse_credits(Provider::Claude, &bad).is_empty());
        let ua = claude_cli_ua();
        assert!(ua.starts_with("claude-cli/") && ua.ends_with(" (external, cli)"), "{ua}");
        let pua = claude_profile_ua();
        assert!(pua.starts_with("claude-code/") && !pua.contains("external"), "{pua}");
    }
}
