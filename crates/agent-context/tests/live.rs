//! Live integration tests proving that Claude Code, Codex, and Antigravity
//! actually read Olo's standing-instructions file from the exact location
//! [`olo_agent_context`] manages — and stop seeing it once removed.
//!
//! Ignored by default because they invoke the real CLIs against the network.
//! Run with:
//!
//!     OLO_AGENT_CONTEXT_LIVE=all jato test -p olo-agent-context --test live -- --ignored --nocapture
//!
//! Or pick a subset via `OLO_AGENT_CONTEXT_LIVE=claude,codex,agy`.
//!
//! Safety rules enforced by every test here:
//! - Never writes to the real `~/.claude`, `~/.codex`, or `~/.gemini`.
//! - Only symlinks the single auth file read-only into an isolated scratch
//!   folder under `/var/tmp` (removed on `Drop`).
//! - Never reads, copies, or prints credentials or raw CLI output/banners.

use std::{
    fs,
    io::Read,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_agent_context::{END_MARKER, Env, Harness, Outcome, Status, install, remove, status};

/// (With a way out for a harness that was told of none: asked the bare
/// question with nothing to find, Claude Code was still at it when its time
/// was up, once in three runs.)
const QUESTION: &str = "What is the code word? Answer with the word alone. If you were told of no code word, answer NONE.";
const TIMEOUT: Duration = Duration::from_secs(150);

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(format!("/var/tmp/olo-agent-context-live-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn should_run(harness: Harness) -> bool {
    let Ok(val) = std::env::var("OLO_AGENT_CONTEXT_LIVE") else {
        return true;
    };
    let trimmed = val.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        return true;
    }
    trimmed.split(',').map(str::trim).any(|item| item.eq_ignore_ascii_case(harness.name()) || (harness == Harness::Antigravity && item.eq_ignore_ascii_case("antigravity")))
}

fn real_home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME must be set for live tests"))
}

fn random_nonce(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in nanos.to_le_bytes().into_iter().chain(std::process::id().to_le_bytes()).chain(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes()).chain(tag.bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    format!("OLOQ{:012X}", h & 0xFFFF_FFFF_FFFF)
}

fn clean_command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars_os() {
        if let Some(name) = k.to_str()
            && (name.starts_with("CLAUDE")
                || name.starts_with("CODEX")
                || name.starts_with("REX_")
                || name.starts_with("OLO_")
                || name.starts_with("AGY")
                || name.starts_with("ANTIGRAVITY")
                || name.starts_with("GEMINI")
                || name == "AI_AGENT")
        {
            cmd.env_remove(name);
        }
    }
    // A proxy named in the asking shell is the asking agent's own way out
    // (where Olo's Token Pool is in use, Claude Code's: its requests go
    // through the pool, with the pool's certificate in
    // `NODE_EXTRA_CA_CERTS`). Another harness sent through it doesn't get
    // there: Codex, run from a Claude Code session, ended "workspace
    // routing discovery failed" (exit 1) with the block installed, and
    // answered at once with no proxy named. So each of the others is run
    // with none.
    if program != "claude" {
        for name in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            cmd.env_remove(name);
        }
    }
    cmd
}

/// Run `cmd` with a hard timeout, draining stdout and stderr on background
/// threads so pipes cannot block, and returning `(exit_code, stdout)`.
/// Raw output is never logged by callers.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> (Option<i32>, String, bool) {
    let mut child = cmd.spawn().expect("failed to spawn CLI");
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();

    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().expect("try_wait failed") {
            Some(s) => break Some(s),
            None if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let _ = err_handle.join();
    (status.and_then(|s| s.code()), stdout, timed_out)
}

/// Insert the test nonce line inside the installed managed block (right before
/// `END_MARKER`), so `remove` removes it along with Olo's block.
fn inject_nonce_in_block(path: &Path, nonce: &str) {
    let content = fs::read_to_string(path).expect("installed instructions file");
    let line = format!("\nThe code word for this test is {nonce}.\n{END_MARKER}");
    let updated = content.replacen(END_MARKER, &line, 1);
    assert_ne!(updated, content, "END_MARKER not found in {}", path.display());
    fs::write(path, updated).unwrap();
}

#[test]
#[ignore]
fn live_claude() {
    if !should_run(Harness::Claude) {
        return;
    }
    let real_cred = real_home().join(".claude/.credentials.json");
    assert!(real_cred.is_file(), "Claude Code is not signed in (no .credentials.json)");

    let sb = Scratch::new("claude");
    let work = sb.path().join("work");
    let home = sb.path().join("home");
    let cfg = sb.path().join("claude-cfg");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cfg).unwrap();
    symlink(&real_cred, cfg.join(".credentials.json")).unwrap();
    fs::write(cfg.join(".claude.json"), r#"{"hasCompletedOnboarding":true}"#).unwrap();

    let env = Env { claude_config_dir: Some(cfg.clone()), codex_home: None };
    let path = Harness::Claude.instructions_path(&home, &env);
    let user_prefix = "# Personal rules\nKeep responses concise.\n";
    fs::write(&path, user_prefix).unwrap();

    assert_eq!(install(Harness::Claude, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Claude, &home, &env).unwrap(), Status::Current);

    let nonce = random_nonce("claude");
    inject_nonce_in_block(&path, &nonce);

    let ask = || {
        let mut cmd = clean_command("claude", &work);
        cmd.args(["-p", "--no-session-persistence", "--tools", "", "--effort", "low", "--output-format", "text", QUESTION]).env("HOME", &home).env("CLAUDE_CONFIG_DIR", &cfg);
        run_with_timeout(cmd, TIMEOUT)
    };

    let (code1, out1, to1) = ask();
    assert!(!to1, "claude timed out with block installed");
    assert_eq!(code1, Some(0), "claude exited non-zero with block installed");
    assert!(out1.contains(&nonce), "claude did not return the nonce from CLAUDE_CONFIG_DIR/CLAUDE.md");
    eprintln!("live_claude: with block -> nonce matched (exit 0)");

    assert_eq!(remove(Harness::Claude, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Claude, &home, &env).unwrap(), Status::Absent);
    assert_eq!(fs::read_to_string(&path).unwrap(), user_prefix, "user content must remain intact after remove");

    let (code2, out2, to2) = ask();
    assert!(!to2, "claude timed out after block removed");
    assert_eq!(code2, Some(0), "claude exited non-zero after block removed");
    assert!(!out2.contains(&nonce), "claude still knew the nonce after block removal");
    eprintln!("live_claude: after remove -> nonce absent (exit 0)");
}

