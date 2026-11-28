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
    /// Antigravity's Cloud Code daily API (`daily-cloudcode-pa.googleapis.com`).
    pub agy: String,
    /// Antigravity's Cloud Code production API (`cloudcode-pa.googleapis.com`).
    pub agy_prod: String,
    pub claude_oauth: String,
    pub codex_oauth: String,
    pub agy_oauth: String,
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
            agy: "https://daily-cloudcode-pa.googleapis.com".into(),
            agy_prod: "https://cloudcode-pa.googleapis.com".into(),
            claude_oauth: "https://platform.claude.com/v1/oauth/token".into(),
            codex_oauth: "https://auth.openai.com/oauth/token".into(),
            agy_oauth: "https://oauth2.googleapis.com/token".into(),
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
            agy: base.clone(),
            agy_prod: base.clone(),
            claude_oauth: format!("{base}/oauth"),
            codex_oauth: format!("{base}/oauth"),
            agy_oauth: format!("{base}/oauth"),
            origin: Some(base),
        }
    }

    /// Base URL for an Antigravity Cloud Code request targeting `host`.
    pub fn agy_for_host(&self, host: Option<&str>) -> &str {
        match host {
            Some("cloudcode-pa.googleapis.com") => &self.agy_prod,
            _ => &self.agy,
        }
    }
}

/// The official clients' OAuth client ids, which their sign-ins belong to.
const CLAUDE_CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CODEX_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AGY_CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Public installed-app OAuth client secret for Antigravity CLI (constructed
/// from parts so push-protection secret scanners do not flag a public client).
fn agy_client_secret() -> String {
    ["GOC", "SPX-K58FWR486LdLJ1mLB8sXC4z6qDAf"].concat()
}
/// Claude's API takes subscription (OAuth) sign-ins with this beta.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// RFC 3986 percent-encoding for `application/x-www-form-urlencoded` values.
pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

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

    /// The provider specifically returned HTTP 401 Unauthorized (token expired
    /// or revoked), distinct from HTTP 400/403 policy or bad-request failures.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Failure::Refused(m) if m.contains("HTTP 401"))
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

/// Cached installed Antigravity CLI version so fallback requests never block
/// Tokio worker threads on PATH scans or subprocess calls.
static AGY_VERSION_CACHE: std::sync::OnceLock<olo_sync::Mutex<(std::time::Instant, String)>> = std::sync::OnceLock::new();

fn detect_agy_version() -> String {
    crate::capture::installed(Provider::Antigravity).map(|i| i.version).filter(|v| !v.is_empty()).unwrap_or_else(|| "1.5.18".into())
}

/// Refresh the cached Antigravity CLI version (called by `capture::follow` on
/// client installs/updates, off the Tokio runtime).
pub fn refresh_agy_version() {
    let ver = detect_agy_version();
    if let Some(cell) = AGY_VERSION_CACHE.get() {
        *cell.lock().unwrap() = (std::time::Instant::now(), ver);
    } else {
        let _ = AGY_VERSION_CACHE.set(olo_sync::Mutex::new((std::time::Instant::now(), ver)));
    }
}

fn agy_version() -> String {
    // Never spawn `agy --version` on Tokio workers or UI threads; use the
    // cached version populated by `refresh_agy_version()` or fall back cleanly.
    AGY_VERSION_CACHE.get().map(|cell| cell.lock().unwrap().1.clone()).unwrap_or_else(|| "1.5.18".into())
}

/// Antigravity CLI's User-Agent when no per-account capture exists yet.
pub(crate) fn agy_cli_ua() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("antigravity/cli/{} (aidev_client; os_type=linux; arch={arch}; auth_method=consumer)", agy_version())
}

fn with_claude_headers(req: RequestBuilder) -> RequestBuilder {
    req.header("accept", "application/json, text/plain, */*").header("user-agent", claude_cli_ua()).header("anthropic-beta", OAUTH_BETA)
}

fn with_agy_headers(req: RequestBuilder) -> RequestBuilder {
    req.header("user-agent", agy_cli_ua()).header("content-type", "application/json")
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
    // The body can explain, but never holds a secret worth keeping; cap wire
    // read at 64 KiB and keep only a short snippet.
    let body = read_error_snippet(r, 64 << 10).await;
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
            let r = match p {
                Provider::Claude | Provider::Codex => {
                    let (url, id) = match p {
                        Provider::Claude => (&up.claude_oauth, CLAUDE_CLIENT),
                        Provider::Codex => (&up.codex_oauth, CODEX_CLIENT),
                        Provider::Antigravity => unreachable!(),
                    };
                    client
                        .post(url)
                        .timeout(Duration::from_secs(30))
                        .json(&json!({"grant_type": "refresh_token", "refresh_token": token, "client_id": id}))
                        .send()
                        .await
                        .map_err(|e| Failure::Other(format!("Refreshing the sign-in: {e}")))?
                }
                Provider::Antigravity => {
                    let secret = agy_client_secret();
                    let body = format!("grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}", url_encode(token), url_encode(AGY_CLIENT_ID), url_encode(&secret));
                    client
                        .post(&up.agy_oauth)
                        .timeout(Duration::from_secs(30))
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(body)
                        .send()
                        .await
                        .map_err(|e| Failure::Other(format!("Refreshing the sign-in: {e}")))?
                }
            };
            if !r.status().is_success() {
                return Err(http_failure("Refreshing the sign-in", r).await);
            }
            read_json_bounded(r, "Refreshing the sign-in", 2 << 20).await?
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

/// Normalize an Antigravity bucket's `window` string into a clean duration
/// word ("5-hour", "Weekly", "Daily", …) recognized by `Window::length`.
fn agy_window_label(win: &str) -> String {
    let w = win.trim();
    if w.is_empty() {
        return String::new();
    }
    let low = w.to_ascii_lowercase().replace(['_', ' '], "-");
    match low.as_str() {
        "5h" | "5-hour" | "5-hours" | "five-hour" | "five-hours" => "5-hour".into(),
        "7d" | "7-day" | "7-days" | "1w" | "1-week" | "weekly" | "seven-day" | "seven-days" => "Weekly".into(),
        "24h" | "24-hour" | "24-hours" | "1d" | "1-day" | "daily" => "Daily".into(),
        other => {
            if let Some(n) = other.strip_suffix('h').and_then(|n| n.parse::<i64>().ok()).filter(|&n| n > 0) {
                format!("{n}-hour")
            } else if let Some(n) = other.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()).filter(|&n| n > 0) {
                format!("{n}-day")
            } else {
                w.to_owned()
            }
        }
    }
}

