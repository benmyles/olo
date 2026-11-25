//! Running a command on the Olo desktop from a Rex host (`olo run`,
//! `rex do olo.run`), asked first in a desktop approval sheet styled after
//! `olo-sudo`'s (or skipped with a quiet notification while Deskbar's
//! Approve Without Asking is active).
//!
//! Streaming & cancellation:
//! - `olo.run` starts a tracked job (`RunJobs::start`), checking permission
//!   and approval, spawning the child in its own process group with the
//!   desktop's clean environment (nothing carried from the host) and desktop
//!   home (or `--cwd DIR`).
//! - `olo.run.poll` blocks until stdout/stderr bytes arrive or the command
//!   exits (up to `POLL_WAIT`), returning chunks immediately so the host
//!   streams output live.
//! - `olo.run.cancel` (called by `olo`'s INT/TERM trap) kills the child
//!   process group or cancels a pending approval sheet right away; a
//!   heartbeat timeout (`HEARTBEAT_TIMEOUT`) also kills orphaned jobs if
//!   the host client disappears.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_system::{env, face, fingerprint};
use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    sync::{Notify, oneshot},
};

/// How long `olo.run` waits inline for a fast command to complete before
/// returning the job id for streaming polls.
const INITIAL_WAIT: Duration = Duration::from_millis(250);
/// How long `olo.run.poll` waits for new output or exit before returning
/// an empty keep-alive chunk.
const POLL_WAIT: Duration = Duration::from_secs(4);
/// If no `olo.run.poll` arrives for this long, cancel the job / alert.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
/// Maximum buffered bytes per stream before throttling reads.
const MAX_BUF: usize = 256 * 1024;
/// How long Touch ID waits on an open approval alert before releasing the sensor.
const FINGER_GIVE_UP: Duration = Duration::from_secs(40);
/// Ignore Allow clicks/keys for this long after the sheet appears so a stray
/// click or keypress while typing in another window can never approve a
/// remote command by accident (Touch ID / Face Unlock bypass this guard).
const ALLOW_GUARD: Duration = Duration::from_millis(500);

pub const USAGE: &str = "\
usage: olo run [--cwd DIR] [--] COMMAND [ARG…]

Run COMMAND on the Olo desktop (as your desktop account, in your desktop home
unless --cwd DIR is given, with none of this host's environment). Requires
the host's 'Ask to run commands on this computer' permission in Preferences ▸ Rex.
Each run asks for approval on the desktop unless Approve Without Asking is
active in Olo's tray. Output streams live; ^C cancels the desktop command.
";

/// Parsed `olo run` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunAsk {
    Help,
    Exec { cwd: Option<String>, argv: Vec<String> },
}

impl RunAsk {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut cwd: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            match a {
                "-h" | "--help" => return Ok(Self::Help),
                "--" => {
                    i += 1;
                    break;
                }
                "--cwd" | "-C" => {
                    i += 1;
                    let val = args.get(i).ok_or_else(|| format!("{a} needs a folder"))?;
                    cwd = Some(val.clone());
                    i += 1;
                }
                _ if a.starts_with("--cwd=") => {
                    cwd = Some(a.trim_start_matches("--cwd=").to_owned());
                    i += 1;
                }
                _ if a.starts_with("-C=") => {
                    cwd = Some(a.trim_start_matches("-C=").to_owned());
                    i += 1;
                }
                _ if a.starts_with('-') => {
                    return Err(format!("unknown option {a} (put -- before the command)"));
                }
                _ => break,
            }
        }
        let argv = args[i..].to_vec();
        if argv.is_empty() || argv[0].is_empty() {
            return Err("say which command to run (e.g. olo run -- ls -la)".into());
        }
        Ok(Self::Exec { cwd, argv })
    }
}

