//! What a client is told of its usage. The pool is many accounts and
//! their reset credits; a client sees one account's meters, the one that
//! answered it, and takes them for all there is: "Approaching weekly
//! limit", "Now using usage credits", a model switched down, an agent
//! that paces itself or stops, while the pool has accounts untouched.
//! Ben, 2026-11-26: "change the way token pool works so that it always
//! reports to the clients full usage when there remain additional
//! accounts or resets to draw from (for both claude and codex)"; and of
//! what the Claude CLI showed him meanwhile: "ignore the usage stats for
//! the claude cli they are wrong -- we have resets and they will be
//! applied automatically."
//!
//! So: for as long as the pool has more to draw from than the account
//! that answered ([`more`]: another account with allowance left for the
//! model asked for, or a reset credit that can be claimed), what goes
//! back says nothing is used: an answer's rate-limit headers
//! ([`headers`]) and a usage report's figures ([`report`]). When the
//! account that answers is the last, with nothing behind it, the client
//! is told how that one really stands, as before.
//!
//! The pool's own reading is of what the provider said, before any of
//! this: its meters, its choice of account and its window are never
//! what a client was told.
//!
//! Not told so yet: a Codex socket's own `codex.rate_limits` events,
//! which pass byte for byte in frames the pool only reads (compressed
//! with a context kept from message to message: one changed or left out
//! would break every message after it). TODO.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::model::{Account, Provider};

/// Whether the pool has more to draw from for `provider`'s `family` than
/// the account `serving` (none: than nothing; a usage report asks so):
/// another account that can serve it now with its own allowance, or a
/// reset credit that can be used now, on any of the provider's accounts
/// that are in use (a reset gives an allowance back whichever account
/// answered last).
pub fn more(accounts: &[Account], provider: Provider, family: &str, serving: Option<&str>, now: i64, has_credential: impl Fn(&Account) -> bool) -> bool {
    let mut in_use = accounts.iter().filter(|a| a.provider == provider && a.enabled && a.signed_out.is_none() && has_credential(a));
    in_use.any(|a| (Some(a.id.as_str()) != serving && crate::schedule::eligible(a, provider, family, now, true)) || a.credits.iter().any(|c| c.usable_at(now)))
}

/// An answer's headers as a client with all its allowance left would get
/// them: every meter at nothing used, no warning, no refusal's status,
/// not on usage credits. When a meter resets, and everything that isn't
/// about how much is used, stays as the provider said it.
pub fn headers(provider: Provider, h: &mut HeaderMap) {
    let names: Vec<HeaderName> = h.keys().cloned().collect();
    let zero = HeaderValue::from_static("0");
    match provider {
        Provider::Claude => {
            for name in names {
                let Some(rest) = name.as_str().strip_prefix("anthropic-ratelimit-unified-") else { continue };
                // (What is paid for beyond the plan is its own account:
                // its figures aren't the allowance's, and stay.)
                let of_credits = rest.starts_with("overage-");
                if rest.ends_with("-utilization") && !of_credits {
                    h.insert(name, zero.clone());
                } else if rest == "status" || (rest.ends_with("-status") && !of_credits) {
                    h.insert(name, HeaderValue::from_static("allowed"));
                } else if rest.ends_with("-surpassed-threshold") && !of_credits {
                    h.remove(name);
                } else if rest == "overage-in-use" {
                    h.insert(name, HeaderValue::from_static("false"));
                }
            }
        }
        Provider::Codex => {
            for name in names {
                let Some(rest) = name.as_str().strip_prefix("x-codex-") else { continue };
                if rest.ends_with("-used-percent") {
                    h.insert(name, zero.clone());
                } else if rest == "rate-limit-reached-type" {
                    h.remove(name);
                }
            }
        }
    }
}

