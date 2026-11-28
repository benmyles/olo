//! What `olo agent` asks of the service: `start`, `list`, `status`,
//! `read`, `watch`, `send`, `approve`, `stop`, `prune`, `focus`. Each is
//! answered here, on the host, through the host's own Rex server.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use olo_rex_link::agents::minder;
use olo_rex_link::agents::wire::{Frame, Request, Response};
use olo_rex_link::agents::{
    AgentCli, ApproveSpec, DesktopPolicy, NativeSubagent, ScreenClassification, StartSpec, SubagentEvent, SubagentLiveState, WatchSpec, effective_state, new_subagent_id, now_unix_ms,
    parse_send_payload, sanitize_paste_text, summary_json, to_unix_ms,
};
use serde_json::{Value, json};

use super::service::{HOST, Service, tail_of};

/// Whether an agent that now stands as `now` says still stands as the
/// event `last` said of it: the same box instance or question (by its key),
/// still at rest, still failed; its end stands for good.
fn still_stands(last: &SubagentEvent, now: &ScreenClassification, live_box_key: Option<&str>) -> bool {
    match last.state {
        SubagentLiveState::WaitingApproval => {
            now.state == SubagentLiveState::WaitingApproval
                && if let Some(ev_box) = last.approval.as_ref().and_then(|a| a["box"].as_str()) {
                    live_box_key == Some(ev_box)
                } else if let Some(ev_box) = last.dedupe_key.strip_prefix("box:") {
                    live_box_key == Some(ev_box)
                } else {
                    super::service::subkey(now) == last.dedupe_key
                }
        }
        SubagentLiveState::WaitingQuestion => now.state == SubagentLiveState::WaitingQuestion && super::service::subkey(now) == last.dedupe_key,
        SubagentLiveState::Idle | SubagentLiveState::Done => matches!(now.state, SubagentLiveState::Idle | SubagentLiveState::Done),
        SubagentLiveState::Error => now.state == SubagentLiveState::Error,
        SubagentLiveState::Exited => true,
        _ => false,
    }
}

/// How long a stopped tab's own program has to end before `stop`
/// answers that it has not.
const STOPPED_WITHIN: Duration = Duration::from_secs(5);

/// Whether the process `pid` that began at `began` (clock ticks after
/// boot) still runs: there, the same one (its number given to another
/// since isn't it), and not merely waiting to be collected by its
/// parent (ended, to anyone but that parent).
fn runs(pid: i32, began: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let Some((_, rest)) = stat.rsplit_once(')') else { return false };
    let mut fields = rest.split_whitespace();
    let state = fields.next().unwrap_or("Z");
    !matches!(state, "Z" | "X") && fields.nth(18).and_then(|t| t.parse::<u64>().ok()) == Some(began)
}

/// Waits until none of `programs` (each a pid and when it began) runs
/// any more, `within` at most: which of them still does.
async fn still_running_after(programs: &[(i32, u64)], within: Duration) -> Vec<bool> {
    let until = tokio::time::Instant::now() + within;
    while programs.iter().any(|(pid, began)| runs(*pid, *began)) && tokio::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    programs.iter().map(|(pid, began)| runs(*pid, *began)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program is waited for until it has ended and no longer; one
    /// that hasn't is said to run still; one ended and not yet collected
    /// by its parent has ended.
    #[test]
    fn a_stopped_program_is_waited_for_and_no_longer() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let me = std::process::id() as i32;
            let began = crate::panes::start_ticks(me);
            assert!(runs(me, began) && !runs(me, began + 1), "this process, and not another that had its number");
            // One that goes on: said so when its time is up, not before and not long after.
            let at = std::time::Instant::now();
            assert_eq!(still_running_after(&[(me, began)], Duration::from_millis(200)).await, [true]);
            assert!(at.elapsed() >= Duration::from_millis(200) && at.elapsed() < Duration::from_millis(900), "{:?}", at.elapsed());
            // One that ends: answered when it has, well before the time given; though nobody has collected it.
            let mut child = std::process::Command::new("sleep").arg("0.3").spawn().unwrap();
            let pid = child.id() as i32;
            let began = crate::panes::start_ticks(pid);
            let at = std::time::Instant::now();
            assert_eq!(still_running_after(&[(pid, began), (me, crate::panes::start_ticks(me))], Duration::from_millis(1200)).await, [false, true]);
            assert!(std::path::Path::new(&format!("/proc/{pid}")).exists(), "ended and not collected: still listed, and not running");
            assert!(at.elapsed() >= Duration::from_millis(1200), "the other was waited for to its end: {:?}", at.elapsed());
            let at = std::time::Instant::now();
            assert_eq!(still_running_after(&[(pid, began)], Duration::from_secs(5)).await, [false]);
            assert!(at.elapsed() < Duration::from_millis(300), "{:?}", at.elapsed());
            let _ = child.wait();
            assert!(!runs(pid, began));
        });
    }
}

/// Who asks: the pane it was asked from, and the child that pane is, if
/// it is one (which may then act on its own descendants alone).
pub struct Caller {
    pub session: Option<String>,
    pub window: Option<String>,
    /// True only when `window` was positively resolved from `child` or from
    /// `block` in `sess.blocks` (never from the `sess.active` fallback).
    pub window_from_block: bool,
    pub block: Option<String>,
    pub child: Option<NativeSubagent>,
}

/// The arguments as the readers of them take them: as typed, with the
/// folder they were typed in; or the structured form.
fn raw_of(req: &Request) -> Value {
    match &req.obj {
        Some(obj) => {
            let mut obj = obj.clone();
            if obj.get("cwd").is_none() && !req.cwd.is_empty() && obj.is_object() {
                obj["cwd"] = json!(req.cwd);
            }
            obj
        }
        None => json!({ "args": req.args, "cwd": req.cwd }),
    }
}

fn args_of(raw: &Value) -> Option<Vec<String>> {
    raw.get("args").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect())
}

fn dialog_up(cls: &ScreenClassification) -> bool {
    cls.approval.is_some() || cls.folder_trust.is_some() || matches!(cls.state, SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion)
}

/// Post-paste settle delay before sending the separate `\r` submit: scales
/// gently with pasted byte length (80 ms base + 20 ms/KiB, capped at 350 ms)
/// so longer bracketed pastes finish rendering in TUI input boxes before Enter.
fn paste_settle_delay(body_len: usize) -> Duration {
    let extra_ms = ((body_len / 1024) as u64).saturating_mul(20);
    Duration::from_millis(80u64.saturating_add(extra_ms).min(350))
}

/// Extracts the active chat input field text (searching up to 120 lines above a
/// bottom ruled box `────\n❯|>\n...\n────`, or from the bottom active unruled
/// prompt line downward) as `(joined_collapsed_text, has_pasted_marker)`.
/// Returns `None` if no chat input field is visible/drawn on the screen.
fn extract_input_field(screen: &str) -> Option<(String, bool)> {
    let lines: Vec<&str> = screen.lines().collect();
    let is_rule = |l: &str| {
        let t = l.trim();
        t.len() >= 12 && t.starts_with("────") && t.ends_with("────")
    };
    fn strip_prompt_prefix(t: &str) -> &str {
        t.strip_prefix("│ ❯").or_else(|| t.strip_prefix("│ >")).or_else(|| t.strip_prefix('❯')).or_else(|| t.strip_prefix('›')).or_else(|| t.strip_prefix('>')).map(str::trim_start).unwrap_or(t)
    }
    let tail_start = lines.len().saturating_sub(34);
    // 1. Ruled input box near the bottom (Claude Code / Antigravity): search up
    //    to 120 lines above `bottom` so >29-line pasted drafts stay visible!
    if let Some(bottom_rel) = lines[tail_start..].iter().rposition(|l| is_rule(l)) {
        let bottom = tail_start + bottom_rel;
        let search_top = bottom.saturating_sub(120);
        if let Some(top_rel) = lines[search_top..bottom].iter().rposition(|l| is_rule(l)) {
            let top = search_top + top_rel;
            let inside = &lines[top + 1..bottom];
            let first_non_empty = inside.iter().map(|l| l.trim()).find(|l| !l.is_empty());
            if first_non_empty.is_some_and(|l| l.starts_with('❯') || l.starts_with('>') || l.starts_with('›') || l.starts_with("│ >") || l.starts_with("│ ❯")) {
                let mut parts = Vec::new();
                let mut pasted = false;
                let mut stripped_first = false;
                for raw in inside.iter().take(120) {
                    let t = raw.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let piece = if !stripped_first {
                        stripped_first = true;
                        strip_prompt_prefix(t)
                    } else {
                        t
                    };
                    if piece.contains("[Pasted text #") {
                        pasted = true;
                    }
                    if !piece.is_empty() {
                        parts.push(piece);
                    }
                }
                let joined = parts.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
                return Some((joined, pasted));
            }
        }
    }
    // 2. Unruled screen (Codex / stand-in): locate the lowest active prompt line
    //    within the bottom 80 lines and read it plus any wrapped continuations below it.
    let start = lines.len().saturating_sub(80);
    let mut prompt_at: Option<usize> = None;
    for (idx, raw) in lines.iter().enumerate().skip(start).rev() {
        let t = raw.trim();
        if t.starts_with('❯') || t.starts_with('›') || t.starts_with("│ >") || t.starts_with("│ ❯") || t == ">" || t.starts_with("> Accept-edits") {
            prompt_at = Some(idx);
            break;
        }
    }
    if let Some(idx) = prompt_at {
        let mut parts = Vec::new();
        let mut pasted = false;
        for (off, raw) in lines[idx..].iter().take(80).enumerate() {
            let t = raw.trim();
            if t.is_empty() {
                continue;
            }
            if off > 0
                && (is_rule(t)
                    || t.starts_with('?')
                    || t.starts_with("← ")
                    || t.contains("for shortcuts")
                    || t.contains("esc to interrupt")
                    || t.contains("esc to cancel")
                    || (t.contains(" · ") && (t.contains(" /") || t.contains(" ~") || t.contains(" left") || t.contains("context"))))
            {
                break;
            }
            let piece = if off == 0 { strip_prompt_prefix(t) } else { t };
            if piece.contains("[Pasted text #") {
                pasted = true;
            }
            if !piece.is_empty() {
                parts.push(piece);
            }
        }
        let joined = parts.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        return Some((joined, pasted));
    }
    // 3. Bare `> text` prompt line (without rules) as fallback for minimal stand-ins:
    if let Some(raw) = lines.iter().rev().take(28).find(|l| l.trim().starts_with('>')) {
        let piece = strip_prompt_prefix(raw.trim());
        let pasted = piece.contains("[Pasted text #");
        let joined = piece.split_whitespace().collect::<Vec<_>>().join(" ");
        return Some((joined, pasted));
    }
    None
}

/// Three-state (plus unrelated-draft) classification of the agent's active
/// input box relative to a submitted `body` (Neon #4 / Astra #15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldState {
    /// Input field is visibly drawn and positively contains `body`.
    VisibleHasDraft,
    /// Input field is visibly drawn and empty / CLI placeholder only.
    VisibleCleared,
    /// Input field is visibly drawn and holds different text (not `body`).
    VisibleDifferentDraft,
    /// Input field is absent/undrawn (inconclusive — never `VisibleCleared`!).
    Undrawn,
}

fn is_exact_cli_placeholder(field: &str, pasted: bool) -> bool {
    if pasted {
        return false;
    }
    if field.is_empty() {
        return true;
    }
    let low = field.to_ascii_lowercase();
    let is_agy_mode_hint = (low.starts_with("accept-edits mode:") || low.starts_with("plan mode:") || low.starts_with("auto mode:") || low.starts_with("default mode:")) && low.contains("shift+tab");
    low == "accept-edits mode"
        || is_agy_mode_hint
        || matches!(
            low.as_str(),
            "ask codex to do anything"
                | "ask anything"
                | "ask anything..."
                | "ask claude..."
                | "ask a follow-up..."
                | "type a message..."
                | "type your message..."
                | "tell claude what to do differently"
                | "tell codex what to do differently"
                | "tell antigravity what to do differently"
                | "what should claude do instead?"
                | "what should codex do instead?"
                | "what should antigravity cli do instead?"
                | "what to do differently"
                | "what to do differently?"
                | "reason:"
                | "feedback:"
        )
        || (low.starts_with("try \"") && low.ends_with('"'))
}

fn classify_input_field(screen: &str, body: &str) -> FieldState {
    let Some((field, pasted)) = extract_input_field(screen) else {
        return FieldState::Undrawn;
    };
    if is_exact_cli_placeholder(&field, pasted) {
        return FieldState::VisibleCleared;
    }
    let body_norm = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let first_line_norm = body.lines().map(str::trim).find(|s| !s.is_empty()).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    if body_norm.is_empty() {
        return FieldState::VisibleDifferentDraft;
    }
    // Claude collapses multi-line / long pastes into `[Pasted text #N +M lines]`:
    // match ONLY when `body` itself is multi-line or long (never treating an
    // unrelated collapsed paste as matching a short single-line message!).
    if pasted {
        let is_collapsible = body.lines().count() > 1 || body.len() >= 120;
        return if is_collapsible { FieldState::VisibleHasDraft } else { FieldState::VisibleDifferentDraft };
    }
    // Exact whole-field match (always valid, including short `<4`-char `"yes"`).
    if field == body_norm {
        return FieldState::VisibleHasDraft;
    }
    // Wrapped/multi-line draft whose first line starts the visible field
    // (require >= 12 chars so a short word like `"yes"` inside someone else's
    // `"yes please review the PR"` NEVER authorizes an automatic retry Enter!).
    if !first_line_norm.is_empty() && body_norm.chars().count() >= 12 {
        if field == first_line_norm || field.starts_with(&first_line_norm) {
            return FieldState::VisibleHasDraft;
        }
        if first_line_norm.chars().count() > 24 {
            let prefix: String = first_line_norm.chars().take(24).collect();
            if field.starts_with(&prefix) {
                return FieldState::VisibleHasDraft;
            }
        }
    }
    FieldState::VisibleDifferentDraft
}

/// Whether `body` is positively sitting unsubmitted in the agent's visible input box.
#[cfg(test)]
fn still_typed(screen: &str, body: &str) -> bool {
    classify_input_field(screen, body) == FieldState::VisibleHasDraft
}

/// True ONLY when the active input field is demonstrably empty or holds an
/// exact known CLI placeholder hint (never matching user drafts like
/// `"Ask the release owner first"` — Astra #6).
fn input_field_is_empty_or_placeholder(screen: &str) -> bool {
    extract_input_field(screen).is_some_and(|(field, pasted)| is_exact_cli_placeholder(&field, pasted))
}

/// Reserves one live-agent slot under a shared minder while a new agent
/// is launching or moving so concurrent bursts honor `minds_most` and
/// empty-minder timers cannot race with selection. Disarmed only once
/// the assignment is committed atomically under the same state lock; on
/// abnormal exit or cancellation, decrements `pending_minds` and re-arms
/// the empty-minder shutdown timer if the minder is now idle.
pub(super) struct PendingMindGuard {
    service: Arc<Service>,
    minder_id: String,
    active: bool,
}

impl PendingMindGuard {
    fn reserve_locked(service: Arc<Service>, st: &mut super::service::State, minder_id: String) -> Self {
        *st.pending_minds.entry(minder_id.clone()).or_insert(0) += 1;
        Service::bump_empty_gen_locked(st, &minder_id);
        Self { service, minder_id, active: true }
    }

    pub(super) fn disarm(&mut self) {
        self.active = false;
    }

    #[cfg(test)]
    pub(super) fn reserve_for_test(service: Arc<Service>, minder_id: &str) -> Self {
        let mut st = service.lock();
        Self::reserve_locked(service.clone(), &mut st, minder_id.to_owned())
    }
}

impl Drop for PendingMindGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let now_empty = {
            let mut st = self.service.lock();
            if let Some(n) = st.pending_minds.get_mut(&self.minder_id) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.pending_minds.remove(&self.minder_id);
                }
            }
            st.reg.minded_by(&self.minder_id).is_empty() && st.pending_minds.get(&self.minder_id).copied().unwrap_or(0) == 0
        };
        if now_empty {
            self.service.arm_empty_minder_if_idle(&self.minder_id, super::service::minder_reports());
        }
    }
}

/// Reserves a freshly generated child ID while `start` writes rules and
/// creates the Rex tab so concurrent starts can never collide.
struct ReservedIdGuard {
    service: Arc<Service>,
    id: String,
}

impl ReservedIdGuard {
    fn allocate(service: Arc<Service>) -> Self {
        let id = {
            let mut st = service.lock();
            loop {
                let candidate = new_subagent_id();
                if !st.reserved_ids.contains(&candidate) && !st.reg.entries.iter().any(|e| e.id == candidate) {
                    st.reserved_ids.insert(candidate.clone());
                    break candidate;
                }
            }
        };
        Self { service, id }
    }
}

impl Drop for ReservedIdGuard {
    fn drop(&mut self) {
        self.service.lock().reserved_ids.remove(&self.id);
    }
}

/// Reserves a live child name under `(session_id, asker_win)` while a
/// `start` or `start_shared_minder` is in flight so concurrent starts
/// and auto-named minders cannot collide on `--name`.
struct PendingNameGuard {
    service: Arc<Service>,
    key: (String, String, String),
}

impl PendingNameGuard {
    fn try_reserve(service: Arc<Service>, sid: &str, asker_win: &str, name: &str) -> Result<Self, String> {
        let key = (sid.to_owned(), asker_win.to_owned(), name.trim().to_ascii_lowercase());
        {
            let mut st = service.lock();
            let live = st.sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default();
            if Service::is_live_name_taken_for_lead_locked(&st, sid, asker_win, name, &live) {
                return Err(format!("a live child named `{name}` already exists under this parent (choose a unique `--name` or stop it first)"));
            }
            st.pending_names.insert(key.clone());
        }
        Ok(Self { service, key })
    }

    fn insert_locked(service: Arc<Service>, st: &mut super::service::State, sid: &str, asker_win: &str, name: &str) -> Self {
        let key = (sid.to_owned(), asker_win.to_owned(), name.trim().to_ascii_lowercase());
        st.pending_names.insert(key.clone());
        Self { service, key }
    }
}

impl Drop for PendingNameGuard {
    fn drop(&mut self) {
        self.service.lock().pending_names.remove(&self.key);
    }
}

/// Scopes `let_go` and `closing_minders` marks strictly to an active
/// non-`keep_tab` `stop_target_list` call, clearing them automatically on
/// return or cancellation so a kept tab or failed close never leaves a
/// stale suppression mark behind.
struct StopScopeGuard {
    service: Arc<Service>,
    let_go_ids: Vec<String>,
    closing_ids: Vec<String>,
}

impl StopScopeGuard {
    fn enter(service: Arc<Service>, targets: &[NativeSubagent], keep_tab: bool, quiet_minders: bool) -> Self {
        if keep_tab {
            return Self { service, let_go_ids: Vec::new(), closing_ids: Vec::new() };
        }
        let mut let_go_ids = Vec::new();
        let mut closing_ids = Vec::new();
        let stopping_workers: HashSet<String> = targets.iter().filter(|t| !t.is_minder() && t.exited_at.is_none()).map(|t| t.id.clone()).collect();
        {
            let mut st = service.lock();
            for t in targets.iter().filter(|t| t.is_minder() && t.exited_at.is_none()) {
                if t.is_shared_minder() && st.closing_minders.insert(t.id.clone()) {
                    closing_ids.push(t.id.clone());
                }
                if quiet_minders {
                    let scope = if stopping_workers.is_empty() { None } else { Some(stopping_workers.clone()) };
                    st.let_go.insert(t.id.clone(), scope);
                    let_go_ids.push(t.id.clone());
                }
            }
        }
        Self { service, let_go_ids, closing_ids }
    }
}

impl Drop for StopScopeGuard {
    fn drop(&mut self) {
        if self.let_go_ids.is_empty() && self.closing_ids.is_empty() {
            return;
        }
        let mut st = self.service.lock();
        for id in &self.let_go_ids {
            st.let_go.remove(id);
        }
        for id in &self.closing_ids {
            st.closing_minders.remove(id);
        }
    }
}

fn format_stop_summary(stopped: &[String], failed: &[Value], programs: &[Value], keep_tab: bool) -> (bool, String) {
    let left: Vec<String> = programs.iter().filter(|p| p["ended"] == false).map(|p| format!("{} (pid {})", p["name"].as_str().or(p["id"].as_str()).unwrap_or_default(), p["pid"])).collect();
    let fail_names: Vec<String> = failed
        .iter()
        .map(|f| {
            let who = f["name"].as_str().or(f["id"].as_str()).unwrap_or_default();
            let err = f["error"].as_str().unwrap_or("close failed");
            format!("{who} ({err})")
        })
        .collect();
    let mut said = match (left.is_empty(), keep_tab) {
        (true, _) => format!("Stopped {} subagent tab(s).\n", stopped.len()),
        (false, true) => format!("Interrupted {} subagent tab(s), kept open: still running: {}.\n", stopped.len(), left.join(", ")),
        (false, false) => format!("Stopped {} subagent tab(s), but not ended after {} s: {}.\n", stopped.len(), STOPPED_WITHIN.as_secs(), left.join(", ")),
    };
    if !fail_names.is_empty() {
        said.push_str(&format!("Failed to close {} tab(s): {}.\n", fail_names.len(), fail_names.join(", ")));
    }
    (left.is_empty(), said)
}

/// The words of a failure: what the conversation's file or the strict
/// reading of the screen gave, or, where an agent says it failed and
/// those have nothing of why (none; or only "API Error", what Claude
/// Code's file says of an error with no message), its screen's own error
/// line of this turn (`olo_agent_screen::error_line`).
pub(super) fn error_words(state: SubagentLiveState, error: Option<String>, screen: &str) -> Option<String> {
    if state != SubagentLiveState::Error {
        return error;
    }
    let bare = |e: &String| e.trim().eq_ignore_ascii_case("api error");
    match error {
        Some(words) if !bare(&words) => Some(words),
        other => olo_agent_screen::error_line(screen).or(other),
    }
}

impl Service {
    /// Enforces exited-caller and minder operation gates before any
    /// dispatch (`follow`, `adopt`, `retire`, `ping`, `policy`, or `agent_op`).
    pub(super) fn gate_caller(&self, req: &Request, caller: &Caller) -> Option<Response> {
        let c = caller.child.as_ref()?;
        if c.exited_at.is_some() {
            return Some(Response::err(&req.op, "this child tab has exited and may no longer act on the agents service", 69));
        }
        if req.op == "adopt" {
            return Some(Response::err(&req.op, "a child subagent tab may not adopt records into the agents service", 69));
        }
        if c.is_shared_minder() {
            if !matches!(req.op.as_str(), "list" | "ls" | "status" | "read" | "tail" | "watch" | "wait" | "send" | "input" | "approve" | "escalate") {
                return Some(Response::err(&req.op, "a minder lists, watches, reads, answers and escalates for the agents it minds: this isn't its to ask", 69));
            }
        } else if c.minds.is_some() && !matches!(req.op.as_str(), "status" | "read" | "tail" | "watch" | "wait" | "send" | "input" | "approve" | "escalate") {
            return Some(Response::err(&req.op, "a minder watches, reads, answers and escalates for the one agent it minds: this isn't its to ask", 69));
        }
        None
    }

    /// True if `me` is still live and still owns `agent_id` (shared `minded_by`
    /// or legacy 1:1 `minds`). Checked under the state lock before returning
    /// any watched payload, setting `told`, or confirming input/approval.
    pub(super) fn minder_owns(st: &super::service::State, me: &NativeSubagent, agent_id: &str) -> bool {
        let minder_live = st.reg.entries.iter().rev().any(|e| e.id == me.id && e.exited_at.is_none());
        if !minder_live {
            return false;
        }
        if me.is_shared_minder() {
            st.reg.entries.iter().rev().find(|e| e.id == agent_id).is_some_and(|e| e.minded_by.as_deref() == Some(me.id.as_str()))
        } else {
            me.minds.as_deref() == Some(agent_id) && st.reg.minder_of(agent_id).is_some_and(|m| m.id == me.id)
        }
    }

    pub async fn agent_op(self: &Arc<Self>, req: &Request, caller: &Caller) -> Response {
        let raw = raw_of(req);
        if let Some(refused) = self.gate_caller(req, caller) {
            return refused;
        }
        match req.op.as_str() {
            "start" | "spawn" => self.start(req, &raw, caller).await,
            "resume" => self.resume(req, &raw, caller).await,
            "list" | "ls" => self.list(&raw, caller).await,
            "status" => self.status(&raw, caller).await,
            "read" | "tail" => self.read(&raw, caller).await,
            "watch" | "wait" => self.watch(&raw, caller).await,
            "send" | "input" => self.send(req, &raw, caller).await,
            "approve" => self.approve(&raw, caller).await,
            "stop" | "close" | "kill" => self.stop(req, &raw, caller).await,
            "prune" => self.prune(&raw, caller),
            "focus" => self.focus(&raw, caller).await,
            "mind" | "minder" => self.mind(req, &raw, caller).await,
            "escalate" => self.escalate(&raw, caller).await,
            other => Response {
                ok: false,
                stdout: String::new(),
                stderr: format!("olo agent: unknown command `{other}`\nusage: olo agent <start|run|resume|list|status|read|watch|send|approve|mind|escalate|stop|prune|focus> [ARGS…]\n"),
                code: 2,
                data: Value::Null,
            },
        }
    }

    /// Authenticate the querying peer strictly by its OS process ancestry
    /// (`peer_pid`), never by `req.block`: used for `policy` queries where
    /// `req.block` names the subject pane being inspected, not the caller.
    pub(super) fn caller_by_peer(&self, peer_pid: Option<i32>) -> Caller {
        let by_process = peer_pid.and_then(|pid| {
            let mut chain = Vec::new();
            let mut at = pid;
            while at > 1 && chain.len() < 64 && !chain.contains(&at) {
                chain.push(at);
                at = crate::procs::proc_of(at).map_or(0, |p| p.ppid);
            }
            let st = self.lock();
            chain.iter().find_map(|pid| st.reg.entries.iter().rev().find(|e| e.exited_at.is_none() && st.live.get(&e.id).is_some_and(|l| l.pid == Some(*pid))).cloned())
        });
        if let Some(child) = by_process {
            return Caller { session: Some(child.session_id.clone()), window: Some(child.window_id.clone()), window_from_block: true, block: Some(child.block_id.clone()), child: Some(child) };
        }
        Caller { session: None, window: None, window_from_block: false, block: None, child: None }
    }

    /// Who asks. A process inside a child's tab is that child, whatever
    /// pane it says it is in (its ancestors are looked at: the
    /// terminal's own process is among them); otherwise the pane it
    /// names.
    pub(super) async fn caller(&self, req: &Request, peer_pid: Option<i32>) -> Caller {
        let claimed = Some(req.block.trim()).filter(|b| !b.is_empty());
        let by_process = peer_pid.and_then(|pid| {
            let mut chain = Vec::new();
            let mut at = pid;
            while at > 1 && chain.len() < 64 && !chain.contains(&at) {
                chain.push(at);
                at = crate::procs::proc_of(at).map_or(0, |p| p.ppid);
            }
            let st = self.lock();
            chain.iter().find_map(|pid| st.reg.entries.iter().rev().find(|e| e.exited_at.is_none() && st.live.get(&e.id).is_some_and(|l| l.pid == Some(*pid))).cloned())
        });
        let child = by_process.or_else(|| claimed.and_then(|b| self.lock().reg.by_block_any(HOST, Some(req.session.as_str()).filter(|s| !s.is_empty()), b).cloned()));
        if let Some(child) = child {
            return Caller { session: Some(child.session_id.clone()), window: Some(child.window_id.clone()), window_from_block: true, block: Some(child.block_id.clone()), child: Some(child) };
        }
        let mut session = Some(req.session.trim()).filter(|s| !s.is_empty()).map(str::to_owned);
        if session.is_none()
            && let Some(block) = claimed
        {
            session = self.session_of_block(block).await;
        }
        if let Some(sid) = &session {
            let _ = self.seen(sid).await;
        }
        let (window, window_from_block) = session.as_deref().map_or((None, false), |sid| {
            let st = self.lock();
            let Some(sess) = st.sessions.get(sid) else { return (None, false) };
            match claimed.and_then(|b| sess.blocks.get(b).cloned()) {
                Some(w) => (Some(w), true),
                None => (sess.active.clone(), false),
            }
        });
        Caller { session, window, window_from_block, block: claimed.map(str::to_owned), child: None }
    }

