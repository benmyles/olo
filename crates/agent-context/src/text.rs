//! The standing-instructions text Olo places in each coding agent harness on a
//! Rex host, and the versioned markers that frame it.

/// Format version written into [`BEGIN_MARKER`]. Bump when the block format or
/// marker contract changes; wording updates alone are already detected because
/// status compares the rendered block bytes.
pub const VERSION: u32 = 1;

/// Opening marker for Olo's managed block.
pub const BEGIN_MARKER: &str = "<!-- olo:rex-host:begin v1 -->";

/// Closing marker for Olo's managed block.
pub const END_MARKER: &str = "<!-- olo:rex-host:end -->";

/// Frontmatter required by Antigravity (`agy` 1.5.18) so a named rule file in
/// `~/.gemini/config/rules/` is loaded unconditionally into every session.
pub const AGY_FRONTMATTER: &str = "---\ntrigger: always_on\n---\n\n";

/// Maximum size guardrails documented in the crate contract (every conversation
/// reads this section at startup).
pub const MAX_LINES: usize = 60;
pub const MAX_BYTES: usize = 3000;

/// Standing instructions for an agent starting on a Rex host, worded once for
/// every harness. Always LF-terminated with a single trailing newline.
/// (Reworded by the lead after the builder's draft: plainer, shorter, and
/// with no status number for a refusal: the code gives 1 where the skill
/// says 69, which is the code's to settle first.)
///
/// Every claim here traces directly to `apps/rex/src/bridge_skill.md`,
/// `apps/rex/src/bridge.rs`, `apps/rex/src/bridge_olo.sh`,
/// `apps/rex/src/bridge_olo.lua`, `apps/rex/src/hub.rs`,
/// `.claude/skills/olo-dev-server/SKILL.md`, and the `rex` skill.
pub const SECTION: &str = "\
# Where you are running: a Rex host (Olo)

This machine is a Rex host: a Linux computer its person reaches from their Olo desktop through Rex, a terminal app whose sessions, tabs and panes live here. If `REX_BLOCK` is set you are in one of those panes. If it isn't, you are in a plain shell, and what below asks the desktop for something won't reach it.

There is no screen, browser or keyring of your own here. One command, `olo`, asks the person's desktop instead (it is on the PATH; else `~/.claude/skills/olo-rex-bridge/olo`):

- A link, or a page of a server you run here, in the person's browser: `olo open URL` (`olo open http://localhost:3000` works: the port is carried over).
- A graphical app, or a browser you drive yourself: `olo screen` (Agent Screens: a screen of your own, on the person's desktop or on this host; start, run, look, tree, click, type).
- A secret from the person's Olo Passwords, once they approve it: `olo op read op://VAULT/ITEM/FIELD`.
- Files between here and the desktop, a notification, the clipboard: `olo send`, `olo get`, `olo notify`, `olo clipboard`.
- Subagents: `olo agent` (start, run, watch, send, approve, stop). Each is a tab under yours that the person sees and can answer, and it needs no desktop. The person prefers this here to a background shell, tmux or any other way of running one.

Each works only for what the person has allowed this host, and only while their desktop is connected (`olo agent`, and screens on this host, excepted). A command that isn't allowed fails and says why: tell the person, and don't look for a way round it.

Before you use one of these, read its skill instead of guessing at its options: `olo-rex-bridge` (every `olo` command; its file is `~/.claude/skills/olo-rex-bridge/SKILL.md`) and `rex` (the `rex` command: sessions, tabs, panes, running commands, reading terminals; `rex skill show` prints it).
";

/// Render the managed block (begin marker, section, end marker) with the given
/// line ending (`"\n"` or `"\r\n"`), without a trailing newline after
/// [`END_MARKER`].
pub fn render_block(eol: &str) -> String {
    let mut out = String::with_capacity(BEGIN_MARKER.len() + SECTION.len() + END_MARKER.len() + 64);
    out.push_str(BEGIN_MARKER);
    out.push_str(eol);
    let body = SECTION.strip_suffix('\n').unwrap_or(SECTION);
    for line in body.split('\n') {
        out.push_str(line);
        out.push_str(eol);
    }
    out.push_str(END_MARKER);
    out
}

/// Render the full contents of Antigravity's Olo-owned rule file
/// (`~/.gemini/config/rules/olo-rex.md`), with `trigger: always_on` frontmatter
/// and the versioned managed block.
pub fn render_rule_file() -> String {
    let mut out = String::with_capacity(AGY_FRONTMATTER.len() + SECTION.len() + 96);
    out.push_str(AGY_FRONTMATTER);
    out.push_str(&render_block("\n"));
    out.push('\n');
    out
}