/// A usage report (Claude's `/api/oauth/usage`, ChatGPT's
/// `/backend-api/wham/usage`) as one with all its allowance left reads:
/// every meter's share used at nothing, each where the provider puts it
/// (the fields `upstream::parse_usage` reads them from). Whether
/// anything was changed.
pub fn report(provider: Provider, d: &mut Value) -> bool {
    let mut changed = false;
    let mut nothing = |v: &mut Value| {
        if v.as_f64().is_some_and(|used| used != 0.0) {
            *v = Value::from(0);
            changed = true;
        }
    };
    match provider {
        Provider::Claude => {
            for row in d.get_mut("limits").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(v) = row.get_mut("percent") {
                    nothing(v);
                }
            }
            for (name, w) in d.as_object_mut().into_iter().flatten() {
                if (name.starts_with("five_hour") || name.starts_with("seven_day"))
                    && !name.ends_with("_breakdown")
                    && let Some(v) = w.get_mut("utilization")
                {
                    nothing(v);
                }
            }
        }
        Provider::Codex => {
            let mut limit = |limit: Option<&mut Value>| {
                let Some(limit) = limit else { return };
                for kind in ["primary_window", "secondary_window"] {
                    if let Some(v) = limit.get_mut(kind).and_then(|w| w.get_mut("used_percent")) {
                        nothing(v);
                    }
                }
                // (ChatGPT's own word that the limit is reached.)
                for (flag, to) in [("limit_reached", false), ("allowed", true)] {
                    if limit.get(flag).is_some_and(|v| v.is_boolean() && v != &Value::Bool(to)) {
                        limit[flag] = Value::Bool(to);
                    }
                }
            };
            limit(d.get_mut("rate_limit"));
            limit(d.get_mut("code_review_rate_limit"));
            for extra in d.get_mut("additional_rate_limits").and_then(Value::as_array_mut).into_iter().flatten() {
                limit(extra.get_mut("rate_limit"));
            }
        }
    }
    changed
}