    /// The session asked from and the tab asked about (`--parent`, else
    /// the asker's own).
    async fn place(&self, caller: &Caller, explicit_parent: Option<&str>) -> Result<(String, String), String> {
        // Asked from a shell that isn't in Rex (a cron job, a plain SSH
        // shell: no `REX_BLOCK`), the child goes under the tab that is
        // named, in whichever session that tab is; with none named it
        // goes nowhere. (It went under the tab in front in the first
        // session the server listed: whoever's that was. The ShiftUp
        // lead's item 45, 2026-11-27: "a child started from a shell that
        // is not in Rex … under a parent tab named by its block".)
        let sid = match (&caller.session, explicit_parent.filter(|s| !s.is_empty())) {
            (Some(sid), _) => sid.clone(),
            (None, Some(named)) => self
                .session_of_tab(named)
                .await
                .ok_or_else(|| format!("parent tab `{named}` isn't in any Rex session on this host: name a pane's block (its `REX_BLOCK`), a tab's window id, or a child's name or id"))?,
            (None, None) => {
                return Err(
                    "this isn't asked from a Rex pane (no `REX_BLOCK`), so say which tab the child goes under: `--parent BLOCK` (a pane's `REX_BLOCK`), a tab's window id, or a child's name or id"
                        .to_owned(),
                );
            }
        };
        self.seen(&sid).await.map_err(|e| format!("Rex session `{sid}` couldn't be read: {e}"))?;
        let st = self.lock();
        let sess = st.sessions.get(&sid).ok_or_else(|| format!("Rex session `{sid}` is not there"))?;
        let parent = if let Some(p) = explicit_parent.filter(|s| !s.is_empty()) {
            let resolved = if sess.windows.contains(p) {
                p.to_owned()
            } else if let Some(w) = sess.blocks.get(p) {
                w.clone()
            } else if let Some(sub) = st.reg.resolve_target(HOST, Some(&sid), p) {
                sub.window_id
            } else {
                return Err(format!("parent tab `{p}` not found in session `{sid}`"));
            };
            // A child may name as parent only itself or one of its own descendants.
            if let Some(cc) = &caller.child
                && resolved != cc.window_id
                && !st.reg.is_strict_descendant(HOST, &sid, &cc.window_id, &resolved)
            {
                return Err(format!("`--parent {p}` refused: child subagent `{}` may only attach under itself or its own descendants", cc.id));
            }
            resolved
        } else if let Some(cc) = &caller.child {
            cc.window_id.clone()
        } else if let Some(w) = caller.window.clone().filter(|w| sess.windows.contains(w)) {
            w
        } else {
            sess.active.clone().or_else(|| sess.windows.iter().next().cloned()).ok_or_else(|| "session has no open tabs".to_owned())?
        };
        Ok((sid, parent))
    }

    /// The session a tab is in, named by one of its panes' blocks, by its
    /// own window id, or by a child's name or id: looked for in every
    /// session of this host.
    async fn session_of_tab(&self, named: &str) -> Option<String> {
        if let Some(sid) = self.session_of_block(named).await {
            return Some(sid);
        }
        for sid in self.session_ids().await.ok()? {
            if self.seen(&sid).await.is_ok() && self.lock().sessions.get(&sid).is_some_and(|s| s.windows.contains(named)) {
                return Some(sid);
            }
        }
        self.lock().reg.resolve_target(HOST, None, named).map(|sub| sub.session_id)
    }

    fn live_windows(&self, sid: &str) -> HashSet<String> {
        self.lock().sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default()
    }