/// Shell-quoted command line for display in the approval alert and notifications.
pub fn format_command(argv: &[String]) -> String {
    argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'=' | b'+' | b',' | b'~')) {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Resolve the requested working directory on the Olo desktop (`~` expands
/// against desktop `$HOME`; omitted defaults to desktop `$HOME`).
pub fn resolve_desktop_cwd(cwd: Option<&str>) -> Result<(PathBuf, String), String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let path = match cwd.map(str::trim).filter(|s| !s.is_empty()) {
        None => home.clone(),
        Some("~") => home.clone(),
        Some(rest) if rest.starts_with("~/") => home.join(rest.trim_start_matches("~/")),
        Some(raw) => {
            let p = PathBuf::from(raw);
            if p.is_absolute() { p } else { home.join(p) }
        }
    };
    if !path.is_dir() {
        return Err(format!("desktop folder doesn't exist: {}", path.display()));
    }
    let display = if path == home {
        "~".to_owned()
    } else if let Ok(rel) = path.strip_prefix(&home) {
        format!("~/{}", rel.display())
    } else {
        path.display().to_string()
    };
    Ok((path, display))
}

/// Whether Deskbar's Approve Without Asking is currently active (blocks on IPC:
/// call off the UI thread).
pub fn approving_without_asking() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var("OLO_REX_TEST_AUTO_APPROVE").ok().as_deref() == Some("1") {
        return true;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let status = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&olo_protocol::Request::PowerStatus).ok());
    matches!(status, Some(olo_protocol::Reply::Power { status }) if status.auto_approve_until.is_some_and(|t| t > now))
}

/// Biometric / camera unlock ways available on this desktop right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnlockWays {
    pub finger: bool,
    pub face: bool,
}

impl UnlockWays {
    /// Probes fprintd and olo-faced (blocking D-Bus — call off UI thread).
    pub fn find() -> Self {
        let Some(user) = olo_system::auth::current_user() else {
            return Self::default();
        };
        Self {
            finger: env::finger_here() && fingerprint::enrolled(&user).unwrap_or(false),
            face: env::face_here() && {
                let s = face::status();
                s.camera && s.enrolled > 0
            },
        }
    }

    pub fn sentence(&self) -> Option<String> {
        match (self.finger, self.face) {
            (false, false) => None,
            (true, false) => Some(olo_kit::words::tr("Or use Touch ID to allow.").to_owned()),
            (false, true) => Some(olo_kit::words::tr("Or look at the camera.").to_owned()),
            (true, true) => Some(olo_kit::words::tr("Or use Touch ID or camera.").to_owned()),
        }
    }
}

/// Holds active Touch ID / face unlock listeners while an approval alert is up.
struct UnlockWaits {
    finger: Option<fingerprint::Stop>,
}

impl UnlockWaits {
    fn start(ways: UnlockWays, id: String, approvals: Arc<RunApprovals>) -> Self {
        let Some(user) = olo_system::auth::current_user() else {
            return Self { finger: None };
        };
        let finger = ways.finger.then(|| {
            let stop = fingerprint::Stop::default();
            let (u, s, app, req_id) = (user.clone(), stop.clone(), approvals.clone(), id.clone());
            let _ = std::thread::Builder::new().name("rex-run-finger".into()).spawn(move || {
                if matches!(fingerprint::verify(&u, &s, |_| {}), Ok(true)) {
                    app.respond(&req_id, true);
                }
            });
            let s = stop.clone();
            let _ = std::thread::Builder::new().name("rex-run-finger-limit".into()).spawn(move || {
                std::thread::sleep(FINGER_GIVE_UP);
                if !s.is_stopped() {
                    s.stop();
                }
            });
            stop
        });
        if ways.face {
            let (app, req_id) = (approvals, id);
            let _ = std::thread::Builder::new().name("rex-run-face".into()).spawn(move || {
                if face::verify(3000) == face::Look::Match {
                    app.respond(&req_id, true);
                }
            });
        }
        Self { finger }
    }
}

impl Drop for UnlockWaits {
    fn drop(&mut self) {
        if let Some(stop) = self.finger.take()
            && !stop.is_stopped()
        {
            std::thread::spawn(move || stop.stop());
        }
    }
}