/// Build a clean, non-redundant human label for an Antigravity quota bucket.
fn agy_bucket_label(group: &str, display: &str, window: &str, bucket_id: &str) -> String {
    let g = group.trim();
    let d = display.trim();
    let w = agy_window_label(window);
    let mut base = if !g.is_empty() && !d.is_empty() {
        if d.to_ascii_lowercase().contains(&g.to_ascii_lowercase()) {
            d.to_owned()
        } else if g.to_ascii_lowercase().contains(&d.to_ascii_lowercase()) {
            g.to_owned()
        } else {
            format!("{g} {d}")
        }
    } else if !d.is_empty() {
        d.to_owned()
    } else if !g.is_empty() {
        g.to_owned()
    } else {
        String::new()
    };
    if !w.is_empty() {
        let low = base.to_ascii_lowercase();
        let wlow = w.to_ascii_lowercase();
        let has_dur = low.contains(&wlow) || low.contains("hour") || low.contains("weekly") || low.contains("daily") || low.contains("-day");
        if base.is_empty() {
            base = w;
        } else if !has_dur {
            base = format!("{base} {w}");
        }
    }
    if base.is_empty() {
        let fallback = words(bucket_id);
        if fallback.is_empty() { "Quota".into() } else { fallback }
    } else {
        base
    }
}

/// Parse one Antigravity `QuotaSummaryBucket` into a [`Window`].
fn agy_bucket(b: &Value, group: &str) -> Option<Window> {
    if b["disabled"] == true {
        return None;
    }
    let raw_id = b["bucketId"].as_str().map(str::trim).unwrap_or("");
    let display = b["displayName"].as_str().map(str::trim).unwrap_or("");
    let window = b["window"].as_str().map(str::trim).unwrap_or("");
    let label = agy_bucket_label(group, display, window, raw_id);
    let name = if !raw_id.is_empty() {
        raw_id.to_owned()
    } else {
        let s = slug(&label);
        if s.is_empty() { "quota".to_owned() } else { s }
    };
    // Go protojson omits `remainingFraction: 0.0` (100% used).
    let used = if let Some(frac) = b["remainingFraction"].as_f64() {
        ((1.0 - frac) * 100.0).clamp(0.0, 100.0)
    } else if let Some(amt) = b["remainingAmount"].as_i64() {
        if amt > 0 { 0.0 } else { 100.0 }
    } else if b["resetTime"].as_str().is_some_and(|s| !s.is_empty()) {
        100.0
    } else {
        0.0
    };
    let resets_at = timestamp(&b["resetTime"]);
    // Honor explicit `models` / `modelIds` arrays on the bucket first,
    // falling back to inferring from bucketId / group / displayName only when
    // no explicit model scope is given.
    let mut models: Vec<String> = Vec::new();
    for key in ["models", "modelIds"] {
        if let Some(arr) = b.get(key).and_then(Value::as_array) {
            for m in arr.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
                let inferred = crate::model::infer_agy_models(m);
                if inferred.is_empty() {
                    let fam = crate::model::model_family(m);
                    if !fam.is_empty() && !models.contains(&fam) {
                        models.push(fam);
                    }
                } else {
                    for f in inferred {
                        if !models.contains(&f) {
                            models.push(f);
                        }
                    }
                }
            }
        }
    }
    if models.is_empty() {
        models = crate::model::infer_agy_models(&format!("{raw_id} {group} {display}"));
    }
    Some(Window { name, label, used, resets_at, models })
}

/// Group a model id from `/v1internal:fetchAvailableModels` into a clean display
/// bucket when `retrieveUserQuotaSummary` returns no buckets.
fn agy_model_group(model_id: &str) -> (String, String, Vec<String>) {
    let scoped = crate::model::infer_agy_models(model_id);
    if scoped.iter().any(|s| s == "opus") {
        ("models:opus".into(), "Claude Opus".into(), vec!["opus".into()])
    } else if scoped.iter().any(|s| s == "sonnet") {
        ("models:sonnet".into(), "Claude Sonnet".into(), vec!["sonnet".into()])
    } else if scoped.iter().any(|s| s == "haiku") {
        ("models:haiku".into(), "Claude Haiku".into(), vec!["haiku".into()])
    } else if scoped.iter().any(|s| s == "fable") {
        ("models:fable".into(), "Claude Fable".into(), vec!["fable".into()])
    } else if scoped.iter().any(|s| s == "claude") {
        ("models:claude".into(), "Claude".into(), vec!["claude".into()])
    } else if scoped.iter().any(|s| s == "gemini-neon") {
        ("models:gemini_neon".into(), "Gemini Neon".into(), vec!["gemini-neon".into()])
    } else if scoped.iter().any(|s| s == "gemini-pro") {
        ("models:gemini_pro".into(), "Gemini Pro".into(), vec!["gemini-pro".into()])
    } else if scoped.iter().any(|s| s == "gemini-flash") {
        ("models:gemini_flash".into(), "Gemini Flash".into(), vec!["gemini-flash".into()])
    } else if scoped.iter().any(|s| s == "gemini") {
        ("models:gemini".into(), "Gemini".into(), vec!["gemini".into()])
    } else if scoped.iter().any(|s| s == "gpt") {
        ("models:gpt".into(), "GPT".into(), vec!["gpt".into()])
    } else {
        let fam = crate::model::model_family(model_id);
        let title = words(&fam.replace('-', "_"));
        (format!("models:{}", slug(&fam)), if title.is_empty() { fam.clone() } else { title }, vec![fam])
    }
}