    fn resolve(&self, caller: &Caller, target: &str) -> Result<NativeSubagent, String> {
        // An older 1:1 minder names one agent: the one it currently minds,
        // verified against live registry ownership so a revoked legacy tab
        // loses access the instant migration commits.
        if let Some(me) = caller.child.as_ref().filter(|c| c.minds.is_some()) {
            let minded = me.minds.as_deref().unwrap();
            let it = {
                let st = self.lock();
                if !Self::minder_owns(&st, me, minded) {
                    return Err("this minder no longer minds that agent".to_owned());
                }
                st.reg.entries.iter().rev().find(|e| e.id == minded).cloned()
            }
            .ok_or_else(|| "the agent this minder minds is no longer known".to_owned())?;
            let t = target.trim();
            if t == it.id || t == it.window_id || it.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(t)) {
                return Ok(it);
            }
            return Err(format!("a minder reaches the agent it minds (`{}`) and no other: not `{t}`", it.name.as_deref().unwrap_or(&it.id)));
        }
        // A shared minder reaches only the agents it minds (live or
        // exited), never siblings, ancestors, or unminded grandchildren.
        if let Some(me) = caller.child.as_ref().filter(|c| c.is_shared_minder()) {
            let t = target.trim();
            if t.is_empty() {
                return Err("empty subagent target".into());
            }
            let st = self.lock();
            let own: Vec<&NativeSubagent> = st.reg.all_minded_by(&me.id);
            // Exact immutable ID / window / block match first (so an exited
            // agent's ID is never shadowed by a newer live agent reusing its name).
            if let Some(hit) = own.iter().rev().find(|e| e.id == t || e.window_id == t || e.block_id == t) {
                return Ok((*hit).clone());
            }
            for live_only in [true, false] {
                let pool: Vec<&NativeSubagent> = own.iter().copied().filter(|e| !live_only || e.exited_at.is_none()).collect();
                let by_name: Vec<&NativeSubagent> = pool.iter().copied().filter(|e| e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(t))).collect();
                if by_name.len() == 1 {
                    return Ok(by_name[0].clone());
                } else if by_name.len() > 1 {
                    let list: Vec<String> = by_name.iter().map(|e| format!("{} ({})", e.id, e.display_title())).collect();
                    return Err(format!("ambiguous name `{t}` among minded agents: {} (use exact ID)", list.join(", ")));
                }
                let by_title: Vec<&NativeSubagent> = pool.iter().copied().filter(|e| e.title.eq_ignore_ascii_case(t)).collect();
                if by_title.len() == 1 {
                    return Ok(by_title[0].clone());
                }
                let by_pref: Vec<&NativeSubagent> = pool.iter().copied().filter(|e| e.id.starts_with(t) || e.window_id.starts_with(t)).collect();
                if by_pref.len() == 1 {
                    return Ok(by_pref[0].clone());
                }
            }
            return Err(format!("a minder reaches only the agents it minds: not `{t}`"));
        }
        self.lock().reg.resolve_target_scoped(HOST, caller.session.as_deref(), caller.window.as_deref(), caller.child.is_some(), target)
    }

    /// Whether what `ev` says a child asks (leave, a question) is for
    /// another to answer than `caller`: the child has a minder that
    /// runs, and the caller isn't it. `reg`: the registry, under the
    /// lock the log is read under.
    fn minders_own(reg: &olo_rex_link::agents::SubagentRegistry, caller: &Caller, id: &str, state: SubagentLiveState) -> bool {
        matches!(state, SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion)
            && reg.minder_of(id).is_some_and(|m| caller.child.as_ref().map(|c| c.id.as_str()) != Some(m.id.as_str()))
    }

    /// What a minder did for an agent, kept beside that agent's rules:
    /// a line each, for `olo agent mind NAME` to say.
    fn minder_did(&self, caller: &Caller, about_id: &str, what: &str, said: Value) {
        let Some(me) = caller.child.as_ref().filter(|c| c.is_minder()) else { return };
        use std::io::Write as _;
        // (`by`: which minder, where one was put in another's place.)
        let line = json!({ "at_ms": now_unix_ms(), "did": what, "said": said, "by": me.id }).to_string();
        let dir = self.paths.dir.join("minders").join(about_id);
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("answers.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    fn entry(&self, id: &str) -> Option<NativeSubagent> {
        self.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned()
    }

    fn cursor(&self) -> u64 {
        self.lock().log.latest_seq()
    }

    // ---- start ----

    fn agent_what(child: &NativeSubagent) -> String {
        // (And where it works: a minder is shown the command an agent
        // asks leave for and nothing of where it would run; rules that
        // speak of a folder are read against this.)
        match (child.cli.is_agent(), child.cwd.as_deref().filter(|c| !c.is_empty())) {
            (true, Some(cwd)) => format!("{}, {}, working in {cwd}", child.cli.display_name(), child.model),
            (true, None) => format!("{}, {}", child.cli.display_name(), child.model),
            (false, Some(cwd)) => format!("a command, run in {cwd}"),
            (false, None) => "a command".to_owned(),
        }
    }

    fn rules_path(&self, agent_id: &str) -> PathBuf {
        self.paths.dir.join("minders").join(agent_id).join("rules.md")
    }

    fn write_agent_rules(&self, agent_id: &str, rules: &str) -> Result<PathBuf, String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = self.rules_path(agent_id);
        let dir = path.parent().ok_or_else(|| format!("invalid rules path: {}", path.display()))?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("rules.md.tmp.{}.{seq}", std::process::id()));
        let written = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(rules.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {e}", path.display()));
        }
        Ok(path)
    }

    /// Tells a shared minder on its own watch that an agent was added,
    /// removed, or had its rules changed. Recorded beside the agent's
    /// state so `last_state_of` is untouched, and invisible to every
    /// non-minder watch.
    fn emit_minding_event(&self, minder: &NativeSubagent, agent: &NativeSubagent, action: &str, rules_path: &Path) -> u64 {
        let (state, rules_gen) = {
            let st = self.lock();
            let s = if agent.exited_at.is_some() { SubagentLiveState::Exited } else { st.live.get(&agent.id).and_then(|l| l.last_state).unwrap_or(SubagentLiveState::Working) };
            let g = st.rules_gen.get(&agent.id).copied().unwrap_or(0);
            (s, g)
        };
        let its_name = agent.name.as_deref().unwrap_or(&agent.id);
        let what = Self::agent_what(agent);
        let summary = match action {
            "added" => format!("Now minding \"{its_name}\" ({what}). Read its rules at {}", rules_path.display()),
            "rules" => format!("Rules changed for \"{its_name}\". Re-read its rules at {}", rules_path.display()),
            "removed" => format!("No longer minding \"{its_name}\"."),
            other => format!("{other}: \"{its_name}\""),
        };
        let mut event = self.event_of(agent, state, Some(summary), String::new());
        event.event = minder::MINDING.to_owned();
        event.parent_window_id = minder.window_id.clone();
        event.minding = Some(json!({
            "action": action,
            "minder_id": minder.id,
            "rules": rules_path.display().to_string(),
            "rules_gen": rules_gen,
            "what": what,
            "cwd": agent.cwd,
        }));
        self.record_beside(event)
    }

    /// True if `name` is already taken by a live or in-flight child
    /// directly under `asker_win` or directly under any live shared
    /// minder of `asker_win` (so a lead's minded, unminded, and minder
    /// children never collide on `--name`).
    fn is_live_name_taken_for_lead_locked(st: &super::service::State, sid: &str, asker_win: &str, name: &str, live: &HashSet<String>) -> bool {
        let pending_key = (sid.to_owned(), asker_win.to_owned(), name.trim().to_ascii_lowercase());
        if st.pending_names.contains(&pending_key) || st.reg.is_live_name_taken(HOST, sid, asker_win, name, live) {
            return true;
        }
        st.reg
            .entries
            .iter()
            .filter(|e| e.host_key == HOST && e.session_id == sid && e.parent_window_id == asker_win && e.exited_at.is_none() && e.is_shared_minder() && live.contains(&e.window_id))
            .any(|m| st.reg.is_live_name_taken(HOST, sid, &m.window_id, name, live))
    }

    /// Atomically releases a fresh minder's pending slot and, if still
    /// live and empty, marks it in `closing_minders` before stopping its
    /// tab so no concurrent start or move can select it while it closes.
    async fn cleanup_fresh_empty_minder(self: &Arc<Self>, minder: &NativeSubagent, guard: &mut PendingMindGuard) {
        let should_stop = {
            let mut st = self.lock();
            if guard.active {
                if let Some(n) = st.pending_minds.get_mut(&minder.id) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        st.pending_minds.remove(&minder.id);
                    }
                }
                guard.disarm();
            }
            let live = st.reg.entries.iter().any(|e| e.id == minder.id && e.exited_at.is_none());
            let empty = st.reg.minded_by(&minder.id).is_empty() && st.pending_minds.get(&minder.id).copied().unwrap_or(0) == 0;
            if live && empty && !st.closing_minders.contains(&minder.id) {
                st.closing_minders.insert(minder.id.clone());
                true
            } else {
                false
            }
        };
        if should_stop {
            let _ = self.stop_tabs_inner(minder, true, false, false, true).await;
            self.lock().closing_minders.remove(&minder.id);
        }
    }

    async fn start(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let spec = match StartSpec::parse(Some(raw)) {
            Ok(s) => s,
            Err(e) => return Response::err("start", e, 2),
        };
        // (Its minder's rules read before anything is started: a file
        // that isn't there is said now, not after the child runs.)
        let minder_req = match (&spec.minder, spec.rules.is_some() || spec.rules_file.is_some()) {
            (None, true) => return Response::err("start", "`--rules` are a minder's: say which with `--minder claude:MODEL:EFFORT` or `--minder NAME`", 2),
            (None, false) => None,
            (Some(choice), _) => match self.rules_given(req, spec.rules.as_deref(), spec.rules_file.as_deref()) {
                Ok(rules) => Some((choice.clone(), rules.unwrap_or_default())),
                Err(e) => return Response::err("start", e, 2),
            },
        };
        // (Its list of what is not allowed without asking: one that
        // isn't a list is said now, with nothing started.)
        if let Some(text) = &spec.approve_all_but
            && let Err(e) = super::leave::LeaveList::parse(text)
        {
            return Response::err("start", format!("--approve-all-but: {e}"), 2);
        }
        // Validate the agent command, parent tab, and name/session
        // conflicts BEFORE starting a shared minder so a typo never
        // leaves an unused minder running.
        if let Err(e) = spec.build_command() {
            return Response::err("start", e, 2);
        }
        let (sid, asker_win) = match self.place(caller, spec.parent_window.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Response::err("start", e, 1),
        };
        let _name_guard = match &spec.name {
            Some(name) => match PendingNameGuard::try_reserve(self.clone(), &sid, &asker_win, name) {
                Ok(g) => Some(g),
                Err(e) => return Response::err("start", e, 2),
            },
            None => None,
        };
        if !spec.fork
            && let Some(rid) = &spec.resume
            && let Some(other) = self.lock().reg.live_using_session_id(HOST, rid).cloned()
        {
            return Response::err("start", format!("session `{rid}` is already running in live child `{}` (stop it first or pass `--fork`)", other.id), 2);
        }

        // Allocate the worker's ID before creating the tab so its rules
        // file can be written atomically BEFORE attaching the worker to
        // a minder (never registering a minded worker without rules).
        let _worker_id_guard = ReservedIdGuard::allocate(self.clone());
        let worker_id = _worker_id_guard.id.clone();

        // Start (or reuse) the shared minder FIRST so the new agent is
        // nested directly under the minder (`lead ▸ minder ▸ agents`).
        let mut agent_spec = spec.clone();
        agent_spec.id = Some(worker_id.clone());
        let mut chosen_minder: Option<(NativeSubagent, bool, PathBuf, PendingMindGuard)> = None;
        let mut minder_error: Option<String> = None;
        if let Some((choice, rules)) = minder_req {
            let mut env = spec.env.clone();
            env.extend(spec.minder_env.iter().cloned());
            match self.find_or_start_minder(req, caller, &sid, &asker_win, &choice, env, spec.unset_env.clone(), None, spec.name.as_deref()).await {
                Ok((minder, fresh, mut guard)) => match self.write_agent_rules(&worker_id, &rules) {
                    Ok(rules_path) => {
                        agent_spec.parent_window = Some(minder.window_id.clone());
                        agent_spec.minded_by = Some(minder.id.clone());
                        chosen_minder = Some((minder, fresh, rules_path, guard));
                    }
                    Err(e) => {
                        if fresh {
                            self.cleanup_fresh_empty_minder(&minder, &mut guard).await;
                        } else {
                            drop(guard);
                        }
                        agent_spec.parent_window = Some(asker_win.clone());
                        agent_spec.minded_by = None;
                        minder_error = Some(format!("its rules couldn't be written: {e}"));
                    }
                },
                Err(e) if matches!(choice, minder::MinderChoice::Named(_)) => {
                    return Response::err("start", e, 1);
                }
                // (Could not start a minder: the child still starts
                // under the asker, unminded, with `minder_error`.)
                Err(e) => minder_error = Some(e),
            }
        }

        let (mut answer, minder_lost, leave_res) = self.start_spec_attached(req, agent_spec, caller, Some(&asker_win), chosen_minder.as_mut().map(|(m, _, _, g)| (m.id.as_str(), g))).await;
        if !answer.ok {
            if let Some((m, fresh, rules_path, mut guard)) = chosen_minder {
                let _ = std::fs::remove_file(&rules_path);
                if fresh {
                    self.cleanup_fresh_empty_minder(&m, &mut guard).await;
                } else {
                    drop(guard);
                }
            }
            return answer;
        }
        if let Some(reason) = minder_lost {
            if let Some((m, fresh, rules_path, mut guard)) = chosen_minder.take() {
                let _ = std::fs::remove_file(&rules_path);
                if fresh {
                    self.cleanup_fresh_empty_minder(&m, &mut guard).await;
                } else {
                    drop(guard);
                }
            }
            minder_error = Some(reason);
        }
        if let Some(id) = answer.data["id"].as_str() {
            self.keep_start(id, raw);
        }
        if let Some(res) = leave_res {
            match res {
                Ok(n) => answer.data["approve_all_but"] = json!(n),
                Err(e) => answer.stderr.push_str(&format!("olo agent start: its list couldn't be kept ({e}): everything it asks is asked\n")),
            }
            if spec.json {
                answer.stdout = format!("{}\n", answer.data);
            }
        }
        if let Some((minder, fresh, rules_path, _guard)) = chosen_minder
            && let Some(child) = answer.data["id"].as_str().and_then(|id| self.entry(id))
        {
            self.emit_minding_event(&minder, &child, "added", &rules_path);
            let minder_json = json!({
                "id": minder.id,
                "name": minder.name,
                "window_id": minder.window_id,
                "cli": minder.cli.binary(),
                "model": minder.model,
                "effort": minder.effort,
                "rules": rules_path.display().to_string(),
                "reused": !fresh,
            });
            if !spec.json {
                let desc = format!("{}:{}:{}", minder.cli.binary(), minder.model, minder.effort);
                let reused = if fresh { "" } else { " (reused)" };
                answer.stdout.push_str(&format!("{}  its minder ({desc}){reused}  tab={}\n", minder.id, minder.window_id));
            }
            answer.data["minder"] = minder_json;
            if spec.json {
                answer.stdout = format!("{}\n", answer.data);
            }
        } else if let Some(e) = minder_error
            && let Some(child) = answer.data["id"].as_str().and_then(|id| self.entry(id))
        {
            answer.stderr.push_str(&format!("olo agent start: `{}` runs, but it is unminded: {e}\n", child.name.as_deref().unwrap_or(&child.id)));
            answer.data["minder_error"] = json!(e);
            if spec.json {
                answer.stdout = format!("{}\n", answer.data);
            }
        }
        answer
    }

    /// The parent's rules for a minder: its words (`--rules-text`), or a
    /// file's (`--rules`, from the folder it was asked in); none given,
    /// none.
    fn rules_given(&self, req: &Request, text: Option<&str>, file: Option<&str>) -> Result<Option<String>, String> {
        match (text, file) {
            (Some(_), Some(_)) => Err("`--rules FILE` or `--rules-text TEXT`, not both".into()),
            (Some(text), None) => Ok(Some(text.to_owned())),
            (None, Some(file)) => {
                let path = if file.starts_with('/') || req.cwd.trim().is_empty() { PathBuf::from(file) } else { Path::new(req.cwd.trim()).join(file) };
                match std::fs::read_to_string(&path) {
                    Ok(text) if text.len() <= 64 << 10 => Ok(Some(text)),
                    Ok(_) => Err(format!("the rules in {} are over 64 KB: a minder reads them at every question", path.display())),
                    Err(e) => Err(format!("the rules couldn't be read ({}): {e}", path.display())),
                }
            }
            (None, None) => Ok(None),
        }
    }

    /// Finds an existing running shared minder under `asker_win` with
    /// room (or by explicit name), or starts a fresh shared minder
    /// under `asker_win`. Serialized by `minder_gate` and reserved
    /// atomically under the state lock so concurrent `start --minder`
    /// bursts and empty-minder timers never race.
    #[allow(clippy::too_many_arguments)]
    async fn find_or_start_minder(
        self: &Arc<Self>,
        req: &Request,
        caller: &Caller,
        sid: &str,
        asker_win: &str,
        choice: &minder::MinderChoice,
        env: Vec<(String, String)>,
        unset_env: Vec<String>,
        already_minded: Option<&str>,
        reserved_name: Option<&str>,
    ) -> Result<(NativeSubagent, bool, PendingMindGuard), String> {
        let _gate = self.minder_gate.lock().await;
        let most = super::service::minds_most();
        let load_of = |st: &super::service::State, mid: &str| -> usize {
            let live = st.reg.minded_by(mid).into_iter().filter(|e| already_minded != Some(e.id.as_str())).count();
            live + st.pending_minds.get(mid).copied().unwrap_or(0)
        };
        let healthy = |st: &super::service::State, mid: &str| -> bool {
            !st.closing_minders.contains(mid) && !st.unhealthy_minders.contains(mid) && st.live.get(mid).and_then(|l| l.last_state) != Some(SubagentLiveState::Error)
        };
        match choice {
            minder::MinderChoice::Named(name) => {
                // Resolve, validate health/load, and reserve atomically under ONE
                // lock hold so an empty-minder timer cannot start closing `m`
                // between lookup and reservation.
                let q = name.trim();
                let mut st = self.lock();
                let live_wins = st.sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default();
                let matches_q = |e: &&NativeSubagent| e.id == q || e.window_id == q || e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(q));
                let own_live_shared: Vec<NativeSubagent> = st
                    .reg
                    .entries
                    .iter()
                    .filter(|e| {
                        e.host_key == HOST
                            && e.session_id == sid
                            && e.parent_window_id == asker_win
                            && e.exited_at.is_none()
                            && live_wins.contains(&e.window_id)
                            && e.is_shared_minder()
                            && matches_q(e)
                    })
                    .cloned()
                    .collect();
                let m = if own_live_shared.len() == 1 {
                    own_live_shared[0].clone()
                } else if own_live_shared.len() > 1 {
                    let list: Vec<String> = own_live_shared.iter().map(|e| format!("{} ({})", e.id, e.display_title())).collect();
                    return Err(format!("ambiguous minder `{q}` under this tab: {} (use exact ID)", list.join(", ")));
                } else {
                    if let Some(other) = st.reg.entries.iter().rev().find(|e| e.host_key == HOST && e.session_id == sid && e.parent_window_id == asker_win && matches_q(e)) {
                        if other.exited_at.is_some() {
                            return Err(format!("minder `{q}` has exited"));
                        }
                        if other.minds.is_some() {
                            return Err(format!("`{q}` is an older 1:1 minder and minds only its one agent"));
                        }
                        return Err(format!("`{q}` is not a minder"));
                    }
                    return Err(format!("no running minder named `{q}` under this tab (give one of this tab's minders by name or id, or `claude:MODEL:EFFORT` to start one)"));
                };
                if st.closing_minders.contains(&m.id) {
                    return Err(format!("minder `{name}` is shutting down"));
                }
                if !healthy(&st, &m.id) {
                    return Err(format!("minder `{name}` is in an error state"));
                }
                let load = load_of(&st, &m.id);
                if load >= most {
                    return Err(format!("minder `{name}` already minds {load} agent(s) (the most is {most})"));
                }
                let guard = PendingMindGuard::reserve_locked(self.clone(), &mut st, m.id.clone());
                Ok((m, false, guard))
            }
            minder::MinderChoice::Spec(who) => {
                let reused = {
                    let mut st = self.lock();
                    let live_wins = st.sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default();
                    let found = st
                        .reg
                        .entries
                        .iter()
                        .find(|e| {
                            e.host_key == HOST
                                && e.session_id == sid
                                && e.parent_window_id == asker_win
                                && e.exited_at.is_none()
                                && live_wins.contains(&e.window_id)
                                && e.is_shared_minder()
                                && e.cli == who.cli
                                && e.model == who.model
                                && e.effort == who.effort
                                && healthy(&st, &e.id)
                                && load_of(&st, &e.id) < most
                        })
                        .cloned();
                    found.map(|m| {
                        let guard = PendingMindGuard::reserve_locked(self.clone(), &mut st, m.id.clone());
                        (m, guard)
                    })
                };
                if let Some((m, guard)) = reused {
                    return Ok((m, false, guard));
                }
                let m = self.start_shared_minder(req, caller, sid, asker_win, who, env, unset_env, reserved_name).await?;
                let mut st = self.lock();
                let still_ok = st.reg.entries.iter().any(|e| e.id == m.id && e.exited_at.is_none()) && healthy(&st, &m.id);
                if !still_ok {
                    return Err(format!("newly started minder `{}` exited or became unavailable immediately", m.id));
                }
                let guard = PendingMindGuard::reserve_locked(self.clone(), &mut st, m.id.clone());
                Ok((m, true, guard))
            }
        }
    }

    /// Starts a new shared minder (`shared_minder: true`, `minds: None`) as
    /// a direct child tab of `asker_win`, with its own command at
    /// `minders/MINDER_ID/mind`.
    #[allow(clippy::too_many_arguments)]
    async fn start_shared_minder(
        self: &Arc<Self>,
        req: &Request,
        caller: &Caller,
        sid: &str,
        asker_win: &str,
        who: &minder::MinderSpec,
        env: Vec<(String, String)>,
        unset_env: Vec<String>,
        reserved_name: Option<&str>,
    ) -> Result<NativeSubagent, String> {
        let _id_guard = ReservedIdGuard::allocate(self.clone());
        let minder_id = _id_guard.id.clone();
        let (name, title, _name_guard) = {
            let mut st = self.lock();
            let live = st.sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default();
            let taken = |st: &super::service::State, cand: &str| {
                reserved_name.is_some_and(|r| r.trim().eq_ignore_ascii_case(cand)) || Self::is_live_name_taken_for_lead_locked(st, sid, asker_win, cand, &live)
            };
            let (name, title) = if !taken(&st, "minder") {
                ("minder".to_owned(), "Minder".to_owned())
            } else {
                let mut n = 2usize;
                loop {
                    let candidate = format!("minder-{n}");
                    if !taken(&st, &candidate) {
                        break (candidate, format!("Minder {n}"));
                    }
                    n += 1;
                }
            };
            let guard = PendingNameGuard::insert_locked(self.clone(), &mut st, sid, asker_win, &name);
            (name, title, guard)
        };
        let dir = self.paths.dir.join("minders").join(&minder_id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let helper = std::env::current_exe().map_err(|e| format!("the helper's own path: {e}"))?;
        let command = dir.join(minder::COMMAND);
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(&command, minder::multi_script(&helper.to_string_lossy()))
                .and_then(|()| std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)))
                .map_err(|e| format!("{}: {e}", command.display()))?;
        }
        let from = self.cursor();
        let spec = StartSpec {
            id: Some(minder_id.clone()),
            cli: Some(who.cli),
            model: Some(who.model.clone()),
            effort: Some(who.effort.clone()),
            name: Some(name),
            title: Some(title),
            role: Some("minder".to_owned()),
            parent_window: Some(asker_win.to_owned()),
            // (One folder for every minder: Claude Code is told once
            // that it is trusted, not once a minder.)
            cwd: Some(self.paths.dir.join("minders").display().to_string()),
            trust: true,
            // Its one command in the shell, and reading: no tool that
            // changes anything, no server's tools, and no asking.
            permission_mode: Some(minder::CLAUDE_MODE.to_owned()),
            allowed_tools: minder::allowed_tools(&command.to_string_lossy()),
            strict_mcp_config: true,
            extra: minder::CLAUDE_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
            append_system_prompt: Some(minder::multi_brief(&command.to_string_lossy(), from)),
            prompt: Some(minder::multi_begin(&command.to_string_lossy(), from)),
            env,
            unset_env,
            desktop: DesktopPolicy::None,
            close_on_exit: true,
            json: true,
            minds: None,
            minded_by: None,
            shared_minder: true,
            ..StartSpec::default()
        };
        let started = self.start_spec(req, spec, caller).await;
        if !started.ok {
            return Err(started.stderr.trim().trim_start_matches("olo agent start: ").to_owned());
        }
        self.entry(&minder_id).ok_or_else(|| "the minder started but wasn't registered".to_owned())
    }

    /// `olo agent mind NAME …`: puts a running agent under the asker's
    /// shared minder (`--with claude:M:E|NAME`), updates its rules
    /// without restarting the minder (`--rules`), takes it off its
    /// minder (`--off`, moving it back under the minder's parent tab),
    /// or reports how it is minded and what has been answered.
    async fn mind(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("mind", msg, code);
        let spec = match minder::MindSpec::parse(&args_of(raw).unwrap_or_default()) {
            Ok(s) => s,
            Err(e) => return err(e, 2),
        };
        // For moves/attaches (`mind NAME --with ...`), resolve the source
        // strictly inside the asking tab's own subtree so one lead never takes
        // another lead's worker by its bare name.
        let it = if spec.with.is_some() && caller.child.is_none() {
            if caller.window_from_block {
                match self.lock().reg.resolve_target_scoped(HOST, caller.session.as_deref(), caller.window.as_deref(), true, &spec.target) {
                    Ok(e) => e,
                    Err(_) => return err(format!("`{}` is not under your tab: `mind --with` only moves your own descendants", spec.target), 1),
                }
            } else {
                let t = spec.target.trim();
                let exact = self.lock().reg.entries.iter().rev().find(|e| e.host_key == HOST && (e.id == t || e.window_id == t || e.block_id == t)).cloned();
                match exact {
                    Some(e) => e,
                    None => return err(format!("`mind {t} --with` without a mapped `REX_BLOCK` requires an exact child ID, not a bare name"), 1),
                }
            }
        } else {
            match self.resolve(caller, &spec.target) {
                Ok(e) => e,
                Err(e) => return err(e, 1),
            }
        };
        let its_name = it.name.clone().unwrap_or_else(|| it.id.clone());
        let dir = self.paths.dir.join("minders").join(&it.id);
        let now = self.lock().reg.minder_of(&it.id).cloned();
        let told = |m: &NativeSubagent| json!({ "id": m.id, "name": m.name, "window_id": m.window_id, "cli": m.cli.binary(), "model": m.model, "effort": m.effort });
        // Its list of what is not allowed without asking: given (or
        // another in its place), or taken away. By itself, or with
        // what is asked of its minder in the same command.
        let mut listed = None::<String>;
        if let Some(text) = &spec.leave {
            match self.set_leave(&it.id, text).await {
                Ok(n) => {
                    listed =
                        Some(format!("{its_name}: the service allows what it asks leave for, once, except what {n} pattern{} name{}\n", if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" }))
                }
                Err(e) => return err(format!("--approve-all-but: {e}"), 2),
            }
        } else if spec.leave_off {
            match self.clear_leave(&it.id).await {
                Ok(had) => {
                    listed = Some(if had { format!("{its_name}: everything it asks leave for is asked again\n") } else { format!("{its_name} had no list\n") });
                }
                Err(e) => return err(format!("--approve-none: {e}"), 1),
            }
        }
        let patterns = || self.leave_of(&it.id).map(|l| l.patterns().iter().map(|p| (*p).to_owned()).collect::<Vec<_>>());
        if let Some(said) = listed
            && !spec.off
            && spec.with.is_none()
            && spec.rules.is_none()
            && spec.rules_file.is_none()
        {
            let payload = json!({ "ok": true, "id": it.id, "name": it.name, "approve_all_but": patterns(), "minder": now.as_ref().map(told) });
            return Response::ok(if spec.json { format!("{payload}\n") } else { said }, payload);
        }
        // Told of: who minds it, by what rules, and what was answered.
        if !spec.off && spec.with.is_none() && spec.rules.is_none() && spec.rules_file.is_none() {
            let rules = std::fs::read_to_string(dir.join("rules.md")).ok();
            let answered: Vec<Value> = std::fs::read_to_string(dir.join("answers.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": now.as_ref().map(told), "rules": rules, "approve_all_but": patterns(), "answered": answered });
            let stdout = if spec.json {
                format!("{payload}\n")
            } else {
                let mut out = match &now {
                    Some(m) => format!("{its_name} is minded by {} ({}:{}:{})\n", m.name.as_deref().unwrap_or(&m.id), m.cli.binary(), m.model, m.effort),
                    None => format!("{its_name} has no minder\n"),
                };
                if let Some(patterns) = patterns() {
                    out.push_str(&format!(
                        "  what it asks leave for is allowed by the service, once, except what these name: {}\n",
                        if patterns.is_empty() { "(nothing)".to_owned() } else { patterns.join("  |  ") }
                    ));
                }
                for a in answered.iter().rev().take(20).rev() {
                    out.push_str(&format!("  {}  {}\n", a["did"].as_str().unwrap_or_default(), a["said"]));
                }
                out
            };
            return Response::ok(stdout, payload);
        }
        // Serialize mutating minder operations (`--off`, `--rules`, `--with`)
        // per worker so concurrent moves or detaches cannot interleave and
        // leave stale `removed` notices or stranded empty minders.
        let _worker_lock = self.worker_lock(&it.id).lock_owned().await;
        let it = match self.entry(&it.id) {
            Some(e) => e,
            None => return err(format!("`{its_name}` is no longer known"), 1),
        };
        let its_name = it.name.clone().unwrap_or_else(|| it.id.clone());
        let now = self.lock().reg.minder_of(&it.id).cloned();

        if it.is_minder() {
            return err("a minder isn't minded".into(), 1);
        }
        if it.exited_at.is_some() {
            return err(format!("`{its_name}` has exited"), 1);
        }
        if spec.off {
            let Some(m) = now else { return err(format!("`{its_name}` has no minder"), 1) };
            // Older 1:1 minder: stop it directly, propagate any close failure,
            // and capture cursor BEFORE republishing any standing question.
            if m.minds.is_some() {
                let (_, failed) = self.stop_tabs_inner(&m, true, true, false, true).await;
                if !failed.is_empty() {
                    let payload = json!({ "ok": false, "id": it.id, "name": it.name, "minder": told(&m), "failed": failed });
                    return Response {
                        ok: false,
                        stdout: if spec.json { format!("{payload}\n") } else { String::new() },
                        stderr: format!("olo agent mind: couldn't close legacy minder `{}`\n", m.id),
                        code: 1,
                        data: payload,
                    };
                }
                let latest_it = self.entry(&it.id).unwrap_or(it.clone());
                let pre_cursor = self.cursor();
                self.republish_standing_question(&latest_it).await;
                let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": Value::Null, "stopped": m.id, "cursor": pre_cursor });
                return Response::ok(if spec.json { format!("{payload}\n") } else { format!("{its_name} is no longer minded (its asking comes to you)\n") }, payload);
            }
            // Shared minder: move the agent back under the minder's
            // parent tab, unmind it, tell followers and the minder,
            // re-publish any standing question so an ongoing lead watch
            // wakes immediately, and schedule the minder to end if empty.
            let remaining = {
                let mut st = self.lock();
                for e in &mut st.reg.entries {
                    if e.id == it.id && e.exited_at.is_none() {
                        e.minded_by = None;
                        if e.parent_window_id == m.window_id {
                            e.parent_window_id = m.parent_window_id.clone();
                        }
                    }
                }
                st.seen.remove(&(m.id.clone(), it.id.clone()));
                st.told.remove(&(m.id.clone(), it.id.clone()));
                st.pending_rules_rev.remove(&(m.id.clone(), it.id.clone()));
                *st.rules_gen.entry(it.id.clone()).or_insert(0) += 1;
                st.reg.minded_by(&m.id).len()
            };
            self.saved();
            self.notify_child_state(&it.id);
            self.tell_entry(&it.id);
            let rules_path = self.rules_path(&it.id);
            self.emit_minding_event(&m, &it, "removed", &rules_path);
            let latest_it = self.entry(&it.id).unwrap_or(it.clone());
            // Capture cursor BEFORE republishing so `watch --since <detach_cursor>`
            // immediately observes the freshly republished standing question.
            let pre_cursor = self.cursor();
            self.republish_standing_question(&latest_it).await;
            self.changed.notify_waiters();
            if remaining == 0 {
                self.arm_empty_minder_if_idle(&m.id, super::service::minder_reports());
            }
            let payload = json!({
                "ok": true, "id": it.id, "name": it.name, "minder": Value::Null,
                "detached_from": m.id, "parent_window_id": m.parent_window_id, "cursor": pre_cursor,
            });
            return Response::ok(if spec.json { format!("{payload}\n") } else { format!("{its_name} is no longer minded (its asking comes to you)\n") }, payload);
        }

        // `--rules` alone on an agent minded by a shared minder: rewrite
        // its rules file atomically and tell the running minder without restarting it.
        if spec.with.is_none() {
            let Some(m) = now.as_ref() else {
                return err(format!("`{its_name}` has no minder to give rules to: say which with `--with claude:MODEL:EFFORT` or `--with NAME`"), 2);
            };
            let rules = match self.rules_given(req, spec.rules.as_deref(), spec.rules_file.as_deref()) {
                Ok(Some(r)) => r,
                Ok(None) => std::fs::read_to_string(dir.join("rules.md")).unwrap_or_default(),
                Err(e) => return err(e, 2),
            };
            if m.is_shared_minder() {
                let rules_path = match self.write_agent_rules(&it.id, &rules) {
                    Ok(p) => p,
                    Err(e) => return err(format!("couldn't write rules for `{its_name}`: {e}"), 1),
                };
                let pre = self.cursor();
                // Void any approval/question grant decided under the old rules
                // and require delivery of the `minding` ("rules") revision before
                // a fresh grant can be minted under the new `rules_gen`.
                {
                    let mut st = self.lock();
                    let still_minded = st.reg.entries.iter().rev().find(|e| e.id == it.id && e.exited_at.is_none()).is_some_and(|e| e.minded_by.as_deref() == Some(m.id.as_str()));
                    if !still_minded {
                        return err(format!("`{its_name}` exited or is no longer minded by `{}`", m.name.as_deref().unwrap_or(&m.id)), 1);
                    }
                    let new_gen = {
                        let g = st.rules_gen.entry(it.id.clone()).or_insert(0);
                        *g += 1;
                        *g
                    };
                    st.seen.remove(&(m.id.clone(), it.id.clone()));
                    st.told.remove(&(m.id.clone(), it.id.clone()));
                    st.pending_rules_rev.insert((m.id.clone(), it.id.clone()), (new_gen, rules_path.display().to_string()));
                }
                self.saved();
                self.notify_child_state(&it.id);
                self.emit_minding_event(m, &it, "rules", &rules_path);
                let mut m_json = told(m);
                m_json["rules"] = json!(rules_path.display().to_string());
                let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": m_json, "replaced": Value::Null, "cursor": pre });
                let stdout = if spec.json { format!("{payload}\n") } else { format!("updated rules for `{its_name}` (minder {})\n", m.name.as_deref().unwrap_or(&m.id)) };
                return Response::ok(stdout, payload);
            }
            // Older 1:1 minder given new rules: migrate cleanly to a shared minder of the same kind.
        }

        let choice = match (&spec.with, &now) {
            (Some(c), _) => c.clone(),
            (None, Some(m)) => minder::MinderChoice::Spec(minder::MinderSpec { cli: m.cli, model: m.model.clone(), effort: m.effort.clone() }),
            (None, None) => return err(format!("`{its_name}` has no minder to give rules to: say which with `--with claude:MODEL:EFFORT` or `--with NAME`"), 2),
        };
        let rules = match self.rules_given(req, spec.rules.as_deref(), spec.rules_file.as_deref()) {
            Ok(Some(rules)) => rules,
            // (None given with `--with`: those it had, if it had any.)
            Ok(None) => std::fs::read_to_string(dir.join("rules.md")).unwrap_or_default(),
            Err(e) => return err(e, 2),
        };
        let asker_win = match self.place(caller, None).await {
            Ok((sid, win)) if sid == it.session_id => win,
            // For an older 1:1 minder, `m.parent_window_id` is `it.window_id` itself;
            // only a shared minder's parent is the worker's lead.
            _ => now.as_ref().filter(|m| m.is_shared_minder()).map(|m| m.parent_window_id.clone()).unwrap_or_else(|| it.parent_window_id.clone()),
        };
        let pre = self.cursor();
        let (target_minder, fresh, mut guard) = match self.find_or_start_minder(req, caller, &it.session_id, &asker_win, &choice, spec.env.clone(), Vec::new(), Some(&it.id), None).await {
            Ok(v) => v,
            Err(e) => return err(format!("`{its_name}`'s minder couldn't be started: {e}"), 1),
        };
        if Service::test_env_allowed()
            && let Some(ms) = std::env::var("OLO_REX_AGENTS_TEST_MIND_HOLD_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0)
        {
            tokio::time::sleep(Duration::from_millis(ms.min(5000))).await;
        }
        // Validate live state, cycles, and duplicate names BEFORE writing rules.
        let pre_err: Option<(String, i32)> = {
            let st = self.lock();
            if !st.reg.entries.iter().rev().any(|e| e.id == it.id && e.exited_at.is_none()) {
                Some((format!("`{its_name}` exited while attaching its minder"), 1))
            } else if target_minder.window_id == it.window_id || st.reg.is_strict_descendant(HOST, &it.session_id, &it.window_id, &target_minder.window_id) {
                Some((format!("cannot put `{its_name}` under `{}`: it is already below `{its_name}` (would form a cycle)", target_minder.name.as_deref().unwrap_or(&target_minder.id)), 1))
            } else if let Some(it_name) = it.name.as_deref().filter(|n| !n.trim().is_empty())
                && now.as_ref().map(|m| m.id.as_str()) != Some(target_minder.id.as_str())
                && st.reg.entries.iter().any(|e| {
                    e.id != it.id
                        && e.exited_at.is_none()
                        && e.session_id == it.session_id
                        && (e.parent_window_id == target_minder.window_id || e.minded_by.as_deref() == Some(target_minder.id.as_str()))
                        && e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(it_name))
                })
            {
                Some((format!("minder `{}` already minds a live child named `{it_name}`", target_minder.name.as_deref().unwrap_or(&target_minder.id)), 2))
            } else {
                None
            }
        };
        if let Some((msg, code)) = pre_err {
            if fresh {
                self.cleanup_fresh_empty_minder(&target_minder, &mut guard).await;
            } else {
                drop(guard);
            }
            return err(msg, code);
        }
        let rules_path = match self.write_agent_rules(&it.id, &rules) {
            Ok(p) => p,
            Err(e) => {
                if fresh {
                    self.cleanup_fresh_empty_minder(&target_minder, &mut guard).await;
                } else {
                    drop(guard);
                }
                return err(format!("couldn't write rules for `{its_name}`: {e}"), 1);
            }
        };
        // Commit the assignment atomically under ONE lock hold: re-verify
        // worker & target minder are both still live and healthy, convert
        // `pending_minds` into the live assignment, bump `empty_gen` and
        // `rules_gen`, and derive the actual previous minder.
        let commit: Result<Option<NativeSubagent>, (String, i32)> = {
            let mut st = self.lock();
            let worker_live = st.reg.entries.iter().rev().any(|e| e.id == it.id && e.exited_at.is_none());
            let target_live = st.reg.entries.iter().rev().find(|e| e.id == target_minder.id && e.exited_at.is_none() && e.is_shared_minder()).is_some_and(|m| {
                !st.closing_minders.contains(&m.id)
                    && !st.unhealthy_minders.contains(&m.id)
                    && st.live.get(&m.id).and_then(|l| l.last_state) != Some(SubagentLiveState::Error)
                    && st.sessions.get(&it.session_id).is_some_and(|s| s.windows.contains(&m.window_id))
            });
            if !worker_live {
                Err((format!("`{its_name}` exited while attaching its minder"), 1))
            } else if !target_live {
                Err((format!("minder `{}` ended or became unavailable while attaching `{its_name}`", target_minder.name.as_deref().unwrap_or(&target_minder.id)), 1))
            } else if target_minder.window_id == it.window_id || st.reg.is_strict_descendant(HOST, &it.session_id, &it.window_id, &target_minder.window_id) {
                Err((format!("cannot put `{its_name}` under `{}`: it is already below `{its_name}` (would form a cycle)", target_minder.name.as_deref().unwrap_or(&target_minder.id)), 1))
            } else if let Some(it_name) = it.name.as_deref().filter(|n| !n.trim().is_empty())
                && st.reg.minder_of(&it.id).map(|m| m.id.as_str()) != Some(target_minder.id.as_str())
                && st.reg.entries.iter().any(|e| {
                    e.id != it.id
                        && e.exited_at.is_none()
                        && e.session_id == it.session_id
                        && (e.parent_window_id == target_minder.window_id || e.minded_by.as_deref() == Some(target_minder.id.as_str()))
                        && e.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(it_name))
                })
            {
                Err((format!("minder `{}` already minds a live child named `{it_name}`", target_minder.name.as_deref().unwrap_or(&target_minder.id)), 2))
            } else {
                let actual_old = st.reg.minder_of(&it.id).cloned();
                let same_minder = actual_old.as_ref().is_some_and(|m| m.id == target_minder.id);
                let new_gen = {
                    let g = st.rules_gen.entry(it.id.clone()).or_insert(0);
                    *g += 1;
                    *g
                };
                if !same_minder {
                    for e in &mut st.reg.entries {
                        if e.id == it.id && e.exited_at.is_none() {
                            e.parent_window_id = target_minder.window_id.clone();
                            e.minded_by = Some(target_minder.id.clone());
                        }
                        // Revoke a replaced legacy 1:1 minder's authority in the
                        // exact same commit so it cannot keep acting if its tab
                        // takes time or fails to close.
                        if let Some(old) = &actual_old
                            && old.minds.is_some()
                            && e.id == old.id
                        {
                            e.minds = None;
                        }
                    }
                    if let Some(old) = &actual_old {
                        st.seen.remove(&(old.id.clone(), it.id.clone()));
                        st.told.remove(&(old.id.clone(), it.id.clone()));
                        st.pending_rules_rev.remove(&(old.id.clone(), it.id.clone()));
                    }
                    st.pending_rules_rev.remove(&(target_minder.id.clone(), it.id.clone()));
                } else {
                    st.pending_rules_rev.insert((target_minder.id.clone(), it.id.clone()), (new_gen, rules_path.display().to_string()));
                }
                if let Some(n) = st.pending_minds.get_mut(&target_minder.id) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        st.pending_minds.remove(&target_minder.id);
                    }
                }
                guard.disarm();
                Self::bump_empty_gen_locked(&mut st, &target_minder.id);
                st.seen.remove(&(target_minder.id.clone(), it.id.clone()));
                st.told.remove(&(target_minder.id.clone(), it.id.clone()));
                Ok(actual_old)
            }
        };
        let actual_old = match commit {
            Ok(old) => old,
            Err((msg, code)) => {
                if fresh {
                    self.cleanup_fresh_empty_minder(&target_minder, &mut guard).await;
                } else {
                    drop(guard);
                }
                return err(msg, code);
            }
        };
        if actual_old.as_ref().is_some_and(|m| m.id == target_minder.id) {
            self.saved();
            self.notify_child_state(&it.id);
            self.emit_minding_event(&target_minder, &it, "rules", &rules_path);
            let mut m_json = told(&target_minder);
            m_json["rules"] = json!(rules_path.display().to_string());
            m_json["reused"] = json!(true);
            let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": m_json, "replaced": Value::Null, "cursor": pre });
            let stdout = if spec.json { format!("{payload}\n") } else { format!("{its_name} is minded by {} ({})\n", target_minder.id, choice.describe()) };
            return Response::ok(stdout, payload);
        }
        // Detach/clean up any previous minder now that `it` is committed to `target_minder`.
        let mut old_close_failed: Vec<Value> = Vec::new();
        if let Some(old) = &actual_old {
            if old.minds.is_some() {
                let (_, failed) = self.stop_tabs_inner(old, true, true, false, true).await;
                old_close_failed = failed;
            } else if old.is_shared_minder() {
                self.emit_minding_event(old, &it, "removed", &rules_path);
                self.arm_empty_minder_if_idle(&old.id, super::service::minder_reports());
            }
        }
        self.saved();
        self.notify_child_state(&it.id);
        self.tell_entry(&it.id);
        let updated = self.entry(&it.id).unwrap_or(it);
        self.emit_minding_event(&target_minder, &updated, "added", &rules_path);
        let mut m_json = told(&target_minder);
        m_json["rules"] = json!(rules_path.display().to_string());
        m_json["reused"] = json!(!fresh);
        let ok = old_close_failed.is_empty();
        let mut payload = json!({
            "ok": ok, "id": updated.id, "name": updated.name, "minder": m_json,
            "replaced": actual_old.as_ref().map(|m| m.id.clone()),
            "parent_window_id": target_minder.window_id,
            "cursor": pre,
        });
        if !old_close_failed.is_empty() {
            payload["old_minder_close_failed"] = json!(old_close_failed);
        }
        let desc = format!("{}:{}:{}", target_minder.cli.binary(), target_minder.model, target_minder.effort);
        let stdout = if spec.json { format!("{payload}\n") } else { format!("{its_name} is minded by {} ({desc})\n", target_minder.id) };
        if ok {
            Response::ok(stdout, payload)
        } else {
            Response {
                ok: false,
                stdout,
                stderr: format!(
                    "olo agent mind: attached `{its_name}` to `{}` (and revoked the old legacy minder), but failed to close legacy minder `{}`\n",
                    target_minder.id,
                    actual_old.as_ref().map(|m| m.id.as_str()).unwrap_or_default()
                ),
                code: 1,
                data: payload,
            }
        }
    }

    /// If `entry` currently has a standing approval or question when detached
    /// from its minder (`--off`), record a fresh event strictly after any
    /// cursor the lead already holds so an ongoing lead `watch --since` wakes
    /// immediately with the open box.
    async fn republish_standing_question(self: &Arc<Self>, entry: &NativeSubagent) {
        if entry.exited_at.is_some() || !entry.cli.is_agent() {
            return;
        }
        let looked = self.look(entry, true).await;
        if !looked.is_fresh_ordered() || looked.cls.state == SubagentLiveState::Exited {
            return;
        }
        if Self::question_suppressed_locked(&self.lock(), &entry.id, &looked.cls, looked.boxed.as_ref()) {
            return;
        }
        let inspected = self.inspect(std::slice::from_ref(entry)).await;
        let info = inspected.get(&entry.id);
        let (eff_state, error, _) = effective_state(&looked.cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true);
        if !matches!(eff_state, SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion) {
            return;
        }
        let entry = self.entry(&entry.id).unwrap_or_else(|| entry.clone());
        {
            let st = self.lock();
            if entry.exited_at.is_some() || st.reg.minder_of(&entry.id).is_some() {
                return;
            }
            if Self::question_suppressed_locked(&st, &entry.id, &looked.cls, looked.boxed.as_ref()) {
                return;
            }
        }
        let pending = Self::pending_tools(info);
        let approval = looked.cls.approval.as_ref().map(|ap| Self::approval_json_with_box(ap, &pending, looked.boxed.as_ref().map(|b| b.key.as_str())));
        let key = super::service::subkey(&looked.cls);
        let dedupe = if eff_state == SubagentLiveState::WaitingApproval
            && let Some(box_key) = approval.as_ref().and_then(|a| a["box"].as_str())
        {
            format!("box:{box_key}")
        } else if !key.is_empty() {
            key
        } else {
            "tx:waiting".to_owned()
        };
        let mut ev = self.event_of(&entry, eff_state, looked.cls.summary.clone(), dedupe);
        ev.last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
        ev.approval = approval;
        ev.error = error;
        ev.tail = tail_of(&looked.screen, 10);
        self.lock().log.reset_child_dedupe(&entry.id);
        self.record(ev);
    }

    /// `olo agent escalate [NAME] "TEXT"`: something for the parent, on
    /// the parent's own watch at once (`event: escalation`), never
    /// typed into its pane. A shared minder may escalate about a minded
    /// agent (`escalate ID|NAME "TEXT"`, recorded as that agent's
    /// escalation) or about itself (`escalate "TEXT"` / `--self "TEXT"`);
    /// an older 1:1 minder's is of its one agent; any other child's is of itself.
    async fn escalate(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("escalate", msg, code);
        let args = args_of(raw).unwrap_or_default();
        let mut json_out = false;
        let mut explicit_self = false;
        let mut pos: Vec<String> = Vec::new();
        let mut rest_literal = false;
        for a in &args {
            if !rest_literal && a == "--" {
                rest_literal = true;
            } else if !rest_literal && a == "--json" {
                json_out = true;
            } else if !rest_literal && a == "--self" {
                explicit_self = true;
            } else if !rest_literal && a.starts_with("--") {
                // ignore unknown flags for compat with previous filter
            } else {
                pos.push(a.clone());
            }
        }
        if pos.is_empty() || pos.iter().all(|s| s.trim().is_empty()) {
            return err("usage: olo agent escalate [ID|NAME] \"TEXT\" [--self] [--json]   (what the parent needs to know, on its own watch at once)".into(), 2);
        }
        let Some(child) = &caller.child else {
            return err("only a child has a parent to tell: this was asked from a tab that isn't one".into(), 69);
        };
        let (about, text, about_agent) = if let Some(minded) = child.minds.as_deref() {
            match self.entry(minded) {
                Some(it) => (it, pos.join(" "), true),
                None => return err("the agent this minder minds is no longer known".into(), 1),
            }
        } else if child.is_shared_minder() {
            if explicit_self {
                (child.clone(), pos.join(" "), false)
            } else if pos.len() == 1 {
                // Single positional argument is always a self-escalation message,
                // regardless of whether any ward shares that word as its name.
                (child.clone(), pos[0].clone(), false)
            } else {
                // Two-argument form (`escalate TARGET "TEXT"`) MUST resolve the
                // target and propagate unknown/ambiguous target errors rather
                // than silently turning into a self-escalation.
                let agent = match self.resolve(caller, &pos[0]) {
                    Ok(a) => a,
                    Err(e) => return err(e, 1),
                };
                let msg = pos[1..].join(" ");
                if msg.trim().is_empty() {
                    return err(format!("escalating about `{}` needs a message: `mind escalate {} \"TEXT\"`", pos[0], pos[0]), 2);
                }
                (agent, msg, true)
            }
        } else {
            (child.clone(), pos.join(" "), false)
        };
        let text = text.trim();
        if text.is_empty() {
            return err("usage: olo agent escalate [ID|NAME] \"TEXT\" [--self] [--json]   (what the parent needs to know, on its own watch at once)".into(), 2);
        }
        // (How it stands as it is said: an event has a state.)
        let state = if about.exited_at.is_some() { SubagentLiveState::Exited } else { self.look(&about, false).await.cls.state };
        let said: String = text.chars().take(4000).collect();
        let mut event = self.event_of(&about, state, Some(said.clone()), String::new());
        event.event = minder::ESCALATION.to_owned();
        event.last_message = Some(said.clone());
        let seq = self.record_beside(event);
        if about_agent {
            self.minder_did(caller, &about.id, "escalate", json!(said));
        }
        // A minder with multiple exited agents keeps its grace period until
        // EVERY pending exit has been reported; only then does it end promptly.
        if about.exited_at.is_some() {
            if child.minds.is_some() {
                self.end_minder_in(child.clone(), Duration::from_millis(500));
            } else if child.is_shared_minder() && about_agent {
                let all_done = {
                    let mut st = self.lock();
                    if let Some(pending) = st.pending_exit_reports.get_mut(&child.id) {
                        pending.remove(&about.id);
                        if pending.is_empty() {
                            st.pending_exit_reports.remove(&child.id);
                        }
                    }
                    st.reg.minded_by(&child.id).is_empty() && st.pending_minds.get(&child.id).copied().unwrap_or(0) == 0 && st.pending_exit_reports.get(&child.id).is_none_or(HashSet::is_empty)
                };
                self.saved();
                if all_done {
                    self.end_minder_in(child.clone(), Duration::from_millis(500));
                }
            }
        }
        let payload = json!({ "ok": true, "cursor": seq, "id": about.id, "name": about.name, "event": minder::ESCALATION });
        Response::ok(if json_out { format!("{payload}\n") } else { format!("told its parent (cursor={seq})\n") }, payload)
    }

    async fn start_spec(self: &Arc<Self>, req: &Request, spec: StartSpec, caller: &Caller) -> Response {
        self.start_spec_attached(req, spec, caller, None, None).await.0
    }

    async fn start_spec_attached(
        self: &Arc<Self>,
        req: &Request,
        mut spec: StartSpec,
        caller: &Caller,
        fallback_parent: Option<&str>,
        pending: Option<(&str, &mut PendingMindGuard)>,
    ) -> (Response, Option<String>, Option<Result<usize, String>>) {
        let err = |msg: String, code: i32| (Response::err("start", msg, code), None, None);
        // A child's own child has no more of the desktop than it has.
        if let Some(cc) = &caller.child {
            match cc.desktop_policy.restrict_to(&spec.desktop) {
                Ok(capped) => spec.desktop = capped,
                Err(e) => return err(e, 69),
            }
        }
        // Folders as the asker meant them: from where it asked.
        let asked_in = Some(req.cwd.trim()).filter(|s| !s.is_empty()).map(str::to_owned);
        let whole = |p: &str, base: Option<&str>| -> String {
            match base {
                Some(b) if !p.starts_with('/') && !p.starts_with('~') => Path::new(b).join(p).display().to_string(),
                _ => p.to_owned(),
            }
        };
        if let Some(c) = spec.cwd.clone() {
            spec.cwd = Some(whole(&c, asked_in.as_deref()));
        }
        let base = spec.cwd.clone().or_else(|| asked_in.clone());
        for d in &mut spec.add_dirs {
            *d = whole(d, base.as_deref());
        }
        let (cli, model, effort, raw_argv) = match spec.build_command() {
            Ok(v) => v,
            Err(e) => return err(e, 2),
        };
        let mut pre_minder_lost: Option<String> = None;
        let (sid, parent_win) = match self.place(caller, spec.parent_window.as_deref()).await {
            Ok(v) => v,
            Err(e) if spec.minded_by.is_some() && fallback_parent.is_some() => match self.place(caller, fallback_parent).await {
                Ok(v) => {
                    spec.minded_by = None;
                    pre_minder_lost = Some(format!("selected minder ended while the agent was starting ({e})"));
                    v
                }
                Err(e2) => return err(e2, 1),
            },
            Err(e) => return err(e, 1),
        };
        {
            let st = self.lock();
            let live = st.sessions.get(&sid).map(|s| s.windows.clone()).unwrap_or_default();
            if let Some(name) = &spec.name
                && st.reg.is_live_name_taken(HOST, &sid, &parent_win, name, &live)
            {
                return err(format!("a live child named `{name}` already exists under this parent (choose a unique `--name` or stop it first)"), 2);
            }
            // Two programs writing one conversation: refused, unless it is forked.
            if !spec.fork
                && let Some(rid) = &spec.resume
                && let Some(other) = st.reg.live_using_session_id(HOST, rid)
            {
                return err(format!("session `{rid}` is already running in live child `{}` (stop it first or pass `--fork`)", other.id), 2);
            }
        }
        // (Before the tab is made: a child that ends at once is still after it.)
        let start_cursor = self.cursor();
        let cwd = spec.cwd.clone().or(asked_in);
        let title = spec.effective_title(cli);

        // Claude Code's question about a new folder, answered before it is asked.
        if spec.trust && cli == AgentCli::Claude {
            let mut folders: Vec<String> = cwd.iter().cloned().collect();
            for d in &spec.add_dirs {
                if !folders.contains(d) {
                    folders.push(d.clone());
                }
            }
            if !folders.is_empty() {
                let given = |name: &str| spec.env.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
                let (home, config) = (given("HOME"), given("CLAUDE_CONFIG_DIR"));
                let _ = tokio::task::spawn_blocking(move || crate::agent::trust(home.as_deref(), config.as_deref(), &folders)).await;
            }
        }

        // What the child's program is run with: its environment set and
        // unset in front of it (the server's own tab takes none).
        let mut launch = spec.clone();
        let explicit_path = launch.env.iter().any(|(k, _)| k == "PATH") || launch.unset_env.iter().any(|k| k == "PATH");
        if matches!(spec.desktop, DesktopPolicy::Inherit) {
            // (What the desktop gave the asking pane, a child that has the desktop's leave has too.)
            for (name, value) in &req.inherit {
                if matches!(name.as_str(), "SSH_AUTH_SOCK" | "BROWSER") && !value.is_empty() && !launch.env.iter().any(|(k, _)| k == name) {
                    launch.env.push((name.clone(), value.clone()));
                }
            }
        }
        let path_prepend = inherited_path_prepend(&spec.desktop, &req.inherit, explicit_path);
        let argv = launch.wrap_with_env_isolation(raw_argv.clone());
        let mut opts = json!({ "command": argv, "interactive": true, "exit": { "on_completion": spec.close_on_exit } });
        if let Some(dir) = &cwd {
            opts["cwd"] = json!(dir);
        }
        if !path_prepend.is_empty() {
            opts["path_prepend"] = json!(path_prepend);
        }
        // As large as the pane it was asked from (or its parent tab's):
        // with no desktop showing it, a tab would be 80 by 24.
        let like = caller.block.clone().or_else(|| self.lock().sessions.get(&sid).and_then(|s| s.blocks.iter().find(|(_, w)| **w == parent_win).map(|(b, _)| b.clone())));
        if let Some(block) = like
            && let Ok(size) = self.rex.terminal(&sid, &block, "size", json!({})).await
            && let (Some(columns), Some(rows)) = (size["columns"].as_u64().filter(|c| *c >= 20), size["rows"].as_u64().filter(|r| *r >= 5))
        {
            opts["size"] = json!({ "columns": columns, "rows": rows });
            if let (Some(w), Some(h)) = (size["cell_width_px"].as_u64().filter(|v| *v > 0), size["cell_height_px"].as_u64().filter(|v| *v > 0)) {
                opts["size"]["cell_width_px"] = json!(w);
                opts["size"]["cell_height_px"] = json!(h);
            }
        }
        // (When its tab was asked for: its program is up, and can have
        // begun its turn, before the server has answered; and nothing
        // of its is from before this. Capture the monotonic instant for
        // startup `--trust` here too, before `session.new_window`.)
        let asked_ms = now_unix_ms();
        let asked_instant = Instant::now();
        let created = match self
            .rex
            .call(
                "session.new_window",
                json!({ "session_id": sid, "window_label": title, "focus": spec.focus, "layout": { "block": { "flavor": "com.superlogical.terminal.shell", "options": opts } } }),
            )
            .await
        {
            Ok(created) => created,
            Err(e) => return err(format!("failed to create Rex tab: {e}"), 1),
        };
        let (Some(window_id), Some(block_id)) = (created["window_id"].as_str(), created["block_ids"][0].as_str()) else {
            // Astra #13: If Rex returned a `window_id` without a `block_id`,
            // roll back and close that orphaned tab before returning an error!
            if let Some(win) = created["window_id"].as_str() {
                let _ = self.rex.call("session.close_window", json!({ "session_id": sid, "window_id": win })).await;
            }
            return err("Rex created the tab but didn't say which it is".into(), 1);
        };
        if let Some(sess) = self.lock().sessions.get_mut(&sid) {
            sess.windows.insert(window_id.to_owned());
            sess.blocks.insert(block_id.to_owned(), window_id.to_owned());
        }
        // (A fork's conversation is a new one, found once its agent has made it.)
        let known = if spec.fork { spec.session_id.clone() } else { spec.session_id.clone().or_else(|| spec.resume.clone()) };
        let entry = NativeSubagent {
            id: spec.id.clone().unwrap_or_else(new_subagent_id),
            name: spec.name.clone(),
            host_key: HOST.to_owned(),
            session_id: sid.clone(),
            window_id: window_id.to_owned(),
            block_id: block_id.to_owned(),
            parent_window_id: parent_win,
            parent_block_id: caller.block.clone(),
            cli,
            model: model.clone(),
            effort: effort.clone(),
            title: title.clone(),
            role: spec.role.clone(),
            cwd: cwd.clone(),
            resume_id: spec.resume.clone(),
            session_confidence: known.as_ref().map(|_| "argv".to_owned()),
            cli_session_id: known,
            transcript_path: None,
            desktop_policy: spec.desktop.clone(),
            stream_mode: spec.print_mode,
            started_at: asked_ms as i64,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: created["revision"].as_u64().unwrap_or(0),
            seen_live: true,
            exited_by_sync: false,
            add_dirs: spec.add_dirs.clone(),
            command: if cli == AgentCli::Command { raw_argv.clone() } else { Vec::new() },
            minds: spec.minds.clone(),
            minded_by: spec.minded_by.clone(),
            shared_minder: spec.shared_minder,
        };
        if entry.minded_by.is_some()
            && Service::test_env_allowed()
            && let Some(ms) = std::env::var("OLO_REX_AGENTS_TEST_START_HOLD_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0)
        {
            tokio::time::sleep(Duration::from_millis(ms.min(5000))).await;
        }
        // (Its own folders' trust is the service's to answer from the moment
        // its tab was requested, spawned immediately at registration BEFORE
        // awaiting `write_now()` so queued registry saves never eat the 15 s
        // trust window. Any startup `--approve-all-but` list is also installed
        // at registration BEFORE `watch_child` starts.)
        let trust_until = (spec.trust && cli.is_agent()).then(|| asked_instant + Duration::from_secs(15));
        let expect_work = (cli.is_agent() && spec.prompt.as_deref().is_some_and(|p| !p.trim().is_empty())).then_some((start_cursor, Duration::from_secs(120)));
        let (entry, lost, leave_res) = match self.begin_attached(entry, fallback_parent, pending, trust_until, false, expect_work, spec.approve_all_but.as_deref()).await {
            Ok(v) => v,
            Err(e) => return err(e, 1),
        };
        let minder_lost = pre_minder_lost.or(lost);

        let payload = json!({
            "ok": true,
            "id": entry.id,
            "name": entry.name,
            "window_id": entry.window_id,
            "block_id": entry.block_id,
            "parent_window_id": entry.parent_window_id,
            "cli": cli.binary(),
            "model": model,
            "effort": effort,
            "title": title,
            "role": entry.role,
            "cwd": cwd,
            "resume_id": entry.resume_id,
            "cli_session_id": entry.effective_session_id(),
            "desktop": entry.desktop_policy.describe(),
            "started_at": to_unix_ms(entry.started_at),
            "cursor": start_cursor,
            "command": raw_argv,
        });
        let stdout = if spec.json {
            format!("{payload}\n")
        } else if cli == AgentCli::Command {
            format!("{}  command  tab={}  {}\n", entry.id, entry.window_id, title)
        } else {
            format!("{}  {} ({}, effort={})  tab={}  {}\n", entry.id, cli.binary(), model, effort, entry.window_id, title)
        };
        (Response::ok(stdout, payload), minder_lost, leave_res)
    }

    // ---- list ----

    async fn list(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let (mut all, mut fast, mut as_json, mut explicit_parent) = (false, false, false, None::<String>);
        if let Some(args) = args_of(raw) {
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--all" | "-a" => all = true,
                    "--fast" => fast = true,
                    "--json" => as_json = true,
                    "--parent" => {
                        i += 1;
                        explicit_parent = args.get(i).cloned();
                    }
                    s if s.starts_with("--parent=") => explicit_parent = Some(s["--parent=".len()..].to_owned()),
                    _ => {}
                }
                i += 1;
            }
        } else {
            all = raw["all"].as_bool().unwrap_or(false);
            fast = raw["fast"].as_bool().unwrap_or(false);
            as_json = raw["json"].as_bool().unwrap_or(false);
            explicit_parent = raw["parent"].as_str().map(str::to_owned);
        }
        let (sid, caller_win) = match self.place(caller, explicit_parent.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Response::err("list", e, 1),
        };
        let caller_is_child = caller.child.is_some();
        let shared_minder_id = caller.child.as_ref().filter(|c| c.is_shared_minder()).map(|c| c.id.clone());
        let live_windows = self.live_windows(&sid);
        let (mut rows, mut exited) = {
            let st = self.lock();
            let rows = if all && explicit_parent.is_none() && !caller_is_child {
                let mut collected = Vec::new();
                let mut tops: Vec<&String> = live_windows.iter().filter(|w| !st.reg.is_child_window(HOST, &sid, w, &live_windows)).collect();
                tops.sort();
                for w in tops {
                    collected.extend(st.reg.descendants_tree(HOST, &sid, w, &live_windows));
                }
                collected
            } else {
                st.reg.descendants_tree(HOST, &sid, &caller_win, &live_windows)
            };
            let exited = if !all {
                Vec::new()
            } else if caller_is_child {
                // A child sees its own exited descendants alone.
                st.reg.exited_entries(HOST, Some(&sid), None).into_iter().filter(|e| st.reg.is_strict_descendant(HOST, &sid, &caller_win, &e.window_id)).collect()
            } else {
                let mut ex = st.reg.exited_entries(HOST, Some(&sid), explicit_parent.as_ref().map(|_| caller_win.as_str()));
                if ex.is_empty() && explicit_parent.is_none() {
                    // (And those of the server's sessions before it was restarted.)
                    ex = st.reg.exited_entries(HOST, None, None);
                }
                ex
            };
            (rows, exited)
        };
        // (A child whose program ended and whose tab stays is listed among the exited.)
        rows.retain(|r| r.entry.exited_at.is_none());
        // A shared minder's `mind list` shows strictly the agents it minds (never unminded grandchildren).
        if let Some(ref mid) = shared_minder_id {
            rows.retain(|r| r.entry.minded_by.as_deref() == Some(mid.as_str()));
            exited.retain(|e| e.minded_by.as_deref() == Some(mid.as_str()));
        }

        // One look at this host for all of them.
        let inspected = if !fast && !rows.is_empty() {
            let entries: Vec<NativeSubagent> = rows.iter().map(|r| r.entry.clone()).collect();
            let map = self.inspect(&entries).await;
            let st = self.lock();
            for row in &mut rows {
                if let Some(updated) = st.reg.by_window(HOST, &sid, &row.entry.window_id) {
                    row.entry = updated.clone();
                }
            }
            map
        } else {
            HashMap::new()
        };

        let mut items = Vec::with_capacity(rows.len() + exited.len());
        let mut text = String::new();
        for row in &rows {
            let looked = self.look(&row.entry, false).await;
            let (cls, how) = (looked.cls, (looked.last_output_at, looked.reports, looked.expects));
            let info = inspected.get(&row.entry.id);
            let (state, _, _) = super::service::standing(&row.entry, &cls, info, true, how);
            let kids = self.lock().reg.direct_children(HOST, &sid, &row.entry.window_id, &live_windows).len();
            let mut item = summary_json(&row.entry, state, cls.summary.as_deref(), row.depth, kids);
            let minder = self.lock().reg.minder_of(&row.entry.id).map(|m| m.id.clone());
            item["minder"] = json!(minder);
            if shared_minder_id.is_some() {
                let rules_path = self.rules_path(&row.entry.id).display().to_string();
                item["what"] = json!(Self::agent_what(&row.entry));
                item["rules"] = json!(rules_path);
            }
            items.push(item);
            let what = if row.entry.is_minder() {
                format!("minder, {} ({}, effort={})", row.entry.cli.binary(), row.entry.model, row.entry.effort)
            } else if row.entry.cli.is_agent() {
                format!("{} ({}, effort={})", row.entry.cli.binary(), row.entry.model, row.entry.effort)
            } else {
                "command".to_owned()
            };
            let minder_extra = if shared_minder_id.is_some() {
                let cwd = row.entry.cwd.as_deref().map(|c| format!("  cwd={c}")).unwrap_or_default();
                format!("{cwd}  rules={}", self.rules_path(&row.entry.id).display())
            } else {
                String::new()
            };
            text.push_str(&format!(
                "{}{}{}  {}  {}  [{}]{}{}\n",
                row.menu_prefix(),
                row.entry.id,
                row.entry.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
                row.entry.display_title(),
                what,
                state.as_str(),
                cls.summary.as_deref().map(|s| format!(" — {s}")).unwrap_or_default(),
                minder_extra
            ));
        }
        for ex in &exited {
            let mut item = summary_json(ex, SubagentLiveState::Exited, Some("exited"), 1, 0);
            if shared_minder_id.is_some() {
                item["what"] = json!(Self::agent_what(ex));
                item["rules"] = json!(self.rules_path(&ex.id).display().to_string());
            }
            items.push(item);
            let what = if ex.is_minder() {
                format!("minder, {} ({}, effort={})", ex.cli.binary(), ex.model, ex.effort)
            } else if ex.cli.is_agent() {
                format!("{} ({}, effort={})", ex.cli.binary(), ex.model, ex.effort)
            } else {
                "command".to_owned()
            };
            text.push_str(&format!(
                "   {}{}  {}  {}  [exited]{}\n",
                ex.id,
                ex.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
                ex.display_title(),
                what,
                ex.effective_session_id().map(|s| format!("  session={s}")).unwrap_or_default()
            ));
        }
        // (`parent_certain`: the asking tab is the one the asker's own pane
        // is in, not the tab in front taken in its place for a pane that
        // isn't known: what acts on "my children" at once goes by it.)
        let payload = json!({ "ok": true, "session_id": sid, "parent_window_id": caller_win, "parent_certain": caller.window_from_block, "cursor": self.cursor(), "subagents": items });
        let stdout = if as_json {
            format!("{payload}\n")
        } else if text.is_empty() {
            "No child subagent tabs.\n".to_owned()
        } else {
            text
        };
        Response::ok(stdout, payload)
    }

    // ---- status ----

    async fn status(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let (mut target, mut as_json) = (None::<String>, false);
        if let Some(args) = args_of(raw) {
            for a in &args {
                if a == "--json" {
                    as_json = true;
                } else if !a.starts_with('-') && target.is_none() {
                    target = Some(a.clone());
                }
            }
        } else {
            target = raw.get("target").or_else(|| raw.get("id")).and_then(Value::as_str).map(str::to_owned);
            as_json = raw["json"].as_bool().unwrap_or(false);
        }
        let Some(target) = target else {
            return Response { ok: false, stdout: String::new(), stderr: "usage: olo agent status <ID|NAME> [--json]\n".into(), code: 2, data: Value::Null };
        };
        let entry = match self.resolve(caller, &target) {
            Ok(e) => e,
            Err(e) => return Response::err("status", e, 1),
        };
        // For an exited ward inspected by a minder, serve frozen exit
        // information from the registry and recorded event log without
        // consulting live PIDs, terminal screens, or a shared transcript
        // file that the lead may have resumed outside the roster.
        if entry.exited_at.is_some() && caller.child.as_ref().is_some_and(|c| c.is_minder()) {
            let (summary, last_message, error, tail, cursor) = {
                let st = self.lock();
                let ev = st.log.last_state_of(&entry.id);
                let sum = ev.and_then(|e| e.summary.clone()).unwrap_or_else(|| match entry.exit_code {
                    Some(code) => format!("exited (code {code})"),
                    None => "tab closed".into(),
                });
                let last_msg = ev.and_then(|e| e.last_message.clone());
                let err = ev.and_then(|e| e.error.clone());
                let tail = ev.map(|e| e.tail.clone()).unwrap_or_default();
                (sum, last_msg, err, tail, st.log.latest_seq())
            };
            let mut payload = summary_json(&entry, SubagentLiveState::Exited, Some(&summary), 1, 0);
            payload["minder"] = json!(self.lock().reg.minder_of(&entry.id).map(|m| m.id.clone()));
            payload["answers"] = Value::Null;
            payload["running"] = json!(false);
            payload["survey"] = json!(false);
            payload["exit_code"] = json!(entry.exit_code);
            payload["aborted"] = json!(false);
            payload["last_message"] = json!(last_message);
            payload["approval"] = Value::Null;
            payload["error"] = json!(error);
            payload["last_output_at"] = json!(0);
            payload["last_state_change_at"] = json!(0);
            payload["quiet_secs"] = json!(0);
            payload["state_age_secs"] = json!(0);
            payload["background_shells"] = json!([]);
            payload["waiting_on"] = Value::Null;
            payload["cursor"] = json!(cursor);
            payload["tail"] = json!(tail);
            let stdout = if as_json {
                format!("{payload}\n")
            } else {
                let what = if entry.cli.is_agent() { format!("{} ({}, effort={})", entry.cli.binary(), entry.model, entry.effort) } else { "command".to_owned() };
                let mut s = format!("{}  {}  {}  state=exited\nsummary: {summary}\n", entry.id, entry.display_title(), what);
                if let Some(msg) = &last_message {
                    s.push_str(&format!("last_message:\n{msg}\n"));
                }
                s
            };
            return Response::ok(stdout, payload);
        }
        // (The screen before the processes and the conversation file: the
        // missed shell's fix, d796809c2.)
        let looked = self.look(&entry, false).await;
        let inspected = self.inspect(std::slice::from_ref(&entry)).await;
        let info = inspected.get(&entry.id);
        let entry = self.entry(&entry.id).unwrap_or(entry);
        let (cls, screen) = (looked.cls, looked.screen);
        let now = now_unix_ms();
        let since = |at: u64| if at > 0 { now.saturating_sub(at) / 1000 } else { 0 };
        let last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
        let pending = Self::pending_tools(info);
        // (Its end, where it ended while its processes were read: the
        // record read again just above says so, and what its screen
        // showed before does not make it "running" beside its own
        // `exited_at`. A code review, 2026-11-27.)
        let alive = cls.state != SubagentLiveState::Exited && entry.exited_at.is_none();
        let (state, error, aborted) = super::service::standing(&entry, &cls, info, alive, (looked.last_output_at, looked.reports, looked.expects));
        let tail = tail_of(&screen, 8);

        let mut payload = summary_json(&entry, state, cls.summary.as_deref(), 1, 0);
        payload["minder"] = json!(self.lock().reg.minder_of(&entry.id).map(|m| m.id.clone()));
        // (The turn it is in, or last ended: the cursor of what it answers.)
        let (answers, live_pid) = {
            let st = self.lock();
            let live = st.live.get(&entry.id);
            (live.and_then(|l| l.answers), live.and_then(|l| l.pid))
        };
        payload["answers"] = json!(answers);
        if let Some(p) = live_pid.or(entry.last_pid).or(looked.pid) {
            payload["pid"] = json!(p);
        }
        payload["running"] = json!(state != SubagentLiveState::Exited);
        // (A feedback survey that wouldn't be set aside: what is read may be under it.)
        payload["survey"] = json!(looked.survey);
        // (Claude Code's offer to send a feedback report it drafted, where it shows: its line, as drawn.)
        payload["feedback_draft"] = json!(entry.cli.screen_agent().and_then(|agent| olo_agent_screen::feedback_draft(agent, &screen)));
        payload["exit_code"] = json!(entry.exit_code);
        payload["aborted"] = json!(aborted);
        payload["last_message"] = json!(last_message);
        // Label the approval payload strictly with the box captured on THIS
        // `looked` reading — never re-reading live `box_now` after `inspect`!
        let approval_val = cls.approval.as_ref().map(|ap| Self::approval_json_with_box(ap, &pending, looked.boxed.as_ref().map(|b| b.key.as_str())));
        payload["approval"] = json!(approval_val);
        // (A minder that looks is told what stands asked — bound to the exact
        // box instance, current rules generation, and reading sequence — ONLY
        // once any pending `minding` ("rules") revision has been delivered,
        // and NEVER overwriting or clearing a grant minted by a newer reading.)
        if let Some(me) = caller.child.as_ref().filter(|c| c.is_minder()) {
            let mut st = self.lock();
            if Self::minder_owns(&st, me, &entry.id) {
                let pair = (me.id.clone(), entry.id.clone());
                if st.pending_rules_rev.contains_key(&pair) {
                    st.told.remove(&pair);
                    payload["rules_changed"] = json!(true);
                } else if !looked.rpc_failed {
                    let cur_accepted = st.live.get(&entry.id).map_or(0, |l| l.accepted_seq);
                    let newer_grant = st.told.get(&pair).is_some_and(|g| g.read_seq > looked.seq);
                    let stale_read = looked.seq > 0 && looked.seq < cur_accepted;
                    if !newer_grant && !stale_read {
                        let rules_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
                        if let Some(lb) = looked.boxed.as_ref() {
                            if Self::question_suppressed_locked(&st, &entry.id, &cls, Some(lb)) {
                                if st.told.get(&pair).is_some_and(|g| g.token == lb.key) {
                                    st.told.remove(&pair);
                                }
                            } else {
                                st.told.insert(pair, super::service::ToldGrant { token: lb.key.clone(), is_approval: true, rules_gen, read_seq: looked.seq });
                            }
                        } else if cls.approval.is_some() {
                            // Box on this reading was superseded or vanished while `inspect` ran:
                            // do NOT clear a newer box's grant.
                        } else if state == SubagentLiveState::WaitingQuestion
                            && cls.folder_trust.is_none()
                            && !cls.approval_fragment
                            && (cls.recognized_question || cls.state != SubagentLiveState::WaitingQuestion)
                        {
                            let key = super::service::subkey(&cls);
                            let token = if key.is_empty() { "tx:waiting".to_owned() } else { key };
                            st.told.insert(pair, super::service::ToldGrant { token, is_approval: false, rules_gen, read_seq: looked.seq });
                        } else if (looked.live && looked.ordered) || looked.seq >= cur_accepted {
                            st.told.remove(&pair);
                        }
                    }
                }
            }
        }
        // (Why it failed, by its error's own words: `state::error_cause`.
        // Where it says it failed and nothing has the words, its screen
        // may: `olo_agent_screen::error_line`.)
        let error = error_words(state, error, &screen);
        if state == SubagentLiveState::Error {
            let (kind, status) = olo_rex_link::agents::state::error_cause(error.as_deref().unwrap_or_default());
            payload["error_kind"] = json!(kind);
            payload["error_status"] = json!(status);
        }
        payload["error"] = json!(error);
        payload["last_output_at"] = json!(looked.last_output_at);
        payload["last_state_change_at"] = json!(looked.last_state_change_at);
        payload["quiet_secs"] = json!(since(looked.last_output_at));
        payload["state_age_secs"] = json!(since(looked.last_state_change_at));
        payload["background_shells"] = info.map(|v| v["background_shells"].clone()).filter(|v| !v.is_null()).unwrap_or_else(|| json!([]));
        payload["waiting_on"] = json!(info.and_then(|v| v["waiting_on"].as_str()));
        // (Antigravity's own tasks aren't among its processes: its footer
        // says how many go. At rest with one going, it waits on its own
        // run, as an agent with a shell of its own still running does.)
        let own_tasks = if entry.cli == AgentCli::Agy { olo_rex_link::agents::state::agy_tasks(&screen) } else { 0 };
        if own_tasks > 0 {
            payload["own_tasks"] = json!(own_tasks);
            if matches!(state, SubagentLiveState::Idle | SubagentLiveState::Done) {
                payload["waiting_on"] = json!("own_run");
            }
        }
        payload["usage"] = json!(cls.usage.as_ref().map(|u| json!({ "line": u.line, "percent": u.percent, "limit_reached": u.limit_reached })));
        payload["cursor"] = json!(self.cursor());
        payload["tail"] = json!(tail);

        let stdout = if as_json {
            format!("{payload}\n")
        } else {
            let what = if entry.cli.is_agent() { format!("{} ({}, effort={})", entry.cli.binary(), entry.model, entry.effort) } else { "command".to_owned() };
            let mut s = format!("{}  {}  {}  state={}\n", entry.id, entry.display_title(), what, state.as_str());
            if let Some(sid) = entry.effective_session_id() {
                s.push_str(&format!("session: {sid}\n"));
            }
            if let Some(sum) = &cls.summary {
                s.push_str(&format!("summary: {sum}\n"));
            }
            if let Some(err) = &error {
                s.push_str(&format!("error: {err}\n"));
            }
            if let Some(msg) = &last_message {
                s.push_str(&format!("last_message:\n{msg}\n"));
            }
            for l in &tail {
                s.push_str(&format!("  {l}\n"));
            }
            s
        };
        Response::ok(stdout, payload)
    }

    // ---- read ----

    async fn read(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let (mut target, mut lines, mut scrollback, mut ansi, mut as_json) = (None::<String>, 80usize, false, false, false);
        if let Some(args) = args_of(raw) {
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--scrollback" | "--all" => scrollback = true,
                    "--ansi" => ansi = true,
                    "--json" => as_json = true,
                    "-n" | "--lines" => {
                        i += 1;
                        if let Some(n) = args.get(i).and_then(|s| s.parse::<usize>().ok()) {
                            lines = n.clamp(1, 5000);
                        }
                    }
                    s if s.starts_with("--lines=") => {
                        if let Ok(n) = s["--lines=".len()..].parse::<usize>() {
                            lines = n.clamp(1, 5000);
                        }
                    }
                    s if !s.starts_with('-') && target.is_none() => target = Some(s.to_owned()),
                    _ => {}
                }
                i += 1;
            }
        } else {
            target = raw.get("target").or_else(|| raw.get("id")).and_then(Value::as_str).map(str::to_owned);
            lines = raw["lines"].as_u64().unwrap_or(80).clamp(1, 5000) as usize;
            scrollback = raw["scrollback"].as_bool().unwrap_or(false);
            ansi = raw["ansi"].as_bool().unwrap_or(false);
            as_json = raw["json"].as_bool().unwrap_or(false);
        }
        let Some(target) = target else {
            return Response { ok: false, stdout: String::new(), stderr: "usage: olo agent read <ID|NAME> [--lines N] [--scrollback] [--ansi] [--json]\n".into(), code: 2, data: Value::Null };
        };
        let entry = match self.resolve(caller, &target) {
            Ok(e) => e,
            Err(e) => return Response::err("read", e, 1),
        };
        if entry.exited_at.is_some() && caller.child.as_ref().is_some_and(|c| c.is_minder()) {
            let (summary, tail) = {
                let st = self.lock();
                let ev = st.log.last_state_of(&entry.id);
                (ev.and_then(|e| e.summary.clone()).unwrap_or_else(|| "exited".into()), ev.map(|e| e.tail.clone()).unwrap_or_default())
            };
            let mut text = tail.join("\n");
            if !text.ends_with('\n') && !text.is_empty() {
                text.push('\n');
            }
            let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "state": "exited", "summary": summary, "output": text });
            return Response::ok(if as_json { format!("{payload}\n") } else { text }, payload);
        }
        let looked = self.look(&entry, true).await;
        // (All it has, colours too where asked; else what it shows now.)
        let mut text = if scrollback || ansi { self.rex.scrollback(&entry.session_id, &entry.block_id, if ansi { "vt" } else { "text" }).await.unwrap_or_default() } else { looked.screen.clone() };
        let all: Vec<&str> = text.lines().collect();
        if all.len() > lines {
            text = all[all.len() - lines..].join("\n");
        }
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "state": looked.cls.state.as_str(), "summary": looked.cls.summary, "output": text });
        Response::ok(if as_json { format!("{payload}\n") } else { text }, payload)
    }

    // ---- watch ----

    async fn watch(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let mut spec = match WatchSpec::parse(Some(raw)) {
            Ok(s) => s,
            Err(e) => return Response::err("watch", e, 2),
        };
        // (All of a minder's is the one agent it minds.)
        if spec.all
            && let Some(minded) = caller.child.as_ref().and_then(|c| c.minds.clone())
        {
            spec.all = false;
            spec.targets = vec![minded];
        }
        let regex = match spec.regex_pat.as_deref().map(regex::Regex::new) {
            Some(Ok(re)) => Some(re),
            Some(Err(e)) => return Response::err("watch", format!("invalid --regex `{}`: {e}", spec.regex_pat.as_deref().unwrap_or_default()), 2),
            None => None,
        };
        let (scope_sid, scope_parent, explicit): (Option<String>, Option<String>, Vec<NativeSubagent>) = if spec.all {
            match self.place(caller, spec.parent.as_deref()).await {
                Ok((sid, parent)) => (Some(sid), Some(parent), Vec::new()),
                Err(e) => return Response::err("watch", e, 1),
            }
        } else {
            let mut entries = Vec::with_capacity(spec.targets.len());
            for t in &spec.targets {
                match self.resolve(caller, t) {
                    Ok(e) => entries.push(e),
                    Err(e) => return Response::err("watch", e, 1),
                }
            }
            (caller.session.clone(), None, entries)
        };
        let explicit_ids: HashSet<String> = explicit.iter().map(|e| e.id.clone()).collect();
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(spec.timeout_secs);

        // A cursor from before the log began again, or older than what
        // the log still holds: said, never silently wrong.
        let (since_floor, had_gap) = {
            let st = self.lock();
            let latest = st.log.latest_seq();
            match spec.since {
                Some(since) if since > latest => {
                    let payload = json!({ "ok": true, "event": "reset", "cursor": latest, "requested_since": since, "elapsed_ms": started.elapsed().as_millis() as u64 });
                    let stdout = if spec.json { format!("{payload}\n") } else { format!("cursor reset (requested={since}, current={latest})\n") };
                    return Response::ok(stdout, payload);
                }
                Some(since) => (since, st.log.has_gap(since)),
                None => (latest, false),
            }
        };
        // (The asker, where it is a minder of either kind.)
        let minding = caller.child.as_ref().filter(|c| c.is_minder()).cloned();
        let in_scope_now = || -> Vec<NativeSubagent> {
            // A minder's scope is strictly the live agents it currently minds,
            // never unminded grandchildren or descendants of its workers.
            if let Some(me) = &minding {
                let st = self.lock();
                if !st.reg.entries.iter().rev().any(|e| e.id == me.id && e.exited_at.is_none()) {
                    return Vec::new();
                }
                if me.is_shared_minder() {
                    if spec.all {
                        return st.reg.minded_by(&me.id).into_iter().cloned().collect();
                    }
                    return explicit
                        .iter()
                        .filter_map(|e| st.reg.entries.iter().rev().find(|r| r.id == e.id && r.exited_at.is_none() && r.minded_by.as_deref() == Some(me.id.as_str())).cloned())
                        .collect();
                }
                return explicit.iter().filter_map(|e| st.reg.entries.iter().rev().find(|r| r.id == e.id && r.exited_at.is_none() && Self::minder_owns(&st, me, &r.id)).cloned()).collect();
            }
            if !spec.all {
                return explicit.iter().filter_map(|e| self.entry(&e.id)).collect();
            }
            let (Some(sid), Some(parent)) = (scope_sid.as_deref(), scope_parent.as_deref()) else { return Vec::new() };
            let live = self.live_windows(sid);
            self.lock().reg.descendants_tree(HOST, sid, parent, &live).into_iter().map(|r| r.entry).filter(|e| e.exited_at.is_none()).collect()
        };
        let lost_named_target = |me: &NativeSubagent| -> Option<Response> {
            if spec.all {
                return None;
            }
            let st = self.lock();
            for e in &explicit {
                if !Self::minder_owns(&st, me, &e.id) {
                    let who = e.name.as_deref().unwrap_or(&e.id);
                    return Some(Response::err("watch", format!("`{who}` is no longer minded by you"), 1));
                }
            }
            None
        };

        // For `--match` and `--regex`: what is on each screen now is not
        // what is waited for (a prompt's own echo never fires).
        let check_text = spec.match_text.is_some() || regex.is_some();
        struct Watching<'a>(&'a std::sync::atomic::AtomicUsize, bool);
        impl Drop for Watching<'_> {
            fn drop(&mut self) {
                if self.1 {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        if check_text {
            self.text_watchers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let _watching = Watching(&self.text_watchers, check_text);
        let mut baseline_lines: HashMap<String, HashSet<String>> = HashMap::new();
        // Without `--since`, a child already at rest when this was asked
        // (and looking as it did) is not what is waited for: `send … &&
        // watch` must not answer with what stood before the sending.
        let mut baseline_idle: HashMap<String, u64> = HashMap::new();
        if check_text || spec.since.is_none() {
            for e in &in_scope_now() {
                let looked = self.look(e, false).await;
                if check_text {
                    baseline_lines.insert(e.id.clone(), looked.screen.lines().map(str::trim).map(str::to_owned).collect());
                }
                let only_done = spec.events.len() == 1 && spec.events[0] == "done";
                if spec.since.is_none() && looked.cls.state == SubagentLiveState::Idle && !only_done {
                    baseline_idle.insert(e.id.clone(), olo_agent_screen::fingerprint(&looked.screen));
                }
            }
        }

        let answer = |ev: &SubagentEvent, matched_line: Option<String>, gap: bool| {
            let mut payload = json!({
                "ok": true,
                "cursor": ev.seq,
                "id": ev.id,
                "name": ev.name,
                "window_id": ev.window_id,
                "block_id": ev.block_id,
                "resume_id": ev.resume_id,
                "cli_session_id": ev.cli_session_id,
                "event": ev.event,
                "state": ev.state.as_str(),
                "summary": ev.summary,
                "last_message": ev.last_message,
                "approval": ev.approval,
                "error": ev.error,
                // (The turn it is of: the cursor of the `start` or `send` it answers.)
                "answers": ev.answers,
                "matched_line": matched_line,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "tail": ev.tail,
            });
            if let Some(m) = &ev.minding {
                payload["minding"] = m.clone();
            }
            // (Why it failed, by its error's own words: `state::error_cause`.)
            // (From the error's words alone: what it had last been doing
            // is no error's words, and a number in it no status.)
            let why = (ev.state == SubagentLiveState::Error).then(|| olo_rex_link::agents::state::error_cause(ev.error.as_deref().unwrap_or_default()));
            if let Some((kind, status)) = why {
                payload["error_kind"] = json!(kind);
                payload["error_status"] = json!(status);
            }
            if gap {
                payload["gap"] = json!(true);
            }
            let stdout = if spec.json {
                format!("{payload}\n")
            } else {
                // (A failure by its own words and whose fault it was, where it has them.)
                let said = match (&why, &ev.error) {
                    (Some((kind, status)), Some(words)) => Some(format!("{kind}{}: {words}", status.map(|s| format!(" {s}")).unwrap_or_default())),
                    _ => None,
                };
                format!("{}  event={}  state={}  cursor={}  {}\n", ev.id, ev.event, ev.state.as_str(), ev.seq, matched_line.or(said).or_else(|| ev.summary.clone()).unwrap_or_default())
            };
            Response::ok(stdout, payload)
        };
        let event_in_scope = |ev: &SubagentEvent, live_ids: &HashSet<String>| -> bool {
            if !spec.all {
                return explicit_ids.contains(&ev.id);
            }
            if scope_sid.as_deref().is_some_and(|sid| ev.session_id != sid) {
                return false;
            }
            // (An exited grandchild is under the tab watched by its ancestors.)
            live_ids.contains(&ev.id) || scope_parent.as_deref().is_some_and(|p| ev.parent_window_id == p || ev.ancestor_windows.iter().any(|a| a == p))
        };
        let wants_done = spec.events.iter().any(|e| e == "done");

        loop {
            let changed = self.changed.notified();
            if let Some(me) = &minding
                && let Some(ended) = lost_named_target(me)
            {
                return ended;
            }
            let current = in_scope_now();
            let live_ids: HashSet<String> = current.iter().map(|e| e.id.clone()).collect();

            // 1. What was written down after the cursor: nothing between
            //    one watch and the next is lost.
            // (What a minder has for the parent is an event of its own
            // kind; what a minded child asks is its minder's to answer,
            // and wakes no other watch while that minder runs. Left in
            // the log: with the minder gone it is told from here.)
            //
            // A minder is told what stands of each agent it minds,
            // once: the last thing written of it, where the agent still
            // stands so and this minder hasn't been told it. Not by a
            // cursor at all. A cursor says what the asker has been
            // told, and a minder that has only just begun has been told
            // nothing: one attached to an agent already asking, or put
            // in another's place while it asked, was given the cursor
            // of that moment and waited on a question that stood before
            // it for as long as it stood (the ShiftUp lead's trial,
            // 2026-11-27; Ben, watching it: "we shouldn't have to wait
            // for the first watch to timeout it should see any
            // outstanding questions when it connects"). And one that
            // took a while over the last question is not told, one by
            // one, of questions long answered or gone (a real one
            // reported "it is waiting" of an agent that had exited,
            // 2026-11-26). For a shared minder, roster/rules updates
            // (`minding` events) are delivered first, in order, once
            // each, so it reads each agent's rules before judging it.
            let retiring_handover = || Response { ok: false, stdout: String::new(), stderr: String::new(), code: 75, data: json!({ "handover": true }) };
            if let Some(me) = &minding {
                if me.is_shared_minder() && spec.wants_minding() {
                    let roster_ev = {
                        let mut st = self.lock();
                        if st.retiring || st.frozen {
                            return retiring_handover();
                        }
                        // Track `minding` delivery per `(minder_id, subject_id)` so a
                        // targeted watch on B never swallows an unread rules event for A.
                        let found = st.log.find_since(0, |ev| {
                            ev.event == minder::MINDING
                                && ev.minding.as_ref().and_then(|m| m["minder_id"].as_str()) == Some(me.id.as_str())
                                && (spec.all || explicit_ids.contains(&ev.id))
                                && ev.seq > st.minding_seen.get(&(me.id.clone(), ev.id.clone())).copied().unwrap_or(0)
                        });
                        if let Some(ev) = found {
                            let pair = (me.id.clone(), ev.id.clone());
                            st.minding_seen.insert(pair.clone(), ev.seq);
                            let action = ev.minding.as_ref().and_then(|m| m["action"].as_str());
                            if matches!(action, Some("rules" | "added"))
                                && let Some(&(req_gen, _)) = st.pending_rules_rev.get(&pair)
                            {
                                let ev_gen = ev.minding.as_ref().and_then(|m| m["rules_gen"].as_u64());
                                let cur_gen = st.rules_gen.get(&ev.id).copied().unwrap_or(0);
                                if ev_gen.is_some_and(|g| g >= req_gen) || (ev_gen.is_none() && cur_gen == req_gen) {
                                    st.pending_rules_rev.remove(&pair);
                                }
                            }
                            Some(ev)
                        } else {
                            None
                        }
                    };
                    if let Some(ev) = roster_ev {
                        self.saved();
                        return answer(&ev, None, false);
                    }
                    // If the 2,000-event log ring evicted an unread `rules` revision
                    // (or handover restored `pending_rules_rev` without the ring entry),
                    // synthesize and deliver it atomically under ONE lock hold (never
                    // appending to `events.jsonl` while retiring!).
                    let synthesized_ev: Option<SubagentEvent> = {
                        let mut st = self.lock();
                        if st.retiring || st.frozen {
                            return retiring_handover();
                        }
                        let found = st.pending_rules_rev.iter().find(|((mid, aid), _)| mid == &me.id && (spec.all || explicit_ids.contains(aid)) && Self::minder_owns(&st, me, aid)).and_then(
                            |((_, aid), (rev_gen, rpath))| st.reg.entries.iter().rev().find(|e| e.id == *aid && e.exited_at.is_none()).cloned().map(|agent| (agent, *rev_gen, PathBuf::from(rpath))),
                        );
                        if let Some((agent, req_gen, rpath)) = found {
                            let state = st.live.get(&agent.id).and_then(|l| l.last_state).unwrap_or(SubagentLiveState::Working);
                            let rules_gen = st.rules_gen.get(&agent.id).copied().unwrap_or(0);
                            let ancestors = st.reg.ancestor_windows(HOST, &agent.session_id, &agent.window_id);
                            let answers = st.live.get(&agent.id).and_then(|l| l.answers);
                            let its_name = agent.name.as_deref().unwrap_or(&agent.id);
                            let what = Self::agent_what(&agent);
                            let summary = format!("Rules changed for \"{its_name}\". Re-read its rules at {}", rpath.display());
                            let mut event = SubagentEvent {
                                seq: 0,
                                timestamp_ms: now_unix_ms(),
                                host_key: HOST.to_owned(),
                                session_id: agent.session_id.clone(),
                                parent_window_id: me.window_id.clone(),
                                ancestor_windows: ancestors,
                                id: agent.id.clone(),
                                name: agent.name.clone(),
                                window_id: agent.window_id.clone(),
                                block_id: agent.block_id.clone(),
                                resume_id: agent.resume_id.clone(),
                                cli_session_id: agent.effective_session_id().map(str::to_owned),
                                event: minder::MINDING.to_owned(),
                                state,
                                summary: Some(summary),
                                last_message: None,
                                approval: None,
                                error: None,
                                tail: Vec::new(),
                                dedupe_key: String::new(),
                                answers,
                                minding: Some(json!({
                                    "action": "rules",
                                    "minder_id": me.id,
                                    "rules": rpath.display().to_string(),
                                    "rules_gen": rules_gen,
                                    "what": what,
                                    "cwd": agent.cwd,
                                })),
                            };
                            let seq = st.log.push_beside(event.clone());
                            event.seq = seq;
                            st.reg.event_seq = seq;
                            if let Ok(mut line) = serde_json::to_string(&event) {
                                use std::io::Write as _;
                                line.push('\n');
                                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.paths.events) {
                                    let _ = f.write_all(line.as_bytes());
                                }
                            }
                            let pair = (me.id.clone(), agent.id.clone());
                            st.minding_seen.insert(pair.clone(), seq);
                            if st.pending_rules_rev.get(&pair).is_some_and(|(g, _)| *g <= req_gen) {
                                st.pending_rules_rev.remove(&pair);
                            }
                            Some(event)
                        } else {
                            None
                        }
                    };
                    if let Some(ev) = synthesized_ev {
                        self.saved();
                        self.changed.notify_waiters();
                        return answer(&ev, None, false);
                    }
                }
                let candidates: Vec<(NativeSubagent, SubagentEvent)> = {
                    let st = self.lock();
                    let agents: Vec<NativeSubagent> = if me.is_shared_minder() {
                        if spec.all {
                            st.reg.all_minded_by(&me.id).into_iter().cloned().collect()
                        } else {
                            explicit.iter().filter_map(|e| st.reg.entries.iter().rev().find(|r| r.id == e.id && r.minded_by.as_deref() == Some(me.id.as_str())).cloned()).collect()
                        }
                    } else {
                        current.first().cloned().or_else(|| explicit.first().cloned()).into_iter().collect()
                    };
                    let mut ready: Vec<(NativeSubagent, SubagentEvent)> = agents
                        .into_iter()
                        .filter_map(|agent| {
                            let pair = (me.id.clone(), agent.id.clone());
                            // Never deliver a state event or mint a grant for an agent whose
                            // updated `minding` ("rules") revision has not been delivered yet.
                            if me.is_shared_minder() && st.pending_rules_rev.contains_key(&pair) {
                                return None;
                            }
                            let told_to = st.seen.get(&pair).copied().unwrap_or(0);
                            // Check BOTH the log ring and `live.latest_event` so a standing
                            // question is never lost after 2,000 newer host events evict the ring.
                            let from_log = st.log.last_state_of(&agent.id);
                            let from_live = st.live.get(&agent.id).and_then(|l| l.latest_event.as_ref());
                            let latest = match (from_log, from_live) {
                                (Some(a), Some(b)) => {
                                    if a.seq >= b.seq {
                                        Some(a)
                                    } else {
                                        Some(b)
                                    }
                                }
                                (a, b) => a.or(b),
                            };
                            let last = latest.filter(|last| last.seq > told_to && spec.matches_state(last.state))?.clone();
                            Some((agent, last))
                        })
                        .collect();
                    ready.sort_by_key(|(_, ev)| ev.seq);
                    ready
                };
                for (agent, last) in candidates {
                    let (holds, live_looked) = if agent.exited_at.is_some() {
                        (true, None)
                    } else {
                        let looked = self.look(&agent, false).await;
                        if looked.rpc_failed {
                            (false, None)
                        } else {
                            let bk = looked.boxed.as_ref().filter(|b| !b.answered).map(|b| b.key.as_str());
                            let ok = if still_stands(&last, &looked.cls, bk) {
                                true
                            } else if last.state == SubagentLiveState::WaitingQuestion && matches!(looked.cls.state, SubagentLiveState::Idle | SubagentLiveState::Working) {
                                let inspected = self.inspect(std::slice::from_ref(&agent)).await;
                                let info = inspected.get(&agent.id);
                                let (eff, _, _) = effective_state(&looked.cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true);
                                eff == SubagentLiveState::WaitingQuestion
                            } else {
                                false
                            };
                            (ok, Some(looked))
                        }
                    };
                    if !holds {
                        continue;
                    }
                    let mut st = self.lock();
                    if st.retiring || st.frozen {
                        return retiring_handover();
                    }
                    if !Self::minder_owns(&st, me, &agent.id) {
                        drop(st);
                        if let Some(ended) = lost_named_target(me) {
                            return ended;
                        }
                        continue;
                    }
                    let pair = (me.id.clone(), agent.id.clone());
                    if me.is_shared_minder() && st.pending_rules_rev.contains_key(&pair) {
                        st.told.remove(&pair);
                        continue;
                    }
                    let told_to = st.seen.get(&pair).copied().unwrap_or(0);
                    if last.seq <= told_to {
                        continue;
                    }
                    if let Some(ref ll) = live_looked
                        && Self::question_suppressed_locked(&st, &agent.id, &ll.cls, ll.boxed.as_ref())
                    {
                        continue;
                    }
                    let rules_gen = st.rules_gen.get(&agent.id).copied().unwrap_or(0);
                    let read_seq = live_looked.as_ref().map_or(0, |ll| ll.seq);
                    // (What it asks, bound to its exact unanswered box instance and
                    // current rules generation: never grant an answered box, never
                    // relabel box 1's stale payload with box 2's key, and never grant
                    // for folder_trust or unrecognised Blocked dialogs.)
                    match last.state {
                        SubagentLiveState::WaitingApproval => {
                            let ev_box = last.approval.as_ref().and_then(|a| a["box"].as_str());
                            let live_box = live_looked.as_ref().and_then(|ll| ll.boxed.as_ref()).filter(|b| !b.answered);
                            let Some(lb) = live_box else {
                                drop(st);
                                self.poke(&agent.id);
                                continue;
                            };
                            if let Some(eb) = ev_box
                                && lb.key != eb
                            {
                                drop(st);
                                self.poke(&agent.id);
                                continue;
                            }
                            if Self::question_suppressed_locked(&st, &agent.id, &live_looked.as_ref().unwrap().cls, Some(lb)) {
                                continue;
                            }
                            st.told.insert(pair.clone(), super::service::ToldGrant { token: lb.key.clone(), is_approval: true, rules_gen, read_seq });
                        }
                        SubagentLiveState::WaitingQuestion => {
                            let allow_q = live_looked
                                .as_ref()
                                .is_none_or(|ll| ll.cls.folder_trust.is_none() && !ll.cls.approval_fragment && (ll.cls.recognized_question || ll.cls.state != SubagentLiveState::WaitingQuestion));
                            if allow_q {
                                let tok = if last.dedupe_key.is_empty() { "tx:waiting".to_owned() } else { last.dedupe_key.clone() };
                                st.told.insert(pair.clone(), super::service::ToldGrant { token: tok, is_approval: false, rules_gen, read_seq });
                            } else {
                                st.told.remove(&pair);
                            }
                        }
                        _ => {
                            st.told.remove(&pair);
                        }
                    };
                    st.seen.insert(pair, last.seq);
                    drop(st);
                    self.saved();
                    return answer(&last, None, false);
                }
            } else {
                let queued = {
                    let st = self.lock();
                    st.log.find_since(since_floor, |ev| {
                        // (`minding` events are strictly for the shared minder they target.)
                        ev.event != minder::MINDING
                            && event_in_scope(ev, &live_ids)
                            && if ev.event == minder::ESCALATION { spec.wants_escalations() } else { spec.matches_state(ev.state) && !Self::minders_own(&st.reg, caller, &ev.id, ev.state) }
                    })
                };
                if let Some(ev) = queued {
                    return answer(&ev, None, had_gap);
                }
            }

            // 2. The screens themselves: for text, and (without a cursor)
            //    for a state that already stands.
            // (A minder's is answered above, by what stands.)
            let check_live = spec.since.is_none() && minding.is_none();
            if check_live || check_text {
                let mut turned_down: std::collections::HashSet<String> = std::collections::HashSet::new();
                'scan: loop {
                    let mut candidate: Option<(NativeSubagent, ScreenClassification, String, Option<String>)> = None;
                    for entry in current.iter().filter(|e| !turned_down.contains(&e.id)) {
                        let looked = self.look(entry, false).await;
                        if looked.rpc_failed {
                            continue;
                        }
                        let (cls, screen) = (looked.cls, looked.screen);
                        if check_text {
                            let base = baseline_lines.entry(entry.id.clone()).or_default();
                            let new_lines: Vec<String> = screen.lines().map(str::trim).filter(|l| !l.is_empty() && !base.contains(*l)).map(str::to_owned).collect();
                            let is_match = regex.as_ref().map(|re| move |l: &str| re.is_match(l));
                            if let Some((name, line)) = spec.check_new_lines(&new_lines, is_match.as_ref().map(|f| f as &dyn Fn(&str) -> bool)) {
                                candidate = Some((entry.clone(), cls, name, line));
                                break;
                            }
                        }
                        if check_live {
                            if cls.state == SubagentLiveState::Idle && baseline_idle.get(&entry.id).is_some_and(|fp| *fp == olo_agent_screen::fingerprint(&screen)) {
                                continue;
                            }
                            if cls.state != SubagentLiveState::Idle {
                                baseline_idle.remove(&entry.id);
                            }
                            // Suppress both answered approval boxes AND startup folder_trust
                            // while the service is answering them.
                            let suppressed = {
                                let st = self.lock();
                                Self::question_suppressed_locked(&st, &entry.id, &cls, looked.boxed.as_ref()) || Self::minders_own(&st.reg, caller, &entry.id, cls.state)
                            };
                            if spec.matches_state(cls.state) && !suppressed {
                                let name = cls.state.as_str().to_owned();
                                candidate = Some((entry.clone(), cls, name, None));
                                break;
                            }
                            // `--on done` of a child at rest: its transcript says whether its turn is over.
                            if wants_done && cls.state == SubagentLiveState::Idle {
                                let inspected = self.inspect(std::slice::from_ref(entry)).await;
                                let info = inspected.get(&entry.id);
                                let (eff, _, _) = effective_state(&cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true);
                                if spec.matches_state(eff) {
                                    candidate = Some((entry.clone(), cls, eff.as_str().to_owned(), None));
                                    break;
                                }
                            }
                        }
                    }
                    let Some((entry, first, name, matched_line)) = candidate else { break 'scan };
                    let turned = entry.id.clone();
                    {
                        let is_text = name == "match" || name == "regex";
                        // (A state must hold on a fresh ordered read and remain unsuppressed; text that came is there.)
                        let confirmed_now = if first.state == SubagentLiveState::Exited || is_text {
                            Some(self.look(&entry, false).await)
                        } else {
                            tokio::time::sleep(super::service::CONFIRM).await;
                            let again = self.look(&entry, true).await;
                            let ok = again.is_fresh_ordered() && again.cls.state == first.state && !Self::question_suppressed_locked(&self.lock(), &entry.id, &again.cls, again.boxed.as_ref());
                            ok.then_some(again)
                        };
                        if let Some(now) = confirmed_now {
                            let inspected = self.inspect(std::slice::from_ref(&entry)).await;
                            if let Some(me) = &minding {
                                if let Some(ended) = lost_named_target(me) {
                                    return ended;
                                }
                                if !Self::minder_owns(&self.lock(), me, &entry.id) {
                                    turned_down.insert(turned);
                                    continue 'scan;
                                }
                            }
                            let info = inspected.get(&entry.id);
                            let entry = self.entry(&entry.id).unwrap_or(entry);
                            // Re-check suppression right before returning (covers an approval or
                            // startup trust answered while `inspect` ran).
                            if !is_text && Self::question_suppressed_locked(&self.lock(), &entry.id, &now.cls, now.boxed.as_ref()) {
                                turned_down.insert(turned);
                                continue 'scan;
                            }
                            let pending = Self::pending_tools(info);
                            let (state, error, _) = super::service::standing(&entry, &now.cls, info, now.cls.state != SubagentLiveState::Exited, (now.last_output_at, now.reports, now.expects));
                            let in_turn_yet = !is_text && first.state == SubagentLiveState::Idle && state == SubagentLiveState::Working;
                            let not_asked = !is_text && !spec.matches_state(state);
                            if !in_turn_yet && !not_asked {
                                let mut ev = self.event_of(&entry, state, now.cls.summary.clone(), String::new());
                                ev.seq = self.cursor();
                                if is_text {
                                    ev.event = name;
                                }
                                ev.last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
                                ev.approval = now.cls.approval.as_ref().map(|ap| Self::approval_json_with_box(ap, &pending, now.boxed.as_ref().map(|b| b.key.as_str())));
                                ev.error = error_words(state, error, &now.screen);
                                ev.tail = tail_of(&now.screen, 10);
                                return answer(&ev, matched_line, had_gap);
                            }
                        }
                    }
                    turned_down.insert(turned);
                }
            }

            tokio::select! {
                _ = changed => {}
                _ = tokio::time::sleep_until(deadline) => {
                    let payload = json!({ "ok": false, "cursor": self.cursor(), "event": "timeout", "elapsed_ms": started.elapsed().as_millis() as u64, "code": 124 });
                    return Response {
                        ok: false,
                        stdout: if spec.json { format!("{payload}\n") } else { String::new() },
                        stderr: format!("olo agent watch: timed out after {}s\n", spec.timeout_secs),
                        code: 124,
                        data: payload,
                    };
                }
            }
        }
    }

    // ---- send ----

    async fn bracketed_paste(&self, entry: &NativeSubagent) -> bool {
        self.rex
            .terminal(&entry.session_id, &entry.block_id, "modes", json!({}))
            .await
            .is_ok_and(|m| m["modes"].as_array().into_iter().flatten().any(|mode| mode["name"] == "bracketed_paste" && mode["set"].as_bool().unwrap_or(false)))
    }

    async fn send(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("send", msg, code);
        let spec = match parse_send_payload(&args_of(raw).unwrap_or_default(), Some(raw)) {
            Ok(v) => v,
            Err(e) => return err(e, 2),
        };
        let entry = match self.resolve(caller, &spec.target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        let as_minder = caller.child.as_ref().filter(|c| c.is_minder());
        // Service-side argument check for minders (enforced regardless of
        // whether the caller went through `mind` or spoke to the socket):
        // reject `--force`, `--wait` (both in args and via `Request::wait`),
        // `--ctrl`/control chords, bare character keys, and any key outside
        // plain chooser navigation.
        if as_minder.is_some() {
            if spec.force {
                return err("a minder may not use `--force`".into(), 69);
            }
            if spec.wait || req.wait {
                return err("a minder may not use `--wait`".into(), 69);
            }
            if spec.has_ctrl {
                return err("a minder may not send control keys (`--ctrl`, `Ctrl+…`, `Alt+…`)".into(), 69);
            }
            if spec.types {
                return err("a minder may not press bare character keys (`--key`); send text with `--enter` or use `approve`".into(), 69);
            }
            for k in &spec.raw_keys {
                if !olo_rex_link::agents::is_minder_chooser_key(k) {
                    return err(format!("a minder may not send `--key {k}` (only chooser keys: Up, Down, Left, Right, Tab, Shift+Tab, Space, Backspace, Enter, Escape)"), 69);
                }
            }
        }
        self.minder_did(caller, &entry.id, "send", raw.get("args").cloned().unwrap_or(Value::Null));
        let body = sanitize_paste_text(&spec.body);
        // Query bracketed-paste mode BEFORE reading the screen so no intervening
        // RPC await sits between the screen/survey gate and writing into the PTY.
        let use_paste = if !body.is_empty() { self.bracketed_paste(&entry).await } else { false };
        let mut before = self.look(&entry, true).await;
        // If another reader owns an active feedback-survey dismissal (`Claimed`
        // or `Submitted`), wait briefly on `survey_notify` (zero polling) so we
        // never type text or `--key 2` into an active survey.
        if entry.cli.is_agent() && before.survey_pending {
            let wait_until = tokio::time::Instant::now() + Duration::from_millis(1200);
            if let Some((_, _, survey_n)) = self.child_notifies(&entry.id) {
                loop {
                    let sw = survey_n.notified();
                    tokio::pin!(sw);
                    sw.as_mut().enable();
                    if !self.survey_pending(&entry.id) {
                        break;
                    }
                    if tokio::time::Instant::now() >= wait_until {
                        break;
                    }
                    tokio::select! {
                        _ = sw => {}
                        _ = tokio::time::sleep_until(wait_until) => break,
                    }
                }
            }
            before = self.look(&entry, true).await;
            if before.survey_pending {
                return err(format!("`{}` has a feedback survey dismissal pending: nothing was sent; try again in a moment", entry.id), 1);
            }
        }
        let out_at_before = before.pre_read_out_seq;
        let mut had_intervening_await = false;
        // What is typed into an agent is typed by its screen as read now, in order.
        if entry.cli.is_agent() && !before.is_fresh_ordered() && before.cls.state != SubagentLiveState::Exited {
            return err(format!("`{}`'s screen couldn't be read reliably just now: nothing was sent; try again", entry.id), 1);
        }
        let only_its_zero = spec.body.is_empty() && !spec.enter && !spec.has_ctrl && spec.post_keys.len() == 1 && spec.raw_keys.len() == 1 && spec.raw_keys[0].trim() == "0";
        // Never type text or rating keys (`--key 1..3`) into a staying or partial
        // feedback survey (only a non-minder's explicit `--key 0` or `--force` may pass).
        if entry.cli.is_agent() && before.survey && !spec.force && (!only_its_zero || as_minder.is_some()) {
            return err(format!("refusing to send to `{0}` while a feedback survey is on screen: nothing was sent (`olo agent send {0} --key 0` sets it aside)", entry.id), 1);
        }
        // Never into Claude Code's offer to send a feedback report it has
        // drafted ("1 to review · 2 to send · 0 to dismiss"): a message
        // that begins "2" would send the report. Its own `0` is the one
        // key pressed there, alone and asked for by name, by whoever
        // looks after the child and never by the service; anything else
        // waits until it has gone, `--force` or not. (The ShiftUp lead's
        // item 48, 2026-11-27. Ben's rule there: "no feedback report is
        // ever sent, and nothing answers that box by itself".) Decided
        // before anything else is: a question said to stand under it is
        // not what the key would answer. A minder, who may press no
        // character key, is refused the same: the lead is told of the
        // box on its own watch (`Service::feedback_draft_seen`).
        if let Some(agent) = entry.cli.screen_agent()
            && let Some(shows) = olo_agent_screen::feedback_draft(agent, &before.screen)
        {
            if !only_its_zero || as_minder.is_some() {
                return err(
                    format!(
                        "refusing to send to `{0}` while Claude Code's feedback draft shows ({shows}): what is typed there would answer it. `olo agent send {0} --key 0` sets it aside; then send again",
                        entry.id
                    ),
                    1,
                );
            }
            // (Its 0, and nothing of the checks a question's answer has: there is none under this key.)
            return match self.rex.write(&entry.session_id, &entry.block_id, "0").await {
                Ok(()) => {
                    let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "sent": "0", "feedback_draft": "set aside", "cursor": self.cursor() });
                    Response::ok(if spec.as_json { format!("{payload}\n") } else { format!("{}: its feedback draft is set aside\n", entry.id) }, payload)
                }
                Err(e) => err(format!("couldn't press 0 in `{}`: {e}", entry.id), 1),
            };
        }
        if before.cls.state == SubagentLiveState::Exited {
            return err(format!("subagent `{}` has already exited", entry.id), 1);
        }
        let mut minder_send_gen: Option<u64> = None;
        if let Some(me) = as_minder {
            if !Self::minder_owns(&self.lock(), me, &entry.id) {
                return err(format!("`{}` is no longer minded by you", entry.name.as_deref().unwrap_or(&entry.id)), 1);
            }
            // Every approval dialog MUST go through `approve` (once-only,
            // exact box instance, current rules generation), never `send`.
            if before.cls.approval.is_some() {
                return err(
                    format!("`{}` has an approval prompt active: a minder must answer approvals with `approve` (`--once` or `--no`), never `send`", entry.name.as_deref().unwrap_or(&entry.id)),
                    69,
                );
            }
            if before.cls.folder_trust.is_some() {
                return err(format!("`{}` is asking to trust a folder: escalate it to the parent, never answer with `send`", entry.name.as_deref().unwrap_or(&entry.id)), 69);
            }
            // Unrecognised `Activity::Blocked` dialog ("needs input (unrecognised dialog)")
            // or incomplete approval fragment is NEVER answered with `send`: escalate to the parent!
            if before.cls.state == SubagentLiveState::WaitingQuestion && !before.cls.recognized_question {
                return err(format!("`{}` has an unrecognised dialog active: escalate it to the parent, never answer with `send`", entry.name.as_deref().unwrap_or(&entry.id)), 69);
            }
            // Also reconcile transcript-only questions (`transcript_state == "waiting"`
            // while raw screen looks `Idle`/`Working`) so they go through the same
            // grant and `rules_gen` gate instead of slipping into the steer branch.
            let eff_state = if matches!(before.cls.state, SubagentLiveState::Idle | SubagentLiveState::Working) && entry.cli.is_agent() {
                had_intervening_await = true;
                let inspected = self.inspect(std::slice::from_ref(&entry)).await;
                let info = inspected.get(&entry.id);
                effective_state(&before.cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true).0
            } else {
                before.cls.state
            };
            if eff_state == SubagentLiveState::WaitingQuestion {
                if !before.cls.recognized_question && !spec.raw_keys.is_empty() {
                    return err("a minder may only send chooser keys while a recognised screen question is active".into(), 69);
                }
                // Non-approval question (screen-recognized or transcript-only):
                // bound to the exact question and current rules generation.
                let q_key = super::service::subkey(&before.cls);
                let pair = (me.id.clone(), entry.id.clone());
                let mut st = self.lock();
                if !Self::minder_owns(&st, me, &entry.id) {
                    return err(format!("`{}` is no longer minded by you", entry.name.as_deref().unwrap_or(&entry.id)), 1);
                }
                let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
                let pending_rev = st.pending_rules_rev.contains_key(&pair);
                let ok = !pending_rev && st.told.get(&pair).is_some_and(|g| !g.is_approval && g.rules_gen == cur_gen && (g.token == q_key || (q_key.is_empty() && g.token == "tx:waiting")));
                if !ok {
                    drop(st);
                    let who = entry.name.as_deref().unwrap_or(&entry.id);
                    let hint = if me.is_shared_minder() { format!("status {who} --json") } else { "status --json".to_owned() };
                    if pending_rev {
                        return err(
                            format!(
                                "rules for `{who}` changed since you were told of this question: nothing was sent. Watch for the `minding` update, re-read its rules at {}, run `{hint}`, then judge again",
                                self.rules_path(&entry.id).display()
                            ),
                            1,
                        );
                    }
                    return err(format!("what it asks now isn't what you were last told of: nothing was sent. Look again (`{hint}` says what stands asked), judge that, then answer"), 1);
                }
                minder_send_gen = Some(cur_gen);
                // Consume the grant for text+enter answers so the same question
                // grant isn't replayed blindly; keep it during chooser arrow moves
                // until Enter submits.
                if !spec.body.is_empty() || spec.enter || spec.raw_keys.iter().any(|k| matches!(k.trim().to_ascii_lowercase().as_str(), "enter" | "return" | "cr")) {
                    st.told.remove(&pair);
                }
            } else {
                // Outside a recognised non-approval question (at rest or working):
                // never allow navigation/chooser `--key` flags (even alongside text!).
                if !spec.raw_keys.is_empty() {
                    return err("a minder may only send chooser keys while a recognised non-approval question is active; send text with `--enter` to steer at rest".into(), 69);
                }
                let st = self.lock();
                if !Self::minder_owns(&st, me, &entry.id) {
                    return err(format!("`{}` is no longer minded by you", entry.name.as_deref().unwrap_or(&entry.id)), 1);
                }
                if st.pending_rules_rev.contains_key(&(me.id.clone(), entry.id.clone())) {
                    let who = entry.name.as_deref().unwrap_or(&entry.id);
                    return err(format!("rules for `{who}` changed: watch for the `minding` update and re-read its rules before steering"), 1);
                }
                minder_send_gen = Some(st.rules_gen.get(&entry.id).copied().unwrap_or(0));
            }
        } else {
            // Text is never typed into a question: answered by `approve`, or forced.
            // Nor a key that types a character (`--key 1`): there it is an
            // answer by its number, with none of what `approve` checks.
            // Exception: explicit non-minder `--key 0` dismissing a staying feedback
            // survey (`before.survey && only_its_zero`) MUST pass even if an approval
            // or question sits behind the survey!
            if (!spec.body.is_empty() || spec.types) && !spec.force && !(before.survey && only_its_zero) && entry.cli.is_agent() && dialog_up(&before.cls) {
                let what = if spec.body.is_empty() { "press a character key in" } else { "type text into" };
                return err(format!("refusing to {what} `{}` while a {} dialog is active (use `olo agent approve` to answer approvals, or pass `--force`)", entry.id, before.cls.state.as_str()), 1);
            }
        }
        let minder_still_ok = || -> bool {
            let Some(me) = as_minder else { return true };
            let Some(exp_gen) = minder_send_gen else { return false };
            let st = self.lock();
            Self::minder_owns(&st, me, &entry.id) && st.rules_gen.get(&entry.id).copied().unwrap_or(0) == exp_gen && !st.pending_rules_rev.contains_key(&(me.id.clone(), entry.id.clone()))
        };
        let survey_blocks_write = || -> bool { entry.cli.is_agent() && !spec.force && (!only_its_zero || as_minder.is_some()) && self.survey_active(&entry.id) };
        // Validates that a fresh ordered foreground screen still permits
        // dispatching input (for BOTH minders and non-minders): rejects
        // unreadable/stale reads, surveys, feedback drafts, Exited state, and
        // newly appeared approval / folder-trust / approval-fragment / changed
        // question dialogs!
        let check_foreground = |recheck: &super::service::Looked, phase: &str| -> Result<(), String> {
            if !recheck.is_fresh_ordered() || recheck.survey || recheck.survey_pending || survey_blocks_write() {
                return Err(format!("refusing to send to `{}`: screen or feedback survey changed {phase}; nothing more was sent", entry.id));
            }
            if recheck.cls.state == SubagentLiveState::Exited {
                return Err(format!("refusing to send to `{}`: child exited {phase}; nothing more was sent", entry.id));
            }
            if entry.cli.screen_agent().is_some_and(|a| olo_agent_screen::feedback_draft(a, &recheck.screen).is_some()) {
                return Err(format!("refusing to send to `{}`: Claude Code's feedback draft appeared {phase}; nothing more was sent", entry.id));
            }
            if as_minder.is_some() {
                let changed_question = recheck.cls.state == SubagentLiveState::WaitingQuestion
                    && (!recheck.cls.recognized_question || before.cls.state != SubagentLiveState::WaitingQuestion || super::service::subkey(&recheck.cls) != super::service::subkey(&before.cls));
                // If a minder is sending chooser `--key`s, require the exact
                // recognized chooser to still be standing (once it vanishes,
                // never send remaining keys into whatever replaced it — Astra #9).
                let chooser_vanished = !spec.raw_keys.is_empty()
                    && (recheck.cls.state != SubagentLiveState::WaitingQuestion || !recheck.cls.recognized_question || super::service::subkey(&recheck.cls) != super::service::subkey(&before.cls));
                if recheck.cls.approval.is_some() || recheck.cls.folder_trust.is_some() || recheck.cls.approval_fragment || changed_question || chooser_vanished {
                    return Err(format!("refusing to send to `{}`: a {} dialog appeared {phase}; escalate or use `approve`", entry.id, recheck.cls.state.as_str()));
                }
            } else if (!body.is_empty() || spec.types) && dialog_up(&recheck.cls) {
                return Err(format!("refusing to send to `{}`: a {} dialog appeared {phase}; nothing more was sent", entry.id, recheck.cls.state.as_str()));
            }
            Ok(())
        };
        // Unconditional feedback-draft protection even when `--force` is set
        // (Astra #10: Ben's rule — "no feedback report is ever sent, `--force` or not").
        let guard_feedback_draft = entry.cli.screen_agent().is_some() && (!only_its_zero || as_minder.is_some());
        let check_feedback_draft_only = |recheck: &super::service::Looked, phase: &str| -> Result<(), String> {
            if !recheck.is_fresh_ordered() {
                return Err(format!("refusing to send to `{}`: screen unreadable {phase}; nothing more was sent", entry.id));
            }
            if entry.cli.screen_agent().is_some_and(|a| olo_agent_screen::feedback_draft(a, &recheck.screen).is_some()) {
                return Err(format!("refusing to send to `{}`: Claude Code's feedback draft appeared {phase}; nothing more was sent", entry.id));
            }
            Ok(())
        };
        let guard_foreground = entry.cli.is_agent() && !spec.force && (!only_its_zero || as_minder.is_some());
        // Recheck survey ownership and screen right before dispatching if an
        // intervening await ran (e.g., minder `inspect`) or output arrived
        // during/after the initial read (`pre_read_out_seq` — Astra #10).
        if guard_foreground || guard_feedback_draft {
            if guard_foreground && survey_blocks_write() {
                return err(format!("refusing to send to `{}`: a feedback survey appeared before dispatch; nothing was sent", entry.id), 1);
            }
            if had_intervening_await || self.out_seq(&entry.id) != out_at_before {
                let recheck = self.look_ordered(&entry, None).await;
                if guard_foreground {
                    if let Err(msg) = check_foreground(&recheck, "before dispatch") {
                        return err(msg, 1);
                    }
                } else if let Err(msg) = check_feedback_draft_only(&recheck, "before dispatch") {
                    return err(msg, 1);
                }
                if !minder_still_ok() || survey_blocks_write() {
                    return err("rules, minder assignment, or survey state changed before dispatch; nothing was sent".into(), 1);
                }
            }
        }
        // (From before the typing: a watch from this cursor has what follows it.)
        let pre_cursor = self.cursor();
        self.reset_edge(&entry, !spec.body.is_empty());
        // A message submitted to an agent at rest is a turn given: the
        // rest it shows until it takes that up is not that turn's end,
        // and the turn's end says which message it answers (`answers`:
        // this cursor). Sent into a turn already going, it is that
        // turn's, and nothing is told apart.
        let began_turn = entry.cli.is_agent() && !spec.body.is_empty() && spec.enter && matches!(before.cls.state, SubagentLiveState::Idle | SubagentLiveState::Done);
        if began_turn {
            self.expect_work(&entry, pre_cursor, super::service::taken_up(), true);
        }

        let (rex, to) = (&self.rex, &entry);
        let write = |data: String| async move { rex.write(&to.session_id, &to.block_id, &data).await.map_err(|e| e.to_string()) };
        let mut total = 0usize;
        let mut slept_before_enter = false;
        let mut out_before_writes = self.out_seq(&entry.id);
        let mut wrote_before_post = false;
        if !body.is_empty() {
            if !minder_still_ok() || survey_blocks_write() {
                return err("rules, minder assignment, or survey state changed before sending; nothing was sent".into(), 1);
            }
            // As a paste where the program takes pastes (its keys are
            // then not keys); what would end a paste early is taken out.
            let chunk = if use_paste { format!("\x1b[200~{body}\x1b[201~") } else { body.clone() };
            total += chunk.len();
            if let Err(e) = write(chunk).await {
                return err(e, 1);
            }
            wrote_before_post = true;
            if spec.enter {
                // (Apart from the paste, scaled slightly for longer pastes so it isn't taken into it.)
                tokio::time::sleep(paste_settle_delay(body.len())).await;
                slept_before_enter = true;
            }
        }
        if spec.enter {
            if !minder_still_ok() || survey_blocks_write() {
                return err("rules, minder assignment, or survey state changed before Enter; stopped without submitting".into(), 1);
            }
            // Re-verify the live foreground screen after the paste-settle sleep
            // before sending Enter, AND recheck `minder_still_ok()` AFTER the
            // awaited `look_ordered` RPC (Astra #9 / #10).
            if (guard_foreground || guard_feedback_draft) && slept_before_enter {
                let recheck = self.look_ordered(&entry, None).await;
                if guard_foreground {
                    if let Err(msg) = check_foreground(&recheck, "before Enter") {
                        return err(msg, 1);
                    }
                } else if let Err(msg) = check_feedback_draft_only(&recheck, "before Enter") {
                    return err(msg, 1);
                }
                if !minder_still_ok() || survey_blocks_write() {
                    return err("rules, minder assignment, or survey state changed before Enter; stopped without submitting".into(), 1);
                }
            }
            total += 1;
            if let Err(e) = write("\r".into()).await {
                return err(e, 1);
            }
            wrote_before_post = true;
        }
        // (Each key by itself: Escape then Enter together is another key.)
        for (i, key) in spec.post_keys.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(35)).await;
            }
            if !minder_still_ok() || survey_blocks_write() {
                return err("rules, minder assignment, or survey state changed while sending keys; stopped".into(), 1);
            }
            if (guard_foreground || guard_feedback_draft) && (i > 0 || wrote_before_post || self.out_seq(&entry.id) != out_before_writes || !self.stream_connected(&entry.id)) {
                let recheck = self.look_ordered(&entry, None).await;
                out_before_writes = self.out_seq(&entry.id);
                if guard_foreground {
                    if let Err(msg) = check_foreground(&recheck, "while sending keys") {
                        return err(msg, 1);
                    }
                } else if let Err(msg) = check_feedback_draft_only(&recheck, "while sending keys") {
                    return err(msg, 1);
                }
                if !minder_still_ok() || survey_blocks_write() {
                    return err("rules, minder assignment, or survey state changed while sending keys; stopped".into(), 1);
                }
            }
            // (A key its bytes can't say: pressed and let go through the
            // server's own encoder, as its keyboard would; its bytes only
            // where the server has no such word.)
            if let Some(press) = spec.post_presses.get(i).and_then(Option::as_ref) {
                let event = |action: &str| json!({ "session_id": entry.session_id, "block_id": entry.block_id, "event": { "type": "key", "payload": { "key": press.key, "mods": press.mods, "action": action } } });
                if self.rex.call("block.event", event("press")).await.is_ok() {
                    let _ = self.rex.call("block.event", event("release")).await;
                    total += 1;
                    continue;
                }
            }
            total += key.len();
            if let Err(e) = write(key.clone()).await {
                return err(e, 1);
            }
        }
        self.poke(&entry.id);

        // That a message sent whole with `--enter` was taken: fresh ordered
        // screen evidence that a dialog is up, OR the agent is genuinely
        // Working (NOT blank `Working("starting")`!) with no unsubmitted draft
        // in its input box (`!still_typed`), OR non-blank screen output changed
        // and the draft is gone. Enter once more ONLY when `spec.enter` is true
        // and the draft still sits in the field at 800 ms (checked in the main
        // loop body so continuous spinner output can never starve `retry_at`).
        let mut delivered = true;
        if spec.verify_delivery && spec.enter && !body.is_empty() && entry.cli.is_agent() {
            delivered = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let retry_at = tokio::time::Instant::now() + Duration::from_millis(800);
            let mut retried = false;
            let is_delivered = |now: &super::service::Looked| -> bool {
                let genuinely_working = now.cls.state == SubagentLiveState::Working && now.cls.in_turn && now.cls.summary.as_deref() != Some(olo_rex_link::agents::state::STARTING);
                // Neon #4 / Astra #15: Require `FieldState::VisibleCleared`
                // (never treat an undrawn/absent input box as cleared!).
                let field_cleared = classify_input_field(&now.screen, &body) == FieldState::VisibleCleared;
                now.is_fresh_ordered() && (dialog_up(&now.cls) || (genuinely_working && field_cleared) || (now.last_output_at > before.last_output_at && now.screen != before.screen && field_cleared))
            };
            let notifies = self.child_notifies(&entry.id);
            let mut last_out = self.out_seq(&entry.id);
            let mut last_sgen = self.stream_gen(&entry.id);
            let mut checked_once = false;
            let mut fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
            loop {
                let now_inst = tokio::time::Instant::now();
                let cur_out = self.out_seq(&entry.id);
                let cur_sgen = self.stream_gen(&entry.id);
                let fallback_ready = fallback_due.is_some_and(|d| now_inst >= d);
                if !checked_once || cur_out != last_out || cur_sgen != last_sgen || fallback_ready {
                    checked_once = true;
                    last_out = cur_out;
                    last_sgen = cur_sgen;
                    fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
                    let now = self.look_until(&entry, true, Some(deadline)).await;
                    if is_delivered(&now) {
                        delivered = true;
                        break;
                    }
                }
                // Check `retry_at` in the main loop body (outside `select!`) so
                // continuous spinner/output ticks can NEVER starve the 800 ms retry!
                let now_inst = tokio::time::Instant::now();
                if spec.enter && !retried && now_inst >= retry_at && now_inst < deadline {
                    retried = true;
                    let check = self.look_until(&entry, true, Some(deadline)).await;
                    if is_delivered(&check) {
                        delivered = true;
                        break;
                    }
                    // Astra #15: Require positive `VisibleHasDraft` match of the
                    // SAME draft before sending retry Enter, bounded by `deadline`.
                    if check.is_fresh_ordered()
                        && minder_still_ok()
                        && !survey_blocks_write()
                        && check_foreground(&check, "before retry Enter").is_ok()
                        && classify_input_field(&check.screen, &body) == FieldState::VisibleHasDraft
                    {
                        last_out = self.out_seq(&entry.id);
                        last_sgen = self.stream_gen(&entry.id);
                        let _ = tokio::time::timeout_at(deadline, write("\r".into())).await;
                    }
                }
                let now_inst = tokio::time::Instant::now();
                if now_inst >= deadline {
                    break;
                }
                let base_wake = if spec.enter && !retried && retry_at < deadline { retry_at } else { deadline };
                let wake = fallback_due.map_or(base_wake, |f| f.min(base_wake));
                if let Some((ref out_n, ref st_n, _)) = notifies {
                    let out_wait = out_n.notified();
                    tokio::pin!(out_wait);
                    out_wait.as_mut().enable();
                    let st_wait = st_n.notified();
                    tokio::pin!(st_wait);
                    st_wait.as_mut().enable();
                    // Neon #3 / Astra #17: Check `out_seq`, `stream_gen`, and
                    // disconnected fallback AFTER `enable()` so a disconnect
                    // during the preceding RPC is never missed!
                    if self.out_seq(&entry.id) != last_out || self.stream_gen(&entry.id) != last_sgen {
                        if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                            fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        }
                        continue;
                    }
                    if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                        fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        continue;
                    }
                    tokio::select! {
                        _ = out_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = st_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = tokio::time::sleep_until(wake) => {}
                    }
                } else {
                    tokio::time::sleep_until(wake).await;
                }
            }
        }
        // (`began_turn`: the child was at rest, so the turn that follows is this
        // message's own, and its end's `answers` is this `cursor`; sent into a
        // turn already going, it is that turn's, and the next rest is its end.
        // What `send --wait` waits by.)
        let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "bytes": total, "delivered": delivered, "cursor": pre_cursor, "began_turn": began_turn });
        Response::ok(if spec.as_json { format!("{payload}\n") } else { format!("Sent {total} bytes to {} (delivered={delivered}).\n", entry.id) }, payload)
    }

    // ---- resume ----

    /// `olo agent resume <ID|NAME> [--prompt TEXT] [--json]`: an ended
    /// child started again as the same conversation, with what it was
    /// started with (the ShiftUp lead's item 44, 2026-11-27: "starts an
    /// exited child again as the same conversation, with the flags it
    /// was started with (model, effort, cwd, env, settings, the list,
    /// its minder and rules). Then our records keep no conversation id
    /// at all"). Its arguments were kept at its start
    /// (`Service::keep_start`); what named its conversation then
    /// (`--session-id`, `--resume`) gives way to the conversation it
    /// had, and it is `start` from there on: a new child, of the same
    /// name, under whoever asks.
    async fn resume(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        const USAGE: &str = "usage: olo agent resume <ID|NAME> [--prompt TEXT | --prompt-file FILE] [--json]";
        let err = |msg: String, code: i32| Response::err("resume", msg, code);
        let args: Vec<String> = raw.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let (mut target, mut prompt, mut as_json) = (None::<String>, None::<String>, false);
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--json" => as_json = true,
                "--prompt" | "-p" => match it.next() {
                    Some(text) => prompt = Some(text),
                    None => return err("--prompt requires its text".into(), 2),
                },
                a if a.starts_with("--prompt=") => prompt = Some(a["--prompt=".len()..].to_owned()),
                a if a.starts_with('-') => return err(format!("unknown resume option `{a}`: it is started with what it was started with\n{USAGE}"), 2),
                _ if target.is_none() => target = Some(arg),
                a => return err(format!("unexpected argument `{a}`\n{USAGE}"), 2),
            }
        }
        let Some(target) = target else { return err(USAGE.into(), 2) };
        let entry = match self.resolve(caller, &target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        let name = entry.name.clone().unwrap_or_else(|| entry.id.clone());
        if entry.exited_at.is_none() {
            return err(format!("`{name}` is running: there is nothing to start again (to speak to it: `olo agent send {name} …`)"), 1);
        }
        if !entry.cli.is_agent() {
            return err(format!("`{name}` is a command, with no conversation to take up again: start it again"), 1);
        }
        let Some(conversation) = entry.effective_session_id().map(str::to_owned) else {
            return err(format!("`{name}` has no conversation to take up again (it ended before it began one): start it anew"), 1);
        };
        let Some(kept) = self.kept_start(&entry.id) else {
            return err(format!("what `{name}` was started with wasn't kept (it was started by an earlier Olo, or not by its arguments): start it with your own, and `--resume {conversation}`"), 1);
        };
        // Its own arguments, less what named a conversation and whether
        // to answer as JSON; then the conversation it had, and what is
        // said to it now.
        let cut = kept.iter().position(|a| a == "--").unwrap_or(kept.len());
        let (mut again, after) = (Vec::with_capacity(kept.len() + 5), kept[cut..].to_vec());
        let mut it = kept[..cut].iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--resume" | "--session-id" => {
                    it.next();
                }
                "--json" => {}
                a if a.starts_with("--resume=") || a.starts_with("--session-id=") => {}
                _ => again.push(arg.clone()),
            }
        }
        again.extend(["--resume".to_owned(), conversation]);
        if let Some(text) = prompt {
            again.extend(["--prompt".to_owned(), text]);
        }
        if as_json {
            again.push("--json".to_owned());
        }
        again.extend(after);
        self.start(req, &json!({ "args": again }), caller).await
    }

    // ---- approve ----

    pub(super) async fn approve(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("approve", msg, code);
        let spec = match ApproveSpec::parse(Some(raw)) {
            Ok(s) => s,
            Err(e) => return err(e, 2),
        };
        let entry = match self.resolve(caller, &spec.target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        let as_minder = caller.child.as_ref().filter(|c| c.is_minder());
        // (A minder's yes is for this once: one that is kept answers
        // questions its rules were never asked about.)
        if as_minder.is_some() && spec.answer == olo_rex_link::agents::Answer::Session {
            return err("a minder's yes is for this once (`--once`): one that is kept is the parent's to give".into(), 69);
        }
        // Capture a minder's exact `(token, rules_gen)` grant BEFORE awaiting
        // `answer_lock`, so a queued minder request can NEVER wait behind
        // another answer and spend a future grant minted while it waited!
        let captured_minder_grant: Option<(String, u64)> = if let Some(me) = as_minder {
            let pair = (me.id.clone(), entry.id.clone());
            let st = self.lock();
            let who = entry.name.as_deref().unwrap_or(&entry.id);
            if !Self::minder_owns(&st, me, &entry.id) {
                return err(format!("`{who}` is no longer minded by you"), 1);
            }
            let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
            if st.pending_rules_rev.contains_key(&pair) {
                let hint = if me.is_shared_minder() { format!("status {who} --json") } else { "status --json".to_owned() };
                return err(
                    format!(
                        "rules for `{who}` changed since you were told of this question: nothing was answered. Watch for the `minding` update, re-read its rules at {}, run `{hint}`, then judge again",
                        self.rules_path(&entry.id).display()
                    ),
                    1,
                );
            }
            let grant = st.told.get(&pair).filter(|g| g.is_approval && g.rules_gen == cur_gen).cloned();
            let Some(grant) = grant else {
                let hint = if me.is_shared_minder() { format!("status {who} --json") } else { "status --json".to_owned() };
                return err(format!("what it asks now isn't what you were last told of: nothing was answered. Look again (`{hint}` says what stands asked), judge that, then answer"), 1);
            };
            if let Some(meant) = spec.box_key.as_deref()
                && meant != grant.token
            {
                return err(
                    format!(
                        "the box you answer (`{meant}`) isn't the one you were last told of (`{}`): nothing was answered. `olo agent status {who} --json` has what it asks; judge that, then answer it by its own key",
                        grant.token
                    ),
                    1,
                );
            }
            Some((grant.token, cur_gen))
        } else {
            None
        };

        // One writer per child's answers. An asking that names its box
        // (`--box KEY`), and a minder's (now pinned to its captured pre-lock
        // grant token), waits its turn and is held strictly to that box.
        // An unkeyed parent request refuses contention immediately.
        let answers = self.answer_lock(&entry.id);
        let _answer_lock = match answers.clone().try_lock_owned() {
            Ok(held) => held,
            Err(_) if spec.box_key.is_none() && as_minder.is_none() => {
                let who = entry.name.as_deref().unwrap_or(&entry.id);
                return err(
                    format!(
                        "`{who}` is being answered just now: nothing more was pressed. Look at what it asks after that (`olo agent status {who} --json`) and answer that box by its key (`--box KEY`)"
                    ),
                    1,
                );
            }
            Err(_) => answers.lock_owned().await,
        };
        let pre_cursor = self.cursor();
        // (As the server shows it this moment, fresh and in order.)
        let looked_ap = self.look(&entry, true).await;
        if entry.cli.is_agent() && !looked_ap.is_fresh_ordered() && looked_ap.cls.state != SubagentLiveState::Exited {
            return err(format!("`{}`'s screen couldn't be read reliably just now: nothing was answered; try again", entry.id), 1);
        }
        // Require a clear foreground (no staying or active feedback survey
        // overlay that would swallow arrow/Return keys!).
        if entry.cli.is_agent() && (looked_ap.survey || looked_ap.survey_pending || self.survey_active(&entry.id)) {
            return err(format!("`{}` has a feedback survey covering the foreground: dismiss it (`send {} --key 0`) before answering the approval", entry.id, entry.id), 1);
        }
        let standing_now = looked_ap.boxed.clone();
        let cls = looked_ap.cls;
        let Some(ap) = cls.approval else {
            return err(format!("`{}` has no active approval prompt (current state: {})", entry.id, cls.state.as_str()), 1);
        };
        // Exact numbered box instance (`bN-hhhhhhhh`): two askings of the same
        // command are two distinct box keys.
        let Some(standing_box) = standing_now else {
            return err(format!("`{}`'s screen changed as it was read: nothing was answered; look again and answer what stands", entry.id), 1);
        };
        let standing = standing_box.key.clone();
        if standing_box.answered {
            return err(format!("the approval box (`{standing}`) on `{}` was already answered and is still dismissing", entry.name.as_deref().unwrap_or(&entry.id)), 1);
        }
        // Verify captured minder grant still matches the exact standing box AND
        // current ownership/rules generation after acquiring `answer_lock`.
        if let (Some(me), Some((cap_token, cap_gen))) = (as_minder, captured_minder_grant.as_ref()) {
            let pair = (me.id.clone(), entry.id.clone());
            let st = self.lock();
            let who = entry.name.as_deref().unwrap_or(&entry.id);
            if !Self::minder_owns(&st, me, &entry.id) {
                return err(format!("`{who}` is no longer minded by you"), 1);
            }
            let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
            let pending_rev = st.pending_rules_rev.contains_key(&pair);
            let valid = !pending_rev && cur_gen == *cap_gen && standing == *cap_token && st.told.get(&pair).is_some_and(|g| g.is_approval && g.token == *cap_token && g.rules_gen == *cap_gen);
            if !valid {
                drop(st);
                let hint = if me.is_shared_minder() { format!("status {who} --json") } else { "status --json".to_owned() };
                if pending_rev || cur_gen != *cap_gen {
                    return err(
                        format!(
                            "rules for `{who}` changed since you were told of this question: nothing was answered. Watch for the `minding` update, re-read its rules at {}, run `{hint}`, then judge again",
                            self.rules_path(&entry.id).display()
                        ),
                        1,
                    );
                }
                return err(format!("what it asks now isn't what you were last told of: nothing was answered. Look again (`{hint}` says what stands asked), judge that, then answer"), 1);
            }
        }
        self.minder_did(caller, &entry.id, "approve", raw.get("args").cloned().unwrap_or(Value::Null));
        // The box it was judged for, where the caller says which
        // (`--box KEY`): another that stands by now is not answered.
        if let Some(meant) = spec.box_key.as_deref()
            && meant != standing
        {
            return err(
                format!(
                    "the box you answer (`{meant}`) isn't the one that stands now (`{standing}`): nothing was answered. `olo agent status {} --json` has what it asks; judge that, then answer it by its own key",
                    entry.name.as_deref().unwrap_or(&entry.id)
                ),
                1,
            );
        }
        let same_instance = |now: &super::service::Looked| now.boxed.as_ref().is_some_and(|b| b.key == standing);
        let ownership_and_rules_hold = || -> bool {
            let Some(me) = as_minder else { return true };
            let Some((_, exp_gen)) = captured_minder_grant.as_ref() else { return false };
            let st = self.lock();
            let pair = (me.id.clone(), entry.id.clone());
            let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
            Self::minder_owns(&st, me, &entry.id) && cur_gen == *exp_gen && !st.pending_rules_rev.contains_key(&pair)
        };
        // Includes BOTH minder authority AND `--by-list` re-validation on the
        // LATEST `cur` prompt (so list revocation OR newly appeared folded lines
        // aborts immediately during navigation!).
        let grant_holds = |cur_ap: &olo_agent_screen::ApprovalPrompt| -> bool {
            if spec.by_list && self.list_allows(&entry.id, cur_ap).is_none() {
                return false;
            }
            let Some(me) = as_minder else { return true };
            let Some((cap_tok, exp_gen)) = captured_minder_grant.as_ref() else { return false };
            let st = self.lock();
            let pair = (me.id.clone(), entry.id.clone());
            let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
            Self::minder_owns(&st, me, &entry.id)
                && cur_gen == *exp_gen
                && !st.pending_rules_rev.contains_key(&pair)
                && st.told.get(&pair).is_some_and(|g| g.is_approval && g.token == *cap_tok && g.rules_gen == *exp_gen)
        };
        if !grant_holds(&ap) {
            return err(
                if spec.by_list {
                    "its list no longer allows what it asks: nothing was answered".into()
                } else {
                    "rules or minder assignment changed before confirmation; nothing was answered".into()
                },
                1,
            );
        }
        // The one choice of the kind asked for: by what it means, never
        // by where it stands. Two of a kind is nobody's to choose between.
        let want = spec.answer.kind();
        let matching: Vec<&olo_agent_screen::ApprovalChoice> = ap.choices.iter().filter(|c| c.kind == want).collect();
        if matching.len() > 1 {
            let labels: Vec<&str> = matching.iter().map(|c| c.label.as_str()).collect();
            return err(format!("ambiguous `{}` choices in active dialog ({}); refusing to pick automatically", want.as_str(), labels.join("; ")), 1);
        }
        let Some(choice) = matching.into_iter().next().cloned() else {
            let there: Vec<String> = ap.choices.iter().map(|c| format!("{} [{}]", c.label, c.kind.as_str())).collect();
            return err(format!("no safe `{}` choice found in active dialog (choices: {})", want.as_str(), there.join("; ")), 1);
        };
        let write = |data: &'static str| self.rex.write(&entry.session_id, &entry.block_id, data);
        let notifies = self.child_notifies(&entry.id);
        let delta = choice.index as isize - ap.selected_index.unwrap_or(0) as isize;
        let on_it = |cur: &olo_agent_screen::ApprovalPrompt| cur.selected_index == Some(choice.index) && cur.choices.get(choice.index).is_some_and(|c| c.kind == want && c.label == choice.label);
        let mut verified_ap = ap.clone();
        if delta != 0 {
            struct NavigatingGuard<'a> {
                service: &'a Service,
                id: &'a str,
            }
            impl Drop for NavigatingGuard<'_> {
                fn drop(&mut self) {
                    self.service.set_navigating(self.id, false);
                }
            }
            self.set_navigating(&entry.id, true);
            let _nav_guard = NavigatingGuard { service: self, id: &entry.id };
            let mut last_out = self.out_seq(&entry.id);
            let mut last_sgen = self.stream_gen(&entry.id);
            for step in 0..delta.unsigned_abs() {
                if self.survey_active(&entry.id) {
                    return err("a feedback survey appeared over the approval dialog during navigation; aborted without further input".into(), 1);
                }
                // Astra #16: After an intervening sleep between arrows, recheck
                // the live foreground screen before sending the next arrow so a
                // survey or replacement dialog never receives arrow keys!
                if step > 0 {
                    let step_now = self.look_ordered(&entry, None).await;
                    if !step_now.is_fresh_ordered() || step_now.survey || step_now.survey_pending || self.survey_active(&entry.id) {
                        return err("a feedback survey or screen change interrupted approval navigation; aborted without further input".into(), 1);
                    }
                    if !same_instance(&step_now) {
                        return err("approval dialog changed during navigation; nothing more was pressed".into(), 1);
                    }
                    if let Some(cur) = step_now.cls.approval.as_ref() {
                        if !grant_holds(cur) {
                            return err(
                                if spec.by_list {
                                    "its list no longer allows what it asks during navigation; aborted without further input".into()
                                } else {
                                    "rules or minder assignment changed during navigation; aborted without further input".into()
                                },
                                1,
                            );
                        }
                        verified_ap = cur.clone();
                    }
                }
                if !grant_holds(&verified_ap) {
                    return err(
                        if spec.by_list {
                            "its list no longer allows what it asks during navigation; aborted without further input".into()
                        } else {
                            "rules or minder assignment changed during navigation; aborted without further input".into()
                        },
                        1,
                    );
                }
                if let Err(e) = write(if delta > 0 { "\x1b[B" } else { "\x1b[A" }).await {
                    return err(format!("failed to navigate approval dialog: {e}"), 1);
                }
                if step + 1 < delta.unsigned_abs() {
                    tokio::time::sleep(Duration::from_millis(45)).await;
                }
            }
            // Wait for cursor to land using per-child `output_notify` + `state_notify`
            // + single expiry timer (zero 55ms stationary polling while `stream_connected`;
            // 250 ms fallback only while disconnected).
            // On unresolved timeout or uncertainty, abort cleanly WITHOUT sending Escape!
            let verify_by = tokio::time::Instant::now() + Duration::from_millis(1500);
            let mut verified = false;
            let mut none_since: Option<tokio::time::Instant> = None;
            let mut fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
            loop {
                if !grant_holds(&verified_ap) {
                    return err(
                        if spec.by_list {
                            "its list no longer allows what it asks during navigation; aborted without further input".into()
                        } else {
                            "rules or minder assignment changed during navigation; aborted without further input".into()
                        },
                        1,
                    );
                }
                let now_inst = tokio::time::Instant::now();
                let cur_out = self.out_seq(&entry.id);
                let cur_sgen = self.stream_gen(&entry.id);
                let blank_ready = none_since.is_some_and(|s| s.elapsed() >= Duration::from_millis(180));
                let fallback_ready = fallback_due.is_some_and(|d| now_inst >= d);
                if cur_out != last_out || cur_sgen != last_sgen || blank_ready || fallback_ready {
                    last_out = cur_out;
                    last_sgen = cur_sgen;
                    fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
                    let cur_looked = self.look_until(&entry, true, Some(verify_by)).await;
                    if cur_looked.cls.state == SubagentLiveState::Exited {
                        drop(_nav_guard);
                        self.box_gone(&entry.id, standing_box.number);
                        return err("approval dialog disappeared before confirmation".into(), 1);
                    }
                    if cur_looked.is_fresh_ordered() {
                        if cur_looked.survey || cur_looked.survey_pending || self.survey_active(&entry.id) {
                            return err("a feedback survey appeared over the approval dialog during navigation; aborted without pressing Enter".into(), 1);
                        }
                        let still_it = same_instance(&cur_looked);
                        match cur_looked.cls.approval {
                            None if super::service::is_positively_elsewhere(&cur_looked.cls) => {
                                drop(_nav_guard);
                                self.box_gone(&entry.id, standing_box.number);
                                return err("approval dialog disappeared before confirmation".into(), 1);
                            }
                            None => match none_since {
                                Some(since) if since.elapsed() >= Duration::from_millis(180) => {
                                    drop(_nav_guard);
                                    self.box_gone(&entry.id, standing_box.number);
                                    return err("approval dialog disappeared before confirmation".into(), 1);
                                }
                                Some(_) => {}
                                None => none_since = Some(tokio::time::Instant::now()),
                            },
                            Some(cur) => {
                                none_since = None;
                                if !still_it {
                                    return err("approval dialog changed during navigation; nothing more was pressed".into(), 1);
                                }
                                if !grant_holds(&cur) {
                                    return err(
                                        if spec.by_list {
                                            "its list no longer allows what it asks during navigation; aborted without further input".into()
                                        } else {
                                            "rules or minder assignment changed during navigation; aborted without further input".into()
                                        },
                                        1,
                                    );
                                }
                                verified_ap = cur;
                                if on_it(&verified_ap) {
                                    verified = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                let now_inst = tokio::time::Instant::now();
                if now_inst >= verify_by {
                    break;
                }
                let base_wake = none_since.map_or(verify_by, |s| (s + Duration::from_millis(180)).min(verify_by));
                let wake_at = fallback_due.map_or(base_wake, |f| f.min(base_wake));
                if let Some((ref out_n, ref st_n, _)) = notifies {
                    let out_wait = out_n.notified();
                    tokio::pin!(out_wait);
                    out_wait.as_mut().enable();
                    let st_wait = st_n.notified();
                    tokio::pin!(st_wait);
                    st_wait.as_mut().enable();
                    if self.out_seq(&entry.id) != last_out || self.stream_gen(&entry.id) != last_sgen {
                        if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                            fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        }
                        continue;
                    }
                    if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                        fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        continue;
                    }
                    tokio::select! {
                        _ = out_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = st_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = tokio::time::sleep_until(wake_at) => {}
                    }
                } else {
                    tokio::time::sleep_until(wake_at).await;
                }
            }
            if !verified {
                if !grant_holds(&verified_ap) {
                    return err(
                        if spec.by_list {
                            "its list no longer allows what it asks during navigation; aborted without further input".into()
                        } else {
                            "rules or minder assignment changed during navigation; aborted without further input".into()
                        },
                        1,
                    );
                }
                // Abort cleanly WITHOUT sending Escape so a slow or replacement dialog is never cancelled blindly.
                return err(format!("cursor did not land on `{}` (`{}`); aborted without pressing Enter or Escape", choice.label, want.as_str()), 1);
            }
        }
        // Pre-load the child's leave list outside the state lock, then atomically
        // re-check the LATEST verified prompt against `list_allows_locked` (for
        // `--by-list`), verify/consume the minder's grant, AND mark this showing
        // as answered under ONE `self.lock()` hold BEFORE writing Enter.
        if spec.by_list {
            let _ = self.leave_of(&entry.id);
        }
        let (standing_num, answer_event_gen) = {
            let mut st = self.lock();
            if entry.cli.is_agent() && st.live.get(&entry.id).is_some_and(|l| l.survey_active_locked()) {
                return err("a feedback survey is covering the foreground before confirmation; nothing was answered".into(), 1);
            }
            if spec.by_list && Self::list_allows_locked(&st, &entry.id, &verified_ap).is_none() {
                return err("its list no longer allows what it asks: nothing was answered".into(), 1);
            }
            if let Some(me) = as_minder {
                let Some((cap_tok, exp_gen)) = captured_minder_grant.as_ref() else {
                    return err("rules or minder assignment changed before confirmation; nothing was answered".into(), 1);
                };
                let pair = (me.id.clone(), entry.id.clone());
                let cur_gen = st.rules_gen.get(&entry.id).copied().unwrap_or(0);
                let holds = Self::minder_owns(&st, me, &entry.id)
                    && cur_gen == *exp_gen
                    && !st.pending_rules_rev.contains_key(&pair)
                    && st.told.get(&pair).is_some_and(|g| g.is_approval && g.token == *cap_tok && g.rules_gen == *exp_gen);
                if !holds {
                    return err("rules or minder assignment changed before confirmation; nothing was answered".into(), 1);
                }
                st.told.remove(&pair);
            }
            match Service::box_mark_answered_locked(&mut st, &entry.id, &standing) {
                Ok(v) => v,
                Err(()) => return err(format!("the approval box (`{standing}`) was already answered or replaced before confirmation"), 1),
            }
        };
        struct Answering<'a> {
            service: &'a Service,
            id: &'a str,
            number: u64,
            answer_event_gen: u64,
            finished: bool,
        }
        impl Drop for Answering<'_> {
            fn drop(&mut self) {
                if !self.finished {
                    self.service.box_finish(self.id, self.number, super::service::BoxEnd::Stays, Some(self.answer_event_gen));
                }
            }
        }
        let mut answering = Answering { service: self, id: &entry.id, number: standing_num, answer_event_gen, finished: false };
        let mut last_out = self.out_seq(&entry.id);
        if let Err(e) = write("\r").await {
            return err(format!("failed to confirm approval dialog: {e}"), 1);
        }

        // Wait up to 2s for the box to dismiss or change via per-child
        // `output_notify` + `state_notify` + single expiry timer (zero 100ms
        // stationary polling while `stream_connected`; 250 ms fallback only
        // while disconnected). Never send an automatic retry Enter.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        use super::service::BoxEnd;
        let mut end = BoxEnd::Stays;
        let mut none_since: Option<tokio::time::Instant> = None;
        let agent_opt = entry.cli.screen_agent();
        let mut fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
        let mut last_sgen_d = self.stream_gen(&entry.id);
        loop {
            if self.box_was_retired_elsewhere(&entry.id, standing_num) {
                end = BoxEnd::Gone;
                break;
            }
            let now_inst = tokio::time::Instant::now();
            let cur_out = self.out_seq(&entry.id);
            let cur_sgen = self.stream_gen(&entry.id);
            let blank_ready = none_since.is_some_and(|s| s.elapsed() >= Duration::from_millis(150));
            let fallback_ready = fallback_due.is_some_and(|d| now_inst >= d);
            if cur_out != last_out || cur_sgen != last_sgen_d || blank_ready || fallback_ready {
                last_out = cur_out;
                last_sgen_d = cur_sgen;
                fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
                let now = self.look_until(&entry, true, Some(deadline)).await;
                if now.cls.state == SubagentLiveState::Exited || self.box_was_retired_elsewhere(&entry.id, standing_num) {
                    end = BoxEnd::Gone;
                    break;
                }
                if now.is_fresh_ordered() {
                    match (now.cls.approval.is_some(), same_instance(&now)) {
                        (true, true) => none_since = None,
                        // Another complete standing box in its place: this one went.
                        (true, false) if now.boxed.is_some() => {
                            end = BoxEnd::Gone;
                            break;
                        }
                        (true, false) => {}
                        // No box, and agent positively elsewhere (running turn, chat prompt, or refusal reason field).
                        (false, _) if super::service::is_positively_elsewhere(&now.cls) || agent_opt.is_some_and(|a| is_refusal_reason_field(a, &now.screen, &now.cls)) => {
                            end = BoxEnd::Gone;
                            break;
                        }
                        (false, _) => match none_since {
                            Some(since) if since.elapsed() >= Duration::from_millis(150) => {
                                end = BoxEnd::Blank;
                                break;
                            }
                            Some(_) => {}
                            None => none_since = Some(tokio::time::Instant::now()),
                        },
                    }
                }
            }
            let now_inst = tokio::time::Instant::now();
            if now_inst >= deadline {
                break;
            }
            let base_wake = none_since.map_or(deadline, |s| (s + Duration::from_millis(150)).min(deadline));
            let wake_at = fallback_due.map_or(base_wake, |f| f.min(base_wake));
            if let Some((ref out_n, ref st_n, _)) = notifies {
                let out_wait = out_n.notified();
                tokio::pin!(out_wait);
                out_wait.as_mut().enable();
                let st_wait = st_n.notified();
                tokio::pin!(st_wait);
                st_wait.as_mut().enable();
                if self.out_seq(&entry.id) != last_out || self.stream_gen(&entry.id) != last_sgen_d || self.box_was_retired_elsewhere(&entry.id, standing_num) {
                    if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                        fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                    }
                    continue;
                }
                if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                    fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                    continue;
                }
                tokio::select! {
                    _ = out_wait => {
                        if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                            fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        }
                    }
                    _ = st_wait => {
                        if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                            fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        }
                    }
                    _ = tokio::time::sleep_until(wake_at) => {}
                }
            } else {
                tokio::time::sleep_until(wake_at).await;
            }
        }

        let dismissed = end != BoxEnd::Stays;
        self.box_finish(&entry.id, standing_num, end, Some(answer_event_gen));
        answering.finished = true;

        // A refusal's reason is pasted ONLY when `end == BoxEnd::Gone` (NEVER
        // after `BoxEnd::Blank`, `BoxEnd::Stays`, or timer expiry!), and ONLY
        // after a fresh, live, ordered positive identification of a safe reason
        // input target (chat prompt or refusal reason field — never a chooser,
        // approval, folder trust, survey, or feedback draft), re-verified
        // immediately before paste AND immediately before Enter.
        let mut reason_sent = false;
        if end == BoxEnd::Gone
            && spec.answer == olo_rex_link::agents::Answer::No
            && let Some(reason) = spec.reason.as_deref().map(sanitize_paste_text).map(|r| r.trim().to_owned()).filter(|r| !r.is_empty())
            && let Some(agent) = agent_opt
        {
            let by = tokio::time::Instant::now() + Duration::from_millis(1200);
            let mut last_out_r = self.out_seq(&entry.id);
            let mut last_sgen_r = self.stream_gen(&entry.id);
            let mut checked_once = false;
            let mut fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
            let safe = loop {
                if !ownership_and_rules_hold() {
                    break false;
                }
                let now_inst = tokio::time::Instant::now();
                let cur_out = self.out_seq(&entry.id);
                let cur_sgen = self.stream_gen(&entry.id);
                let fallback_ready = fallback_due.is_some_and(|d| now_inst >= d);
                if !checked_once || cur_out != last_out_r || cur_sgen != last_sgen_r || fallback_ready {
                    checked_once = true;
                    last_out_r = cur_out;
                    last_sgen_r = cur_sgen;
                    fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
                    let now = self.look_ordered(&entry, Some(by)).await;
                    if now.cls.state == SubagentLiveState::Exited || now.cls.approval.is_some() || now.cls.folder_trust.is_some() || now.cls.state == SubagentLiveState::WaitingQuestion {
                        break false;
                    }
                    if is_safe_reason_target_for_paste(agent, &now) {
                        break ownership_and_rules_hold();
                    }
                }
                if tokio::time::Instant::now() >= by {
                    // Never paste on timer expiry!
                    break false;
                }
                let wake_at = fallback_due.map_or(by, |f| f.min(by));
                if let Some((ref out_n, ref st_n, _)) = notifies {
                    let out_wait = out_n.notified();
                    tokio::pin!(out_wait);
                    out_wait.as_mut().enable();
                    let st_wait = st_n.notified();
                    tokio::pin!(st_wait);
                    st_wait.as_mut().enable();
                    if self.out_seq(&entry.id) != last_out_r || self.stream_gen(&entry.id) != last_sgen_r {
                        if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                            fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        }
                        continue;
                    }
                    if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                        fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                        continue;
                    }
                    tokio::select! {
                        _ = out_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = st_wait => {
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = tokio::time::sleep_until(wake_at) => {}
                    }
                } else {
                    tokio::time::sleep_until(wake_at).await;
                }
            };
            if safe && ownership_and_rules_hold() {
                let text = if self.bracketed_paste(&entry).await { format!("\x1b[200~{reason}\x1b[201~") } else { reason.clone() };
                let pre_paste = self.look_ordered(&entry, None).await;
                if ownership_and_rules_hold() && is_safe_reason_target_for_paste(agent, &pre_paste) && self.rex.write(&entry.session_id, &entry.block_id, &text).await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    let pre_enter = self.look_ordered(&entry, None).await;
                    if ownership_and_rules_hold() && is_safe_reason_target_for_enter(agent, &pre_enter, &reason) && write("\r").await.is_ok() {
                        reason_sent = true;
                    }
                    self.poke(&entry.id);
                }
            }
        }

        let dismissal = match end {
            BoxEnd::Gone => "gone",
            BoxEnd::Blank => "blank",
            BoxEnd::Stays => "stays",
        };
        let payload = json!({
            "ok": dismissed,
            "id": entry.id,
            "name": entry.name,
            "chosen_label": choice.label,
            "chosen_kind": choice.kind.as_str(),
            "dismissed": dismissed,
            "dismissal": dismissal,
            "ambiguous": end == BoxEnd::Blank,
            "reason_sent": reason_sent,
            "cursor": pre_cursor,
        });
        let stdout = if spec.json {
            format!("{payload}\n")
        } else if dismissed {
            format!("{}  approved ({}) — {}\n", entry.id, choice.kind.as_str(), choice.label)
        } else {
            format!("{}  sent ({}) but dialog did not dismiss within 2s\n", entry.id, choice.kind.as_str())
        };
        Response {
            ok: dismissed,
            stdout,
            stderr: if dismissed { String::new() } else { format!("olo agent approve: dialog on `{}` did not dismiss within 2s\n", entry.id) },
            code: i32::from(!dismissed),
            data: payload,
        }
    }

    // ---- stop ----

    async fn stop(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let (mut target, mut force, mut tree, mut keep_tab, mut as_json, mut save) = (None::<String>, false, false, false, false, None::<String>);
        if let Some(args) = args_of(raw) {
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--force" | "-f" => force = true,
                    "--tree" | "--recursive" | "-r" => tree = true,
                    "--keep-tab" => keep_tab = true,
                    "--json" => as_json = true,
                    "--save" => {
                        i += 1;
                        save = args.get(i).cloned();
                    }
                    s if s.starts_with("--save=") => save = Some(s["--save=".len()..].to_owned()),
                    s if !s.starts_with('-') && target.is_none() => target = Some(s.to_owned()),
                    _ => {}
                }
                i += 1;
            }
        } else {
            target = raw.get("target").or_else(|| raw.get("id")).and_then(Value::as_str).map(str::to_owned);
            force = raw["force"].as_bool().unwrap_or(false);
            tree = raw["tree"].as_bool().unwrap_or(false);
            keep_tab = raw["keep_tab"].as_bool().unwrap_or(false);
            as_json = raw["json"].as_bool().unwrap_or(false);
            save = raw["save"].as_str().map(str::to_owned);
        }
        let Some(target) = target else {
            return Response {
                ok: false,
                stdout: String::new(),
                stderr: "usage: olo agent stop <ID|NAME> [--save PATH] [--force] [--tree] [--keep-tab] [--json]\n".into(),
                code: 2,
                data: Value::Null,
            };
        };
        if force && keep_tab {
            return Response::err("stop", "`--force` and `--keep-tab` cannot be combined", 2);
        }
        let entry = match self.resolve(caller, &target) {
            Ok(e) => e,
            Err(e) => {
                // `stop <LEAD_WINDOW_ID> --tree` on a top-level (non-subagent)
                // tab stops every descendant under the asking tab's OWN window
                // (never another lead's tab in the same session), leaving the
                // lead's own tab open.
                if tree
                    && caller.child.is_none()
                    && save.is_none()
                    && let Some(sid) = caller.session.as_deref()
                    && self.live_windows(sid).contains(&target)
                {
                    if !caller.window_from_block || caller.window.as_deref() != Some(target.as_str()) {
                        return Response::err(
                            "stop",
                            format!("refused: top-level `stop --tree` requires a mapped `REX_BLOCK` in your own tab and may only stop your own tab's descendants, not `{target}`"),
                            69,
                        );
                    }
                    let live = self.live_windows(sid);
                    let mut under = self.lock().reg.descendants_tree(HOST, sid, &target, &live);
                    under.reverse();
                    let targets: Vec<NativeSubagent> = under.into_iter().map(|r| r.entry).collect();
                    let mut running: Vec<(NativeSubagent, i32, u64)> = Vec::new();
                    {
                        let st = self.lock();
                        for t in targets.iter().filter(|t| t.exited_at.is_none()) {
                            if let Some(pid) = st.live.get(&t.id).and_then(|l| l.pid).or(t.last_pid).filter(|p| *p > 1) {
                                let began = crate::panes::start_ticks(pid);
                                if began > 0 {
                                    running.push((t.clone(), pid, began));
                                }
                            }
                        }
                    }
                    let (stopped, failed) = self.stop_target_list(&targets, force, keep_tab, true).await;
                    let pids: Vec<(i32, u64)> = running.iter().map(|(_, pid, began)| (*pid, *began)).collect();
                    let still = still_running_after(&pids, if keep_tab { Duration::ZERO } else { STOPPED_WITHIN }).await;
                    let programs: Vec<Value> = running.iter().zip(&still).map(|((e, pid, _), still)| json!({ "id": e.id, "name": e.name, "pid": pid, "ended": !still })).collect();
                    let (ended, said) = format_stop_summary(&stopped, &failed, &programs, keep_tab);
                    let ok = failed.is_empty();
                    let mut payload = json!({
                        "ok": ok, "id": target, "name": Value::Null, "stopped": stopped,
                        "ended": ended, "programs": programs,
                    });
                    if !failed.is_empty() {
                        payload["failed"] = json!(failed);
                    }
                    return Response {
                        ok,
                        stdout: if as_json { format!("{payload}\n") } else { said },
                        stderr: if ok { String::new() } else { format!("olo agent stop: failed to close {} tab(s)\n", failed.len()) },
                        code: i32::from(!ok),
                        data: payload,
                    };
                }
                return Response::err("stop", e, 1);
            }
        };
        // (Its conversation's id and where it is written, while its agent still runs.)
        if entry.cli_session_id.is_none() || entry.transcript_path.is_none() {
            let _ = self.inspect(std::slice::from_ref(&entry)).await;
        }
        let entry = self.entry(&entry.id).unwrap_or(entry);

        // All it showed, saved on this host before it is closed.
        let saved = match save.map(|p| if p.starts_with('/') || req.cwd.is_empty() { p } else { Path::new(&req.cwd).join(&p).display().to_string() }) {
            Some(dest) => {
                let text = self.rex.scrollback(&entry.session_id, &entry.block_id, "text").await.unwrap_or_default();
                let write_to = dest.clone();
                let written = tokio::task::spawn_blocking(move || {
                    let path = Path::new(&write_to);
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    let tmp = path.with_extension(format!("olo-tmp-{}", std::process::id()));
                    std::fs::write(&tmp, text)?;
                    std::fs::rename(&tmp, path)
                })
                .await;
                match written {
                    Ok(Ok(())) => Some(dest),
                    Ok(Err(e)) => return Response::err("stop", format!("couldn't save to {dest}: {e} (nothing was stopped)"), 1),
                    Err(e) => return Response::err("stop", format!("couldn't save to {dest}: {e} (nothing was stopped)"), 1),
                }
            }
            None => None,
        };

        // Each tab's own program, as it runs now (by its process and
        // when that started: its number given to another later isn't
        // it): what "stopped" must mean when this answers.
        // Stopping a shared minder (with or without `--tree`) NEVER
        // stops the agents nested under it: they move back under its
        // parent tab, unminded, and the lead is told.
        let stop_tree = tree && !entry.is_shared_minder();
        let mut running: Vec<(NativeSubagent, i32, u64)> = Vec::new();
        {
            let mut of = vec![entry.clone()];
            if stop_tree {
                let live = self.live_windows(&entry.session_id);
                of.extend(self.lock().reg.descendants_tree(HOST, &entry.session_id, &entry.window_id, &live).into_iter().map(|r| r.entry));
            }
            let st = self.lock();
            for e in of.into_iter().filter(|e| e.exited_at.is_none()) {
                if let Some(pid) = st.live.get(&e.id).and_then(|l| l.pid).or(e.last_pid).filter(|p| *p > 1) {
                    let began = crate::panes::start_ticks(pid);
                    if began > 0 {
                        running.push((e, pid, began));
                    }
                }
            }
        }
        let (stopped, failed) = self.stop_tabs(&entry, force, stop_tree, keep_tab).await;
        // It answers once those programs have ended, or says which has
        // not, with its pid (the ShiftUp lead's item 35, 2026-11-26:
        // "`stop` answers only once the tab's own program has ended, or
        // says it has not, with its pid. Then a launcher doesn't have to
        // watch the program go itself"). A tab closed is a program told
        // to go, not one gone: five seconds for it. (Kept open, its
        // program was only interrupted: it goes on, and is said so.)
        let pids: Vec<(i32, u64)> = running.iter().map(|(_, pid, began)| (*pid, *began)).collect();
        let still = still_running_after(&pids, if keep_tab { Duration::ZERO } else { STOPPED_WITHIN }).await;
        let programs: Vec<Value> = running.iter().zip(&still).map(|((e, pid, _), still)| json!({ "id": e.id, "name": e.name, "pid": pid, "ended": !still })).collect();
        let (ended, said) = format_stop_summary(&stopped, &failed, &programs, keep_tab);
        let ok = failed.is_empty();
        let mut payload = json!({
            "ok": ok, "id": entry.id, "name": entry.name, "stopped": stopped,
            // (Whether every program that ran in what was stopped has ended; each, with its pid.)
            "ended": ended, "programs": programs,
            "cli_session_id": entry.effective_session_id(), "transcript_path": entry.transcript_path, "saved_path": saved,
        });
        if !failed.is_empty() {
            payload["failed"] = json!(failed);
        }
        Response {
            ok,
            stdout: if as_json { format!("{payload}\n") } else { said },
            stderr: if ok { String::new() } else { format!("olo agent stop: failed to close {} tab(s)\n", failed.len()) },
            code: i32::from(!ok),
            data: payload,
        }
    }

    /// Interrupts a child (and, with `tree`, those under it, deepest
    /// first) and closes its tab. Stopping a shared minder directly
    /// never stops its minded agents (even if `tree` is true) and lets
    /// `minding_ends` escalate to the lead on each live agent; stopping
    /// an ancestor with `tree` stops everything under it quietly.
    pub async fn stop_tabs(self: &Arc<Self>, entry: &NativeSubagent, force: bool, tree: bool, keep_tab: bool) -> (Vec<String>, Vec<Value>) {
        let quiet_minders = !entry.is_shared_minder();
        let tree = tree && !entry.is_shared_minder();
        self.stop_tabs_inner(entry, force, tree, keep_tab, quiet_minders).await
    }

    pub async fn stop_tabs_inner(self: &Arc<Self>, entry: &NativeSubagent, force: bool, tree: bool, keep_tab: bool, quiet_minders: bool) -> (Vec<String>, Vec<Value>) {
        let live = self.live_windows(&entry.session_id);
        let mut targets: Vec<NativeSubagent> = Vec::new();
        if tree {
            let mut under = self.lock().reg.descendants_tree(HOST, &entry.session_id, &entry.window_id, &live);
            under.reverse();
            targets.extend(under.into_iter().map(|r| r.entry));
        }
        targets.push(entry.clone());
        self.stop_target_list(&targets, force, keep_tab, quiet_minders).await
    }

    async fn stop_target_list(self: &Arc<Self>, targets: &[NativeSubagent], force: bool, keep_tab: bool, quiet_minders: bool) -> (Vec<String>, Vec<Value>) {
        // Scope `let_go` and `closing_minders` strictly to this active
        // non-`keep_tab` close call so kept tabs and failed closes never
        // leave stale suppression marks behind.
        let _scope = StopScopeGuard::enter(self.clone(), targets, keep_tab, quiet_minders);
        let quiet_minder_ids: HashSet<String> = _scope.let_go_ids.iter().cloned().collect();
        if !force {
            // Twice, a moment apart: an agent takes the first as "stop
            // what you're doing" and the second as "leave".
            for round in 0..2 {
                for t in targets.iter().filter(|t| t.exited_at.is_none()) {
                    let _ = self.rex.write(&t.session_id, &t.block_id, "\x03").await;
                }
                tokio::time::sleep(Duration::from_millis(if round == 0 { 150 } else { 250 })).await;
            }
        }
        let mut stopped = Vec::with_capacity(targets.len());
        let mut failed: Vec<Value> = Vec::new();
        let mut late_survivors: Vec<String> = Vec::new();
        for t in targets {
            if keep_tab {
                stopped.push(t.id.clone());
                continue;
            }
            match self.rex.call("session.close_window", json!({ "session_id": t.session_id, "window_id": t.window_id })).await {
                Ok(_) => {
                    // (Said now: the server's own word of it follows, and says nothing new.)
                    self.exit(&t.id, None);
                    stopped.push(t.id.clone());
                }
                Err(e) if e.gone() => {
                    self.exit(&t.id, None);
                    stopped.push(t.id.clone());
                }
                Err(e) => {
                    match self.tab_presence(t).await {
                        super::service::TabPresence::Gone => {
                            self.exit(&t.id, None);
                            stopped.push(t.id.clone());
                        }
                        presence => {
                            // Tab is still present or presence could not be verified
                            // (transient RPC error): keep the record live, un-suppress
                            // escalation, and report failure.
                            {
                                let mut st = self.lock();
                                for workers in st.let_go.values_mut().flatten() {
                                    workers.remove(&t.id);
                                }
                            }
                            if let Some(mid) = &t.minded_by
                                && quiet_minder_ids.contains(mid)
                                && self.entry(mid).is_some_and(|m| m.exited_at.is_some())
                            {
                                late_survivors.push(t.id.clone());
                            }
                            let why = match presence {
                                super::service::TabPresence::Unknown(check_err) => format!("{e}; presence check failed: {check_err}"),
                                _ => e.to_string(),
                            };
                            failed.push(json!({ "id": t.id, "name": t.name, "window_id": t.window_id, "error": why }));
                        }
                    }
                }
            }
        }
        drop(_scope);
        if !late_survivors.is_empty() {
            self.escalate_minder_ended(&late_survivors, "exited while its ancestor stop failed to close this agent");
        }
        (stopped, failed)
    }

    // ---- prune, focus ----

    /// `olo agent prune`: the records of children that have exited,
    /// taken out of the list. **Of the asking tab's own children, and of
    /// nobody else's.** It used to take every exited record of the
    /// whole session, and two leads' tabs are one session: a tidying
    /// after one check child took 124 records with it, most of them
    /// another lead's, their conversations' ids among them (2026-11-27,
    /// 00:50 UTC; 120 were rebuilt from the event log, four were not).
    /// What isn't the asker's own is taken only when asked for by what
    /// it is: `--session` (every tab's of this session), `--all` (every
    /// session's).
    fn prune(&self, raw: &Value, caller: &Caller) -> Response {
        if caller.child.is_some() {
            return Response::err("prune", "refused from child subagent tab", 69);
        }
        let (mut all, mut session, mut as_json) = (false, false, false);
        if let Some(args) = args_of(raw) {
            for a in &args {
                match a.as_str() {
                    "--all" | "-a" => all = true,
                    "--session" => session = true,
                    "--json" => as_json = true,
                    other => {
                        return Response::err(
                            "prune",
                            format!(
                                "unknown option `{other}`\nusage: olo agent prune [--session | --all] [--json]   (this tab's own exited children; --session: every tab's of this session; --all: every session's)"
                            ),
                            2,
                        );
                    }
                }
            }
        } else {
            all = raw["all"].as_bool().unwrap_or(false);
            session = raw["session"].as_bool().unwrap_or(false);
            as_json = raw["json"].as_bool().unwrap_or(false);
        }
        let (gone, others): (Vec<String>, usize) = {
            let mut st = self.lock();
            // (Under the asking tab: its child, or a child of one, by
            // the records' own tabs, whether those still stand or not.)
            let under = |e: &NativeSubagent, tab: &str| {
                let mut parent = e.parent_window_id.as_str();
                for _ in 0..32 {
                    if parent == tab {
                        return true;
                    }
                    match st.reg.entries.iter().find(|p| p.host_key == e.host_key && p.session_id == e.session_id && p.window_id == parent) {
                        Some(above) => parent = above.parent_window_id.as_str(),
                        None => return false,
                    }
                }
                false
            };
            let exited: Vec<&NativeSubagent> = st.reg.entries.iter().filter(|e| e.host_key == HOST && e.exited_at.is_some()).collect();
            let gone: Vec<String> = exited
                .iter()
                .filter(|e| {
                    if all {
                        true
                    } else if session {
                        caller.session.as_deref() == Some(e.session_id.as_str())
                    } else {
                        caller.session.as_deref() == Some(e.session_id.as_str()) && caller.window.as_deref().is_some_and(|tab| under(e, tab))
                    }
                })
                .map(|e| e.id.clone())
                .collect();
            let others = exited.len() - gone.len();
            st.reg.entries.retain(|e| !gone.contains(&e.id));
            for id in &gone {
                st.live.remove(id);
            }
            (gone, others)
        };
        if !gone.is_empty() {
            self.saved();
            for id in &gone {
                self.said(Frame::Gone { id: id.clone() });
            }
        }
        let whose = if all {
            "of every session"
        } else if session {
            "of this session, every tab's"
        } else {
            "of this tab's own children"
        };
        let payload = json!({ "ok": true, "pruned": gone.len(), "scope": if all { "all" } else if session { "session" } else { "own" }, "left": others });
        let left = if others > 0 && !all { format!(" {others} of other tabs' are left as they are.") } else { String::new() };
        Response::ok(if as_json { format!("{payload}\n") } else { format!("Pruned {} exited subagent record(s) {whose}.{left}\n", gone.len()) }, payload)
    }

    async fn focus(&self, raw: &Value, caller: &Caller) -> Response {
        let target = raw.get("target").or_else(|| raw.get("id")).and_then(Value::as_str).map(str::to_owned).or_else(|| args_of(raw).and_then(|a| a.into_iter().find(|s| !s.starts_with('-'))));
        let Some(target) = target else {
            return Response { ok: false, stdout: String::new(), stderr: "usage: olo agent focus <ID|NAME>\n".into(), code: 2, data: Value::Null };
        };
        let entry = match self.resolve(caller, &target) {
            Ok(e) => e,
            Err(e) => return Response::err("focus", e, 1),
        };
        if let Err(e) = self.rex.call("session.focus_window", json!({ "session_id": entry.session_id, "window_id": entry.window_id })).await {
            return Response::err("focus", e.to_string(), 1);
        }
        Response::ok(format!("Focused {} ({}).\n", entry.id, entry.display_title()), json!({ "ok": true, "id": entry.id, "name": entry.name, "window_id": entry.window_id }))
    }
}