/// Snapshot of the frontmost pending approval request for UI rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPrompt {
    pub id: String,
    pub host: String,
    pub command: String,
    pub cwd: String,
    pub ways: UnlockWays,
    pub ways_note: Option<String>,
    pub queued: usize,
}

struct PendingApproval {
    id: String,
    host: String,
    command: String,
    cwd: String,
    ways: UnlockWays,
    shown_at: Instant,
    tx: Option<oneshot::Sender<bool>>,
    waits: Option<UnlockWaits>,
}

/// Queue of desktop approval requests shown in `WinRole::RunApprove`.
pub struct RunApprovals {
    queue: Mutex<VecDeque<PendingApproval>>,
    on_open: Arc<dyn Fn() + Send + Sync>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl RunApprovals {
    pub fn new(on_open: impl Fn() + Send + Sync + 'static, on_change: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { queue: Mutex::new(VecDeque::new()), on_open: Arc::new(on_open), on_change: Arc::new(on_change) })
    }

    /// Enqueue a new command approval request and open/raise the alert window.
    pub fn enqueue(self: &Arc<Self>, id: String, host: String, command: String, cwd: String, ways: UnlockWays) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            let was_empty = q.is_empty();
            let mut entry = PendingApproval { id: id.clone(), host, command, cwd, ways, shown_at: Instant::now(), tx: Some(tx), waits: None };
            if was_empty {
                entry.waits = Some(UnlockWaits::start(ways, id, self.clone()));
            }
            q.push_back(entry);
        }
        (self.on_open)();
        (self.on_change)();
        rx
    }

    /// Populate a deterministic demo prompt for headless visual QA (`OLO_REX_DEMO_RUN`).
    pub fn show_demo(self: &Arc<Self>, mode: &str) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if !q.is_empty() {
            return;
        }
        let shown_at = Instant::now().checked_sub(Duration::from_secs(2)).unwrap_or_else(Instant::now);
        let (host, command, cwd, ways) = match mode {
            "long" => (
                "build-runner-west-04",
                "systemctl --user status pipewire pipewire-pulse wireplumber && pactl list short sinks && pactl list short sources && wpctl status --nick && journalctl --user -u pipewire -n 40 --no-pager && ls -la /run/user/1000/pipewire-0 /etc/pipewire/pipewire.conf.d && env | grep -E '^(XDG|WAYLAND|PIPEWIRE|PULSE)' | sort",
                "~/src/olo/apps/rex/worktrees/feature-streaming-approval/scripts",
                UnlockWays { finger: true, face: false },
            ),
            "plain" => ("dev-box", "systemctl --user status pipewire && pactl list short sinks", "~/src/olo", UnlockWays { finger: false, face: false }),
            _ => ("dev-box", "systemctl --user status pipewire && pactl list short sinks", "~/src/olo", UnlockWays { finger: true, face: false }),
        };
        q.push_back(PendingApproval { id: "demo-run".into(), host: host.into(), command: command.into(), cwd: cwd.into(), ways, shown_at, tx: None, waits: None });
    }

    /// Frontmost pending prompt to display in the alert window.
    pub fn current(&self) -> Option<RunPrompt> {
        let q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let front = q.front()?;
        Some(RunPrompt { id: front.id.clone(), host: front.host.clone(), command: front.command.clone(), cwd: front.cwd.clone(), ways: front.ways, ways_note: front.ways.sentence(), queued: q.len() })
    }

    /// Answer from a UI click or key: ignores Allow for `ALLOW_GUARD` after
    /// the prompt became frontmost so a stray click or keypress never runs a
    /// remote command. Returns `false` if ignored by the guard.
    pub fn respond_from_ui(self: &Arc<Self>, id: &str, allow: bool) -> bool {
        if allow {
            let q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(front) = q.front()
                && (id.is_empty() || front.id == id)
                && front.shown_at.elapsed() < ALLOW_GUARD
            {
                return false;
            }
        }
        self.respond(id, allow);
        true
    }

    /// Answer the request matching `id` (or frontmost if `id` is empty).
    pub fn respond(self: &Arc<Self>, id: &str, allow: bool) {
        let tx = {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            let pos = if id.is_empty() { Some(0).filter(|_| !q.is_empty()) } else { q.iter().position(|p| p.id == id) };
            let Some(pos) = pos else { return };
            let removed = q.remove(pos);
            if pos == 0
                && let Some(next) = q.front_mut()
                && next.waits.is_none()
            {
                next.shown_at = Instant::now();
                next.waits = Some(UnlockWaits::start(next.ways, next.id.clone(), self.clone()));
            }
            removed.and_then(|mut r| r.tx.take())
        };
        if let Some(tx) = tx {
            let _ = tx.send(allow);
        }
        (self.on_change)();
    }

    /// Deny all remaining requests (e.g. window closed).
    pub fn deny_front(self: &Arc<Self>) {
        self.respond("", false);
    }

    /// Cancel a pending approval if the host command was interrupted (`^C`).
    pub fn cancel(self: &Arc<Self>, id: &str) {
        self.respond(id, false);
    }
}