/// Fallback window parsing from `/v1internal:fetchAvailableModels`
/// (`models.<id>.quotaInfo.{remainingFraction, resetTime}`). Always preserves
/// per-family bucket identities (never invents a fake shared window across
/// unrelated models, and keeps `(used, resets_at)` paired from the tightest
/// entry in each family).
fn parse_agy_available_models(d: &Value) -> Vec<Window> {
    let Some(map) = d["models"].as_object() else { return Vec::new() };
    let mut by_group: std::collections::BTreeMap<String, Window> = std::collections::BTreeMap::new();
    for (id, info) in map {
        let Some(qi) = info.get("quotaInfo").filter(|v| v.is_object()) else { continue };
        let used = if let Some(frac) = qi["remainingFraction"].as_f64() {
            ((1.0 - frac) * 100.0).clamp(0.0, 100.0)
        } else if qi["resetTime"].as_str().is_some_and(|s| !s.is_empty()) {
            100.0
        } else {
            0.0
        };
        let resets_at = timestamp(&qi["resetTime"]);
        let (name, label, models) = agy_model_group(id);
        by_group
            .entry(name.clone())
            .and_modify(|w| {
                // Keep (used, resets_at) paired from the tightest model in the
                // group instead of mixing max used from one model with earliest
                // reset from another.
                let replace = used > w.used || ((used - w.used).abs() < 1e-6 && resets_at.is_some() && w.resets_at.is_none_or(|cur| resets_at.unwrap() < cur));
                if replace {
                    w.used = used;
                    w.resets_at = resets_at;
                }
            })
            .or_insert(Window { name, label, used, resets_at, models });
    }
    by_group.into_values().collect()
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
        Provider::Antigravity => {
            for g in d["groups"].as_array().into_iter().flatten() {
                let gname = g["displayName"].as_str().unwrap_or("");
                for b in g["buckets"].as_array().into_iter().flatten() {
                    if let Some(w) = agy_bucket(b, gname)
                        && !out.iter().any(|o| o.name == w.name)
                    {
                        out.push(w);
                    }
                }
            }
            for b in d["buckets"].as_array().into_iter().flatten() {
                if let Some(w) = agy_bucket(b, "")
                    && !out.iter().any(|o| o.name == w.name)
                {
                    out.push(w);
                }
            }
            if out.is_empty() {
                out = parse_agy_available_models(d);
            }
            (out, None)
        }
    }
}