/// Computes `opts["path_prepend"]` for `session.new_window`:
/// - If the caller explicitly set or unset `PATH` (`--env PATH=…` / `--unset-env PATH`),
///   returns empty so the caller's explicit `PATH` wins.
/// - Preserves the caller's exact `PATH` order across `PATH_PREPEND` and
///   `PATH_EXTRA` (never hoisting bridge dirs ahead of non-bridge dirs under
///   `DesktopPolicy::Inherit`).
/// - Rejects/drops relative entries and bare `.`, lexically normalizes `.`/`..`,
///   and under `DesktopPolicy::None` / `Screens` defense-in-depth strips both
///   lexical and canonical desktop-bridge directories (`!is_bridge_path_dir`).
fn inherited_path_prepend(desktop: &DesktopPolicy, inherit: &[(String, String)], explicit_path: bool) -> Vec<String> {
    if explicit_path {
        return Vec::new();
    }
    let allow_bridge = matches!(desktop, DesktopPolicy::Inherit);
    let mut out = Vec::new();
    for (name, value) in inherit {
        let include = match name.as_str() {
            "PATH_PREPEND" => allow_bridge,
            "PATH_EXTRA" => true,
            _ => false,
        };
        if !include || value.is_empty() {
            continue;
        }
        let p = Path::new(value);
        if !p.is_absolute() {
            continue;
        }
        let (resolved, lexical) = match std::fs::canonicalize(p) {
            Ok(c) => (c.clone(), c),
            Err(_) => {
                let lex = super::client::normalize_lexical_path(p);
                if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) { (p.to_path_buf(), lex) } else { (lex.clone(), lex) }
            }
        };
        let norm = resolved.to_string_lossy().into_owned();
        if norm.is_empty() {
            continue;
        }
        if !allow_bridge && (super::client::is_bridge_path_dir(&norm) || super::client::is_bridge_path_dir(&lexical.to_string_lossy()) || super::client::is_bridge_path_dir(value)) {
            continue;
        }
        if !out.iter().any(|d| d == &norm) {
            out.push(norm);
        }
    }
    out
}

