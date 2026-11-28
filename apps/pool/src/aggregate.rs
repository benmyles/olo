//! Pure data aggregation, custody resolution, host status grammar, tab badges,
//! and synthetic demo fixtures for the unified Token Pool window.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::{Path, PathBuf},
};

use olo_kit::{palette::Hue, widgets::Light};
use olo_token_pool::{
    activity::{Activity, Minute},
    gateway::{Open, Pool, Served},
    hosts::Stands,
    model::{Account, Outcome, Provider, RECENT, Request, ResetCredit, Usage, Window as QuotaWindow, now, today},
    running::{Connections, Daemon, Process, Running},
    schedule,
};
use serde_json::Value;

/// The three tabs of the unified Token Pool window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Accounts = 0,
    Activity = 1,
    Machines = 2,
}

impl Tab {
    pub fn from_index(i: usize) -> Tab {
        match i {
            1 => Tab::Activity,
            2 => Tab::Machines,
            _ => Tab::Accounts,
        }
    }

    pub fn parse_name(s: &str) -> Option<Tab> {
        match s.trim().to_ascii_lowercase().as_str() {
            "accounts" | "0" => Some(Tab::Accounts),
            "activity" | "clients" | "1" => Some(Tab::Activity),
            "machines" | "hosts" | "2" => Some(Tab::Machines),
            _ => None,
        }
    }
}

/// Which machine(s) the Activity tab is filtered to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MachineScope {
    #[default]
    All,
    Local,
    Host(String),
}

impl MachineScope {
    pub fn matches_local(&self) -> bool {
        matches!(self, MachineScope::All | MachineScope::Local)
    }

    pub fn matches_host(&self, host_id: &str) -> bool {
        match self {
            MachineScope::All => true,
            MachineScope::Local => false,
            MachineScope::Host(id) => id == host_id,
        }
    }
}

/// Where an account's refresh-token sign-in is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Custody {
    /// Renewed on This Computer.
    Here,
    /// Renewed on the named remote Rex host.
    OnHost(String),
    /// Handover to the named remote Rex host is pending acceptance.
    PendingHandover(String),
}

impl Custody {
    /// Machine display name for the account row's custody chip.
    pub fn machine_name(&self) -> String {
        match self {
            Custody::Here => olo_kit::words::tr("This Computer").to_string(),
            Custody::OnHost(name) | Custody::PendingHandover(name) => name.clone(),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Custody::Here)
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Custody::PendingHandover(_))
    }
}

/// Resolves an account's sign-in custody using `account.kept_on`, the host list
/// for friendly display names, and whether a local refresh token is still held
/// waiting for handover acceptance.
pub fn custody(account: &Account, hosts: &[Stands], unaccepted_ids: &[String]) -> Custody {
    let Some(host_id) = account.kept_on.as_deref() else {
        return Custody::Here;
    };
    let name = hosts.iter().find(|h| h.id == host_id).map(|h| h.name.clone()).unwrap_or_else(|| host_id.to_owned());
    if unaccepted_ids.iter().any(|id| id == &account.id) { Custody::PendingHandover(name) } else { Custody::OnHost(name) }
}

/// Whether `account` is out of quota at time `t`, returning `Some(resets_at)`
/// when blocked. Handles shared-window accounts (`schedule::blocked_until`)
/// AND Antigravity accounts with only model-scoped windows: a reported family
/// is out of quota if ANY window covering it is exhausted (e.g. 5-hour 100%
/// while weekly is 30%), and the account is out of quota when every reported
/// family is blocked.
pub fn account_out_of_quota(a: &Account, t: i64) -> Option<Option<i64>> {
    if let Some(until) = schedule::blocked_until(a, "", t) {
        return Some(Some(until));
    }
    let non_overage: Vec<&QuotaWindow> = a.windows.iter().filter(|w| w.name != "overage" && w.name != "seven_day_overage_included").collect();
    if !non_overage.is_empty() && non_overage.iter().all(|w| !w.models.is_empty()) {
        let mut families: Vec<&str> = Vec::new();
        for w in &non_overage {
            for m in &w.models {
                if !families.contains(&m.as_str()) {
                    families.push(m.as_str());
                }
            }
        }
        if !families.is_empty() && families.iter().all(|fam| a.model_cooldowns.get(*fam).is_some_and(|&c| c > t) || non_overage.iter().any(|w| w.applies_to(fam) && w.exhausted(t))) {
            let earliest = non_overage.iter().filter(|w| w.exhausted(t)).filter_map(|w| w.resets_at.filter(|&r| r > t)).chain(a.model_cooldowns.values().copied().filter(|&c| c > t)).min();
            return Some(earliest);
        }
    }
    None
}

/// Whether `account` is currently serving from paid usage credits on either its
/// shared quota or any model-scoped bucket.
#[allow(dead_code)]
pub fn account_any_paid(a: &Account, t: i64) -> bool {
    account_paid_scope(a, t).is_some()
}

/// Active paid-usage-credits state for `account` at `t`: returns
/// `Some((optional_family_label, resets_at))` when either account-wide `paid`
/// or any model-scoped `model_paid` entry is active (even if another model
/// family on the same account still has plan allowance).
pub fn account_paid_scope(a: &Account, t: i64) -> Option<(Option<String>, i64)> {
    if let Some(until) = a.paid.as_ref().filter(|p| p.until > t).map(|p| p.until) {
        return Some((None, until));
    }
    a.model_paid.iter().filter(|(_, p)| p.until > t).min_by_key(|(_, p)| p.until).map(|(fam, p)| {
        let label = a.windows.iter().find(|w| w.applies_to(fam) && !w.label.is_empty()).map(|w| w.label.clone()).unwrap_or_else(|| crate::plan_name(fam));
        (Some(label), p.until)
    })
}

/// Returns accounts for `prov` sorted with problems first (signed out, then
/// exhausted/blocked, then enabled, then paused) so tab alert badges point at
/// visible rows without scrolling.
pub fn sorted_accounts(accounts: &[Account], prov: Provider, t: i64) -> Vec<&Account> {
    let mut out: Vec<&Account> = accounts.iter().filter(|a| a.provider == prov).collect();
    out.sort_by_key(|a| {
        if a.signed_out.is_some() {
            0u8
        } else if a.enabled && account_out_of_quota(a, t).is_some() {
            1u8
        } else if a.enabled {
            2u8
        } else {
            3u8
        }
    });
    out
}

/// Returns account names whose sign-in is kept on `host_id` (or `This Computer`
/// when `host_id` is `None`).
pub fn kept_names(pool: &Pool, host_id: Option<&str>) -> Vec<String> {
    pool.saved.accounts.iter().filter(|a| a.kept_on.as_deref() == host_id).map(|a| a.name.clone()).collect()
}