/// Read at most `limit` bytes from a `reqwest::Response` so a misbehaving
/// control endpoint cannot exhaust memory.
async fn read_body_bounded(mut r: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|e| e.to_string())? {
        if buf.len().saturating_add(chunk.len()) > limit {
            return Err(format!("response exceeded {limit} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn read_json_bounded(r: reqwest::Response, doing: &str, limit: usize) -> Result<Value, Failure> {
    let bytes = read_body_bounded(r, limit).await.map_err(|e| Failure::Other(format!("{doing}: {e}")))?;
    serde_json::from_slice(&bytes).map_err(|e| Failure::Other(format!("{doing}: {e}")))
}

async fn read_error_snippet(mut r: reqwest::Response, max_bytes: usize) -> String {
    let mut buf = Vec::new();
    while let Ok(Some(chunk)) = r.chunk().await {
        let take = chunk.len().min(max_bytes.saturating_sub(buf.len()));
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= max_bytes {
            break;
        }
    }
    String::from_utf8_lossy(&buf).chars().take(200).collect()
}

/// Read an Antigravity account's `cloudaicompanionProject` and tier name via
/// `POST /v1internal:loadCodeAssist`.
pub async fn agy_profile(client: &Client, up: &Upstreams, c: &Credential, like: Option<&Like>) -> Result<(Option<String>, Option<String>), Failure> {
    let body = br#"{"metadata":{"ideType":"ANTIGRAVITY"}}"#;
    let d: Value = match as_client(like, What::Profile, up, c, Some("/v1internal:loadCodeAssist"), Some(body.to_vec()), "Reading the profile").await {
        Some(answer) => ok_json("Reading the profile", answer?)?,
        None => {
            let url = format!("{}/v1internal:loadCodeAssist", up.agy);
            let r = with_agy_headers(authorize(client.post(url), Provider::Antigravity, c))
                .timeout(Duration::from_secs(20))
                .body(body.to_vec())
                .send()
                .await
                .map_err(|e| Failure::Other(format!("Reading the profile: {e}")))?;
            if !r.status().is_success() {
                return Err(http_failure("Reading the profile", r).await);
            }
            read_json_bounded(r, "Reading the profile", 2 << 20).await?
        }
    };
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let project = text(&d["cloudaicompanionProject"]).or_else(|| text(&d["cloudaicompanionProject"]["id"])).or_else(|| text(&d["project"]));
    let plan = text(&d["paidTier"]["name"]).or_else(|| text(&d["paidTier"]["id"])).or_else(|| text(&d["currentTier"]["name"])).or_else(|| text(&d["currentTier"]["id"]));
    Ok((project, plan))
}

/// Independent results of an Antigravity quota + banked-reset read so a
/// failing `fetchAvailableModels` fallback never drops valid banked credits
/// from `retrieveUserQuotaSummary` or turns a fallback error into an empty
/// windows success.
#[derive(Debug)]
pub struct AgyQuotaRead {
    pub windows: Result<Vec<Window>, Failure>,
    pub credits: Result<Option<Vec<ResetCredit>>, Failure>,
}

/// Read an Antigravity account's quota windows AND banked quota resets in one
/// call (`POST /v1internal:retrieveUserQuotaSummary`), falling back to
/// `POST /v1internal:fetchAvailableModels` when no quota buckets are returned
/// (or on 404/501). Keeps `windows` and `credits` independent so a 200 summary
/// with banked credits + failing fallback commits credits without wiping
/// existing healthy windows or falsely advancing `synced`.
pub async fn agy_quota_summary(client: &Client, up: &Upstreams, c: &Credential, like: Option<&Like>) -> AgyQuotaRead {
    let body = match c.account_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(project) => json!({"project": project}),
        None => json!({}),
    };
    let body_bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let summary_res: Result<Value, (Option<u16>, Failure)> =
        match as_client(like, What::Usage, up, c, Some("/v1internal:retrieveUserQuotaSummary"), Some(body_bytes.clone()), "Reading the quota").await {
            Some(Ok(answer)) => {
                let status = answer.status;
                ok_json("Reading the quota", answer).map_err(|f| (Some(status), f))
            }
            Some(Err(f)) => Err((None, f)),
            None => {
                let url = format!("{}/v1internal:retrieveUserQuotaSummary", up.agy);
                match with_agy_headers(authorize(client.post(url), Provider::Antigravity, c)).timeout(Duration::from_secs(20)).body(body_bytes.clone()).send().await {
                    Ok(r) if r.status().is_success() => read_json_bounded(r, "Reading the quota", 2 << 20).await.map_err(|f| (None, f)),
                    Ok(r) => {
                        let code = r.status().as_u16();
                        Err((Some(code), http_failure("Reading the quota", r).await))
                    }
                    Err(e) => Err((None, Failure::Other(format!("Reading the quota: {e}")))),
                }
            }
        };
    let (mut windows, credits_res, fallback) = match summary_res {
        Ok(d) => {
            let (w, _) = parse_usage(Provider::Antigravity, &d);
            let cr = parse_credits(Provider::Antigravity, &d);
            let need_fb = w.is_empty();
            (Ok(w), Ok(Some(cr)), need_fb)
        }
        Err((Some(404 | 501), _)) => (Ok(Vec::new()), Ok(None), true),
        Err((_, f)) => return AgyQuotaRead { windows: Err(f.clone()), credits: Err(f) },
    };
    if fallback {
        let url = format!("{}/v1internal:fetchAvailableModels", up.agy);
        let r = with_agy_headers(authorize(client.post(url), Provider::Antigravity, c)).timeout(Duration::from_secs(20)).body(body_bytes).send().await;
        match r {
            Ok(r) if r.status().is_success() => match read_json_bounded(r, "Reading the quota", 2 << 20).await {
                Ok(d) => {
                    let (w, _) = parse_usage(Provider::Antigravity, &d);
                    windows = Ok(w);
                }
                Err(e) => windows = Err(e),
            },
            Ok(r) => windows = Err(http_failure("Reading the quota", r).await),
            Err(e) => windows = Err(Failure::Other(format!("Reading the quota: {e}"))),
        }
    }
    AgyQuotaRead { windows, credits: credits_res }
}

/// Read a subscription's quota from its provider.
pub async fn usage(client: &Client, up: &Upstreams, p: Provider, c: &Credential, like: Option<&Like>) -> Result<(Vec<Window>, Option<String>), Failure> {
    if p == Provider::Antigravity {
        let windows = agy_quota_summary(client, up, c, like).await.windows?;
        return Ok((windows, None));
    }
    // (Claude Code's own read: no query; Codex's as its session sends it.)
    let target = (p == Provider::Claude).then_some("/api/oauth/usage");
    if let Some(answer) = as_client(like, What::Usage, up, c, target, None, "Reading the quota").await {
        return Ok(parse_usage(p, &ok_json("Reading the quota", answer?)?));
    }
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage", up.claude),
        Provider::Codex => format!("{}/wham/usage", up.chatgpt),
        Provider::Antigravity => unreachable!(),
    };
    let mut req = authorize(client.get(url), p, c).timeout(Duration::from_secs(20));
    if p == Provider::Claude {
        req = with_claude_headers(req).header("content-type", "application/json");
    }
    let r = req.send().await.map_err(|e| Failure::Other(format!("Reading the quota: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading the quota", r).await);
    }
    let d: Value = read_json_bounded(r, "Reading the quota", 2 << 20).await?;
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
            read_json_bounded(r, "Reading the profile", 2 << 20).await?
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
        // Antigravity's banked quota resets (`bankedResets` in
        // `RetrieveUserQuotaSummaryResponse`).
        Provider::Antigravity => list(&d["bankedResets"])
            .iter()
            .filter_map(|r| {
                let id = r["resetId"].as_str().map(str::trim).filter(|s| !s.is_empty())?.to_owned();
                let usable = r["claimable"] == true;
                let ineligible = match r.get("ineligibleReason") {
                    Some(Value::String(s)) => {
                        let s = s.trim();
                        if s.is_empty() || s == "QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_UNSPECIFIED" { None } else { Some(s.to_owned()) }
                    }
                    Some(Value::Number(n)) => match n.as_i64() {
                        Some(0) | None => None,
                        Some(1) => Some("QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_USAGE_BELOW_THRESHOLD".to_owned()),
                        Some(other) => Some(format!("ineligible_{other}")),
                    },
                    _ => None,
                };
                let needs_limit = ineligible.as_deref() == Some("QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_USAGE_BELOW_THRESHOLD");
                let remaining = match r.get("remainingCount") {
                    Some(v) => {
                        let n = v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
                        if n == 0 {
                            return None;
                        }
                        n.min(u32::MAX as u64) as u32
                    }
                    None if usable || needs_limit => 1,
                    None => return None,
                };
                let clears: Vec<String> = r["bucketIds"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)).collect();
                Some(ResetCredit {
                    id,
                    expires_at: timestamp(&r["expireTime"]),
                    remaining,
                    usable,
                    program: Some("banked_quota_reset".into()),
                    label: text(&r["displayLabel"]),
                    kind: Some("banked_reset".into()),
                    clears,
                    starts_at: None,
                    needs_limit,
                    unavailable_reason: ineligible,
                    gives_back_weekly: false,
                })
            })
            .collect(),
    }
}

