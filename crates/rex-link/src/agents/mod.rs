//! Children of a Rex tab (`olo agent`): agents (`claude`, `codex`,
//! `agy`) and plain commands, each a real Rex tab under the tab that
//! started it.
//!
//! **They are the host's.** The agents service on the host
//! (`olo-rex-link agent serve`, the helper's `agentd`) starts them,
//! reads their screens, answers and stops them through the host's own
//! Rex server, keeps who is whose child, and writes down what happens to
//! each. The `olo agent` command there asks it over a Unix socket; no
//! Olo desktop is between them, or need be connected. A desktop that
//! connects follows the service (`agent follow`) and shows what it says:
//! every child is there the moment it does.
//!
//! This is what the two agree on: the children and their tree
//! ([`model`]), what is asked ([`spec`]), how a child stands
//! ([`state`]), what happened ([`events`]) and how it is said over the
//! socket ([`wire`]). All of it plain Rust with nothing of the service's
//! own machinery, so the desktop carries none of that.

pub mod events;
pub mod minder;
pub mod model;
pub mod spec;
pub mod state;
pub mod wire;

pub use events::{SubagentEvent, SubagentEventLog};
pub use model::{AgentCli, DesktopPolicy, NativeSubagent, SubagentRegistry, TreeRow, new_subagent_id, now_unix_ms, screen_target_name, to_unix_ms};
pub use spec::{Answer, ApproveSpec, Pressed, SendPayload, StartSpec, WatchSpec, ctrl_byte, is_minder_chooser_key, parse_send_payload, pressed_key, sanitize_paste_text, special_key, start_usage};
pub use state::{Activity, ScreenClassification, SubagentLiveState, approval_to_json, classify_screen_rich, effective_state, summary_json};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    /// The keys the server's encoder presses: those xterm's bytes can't
    /// say. The rest are their bytes.
    /// What a reading takes up, and when a Codex's conversation file is
    /// believed that a turn is on (a code review's case and the ShiftUp
    /// lead's items 50 and 51, 2026-11-27).
    #[test]
    fn a_reading_takes_up_what_stood_when_it_began_and_a_codex_in_a_turn_is_not_at_rest() {
        use super::state::{Expected, TURN_ON_MS, takes_up, turn_on};
        let given = |cursor| Some(Expected { cursor, given_ms: 0, until_ms: 0, transcript_len: None, turn_from_ms: 0, seq: 1 });
        // The message that stood when the reading began, seen begun: taken up.
        assert!(takes_up(true, Some(1), given(7)));
        // A reading begun while turn A was on (nothing given then), and a
        // message noted since: not that one's beginning.
        assert!(!takes_up(true, None, given(8)));
        // One given at the reading, another since (each giving has its own number, whatever its cursor).
        assert!(!takes_up(true, Some(1), Some(Expected { seq: 2, ..given(7).unwrap() })));
        // No turn seen on; nothing given.
        assert!(!takes_up(false, Some(1), given(7)));
        assert!(!takes_up(true, None, None));

        let now = 1_000_000;
        // (A turn of the life before can have begun a moment before this child was asked for.)
        let (made, this_life, life_before) = (900_000, Some(900_000), Some(899_999));
        // Codex's file says a task is started and not complete, begun in this child's life, and it wrote to
        // its screen a second ago: in a turn, whatever the screen reads.
        assert!(turn_on(AgentCli::Codex, Some("working"), this_life, made, 0, now - 1_000, now, TURN_ON_MS));
        // Or wrote its file a second ago.
        assert!(turn_on(AgentCli::Codex, Some("working"), this_life, made, now - 1_000, now - 60_000, now, TURN_ON_MS));
        // Its file says the turn is over, was aborted, or says nothing: by its screen.
        for said in [Some("done"), Some("aborted"), Some("waiting"), Some("error"), Some("unknown"), None] {
            assert!(!turn_on(AgentCli::Codex, said, this_life, made, now, now, now, TURN_ON_MS), "{said:?}");
        }
        // A turn of the conversation's life before this child (killed then, the file left saying so), or a
        // file that doesn't say when its turn began: not this child's turn, however lively it is as it starts.
        assert!(!turn_on(AgentCli::Codex, Some("working"), life_before, made, now, now, now, TURN_ON_MS));
        assert!(!turn_on(AgentCli::Codex, Some("working"), None, made, now, now, now, TURN_ON_MS));
        // A turn of its own that never ended, the agent still for longer than the file is believed: at rest.
        assert!(!turn_on(AgentCli::Codex, Some("working"), this_life, made, now - TURN_ON_MS, now - TURN_ON_MS - 1, now, TURN_ON_MS));
        assert!(!turn_on(AgentCli::Codex, Some("working"), this_life, made, 0, 0, now, TURN_ON_MS));
        // The others say on their screens that a turn is on, all through it.
        assert!(!turn_on(AgentCli::Claude, Some("working"), this_life, made, now, now, now, TURN_ON_MS));
        assert!(!turn_on(AgentCli::Agy, Some("working"), this_life, made, now, now, now, TURN_ON_MS));
    }

    #[test]
    fn the_keys_that_bytes_cannot_say() {
        let press = |raw: &str| pressed_key(raw).map(|p| (p.key, p.mods));
        assert_eq!(press("Ctrl+Enter"), Some(("enter", "ctrl".to_owned())));
        assert_eq!(press("shift+return"), Some(("enter", "shift".to_owned())));
        assert_eq!(press("C-M-S-Enter"), Some(("enter", "ctrl+alt+shift".to_owned())));
        assert_eq!(press("Ctrl+Tab"), Some(("tab", "ctrl".to_owned())));
        assert_eq!(press("Shift+Backspace"), Some(("backspace", "shift".to_owned())));
        assert_eq!(press("Shift+Escape"), Some(("escape", "shift".to_owned())));
        for theirs in ["Enter", "Alt+Enter", "Tab", "Shift+Tab", "Escape", "Backspace", "Ctrl+Backspace", "Alt+m", "Ctrl+Shift+Up", "F5", "x", "-", "^C"] {
            assert_eq!(press(theirs), None, "{theirs}");
        }
        // (And its bytes are still there, for a server with no such word.)
        assert_eq!(special_key("Ctrl+Enter").unwrap(), "\r");
    }

    /// A turn's end is an end, whatever the agent left running (ShiftUp's
    /// item 24). The screen is Claude Code's after a turn that started a
    /// shell in the background and ended with a question, read from one
    /// (2026-11-26): Rex still calls the pane working, for the shell.
    #[test]
    fn a_turn_that_ended_with_a_shell_still_running_has_ended() {
        let named = format!("{} probe24 ─", "─".repeat(86));
        let rule = "─".repeat(96);
        let ended = format!("● QUESTION: is this seen?\n\n✻ Cooked for 9s · 1 shell still running\n\n{named}\n❯ \n{rule}\n  ⏵⏵ auto mode on · 1 shell · ← 1 agent · ↓ to manage\n");
        let cls = classify_screen_rich(AgentCli::Claude, Activity::Working, &ended, true);
        assert_eq!(cls.state, SubagentLiveState::Idle, "at its prompt with no turn on: not working, whatever Rex says of the pane");
        // With its transcript's word that the turn ended, it is done; with a question read there, it asks.
        assert_eq!(effective_state(&cls, Some("done"), None, true).0, SubagentLiveState::Done);
        assert_eq!(effective_state(&cls, Some("waiting"), None, true).0, SubagentLiveState::WaitingQuestion);
        // In a turn (its footer says so), with the same shell: working, by its own screen.
        let turning = format!("✢ Fermenting… (37s · ↓ 3.0k tokens)\n\n{named}\n❯ \n{rule}\n  ⏵⏵ auto mode on · 1 shell · esc to interrupt · ← 1 agent · ↓ to manage\n");
        let cls = classify_screen_rich(AgentCli::Claude, Activity::Working, &turning, true);
        assert_eq!((cls.state, cls.summary.as_deref()), (SubagentLiveState::Working, Some("✢ Fermenting… (37s · ↓ 3.0k tokens)")));
        // A screen that isn't its prompt and says nothing of itself: Rex's word still stands.
        let unknown = "Compiling olo v0.1.0\n   Building [=====>   ] 12/40\n";
        assert_eq!(classify_screen_rich(AgentCli::Claude, Activity::Working, unknown, true).state, SubagentLiveState::Working);
    }

    /// An agent at work is said by what it is doing, the line over its
    /// field, never by its footer; whatever stands in the rule over the
    /// field, and for an agent that draws no rules.
    #[test]
    fn an_agent_at_work_is_said_by_the_line_over_its_field() {
        use crate::agents::state::doing_line;
        let rule = "─".repeat(96);
        // Claude Code, the session's name in the rule over its field.
        let named = format!("──────────────── opus-batch2 {}", "─".repeat(60));
        let claude = format!(
            "● Read 6 files\n\n✽ Hatching… (3m 0s · ↓ 16.9k tokens · deep in thought with max effort)\n\n{named}\n❯ \n{rule}\n  ⏵⏵ accept edits on (shift+tab to cycle) · esc to interrupt · ← 1 agent\n"
        );
        assert_eq!(doing_line(&claude).as_deref(), Some("✽ Hatching… (3m 0s · ↓ 16.9k tokens · deep in thought with max effort)"));
        let cls = classify_screen_rich(AgentCli::Claude, Activity::Working, &claude, true);
        assert_eq!((cls.state, cls.summary.as_deref()), (SubagentLiveState::Working, Some("✽ Hatching… (3m 0s · ↓ 16.9k tokens · deep in thought with max effort)")));
        // Antigravity: its spinner's line, over two rules and its mode's hint.
        let agy = format!(
            "{rule}\n> Read /var/tmp/x/brief.md and do what it says.\n⣾  Generating...\n{rule}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{rule}\nesc to cancel          accept-edits · Gemini 4 Neon · xhigh\n"
        );
        assert_eq!(doing_line(&agy).as_deref(), Some("⣾  Generating..."));
        // A tip under its work isn't what it is doing.
        let tipped = format!("✻ Thinking… (12s)\n└ Tip: press shift+tab to cycle modes\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        assert_eq!(doing_line(&tipped).as_deref(), Some("✻ Thinking… (12s)"));
        // (Claude Code 2.1.332's word of its effort, set at the right under its work, isn't what it is doing: as read off one, 2026-11-26.)
        let hinted = format!(
            "● Background: wait for the build\n  ⎿  $ echo STARTED_WAITING; sleep 45\n✢ Whatchamacalliting… (0s · ↓ 2 tokens)\n{}◈ max · /effort\n{} waiter ─\n❯ \n{rule}\n  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt · ← for agents\n",
            " ".repeat(103),
            "─".repeat(111)
        );
        assert_eq!(doing_line(&hinted).as_deref(), Some("✢ Whatchamacalliting… (0s · ↓ 2 tokens)"));
        // No rules (Codex, drawn inline): the line that says it works.
        let codex = "• Ran cargo test\n\n• Working (12s • esc to interrupt)\n\n› Ask Codex to do anything\n  gpt-6.2-astra high · ~/src · Context 91% left\n";
        assert_eq!(doing_line(codex).as_deref(), Some("• Working (12s • esc to interrupt)"));
        // With a long message waiting in its field: still its status line, never its footer. And with no
        // status line on its screen (its turn on by its conversation's file alone): the last it said.
        let waiting = (1..=9).map(|i| format!("  line {i} of what the lead sent")).collect::<Vec<_>>().join("\n");
        let long = format!("• Ran cargo test\n\n• Working (12s • esc to interrupt)\n\n› First line\n{waiting}\n\n  gpt-6.2-astra high · ~/src · Context 91% left\n  ← for agents · ? for shortcuts\n");
        assert_eq!(doing_line(&long).as_deref(), Some("• Working (12s • esc to interrupt)"));
        assert_eq!(doing_line(&long.replace("• Working (12s • esc to interrupt)\n", "")).as_deref(), Some("• Ran cargo test"));
        // Nothing drawn yet.
        assert_eq!(doing_line("\n  \n"), None);
    }

    #[test]
    fn a_key_is_sent_as_a_terminal_sends_it_with_any_modifier() {
        let key = |name: &str| special_key(name).unwrap();
        // (As before.)
        assert_eq!(key("Enter"), "\r");
        assert_eq!(key("Escape"), "\x1b");
        assert_eq!(key("Up"), "\x1b[A");
        assert_eq!(key("Shift-Tab"), "\x1b[Z");
        assert_eq!(key("Ctrl-C"), "\x03");
        assert_eq!(key("ctrl+d"), "\x04");
        assert_eq!(key("^C"), "\x03");
        assert_eq!(key("PageDown"), "\x1b[6~");
        // Alt is the escape before the key: what tmux sends for `M-m`.
        assert_eq!(key("Alt+m"), "\x1bm");
        assert_eq!(key("M-m"), "\x1bm");
        assert_eq!(key("Meta+M"), "\x1bM");
        assert_eq!(key("Alt+Shift+m"), "\x1bM");
        assert_eq!(key("alt-enter"), "\x1b\r");
        assert_eq!(key("Alt+Backspace"), "\x1b\x7f");
        assert_eq!(key("Ctrl+Alt+c"), "\x1b\x03");
        assert_eq!(key("C-M-c"), "\x1b\x03");
        assert_eq!(key("Alt+-"), "\x1b-");
        assert_eq!(key("Alt++"), "\x1b+");
        // A named key with modifiers, as xterm sends it.
        assert_eq!(key("Ctrl+Up"), "\x1b[1;5A");
        assert_eq!(key("Ctrl+Shift+Up"), "\x1b[1;6A");
        assert_eq!(key("Alt+Left"), "\x1b[1;3D");
        assert_eq!(key("Shift+End"), "\x1b[1;2F");
        assert_eq!(key("Ctrl+Delete"), "\x1b[3;5~");
        assert_eq!(key("Shift+Tab"), "\x1b[Z");
        assert_eq!(key("Ctrl+Space"), "\0");
        assert_eq!(key("F1"), "\x1bOP");
        assert_eq!(key("F5"), "\x1b[15~");
        assert_eq!(key("Shift+F5"), "\x1b[15;2~");
        assert_eq!(key("Ctrl+F2"), "\x1b[1;5Q");
        assert_eq!(key("F12"), "\x1b[24~");
        // What isn't a key says what is.
        // A character by itself is that key (it was refused: a box that
        // takes one key and no text couldn't be answered in a tab).
        assert_eq!(key("m"), "m");
        assert_eq!(key("0"), "0");
        assert_eq!(key("Shift+y"), "Y");
        assert_eq!(key("é"), "é");
        assert!(special_key("\u{7}").is_err());
        assert!(special_key("ab").unwrap_err().contains("alone or with"));
        for (name, types) in [("0", true), ("y", true), ("Shift+y", true), ("Enter", false), ("Alt+m", false), ("Ctrl+c", false), ("^C", false), ("F5", false), ("Up", false)] {
            assert_eq!(super::spec::key_types(name), types, "{name}");
        }
        assert!(special_key("Hyper+x").is_err());
        assert!(special_key("F13").is_err());
        assert!(special_key("").is_err());
    }

    #[test]
    fn start_always_requires_explicit_model_and_effort_and_rejects_non_codex_approve_for_me() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(StartSpec::parse_cli_args(&args(&["claude"])).unwrap().build_command().unwrap_err().contains("--model"));
        assert!(StartSpec::parse_cli_args(&args(&["claude", "--model", "claude-opus-5-5"])).unwrap().build_command().unwrap_err().contains("--effort"));
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--resume", "abc123", "--model", "gpt-5.4-codex"])).unwrap().build_command().unwrap_err().contains("--effort"));
        assert!(StartSpec::parse_cli_args(&args(&["agy", "--continue", "--effort", "xhigh"])).unwrap().build_command().unwrap_err().contains("--model"));
        // --approve-for-me is Codex-only!
        assert!(StartSpec::parse_cli_args(&args(&["claude", "--model", "claude-opus-5-5", "--effort", "max", "--approve-for-me"])).unwrap().build_command().unwrap_err().contains("Codex-only"));
    }

    #[test]
    fn builds_commands_with_orchestration_flags_and_desktop_isolation() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let spec = StartSpec::parse_cli_args(&args(&[
            "claude",
            "--model",
            "claude-opus-5-5",
            "--effort",
            "max",
            "--name",
            "builder-1",
            "--trust",
            "--session-id",
            "11111111-2222-3333-4444-555555555555",
            "--strict-mcp-config",
            "--settings",
            "{\"hooks\":{}}",
            "--disallowed-tools",
            "Agent",
            "Workflow",
            "--add-dir",
            "/var/tmp/shared",
            "--permission-mode",
            "auto",
            "--unset-env",
            "CUSTOM_SECRET",
        ]))
        .unwrap();
        assert!(spec.trust);
        assert_eq!(spec.desktop, DesktopPolicy::None);
        assert_eq!(spec.session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        let (_, _, _, argv) = spec.build_command().unwrap();
        assert!(argv.windows(2).any(|w| w == ["--session-id", "11111111-2222-3333-4444-555555555555"]));
        assert!(argv.contains(&"--strict-mcp-config".to_owned()));
        assert!(argv.windows(3).any(|w| w == ["--disallowed-tools", "Agent", "Workflow"]));
        assert!(argv.windows(2).any(|w| w == ["--settings", "{\"hooks\":{}}"]));
        assert!(argv.windows(2).any(|w| w == ["--permission-mode", "auto"]));
        assert!(argv.windows(2).any(|w| w == ["--name", "builder-1"]));
        let wrapped = spec.wrap_with_env_isolation(argv);
        assert_eq!(wrapped[0], "env");
        assert!(wrapped.windows(2).any(|w| w == ["-u", "REX_SESSION"]));
        assert!(wrapped.windows(2).any(|w| w == ["-u", "DISPLAY"]));
        assert!(wrapped.windows(2).any(|w| w == ["-u", "CUSTOM_SECRET"]));

        // Agy stream mode + accept-edits + settings (`--print=` string flag, before extra!)
        let agy = StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon", "--effort", "xhigh", "--mode", "accept-edits", "--settings", "{\"allow\":[]}", "--stream"])).unwrap();
        let (_, _, _, a_argv) = agy.build_command().unwrap();
        assert!(a_argv.contains(&"--print=".to_owned()), "agy argv: {a_argv:?}");
        assert!(a_argv.windows(2).any(|w| w == ["--input-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--output-format", "stream-json"]));
        assert!(a_argv.windows(2).any(|w| w == ["--mode", "accept-edits"]));
        assert!(a_argv.windows(2).any(|w| w == ["--settings", "{\"allow\":[]}"]));

        // Golden argv checks (`H12`, `M6`, `M7`, `M8`):
        // 1. Codex `-i` placed before `-m`, and `--` before prompt starting with `-` / `---`.
        let cx = StartSpec::parse_cli_args(&args(&[
            "codex",
            "--model",
            "gpt-5.4-codex",
            "--effort",
            "high",
            "--image",
            "shot.png",
            "--resume",
            "11111111-2222-3333-4444-555555555555",
            "--prompt",
            "---\nfront matter\n---",
        ]))
        .unwrap();
        let (_, _, _, cx_argv) = cx.build_command().unwrap();
        let img_pos = cx_argv.iter().position(|x| x == "-i").unwrap();
        let mod_pos = cx_argv.iter().position(|x| x == "-m").unwrap();
        let sep_pos = cx_argv.iter().position(|x| x == "--").unwrap();
        assert!(img_pos < mod_pos && mod_pos < sep_pos, "codex argv ordering wrong: {cx_argv:?}");
        assert_eq!(cx_argv[sep_pos + 1], "11111111-2222-3333-4444-555555555555");
        assert_eq!(cx_argv[sep_pos + 2], "---\nfront matter\n---");

        // 2. Bare `-w` does NOT eat the positional prompt (`M6`), while `--worktree=feat` sets name.
        let wt1 = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "-w", "fix the parser"])).unwrap();
        assert_eq!(wt1.worktree, Some(None));
        assert_eq!(wt1.prompt.as_deref(), Some("fix the parser"));
        let wt2 = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "--worktree=feat-1", "fix the parser"])).unwrap();
        assert_eq!(wt2.worktree, Some(Some("feat-1".into())));

        // 3. Per-CLI unsupported flags rejected (`M7`/`M8`) & Claude `--stream` adds `--verbose`.
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--model", "m", "--effort", "high", "--session-id", "abc"])).unwrap().build_command().is_err());
        assert!(StartSpec::parse_cli_args(&args(&["codex", "--model", "m", "--effort", "high", "--permission-mode", "acceptEdits"])).unwrap().build_command().is_err());
        assert!(StartSpec::parse_cli_args(&args(&["agy", "--model", "gemini-4-neon-xhigh", "--effort", "high"])).unwrap().build_command().is_err());
        let cl_stream = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "high", "--stream", "--prompt", "hi"])).unwrap();
        let (_, _, _, cl_s_argv) = cl_stream.build_command().unwrap();
        assert!(cl_s_argv.contains(&"--verbose".to_owned()), "claude --stream must include --verbose: {cl_s_argv:?}");
    }

    #[test]
    fn desktop_policy_enforces_none_and_scoped_screens() {
        let none = DesktopPolicy::None;
        assert!(none.check_action("olo.op", None).is_err());
        assert!(none.check_action("olo.screen", Some(&json!({"args": ["start", "qa-1"]}))).is_err());
        assert!(none.check_action("olo.open", None).is_err());
        assert!(none.check_action("olo.clipboard.get", None).is_err());
        // Catch-all & non-allowlisted actions MUST be denied (`C1`)!
        assert!(none.check_action("agent.auto_approve_toggle", None).is_err());
        assert!(none.check_action("olo.passwords.fill", None).is_err());
        assert!(none.check_action("olo.install_remote_skill", None).is_err());
        assert!(none.check_action("olo.agent.list", None).is_ok());
        assert!(none.check_action("olo.wait", None).is_ok());
        assert!(none.check_action("olo.file", None).is_ok());
        // Privilege attenuation (`C2`): none cannot escalate to screens or inherit!
        assert!(none.restrict_to(&DesktopPolicy::Inherit).is_err());
        assert!(none.restrict_to(&DesktopPolicy::parse("screens=qa-1").unwrap()).is_err());
        assert!(none.restrict_to(&DesktopPolicy::None).is_ok());

        let scoped = DesktopPolicy::parse("screens=qa-1,qa-2").unwrap();
        assert!(scoped.check_action("olo.op", None).is_err());
        assert!(scoped.check_action("olo.open", None).is_err());
        assert!(scoped.check_action("agent.auto_approve_toggle", None).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["look", "qa-1", "/var/tmp/x.png"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "qa-2"]}))).is_ok());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["tree", "other-screen"]}))).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["--owner", "qa-1", "look", "other"]}))).is_err());
        assert!(scoped.check_action("olo.screen", Some(&json!({"args": ["list"]}))).is_err());
        assert!(scoped.restrict_to(&DesktopPolicy::parse("screens=qa-1").unwrap()).is_ok());
        assert!(scoped.restrict_to(&DesktopPolicy::parse("screens=qa-1,qa-3").unwrap()).is_err());
        assert!(scoped.restrict_to(&DesktopPolicy::Inherit).is_err());
    }

    #[test]
    fn classifies_screens_without_false_approval_events() {
        // Bare AgentActivity::Blocked with empty screen stays Working (startup frame), while non-empty
        // unrecognised Blocked dialog fails visible as WaitingQuestion (`H2`), never WaitingApproval!
        let blank = classify_screen_rich(AgentCli::Claude, Activity::Blocked, "", true);
        assert_eq!(blank.state, SubagentLiveState::Working);
        assert!(blank.approval.is_none());
        let unknown_dlg = classify_screen_rich(AgentCli::Claude, Activity::Blocked, "Some brand-new prompt widget", true);
        assert_eq!(unknown_dlg.state, SubagentLiveState::WaitingQuestion);

        // Real Agy approval prompt emits WaitingApproval with structured choices
        let agy_ask = "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n";
        let ap = classify_screen_rich(AgentCli::Agy, Activity::Idle, agy_ask, true);
        assert_eq!(ap.state, SubagentLiveState::WaitingApproval);
        assert!(ap.approval.is_some());

        // H14: parallel pending tool calls MUST NOT overwrite screen command with a different longer tool!
        let ap_prompt = ap.approval.as_ref().unwrap();
        let pending = vec![json!({"tool": "Bash", "command": "git status --porcelain=v2 --untracked-files=all"}), json!({"tool": "Bash", "command": "rm -f /var/tmp/x"})];
        let ap_j = approval_to_json(ap_prompt, &pending);
        assert_eq!(ap_j["command"], json!("rm -f /var/tmp/x"));
        let mismatch_only = vec![json!({"tool": "Bash", "command": "git status --porcelain=v2 --untracked-files=all"})];
        let ap_m = approval_to_json(ap_prompt, &mismatch_only);
        assert_eq!(ap_m["command"], json!("rm -f /var/tmp/x"));
        assert!(ap_m["transcript_candidates"].as_array().is_some_and(|a| a.len() == 1));
        assert_eq!((&ap_j["command_from"], &ap_m["command_from"]), (&json!("screen"), &json!("screen")), "the box's own text, whole on the screen");

        // A box is told with its own command, never a like one's (ShiftUp's item 39): two commands that begin
        // alike for forty characters, the one before still listed as waiting.
        let before = "git -C /home/u/worktrees/shiftup-v2/cc-setup show 0f8dcff6 --stat";
        let own = "git -C /home/u/worktrees/shiftup-v2/cc-setup log --oneline 16f71b91..0f8dcff6";
        let call = |command: &str| json!({"tool": "run_command", "command": command});
        let box_of = |command: &str, folded: Option<usize>| olo_agent_screen::ApprovalPrompt { command: Some(command.to_owned()), folded_lines: folded, ..ap_prompt.clone() };
        // Whole on the screen: its own call's, with the other listed first; the screen's, where only the other waits.
        let told = approval_to_json(&box_of(own, None), &[call(before), call(own)]);
        assert_eq!((&told["command"], &told["command_from"]), (&json!(own), &json!("screen")));
        let told = approval_to_json(&box_of(own, None), &[call(before)]);
        assert_eq!(told["command"], json!(own), "never the call before's: {told}");
        assert!(told["transcript_candidates"].as_array().is_some_and(|a| a.len() == 1));
        // Wrapped on the screen (a line's end in a word's middle): still its own, by its whole text.
        let wrapped = "git -C /home/u/worktrees/shiftup-v2/cc-setup log --one\n   line 16f71b91..0f8dcff6";
        assert_eq!(approval_to_json(&box_of(wrapped, None), &[call(before), call(own)])["transcript_candidates"], serde_json::Value::Null, "its own call is found");
        // Only its head on the screen (folded, or cut): the one call that begins so gives the whole of it;
        let head = "git -C /home/u/worktrees/shiftup-v2/cc-setup log --oneline 16f7…";
        let told = approval_to_json(&box_of(head, Some(2)), &[call(before), call(own)]);
        assert_eq!((&told["command"], &told["command_from"]), (&json!(own), &json!("transcript")));
        // no call that begins so, or two different ones that do: no command at all, and why.
        let both = "git -C /home/u/worktrees/shiftup-v2/cc-setup …";
        for (shown, waiting) in [(head, vec![call(before)]), (both, vec![call(before), call(own)])] {
            let told = approval_to_json(&box_of(shown, None), &waiting);
            assert_eq!((&told["command"], &told["command_from"]), (&serde_json::Value::Null, &serde_json::Value::Null), "{told}");
            assert!(told["command_note"].as_str().is_some_and(|n| n.contains("only the head")) && told["screen_command"] == json!(shown));
        }
        // (The same command asked twice is one command.)
        let told = approval_to_json(&box_of(head, Some(2)), &[call(own), call(own)]);
        assert_eq!(told["command"], json!(own));

        // Claude folder-trust dialog is WaitingQuestion (never WaitingApproval)
        let trust = "────────────────────\n Accessing workspace:\n /var/tmp/wt\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n";
        let tr = classify_screen_rich(AgentCli::Claude, Activity::Blocked, trust, true);
        assert_eq!(tr.state, SubagentLiveState::WaitingQuestion);
        assert!(tr.folder_trust.is_some());
        assert!(tr.approval.is_none());

        // API mid-response disconnect emits Error
        let err = "● Working\n  ⎿  API Error: Connection lost mid-response\n────────────────────\n❯ \n";
        let er = classify_screen_rich(AgentCli::Claude, Activity::Idle, err, true);
        assert_eq!(er.state, SubagentLiveState::Error);
        assert!(er.api_error.as_deref().unwrap().contains("Connection lost mid-response"));

        // H18: sanitize_paste_text strips bracketed paste escapes & raw ESC/C0 while keeping tabs/newlines!
        assert_eq!(sanitize_paste_text("hi\x1b[201~\r\n/exit\x1b[200~\t\x03ok"), "hi\n/exit\tok");
    }

    /// When a child read at rest is told to be so (`state::rest_now`).
    #[test]
    fn a_rest_is_told_when_it_can_be_a_turns_end() {
        use crate::agents::state::{Expected, Rest, UNDRAWN_MS, Written, rest_now};
        let (started, now) = (1_000_000, 1_002_000);
        // Started with nothing to do: at its own prompt it is at rest, at once, worked or not.
        assert_eq!(rest_now(now, started, false, true, None, None), Rest::Told { answered: None });
        // What is on its screen isn't its own yet (the tab's shell): not for its first fifteen seconds; then as it stands.
        assert_eq!(rest_now(now, started, false, false, None, None), Rest::Kept { until_ms: started + UNDRAWN_MS });
        assert_eq!(rest_now(started + UNDRAWN_MS, started, false, false, None, None), Rest::Told { answered: None });
        // (One that has worked rests wherever it stands.)
        assert_eq!(rest_now(now, started, true, false, None, None), Rest::Told { answered: None });

        // Given work it hasn't been seen at: the rest before that turn, at its prompt or not, worked before or not.
        let given = Expected { cursor: 41, given_ms: now, until_ms: now + 30_000, transcript_len: Some(900), turn_from_ms: now, seq: 1 };
        for (seen, at_prompt) in [(false, true), (true, true), (true, false)] {
            assert_eq!(rest_now(now, started, seen, at_prompt, Some(given), None), Rest::Kept { until_ms: now + 30_000 });
        }
        let file = |len, over| Some(Written { len, over, at_ms: now + 500, said: true, turn_ms: None });
        // Its conversation's file the same, or longer with the turn not over (its message written, no reply yet): still before it.
        assert_eq!(rest_now(now, started, true, true, Some(given), file(900, true)), Rest::Kept { until_ms: now + 30_000 });
        assert_eq!(rest_now(now, started, true, true, Some(given), file(1400, false)), Rest::Kept { until_ms: now + 30_000 });
        // Longer, and its turn over: the turn came and went unseen. This rest is its end, and answers what was given.
        assert_eq!(rest_now(now, started, true, true, Some(given), file(1400, true)), Rest::Told { answered: Some(41) });
        // Its time up and nothing taken: told as it stands, answering nothing.
        assert_eq!(rest_now(now + 30_000, started, true, true, Some(given), file(900, true)), Rest::Told { answered: None });

        // Given a prompt at its start, its file's length then not known (a new child has none yet). At its own
        // prompt a moment later, nothing written: the rest before its turn, for as long as it is given to come to it.
        let at_start = Expected { cursor: 7, given_ms: started, until_ms: started + 120_000, transcript_len: None, turn_from_ms: started, seq: 1 };
        assert_eq!(rest_now(now, started, true, true, Some(at_start), None), Rest::Kept { until_ms: started + 120_000 });
        // A file last written before it was given (a conversation resumed: the old one's), with a message and its
        // turn over: says nothing of this turn.
        assert_eq!(
            rest_now(now, started, true, true, Some(at_start), Some(Written { len: 5000, over: true, at_ms: started - 60_000, said: true, turn_ms: None })),
            Rest::Kept { until_ms: started + 120_000 }
        );
        // Written since, its turn over, and nothing of the agent's in it (its own message alone): not yet.
        assert_eq!(rest_now(now, started, true, true, Some(at_start), Some(Written { len: 300, over: true, at_ms: now, said: false, turn_ms: None })), Rest::Kept { until_ms: started + 120_000 });
        // Written since, and not over: not yet.
        assert_eq!(rest_now(now, started, true, true, Some(at_start), Some(Written { len: 900, over: false, at_ms: now, said: true, turn_ms: None })), Rest::Kept { until_ms: started + 120_000 });
        // Written since, its turn over, a message of the agent's there: the prompt's turn, come and gone unseen.
        assert_eq!(rest_now(now, started, true, true, Some(at_start), Some(Written { len: 900, over: true, at_ms: now, said: true, turn_ms: None })), Rest::Told { answered: Some(7) });

        // Where the file says when its last turn began, that is what tells this turn from the one before
        // (both met under load, 2026-11-27).
        // A conversation taken up again with a prompt: its program wrote to the file as it started, and the
        // last turn in it is still the old one, over, with the old closing words. Not this turn.
        let old_turn = Written { len: 6000, over: true, at_ms: now, said: true, turn_ms: Some(started - 600_000) };
        assert_eq!(rest_now(now, started, true, true, Some(at_start), Some(old_turn)), Rest::Kept { until_ms: started + 120_000 });
        // A turn so quick that it was over before the service had noted its prompt (`given_ms` later than
        // the file's last writing): it began after the child was made, and is this turn.
        let late_noted = Expected { given_ms: started + 900, ..at_start };
        let quick = Written { len: 900, over: true, at_ms: started + 700, said: true, turn_ms: Some(started + 300) };
        assert_eq!(rest_now(now, started, true, true, Some(late_noted), Some(quick)), Rest::Told { answered: Some(7) });
        // A message sent at rest: the turn before it (begun before the message was typed, however little)
        // is not its turn, though the file has grown since; one begun after is.
        assert_eq!(
            rest_now(now, started, true, true, Some(given), Some(Written { len: 1400, over: true, at_ms: now + 500, said: true, turn_ms: Some(now - 1) })),
            Rest::Kept { until_ms: now + 30_000 }
        );
        assert_eq!(rest_now(now, started, true, true, Some(given), Some(Written { len: 1400, over: true, at_ms: now + 500, said: true, turn_ms: Some(now + 40) })), Rest::Told { answered: Some(41) });
    }

    /// A child is started in its agent's own automatic mode unless its parent says otherwise: by `--no-auto`, or
    /// by any word of its own about how leave is given, which is then all that is passed.
    #[test]
    fn a_child_is_started_in_auto_mode_unless_its_parent_says_otherwise() {
        let argv = |args: &[&str]| -> Vec<String> {
            let raw = json!({ "args": args });
            StartSpec::parse(Some(&raw)).unwrap_or_else(|e| panic!("{args:?}: {e}")).build_command().unwrap_or_else(|e| panic!("{args:?}: {e}")).3
        };
        let after = |argv: &[String], flag: &str| argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).cloned();
        // By default, each in its own word for it.
        assert_eq!(after(&argv(&["claude", "--model", "m", "--effort", "max"]), "--permission-mode").as_deref(), Some("auto"));
        assert_eq!(after(&argv(&["agy", "--model", "m", "--effort", "xhigh"]), "--mode").as_deref(), Some("auto"));
        assert!(argv(&["codex", "--model", "m", "--effort", "high"]).iter().any(|a| a == "--approve-for-me"));
        // Turned off by name: nothing about leave is passed, and the agent asks as it does by itself.
        let off = argv(&["claude", "--model", "m", "--effort", "max", "--no-auto"]);
        assert!(!off.iter().any(|a| a == "--permission-mode" || a == "--no-auto"), "{off:?}");
        assert!(!argv(&["agy", "--model", "m", "--effort", "xhigh", "--no-auto"]).iter().any(|a| a == "--mode"));
        assert!(!argv(&["codex", "--model", "m", "--effort", "high", "--no-auto"]).iter().any(|a| a == "--approve-for-me"));
        // The parent's own choice is all that is passed.
        assert_eq!(after(&argv(&["claude", "--model", "m", "--effort", "max", "--permission-mode", "acceptEdits"]), "--permission-mode").as_deref(), Some("acceptEdits"));
        assert_eq!(after(&argv(&["agy", "--model", "m", "--effort", "xhigh", "--mode", "accept-edits"]), "--mode").as_deref(), Some("accept-edits"));
        let codex = argv(&["codex", "--model", "m", "--effort", "high", "--approval", "untrusted"]);
        assert!(after(&codex, "-a").as_deref() == Some("untrusted") && !codex.iter().any(|a| a == "--approve-for-me"), "{codex:?}");
        let boxed = argv(&["codex", "--model", "m", "--effort", "high", "--sandbox", "read-only"]);
        assert!(!boxed.iter().any(|a| a == "--approve-for-me"), "{boxed:?}");
        // (Asked for by name beside another word of the parent's: both.)
        assert!(argv(&["codex", "--model", "m", "--effort", "high", "--sandbox", "workspace-write", "--auto"]).iter().any(|a| a == "--approve-for-me"));
    }

    /// Antigravity's own running tasks, by its footer (`state::agy_tasks`).
    #[test]
    fn antigravitys_footer_says_how_many_of_its_tasks_run() {
        use crate::agents::state::agy_tasks;
        let rule = "─".repeat(40);
        let footer = |end: &str| format!("  Waiting for the tests.\n{rule}\n>\n{rule}\n? for shortcuts            accept-edits · Gemini 4 Neon · xhigh{end}\n");
        assert_eq!(agy_tasks(&footer(" · 1 task(s) · /tasks")), 1);
        assert_eq!(agy_tasks(&footer(" · 12 task(s) · /tasks")), 12);
        assert_eq!(agy_tasks(&footer("")), 0);
        // (Said of tasks somewhere above its footer: not its footer's word.)
        assert_eq!(agy_tasks(&format!("  I started 3 task(s) for you.\n{}", footer(""))), 0);
    }

    /// What makes a box the box it is (`state::box_hash`): its command to the letter, its tool, its
    /// question and its choices; not a count running down in its question, nor which choice is marked.
    #[test]
    fn a_box_is_its_command_and_choices_to_the_letter() {
        use crate::agents::state::box_hash;
        use olo_agent_screen::{Agent, parse_approval};
        let (rule, dashed) = ("─".repeat(60), "╌".repeat(60));
        let asks = |command: &str, question: &str, marked: usize| {
            let mark = |i: usize| if i == marked { "❯" } else { " " };
            let screen = format!(
                "● Bash({command})\n{rule}\n Bash command\n Make it so\n{dashed}\n │ {command}\n{dashed}\n {question}\n {} 1. Yes\n {} 2. Yes, and don’t ask again for: {command}\n {} 3. No\n Esc to cancel · Tab to amend\n",
                mark(0),
                mark(1),
                mark(2)
            );
            parse_approval(Agent::Claude, &screen).unwrap_or_else(|| panic!("not read as a box:\n{screen}"))
        };
        let (five, nine) = (asks("sleep 5", "Do you want to proceed?", 0), asks("sleep 9", "Do you want to proceed?", 0));
        // (The screen's own key holds these two for one: every digit is aside in it.)
        assert_eq!(five.key, nine.key, "the screen's key has changed: this test's premise");
        assert_ne!(box_hash(&five), box_hash(&nine), "a command a digit different is another box");
        // The mark moved to another choice: the same box.
        assert_eq!(box_hash(&five), box_hash(&asks("sleep 5", "Do you want to proceed?", 2)));
        // A count in its question, a second later: the same box.
        assert_eq!(box_hash(&asks("sleep 5", "Do you want to proceed (blocked in 1:53)?", 0)), box_hash(&asks("sleep 5", "Do you want to proceed (blocked in 1:52)?", 0)));
        // Another question, the same command: another box.
        assert_ne!(box_hash(&five), box_hash(&asks("sleep 5", "Do you want to run this in the background?", 0)));
    }

    /// What says a child has taken up what it was given: its own
    /// screen saying a turn is on, never Rex's word for its pane nor a
    /// question at its start (the ShiftUp lead's item 46, 2026-11-27:
    /// an agy started with a prompt was told done 1.3 s later, at its
    /// own empty prompt, having been "at work" while it loaded).
    #[test]
    fn a_turn_is_on_by_the_agents_own_screen() {
        use crate::agents::state::classify_screen_rich;
        // Loading, as Rex has it: its pane is at work (the program runs), its screen not yet its own.
        let loading = "$ agy --mode auto 'read this'\n\n  Antigravity CLI\n  loading…\n";
        let cls = classify_screen_rich(AgentCli::Agy, Activity::Working, loading, true);
        assert_eq!((cls.state, cls.in_turn), (SubagentLiveState::Working, false), "{:?}", cls.summary);
        // At its own empty prompt, the pane still "at work": at rest, and no turn.
        let prompt = "────────────────────\n>\n────────────────────\n? for shortcuts\n";
        let cls = classify_screen_rich(AgentCli::Agy, Activity::Working, prompt, true);
        assert_eq!((cls.state, cls.in_turn), (SubagentLiveState::Idle, false), "{:?}", cls.summary);
        // The folder's trust, asked at a start: a question, and no turn.
        let trust = "────────────────────\n Accessing workspace:\n /var/tmp/wt\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n";
        let cls = classify_screen_rich(AgentCli::Claude, Activity::Blocked, trust, true);
        assert_eq!((cls.state, cls.in_turn), (SubagentLiveState::WaitingQuestion, false));
        // The API failing under a turn: a turn, cut short.
        let failed = "● Working\n  ⎿  API Error: Connection lost mid-response\n────────────────────\n❯ \n";
        let cls = classify_screen_rich(AgentCli::Claude, Activity::Idle, failed, true);
        assert_eq!((cls.state, cls.in_turn), (SubagentLiveState::Error, true));
    }

    /// A child's conversation is kept only by what is sure. What was
    /// guessed for it is forgotten when a registry is read, what its own
    /// process says now replaces what it said before, and a weaker word
    /// never replaces a surer one.
    #[test]
    fn a_childs_conversation_is_kept_only_by_what_is_sure() {
        let entry = |id: &str, cli: AgentCli, sid: &str, how: &str| NativeSubagent {
            id: id.into(),
            name: Some(id.into()),
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: format!("w-{id}"),
            block_id: format!("b-{id}"),
            parent_window_id: "w1".into(),
            parent_block_id: None,
            cli,
            model: "m".into(),
            effort: "e".into(),
            title: id.into(),
            role: None,
            cwd: None,
            resume_id: matches!(id, "resumed" | "given-at-start").then(|| "given-at-start".to_owned()),
            cli_session_id: Some(sid.into()),
            transcript_path: Some(format!("/t/{sid}")),
            desktop_policy: DesktopPolicy::None,
            stream_mode: false,
            started_at: 1,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 1,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: Some(how.into()),
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        };
        let mut kept = SubagentRegistry::default();
        for (id, cli, sid, how) in [
            ("agy-open", AgentCli::Agy, "anothers-1", "open"),
            ("agy-guess", AgentCli::Agy, "anothers-2", "guess"),
            ("claude-written", AgentCli::Claude, "anothers-3", "written"),
            ("resumed", AgentCli::Agy, "anothers-4", "guess"),
            ("kept-as-known", AgentCli::Agy, "anothers-5", "known"),
            ("given-at-start", AgentCli::Claude, "given-at-start", "known"),
            ("claude-record", AgentCli::Claude, "own-1", "record"),
            ("codex-open", AgentCli::Codex, "own-2", "open"),
            ("agy-argv", AgentCli::Agy, "own-3", "argv"),
            ("agy-lock", AgentCli::Agy, "own-4", "lock"),
        ] {
            kept.entries.push(entry(id, cli, sid, how));
        }
        let dir = std::env::temp_dir().join(format!("olo-registry-guesses-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("registry.json");
        kept.save_to(&file);
        let mut reg = SubagentRegistry::read_from(&file).unwrap_or_else(|e| panic!("{e}"));
        let of = |reg: &SubagentRegistry, id: &str| reg.entries.iter().find(|e| e.id == id).map(|e| (e.cli_session_id.clone(), e.session_confidence.clone(), e.transcript_path.is_some())).unwrap();
        for forgotten in ["agy-open", "agy-guess", "claude-written", "resumed", "kept-as-known"] {
            assert_eq!(of(&reg, forgotten), (None, None, false), "{forgotten}");
        }
        // (What it was started with is its own, guess or no guess.)
        assert_eq!(reg.entries.iter().find(|e| e.id == "resumed").unwrap().effective_session_id(), Some("given-at-start"));
        for (sure, sid, how) in [("claude-record", "own-1", "record"), ("codex-open", "own-2", "open"), ("agy-argv", "own-3", "argv"), ("agy-lock", "own-4", "lock")] {
            assert_eq!(of(&reg, sure), (Some(sid.to_owned()), Some(how.to_owned()), true), "{sure}");
        }
        // ("Known" is true of what a child was started with, and kept; of anything else it is a guess told back.)
        assert_eq!(of(&reg, "given-at-start"), (Some("given-at-start".to_owned()), Some("known".to_owned()), true));
        assert_eq!(SubagentRegistry::load_from(&file).entries.iter().filter(|e| e.cli_session_id.is_some()).count(), 5, "the other reading forgets the same");
        let _ = std::fs::remove_dir_all(&dir);

        // Read again as it runs: the lock it holds now is its conversation (a new one begun), and its command line's word doesn't take that back.
        assert!(reg.update_discovered("agy-lock", Some("own-5"), Some("lock"), None, Some(7)));
        assert_eq!(of(&reg, "agy-lock").0.as_deref(), Some("own-5"));
        assert!(!reg.update_discovered("agy-lock", Some("own-3"), Some("argv"), None, Some(7)));
        assert_eq!(of(&reg, "agy-lock"), (Some("own-5".to_owned()), Some("lock".to_owned()), true));
        // One with none yet takes the first sure word; a child read with nothing found keeps what it had.
        assert!(reg.update_discovered("agy-open", Some("own-6"), Some("lock"), Some("/t/own-6"), Some(8)));
        assert!(!reg.update_discovered("agy-open", None, None, None, Some(8)));
        assert_eq!(of(&reg, "agy-open"), (Some("own-6".to_owned()), Some("lock".to_owned()), true));
    }

    #[test]
    fn registry_preserves_exited_children_and_resolves_by_name_and_session_id() {
        let mut reg = SubagentRegistry::default();
        let mk = |id: &str, name: &str, wid: &str, pwid: &str, sid: &str| NativeSubagent {
            id: id.into(),
            name: Some(name.into()),
            host_key: "local".into(),
            session_id: "s1".into(),
            window_id: wid.into(),
            block_id: format!("b-{wid}"),
            parent_window_id: pwid.into(),
            parent_block_id: None,
            cli: AgentCli::Claude,
            model: "claude-opus-5-5".into(),
            effort: "max".into(),
            title: name.into(),
            role: None,
            cwd: None,
            resume_id: None,
            cli_session_id: Some(sid.into()),
            transcript_path: None,
            desktop_policy: DesktopPolicy::None,
            stream_mode: false,
            started_at: now_unix_ms() as i64 - 10_000,
            exited_at: None,
            exit_code: None,
            last_pid: Some(999),
            created_rev: 10,
            seen_live: true,
            exited_by_sync: false,
            session_confidence: Some("record".into()),
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        };
        reg.register(mk("sa-1", "builder-a", "w2", "w1", "11111111-2222-3333-4444-555555555555"));
        let live: HashSet<String> = ["w1", "w2"].into_iter().map(str::to_owned).collect();
        assert!(reg.is_live_name_taken("local", "s1", "w1", "builder-a", &live));
        assert_eq!(reg.resolve_target("local", Some("s1"), "builder-a").unwrap().id, "sa-1");
        assert_eq!(reg.resolve_target("local", Some("s1"), "11111111-2222-3333-4444-555555555555").unwrap().id, "sa-1");

        // C3: child `w2` CANNOT resolve itself!
        assert!(reg.resolve_target_scoped("local", Some("s1"), Some("w2"), true, "builder-a").is_err());
        assert!(reg.resolve_target_scoped("local", Some("s1"), Some("w2"), true, "sa-1").is_err());

        // H1: older view revision (9 < created_rev 10) MUST NOT mark `w2` exited!
        let empty: HashSet<String> = ["w1"].into_iter().map(str::to_owned).collect();
        let (changed_old, exited_old) = reg.sync_live_windows("local", "s1", &empty, 9);
        assert!(!changed_old && exited_old.is_empty());

        // Newer view revision (11 >= 10) marks it exited with session ID intact; reappearing heals it!
        let (changed, exited) = reg.sync_live_windows("local", "s1", &empty, 11);
        assert!(changed);
        assert_eq!(exited.len(), 1);
        assert!(!reg.is_live_name_taken("local", "s1", "w1", "builder-a", &empty));
        let found = reg.resolve_target("local", Some("s1"), "builder-a").unwrap();
        assert!(found.exited_at.is_some());
        assert_eq!(found.cli_session_id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
        // Healing when window reappears in newer view (`H1`):
        let (healed, _) = reg.sync_live_windows("local", "s1", &live, 12);
        assert!(healed);
        assert!(reg.by_window("local", "s1", "w2").is_some());
    }

    #[test]
    fn event_log_cursors_and_watch_multi_parse() {
        let mut log = SubagentEventLog::default();
        let mk_ev = |id: &str, state: SubagentLiveState, dk: &str| SubagentEvent {
            seq: 0,
            timestamp_ms: 1000,
            host_key: "local".into(),
            session_id: "s1".into(),
            parent_window_id: "w1".into(),
            ancestor_windows: vec!["w1".into()],
            id: id.into(),
            name: Some(format!("n-{id}")),
            window_id: format!("win-{id}"),
            block_id: format!("blk-{id}"),
            resume_id: None,
            cli_session_id: None,
            event: state.as_str().into(),
            state,
            summary: None,
            last_message: Some("DONE".into()),
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key: dk.into(),
            answers: None,
            minding: None,
        };
        let c1 = log.push(mk_ev("sa-1", SubagentLiveState::Done, "msg:1"));
        // Duplicate push for same (id, state, key) is dropped (`H7`), while new key advances (`H5`)!
        assert_eq!(log.push(mk_ev("sa-1", SubagentLiveState::Done, "msg:1")), c1);
        let c2 = log.push(mk_ev("sa-2", SubagentLiveState::WaitingApproval, "ap:100"));
        let c3 = log.push(mk_ev("sa-2", SubagentLiveState::WaitingApproval, "ap:200"));
        assert_eq!((c1, c2, c3), (1, 2, 3));
        assert_eq!(log.find_since(0, |_| true).unwrap().id, "sa-1");
        assert_eq!(log.find_since(c1, |_| true).unwrap().id, "sa-2");
        assert_eq!(log.find_since(c2, |_| true).unwrap().seq, 3);
        assert!(log.find_since(c3, |_| true).is_none());

        let ws = WatchSpec::parse(Some(&json!({"args": ["--all", "--since", "42", "--on", "done,approval", "--json"]}))).unwrap();
        assert!(ws.all);
        assert_eq!(ws.since, Some(42));
        assert!(ws.matches_state(SubagentLiveState::Done));
        assert!(ws.matches_state(SubagentLiveState::WaitingApproval));
        assert!(!ws.matches_state(SubagentLiveState::Working));
        // H10: `--on idle` matches BOTH Idle and Done!
        let ws_idle = WatchSpec::parse(Some(&json!({"args": ["--all", "--on", "idle"]}))).unwrap();
        assert!(ws_idle.matches_state(SubagentLiveState::Idle));
        assert!(ws_idle.matches_state(SubagentLiveState::Done));
        // L6 / M24: unknown `--on` and non-numeric JSON `since`/`timeout` rejected!
        assert!(WatchSpec::parse(Some(&json!({"args": ["--all", "--on", "typo"]}))).is_err());
        assert!(WatchSpec::parse(Some(&json!({"all": true, "since": "42"}))).is_err());
        assert!(WatchSpec::parse(Some(&json!({"all": true, "timeout": "60"}))).is_err());

        let ap = ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--json"]}))).unwrap();
        assert_eq!((ap.target.as_str(), ap.answer, ap.json), ("builder-a", spec::Answer::Once, true));
        assert_eq!(ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--session"]}))).unwrap().answer, spec::Answer::Session);
        assert_eq!(ApproveSpec::parse(Some(&json!({"target": "builder-a", "session": true}))).unwrap().answer, spec::Answer::Session);
        assert_eq!(ApproveSpec::parse(Some(&json!({"target": "builder-a", "no": true, "reason": "not that"}))).unwrap().answer, spec::Answer::No);
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--session"]}))).is_err());
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a"]}))).is_err());
        assert!(ApproveSpec::parse(Some(&json!({"args": ["builder-a", "--once", "--no"]}))).is_err());
    }

    /// Registry compatibility across the shared-minder upgrade:
    /// - Old records (without `minded_by`) read cleanly and `minder_of` finds old 1:1 minders (`minds: Some(id)`).
    /// - New records (with `minded_by`) find shared minders (`role: "minder"`, `minds: None`), reparent and clear `minded_by` when the minder exits, and ignore unknown fields so an older service does not choke on new fields.
    /// - `last_state_of` skips both `escalation` and `minding` events.
    #[test]
    fn shared_and_legacy_minders_coexist_and_serialize_compatibly() {
        // Record written by an older service (no `minded_by` field at all).
        let old_json = json!({
            "entries": [
                {
                    "id": "sa-a1", "name": "worker", "host_key": "local", "session_id": "s1",
                    "window_id": "w-a1", "block_id": "b-a1", "parent_window_id": "w-lead",
                    "cli": "claude", "model": "m", "effort": "e", "title": "worker",
                    "started_at": 1000, "seen_live": true
                },
                {
                    "id": "sa-m1", "name": "worker-minder", "host_key": "local", "session_id": "s1",
                    "window_id": "w-m1", "block_id": "b-m1", "parent_window_id": "w-a1",
                    "cli": "claude", "model": "m", "effort": "max", "title": "Minder of worker",
                    "role": "minder", "started_at": 1001, "seen_live": true, "minds": "sa-a1"
                }
            ],
            "event_seq": 5
        });
        let mut reg: SubagentRegistry = serde_json::from_value(old_json).unwrap();
        assert_eq!(reg.minder_of("sa-a1").map(|m| m.id.as_str()), Some("sa-m1"));
        assert!(reg.entries[1].is_minder() && !reg.entries[1].is_shared_minder());

        // Add a new-kind shared minder (`role: "minder"`, `minds: None`) and two agents nested under it.
        let mut shared = reg.entries[0].clone();
        shared.id = "sa-sm".into();
        shared.name = Some("minder".into());
        shared.window_id = "w-sm".into();
        shared.block_id = "b-sm".into();
        shared.parent_window_id = "w-lead".into();
        shared.role = Some("minder".into());
        shared.minds = None;
        shared.minded_by = None;
        shared.shared_minder = true;
        let mut child1 = reg.entries[0].clone();
        child1.id = "sa-c1".into();
        child1.name = Some("alpha".into());
        child1.window_id = "w-c1".into();
        child1.block_id = "b-c1".into();
        child1.parent_window_id = "w-sm".into();
        child1.minded_by = Some("sa-sm".into());
        let mut child2 = child1.clone();
        child2.id = "sa-c2".into();
        child2.name = Some("beta".into());
        child2.window_id = "w-c2".into();
        child2.block_id = "b-c2".into();
        reg.register(shared);
        reg.register(child1);
        reg.register(child2);

        assert!(reg.entries.iter().find(|e| e.id == "sa-sm").unwrap().is_shared_minder());
        assert_eq!(reg.minder_of("sa-c1").map(|m| m.id.as_str()), Some("sa-sm"));
        assert_eq!(reg.minder_of("sa-c2").map(|m| m.id.as_str()), Some("sa-sm"));
        assert_eq!(reg.minded_by("sa-sm").len(), 2);

        // Unknown future fields don't break deserialization (matching how an older service reads new records).
        let mut val = serde_json::to_value(&reg).unwrap();
        val["entries"][0]["some_future_field"] = json!("ignored");
        let parsed: SubagentRegistry = serde_json::from_value(val).unwrap();
        assert_eq!(parsed.entries.len(), 5);

        // When the shared minder exits, its agents move back under `w-lead` and `minded_by` is cleared.
        assert!(reg.mark_exited_with_code("local", "s1", "w-sm", Some(1)));
        assert!(reg.minder_of("sa-c1").is_none());
        assert!(reg.minder_of("sa-c2").is_none());
        for id in ["sa-c1", "sa-c2"] {
            let e = reg.entries.iter().find(|e| e.id == id).unwrap();
            assert_eq!((e.parent_window_id.as_str(), e.minded_by.as_deref()), ("w-lead", None));
        }

        // `last_state_of` skips both `escalation` and `minding`.
        let mut log = SubagentEventLog::default();
        let base = |event: &str, state: SubagentLiveState| SubagentEvent {
            seq: 0,
            timestamp_ms: 1,
            host_key: "local".into(),
            session_id: "s1".into(),
            parent_window_id: "w-sm".into(),
            ancestor_windows: vec!["w-sm".into(), "w-lead".into()],
            id: "sa-c1".into(),
            name: Some("alpha".into()),
            window_id: "w-c1".into(),
            block_id: "b-c1".into(),
            resume_id: None,
            cli_session_id: None,
            event: event.into(),
            state,
            summary: None,
            last_message: None,
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key: "ap:1".into(),
            answers: None,
            minding: None,
        };
        let s1 = log.push(base("waiting_approval", SubagentLiveState::WaitingApproval));
        log.push_beside(base(minder::MINDING, SubagentLiveState::Working));
        log.push_beside(base(minder::ESCALATION, SubagentLiveState::WaitingApproval));
        assert_eq!(log.last_state_of("sa-c1").map(|e| e.seq), Some(s1));
    }

    /// Round 2 (#1): `--role minder` (CLI or structured JSON) is refused
    /// from any caller, and a record that merely has `role: "minder"`
    /// without the service's own marker is no minder.
    #[test]
    fn round2_1_role_minder_is_refused_and_does_not_grant_authority() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        for role in ["minder", "Minder", " MINDER "] {
            let err = StartSpec::parse_cli_args(&args(&["claude", "--model", "m", "--effort", "e", "--role", role])).expect_err(&format!("CLI --role {role} must be refused"));
            assert!(err.to_ascii_lowercase().contains("minder"), "{err}");
            let jerr = StartSpec::parse_json_object(&json!({ "cli": "claude", "model": "m", "effort": "e", "role": role })).expect_err(&format!("JSON role={role} must be refused"));
            assert!(jerr.to_ascii_lowercase().contains("minder"), "{jerr}");
        }
        // A legacy or hand-edited record with only `role: "minder"` and no `minds` or service marker is NOT a minder.
        let spoofed_json = json!({
            "id": "sa-spoof", "name": "fake-minder", "host_key": "local", "session_id": "s1",
            "window_id": "w-sp", "block_id": "b-sp", "parent_window_id": "w-lead",
            "cli": "claude", "model": "m", "effort": "max", "title": "Fake",
            "role": "minder", "started_at": 1000, "seen_live": true
        });
        let spoofed: NativeSubagent = serde_json::from_value(spoofed_json).unwrap();
        assert!(!spoofed.is_minder() && !spoofed.is_shared_minder(), "bare role=minder must confer no minder authority");
        // Legacy 1:1 minder (`minds: Some(...)`) remains a minder.
        let legacy_json = json!({
            "id": "sa-leg", "name": "old-m", "host_key": "local", "session_id": "s1",
            "window_id": "w-leg", "block_id": "b-leg", "parent_window_id": "w-w1",
            "cli": "claude", "model": "m", "effort": "max", "title": "Old",
            "role": "minder", "minds": "sa-w1", "started_at": 1000, "seen_live": true
        });
        let legacy: NativeSubagent = serde_json::from_value(legacy_json).unwrap();
        assert!(legacy.is_minder() && !legacy.is_shared_minder());
    }

    /// Round 2 (#33): restoring the event log rebuilds `last_by_id` from
    /// state records only — `minding` and `escalation` (`push_beside`)
    /// records stay in the ring without turning a standing state into a
    /// fresh transition after restart.
    #[test]
    fn round2_33_restored_log_ignores_beside_events_for_dedupe() {
        let mk = |seq: u64, event: &str, state: SubagentLiveState, dedupe: &str| SubagentEvent {
            seq,
            timestamp_ms: seq,
            host_key: String::new(),
            session_id: "s1".into(),
            parent_window_id: "w-m".into(),
            ancestor_windows: vec!["w-m".into(), "w-lead".into()],
            id: "sa-a1".into(),
            name: Some("alpha".into()),
            window_id: "w-a1".into(),
            block_id: "b-a1".into(),
            resume_id: None,
            cli_session_id: None,
            event: event.into(),
            state,
            summary: None,
            last_message: None,
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key: dedupe.into(),
            answers: None,
            minding: None,
        };
        let mut restored = SubagentEventLog::restored(
            vec![
                mk(1, "waiting_approval", SubagentLiveState::WaitingApproval, "ap:42"),
                mk(2, minder::MINDING, SubagentLiveState::WaitingApproval, ""),
                mk(3, minder::ESCALATION, SubagentLiveState::WaitingApproval, ""),
            ],
            3,
        );
        // Pushing the exact same standing approval after restore must deduplicate back to seq 1!
        let pushed = restored.push(mk(0, "waiting_approval", SubagentLiveState::WaitingApproval, "ap:42"));
        assert_eq!(pushed, 1, "restored beside events must not clobber last_by_id dedupe state");
        assert_eq!(restored.latest_seq(), 3);
    }
}