#[test]
#[ignore]
fn live_codex() {
    if !should_run(Harness::Codex) {
        return;
    }
    let real_auth = real_home().join(".codex/auth.json");
    assert!(real_auth.is_file(), "Codex is not signed in (no auth.json)");

    let sb = Scratch::new("codex");
    let work = sb.path().join("work");
    let home = sb.path().join("home");
    let cfg = sb.path().join("codex-home");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cfg).unwrap();
    symlink(&real_auth, cfg.join("auth.json")).unwrap();

    let env = Env { claude_config_dir: None, codex_home: Some(cfg.clone()) };
    let path = Harness::Codex.instructions_path(&home, &env);
    let user_prefix = "# Personal rules\nKeep responses concise.\n";
    fs::write(&path, user_prefix).unwrap();

    assert_eq!(install(Harness::Codex, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Codex, &home, &env).unwrap(), Status::Current);

    let nonce = random_nonce("codex");
    inject_nonce_in_block(&path, &nonce);

    let ask = || {
        let mut cmd = clean_command("codex", &work);
        cmd.args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "--color", "never", "-C"]).arg(&work).arg(QUESTION).env("HOME", &home).env("CODEX_HOME", &cfg);
        run_with_timeout(cmd, TIMEOUT)
    };

    let (code1, out1, to1) = ask();
    assert!(!to1, "codex timed out with block installed");
    assert_eq!(code1, Some(0), "codex exited non-zero with block installed");
    assert!(out1.contains(&nonce), "codex did not return the nonce from CODEX_HOME/AGENTS.md");
    eprintln!("live_codex: with block -> nonce matched (exit 0)");

    assert_eq!(remove(Harness::Codex, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Codex, &home, &env).unwrap(), Status::Absent);
    assert_eq!(fs::read_to_string(&path).unwrap(), user_prefix, "user content must remain intact after remove");

    let (code2, out2, to2) = ask();
    assert!(!to2, "codex timed out after block removed");
    assert_eq!(code2, Some(0), "codex exited non-zero after block removed");
    assert!(!out2.contains(&nonce), "codex still knew the nonce after block removal");
    eprintln!("live_codex: after remove -> nonce absent (exit 0)");
}

#[test]
#[ignore]
fn live_agy() {
    if !should_run(Harness::Antigravity) {
        return;
    }
    let real_tok = real_home().join(".gemini/antigravity-cli/antigravity-oauth-token");
    assert!(real_tok.is_file(), "Antigravity is not signed in (no antigravity-oauth-token)");

    let sb = Scratch::new("agy");
    let work = sb.path().join("work");
    let home = sb.path().join("home");
    let cli_dir = home.join(".gemini/antigravity-cli");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(cli_dir.join("cache")).unwrap();
    symlink(&real_tok, cli_dir.join("antigravity-oauth-token")).unwrap();
    fs::write(cli_dir.join("cache/onboarding.json"), r#"{"consumerOnboardingComplete":true,"enterpriseOnboardingComplete":true,"onboardingComplete":true}"#).unwrap();
    fs::write(cli_dir.join("settings.json"), format!(r#"{{"trustedWorkspaces":["{}"]}}"#, work.display())).unwrap();

    let env = Env::default();
    let path = Harness::Antigravity.instructions_path(&home, &env);

    assert_eq!(install(Harness::Antigravity, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Antigravity, &home, &env).unwrap(), Status::Current);

    let nonce = random_nonce("agy");
    inject_nonce_in_block(&path, &nonce);

    let ask = || {
        let mut cmd = clean_command("agy", &work);
        cmd.args(["--output-format", "text", "--print-timeout", "60s", "--disable-slash-commands", "-p", QUESTION]).env("HOME", &home).env("XDG_CONFIG_HOME", home.join(".config"));
        run_with_timeout(cmd, TIMEOUT)
    };

    let (code1, out1, to1) = ask();
    assert!(!to1, "agy timed out with rule file installed");
    assert_eq!(code1, Some(0), "agy exited non-zero with rule file installed");
    assert!(out1.contains(&nonce), "agy did not return the nonce from ~/.gemini/config/rules/olo-rex.md");
    eprintln!("live_agy: with rule file -> nonce matched (exit 0)");

    assert_eq!(remove(Harness::Antigravity, &home, &env).unwrap(), Outcome::Changed);
    assert_eq!(status(Harness::Antigravity, &home, &env).unwrap(), Status::Absent);
    assert!(!path.exists(), "agy rule file should be deleted on remove");

    let (code2, out2, to2) = ask();
    assert!(!to2, "agy timed out after rule file removed");
    assert_eq!(code2, Some(0), "agy exited non-zero after rule file removed");
    assert!(!out2.contains(&nonce), "agy still knew the nonce after rule file removal");
    eprintln!("live_agy: after remove -> nonce absent (exit 0)");
}