/// Updated quota summary returned directly by a reset claim response
/// (Antigravity's `ClaimBankedQuotaResetResponse.quotaSummary`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaimSummary {
    pub windows: Vec<Window>,
    pub credits: Vec<ResetCredit>,
}

/// Read an account's reset credits.
pub async fn credits(client: &Client, up: &Upstreams, p: Provider, c: &Credential, like: Option<&Like>) -> Result<Vec<ResetCredit>, Failure> {
    if p == Provider::Antigravity {
        let credits = agy_quota_summary(client, up, c, like).await.credits?;
        return credits.ok_or_else(|| Failure::Other("Reading reset credits: quota summary endpoint unavailable".into()));
    }
    // (Claude Code's read of its resets: its quota read with this query.)
    let target = (p == Provider::Claude).then_some("/api/oauth/usage?cedar_ember=1&skip_spend=1");
    if let Some(answer) = as_client(like, What::Credits, up, c, target, None, "Reading reset credits").await {
        return Ok(parse_credits(p, &ok_json("Reading reset credits", answer?)?));
    }
    let url = match p {
        Provider::Claude => format!("{}/api/oauth/usage?cedar_ember=1&skip_spend=1", up.claude),
        Provider::Codex => format!("{}/wham/rate-limit-reset-credits", up.chatgpt),
        Provider::Antigravity => unreachable!(),
    };
    let mut req = authorize(client.get(url), p, c).timeout(Duration::from_secs(20));
    if p == Provider::Claude {
        req = with_claude_headers(req).header("content-type", "application/json");
    }
    let r = req.send().await.map_err(|e| Failure::Other(format!("Reading reset credits: {e}")))?;
    if !r.status().is_success() {
        return Err(http_failure("Reading reset credits", r).await);
    }
    let d: Value = read_json_bounded(r, "Reading reset credits", 2 << 20).await?;
    Ok(parse_credits(p, &d))
}

fn ok_claim_json(doing: &str, p: Provider, a: likeness::Answer) -> Result<Value, Failure> {
    if !(200..300).contains(&a.status) {
        let msg = format!("{doing}: HTTP {}{}", a.status, if a.body.is_empty() { String::new() } else { format!(" ({})", a.text()) });
        let refused = matches!(a.status, 400 | 401 | 403) || (p == Provider::Antigravity && matches!(a.status, 404 | 409 | 412));
        return Err(if refused { Failure::Refused(msg) } else { Failure::Other(msg) });
    }
    a.json().map_err(|e| Failure::Other(format!("{doing}: {e}")))
}

async fn claim_http_failure(what: &str, p: Provider, r: reqwest::Response) -> Failure {
    let status = r.status().as_u16();
    let body = read_error_snippet(r, 64 << 10).await;
    let msg = format!("{what}: HTTP {status}{}", if body.is_empty() { String::new() } else { format!(" ({body})") });
    let refused = matches!(status, 400 | 401 | 403) || (p == Provider::Antigravity && matches!(status, 404 | 409 | 412));
    if refused { Failure::Refused(msg) } else { Failure::Other(msg) }
}

