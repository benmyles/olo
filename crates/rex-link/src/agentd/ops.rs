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
    AgentCli, ApproveSpec, DesktopPolicy, NativeSubagent, ScreenClassification, StartSpec, SubagentEvent, SubagentLiveState, WatchSpec, approval_to_json, effective_state, new_subagent_id,
    now_unix_ms, parse_send_payload, sanitize_paste_text, summary_json, to_unix_ms,
};
use serde_json::{Value, json};

use super::service::{HOST, Service, tail_of};

/// Whether an agent that now stands as `now` says still stands as the
/// event `last` said of it: the same question (by its key), still at
/// rest, still failed; its end stands for good.
fn still_stands(last: &SubagentEvent, now: &ScreenClassification) -> bool {
    match last.state {
        SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion => now.state == last.state && super::service::subkey(now) == last.dedupe_key,
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

/// Whether the first line of what was sent is still in the agent's
/// input box.
fn still_typed(screen: &str, first_line: Option<&str>) -> bool {
    first_line.is_some_and(|fl| {
        screen.lines().rev().take(6).any(|l| {
            let t = l.trim();
            (t.starts_with('❯') || t.starts_with('>') || t.starts_with('›')) && t.contains(fl)
        })
    })
}

impl Service {
    pub async fn agent_op(self: &Arc<Self>, req: &Request, peer_pid: Option<i32>) -> Response {
        let raw = raw_of(req);
        let caller = self.caller(req, peer_pid).await;
        // A minder has the few commands that are of the agent it minds;
        // nothing else is its to ask, whatever it was told or tried.
        if caller.child.as_ref().is_some_and(|c| c.minds.is_some()) && !matches!(req.op.as_str(), "status" | "read" | "tail" | "watch" | "wait" | "send" | "input" | "approve" | "escalate") {
            return Response::err(&req.op, "a minder watches, reads, answers and escalates for the one agent it minds: this isn't its to ask", 69);
        }
        match req.op.as_str() {
            "start" | "spawn" => self.start(req, &raw, &caller).await,
            "list" | "ls" => self.list(&raw, &caller).await,
            "status" => self.status(&raw, &caller).await,
            "read" | "tail" => self.read(&raw, &caller).await,
            "watch" | "wait" => self.watch(&raw, &caller).await,
            "send" | "input" => self.send(&raw, &caller).await,
            "approve" => self.approve(&raw, &caller).await,
            "stop" | "close" | "kill" => self.stop(req, &raw, &caller).await,
            "prune" => self.prune(&raw, &caller),
            "focus" => self.focus(&raw, &caller).await,
            "mind" | "minder" => self.mind(req, &raw, &caller).await,
            "escalate" => self.escalate(&raw, &caller).await,
            other => Response {
                ok: false,
                stdout: String::new(),
                stderr: format!("olo agent: unknown command `{other}`\nusage: olo agent <start|list|status|read|watch|send|approve|mind|escalate|stop|prune|focus> [ARGS…]\n"),
                code: 2,
                data: Value::Null,
            },
        }
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
            return Caller { session: Some(child.session_id.clone()), window: Some(child.window_id.clone()), block: Some(child.block_id.clone()), child: Some(child) };
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
        let window = session.as_deref().and_then(|sid| {
            let st = self.lock();
            let sess = st.sessions.get(sid)?;
            claimed.and_then(|b| sess.blocks.get(b).cloned()).or_else(|| sess.active.clone())
        });
        Caller { session, window, block: claimed.map(str::to_owned), child: None }
    }

    /// The session asked from and the tab asked about (`--parent`, else
    /// the asker's own).
    async fn place(&self, caller: &Caller, explicit_parent: Option<&str>) -> Result<(String, String), String> {
        let sid = match &caller.session {
            Some(sid) => sid.clone(),
            None => self.session_ids().await.map_err(|e| e.to_string())?.into_iter().next().ok_or_else(|| "no Rex session found on this host".to_owned())?,
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

    fn live_windows(&self, sid: &str) -> HashSet<String> {
        self.lock().sessions.get(sid).map(|s| s.windows.clone()).unwrap_or_default()
    }

    fn resolve(&self, caller: &Caller, target: &str) -> Result<NativeSubagent, String> {
        // A minder names one agent: the one it minds (its parent tab,
        // which no other child may reach), and no other.
        if let Some(minded) = caller.child.as_ref().and_then(|c| c.minds.as_deref()) {
            let it = self.entry(minded).ok_or_else(|| "the agent this minder minds is no longer known".to_owned())?;
            let t = target.trim();
            if t == it.id || t == it.window_id || it.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(t)) {
                return Ok(it);
            }
            return Err(format!("a minder reaches the agent it minds (`{}`) and no other: not `{t}`", it.name.as_deref().unwrap_or(&it.id)));
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

    /// What a minder did for its agent, kept beside its rules: a line
    /// each, for `olo agent mind NAME` to say.
    fn minder_did(&self, caller: &Caller, what: &str, said: Value) {
        let Some(minded) = caller.child.as_ref().and_then(|c| c.minds.clone()) else { return };
        use std::io::Write as _;
        // (`by`: which minder, where one was put in another's place.)
        let line = json!({ "at_ms": now_unix_ms(), "did": what, "said": said, "by": caller.child.as_ref().map(|c| c.id.clone()) }).to_string();
        let dir = self.paths.dir.join("minders").join(&minded);
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

    async fn start(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let spec = match StartSpec::parse(Some(raw)) {
            Ok(s) => s,
            Err(e) => return Response::err("start", e, 2),
        };
        // (Its minder's rules read before anything is started: a file
        // that isn't there is said now, not after the child runs.)
        let minder = match (&spec.minder, spec.rules.is_some() || spec.rules_file.is_some()) {
            (None, true) => return Response::err("start", "`--rules` are a minder's: say which with `--minder claude:MODEL:EFFORT`", 2),
            (None, false) => None,
            (Some(who), _) => match self.rules_given(req, spec.rules.as_deref(), spec.rules_file.as_deref()) {
                Ok(rules) => Some((who.clone(), rules.unwrap_or_default())),
                Err(e) => return Response::err("start", e, 2),
            },
        };
        let mut answer = self.start_spec(req, spec.clone(), caller).await;
        if let (true, Some((who, rules))) = (answer.ok, minder)
            && let Some(child) = answer.data["id"].as_str().and_then(|id| self.entry(id))
        {
            let from = answer.data["cursor"].as_u64().unwrap_or_else(|| self.cursor());
            let mut env = spec.env.clone();
            env.extend(spec.minder_env.iter().cloned());
            match self.attach_minder(req, caller, &child, &who, &rules, from, env, spec.unset_env.clone()).await {
                Ok(minder) => {
                    if !spec.json {
                        answer.stdout.push_str(&format!("{}  its minder ({})  tab={}\n", minder["id"].as_str().unwrap_or_default(), who.describe(), minder["window_id"].as_str().unwrap_or_default()));
                    }
                    answer.data["minder"] = minder;
                }
                // (The child runs: said, and its asking comes to the
                // parent as it would with no minder.)
                Err(e) => {
                    answer.stderr.push_str(&format!("olo agent start: `{}` runs, but its minder couldn't be started: {e}\n", child.name.as_deref().unwrap_or(&child.id)));
                    answer.data["minder_error"] = json!(e);
                }
            }
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

    /// Starts a minder for `child` (`agents::minder`): its own folder
    /// with the parent's rules and its one command, then the agent
    /// itself, a child tab of the one it minds, started so that the
    /// command needs no leave. What it is, for the asker.
    #[allow(clippy::too_many_arguments)]
    async fn attach_minder(
        self: &Arc<Self>,
        req: &Request,
        caller: &Caller,
        child: &NativeSubagent,
        who: &minder::MinderSpec,
        rules: &str,
        from: u64,
        env: Vec<(String, String)>,
        unset_env: Vec<String>,
    ) -> Result<Value, String> {
        if child.minds.is_some() {
            return Err("a minder isn't minded".into());
        }
        if child.exited_at.is_some() {
            return Err("it has exited".into());
        }
        let dir = self.paths.dir.join("minders").join(&child.id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let helper = std::env::current_exe().map_err(|e| format!("the helper's own path: {e}"))?;
        let command = dir.join(minder::COMMAND);
        let write = |path: &Path, text: &str, mode: u32| -> Result<(), String> {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(path, text).and_then(|()| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))).map_err(|e| format!("{}: {e}", path.display()))
        };
        write(&dir.join("rules.md"), rules, 0o600)?;
        write(&command, &minder::script(&helper.to_string_lossy(), &child.id), 0o700)?;
        let its_name = child.name.clone().unwrap_or_else(|| child.id.clone());
        // (And where it works: a minder is shown the command an agent
        // asks leave for and nothing of where it would run; rules that
        // speak of a folder are read against this.)
        let what = match (child.cli.is_agent(), child.cwd.as_deref().filter(|c| !c.is_empty())) {
            (true, Some(cwd)) => format!("{}, {}, working in {cwd}", child.cli.display_name(), child.model),
            (true, None) => format!("{}, {}", child.cli.display_name(), child.model),
            (false, Some(cwd)) => format!("a command, run in {cwd}"),
            (false, None) => "a command".to_owned(),
        };
        let spec = StartSpec {
            cli: Some(who.cli),
            model: Some(who.model.clone()),
            effort: Some(who.effort.clone()),
            name: Some(format!("{its_name}-minder")),
            title: Some(format!("Minder of {}", child.display_title())),
            role: Some("minder".to_owned()),
            parent_window: Some(child.window_id.clone()),
            // (One folder for every minder: Claude Code is told once
            // that it is trusted, not once a minder.)
            cwd: Some(self.paths.dir.join("minders").display().to_string()),
            trust: true,
            // Its one command and nothing else, in the shell or out of
            // it: no other tool, no server's tools, and no asking.
            permission_mode: Some(minder::CLAUDE_MODE.to_owned()),
            allowed_tools: vec![minder::allowed_tool(&command.to_string_lossy())],
            strict_mcp_config: true,
            extra: minder::CLAUDE_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
            append_system_prompt: Some(minder::brief(&its_name, &what, &command.to_string_lossy(), from, rules)),
            prompt: Some(minder::begin(&its_name, &command.to_string_lossy(), from)),
            env,
            unset_env,
            desktop: DesktopPolicy::None,
            close_on_exit: true,
            json: true,
            minds: Some(child.id.clone()),
            ..StartSpec::default()
        };
        let started = self.start_spec(req, spec, caller).await;
        if !started.ok {
            return Err(started.stderr.trim().trim_start_matches("olo agent start: ").to_owned());
        }
        Ok(json!({
            "id": started.data["id"], "name": started.data["name"], "window_id": started.data["window_id"],
            "cli": who.cli.binary(), "model": who.model, "effort": who.effort,
            "rules": dir.join("rules.md").display().to_string(),
        }))
    }

    /// `olo agent mind NAME …`: a minder attached to a running agent or
    /// put in another's place (`--with`), given new rules (`--rules`:
    /// a minder begun anew with them; what the one before answered is
    /// kept), ended (`--off`), or told of (nothing more).
    async fn mind(self: &Arc<Self>, req: &Request, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("mind", msg, code);
        let spec = match minder::MindSpec::parse(&args_of(raw).unwrap_or_default()) {
            Ok(s) => s,
            Err(e) => return err(e, 2),
        };
        let it = match self.resolve(caller, &spec.target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        let its_name = it.name.clone().unwrap_or_else(|| it.id.clone());
        let dir = self.paths.dir.join("minders").join(&it.id);
        let now = self.lock().reg.minder_of(&it.id).cloned();
        let told = |m: &NativeSubagent| json!({ "id": m.id, "name": m.name, "window_id": m.window_id, "cli": m.cli.binary(), "model": m.model, "effort": m.effort });
        // Told of: who minds it, by what rules, and what was answered.
        if !spec.off && spec.with.is_none() && spec.rules.is_none() && spec.rules_file.is_none() {
            let rules = std::fs::read_to_string(dir.join("rules.md")).ok();
            let answered: Vec<Value> = std::fs::read_to_string(dir.join("answers.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": now.as_ref().map(&told), "rules": rules, "answered": answered });
            let stdout = if spec.json {
                format!("{payload}\n")
            } else {
                let mut out = match &now {
                    Some(m) => format!("{its_name} is minded by {} ({}:{}:{})\n", m.name.as_deref().unwrap_or(&m.id), m.cli.binary(), m.model, m.effort),
                    None => format!("{its_name} has no minder\n"),
                };
                for a in answered.iter().rev().take(20).rev() {
                    out.push_str(&format!("  {}  {}\n", a["did"].as_str().unwrap_or_default(), a["said"]));
                }
                out
            };
            return Response::ok(stdout, payload);
        }
        if it.exited_at.is_some() {
            return err(format!("`{its_name}` has exited"), 1);
        }
        if spec.off {
            let Some(m) = now else { return err(format!("`{its_name}` has no minder"), 1) };
            self.stop_tabs(&m, true, true, false).await;
            let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": Value::Null, "stopped": m.id, "cursor": self.cursor() });
            return Response::ok(if spec.json { format!("{payload}\n") } else { format!("{its_name} is no longer minded (its asking comes to you)\n") }, payload);
        }
        // A minder, new or in another's place; or new rules for the one
        // it has, which is the same minder begun anew with them.
        let who = match (&spec.with, &now) {
            (Some(who), _) => who.clone(),
            (None, Some(m)) => minder::MinderSpec { cli: m.cli, model: m.model.clone(), effort: m.effort.clone() },
            (None, None) => return err(format!("`{its_name}` has no minder to give rules to: say which with `--with claude:MODEL:EFFORT`"), 2),
        };
        let rules = match self.rules_given(req, spec.rules.as_deref(), spec.rules_file.as_deref()) {
            Ok(Some(rules)) => rules,
            // (None given with `--with`: those it had, if it had any.)
            Ok(None) => std::fs::read_to_string(dir.join("rules.md")).unwrap_or_default(),
            Err(e) => return err(e, 2),
        };
        if let Some(old) = &now {
            self.stop_tabs(old, true, true, false).await;
        }
        let from = self.cursor();
        match self.attach_minder(req, caller, &it, &who, &rules, from, spec.env.clone(), Vec::new()).await {
            Ok(minder) => {
                let payload = json!({ "ok": true, "id": it.id, "name": it.name, "minder": minder, "replaced": now.as_ref().map(|m| m.id.clone()), "cursor": from });
                let stdout = if spec.json { format!("{payload}\n") } else { format!("{its_name} is minded by {} ({})\n", payload["minder"]["id"].as_str().unwrap_or_default(), who.describe()) };
                Response::ok(stdout, payload)
            }
            Err(e) => err(format!("`{its_name}`'s minder couldn't be started: {e}"), 1),
        }
    }

    /// `olo agent escalate "TEXT"`: something for the parent, on the
    /// parent's own watch at once (`event: escalation`), never typed
    /// into its pane. A minder's is of the agent it minds; any other
    /// child's is of itself (a child that needs its parent says so).
    async fn escalate(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("escalate", msg, code);
        let args = args_of(raw).unwrap_or_default();
        let json_out = args.iter().any(|a| a == "--json");
        let text = args.iter().filter(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>().join(" ");
        let text = text.trim();
        if text.is_empty() {
            return err("usage: olo agent escalate \"TEXT\" [--json]   (what the parent needs to know, on its own watch at once)".into(), 2);
        }
        let Some(child) = &caller.child else {
            return err("only a child has a parent to tell: this was asked from a tab that isn't one".into(), 69);
        };
        let about = match child.minds.as_deref() {
            Some(minded) => match self.entry(minded) {
                Some(it) => it,
                None => return err("the agent this minder minds is no longer known".into(), 1),
            },
            None => child.clone(),
        };
        // (How it stands as it is said: an event has a state.)
        let state = if about.exited_at.is_some() { SubagentLiveState::Exited } else { self.look(&about, false).await.cls.state };
        let said: String = text.chars().take(4000).collect();
        let mut event = self.event_of(&about, state, Some(said.clone()), String::new());
        event.event = minder::ESCALATION.to_owned();
        event.last_message = Some(said.clone());
        let seq = self.record_beside(event);
        self.minder_did(caller, "escalate", json!(said));
        // (A minder's word once its agent has ended is its report, and
        // its last: it is stopped when this has been answered.)
        if child.minds.is_some() && about.exited_at.is_some() {
            self.end_minder_in(child.clone(), Duration::from_millis(500));
        }
        let payload = json!({ "ok": true, "cursor": seq, "id": about.id, "name": about.name, "event": minder::ESCALATION });
        Response::ok(if json_out { format!("{payload}\n") } else { format!("told its parent (cursor={seq})\n") }, payload)
    }

    async fn start_spec(self: &Arc<Self>, req: &Request, mut spec: StartSpec, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("start", msg, code);
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
        let (sid, parent_win) = match self.place(caller, spec.parent_window.as_deref()).await {
            Ok(v) => v,
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
        let mut path_prepend: Vec<String> = Vec::new();
        if matches!(spec.desktop, DesktopPolicy::Inherit) {
            // (What the desktop gave the asking pane, a child that has the desktop's leave has too.)
            for (name, value) in &req.inherit {
                match name.as_str() {
                    "SSH_AUTH_SOCK" | "BROWSER" if !value.is_empty() && !launch.env.iter().any(|(k, _)| k == name) => launch.env.push((name.clone(), value.clone())),
                    "PATH_PREPEND" if !value.is_empty() => path_prepend.push(value.clone()),
                    _ => {}
                }
            }
        }
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
            return err("Rex created the tab but didn't say which it is".into(), 1);
        };
        if let Some(sess) = self.lock().sessions.get_mut(&sid) {
            sess.windows.insert(window_id.to_owned());
            sess.blocks.insert(block_id.to_owned(), window_id.to_owned());
        }
        // (A fork's conversation is a new one, found once its agent has made it.)
        let known = if spec.fork { spec.session_id.clone() } else { spec.session_id.clone().or_else(|| spec.resume.clone()) };
        let entry = NativeSubagent {
            id: new_subagent_id(),
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
            started_at: now_unix_ms() as i64,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: created["revision"].as_u64().unwrap_or(0),
            seen_live: true,
            exited_by_sync: false,
            add_dirs: spec.add_dirs.clone(),
            command: if cli == AgentCli::Command { raw_argv.clone() } else { Vec::new() },
            minds: spec.minds.clone(),
        };
        if let Err(e) = self.begin(entry.clone()).await {
            return err(format!("the new tab couldn't be watched: {e}"), 1);
        }
        // Started with a prompt, an agent works as soon as it is up: no
        // rest of its is its turn's end before then (two minutes for it
        // to come to it, after which it is told as it stands).
        if cli.is_agent() && spec.prompt.as_deref().is_some_and(|p| !p.trim().is_empty()) {
            self.expect_work(&entry, start_cursor, Duration::from_secs(120), false);
        }

        // Where the folder is asked about all the same, it is answered
        // by where the mark stands (never by timing), for the child's
        // own folders alone.
        if spec.trust && matches!(cli, AgentCli::Claude | AgentCli::Codex) {
            let (service, entry) = (self.clone(), entry.clone());
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                let mut attempts = 0u8;
                let own: Vec<String> = entry.cwd.iter().chain(entry.add_dirs.iter()).map(|d| d.trim_end_matches('/').to_owned()).collect();
                loop {
                    let changed = service.changed.notified();
                    let cls = service.look(&entry, false).await.cls;
                    if cls.state == SubagentLiveState::Exited {
                        break;
                    }
                    if let Some(trust) = cls.folder_trust {
                        let its_own = match trust.folder.as_deref().map(|f| f.trim_end_matches('/')) {
                            Some(f) => own.iter().any(|a| a == f),
                            None => !trust.is_add_dir,
                        };
                        if !its_own || attempts >= 4 {
                            break;
                        }
                        attempts += 1;
                        let delta = trust.trust_index as isize - trust.selected_index as isize;
                        let key = if delta > 0 { "\x1b[B" } else { "\x1b[A" };
                        for _ in 0..delta.unsigned_abs() {
                            let _ = service.rex.write(&entry.session_id, &entry.block_id, key).await;
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        if delta != 0 {
                            // (Answered only where the mark is seen to stand on it.)
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            let again = service.look(&entry, true).await.cls;
                            if !again.folder_trust.is_some_and(|t| t.selected_index == t.trust_index) {
                                continue;
                            }
                        }
                        let _ = service.rex.write(&entry.session_id, &entry.block_id, "\r").await;
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        service.poke(&entry.id);
                        continue;
                    }
                    if attempts > 0 && cls.state == SubagentLiveState::Working {
                        break;
                    }
                    tokio::select! {
                        _ = changed => {}
                        _ = tokio::time::sleep_until(deadline) => break,
                    }
                }
            });
        }

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
        Response::ok(stdout, payload)
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
        let live_windows = self.live_windows(&sid);
        let (mut rows, exited) = {
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
            let cls = self.look(&row.entry, false).await.cls;
            let info = inspected.get(&row.entry.id);
            let (state, _, _) = effective_state(&cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), true);
            let kids = self.lock().reg.direct_children(HOST, &sid, &row.entry.window_id, &live_windows).len();
            let mut item = summary_json(&row.entry, state, cls.summary.as_deref(), row.depth, kids);
            let minder = self.lock().reg.minder_of(&row.entry.id).map(|m| m.id.clone());
            item["minder"] = json!(minder);
            items.push(item);
            let what = if row.entry.minds.is_some() {
                format!("minder, {} ({}, effort={})", row.entry.cli.binary(), row.entry.model, row.entry.effort)
            } else if row.entry.cli.is_agent() {
                format!("{} ({}, effort={})", row.entry.cli.binary(), row.entry.model, row.entry.effort)
            } else {
                "command".to_owned()
            };
            text.push_str(&format!(
                "{}{}{}  {}  {}  [{}]{}\n",
                row.menu_prefix(),
                row.entry.id,
                row.entry.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
                row.entry.display_title(),
                what,
                state.as_str(),
                cls.summary.as_deref().map(|s| format!(" — {s}")).unwrap_or_default()
            ));
        }
        for ex in &exited {
            items.push(summary_json(ex, SubagentLiveState::Exited, Some("exited"), 1, 0));
            let what = if ex.cli.is_agent() { format!("{} ({}, effort={})", ex.cli.binary(), ex.model, ex.effort) } else { "command".to_owned() };
            text.push_str(&format!(
                "   {}{}  {}  {}  [exited]{}\n",
                ex.id,
                ex.name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default(),
                ex.display_title(),
                what,
                ex.effective_session_id().map(|s| format!("  session={s}")).unwrap_or_default()
            ));
        }
        let payload = json!({ "ok": true, "session_id": sid, "parent_window_id": caller_win, "cursor": self.cursor(), "subagents": items });
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
        let inspected = self.inspect(std::slice::from_ref(&entry)).await;
        let info = inspected.get(&entry.id);
        let entry = self.entry(&entry.id).unwrap_or(entry);
        let looked = self.look(&entry, false).await;
        let (cls, screen) = (looked.cls, looked.screen);
        let now = now_unix_ms();
        let since = |at: u64| if at > 0 { now.saturating_sub(at) / 1000 } else { 0 };
        let last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
        let pending = Self::pending_tools(info);
        let (state, error, aborted) =
            effective_state(&cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), cls.state != SubagentLiveState::Exited);
        let tail = tail_of(&screen, 8);

        let mut payload = summary_json(&entry, state, cls.summary.as_deref(), 1, 0);
        payload["minder"] = json!(self.lock().reg.minder_of(&entry.id).map(|m| m.id.clone()));
        // (The turn it is in, or last ended: the cursor of what it answers.)
        payload["answers"] = json!(self.lock().live.get(&entry.id).and_then(|l| l.answers));
        if let Some(p) = looked.pid {
            payload["pid"] = json!(p);
        }
        payload["running"] = json!(state != SubagentLiveState::Exited);
        // (A feedback survey that wouldn't be set aside: what is read may be under it.)
        payload["survey"] = json!(looked.survey);
        payload["exit_code"] = json!(entry.exit_code);
        payload["aborted"] = json!(aborted);
        payload["last_message"] = json!(last_message);
        payload["approval"] = json!(cls.approval.as_ref().map(|ap| approval_to_json(ap, &pending)));
        // (A minder that looks is told what stands asked: that is the
        // question it may answer.)
        if let Some(me) = caller.child.as_ref().filter(|c| c.minds.is_some()) {
            let key = super::service::subkey(&cls);
            let mut st = self.lock();
            if key.is_empty() {
                st.told.remove(&me.id)
            } else {
                st.told.insert(me.id.clone(), key)
            };
        }
        payload["error"] = json!(error);
        payload["last_output_at"] = json!(looked.last_output_at);
        payload["last_state_change_at"] = json!(looked.last_state_change_at);
        payload["quiet_secs"] = json!(since(looked.last_output_at));
        payload["state_age_secs"] = json!(since(looked.last_state_change_at));
        payload["background_shells"] = info.map(|v| v["background_shells"].clone()).filter(|v| !v.is_null()).unwrap_or_else(|| json!([]));
        payload["waiting_on"] = json!(info.and_then(|v| v["waiting_on"].as_str()));
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
        let in_scope_now = || -> Vec<NativeSubagent> {
            if !spec.all {
                return explicit.iter().filter_map(|e| self.entry(&e.id)).collect();
            }
            let (Some(sid), Some(parent)) = (scope_sid.as_deref(), scope_parent.as_deref()) else { return Vec::new() };
            let live = self.live_windows(sid);
            self.lock().reg.descendants_tree(HOST, sid, parent, &live).into_iter().map(|r| r.entry).filter(|e| e.exited_at.is_none()).collect()
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
            if gap {
                payload["gap"] = json!(true);
            }
            let stdout = if spec.json {
                format!("{payload}\n")
            } else {
                format!("{}  event={}  state={}  cursor={}  {}\n", ev.id, ev.event, ev.state.as_str(), ev.seq, matched_line.or_else(|| ev.summary.clone()).unwrap_or_default())
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
        // (The asker, where it is a minder.)
        let minding = caller.child.as_ref().filter(|c| c.minds.is_some()).map(|c| c.id.clone());

        loop {
            let changed = self.changed.notified();
            let current = in_scope_now();
            let live_ids: HashSet<String> = current.iter().map(|e| e.id.clone()).collect();

            // 1. What was written down after the cursor: nothing between
            //    one watch and the next is lost.
            // (What a minder has for the parent is an event of its own
            // kind; what a minded child asks is its minder's to answer,
            // and wakes no other watch while that minder runs. Left in
            // the log: with the minder gone it is told from here.)
            //
            // A minder is told what stands of its agent, once: the last
            // thing written of it, where the agent still stands so and
            // this minder hasn't been told it. Not by a cursor at all.
            // A cursor says what the asker has been told, and a minder
            // that has only just begun has been told nothing: one
            // attached to an agent already asking, or put in another's
            // place while it asked, was given the cursor of that moment
            // and waited on a question that stood before it for as long
            // as it stood (the ShiftUp lead's trial, 2026-11-27; Ben,
            // watching it: "we shouldn't have to wait for the first
            // watch to timeout it should see any outstanding questions
            // when it connects"). And one that took a while over the
            // last question is not told, one by one, of questions long
            // answered or gone (a real one reported "it is waiting" of
            // an agent that had exited, 2026-11-26).
            if let Some(me) = &minding {
                if let Some(agent) = current.first().cloned().or_else(|| explicit.first().cloned()) {
                    let now = if agent.exited_at.is_some() { None } else { Some(self.look(&agent, false).await.cls) };
                    let mut st = self.lock();
                    let told_to = st.seen.get(me).copied().unwrap_or(0);
                    let stands = st.log.last_state_of(&agent.id).filter(|last| last.seq > told_to && spec.matches_state(last.state) && now.as_ref().is_none_or(|cls| still_stands(last, cls))).cloned();
                    if let Some(ev) = stands {
                        st.seen.insert(me.clone(), ev.seq);
                        // (What it asks, by its key: the one question
                        // this minder may now answer.)
                        match ev.state {
                            SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion => st.told.insert(me.clone(), ev.dedupe_key.clone()),
                            _ => st.told.remove(me),
                        };
                        drop(st);
                        return answer(&ev, None, false);
                    }
                }
            } else {
                let queued = {
                    let st = self.lock();
                    st.log.find_since(since_floor, |ev| {
                        event_in_scope(ev, &live_ids)
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
                let mut candidate: Option<(NativeSubagent, ScreenClassification, String, Option<String>)> = None;
                for entry in &current {
                    let looked = self.look(entry, false).await;
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
                        if spec.matches_state(cls.state) && !Self::minders_own(&self.lock().reg, caller, &entry.id, cls.state) {
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
                if let Some((entry, first, name, matched_line)) = candidate {
                    let is_text = name == "match" || name == "regex";
                    // (A state must hold; text that came is there.)
                    let holds = if first.state == SubagentLiveState::Exited || is_text {
                        true
                    } else {
                        tokio::time::sleep(super::service::CONFIRM).await;
                        self.look(&entry, true).await.cls.state == first.state
                    };
                    if holds {
                        let inspected = self.inspect(std::slice::from_ref(&entry)).await;
                        let info = inspected.get(&entry.id);
                        let entry = self.entry(&entry.id).unwrap_or(entry);
                        let now = self.look(&entry, false).await;
                        let pending = Self::pending_tools(info);
                        let (state, error, _) =
                            effective_state(&now.cls, info.and_then(|v| v["transcript_state"].as_str()), info.and_then(|v| v["transcript_error"].as_str()), now.cls.state != SubagentLiveState::Exited);
                        // (Answered from what stands, not written into the
                        // log: the log is what happened, once each.)
                        let mut ev = self.event_of(&entry, state, now.cls.summary.clone(), String::new());
                        ev.seq = self.cursor();
                        if is_text {
                            ev.event = name;
                        }
                        ev.last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
                        ev.approval = now.cls.approval.as_ref().map(|ap| approval_to_json(ap, &pending));
                        ev.error = error;
                        ev.tail = tail_of(&now.screen, 10);
                        return answer(&ev, matched_line, had_gap);
                    }
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

    async fn send(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("send", msg, code);
        let spec = match parse_send_payload(&args_of(raw).unwrap_or_default(), Some(raw)) {
            Ok(v) => v,
            Err(e) => return err(e, 2),
        };
        let entry = match self.resolve(caller, &spec.target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        self.minder_did(caller, "send", raw.get("args").cloned().unwrap_or(Value::Null));
        let before = self.look(&entry, true).await;
        if before.cls.state == SubagentLiveState::Exited {
            return err(format!("subagent `{}` has already exited", entry.id), 1);
        }
        // Text is never typed into a question: answered by `approve`, or forced.
        // Nor a key that types a character (`--key 1`): there it is an
        // answer by its number, with none of what `approve` checks.
        if (!spec.body.is_empty() || spec.types) && !spec.force && entry.cli.is_agent() && dialog_up(&before.cls) {
            let what = if spec.body.is_empty() { "press a character key in" } else { "type text into" };
            return err(format!("refusing to {what} `{}` while a {} dialog is active (use `olo agent approve` to answer approvals, or pass `--force`)", entry.id, before.cls.state.as_str()), 1);
        }
        // (From before the typing: a watch from this cursor has what follows it.)
        let pre_cursor = self.cursor();
        self.reset_edge(&entry, !spec.body.is_empty());
        // A message submitted to an agent at rest is a turn given: the
        // rest it shows until it takes that up is not that turn's end,
        // and the turn's end says which message it answers (`answers`:
        // this cursor). Sent into a turn already going, it is that
        // turn's, and nothing is told apart.
        if entry.cli.is_agent() && !spec.body.is_empty() && spec.enter && matches!(before.cls.state, SubagentLiveState::Idle | SubagentLiveState::Done) {
            self.expect_work(&entry, pre_cursor, super::service::taken_up(), true);
        }

        let (rex, to) = (&self.rex, &entry);
        let write = |data: String| async move { rex.write(&to.session_id, &to.block_id, &data).await.map_err(|e| e.to_string()) };
        let mut total = 0usize;
        let body = sanitize_paste_text(&spec.body);
        if !body.is_empty() {
            // As a paste where the program takes pastes (its keys are
            // then not keys); what would end a paste early is taken out.
            let chunk = if self.bracketed_paste(&entry).await { format!("\x1b[200~{body}\x1b[201~") } else { body.clone() };
            total += chunk.len();
            if let Err(e) = write(chunk).await {
                return err(e, 1);
            }
            if spec.enter {
                // (Apart from the paste, so it isn't taken into it.)
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        }
        if spec.enter {
            total += 1;
            if let Err(e) = write("\r".into()).await {
                return err(e, 1);
            }
        }
        // (Each key by itself: Escape then Enter together is another key.)
        for (i, key) in spec.post_keys.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(35)).await;
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

        // That a message sent whole was taken: the agent works, or asks
        // something, or its screen changed and the text is no longer in
        // its input. Enter once more, only where it still sits there.
        let mut delivered = true;
        if spec.verify_delivery && !body.is_empty() && entry.cli.is_agent() {
            delivered = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let retry_at = tokio::time::Instant::now() + Duration::from_millis(800);
            let mut retried = false;
            let first_line = body.lines().next().map(str::trim).filter(|s| s.len() >= 4);
            loop {
                let changed = self.changed.notified();
                let now = self.look(&entry, true).await;
                if dialog_up(&now.cls)
                    || now.cls.state == SubagentLiveState::Working
                    || (now.last_output_at > before.last_output_at && now.screen != before.screen && !still_typed(&now.screen, first_line))
                {
                    delivered = true;
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                let wake = if !retried && retry_at < deadline { retry_at } else { deadline };
                tokio::select! {
                    _ = changed => {}
                    _ = tokio::time::sleep(Duration::from_millis(120)) => {}
                    _ = tokio::time::sleep_until(wake) => {
                        if !retried && tokio::time::Instant::now() >= retry_at {
                            retried = true;
                            let check = self.look(&entry, true).await;
                            if !dialog_up(&check.cls) && still_typed(&check.screen, first_line) {
                                let _ = write("\r".into()).await;
                            }
                        }
                    }
                }
            }
        }
        let payload = json!({ "ok": true, "id": entry.id, "name": entry.name, "bytes": total, "delivered": delivered, "cursor": pre_cursor });
        Response::ok(if spec.as_json { format!("{payload}\n") } else { format!("Sent {total} bytes to {} (delivered={delivered}).\n", entry.id) }, payload)
    }

    // ---- approve ----

    async fn approve(self: &Arc<Self>, raw: &Value, caller: &Caller) -> Response {
        let err = |msg: String, code: i32| Response::err("approve", msg, code);
        let spec = match ApproveSpec::parse(Some(raw)) {
            Ok(s) => s,
            Err(e) => return err(e, 2),
        };
        let entry = match self.resolve(caller, &spec.target) {
            Ok(e) => e,
            Err(e) => return err(e, 1),
        };
        // (A minder's yes is for this once: one that is kept answers
        // questions its rules were never asked about.)
        if caller.child.as_ref().is_some_and(|c| c.minds.is_some()) && spec.answer == olo_rex_link::agents::Answer::Session {
            return err("a minder's yes is for this once (`--once`): one that is kept is the parent's to give".into(), 69);
        }
        self.minder_did(caller, "approve", raw.get("args").cloned().unwrap_or(Value::Null));
        let pre_cursor = self.cursor();
        // (As the server shows it this moment.)
        let cls = self.look(&entry, true).await.cls;
        // A minder answers the question it was last told of and no
        // other: what is on the screen now may have come since it
        // judged, and a yes would be a yes to that.
        if let Some(me) = caller.child.as_ref().filter(|c| c.minds.is_some()) {
            let now = super::service::subkey(&cls);
            if cls.approval.is_some() && self.lock().told.get(&me.id) != Some(&now) {
                return err("what it asks now isn't what you were last told of: nothing was answered. Look again (`status --json` says what stands asked), judge that, then answer".into(), 1);
            }
        }
        let Some(ap) = cls.approval else {
            return err(format!("`{}` has no active approval prompt (current state: {})", entry.id, cls.state.as_str()), 1);
        };
        let orig_key = ap.key;
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
        let delta = choice.index as isize - ap.selected_index.unwrap_or(0) as isize;
        for _ in 0..delta.unsigned_abs() {
            if let Err(e) = write(if delta > 0 { "\x1b[B" } else { "\x1b[A" }).await {
                return err(format!("failed to navigate approval dialog: {e}"), 1);
            }
            tokio::time::sleep(Duration::from_millis(45)).await;
        }
        // Enter only where the mark is seen to stand on the very choice
        // meant, in the very question asked about; otherwise Escape.
        let on_it = |cur: &olo_agent_screen::ApprovalPrompt| cur.selected_index == Some(choice.index) && cur.choices.get(choice.index).is_some_and(|c| c.kind == want && c.label == choice.label);
        if delta != 0 {
            let verify_by = tokio::time::Instant::now() + Duration::from_millis(600);
            let mut verified = false;
            while tokio::time::Instant::now() < verify_by {
                tokio::time::sleep(Duration::from_millis(55)).await;
                let Some(cur) = self.look(&entry, true).await.cls.approval else {
                    return err("approval dialog disappeared before confirmation".into(), 1);
                };
                if cur.key != orig_key {
                    let _ = write("\x1b").await;
                    return err("approval dialog changed during navigation; aborted safely".into(), 1);
                }
                if on_it(&cur) {
                    verified = true;
                    break;
                }
            }
            if !verified {
                let _ = write("\x1b").await;
                return err(format!("cursor did not land on `{}` (`{}`); sent Escape instead of Enter", choice.label, want.as_str()), 1);
            }
        }
        // (Before Enter: a question that follows at once is a new event.)
        self.reset_edge(&entry, false);
        if let Err(e) = write("\r").await {
            return err(format!("failed to confirm approval dialog: {e}"), 1);
        }
        self.poke(&entry.id);

        // That the question went (another in its place is gone too: never
        // Enter again onto one nobody looked at), within two seconds;
        // Enter once more only where the same question still stands with
        // the mark where it was put.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let retry_at = tokio::time::Instant::now() + Duration::from_millis(700);
        let (mut retried, mut dismissed) = (false, false);
        loop {
            let changed = self.changed.notified();
            match self.look(&entry, true).await.cls.approval.as_ref() {
                None => {
                    dismissed = true;
                    break;
                }
                Some(cur) if cur.key != orig_key => {
                    dismissed = true;
                    break;
                }
                Some(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::select! {
                _ = changed => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            if !retried && tokio::time::Instant::now() >= retry_at {
                retried = true;
                if self.look(&entry, true).await.cls.approval.as_ref().is_some_and(|cur| cur.key == orig_key && on_it(cur)) {
                    let _ = write("\r").await;
                }
            }
        }

        // A refusal's reason, only once the question is gone and none
        // stands in its place, as a paste into the agent's own input.
        if dismissed
            && spec.answer == olo_rex_link::agents::Answer::No
            && let Some(reason) = spec.reason.as_deref().map(sanitize_paste_text).map(|r| r.trim().to_owned()).filter(|r| !r.is_empty())
            && let Some(agent) = entry.cli.screen_agent()
        {
            let by = tokio::time::Instant::now() + Duration::from_millis(1200);
            let safe = loop {
                let now = self.look(&entry, true).await;
                if now.cls.approval.is_some() || now.cls.folder_trust.is_some() {
                    break false;
                }
                let low = now.screen.to_ascii_lowercase();
                if olo_agent_screen::at_prompt(agent, &now.screen) || low.contains("tell ") || low.contains("what to do differently") || tokio::time::Instant::now() >= by {
                    break true;
                }
                tokio::time::sleep(Duration::from_millis(60)).await;
            };
            if safe {
                let text = if self.bracketed_paste(&entry).await { format!("\x1b[200~{reason}\x1b[201~") } else { reason };
                let _ = self.rex.write(&entry.session_id, &entry.block_id, &text).await;
                tokio::time::sleep(Duration::from_millis(80)).await;
                let now = self.look(&entry, true).await.cls;
                if now.approval.is_none() && now.folder_trust.is_none() {
                    let _ = write("\r").await;
                }
                self.poke(&entry.id);
            }
        }

        let payload = json!({ "ok": dismissed, "id": entry.id, "name": entry.name, "chosen_label": choice.label, "chosen_kind": choice.kind.as_str(), "dismissed": dismissed, "cursor": pre_cursor });
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
            Err(e) => return Response::err("stop", e, 1),
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
        let mut running: Vec<(NativeSubagent, i32, u64)> = Vec::new();
        {
            let mut of = vec![entry.clone()];
            if tree {
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
        let stopped = self.stop_tabs(&entry, force, tree, keep_tab).await;
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
        let left: Vec<String> = programs.iter().filter(|p| p["ended"] == false).map(|p| format!("{} (pid {})", p["name"].as_str().or(p["id"].as_str()).unwrap_or_default(), p["pid"])).collect();
        let payload = json!({
            "ok": true, "id": entry.id, "name": entry.name, "stopped": stopped,
            // (Whether every program that ran in what was stopped has ended; each, with its pid.)
            "ended": left.is_empty(), "programs": programs,
            "cli_session_id": entry.effective_session_id(), "transcript_path": entry.transcript_path, "saved_path": saved,
        });
        let said = match (left.is_empty(), keep_tab) {
            (true, _) => format!("Stopped {} subagent tab(s).\n", stopped.len()),
            (false, true) => format!("Interrupted {} subagent tab(s), kept open: still running: {}.\n", stopped.len(), left.join(", ")),
            (false, false) => format!("Stopped {} subagent tab(s), but not ended after {} s: {}.\n", stopped.len(), STOPPED_WITHIN.as_secs(), left.join(", ")),
        };
        Response::ok(if as_json { format!("{payload}\n") } else { said }, payload)
    }

    /// Interrupts a child (and, with `tree`, those under it, deepest
    /// first) and closes its tab.
    pub async fn stop_tabs(self: &Arc<Self>, entry: &NativeSubagent, force: bool, tree: bool, keep_tab: bool) -> Vec<String> {
        let live = self.live_windows(&entry.session_id);
        let mut targets: Vec<NativeSubagent> = Vec::new();
        if tree {
            let mut under = self.lock().reg.descendants_tree(HOST, &entry.session_id, &entry.window_id, &live);
            under.reverse();
            targets.extend(under.into_iter().map(|r| r.entry));
        }
        targets.push(entry.clone());
        // (A minder stopped here is stopped by one who may: its end
        // isn't news to the parent.)
        self.lock().let_go.extend(targets.iter().filter(|t| t.minds.is_some() && t.exited_at.is_none()).map(|t| t.id.clone()));
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
        for t in &targets {
            if !keep_tab {
                let _ = self.rex.call("session.close_window", json!({ "session_id": t.session_id, "window_id": t.window_id })).await;
                // (Said now: the server's own word of it follows, and says nothing new.)
                self.exit(&t.id, None);
            }
            stopped.push(t.id.clone());
        }
        stopped
    }

    // ---- prune, focus ----

    fn prune(&self, raw: &Value, caller: &Caller) -> Response {
        if caller.child.is_some() {
            return Response::err("prune", "refused from child subagent tab", 69);
        }
        let (mut all, mut as_json) = (false, false);
        if let Some(args) = args_of(raw) {
            for a in &args {
                match a.as_str() {
                    "--all" | "-a" => all = true,
                    "--json" => as_json = true,
                    _ => {}
                }
            }
        } else {
            all = raw["all"].as_bool().unwrap_or(false);
            as_json = raw["json"].as_bool().unwrap_or(false);
        }
        let gone: Vec<String> = {
            let mut st = self.lock();
            let scope = if all { None } else { caller.session.clone() };
            let gone: Vec<String> = st.reg.entries.iter().filter(|e| e.exited_at.is_some() && scope.as_deref().is_none_or(|s| e.session_id == s)).map(|e| e.id.clone()).collect();
            st.reg.prune_exited(HOST, scope.as_deref());
            for id in &gone {
                st.live.remove(id);
            }
            gone
        };
        if !gone.is_empty() {
            self.saved();
            for id in &gone {
                self.said(Frame::Gone { id: id.clone() });
            }
        }
        let payload = json!({ "ok": true, "pruned": gone.len() });
        Response::ok(if as_json { format!("{payload}\n") } else { format!("Pruned {} exited subagent record(s).\n", gone.len()) }, payload)
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