/// True when the bottom of `screen` positively shows an agent's post-refusal
/// reason input prompt (e.g., Claude's `"Tell Claude what to do differently:"`
/// with an actual unnumbered input cursor line `>` / `❯` / `›`, or inline
/// placeholder `"❯ Tell Claude what to do differently"`) with NO approval,
/// approval fragment, trust, survey, feedback draft, or interactive chooser.
/// Plain prose containing `"what to do differently"` with no input field chrome
/// is strictly rejected.
fn is_refusal_reason_field(agent: olo_agent_screen::Agent, screen: &str, cls: &ScreenClassification) -> bool {
    if cls.approval.is_some()
        || cls.folder_trust.is_some()
        || cls.approval_fragment
        || cls.state == SubagentLiveState::WaitingQuestion
        || cls.state == SubagentLiveState::Exited
        || olo_agent_screen::parse_survey(agent, screen).is_some()
        || olo_agent_screen::has_survey_fragment(agent, screen)
        || olo_agent_screen::feedback_draft(agent, screen).is_some()
    {
        return false;
    }
    let bottom: Vec<&str> = screen.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    // Never accept arbitrary markdown `> prose` lines as an input cursor for
    // Claude/Codex: require real input chrome (`❯`, `›`, `│ ❯`, `│ >`), or an
    // empty/placeholder `>` (or Antigravity prompt `>`).
    fn strip_input_cursor(agent: olo_agent_screen::Agent, l: &str) -> Option<&str> {
        let rest = l.strip_prefix("│ ❯").or_else(|| l.strip_prefix("│ >")).or_else(|| l.strip_prefix('❯')).or_else(|| l.strip_prefix('›')).or_else(|| {
            let r = l.strip_prefix('>')?;
            let tr = r.trim();
            let empty_or_agy = tr.is_empty() || agent == olo_agent_screen::Agent::Antigravity;
            empty_or_agy.then_some(r)
        })?;
        let trimmed = rest.trim_start();
        let is_numbered = trimmed.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()));
        (!is_numbered).then_some(trimmed)
    }
    let is_reason_cue = |l: &str| -> bool {
        let t = l.trim();
        if t.starts_with(['●', '•', '⎿', '│']) {
            return false;
        }
        let low = t.to_ascii_lowercase();
        (low.starts_with("tell ") && (low.contains("what to do") || low.contains("instead") || low.contains("why")))
            || (low.starts_with("what should ") && (low.contains("do instead") || low.contains("do differently")))
            || low.starts_with("what to do differently")
            || low.starts_with("reason:")
            || low.starts_with("feedback:")
    };
    for (i, line) in bottom.iter().enumerate() {
        if let Some(after_cursor) = strip_input_cursor(agent, line)
            && is_reason_cue(after_cursor)
        {
            return true;
        }
        if is_reason_cue(line) && bottom[i + 1..].iter().any(|next| strip_input_cursor(agent, next).is_some()) {
            return true;
        }
    }
    false
}