/// Which provider's usage report a request is for, by its host and path
/// (as the clients ask: Claude Code's `/usage`, its status line and its
/// near-limit notices; Codex's `/status`).
pub fn is_report(host: &str, path: &str) -> Option<Provider> {
    match host {
        "api.anthropic.com" if path == "/api/oauth/usage" => Some(Provider::Claude),
        "chatgpt.com" if path == "/backend-api/wham/usage" => Some(Provider::Codex),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ResetCredit, Window};
    use serde_json::json;

    fn account(id: &str, used: f64) -> Account {
        let mut a = Account::new(id.into(), id.into(), Provider::Claude);
        a.windows = vec![Window { name: "seven_day".into(), label: "Weekly".into(), used, resets_at: Some(9000), models: vec![] }];
        a
    }

    /// More to draw from: another account with allowance, or a reset to
    /// claim; not the serving account's own allowance, not one turned
    /// off, signed out or with no sign-in held, not another provider's,
    /// not one that is out, not a reset that can't be used.
    #[test]
    fn what_more_there_is_to_draw_from() {
        let more = |accounts: &[Account], serving: Option<&str>| super::more(accounts, Provider::Claude, "opus", serving, 1000, |_| true);
        let (a, b) = (account("a", 97.0), account("b", 10.0));
        assert!(!more(&[a.clone()], Some("a")), "the last account, alone");
        assert!(more(&[a.clone(), b.clone()], Some("a")), "another with allowance");
        assert!(more(&[a.clone(), b.clone()], Some("b")), "a has 3% left: allowance still");
        assert!(more(&[a.clone()], None), "a report asks of all of them");
        let mut out = account("b", 100.0);
        assert!(!more(&[a.clone(), out.clone()], Some("a")), "the other is out");
        out.credits = vec![ResetCredit { id: "g".into(), remaining: 1, usable: true, ..Default::default() }];
        assert!(more(&[a.clone(), out.clone()], Some("a")), "a reset to claim");
        out.credits[0].usable = false;
        assert!(!more(&[a.clone(), out.clone()], Some("a")), "a reset that can't be used");
        let mut off = b.clone();
        off.enabled = false;
        assert!(!more(&[a.clone(), off], Some("a")));
        let mut gone = b.clone();
        gone.signed_out = Some("refused".into());
        assert!(!more(&[a.clone(), gone], Some("a")));
        assert!(!super::more(&[a.clone(), b.clone()], Provider::Claude, "opus", Some("a"), 1000, |x| x.id == "a"), "no sign-in held for it");
        let mut codex = b.clone();
        codex.provider = Provider::Codex;
        assert!(!more(&[a.clone(), codex], Some("a")));
        let mut paid = b.clone();
        paid.now_paid("opus", false, 9000, "five_hour");
        assert!(!more(&[a, paid], Some("a")), "one answered from usage credits has no allowance");
    }

    /// Claude's headers: every meter at nothing, status allowed, not on
    /// credits; resets and the rest as they came; the credits' own
    /// figures left.
    #[test]
    fn claudes_headers_say_nothing_is_used() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-5h-status", "allowed_warning"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.93"),
            ("anthropic-ratelimit-unified-5h-reset", "5000"),
            ("anthropic-ratelimit-unified-7d-utilization", "1"),
            ("anthropic-ratelimit-unified-7d_oi-utilization", "0.53"),
            ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.75"),
            ("anthropic-ratelimit-unified-representative-claim", "seven_day"),
            ("anthropic-ratelimit-unified-reset", "9000"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
            ("anthropic-ratelimit-unified-overage-utilization", "0.2"),
            ("anthropic-ratelimit-unified-overage-in-use", "true"),
            ("request-id", "req_1"),
        ] {
            h.insert(k, v.parse().unwrap());
        }
        headers(Provider::Claude, &mut h);
        let got = |k: &str| h.get(k).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(got("anthropic-ratelimit-unified-status").as_deref(), Some("allowed"));
        assert_eq!(got("anthropic-ratelimit-unified-5h-status").as_deref(), Some("allowed"));
        for meter in ["5h", "7d", "7d_oi"] {
            assert_eq!(got(&format!("anthropic-ratelimit-unified-{meter}-utilization")).as_deref(), Some("0"), "{meter}");
        }
        assert_eq!(got("anthropic-ratelimit-unified-7d-surpassed-threshold"), None);
        assert_eq!(got("anthropic-ratelimit-unified-overage-in-use").as_deref(), Some("false"));
        // (As they came.)
        assert_eq!(
            (got("anthropic-ratelimit-unified-5h-reset").as_deref(), got("anthropic-ratelimit-unified-reset").as_deref(), got("request-id").as_deref()),
            (Some("5000"), Some("9000"), Some("req_1"))
        );
        assert_eq!((got("anthropic-ratelimit-unified-overage-status").as_deref(), got("anthropic-ratelimit-unified-overage-utilization").as_deref()), (Some("allowed"), Some("0.2")));
        assert_eq!(got("anthropic-ratelimit-unified-representative-claim").as_deref(), Some("seven_day"));
        // (And what Claude Code makes of them is what the pool makes of
        // them: nothing paid for, no meter with anything used.)
        let (w, _) = crate::upstream::header_windows(&h, Provider::Claude, "opus", 200, 1000);
        assert!(w.iter().filter(|w| w.name != "overage").all(|w| w.used == 0.0), "{w:?}");
        assert_eq!(crate::upstream::paid_by_credits(&h, Provider::Claude, "opus", 200, 1000, &mut Vec::new()), None);
    }

    #[test]
    fn codexs_headers_say_nothing_is_used() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("x-codex-primary-used-percent", "81"),
            ("x-codex-primary-reset-after-seconds", "300"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-secondary-used-percent", "97.5"),
            ("x-codex-rate-limit-reached-type", "secondary"),
            ("x-codex-active-limit", "codex"),
        ] {
            h.insert(k, v.parse().unwrap());
        }
        headers(Provider::Codex, &mut h);
        let got = |k: &str| h.get(k).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!((got("x-codex-primary-used-percent").as_deref(), got("x-codex-secondary-used-percent").as_deref()), (Some("0"), Some("0")));
        assert_eq!(got("x-codex-rate-limit-reached-type"), None);
        assert_eq!(
            (got("x-codex-primary-reset-after-seconds").as_deref(), got("x-codex-primary-window-minutes").as_deref(), got("x-codex-active-limit").as_deref()),
            (Some("300"), Some("300"), Some("codex"))
        );
    }

    /// The reports: each meter where its provider puts it, read back by
    /// the pool's own reading of a report as nothing used; what isn't a
    /// share used (when it resets, the plan, what the week went on) as
    /// it came.
    #[test]
    fn the_reports_say_nothing_is_used() {
        let mut claude = json!({
            "five_hour": {"utilization": 91.0, "resets_at": "2026-11-27T02:00:00Z"},
            "seven_day": {"utilization": 100.0, "resets_at": "2026-11-29T19:00:00Z"},
            "seven_day_opus": null,
            "seven_day_fable": {"utilization": null, "resets_at": null},
            "seven_day_breakdown": {"utilization": 55.0},
            "limits": [
                {"kind": "session", "group": "session", "percent": 91, "resets_at": "2026-11-27T02:00:00Z"},
                {"kind": "weekly_all", "group": "weekly", "percent": 100, "resets_at": "2026-11-29T19:00:00Z"},
                {"kind": "weekly_model", "group": "weekly", "percent": 53, "scope": {"model": {"display_name": "Fable"}}}
            ],
            "extra_usage": {"is_enabled": true, "utilization": 12.0}
        });
        assert!(report(Provider::Claude, &mut claude));
        let (w, _) = crate::upstream::parse_usage(Provider::Claude, &claude);
        assert!(w.len() >= 3 && w.iter().all(|w| w.used == 0.0), "{w:?}");
        assert_eq!(claude["five_hour"]["resets_at"], "2026-11-27T02:00:00Z");
        assert_eq!((claude["seven_day_breakdown"]["utilization"].as_f64(), claude["extra_usage"]["utilization"].as_f64(), claude["seven_day_opus"].is_null()), (Some(55.0), Some(12.0), true));
        assert!(!report(Provider::Claude, &mut claude), "nothing more to change");

        let mut codex = json!({
            "plan_type": "pro",
            "rate_limit": {"allowed": false, "limit_reached": true, "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_after_seconds": 600}, "secondary_window": {"used_percent": 61, "limit_window_seconds": 604800, "reset_at": 1795800000}},
            "additional_rate_limits": [{"limit_name": "GPT-5.3-Codex-Spark", "metered_feature": "spark", "rate_limit": {"primary_window": {"used_percent": 40, "limit_window_seconds": 18000}}}],
            "code_review_rate_limit": {"primary_window": {"used_percent": 7, "limit_window_seconds": 604800}},
            "credits": {"balance": "3.20"}
        });
        assert!(report(Provider::Codex, &mut codex));
        let (w, plan) = crate::upstream::parse_usage(Provider::Codex, &codex);
        assert!(w.len() == 4 && w.iter().all(|w| w.used == 0.0), "{w:?}");
        assert_eq!((plan.as_deref(), codex["rate_limit"]["allowed"].as_bool(), codex["rate_limit"]["limit_reached"].as_bool()), (Some("pro"), Some(true), Some(false)));
        assert_eq!((codex["rate_limit"]["primary_window"]["reset_after_seconds"].as_i64(), codex["credits"]["balance"].as_str()), (Some(600), Some("3.20")));
        // (A report of another shape, or none: left as it is.)
        assert!(!report(Provider::Codex, &mut json!({"error": "nope"})));
        assert!(!report(Provider::Claude, &mut json!([1, 2])));
    }

    #[test]
    fn which_requests_are_usage_reports() {
        assert_eq!(is_report("api.anthropic.com", "/api/oauth/usage"), Some(Provider::Claude));
        assert_eq!(is_report("chatgpt.com", "/backend-api/wham/usage"), Some(Provider::Codex));
        assert_eq!(is_report("api.anthropic.com", "/v1/messages"), None);
        assert_eq!(is_report("chatgpt.com", "/api/oauth/usage"), None);
    }
}