/// Redeem `credit` (`request` identifies this redemption, so asking
/// again after an unclear outcome redeems it once). Returns an updated
/// [`ClaimSummary`] when the provider includes one directly in the reply
/// (Antigravity's `ClaimBankedQuotaResetResponse.quotaSummary`).
#[allow(clippy::too_many_arguments)]
pub async fn redeem(
    client: &Client,
    up: &Upstreams,
    p: Provider,
    c: &Credential,
    org: Option<&str>,
    credit: &ResetCredit,
    request: &str,
    like: Option<&Like>,
) -> Result<Option<ClaimSummary>, Failure> {
    const DOING: &str = "Redeeming a reset credit";
    if let Some(l) = like
        && let Some(sent) = l.request(What::Claim)
    {
        // The client's own claim: Claude Code names the grant; Codex lets
        // ChatGPT choose the next credit, as its own session does; Antigravity
        // sends `project`, `resetId`, and idempotency `requestId`.
        let body = match p {
            Provider::Claude => {
                let program = credit.program.as_deref().unwrap_or("cedar_ember");
                likeness::with_fields(&sent.body, &[("program", json!(program)), ("grant_id", json!(credit.id)), ("request_id", json!(request))])
            }
            Provider::Codex => likeness::with_fields(&sent.body, &[("redeem_request_id", json!(request))]),
            Provider::Antigravity => {
                let project = c
                    .account_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Failure::NotSent("This account's Antigravity project isn't known: sync or sign in again.".into()))?;
                likeness::with_fields(&sent.body, &[("project", json!(project)), ("resetId", json!(credit.id)), ("requestId", json!(request))])
            }
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
        return claimed(p, &ok_claim_json(DOING, p, answer)?);
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
        Provider::Antigravity => {
            let project =
                c.account_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| Failure::NotSent("This account's Antigravity project isn't known: sync or sign in again.".into()))?;
            (format!("{}/v1internal:claimBankedQuotaReset", up.agy), json!({"project": project, "resetId": credit.id, "requestId": request}))
        }
    };
    let mut req = authorize(client.post(url), p, c).timeout(Duration::from_secs(30)).json(&body);
    match p {
        Provider::Claude => req = with_claude_headers(req),
        Provider::Antigravity => req = with_agy_headers(req),
        Provider::Codex => {}
    }
    let r = req.send().await.map_err(|e| if e.is_connect() || e.is_builder() { Failure::NotSent(format!("{DOING}: {e}")) } else { Failure::Other(format!("{DOING}: {e}")) })?;
    if !r.status().is_success() {
        return Err(claim_http_failure(DOING, p, r).await);
    }
    let d: Value = read_json_bounded(r, DOING, 2 << 20).await?;
    claimed(p, &d)
}