struct RunBuffer {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    done: bool,
    ok: bool,
    code: Option<i32>,
    last_seen: Instant,
}

struct RunJob {
    buf: Mutex<RunBuffer>,
    changed: Notify,
    cancelled: AtomicBool,
    cancel_notify: Notify,
}

impl RunJob {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: Mutex::new(RunBuffer { stdout: Vec::new(), stderr: Vec::new(), done: false, ok: false, code: None, last_seen: Instant::now() }),
            changed: Notify::new(),
            cancelled: AtomicBool::new(false),
            cancel_notify: Notify::new(),
        })
    }

    fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            self.cancel_notify.notify_waiters();
        }
    }

    fn finish(&self, ok: bool, code: Option<i32>, extra_err: Option<&str>) {
        {
            let mut b = self.buf.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = extra_err {
                b.stderr.extend_from_slice(e.as_bytes());
            }
            b.ok = ok;
            b.code = code;
            b.done = true;
        }
        self.changed.notify_waiters();
    }

    fn drain_response(&self, id: &str) -> (Value, bool) {
        let mut b = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        b.last_seen = Instant::now();
        let out = String::from_utf8_lossy(&std::mem::take(&mut b.stdout)).into_owned();
        let err = String::from_utf8_lossy(&std::mem::take(&mut b.stderr)).into_owned();
        let done = b.done;
        let ok = if done { b.ok } else { true };
        let code = b.code;
        let mut res = json!({
            "ok": ok,
            "id": id,
            "done": done,
        });
        if !out.is_empty() {
            res["stdout"] = json!(out);
        }
        if !err.is_empty() {
            res["stderr"] = json!(err);
        }
        if let Some(c) = code {
            res["data"] = json!({ "code": c });
        }
        (res, done)
    }

    fn has_data_or_done(&self) -> bool {
        let b = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        b.done || !b.stdout.is_empty() || !b.stderr.is_empty()
    }
}

/// Manages active `olo run` desktop command jobs for Rex.
#[derive(Default)]
pub struct RunJobs {
    jobs: Mutex<HashMap<String, Arc<RunJob>>>,
}