/// Groups remotely kept accounts by host display name for `This Computer`'s
/// custody summary ("Keeps Personal, Sandbox · From build: Work Max, Team Plus"),
/// placing the source host name first so wrapping never strands it at the tail.
pub fn local_custody_summary(pool: &Pool, hosts: &[Stands]) -> Option<String> {
    let local = kept_names(pool, None);
    let mut by_host: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in &pool.saved.accounts {
        if let Some(hid) = a.kept_on.as_deref() {
            let name = hosts.iter().find(|h| h.id == hid).map(|h| h.name.clone()).unwrap_or_else(|| hid.to_owned());
            by_host.entry(name).or_default().push(a.name.clone());
        }
    }
    let mut parts = Vec::new();
    if !local.is_empty() {
        parts.push(format!("Keeps {}", local.join(", ")));
    }
    for (host_name, names) in by_host {
        parts.push(format!("From {host_name}: {}", names.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// An in-flight request on either This Computer or a remote Rex host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveReq {
    pub key: String,
    pub machine_id: String,
    pub machine_name: String,
    pub is_local: bool,
    pub account: String,
    pub account_id: String,
    pub provider: Provider,
    pub model: String,
    pub client: String,
    pub started_ms: i64,
}

/// A finished request on either This Computer or a remote Rex host.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentReq {
    pub key: String,
    pub machine_id: String,
    pub machine_name: String,
    pub is_local: bool,
    pub account: String,
    pub account_id: String,
    pub provider: Provider,
    pub request: Request,
}

/// Combined local + remote stats for one account row.
#[derive(Clone, Debug, Default)]
pub struct AccountRollup {
    pub total_today: Usage,
    #[allow(dead_code)]
    pub local_requests: u64,
    #[allow(dead_code)]
    pub remote_requests: u64,
    #[allow(dead_code)]
    pub busy_machines: Vec<String>,
    /// Up to [`RECENT`] newest requests for this account across all machines,
    /// ordered **newest first** (leftmost tick = newest).
    pub recent: Vec<RecentReq>,
}

fn parse_provider(v: &Value, model: &str, client: &str) -> Provider {
    if let Some(s) = v.as_str()
        && let Some(p) = Provider::parse(s)
    {
        return p;
    }
    let ml = model.to_ascii_lowercase();
    let cl = client.to_ascii_lowercase();
    if cl.contains("antigravity") || cl == "agy" || ml.contains("gemini") {
        Provider::Antigravity
    } else if ml.contains("claude") || cl.contains("claude") {
        Provider::Claude
    } else {
        Provider::Codex
    }
}

fn parse_outcome(v: &Value) -> Outcome {
    match v.as_str() {
        Some("completed" | "ok") | None => Outcome::Completed,
        Some("rate_limited" | "rate limited") => Outcome::RateLimited,
        Some("cancelled" | "canceled") => Outcome::Cancelled,
        Some(_) => Outcome::Failed,
    }
}

fn parse_usage(v: &Value) -> Usage {
    Usage { requests: v["requests"].as_u64().unwrap_or(0), input_tokens: v["input_tokens"].as_u64().unwrap_or(0), output_tokens: v["output_tokens"].as_u64().unwrap_or(0) }
}

fn parse_remote_request(v: &Value) -> Request {
    Request {
        at: v["at"].as_i64().unwrap_or(0),
        model: v["model"].as_str().unwrap_or("?").to_owned(),
        client: v["client"].as_str().unwrap_or("").to_owned(),
        outcome: parse_outcome(&v["outcome"]),
        status: v["status"].as_u64().unwrap_or(0) as u16,
        ms: v["ms"].as_u64().unwrap_or(0),
        usage: parse_usage(&v["usage"]),
    }
}

/// Live in-flight requests across the selected [`MachineScope`], **newest first**.
pub fn live_requests(pool: &Pool, hosts: &[Stands], scope: &MachineScope) -> Vec<LiveReq> {
    let mut out = Vec::new();
    let this_computer = olo_kit::words::tr("This Computer").to_string();
    if scope.matches_local() {
        for o in &pool.open {
            out.push(LiveReq {
                key: format!("local:{}", o.id),
                machine_id: "local".into(),
                machine_name: this_computer.clone(),
                is_local: true,
                account: o.account.clone(),
                account_id: o.account_id.clone(),
                provider: o.provider,
                model: o.model.clone(),
                client: o.client.clone(),
                started_ms: o.started,
            });
        }
    }
    for h in hosts {
        if !scope.matches_host(&h.id) {
            continue;
        }
        let Some(status) = &h.status else { continue };
        let Some(open) = status["open"].as_array() else { continue };
        for (idx, o) in open.iter().enumerate() {
            let id = o["id"].as_u64().unwrap_or(idx as u64);
            let model = o["model"].as_str().unwrap_or("?").to_owned();
            let client = o["client"].as_str().unwrap_or("").to_owned();
            let provider = parse_provider(&o["provider"], &model, &client);
            out.push(LiveReq {
                key: format!("host:{}:{id}", h.id),
                machine_id: h.id.clone(),
                machine_name: h.name.clone(),
                is_local: false,
                account: o["account"].as_str().unwrap_or("?").to_owned(),
                account_id: o["account_id"].as_str().unwrap_or("").to_owned(),
                provider,
                model,
                client,
                started_ms: o["started"].as_i64().unwrap_or(0),
            });
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.started_ms));
    out
}

/// Recent completed/failed requests across the selected [`MachineScope`],
/// ordered **newest first**, capped at `limit`.
pub fn recent_requests(pool: &Pool, hosts: &[Stands], scope: &MachineScope, limit: usize) -> Vec<RecentReq> {
    let mut out = Vec::new();
    let this_computer = olo_kit::words::tr("This Computer").to_string();
    if scope.matches_local() {
        for (idx, s) in pool.served.iter().enumerate() {
            out.push(RecentReq {
                key: format!("local:{}:{}:{idx}", s.request.at, s.account_id),
                machine_id: "local".into(),
                machine_name: this_computer.clone(),
                is_local: true,
                account: s.account.clone(),
                account_id: s.account_id.clone(),
                provider: s.provider,
                request: s.request.clone(),
            });
        }
    }
    for h in hosts {
        if !scope.matches_host(&h.id) {
            continue;
        }
        let Some(status) = &h.status else { continue };
        let Some(served) = status["served"].as_array() else { continue };
        for (idx, s) in served.iter().enumerate() {
            let req = parse_remote_request(s);
            let provider = parse_provider(&s["provider"], &req.model, &req.client);
            let account = s["account"].as_str().unwrap_or("?").to_owned();
            let account_id = s["account_id"].as_str().unwrap_or("").to_owned();
            out.push(RecentReq {
                key: format!("host:{}:{}:{idx}", h.id, req.at),
                machine_id: h.id.clone(),
                machine_name: h.name.clone(),
                is_local: false,
                account,
                account_id,
                provider,
                request: req,
            });
        }
    }
    out.sort_by(|a, b| b.request.at.cmp(&a.request.at).then_with(|| b.key.cmp(&a.key)));
    out.truncate(limit);
    out
}

/// Merges per-minute [`Activity`] across the selected [`MachineScope`].
pub fn merged_activity(local: &Activity, hosts: &[Stands], scope: &MachineScope) -> Activity {
    let mut buckets: BTreeMap<(i64, String), Minute> = BTreeMap::new();
    let mut add = |m: &Minute| {
        let client = if m.client.is_empty() { "Other".to_owned() } else { m.client.clone() };
        let entry = buckets.entry((m.minute, client.clone())).or_insert_with(|| Minute { minute: m.minute, client, ..Default::default() });
        entry.requests = entry.requests.saturating_add(m.requests);
        entry.failed = entry.failed.saturating_add(m.failed);
        entry.input_tokens = entry.input_tokens.saturating_add(m.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(m.output_tokens);
    };
    if scope.matches_local() {
        for m in &local.minutes {
            add(m);
        }
    }
    for h in hosts {
        if !scope.matches_host(&h.id) {
            continue;
        }
        let Some(status) = &h.status else { continue };
        let Some(arr) = status["activity"].as_array() else { continue };
        for v in arr {
            if let Ok(m) = serde_json::from_value::<Minute>(v.clone()) {
                add(&m);
            }
        }
    }
    Activity { minutes: buckets.into_values().collect(), dirty: false }
}

/// Computes combined local + remote stats and recent requests (newest first) for one account.
pub fn account_rollup(pool: &Pool, account: &Account, hosts: &[Stands]) -> AccountRollup {
    let this_computer = olo_kit::words::tr("This Computer").to_string();
    let local_today = if account.today.0 == today() { account.today.1.clone() } else { Usage::default() };
    let mut total_today = local_today.clone();
    let local_requests = local_today.requests;
    let mut remote_requests = 0u64;
    let mut busy_machines = Vec::new();
    if pool.busy(&account.id) {
        busy_machines.push(this_computer.clone());
    }

    let mut recent: Vec<RecentReq> = account
        .recent
        .iter()
        .enumerate()
        .map(|(i, r)| RecentReq {
            key: format!("acct:local:{}:{}:{i}", account.id, r.at),
            machine_id: "local".into(),
            machine_name: this_computer.clone(),
            is_local: true,
            account: account.name.clone(),
            account_id: account.id.clone(),
            provider: account.provider,
            request: r.clone(),
        })
        .collect();

    for h in hosts {
        let Some(status) = &h.status else { continue };
        if let Some(arr) = status["accounts"].as_array()
            && let Some(ha) = arr.iter().find(|a| a["id"].as_str() == Some(account.id.as_str()))
        {
            let u = parse_usage(&ha["today"]);
            total_today.requests = total_today.requests.saturating_add(u.requests);
            total_today.input_tokens = total_today.input_tokens.saturating_add(u.input_tokens);
            total_today.output_tokens = total_today.output_tokens.saturating_add(u.output_tokens);
            remote_requests = remote_requests.saturating_add(u.requests);
            if ha["in_flight"].as_u64().unwrap_or(0) > 0 {
                busy_machines.push(h.name.clone());
            }
            if let Some(r_arr) = ha["recent"].as_array() {
                for (i, rv) in r_arr.iter().enumerate() {
                    recent.push(RecentReq {
                        key: format!("acct:host:{}:{}:{}:{i}", h.id, account.id, rv["at"].as_i64().unwrap_or(0)),
                        machine_id: h.id.clone(),
                        machine_name: h.name.clone(),
                        is_local: false,
                        account: account.name.clone(),
                        account_id: account.id.clone(),
                        provider: account.provider,
                        request: parse_remote_request(rv),
                    });
                }
            }
        }
    }

    // Newest first (leftmost tick = newest request).
    recent.sort_by_key(|a| std::cmp::Reverse(a.request.at));
    recent.truncate(RECENT);

    AccountRollup { total_today, local_requests, remote_requests, busy_machines, recent }
}

/// Latest quota sync timestamp across all accounts.
pub fn last_synced_at(pool: &Pool) -> Option<i64> {
    pool.saved.accounts.iter().filter_map(|a| a.synced).max()
}

/// Latest check/heard timestamp across remote Rex hosts (for Machines footer).
pub fn last_checked_at(hosts: &[Stands]) -> Option<i64> {
    hosts.iter().filter_map(|h| h.heard_at.or(h.carried_at)).max()
}

/// Computes the optional status badge lights for `[Accounts, Activity, Machines]`.
/// Only genuine attention states get badges (no permanent green dot on Activity).
pub fn tab_lights(pool: &Pool, hosts: &[Stands], running: Option<&Running>, uses: Option<[bool; 3]>) -> [Option<Light>; 3] {
    let t = now();
    let any_signed_out = pool.saved.accounts.iter().any(|a| a.signed_out.is_some());
    let any_provider_exhausted = crate::PROVIDERS.iter().any(|&prov| {
        let enabled: Vec<&Account> = pool.saved.accounts.iter().filter(|a| a.provider == prov && a.enabled && a.signed_out.is_none()).collect();
        !enabled.is_empty() && enabled.iter().all(|a| account_out_of_quota(a, t).is_some())
    });
    let accounts_light = if pool.error.is_some() || any_signed_out {
        Some(Light::Down)
    } else if any_provider_exhausted {
        Some(Light::Waiting)
    } else {
        None
    };

    let daemon_bad = running.and_then(|r| r.daemon.as_ref()).is_some_and(|d| {
        let codex_on = uses.is_some_and(|u| u[crate::client_index(Provider::Codex)]);
        d.stale || (codex_on && d.connections.direct > 0 && d.connections.pool == 0)
    });
    let any_host_err = hosts.iter().any(|h| h.error.is_some());
    let any_host_pending = hosts.iter().any(|h| h.doing.is_some() || (h.wanted && !h.in_step));
    let machines_light = if pool.error.is_some() || daemon_bad || any_host_err {
        Some(Light::Down)
    } else if any_host_pending {
        Some(Light::Waiting)
    } else {
        None
    };

    [accounts_light, None, machines_light]
}

/// "just now", "2 minutes ago", "3 hours ago", else the day and time.
pub fn ago(at: i64) -> String {
    match now() - at {
        s if s < 60 => olo_kit::words::tr("just now").to_string(),
        s if s < 120 => olo_kit::words::tr("a minute ago").to_string(),
        s if s < 3600 => olo_kit::tr!("{} minutes ago", s / 60).to_string(),
        s if s < 7200 => olo_kit::words::tr("an hour ago").to_string(),
        s if s < 86_400 => olo_kit::tr!("{} hours ago", s / 3600).to_string(),
        _ => crate::clock(at),
    }
}

/// How a remote Rex host's pool stands, matching Rex's Manage Host status grammar.
pub fn host_stands(s: &Stands) -> (Option<Light>, String) {
    if let Some(doing) = &s.doing {
        return (None, format!("{}…", olo_kit::words::of(doing.clone())));
    }
    match (s.wanted, s.in_step, &s.error, s.behind_since) {
        (false, _, Some(_), _) => (Some(Light::Down), olo_kit::words::tr("Can’t install").into()),
        (false, _, None, _) => (Some(Light::Off), olo_kit::words::tr("Not installed").into()),
        (true, true, _, _) => (Some(Light::On), olo_kit::words::tr("Up to date").into()),
        (true, false, Some(_), _) => (Some(Light::Down), olo_kit::words::tr("Can’t sync").into()),
        (true, false, None, Some(since)) => (Some(Light::Waiting), olo_kit::tr!("Behind since {}", crate::clock(since))),
        (true, false, None, None) => (Some(Light::Waiting), olo_kit::words::tr("Behind").into()),
    }
}

/// Keeps provider sections in fixed canonical order (`Claude Code`, then `Codex`)
/// so fixing a broken account never swaps whole sections across the window;
/// problem accounts still float to the top *within* their own section (`sorted_accounts`).
pub fn sorted_providers(accounts: &[Account], _now_s: i64) -> Vec<Provider> {
    crate::PROVIDERS.into_iter().filter(|&p| accounts.iter().any(|a| a.provider == p)).collect()
}

/// Orders remote Rex hosts by severity first: errors -> installing/behind -> healthy -> not installed.
pub fn sorted_hosts(hosts: &[Stands]) -> Vec<Stands> {
    let rank = |s: &Stands| -> u8 {
        if s.doing.is_none() && s.error.is_some() {
            0
        } else if s.doing.is_some() || (s.wanted && !s.in_step) {
            1
        } else if s.wanted && s.in_step {
            2
        } else {
            3
        }
    };
    let mut out = hosts.to_vec();
    out.sort_by_key(rank);
    out
}

/// One concise summary line for a remote host's reported status:
/// accounts, kept account names, today's requests, running count, and staleness.
/// When `compact` is true, uses whole-segment fallbacks and never cuts inside a word.
pub fn host_summary(status: &Value, kept: &[String], heard_at: Option<i64>, compact: bool) -> String {
    let accounts = status["accounts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let today_reqs: u64 = accounts.iter().map(|a| a["today"]["requests"].as_u64().unwrap_or(0)).sum();
    let open = status["open"].as_array().map_or(0, Vec::len);
    let mut parts = Vec::new();
    if !kept.is_empty() {
        parts.push(if compact {
            match kept.len() {
                1 => format!("Keeps {}", kept[0]),
                n => format!("Keeps {n} accounts"),
            }
        } else {
            format!("Keeps {}", kept.join(", "))
        });
    } else {
        parts.push(match accounts.len() {
            1 => olo_kit::words::tr("1 account").to_string(),
            n => olo_kit::tr!("{} accounts", n).to_string(),
        });
    }
    if !compact || parts[0].len() < 24 {
        parts.push(match today_reqs {
            1 => olo_kit::words::tr("1 request today").to_string(),
            n => olo_kit::tr!("{} requests today", n).to_string(),
        });
    }
    if open > 0 && !compact {
        parts.push(match open {
            1 => olo_kit::words::tr("1 running").to_string(),
            n => olo_kit::tr!("{} running", n).to_string(),
        });
    }
    if !compact && let Some(at) = heard_at.filter(|at| now() - at > 120) {
        parts.push(olo_kit::tr!("as of {}", ago(at)).to_string());
    }
    parts.join(" · ")
}

/// Short human reason for a host error sentence ("No route to host").
pub fn host_brief_sentence(why: &str) -> String {
    let last = why.rsplit(": ").next().unwrap_or(why).trim().trim_end_matches('.');
    sentence(last)
}

/// "~/src/olo" for a folder under `$HOME`.
pub fn folder(p: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// Middle-truncates a path or string (`"/work…/olo"`) so directory tails stay visible.
pub fn middle_truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars || max_chars < 5 {
        return s.to_owned();
    }
    let keep_tail = (max_chars / 2).max(4);
    let keep_head = max_chars.saturating_sub(keep_tail + 1);
    let head: String = chars[..keep_head].iter().collect();
    let tail: String = chars[chars.len() - keep_tail..].iter().collect();
    format!("{head}…{tail}")
}

/// Capitalizes the first character of `s`.
pub fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Client colour for Activity graphs, live cells, swatches, and legends.
pub fn hue(client: &str) -> Hue {
    match client {
        "Claude Code" => Hue::Orange,
        "Codex" => Hue::Blue,
        "Antigravity" => Hue::Purple,
        _ => Hue::Gray,
    }
}

pub fn provider_hue(p: Provider) -> Hue {
    match p {
        Provider::Claude => Hue::Orange,
        Provider::Codex => Hue::Blue,
        Provider::Antigravity => Hue::Purple,
    }
}

// ---- Demo fixtures (`OLO_POOL_DEMO=1` / `OLO_POOL_DEMO_HOSTS=1`) ----------

pub fn demo_enabled() -> bool {
    std::env::var_os("OLO_POOL_DEMO").is_some() || std::env::var_os("OLO_POOL_DEMO_HOSTS").is_some()
}

pub struct DemoData {
    pub pool: Pool,
    pub activity: Activity,
    pub hosts: Vec<Stands>,
    pub running: Running,
    pub uses: [bool; 3],
    pub unaccepted: HashMap<String, Vec<String>>,
}

pub fn demo_data() -> DemoData {
    let mode = std::env::var("OLO_POOL_DEMO").unwrap_or_default();
    if mode == "empty" {
        let mut pool = Pool::default();
        pool.loaded = true;
        return DemoData { pool, activity: Activity::default(), hosts: Vec::new(), running: Running::default(), uses: [true, true, true], unaccepted: HashMap::new() };
    }

    let t = now();
    let t_ms = crate::now_ms();
    let d_today = today();
    let min_now = t.div_euclid(60);

    let req = |ago_s: i64, model: &str, client: &str, outcome: Outcome, status: u16, ms: u64, inp: u64, out: u64| Request {
        at: t - ago_s,
        model: model.into(),
        client: client.into(),
        outcome,
        status,
        ms,
        usage: Usage { requests: 1, input_tokens: inp, output_tokens: out },
    };

    // Account 1: Claude Max, healthy, sign-in kept on `build`, active now on `build`
    let mut claude_work = Account::new("acc-c1".into(), "Work Max".into(), Provider::Claude);
    claude_work.email = Some("alex@work.example".into());
    claude_work.plan = Some("max".into());
    claude_work.kept_on = Some("build".into());
    claude_work.last_used = Some(t - 18);
    claude_work.synced = Some(t - 40);
    claude_work.today = (d_today.clone(), Usage { requests: 14, input_tokens: 184_000, output_tokens: 9_200 });
    claude_work.windows = vec![
        QuotaWindow { name: "five_hour".into(), label: "5-hour".into(), used: 42.0, resets_at: Some(t + 7_800), models: vec![] },
        QuotaWindow { name: "seven_day".into(), label: "Weekly".into(), used: 68.0, resets_at: Some(t + 262_800), models: vec![] },
    ];
    claude_work.credits = vec![ResetCredit { id: "rc-1".into(), expires_at: Some(t + 86_400 * 6), remaining: 1, usable: true, ..Default::default() }];
    claude_work.recent = VecDeque::from(vec![
        req(900, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 4_120, 42_000, 2_100),
        req(520, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 3_680, 61_500, 3_400),
        req(240, "claude-opus-5-5", "Claude Code", Outcome::RateLimited, 429, 320, 0, 0),
        req(65, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 5_100, 80_500, 3_700),
    ]);

    // Account 2: Claude Pro, kept on This Computer, near limit (86% -> warning amber!)
    let mut claude_personal = Account::new("acc-c2".into(), "Personal".into(), Provider::Claude);
    claude_personal.email = Some("alex@home.example".into());
    claude_personal.plan = Some("pro".into());
    claude_personal.last_used = Some(t - 420);
    claude_personal.synced = Some(t - 45);
    claude_personal.today = (d_today.clone(), Usage { requests: 7, input_tokens: 68_400, output_tokens: 3_150 });
    claude_personal.windows = vec![
        QuotaWindow { name: "five_hour".into(), label: "5-hour".into(), used: 86.0, resets_at: Some(t + 3_120), models: vec![] },
        QuotaWindow { name: "seven_day".into(), label: "Weekly".into(), used: 31.0, resets_at: Some(t + 410_000), models: vec![] },
    ];
    claude_personal.recent = VecDeque::from(vec![
        req(1_200, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 2_900, 24_000, 1_100),
        req(420, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 3_400, 44_400, 2_050),
    ]);
    // (`OLO_POOL_DEMO=credits`: its 5-hour allowance out, its provider
    // answering it from usage credits: the last the pool turns to.)
    if mode == "credits" {
        claude_personal.windows[0].used = 100.0;
        claude_personal.now_paid("", false, t + 3_120, "five_hour");
    }

    // Account 3: Codex Team, sign-in kept on `build`
    let mut codex_team = Account::new("acc-x1".into(), "Team Plus".into(), Provider::Codex);
    codex_team.email = Some("dev@team.example".into());
    codex_team.plan = Some("team".into());
    codex_team.kept_on = Some("build".into());
    codex_team.last_used = Some(t - 95);
    codex_team.synced = Some(t - 30);
    codex_team.today = (d_today.clone(), Usage { requests: 11, input_tokens: 94_000, output_tokens: 5_400 });
    codex_team.windows = vec![
        QuotaWindow { name: "five_hour".into(), label: "5-hour".into(), used: 24.0, resets_at: Some(t + 12_600), models: vec![] },
        QuotaWindow { name: "seven_day".into(), label: "Weekly".into(), used: 52.0, resets_at: Some(t + 320_000), models: vec![] },
    ];
    codex_team.recent =
        VecDeque::from(vec![req(740, "gpt-5.4-codex", "Codex", Outcome::Completed, 200, 2_450, 32_000, 1_800), req(95, "gpt-5.4-codex", "Codex", Outcome::Completed, 200, 1_890, 62_000, 3_600)]);

    // Account 4: Codex Sandbox, signed out (sorts to top of Codex + triggers alert badge!)
    let mut codex_old = Account::new("acc-x2".into(), "Sandbox".into(), Provider::Codex);
    codex_old.email = Some("sandbox@team.example".into());
    codex_old.plan = Some("plus".into());
    codex_old.signed_out = Some("Refresh token was revoked by the provider.".into());
    codex_old.today = (d_today.clone(), Usage::default());

    // Account 5: Antigravity Ultra, sign-in kept on `build`, with banked quota resets
    let mut agy_ultra = Account::new("acc-g1".into(), "Google One AI".into(), Provider::Antigravity);
    agy_ultra.email = Some("alex@gmail.example".into());
    agy_ultra.plan = Some("AI Ultra".into());
    agy_ultra.kept_on = Some("build".into());
    agy_ultra.last_used = Some(t - 50);
    agy_ultra.synced = Some(t - 25);
    agy_ultra.today = (d_today.clone(), Usage { requests: 9, input_tokens: 112_000, output_tokens: 6_400 });
    agy_ultra.windows = vec![
        QuotaWindow { name: "bucket_5h".into(), label: "All models".into(), used: 54.0, resets_at: Some(t + 9_400), models: vec![] },
        QuotaWindow { name: "bucket_gemini_pro".into(), label: "Gemini Pro".into(), used: 84.0, resets_at: Some(t + 9_400), models: vec!["gemini-pro".into()] },
    ];
    agy_ultra.credits = vec![ResetCredit {
        id: "br-1".into(),
        expires_at: Some(t + 86_400 * 5),
        remaining: 2,
        usable: true,
        program: Some("banked_quota_reset".into()),
        label: Some("Banked Quota Reset".into()),
        kind: Some("banked_reset".into()),
        clears: vec!["bucket_5h".into(), "bucket_gemini_pro".into()],
        ..Default::default()
    }];
    agy_ultra.recent = VecDeque::from(vec![
        req(340, "gemini-4-neon-high", "Antigravity", Outcome::Completed, 200, 2_980, 48_000, 2_600),
        req(50, "gemini-4-neon-high", "Antigravity", Outcome::Completed, 200, 3_410, 64_000, 3_800),
    ]);

    // Account 6: Antigravity Pro, kept on This Computer, with a threshold-gated banked reset
    let mut agy_dev = Account::new("acc-g2".into(), "Workspace Dev".into(), Provider::Antigravity);
    agy_dev.email = Some("dev@work.example".into());
    agy_dev.plan = Some("Pro".into());
    agy_dev.last_used = Some(t - 610);
    agy_dev.synced = Some(t - 35);
    agy_dev.today = (d_today, Usage { requests: 6, input_tokens: 58_000, output_tokens: 3_100 });
    agy_dev.windows = vec![
        QuotaWindow { name: "bucket_5h".into(), label: "5-hour".into(), used: 19.0, resets_at: Some(t + 14_100), models: vec![] },
        QuotaWindow { name: "bucket_weekly".into(), label: "Weekly".into(), used: 37.0, resets_at: Some(t + 380_000), models: vec![] },
    ];
    agy_dev.credits = vec![ResetCredit {
        id: "br-2".into(),
        expires_at: Some(t + 86_400 * 10),
        remaining: 1,
        usable: false,
        needs_limit: true,
        unavailable_reason: Some("QUOTA_SUMMARY_BANKED_RESET_INELIGIBLE_REASON_USAGE_BELOW_THRESHOLD".into()),
        program: Some("banked_quota_reset".into()),
        label: Some("Banked Quota Reset".into()),
        kind: Some("banked_reset".into()),
        clears: vec!["bucket_5h".into()],
        ..Default::default()
    }];
    agy_dev.recent = VecDeque::from(vec![req(610, "gemini-3-flash", "Antigravity", Outcome::Completed, 200, 1_140, 28_000, 1_450)]);

    let mut pool = Pool::default();
    pool.loaded = true;
    pool.saved.accounts = vec![claude_work, claude_personal, codex_team, codex_old, agy_ultra, agy_dev];
    pool.in_flight.insert("acc-c2".into(), 1);
    pool.open = vec![Open {
        id: 101,
        account: "Personal".into(),
        account_id: "acc-c2".into(),
        provider: Provider::Claude,
        model: "claude-opus-5-5".into(),
        client: "Claude Code".into(),
        started: t_ms - 4_200,
    }];
    pool.served = VecDeque::from(vec![
        Served {
            account: "Work Max".into(),
            account_id: "acc-c1".into(),
            provider: Provider::Claude,
            request: req(520, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 3_680, 61_500, 3_400),
        },
        Served {
            account: "Personal".into(),
            account_id: "acc-c2".into(),
            provider: Provider::Claude,
            request: req(420, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 3_400, 44_400, 2_050),
        },
        Served {
            account: "Google One AI".into(),
            account_id: "acc-g1".into(),
            provider: Provider::Antigravity,
            request: req(340, "gemini-4-neon-high", "Antigravity", Outcome::Completed, 200, 2_980, 48_000, 2_600),
        },
        Served { account: "Team Plus".into(), account_id: "acc-x1".into(), provider: Provider::Codex, request: req(95, "gpt-5.4-codex", "Codex", Outcome::Completed, 200, 1_890, 62_000, 3_600) },
        Served {
            account: "Work Max".into(),
            account_id: "acc-c1".into(),
            provider: Provider::Claude,
            request: req(65, "claude-opus-5-5", "Claude Code", Outcome::Completed, 200, 5_100, 80_500, 3_700),
        },
        Served {
            account: "Google One AI".into(),
            account_id: "acc-g1".into(),
            provider: Provider::Antigravity,
            request: req(50, "gemini-4-neon-high", "Antigravity", Outcome::Completed, 200, 3_410, 64_000, 3_800),
        },
    ]);

    // Organic non-periodic 60-minute local + remote traffic (quiet morning stretch,
    // one sharp burst, long lull, trailing edits, current active burst) whose last-hour
    // totals stay strictly <= Today totals and whose single failure matches Codex 502!
    let local_c: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 2, 1, 1];
    let local_x: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0];
    let local_g: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1];
    let build_c: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 1, 4, 8, 11, 9, 4, 2, 1, 1, 0, 0, 0, 0, 0, 2, 3, 1, 0, 0, 0, 0, 0, 1, 5, 6, 3, 1, 0, 0, 1, 2, 4, 7, 9, 6, 4, 3];
    let build_x: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 1, 1, 2, 3, 2, 1, 1];
    let build_g: [u32; 61] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 4, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 2, 3, 2, 1, 0, 0, 0, 1, 1, 2, 3, 2, 1, 0];
    let mut activity = Activity::default();
    for (idx, offset) in (0..=60).rev().enumerate() {
        let m = min_now - offset;
        let c_reqs = local_c[idx];
        activity.minutes.push_back(Minute { minute: m, client: "Claude Code".into(), requests: c_reqs, failed: 0, input_tokens: c_reqs as u64 * 11_000, output_tokens: c_reqs as u64 * 540 });
        let x_reqs = local_x[idx];
        activity.minutes.push_back(Minute { minute: m, client: "Codex".into(), requests: x_reqs, failed: 0, input_tokens: x_reqs as u64 * 9_500, output_tokens: x_reqs as u64 * 520 });
        let g_reqs = local_g[idx];
        activity.minutes.push_back(Minute { minute: m, client: "Antigravity".into(), requests: g_reqs, failed: 0, input_tokens: g_reqs as u64 * 9_500, output_tokens: g_reqs as u64 * 510 });
    }

    let mut build_minutes = Vec::new();
    for (idx, offset) in (0..=60).rev().enumerate() {
        let m = min_now - offset;
        let c_reqs = build_c[idx];
        build_minutes.push(serde_json::json!({
            "minute": m, "client": "Claude Code", "requests": c_reqs,
            "failed": 0u32,
            "input_tokens": c_reqs as u64 * 8_800, "output_tokens": c_reqs as u64 * 440,
        }));
        let x_reqs = build_x[idx];
        build_minutes.push(serde_json::json!({
            "minute": m, "client": "Codex", "requests": x_reqs,
            "failed": u32::from(offset == 2),
            "input_tokens": x_reqs as u64 * 10_200, "output_tokens": x_reqs as u64 * 560,
        }));
        let g_reqs = build_g[idx];
        build_minutes.push(serde_json::json!({
            "minute": m, "client": "Antigravity", "requests": g_reqs,
            "failed": 0u32,
            "input_tokens": g_reqs as u64 * 9_200, "output_tokens": g_reqs as u64 * 480,
        }));
    }
    let j_req = |ago_s: i64, model: &str, client: &str, account: &str, account_id: &str, provider: &str, outcome: &str, status: u16, ms: u64, inp: u64, out: u64| {
        serde_json::json!({
            "at": t - ago_s, "model": model, "client": client, "account": account, "account_id": account_id,
            "provider": provider, "outcome": outcome, "status": status, "ms": ms,
            "usage": {"requests": 1, "input_tokens": inp, "output_tokens": out},
        })
    };
    let build_status = serde_json::json!({
        "accounts": [
            {
                "id": "acc-c1", "name": "Work Max", "in_flight": 1,
                "today": {"requests": 212, "input_tokens": 1_840_000, "output_tokens": 94_000},
                "recent": [
                    j_req(310, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 4800, 92_000, 4_100),
                    j_req(180, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 5200, 124_000, 5_300),
                    j_req(42, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 6100, 181_000, 6_450),
                ]
            },
            {
                "id": "acc-c2", "name": "Personal", "in_flight": 0,
                "today": {"requests": 19, "input_tokens": 210_000, "output_tokens": 11_200},
                "recent": [
                    j_req(210, "claude-opus-5-5", "Claude Code", "Personal", "acc-c2", "claude", "completed", 200, 3100, 48_200, 1_900),
                ]
            },
            {
                "id": "acc-x1", "name": "Team Plus", "in_flight": 1,
                "today": {"requests": 48, "input_tokens": 520_000, "output_tokens": 28_600},
                "recent": [
                    j_req(130, "gpt-5.4-codex", "Codex", "Team Plus", "acc-x1", "codex", "failed", 502, 1120, 0, 0),
                    j_req(28, "gpt-5.4-codex", "Codex", "Team Plus", "acc-x1", "codex", "completed", 200, 2750, 54_000, 2_800),
                ]
            },
            {
                "id": "acc-g1", "name": "Google One AI", "in_flight": 1,
                "today": {"requests": 64, "input_tokens": 740_000, "output_tokens": 38_200},
                "recent": [
                    j_req(155, "gemini-4-neon-high", "Antigravity", "Google One AI", "acc-g1", "agy", "completed", 200, 3120, 96_000, 4_900),
                    j_req(20, "gemini-3-pro", "Antigravity", "Google One AI", "acc-g1", "agy", "completed", 200, 2190, 72_000, 3_500),
                ]
            },
            {
                "id": "acc-g2", "name": "Workspace Dev", "in_flight": 0,
                "today": {"requests": 12, "input_tokens": 135_000, "output_tokens": 7_200},
                "recent": []
            }
        ],
        "open": [
            {"id": 501, "account": "Work Max", "account_id": "acc-c1", "provider": "claude", "model": "claude-opus-5-5", "client": "Claude Code", "started": t_ms - 8_700},
            {"id": 502, "account": "Team Plus", "account_id": "acc-x1", "provider": "codex", "model": "gpt-5.4-codex", "client": "Codex", "started": t_ms - 2_300},
            {"id": 503, "account": "Google One AI", "account_id": "acc-g1", "provider": "agy", "model": "gemini-4-neon-high", "client": "Antigravity", "started": t_ms - 1_400},
        ],
        "served": [
            j_req(310, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 4800, 92_000, 4_100),
            j_req(210, "claude-opus-5-5", "Claude Code", "Personal", "acc-c2", "claude", "completed", 200, 3100, 48_200, 1_900),
            j_req(180, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 5200, 124_000, 5_300),
            j_req(155, "gemini-4-neon-high", "Antigravity", "Google One AI", "acc-g1", "agy", "completed", 200, 3120, 96_000, 4_900),
            j_req(130, "gpt-5.4-codex", "Codex", "Team Plus", "acc-x1", "codex", "failed", 502, 1120, 0, 0),
            j_req(42, "claude-opus-5-5", "Claude Code", "Work Max", "acc-c1", "claude", "completed", 200, 6100, 181_000, 6_450),
            j_req(28, "gpt-5.4-codex", "Codex", "Team Plus", "acc-x1", "codex", "completed", 200, 2750, 54_000, 2_800),
            j_req(20, "gemini-3-pro", "Antigravity", "Google One AI", "acc-g1", "agy", "completed", 200, 2190, 72_000, 3_500),
        ],
        "activity": build_minutes,
    });

    let host = |id: &str, name: &str, address: &str| Stands { id: id.into(), name: name.into(), address: address.into(), ..Stands::default() };
    let hosts = vec![
        Stands { wanted: true, in_step: true, carried_at: Some(t - 150), status: Some(build_status), heard_at: Some(t - 12), ..host("build", "build", "alex@build.example") },
        Stands {
            wanted: true,
            behind_since: Some(t - 2400),
            carried_at: Some(t - 9000),
            error: Some("ssh error: connect to nas.example:22 failed: No route to host".into()),
            ..host("nas", "nas", "alex@nas.example")
        },
        Stands { doing: Some("Installing".into()), ..host("staging", "staging", "deploy@staging.example:2222") },
        Stands { error: Some("Olo has no Token Pool for FreeBSD amd64.".into()), hopeless: true, ..host("bsd", "bsd", "alex@bsd.example") },
        host("dev-box", "dev-box", "alex@dev-box.example"),
    ];

    // Keep local running count (1: Claude Code) + build (3) = 4 running,
    // matching Activity's 4 live requests while also showcasing Idle rows.
    let mut running = Running {
        claude: vec![Process { pid: 41208, started: t - 5_400, cwd: Some(PathBuf::from("/workspace/olo")), connections: Connections { pool: 2, direct: 0 } }],
        codex: vec![],
        agy: vec![],
        daemon: Some(Daemon { pid: 38910, started: t - 14_200, version: Some("0.168.0".into()), stale: false, connections: Connections { pool: 0, direct: 0 }, threads: Ok(vec![]) }),
    };

    if mode == "stopped" {
        pool.error = Some("Could not bind 127.0.0.1:17892 (address already in use).".into());
        pool.open.clear();
        running.daemon = None;
    }

    // Keep Accounts custody consistent with `build` being "Up to date" on Machines.
    let unaccepted = HashMap::new();

    DemoData { pool, activity, hosts, running, uses: [true, true, true], unaccepted }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_distinguishes_local_remote_and_pending_handover() {
        let hosts = vec![Stands { id: "h1".into(), name: "build".into(), ..Stands::default() }];
        let mut a = Account::new("a1".into(), "Work".into(), Provider::Claude);
        assert_eq!(custody(&a, &hosts, &[]), Custody::Here);

        a.kept_on = Some("h1".into());
        assert_eq!(custody(&a, &hosts, &[]), Custody::OnHost("build".into()));
        assert_eq!(custody(&a, &hosts, &["a1".into()]), Custody::PendingHandover("build".into()));
    }

    #[test]
    fn merged_activity_sums_overlapping_local_and_remote_minutes() {
        let mut local = Activity::default();
        local.minutes.push_back(Minute { minute: 100, client: "Claude Code".into(), requests: 2, failed: 0, input_tokens: 1000, output_tokens: 100 });
        local.minutes.push_back(Minute { minute: 101, client: "Codex".into(), requests: 1, failed: 0, input_tokens: 500, output_tokens: 50 });

        let host = Stands {
            id: "build".into(),
            name: "build".into(),
            wanted: true,
            status: Some(serde_json::json!({
                "activity": [
                    {"minute": 100, "client": "Claude Code", "requests": 5, "failed": 1, "input_tokens": 4000, "output_tokens": 400},
                    {"minute": 102, "client": "Claude Code", "requests": 3, "failed": 0, "input_tokens": 2000, "output_tokens": 200},
                ]
            })),
            ..Stands::default()
        };

        let all = merged_activity(&local, std::slice::from_ref(&host), &MachineScope::All);
        assert_eq!(all.minutes.len(), 3);
        let m100 = all.minutes.iter().find(|m| m.minute == 100 && m.client == "Claude Code").unwrap();
        assert_eq!((m100.requests, m100.failed, m100.input_tokens, m100.output_tokens), (7, 1, 5000, 500));

        let local_only = merged_activity(&local, std::slice::from_ref(&host), &MachineScope::Local);
        assert_eq!(local_only.minutes.len(), 2);

        let host_only = merged_activity(&local, &[host], &MachineScope::Host("build".into()));
        assert_eq!(host_only.minutes.len(), 2);
        assert_eq!(host_only.minutes[0].requests, 5);
    }

    #[test]
    fn account_rollup_and_live_recent_aggregate_across_machines() {
        let d = demo_data();
        let c1 = d.pool.account("acc-c1").unwrap();
        let roll = account_rollup(&d.pool, c1, &d.hosts);
        assert_eq!(roll.local_requests, 14);
        assert_eq!(roll.remote_requests, 212);
        assert_eq!(roll.total_today.requests, 226);
        assert_eq!(roll.busy_machines, vec!["build".to_owned()]);
        assert_eq!(roll.recent.len(), 7);
        assert!(roll.recent.windows(2).all(|w| w[0].request.at >= w[1].request.at));

        let live_all = live_requests(&d.pool, &d.hosts, &MachineScope::All);
        assert_eq!(live_all.len(), 4);
        assert!(live_all.windows(2).all(|w| w[0].started_ms >= w[1].started_ms));
        let live_local = live_requests(&d.pool, &d.hosts, &MachineScope::Local);
        assert_eq!(live_local.len(), 1);
        let live_build = live_requests(&d.pool, &d.hosts, &MachineScope::Host("build".into()));
        assert_eq!(live_build.len(), 3);

        let recent_all = recent_requests(&d.pool, &d.hosts, &MachineScope::All, 50);
        assert_eq!(recent_all.len(), 14);
        assert!(recent_all.windows(2).all(|w| w[0].request.at >= w[1].request.at));

        let g1 = d.pool.account("acc-g1").unwrap();
        let groll = account_rollup(&d.pool, g1, &d.hosts);
        assert_eq!((groll.local_requests, groll.remote_requests, groll.total_today.requests), (9, 64, 73));
        assert_eq!(groll.busy_machines, vec!["build".to_owned()]);
    }

    #[test]
    fn host_stands_summary_sorting_and_tab_lights() {
        let base = Stands { id: "h".into(), name: "build".into(), ..Stands::default() };
        assert_eq!(host_stands(&base), (Some(Light::Off), "Not installed".into()));
        assert_eq!(host_stands(&Stands { doing: Some("Installing".into()), ..base.clone() }), (None, "Installing…".into()));
        assert_eq!(host_stands(&Stands { error: Some("no room".into()), ..base.clone() }), (Some(Light::Down), "Can’t install".into()));
        assert_eq!(host_stands(&Stands { wanted: true, in_step: true, ..base.clone() }), (Some(Light::On), "Up to date".into()));
        assert_eq!(host_stands(&Stands { wanted: true, error: Some("no route".into()), ..base.clone() }), (Some(Light::Down), "Can’t sync".into()));
        assert_eq!(host_stands(&Stands { wanted: true, ..base }), (Some(Light::Waiting), "Behind".into()));

        let status = serde_json::json!({"accounts": [{"today": {"requests": 3}}, {"today": {"requests": 4}}], "open": [{}, {}]});
        assert_eq!(host_summary(&status, &["Work Max".into()], None, false), "Keeps Work Max · 7 requests today · 2 running");
        assert_eq!(host_brief_sentence("ssh error: connect to nas.example:22 failed: No route to host"), "No route to host");
        assert_eq!(middle_truncate("/workspace/very-long-directory/olo", 20), "/workspac…ectory/olo");

        let d = demo_data();
        // Signed-out Codex account sorts to top of Codex group; sections stay in canonical order (Claude Code -> Codex -> Antigravity)
        let codex = sorted_accounts(&d.pool.saved.accounts, Provider::Codex, now());
        assert_eq!(codex[0].name, "Sandbox");
        assert_eq!(sorted_providers(&d.pool.saved.accounts, now()), vec![Provider::Claude, Provider::Codex, Provider::Antigravity]);
        // Last-hour demo totals never exceed Today totals, and the 1 failure belongs to Codex
        let all_act = merged_activity(&d.activity, &d.hosts, &MachineScope::All);
        let c_tot = all_act.totals("Claude Code", now() - 3600);
        let x_tot = all_act.totals("Codex", now() - 3600);
        let g_tot = all_act.totals("Antigravity", now() - 3600);
        assert_eq!(c_tot.failed, 0);
        assert_eq!(x_tot.failed, 1);
        assert_eq!(g_tot.failed, 0);
        assert!(c_tot.requests <= 252 && c_tot.input_tokens <= 2_302_400);
        assert!(x_tot.requests <= 59 && x_tot.input_tokens <= 614_000);
        assert!(g_tot.requests <= 91 && g_tot.input_tokens <= 1_045_000);
        // Severity-sorted hosts put error hosts first, installing next, healthy after
        let sh = sorted_hosts(&d.hosts);
        assert_eq!(sh[0].id, "nas");
        assert_eq!(sh[1].id, "bsd");
        assert_eq!(sh[2].id, "staging");
        assert_eq!(sh[3].id, "build");
        // Activity tab has no permanent green dot; Accounts & Machines carry real Down badges
        assert_eq!(tab_lights(&d.pool, &d.hosts, Some(&d.running), Some(d.uses)), [Some(Light::Down), None, Some(Light::Down)]);
        assert_eq!(local_custody_summary(&d.pool, &d.hosts).as_deref(), Some("Keeps Personal, Sandbox, Workspace Dev · From build: Work Max, Team Plus, Google One AI"));
        assert!(last_checked_at(&d.hosts).is_some());
    }

    #[test]
    fn scoped_only_antigravity_out_of_quota_and_paid_surface_in_sort_and_tab_lights() {
        let t = now();
        let mut a = Account::new("g-scoped".into(), "Scoped AGY".into(), Provider::Antigravity);
        a.windows = vec![
            QuotaWindow { name: "b_pro".into(), label: "Gemini Pro".into(), used: 100.0, resets_at: Some(t + 1800), models: vec!["gemini-pro".into()] },
            QuotaWindow { name: "b_flash".into(), label: "Gemini Flash".into(), used: 100.0, resets_at: Some(t + 3600), models: vec!["gemini-flash".into()] },
        ];
        a.now_paid("gemini-pro", false, t + 1800, "b_pro");
        assert_eq!(account_out_of_quota(&a, t), Some(Some(t + 1800)));
        assert!(account_any_paid(&a, t));

        let mut healthy = Account::new("g-ok".into(), "Healthy AGY".into(), Provider::Antigravity);
        healthy.windows = vec![QuotaWindow { name: "b_pro".into(), label: "Gemini Pro".into(), used: 10.0, resets_at: Some(t + 1800), models: vec!["gemini-pro".into()] }];
        let accounts = vec![healthy.clone(), a.clone()];
        let ordered = sorted_accounts(&accounts, Provider::Antigravity, t);
        assert_eq!(ordered[0].id, "g-scoped");

        let mut pool = Pool::default();
        pool.saved.accounts = vec![a];
        let lights = tab_lights(&pool, &[], None, None);
        assert_eq!(lights[0], Some(Light::Waiting));

        // Overlapping 5h (100%) + 7d (30%) on the same scoped family still
        // counts that family as out of quota, and model-scoped paid credit
        // surfaces via `account_paid_scope` even while another family is healthy.
        let mut overlap = Account::new("g-overlap".into(), "Overlap AGY".into(), Provider::Antigravity);
        overlap.windows = vec![
            QuotaWindow { name: "pro_5h".into(), label: "Gemini Pro 5-hour".into(), used: 100.0, resets_at: Some(t + 1200), models: vec!["gemini-pro".into()] },
            QuotaWindow { name: "pro_7d".into(), label: "Gemini Pro Weekly".into(), used: 30.0, resets_at: Some(t + 86_400), models: vec!["gemini-pro".into()] },
        ];
        assert_eq!(account_out_of_quota(&overlap, t), Some(Some(t + 1200)));

        overlap.windows.push(QuotaWindow { name: "flash_5h".into(), label: "Gemini Flash".into(), used: 10.0, resets_at: Some(t + 1800), models: vec!["gemini-flash".into()] });
        assert_eq!(account_out_of_quota(&overlap, t), None, "Flash is still healthy");
        overlap.now_paid("gemini-pro", true, t + 1200, "pro_5h");
        assert_eq!(account_paid_scope(&overlap, t), Some((Some("Gemini Pro 5-hour".into()), t + 1200)));
    }
}