/// True ONLY when `now` is a fresh, live, ordered reading positively showing a
/// safe **empty** (or placeholder-only) text input target for `--no --reason`
/// before pasting (never pasting into a chat box holding an existing user draft!).
fn is_safe_reason_target_for_paste(agent: olo_agent_screen::Agent, now: &super::service::Looked) -> bool {
    is_safe_reason_target(agent, now) && input_field_is_empty_or_placeholder(&now.screen)
}

/// True ONLY when `now` is a fresh, live, ordered safe reason target AND the
/// whole visible input field equals the requested `reason` (whitespace-collapsed,
/// never accepting an unrelated `[Pasted text #` or combined user draft — Astra #6).
fn is_safe_reason_target_for_enter(agent: olo_agent_screen::Agent, now: &super::service::Looked, reason: &str) -> bool {
    if !is_safe_reason_target(agent, now) {
        return false;
    }
    let Some((field, pasted)) = extract_input_field(&now.screen) else {
        return false;
    };
    if pasted {
        return false;
    }
    let want = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    !want.is_empty() && field == want
}

/// True ONLY when `now` is a fresh, live, ordered reading positively showing a
/// safe text input target for `--no --reason` (either the agent's own chat
/// prompt or its post-refusal reason field — never a blank repaint, chooser,
/// approval, approval fragment, folder trust, survey, or feedback draft).
fn is_safe_reason_target(agent: olo_agent_screen::Agent, now: &super::service::Looked) -> bool {
    if !now.is_fresh_ordered()
        || now.survey
        || now.survey_pending
        || now.cls.state == SubagentLiveState::Exited
        || now.cls.approval.is_some()
        || now.cls.folder_trust.is_some()
        || now.cls.approval_fragment
        || now.cls.state == SubagentLiveState::WaitingQuestion
        || olo_agent_screen::parse_survey(agent, &now.screen).is_some()
        || olo_agent_screen::has_survey_fragment(agent, &now.screen)
        || olo_agent_screen::feedback_draft(agent, &now.screen).is_some()
    {
        return false;
    }
    olo_agent_screen::at_prompt(agent, &now.screen) || is_refusal_reason_field(agent, &now.screen, &now.cls)
}