impl RunJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a desktop command run (`olo.run`), returning the initial response
    /// (complete if fast, else `{ id, done: false, ... }` for streaming polls).
    pub async fn start(&self, requested_id: Option<String>, host_label: String, ask: RunAsk, approvals: Arc<RunApprovals>, on_quiet_note: Arc<dyn Fn(String, String) + Send + Sync>) -> Value {
        let RunAsk::Exec { cwd, argv } = ask else {
            return json!({ "ok": true, "done": true, "stdout": USAGE });
        };
        let (dir, cwd_display) = match resolve_desktop_cwd(cwd.as_deref()) {
            Ok(v) => v,
            Err(e) => return json!({ "ok": false, "done": true, "stderr": format!("olo: {e}\n"), "data": { "code": 1 } }),
        };
        let id = requested_id.filter(|s| !s.is_empty() && s.len() <= 96).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let job = RunJob::new();
        {
            let mut map = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            map.insert(id.clone(), job.clone());
        }

        let job_bg = job.clone();
        let id_bg = id.clone();
        let cmd_display = format_command(&argv);
        tokio::spawn(async move {
            run_job_lifecycle(id_bg, host_label, cmd_display, dir, cwd_display, argv, job_bg, approvals, on_quiet_note).await;
        });

        // Give very fast auto-approved commands a brief window to finish in 1 RTT.
        if !job.has_data_or_done() {
            let _ = tokio::time::timeout(INITIAL_WAIT, job.changed.notified()).await;
        }
        let (res, done) = job.drain_response(&id);
        if done {
            self.jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        }
        res
    }

    /// Stream next output chunk or completion (`olo.run.poll`).
    pub async fn poll(&self, id: &str) -> Value {
        let job = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
        let Some(job) = job else {
            return json!({ "ok": false, "done": true, "stderr": "olo: command is no longer running\n", "data": { "code": 1 } });
        };
        {
            let mut b = job.buf.lock().unwrap_or_else(|e| e.into_inner());
            b.last_seen = Instant::now();
        }
        if !job.has_data_or_done() {
            let _ = tokio::time::timeout(POLL_WAIT, job.changed.notified()).await;
        }
        let (res, done) = job.drain_response(id);
        if done {
            self.jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        }
        res
    }

    /// Cancel an active or awaiting-approval run (`olo.run.cancel`).
    pub fn cancel(&self, id: &str) -> Value {
        let job = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
        if let Some(job) = job {
            job.cancel();
        }
        json!({ "ok": true })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_job_lifecycle(
    id: String,
    host_label: String,
    cmd_display: String,
    dir: PathBuf,
    cwd_display: String,
    argv: Vec<String>,
    job: Arc<RunJob>,
    approvals: Arc<RunApprovals>,
    on_quiet_note: Arc<dyn Fn(String, String) + Send + Sync>,
) {
    // 1. Check Deskbar's Approve Without Asking off the async/UI thread.
    let auto = tokio::task::spawn_blocking(approving_without_asking).await.unwrap_or(false);
    if job.cancelled.load(Ordering::SeqCst) {
        job.finish(false, Some(130), Some("olo: cancelled\n"));
        return;
    }

    if auto {
        tracing::info!(host = %host_label, cmd = %cmd_display, "olo run: auto-approved (Approve Without Asking is on)");
        on_quiet_note("Approved without asking".into(), format!("{host_label}: {cmd_display}"));
    } else {
        let ways = tokio::task::spawn_blocking(UnlockWays::find).await.unwrap_or_default();
        let mut rx = approvals.enqueue(id.clone(), host_label.clone(), cmd_display.clone(), cwd_display.clone(), ways);
        // Wait for user approval, explicit cancellation, or heartbeat expiry.
        let allowed = loop {
            tokio::select! {
                res = &mut rx => break res.unwrap_or(false),
                _ = job.cancel_notify.notified() => {
                    approvals.cancel(&id);
                    job.finish(false, Some(130), Some("olo: cancelled\n"));
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    let stale = job.buf.lock().unwrap_or_else(|e| e.into_inner()).last_seen.elapsed() > HEARTBEAT_TIMEOUT;
                    if stale || job.cancelled.load(Ordering::SeqCst) {
                        approvals.cancel(&id);
                        job.finish(false, Some(130), Some("olo: cancelled\n"));
                        return;
                    }
                }
            }
        };
        if !allowed {
            tracing::info!(host = %host_label, cmd = %cmd_display, "olo run: denied by user");
            job.finish(false, Some(126), Some("olo: denied on the Olo desktop\n"));
            return;
        }
        tracing::info!(host = %host_label, cmd = %cmd_display, "olo run: allowed by user");
    }

    if job.cancelled.load(Ordering::SeqCst) {
        job.finish(false, Some(130), Some("olo: cancelled\n"));
        return;
    }

    // 2. Spawn the command with desktop's clean environment (nothing of host's
    //    or Rex's internal variables carried) in its own process group.
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for k in ["REX_SESSION", "REX_BLOCK", "REX_CLIENT_ID", "OLO_BRIDGE_CMD", "OLO_BRIDGE_ARGC", "OLO_BRIDGE_DIR", "OLO_RUN_ID", "OLO_REX_DEMO", "OLO_REX_DEMO_RUN", "OLO_REX_TEST_AUTO_APPROVE"] {
        cmd.env_remove(k);
    }
    // SAFETY: `setpgid(0, 0)` is async-signal-safe and puts the child in its
    // own process group so cancelling kills any grandchildren cleanly.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            job.finish(false, Some(127), Some(&format!("olo: couldn't start {}: {e}\n", argv[0])));
            return;
        }
    };
    let pgid = child.id().map(|p| p as i32);

    // 3. Stream stdout & stderr concurrently into the shared job buffer.
    let out_task = child.stdout.take().map(|mut reader| {
        let j = job.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        loop {
                            let full = j.buf.lock().unwrap_or_else(|e| e.into_inner()).stdout.len() >= MAX_BUF;
                            if !full || j.cancelled.load(Ordering::SeqCst) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        {
                            let mut b = j.buf.lock().unwrap_or_else(|e| e.into_inner());
                            b.stdout.extend_from_slice(&chunk[..n]);
                        }
                        j.changed.notify_waiters();
                    }
                }
            }
        })
    });
    let err_task = child.stderr.take().map(|mut reader| {
        let j = job.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        loop {
                            let full = j.buf.lock().unwrap_or_else(|e| e.into_inner()).stderr.len() >= MAX_BUF;
                            if !full || j.cancelled.load(Ordering::SeqCst) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        {
                            let mut b = j.buf.lock().unwrap_or_else(|e| e.into_inner());
                            b.stderr.extend_from_slice(&chunk[..n]);
                        }
                        j.changed.notify_waiters();
                    }
                }
            }
        })
    });

    // 4. Wait for completion, explicit cancel, or heartbeat timeout.
    let status = loop {
        tokio::select! {
            res = child.wait() => break Some(res),
            _ = job.cancel_notify.notified() => {
                kill_process_group(pgid, &mut child).await;
                break None;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                let stale = job.buf.lock().unwrap_or_else(|e| e.into_inner()).last_seen.elapsed() > HEARTBEAT_TIMEOUT;
                if stale || job.cancelled.load(Ordering::SeqCst) {
                    kill_process_group(pgid, &mut child).await;
                    break None;
                }
            }
        }
    };

    if let Some(t) = out_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), t).await;
    }
    if let Some(t) = err_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), t).await;
    }

    match status {
        None => job.finish(false, Some(130), None),
        Some(Ok(st)) => {
            let code = st.code().unwrap_or(1);
            job.finish(st.success(), Some(code), None);
        }
        Some(Err(e)) => job.finish(false, Some(1), Some(&format!("olo: wait failed: {e}\n"))),
    }
}