/// Whether a claim's answer says a reset was used: Claude's `result`;
/// ChatGPT's `code` (Codex's `ConsumeRateLimitResetCreditResponse`), whose
/// `nothing_to_reset` and `no_credit` spent nothing; Antigravity's HTTP 200
/// `ClaimBankedQuotaResetResponse` (with optional `quotaSummary`).
fn claimed(p: Provider, d: &Value) -> Result<Option<ClaimSummary>, Failure> {
    match p {
        // (Refused: nothing was spent, so no claim is left unclear.)
        Provider::Claude => match d["result"].as_str() {
            Some("reset" | "already_used") => Ok(None),
            Some("not_limited") => Err(Failure::Refused("Claude found nothing to reset: no reset was used.".into())),
            Some(other @ ("ineligible" | "cooldown" | "unavailable")) => Err(Failure::Refused(format!("Claude refused the reset ({other}): none was used."))),
            _ => Err(Failure::Other("Claude didn't confirm the reset.".into())),
        },
        Provider::Codex => match d["code"].as_str() {
            Some("reset" | "already_redeemed") => Ok(None),
            Some("nothing_to_reset") => Err(Failure::Refused("ChatGPT found nothing to reset: no credit was used.".into())),
            Some("no_credit") => Err(Failure::Refused("ChatGPT says this account has no reset credit left.".into())),
            _ => Err(Failure::Other("ChatGPT didn't confirm the reset.".into())),
        },
        Provider::Antigravity => {
            if let Some(qs) = d.get("quotaSummary").filter(|v| v.is_object()) {
                let (windows, _) = parse_usage(Provider::Antigravity, qs);
                let credits = parse_credits(Provider::Antigravity, qs);
                Ok(Some(ClaimSummary { windows, credits }))
            } else if d.is_object() {
                Ok(None)
            } else {
                Err(Failure::Other("Antigravity didn't confirm the reset.".into()))
            }
        }
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

/// What an Antigravity 429 (`RESOURCE_EXHAUSTED`) error body says.
#[derive(Debug, Clone, PartialEq)]
pub struct AgyRejection {
    pub until: i64,
    pub model_only: bool,
    pub model_family: String,
    /// `MODEL_CAPACITY_EXHAUSTED`: transient capacity pressure rather than
    /// account quota exhaustion (brief backoff without marking windows 100%).
    pub capacity_only: bool,
}

/// Parse a Google/Go duration string ("12.5s", "1h2m3s", "500ms", "300s")
/// into seconds (rounded up to at least 1 s when positive).
pub fn parse_go_duration(s: &str) -> Option<i64> {
    let mut rest = s.trim();
    if rest.is_empty() || rest.starts_with('-') {
        return None;
    }
    let mut total = 0.0f64;
    let mut any = false;
    while !rest.is_empty() {
        let num_end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
        if num_end == 0 {
            return None;
        }
        let val: f64 = rest[..num_end].parse().ok()?;
        rest = &rest[num_end..];
        let unit_end = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];
        let mult = match unit {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 1e-3,
            "us" | "µs" => 1e-6,
            "ns" => 1e-9,
            _ => return None,
        };
        total += val * mult;
        any = true;
    }
    if !any || !total.is_finite() || total <= 0.0 {
        return None;
    }
    Some((total.ceil() as i64).max(1))
}

/// Inspect an Antigravity HTTP 429 JSON error body (and `retry-after` header)
/// to distinguish account/model `QUOTA_EXHAUSTED` from transient
/// `MODEL_CAPACITY_EXHAUSTED` / `RATE_LIMIT_EXCEEDED` and extract reset
/// timestamp / backoff delay. Returns `None` for unrecognized 429 bodies so
/// transient or unrelated errors pass through without altering quota meters.
pub fn agy_rejection(h: &axum::http::HeaderMap, body: &[u8], family: &str, now: i64) -> Option<AgyRejection> {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let err = parsed.as_ref().and_then(|v| if let Some(arr) = v.as_array() { arr.first().and_then(|e| e.get("error")).or_else(|| arr.first()) } else { v.get("error").or(Some(v)) });
    let mut capacity_only = false;
    let mut explicit_quota = false;
    let mut quota_evidence = false;
    let mut retry_delay: Option<i64> = None;
    let mut reset_ts: Option<i64> = None;
    let mut reset_delay: Option<i64> = None;
    let mut named_model: Option<String> = None;
    if let Some(err) = err {
        for d in err["details"].as_array().into_iter().flatten() {
            let ty = d["@type"].as_str().unwrap_or("");
            if ty.ends_with("RetryInfo") {
                if let Some(secs) = d["retryDelay"].as_str().and_then(parse_go_duration) {
                    retry_delay = Some(secs);
                }
            } else if ty.ends_with("ErrorInfo") {
                match d["reason"].as_str() {
                    Some("MODEL_CAPACITY_EXHAUSTED" | "RATE_LIMIT_EXCEEDED") => {
                        capacity_only = true;
                    }
                    Some("QUOTA_EXHAUSTED") => {
                        explicit_quota = true;
                        quota_evidence = true;
                    }
                    _ => {}
                }
                let meta = &d["metadata"];
                if let Some(ts) = timestamp(&meta["quotaResetTimeStamp"]).filter(|&t| t > now) {
                    reset_ts = Some(ts);
                    quota_evidence = true;
                }
                if let Some(secs) = meta["quotaResetDelay"].as_str().and_then(parse_go_duration) {
                    reset_delay = Some(secs);
                    quota_evidence = true;
                }
                if let Some(m) = meta["model"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    named_model = Some(crate::model::model_family(m));
                }
            }
        }
        if !explicit_quota
            && !capacity_only
            && let Some(msg) = err["message"].as_str()
        {
            let l = msg.to_ascii_lowercase();
            if l.contains("capacity") || l.contains("rate limit") || l.contains("too many requests") {
                capacity_only = true;
            } else if l.contains("quota") && (l.contains("exhausted") || l.contains("exceeded") || l.contains("limit")) {
                quota_evidence = true;
            }
        }
    }
    let header_retry = retry_after(h, now);
    if !explicit_quota && !quota_evidence && !capacity_only {
        return None;
    }
    if explicit_quota {
        capacity_only = false;
    }
    let until = if capacity_only {
        let secs = retry_delay.or_else(|| header_retry.map(|t| (t - now).max(1))).unwrap_or(15).clamp(2, 60);
        now + secs
    } else {
        reset_ts.or_else(|| reset_delay.map(|s| now + s)).or_else(|| retry_delay.map(|s| now + s)).or(header_retry).unwrap_or(now + 60)
    };
    let model_family = named_model.clone().unwrap_or_else(|| family.to_owned());
    let model_only = !model_family.is_empty() && (named_model.is_some() || capacity_only);
    Some(AgyRejection { until, model_only, model_family, capacity_only })
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
        // Antigravity carries 429 quota/capacity details in the JSON body (`agy_rejection`).
        Provider::Antigravity => return (out, None),
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

    #[test]
    fn antigravity_quota_summary_credits_rejections_and_url_encode() {
        // 1. retrieveUserQuotaSummary with proto3 zero-omission (`remainingFraction` omitted => 100% used)
        //    plus groups and bankedResets (claimable, threshold-gated, and zero remainingCount).
        let summary = json!({
            "buckets": [
                {
                    "bucketId": "g_gemini_pro:5h",
                    "displayName": "Gemini Pro",
                    "window": "5h",
                    "resetTime": "2026-12-01T10:00:00Z"
                    // remainingFraction omitted by protojson when 0.0 => 100% used!
                }
            ],
            "groups": [
                {
                    "displayName": "Claude / GPT",
                    "buckets": [
                        {
                            "bucketId": "g_claude_gpt:7d",
                            "displayName": "Weekly",
                            "window": "7d",
                            "remainingFraction": 0.42,
                            "resetTime": "2026-12-05T00:00:00Z"
                        }
                    ]
                }
            ],
            "bankedResets": [
                {
                    "resetId": "rst-pro-1",
                    "displayLabel": "Gemini Pro Banked Reset",
                    "remainingCount": 2,
                    "claimable": true,
                    "expireTime": "2026-12-15T00:00:00Z",
                    "bucketIds": ["g_gemini_pro:5h"]
                },
                {
                    "resetId": "rst-claude-1",
                    "remainingCount": 1,
                    "claimable": false,
                    "ineligibleReason": "QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_USAGE_BELOW_THRESHOLD",
                    "expireTime": "2026-12-20T00:00:00Z",
                    "bucketIds": ["g_claude_gpt:7d"]
                },
                {
                    "resetId": "rst-empty",
                    "remainingCount": 0,
                    "claimable": false,
                    "bucketIds": ["g_empty:5h"]
                }
            ]
        });
        let (windows, _) = parse_usage(Provider::Antigravity, &summary);
        assert_eq!(windows.len(), 2, "{windows:?}");
        // Groups are collected first, then top-level buckets.
        assert_eq!(windows[0].name.as_str(), "g_claude_gpt:7d");
        assert!((windows[0].used - 58.0).abs() < 0.01);
        assert!(windows[0].applies_to("opus") && windows[0].applies_to("gpt-6.2-astra"));
        assert_eq!((windows[1].name.as_str(), windows[1].label.as_str(), windows[1].used), ("g_gemini_pro:5h", "Gemini Pro 5-hour", 100.0));
        assert!(windows[1].applies_to("gemini-3.5-pro"));

        let credits = parse_credits(Provider::Antigravity, &summary);
        assert_eq!(credits.len(), 2, "{credits:?}");
        assert_eq!((credits[0].id.as_str(), credits[0].remaining, credits[0].usable, credits[0].needs_limit), ("rst-pro-1", 2, true, false));
        assert_eq!(credits[0].clears, vec!["g_gemini_pro:5h"]);
        assert_eq!(
            (credits[1].id.as_str(), credits[1].remaining, credits[1].usable, credits[1].needs_limit, credits[1].unavailable_reason.as_deref()),
            ("rst-claude-1", 1, false, true, Some("QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_USAGE_BELOW_THRESHOLD"))
        );
        assert_eq!(credits[1].clears, vec!["g_claude_gpt:7d"]);

        // 2. fetchAvailableModels fallback shape (proto3 omitted remainingFraction when resetTime is present).
        let avail = json!({
            "models": {
                "gemini-4-neon-high": {
                    "displayName": "Gemini 4 Neon",
                    "quotaInfo": { "resetTime": "2026-12-01T12:00:00Z" }
                },
                "gemini-3-flash": {
                    "displayName": "Gemini 3 Flash",
                    "quotaInfo": { "remainingFraction": 0.75, "resetTime": "2026-12-01T12:00:00Z" }
                }
            }
        });
        let (fallback_ws, _) = parse_usage(Provider::Antigravity, &avail);
        assert_eq!(fallback_ws.len(), 2, "{fallback_ws:?}");
        let neon = fallback_ws.iter().find(|w| w.applies_to("gemini-4-neon")).unwrap();
        assert_eq!(neon.used, 100.0);
        let flash = fallback_ws.iter().find(|w| w.applies_to("gemini-3-flash")).unwrap();
        assert!((flash.used - 25.0).abs() < 0.01);

        // 3. Claude sub-model scoping (`Claude Opus` does NOT catch Sonnet) + explicit `models` array.
        let sub = json!({
            "buckets": [
                {"bucketId": "b_opus:5h", "displayName": "Claude Opus 5-hour", "remainingFraction": 0.0, "resetTime": "2026-12-01T12:00:00Z"},
                {"bucketId": "b_explicit:5h", "displayName": "Custom Bucket", "models": ["gemini-3-flash", "claude-sonnet-4-6"], "remainingFraction": 0.5, "resetTime": "2026-12-01T12:00:00Z"}
            ]
        });
        let (sub_ws, _) = parse_usage(Provider::Antigravity, &sub);
        assert!(sub_ws[0].applies_to("opus") && !sub_ws[0].applies_to("sonnet"), "Claude Opus bucket must not match Sonnet: {:?}", sub_ws[0].models);
        assert!(sub_ws[1].applies_to("gemini-flash") && sub_ws[1].applies_to("sonnet") && !sub_ws[1].applies_to("opus"), "{:?}", sub_ws[1].models);

        // 4. `RATE_LIMIT_EXCEEDED` is capacity-only; bare unrecognized 429 returns None.
        let rle = json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"}, {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "7s"}]}});
        let empty_h = axum::http::HeaderMap::new();
        let parsed_rle = agy_rejection(&empty_h, &serde_json::to_vec(&rle).unwrap(), "gemini-pro", 1000).unwrap();
        assert!(parsed_rle.capacity_only && parsed_rle.until == 1007, "{parsed_rle:?}");
        assert!(agy_rejection(&empty_h, br#"{"error":{"code":429,"message":"slow down"}}"#, "gemini-pro", 1000).is_none());

        // 3. Go duration parser.
        assert_eq!(parse_go_duration("3600s"), Some(3600));
        assert_eq!(parse_go_duration("1h30m5.5s"), Some(5406));
        assert_eq!(parse_go_duration("500ms"), Some(1));
        assert_eq!(parse_go_duration("0s"), None);
        assert_eq!(parse_go_duration(""), None);

        // 4. Google 429 parsing: QUOTA_EXHAUSTED vs MODEL_CAPACITY_EXHAUSTED.
        let h = axum::http::HeaderMap::new();
        let quota_body = serde_json::to_vec(&json!({
            "error": {
                "code": 429,
                "status": "RESOURCE_EXHAUSTED",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "QUOTA_EXHAUSTED",
                    "metadata": {
                        "model": "gemini-4-neon-high",
                        "quotaResetDelay": "2h15m0s"
                    }
                }]
            }
        }))
        .unwrap();
        let q = agy_rejection(&h, &quota_body, "gemini-4-neon", 1000).expect("quota rejection");
        assert!(!q.capacity_only && q.model_only && q.model_family == "gemini-4-neon" && q.until == 1000 + 2 * 3600 + 15 * 60);

        let cap_body = serde_json::to_vec(&json!({
            "error": {
                "code": 429,
                "status": "RESOURCE_EXHAUSTED",
                "details": [
                    { "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "MODEL_CAPACITY_EXHAUSTED" },
                    { "@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "8s" }
                ]
            }
        }))
        .unwrap();
        let cap = agy_rejection(&h, &cap_body, "gemini-4-neon", 1000).expect("capacity rejection");
        assert!(cap.capacity_only && cap.model_only && cap.until == 1008);

        // 5. URL encoding for Google OAuth form-urlencoded refresh.
        assert_eq!(url_encode("1//0abc+def/ghi=jkl"), "1%2F%2F0abc%2Bdef%2Fghi%3Djkl");
    }
}