#[cfg(test)]
mod ops_tests {
    use super::*;

    #[test]
    fn non_bridge_path_preserved_under_desktop_none_and_bridge_dirs_stripped() {
        let dir = PathBuf::from(format!("/var/tmp/olo-ops-path-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let real_bridge = dir.join("state/rex-commands");
        let symlink_bridge = dir.join("bridge-alias");
        let sdk_real_bin = dir.join("opt/sdk/bin");
        let sdk_cur = dir.join("opt/sdk/current");
        let work_tools = dir.join("work/tools");
        std::fs::create_dir_all(&real_bridge).unwrap();
        std::fs::create_dir_all(&sdk_real_bin).unwrap();
        std::fs::create_dir_all(&sdk_cur).unwrap();
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::os::unix::fs::symlink(&real_bridge, &symlink_bridge).unwrap();
        std::os::unix::fs::symlink(&sdk_cur, &work_tools).unwrap();
        let symlink_dotdot_bin = work_tools.join("../bin");

        let inherit = vec![
            ("PATH_EXTRA".to_owned(), "/home/u/.local/bin".to_owned()),
            ("PATH_PREPEND".to_owned(), "/home/u/.local/share/olo/rex/bin".to_owned()),
            ("PATH_EXTRA".to_owned(), "/home/u/.cargo/bin".to_owned()),
            ("PATH_PREPEND".to_owned(), "/run/user/1000/rex-commands".to_owned()),
            // Relative entries, bare `.`, lexical `..` bridge paths, and symlinks to bridge dirs
            // are all normalized/fenced defense-in-depth under `--desktop none`!
            // A32: `/work/tools/../bin` (where `/work/tools -> /opt/sdk/current`) MUST resolve
            // through the real symlink target to `/opt/sdk/bin` (NOT `/work/bin`)!
            ("PATH_EXTRA".to_owned(), ".".to_owned()),
            ("PATH_EXTRA".to_owned(), "relative/bin".to_owned()),
            ("PATH_EXTRA".to_owned(), "/var/tmp/fake/rex-commands/./sub/..".to_owned()),
            ("PATH_EXTRA".to_owned(), symlink_bridge.display().to_string()),
            ("PATH_EXTRA".to_owned(), symlink_dotdot_bin.display().to_string()),
        ];
        let canon_sdk_bin = std::fs::canonicalize(&sdk_real_bin).unwrap().to_string_lossy().into_owned();
        let canon_bridge = std::fs::canonicalize(&real_bridge).unwrap().to_string_lossy().into_owned();
        // Under `--desktop none`: bridge dirs (including lexical `..` and symlink) & relative entries stripped; user CLI dirs preserved in order!
        assert_eq!(inherited_path_prepend(&DesktopPolicy::None, &inherit, false), vec!["/home/u/.local/bin", "/home/u/.cargo/bin", &canon_sdk_bin]);
        // Under `--desktop inherit`: exact original caller precedence across PATH_EXTRA and PATH_PREPEND is preserved (bridge dirs are NOT hoisted ahead of earlier non-bridge dirs)!
        assert_eq!(
            inherited_path_prepend(&DesktopPolicy::Inherit, &inherit, false),
            vec![
                "/home/u/.local/bin".to_owned(),
                "/home/u/.local/share/olo/rex/bin".to_owned(),
                "/home/u/.cargo/bin".to_owned(),
                "/run/user/1000/rex-commands".to_owned(),
                "/var/tmp/fake/rex-commands/./sub/..".to_owned(),
                canon_bridge,
                canon_sdk_bin,
            ]
        );
        // Explicit `--env PATH=...` or `--unset-env PATH` disables implicit PATH_PREPEND/PATH_EXTRA.
        assert!(inherited_path_prepend(&DesktopPolicy::None, &inherit, true).is_empty());
        assert!(inherited_path_prepend(&DesktopPolicy::Inherit, &inherit, true).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reason_target_rejects_blank_chooser_survey_and_accepts_prompt_or_reason_field() {
        let make_looked = |screen: &str, state: SubagentLiveState, ordered: bool| super::super::service::Looked {
            cls: ScreenClassification { state, summary: None, approval: None, folder_trust: None, api_error: None, usage: None, in_turn: false, recognized_question: false, approval_fragment: false },
            screen: screen.to_owned(),
            last_output_at: 1,
            last_state_change_at: 1,
            pid: None,
            survey: false,
            survey_pending: false,
            boxed: None,
            expected: None,
            reports: false,
            expects: None,
            live: true,
            ordered,
            rpc_failed: false,
            seq: 1,
            pre_read_out_seq: 0,
        };
        let rule = "─".repeat(40);
        let at_chat = make_looked(&format!("{rule}\n❯ \n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(is_safe_reason_target(olo_agent_screen::Agent::Claude, &at_chat));
        // Superseded/unordered read is NEVER safe.
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked(&at_chat.screen, SubagentLiveState::Idle, false)));
        // Blank repaint is NEVER safe.
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("", SubagentLiveState::Working, true)));
        // Ordinary chooser Q (`WaitingQuestion`) is NEVER safe.
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("Which branch?\n ❯ 1. main\n   2. dev\n Enter to select", SubagentLiveState::WaitingQuestion, true)));
        // Plain prose containing "what to do differently" with NO input field chrome is NEVER safe!
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("The report explains what to do differently.\n", SubagentLiveState::Idle, true)));
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("● Tell Claude what to do differently in the docs.\n", SubagentLiveState::Idle, true)));
        // A numbered refusal choice line is NEVER a reason input field!
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("❯ 3. No, and tell Claude what to do differently (esc)\n", SubagentLiveState::Idle, true)));
        // Genuine refusal reason field (cue + input cursor, or cursor with placeholder cue) IS safe.
        assert!(is_safe_reason_target_for_paste(olo_agent_screen::Agent::Claude, &make_looked("Tell Claude what to do differently:\n> ", SubagentLiveState::Idle, true)));
        assert!(is_safe_reason_target_for_paste(olo_agent_screen::Agent::Claude, &make_looked("❯ Tell Claude what to do differently", SubagentLiveState::Idle, true)));
        // A25 / N11: Markdown blockquote `> prose` after `Reason:` is NEVER a Claude refusal reason field!
        assert!(!is_safe_reason_target(olo_agent_screen::Agent::Claude, &make_looked("Reason: check failed\n> expected 42, got 0\n", SubagentLiveState::Idle, true)));
        // A25: Occupied chat box with an existing unsent user draft is NEVER safe for reason paste!
        let occupied_chat = make_looked(&format!("{rule}\n❯ Existing unsent user draft\n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(!is_safe_reason_target_for_paste(olo_agent_screen::Agent::Claude, &occupied_chat));
        // Astra #6: User draft starting with `"Ask "` is NEVER a CLI placeholder!
        let ask_owner_draft = make_looked(&format!("{rule}\n❯ Ask the release owner first\n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(!is_safe_reason_target_for_paste(olo_agent_screen::Agent::Claude, &ask_owner_draft));
        // A25 / Astra #6: Pre-Enter verification requires whole-field equality with the pasted
        // reason (rejects combined drafts and ambiguous collapsed `[Pasted text #` markers!).
        let pasted_reason = make_looked(&format!("{rule}\n❯ use jato instead\n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(is_safe_reason_target_for_enter(olo_agent_screen::Agent::Claude, &pasted_reason, "use jato instead"));
        assert!(!is_safe_reason_target_for_enter(olo_agent_screen::Agent::Claude, &at_chat, "use jato instead"));
        let combined_draft = make_looked(&format!("{rule}\n❯ Ask the release owner first use jato instead\n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(!is_safe_reason_target_for_enter(olo_agent_screen::Agent::Claude, &combined_draft, "use jato instead"));
        let collapsed_paste = make_looked(&format!("{rule}\n❯ [Pasted text #1 +5 lines]\n{rule}\n"), SubagentLiveState::Idle, true);
        assert!(!is_safe_reason_target_for_enter(olo_agent_screen::Agent::Claude, &collapsed_paste, "use jato instead"));
    }

    #[test]
    fn paste_settle_scales_and_still_typed_handles_tall_drafts_vs_echoed_submissions() {
        assert_eq!(paste_settle_delay(50), Duration::from_millis(80));
        assert_eq!(paste_settle_delay(4096), Duration::from_millis(160));
        assert_eq!(paste_settle_delay(50_000), Duration::from_millis(350));

        let rule = "─".repeat(50);
        // 1. Tall 35-line unsubmitted draft (>29 lines, Neon #4) inside ruled input box IS VisibleHasDraft!
        let mut tall_draft = format!("● Previous output\n{rule}\n❯ First line of tall prompt\n");
        for i in 2..=35 {
            tall_draft.push_str(&format!("  line {i} of pasted prompt\n"));
        }
        tall_draft.push_str(&format!("{rule}\n  ? for shortcuts\n"));
        assert_eq!(classify_input_field(&tall_draft, "First line of tall prompt"), FieldState::VisibleHasDraft);
        assert!(still_typed(&tall_draft, "First line of tall prompt"));

        // 1b. Neon #4 / Astra #15: Undrawn field during repaint is `Undrawn`, NEVER `VisibleCleared`!
        let undrawn_repaint = "✻ Thinking… (4s)\n  Working on task\n";
        assert_eq!(classify_input_field(undrawn_repaint, "Fix bug"), FieldState::Undrawn);

        // 1c. Astra #15: Short `"yes"` does NOT match unrelated `"yes please review the PR"` or unrelated `[Pasted text #`!
        let other_draft = format!("{rule}\n❯ yes please review the PR\n{rule}\n");
        assert_eq!(classify_input_field(&other_draft, "yes"), FieldState::VisibleDifferentDraft);
        let unrelated_paste = format!("{rule}\n❯ [Pasted text #1 +5 lines]\n{rule}\n");
        assert_eq!(classify_input_field(&unrelated_paste, "yes"), FieldState::VisibleDifferentDraft);

        // 2. Same prompt AFTER submission (echoed above spinner, input box now empty `❯ `) MUST return false!
        let submitted_working = format!("❯ First line of tall prompt\n  line 2\n✻ Thinking… (2s)\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        assert!(!still_typed(&submitted_working, "First line of tall prompt"));

        // 3. Working agent with an UNSUBMITTED draft still sitting inside the ruled input box MUST return true!
        let working_with_draft = format!("✻ Thinking… (2s)\n{rule}\n❯ Queued unsubmitted draft\n  second line\n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        assert!(still_typed(&working_with_draft, "Queued unsubmitted draft"));

        // 4. N4 / A22: Short (<4 char) message ("yes"), leading blank line, wrapped long first line,
        //    and Claude's collapsed `[Pasted text #1 +5 lines]` MUST ALL return `still_typed == true`!
        let short_draft = format!("{rule}\n❯ yes\n{rule}\n  ? for shortcuts\n");
        assert!(still_typed(&short_draft, "yes"));
        let leading_blank_draft = format!("{rule}\n❯ Run tests now\n{rule}\n");
        assert!(still_typed(&leading_blank_draft, "\n\nRun tests now"));
        let wrapped_draft = format!("{rule}\n❯ Please inspect why the auth middleware rejects\n  valid refresh tokens and add a regression test.\n{rule}\n");
        assert!(still_typed(&wrapped_draft, "Please inspect why the auth middleware rejects valid refresh tokens and add a regression test."));
        let collapsed_paste = format!("✻ Thinking… (2s)\n{rule}\n❯ [Pasted text #1 +5 lines]\n{rule}\n");
        assert!(still_typed(&collapsed_paste, "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6"));

        // 5. Unruled Codex screen: tall unsubmitted draft returns true, echoed submitted message above empty prompt returns false!
        let codex_draft = "› Fix the parser\n  line 2\n  > quoted block\n  line 4\n  line 5\n  line 6\n  line 7\n  line 8\n";
        assert!(still_typed(codex_draft, "Fix the parser"));
        let codex_submitted =
            "› Fix the parser\n• Working (2s • esc to interrupt)\n› Ask Codex to do anything\n\n  gpt-5.4-codex high · /var/tmp/work · Fix the parser\n  ← for agents · ? for shortcuts\n";
        assert!(!still_typed(codex_submitted, "Fix the parser"));
        assert!(input_field_is_empty_or_placeholder(codex_submitted));
        assert_eq!(extract_input_field(codex_submitted), Some(("Ask Codex to do anything".into(), false)));

        // 6. Antigravity CLI mode placeholder is recognized as empty/placeholder, while a user draft is rejected!
        let agy_placeholder = format!("{rule}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{rule}\n? for shortcuts\n");
        assert!(input_field_is_empty_or_placeholder(&agy_placeholder));
        let agy_user_draft = format!("{rule}\n> Accept-edits mode note: fix bug\n{rule}\n? for shortcuts\n");
        assert!(!input_field_is_empty_or_placeholder(&agy_user_draft));
    }
}