async fn kill_process_group(pgid: Option<i32>, child: &mut tokio::process::Child) {
    if let Some(p) = pgid
        && p > 0
    {
        // SAFETY: sends SIGTERM to the child's own process group created via setpgid(0, 0).
        unsafe {
            libc::kill(-p, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_millis(500), child.wait()).await.is_err() {
            // SAFETY: escalate to SIGKILL on the same process group.
            unsafe {
                libc::kill(-p, libc::SIGKILL);
            }
            let _ = child.wait().await;
        }
    } else {
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_run_args() {
        assert_eq!(RunAsk::parse(&["--help".into()]).unwrap(), RunAsk::Help);
        assert_eq!(RunAsk::parse(&["--cwd".into(), "/tmp".into(), "--".into(), "ls".into(), "-la".into()]).unwrap(), RunAsk::Exec { cwd: Some("/tmp".into()), argv: vec!["ls".into(), "-la".into()] });
        assert_eq!(
            RunAsk::parse(&["--cwd=~/src".into(), "echo".into(), "-n".into(), "hi".into()]).unwrap(),
            RunAsk::Exec { cwd: Some("~/src".into()), argv: vec!["echo".into(), "-n".into(), "hi".into()] }
        );
        assert!(RunAsk::parse(&[]).is_err());
        assert!(RunAsk::parse(&["--bogus".into(), "ls".into()]).is_err());
    }

    #[test]
    fn format_command_quotes_spaces() {
        assert_eq!(format_command(&["echo".into(), "hello world".into(), "a'b".into()]), "echo 'hello world' 'a'\\''b'");
    }

    #[tokio::test]
    async fn run_job_allow_deny_stream_and_cancel() {
        let jobs = RunJobs::new();
        let notes = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let notes_cb = notes.clone();
        let quiet = Arc::new(move |t: String, b: String| {
            notes_cb.lock().unwrap().push((t, b));
        });

        // 1. Denied
        let approvals = RunApprovals::new(|| {}, || {});
        let app_clone = approvals.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                if let Some(p) = app_clone.current() {
                    app_clone.respond(&p.id, false);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let mut r = jobs.start(Some("deny-1".into()), "far".into(), RunAsk::parse(&["echo".into(), "nope".into()]).unwrap(), approvals.clone(), quiet.clone()).await;
        while r.get("done").and_then(Value::as_bool) != Some(true) {
            r = jobs.poll("deny-1").await;
        }
        assert_eq!(r["ok"], false);
        assert_eq!(r["data"]["code"], 126);

        // 2. Allowed & streamed
        let app_clone = approvals.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                if let Some(p) = app_clone.current() {
                    app_clone.respond(&p.id, true);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let mut r =
            jobs.start(Some("ok-1".into()), "far".into(), RunAsk::parse(&["sh".into(), "-c".into(), "echo one; sleep 0.05; echo two; exit 0".into()]).unwrap(), approvals.clone(), quiet.clone()).await;
        let mut collected = r.get("stdout").and_then(Value::as_str).unwrap_or("").to_owned();
        while r.get("done").and_then(Value::as_bool) != Some(true) {
            r = jobs.poll("ok-1").await;
            collected.push_str(r.get("stdout").and_then(Value::as_str).unwrap_or(""));
        }
        assert_eq!(r["ok"], true);
        assert_eq!(r["data"]["code"], 0);
        assert!(collected.contains("one") && collected.contains("two"), "got: {collected:?}");

        // 3. Long command cancelled
        let app_clone = approvals.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                if let Some(p) = app_clone.current() {
                    app_clone.respond(&p.id, true);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let mut r = jobs.start(Some("cancel-1".into()), "far".into(), RunAsk::parse(&["sh".into(), "-c".into(), "echo started; sleep 30".into()]).unwrap(), approvals.clone(), quiet).await;
        let mut out = r.get("stdout").and_then(Value::as_str).unwrap_or("").to_owned();
        for _ in 0..20 {
            if out.contains("started") {
                break;
            }
            r = jobs.poll("cancel-1").await;
            out.push_str(r.get("stdout").and_then(Value::as_str).unwrap_or(""));
        }
        assert!(out.contains("started"), "expected streamed line before cancel, got {out:?}");
        jobs.cancel("cancel-1");
        while r.get("done").and_then(Value::as_bool) != Some(true) {
            r = jobs.poll("cancel-1").await;
        }
        assert_eq!(r["ok"], false);
        assert_eq!(r["data"]["code"], 130);
    }
}
