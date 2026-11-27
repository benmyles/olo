//! Coding agents in a terminal (Claude Code, Codex, Antigravity), read as
//! a person reads them: which one a program is, what it is doing from the
//! bottom of its screen, whether its own prompt is showing, and whether
//! the question it asks may be answered Yes for the person. Text in, an
//! answer out: nothing here reads a process, a terminal or a clock.
//! Terminal reads its tabs' screens with it, and Rex its panes', on this
//! computer or a far host, so the two apps' Keep It Going and Auto
//! Approve see an agent the same way (Ben, 2026-11-24: "did we already
//! add the support for auto approve + keep it going for all the coding
//! harnesses in our olo rex?").

use std::time::Duration;

/// How long a question shows before Auto Approve answers it (Ben,
/// 2026-11-09: "have it auto approve / select yes after 30s").
pub const AUTO_APPROVE_AFTER: Duration = Duration::from_secs(30);
/// How long Keep It Going waits with nothing happening (Ben: "change it
/// from 60s to 120s without activity").
pub const KEEP_GOING_QUIET: Duration = Duration::from_secs(120);
/// At most once in this long ("limit it to sending up to one "please
/// continue" per hour").
pub const KEEP_GOING_EVERY: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    /// Google's Antigravity CLI (`agy`). (Ben, 2026-11-23: "make sure
    /// that keep it going and auto approve and any and all other agent
    /// features for both terminal and olo rex work with antigravity tui
    /// (agy)". Its screens here are of agy 1.5.15.)
    Antigravity,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Antigravity => "Antigravity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Busy with its turn.
    Working,
    /// Stopped on a question: a permission to grant, a choice to make.
    Blocked,
    /// Its turn is over: waiting for the next message.
    Idle,
}

impl State {
    pub fn describe(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "needs your answer",
            State::Idle => "waiting for you",
        }
    }
}

pub fn identify(comm: &str, exe: &str, args: &[String]) -> Option<Agent> {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let names: Vec<String> = [comm.to_owned(), base(exe)].into_iter().chain(args.iter().take(2).map(|a| base(a))).collect();
    if exe.contains("/claude/versions/") || names.iter().any(|n| n == "claude") || args.iter().any(|a| a.contains("@anthropic-ai/claude-code")) {
        return Some(Agent::Claude);
    }
    if names.iter().any(|n| n == "codex") || args.iter().any(|a| a.contains("@openai/codex")) {
        return Some(Agent::Codex);
    }
    // (One program, `agy`, wherever its installer put it.)
    if names.iter().any(|n| n == "agy" || n == "antigravity") {
        return Some(Agent::Antigravity);
    }
    None
}

/// Whether a program is an agent's server for all its sessions: Codex's
/// app server, one daemon that runs every Codex session's tools and hooks.
pub fn shared_server(comm: &str, exe: &str, args: &[String]) -> bool {
    identify(comm, exe, args) == Some(Agent::Codex) && args.iter().skip(1).take(3).any(|a| a == "app-server")
}

/// Codex's status line, by its own shape: a bullet or a spinner, words,
/// and its clock with its hint in brackets, "• Working (1m 43s • esc to
/// interrupt)" (and whatever it writes after them). Not any line that
/// has the hint's words: an answer that speaks of the hint ("• The hint
/// reads (esc to interrupt)") kept a finished Codex "working" for ever,
/// and so did those words typed into its field (a code review,
/// 2026-11-27).
fn codex_status(line: &str) -> bool {
    let l = line.trim();
    let mut chars = l.chars();
    // (A bullet or a spinner. Not the marks Codex sets before what a
    // command printed or what waits in its queue: those lines are quoted.)
    // (Nor Codex's own mark, which begins its field and every message
    // of the person's shown above it: "› Explain Working (4s • esc to
    // interrupt)" typed at a finished Codex kept it "at work". A code
    // review, 2026-11-27.)
    let led = chars.next().is_some_and(|c| !c.is_alphanumeric() && !c.is_ascii_punctuation() && !matches!(c, '└' | '↳' | '│' | '›')) && chars.next() == Some(' ');
    let Some(at) = l.find(" • esc to interrupt)").filter(|_| led) else { return false };
    let Some(open) = l[..at].rfind('(') else { return false };
    // (Its clock: "4s", "1m 43s", "2h 03m 10s".)
    let clock: Vec<&str> = l[open + 1..at].split(' ').collect();
    let part = |p: &str, unit: char| p.strip_suffix(unit).is_some_and(|n| !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit()));
    match clock.as_slice() {
        [s] => part(s, 's'),
        [m, s] => part(m, 'm') && part(s, 's'),
        [h, m, s] => part(h, 'h') && part(m, 'm') && part(s, 's'),
        _ => false,
    }
}

/// Codex's own word that it is at work, wherever its field has pushed
/// it: its status line stands above its field, and the field grows with
/// what is typed or queued in it. Looked for in the screen's last six
/// lines alone, a working Codex with a long message waiting in its field
/// read as at rest, at its prompt (the ShiftUp lead's items 50 and 51,
/// 2026-11-27; what settles those is its conversation's file,
/// `olo_rex_link::agents::state::turn_on`: this is what its screen can
/// say). The field is the last line that begins with Codex's mark at the
/// screen's edge (what is typed under it is set in from the edge; a
/// message that begins "2. Verify" is still the field: only a list of
/// choices with the mark on one of them is not), and the status line is
/// looked for among the sixteen lines above it, no further up than the
/// line Codex ends a turn with ("Worked for 16s • 1:04 PM", as 0.172
/// draws it) or a rule drawn across the screen: what stands above
/// either is of a turn before.
fn codex_at_work(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let numbered = |s: &str| s.trim_start().split_once('.').is_some_and(|(n, rest)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && rest.starts_with(' '));
    let Some(field) = lines.iter().rposition(|l| l.starts_with('›')) else { return false };
    // (A chooser: the mark on one numbered choice, another numbered one
    // under or over it, and Codex's own words under a list of choices,
    // "Press enter to confirm or esc to cancel". Without those words a
    // message in its field that begins "2. Verify" with "3. Repeat"
    // under it was a chooser.)
    let choice = numbered(lines[field].trim_start_matches('›'));
    let beside = |i: usize| lines.get(i).is_some_and(|l| !l.starts_with('›') && numbered(l));
    let foot = lines[field + 1..].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_ascii_lowercase()).unwrap_or_default();
    let asks = foot.contains("enter to confirm") || foot.contains("esc to cancel") || foot.contains("enter to select");
    if choice && asks && (beside(field + 1) || field.checked_sub(1).is_some_and(beside)) {
        return false;
    }
    // What Codex draws only while a turn is on: under its field, "tab
    // to queue message" (its footer while something is typed there);
    // over it, "shift+← edit last queued message" under what was queued.
    // (Its footer: the screen's last line. Those words on a line of a
    // message in the field are the person's.)
    if foot == "tab to queue message" {
        return true;
    }
    // (Sixteen lines: what is queued stands between the status line and
    // the field, "• Queued follow-up inputs" and a line for each.)
    lines[..field]
        .iter()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(16)
        .take_while(|l| !l.trim_start().starts_with('─') && !l.trim_start().starts_with("Worked for "))
        .any(|l| codex_status(l) || l.trim().eq_ignore_ascii_case("shift+← edit last queued message"))
}

/// What the agent is doing, from the bottom of its screen (where its
/// prompt, dialogs and status line are). Antigravity's words are its
/// own: "esc to cancel" is under its field while it works (Claude
/// Code's is under a question), and a question of its has "↑/↓
/// Navigate" under its answers; working with a message queued, its last
/// line is "Press up to edit queued messages" and only its spinner (a
/// braille figure at a line's start, above its field) says it works.
///
/// Scoped to each CLI's bottom chrome / dialog region so ordinary output
/// prose (e.g. `"Do you want to keep the old API?"` or `"esc to cancel"`
/// in docs or test logs) is never misclassified as `Blocked` or `Working`.
pub fn classify(agent: Agent, screen: &str) -> State {
    let non_empty: Vec<&str> = screen.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail_joined = |n: usize| -> String {
        let start = non_empty.len().saturating_sub(n);
        non_empty[start..].join("\n").to_ascii_lowercase()
    };
    if agent == Agent::Antigravity {
        // Check only the bottom 8 non-empty chrome lines (banner art / scrollback prose ignored).
        let bottom8 = tail_joined(8);
        let spinning = || non_empty.iter().rev().take(8).any(|l| l.chars().next().is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c)));
        return if bottom8.contains("↑/↓ navigate") || parse_approval(agent, screen).is_some() {
            State::Blocked
        } else if bottom8.contains("esc to cancel") || bottom8.contains("press up to edit queued messages") || spinning() {
            State::Working
        } else {
            State::Idle
        };
    }
    if parse_folder_trust(screen).is_some() || parse_approval(agent, screen).is_some() {
        return State::Blocked;
    }
    let bottom6 = tail_joined(6);
    let bottom10 = tail_joined(10);
    // Genuine working indicator in Claude Code / Codex status footer.
    // (Codex's by its status line's own shape, at the foot or over its
    // field: `codex_status`, `codex_at_work`.)
    let at_work = match agent {
        Agent::Codex => non_empty.iter().rev().take(6).any(|l| codex_status(l)) || codex_at_work(screen),
        _ => bottom6.contains("esc to interrupt"),
    };
    if at_work {
        return State::Working;
    }
    // Check chooser/dialog chrome strictly within the bottom region so prose never triggers Blocked.
    let has_dialog_mark = bottom10.contains("❯ 1. ") || bottom10.contains("› 1. ") || bottom6.contains("❯ 1.") || bottom6.contains("› 1.");
    let has_footer_chrome = bottom6.contains("enter to select") || bottom6.contains("enter to confirm") || bottom6.contains("press enter to continue") || bottom6.contains("esc to cancel");
    let has_non_prompt_chooser_mark = {
        let lines: Vec<&str> = screen.lines().collect();
        let prompt_idxs = prompt_line_indices(agent, &lines);
        let start = lines.len().saturating_sub(14);
        lines.iter().enumerate().skip(start).any(|(i, l)| {
            !prompt_idxs.contains(&i) && {
                let t = l.trim_start();
                t.starts_with('❯') || t.starts_with('›')
            }
        })
    };
    if has_dialog_mark
        || (has_footer_chrome && has_non_prompt_chooser_mark)
        || bottom6.contains("allow command?")
        || non_empty.last().is_some_and(|l| {
            let low = l.to_ascii_lowercase();
            low.ends_with("(y/n)") || low.ends_with("[y/n]")
        })
    {
        State::Blocked
    } else {
        State::Idle
    }
}

/// Returns line indices in `lines` that correspond to the agent's own chat input prompt
/// (as opposed to chooser cursor lines like `❯ 1. Yes` or `❯ No, exit`).
fn prompt_line_indices(agent: Agent, lines: &[&str]) -> Vec<usize> {
    let rule = |l: &str| {
        let t = l.trim();
        t.contains("────") && (t.starts_with('─') || t.ends_with('─'))
    };
    let numbered = |s: &str| s.split_once('.').is_some_and(|(n, rest)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let is_prompt = match agent {
            Agent::Claude => (t.starts_with('❯') && lines[..i].iter().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l))) || t.starts_with("│ >") || t.starts_with("│ ❯"),
            Agent::Codex => t.strip_prefix('›').is_some_and(|rest| !numbered(rest.trim_start())),
            Agent::Antigravity => {
                let ruled = |l: Option<&&str>| l.is_some_and(|l| rule(l));
                t.starts_with('>') && ruled(lines[..i].iter().rev().find(|l| !l.trim().is_empty())) && ruled(lines[i + 1..].iter().find(|l| !l.trim().is_empty()))
            }
        };
        if is_prompt {
            out.push(i);
        }
    }
    out
}

/// `agent`'s own prompt is on `screen`, waiting for a message: what Keep
/// It Going types into, and nothing else (a chooser takes its words as
/// keys and its Return as a choice). Claude Code's "❯" in the box under
/// its rule (or its older "│ >" box), Codex's "›" on a line that isn't an
/// answer ("› 1. Yes"). Not Claude Code's "Make auto mode your default
/// permission mode?", whose "❯" is on an answer with no number: Return
/// there would have said yes for the person. Antigravity's ">" between
/// its field's two rules, with nothing being asked (a question's chosen
/// answer begins with ">" too) and nothing under way.
pub fn at_prompt(agent: Agent, screen: &str) -> bool {
    // (Not while Claude Code's offer to send a drafted feedback report
    // stands over its field: what is typed there may answer it. Whoever
    // types by this word, Keep It Going, a refusal's reason, a notice
    // after a move, holds off until it is gone: [`feedback_draft`].)
    if feedback_draft(agent, screen).is_some() {
        return false;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let rule = |l: &str| {
        let t = l.trim();
        t.contains("────") && (t.starts_with('─') || t.ends_with('─'))
    };
    let numbered = |s: &str| s.split_once('.').is_some_and(|(n, rest)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
    lines.iter().enumerate().any(|(i, l)| {
        let t = l.trim_start();
        match agent {
            Agent::Claude => (t.starts_with('❯') && lines[..i].iter().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l))) || t.starts_with("│ >") || t.starts_with("│ ❯"),
            Agent::Codex => t.strip_prefix('›').is_some_and(|rest| !numbered(rest.trim_start())),
            Agent::Antigravity => {
                let ruled = |l: Option<&&str>| l.is_some_and(|l| rule(l));
                classify(agent, screen) == State::Idle
                    && t.starts_with('>')
                    && ruled(lines[..i].iter().rev().find(|l| !l.trim().is_empty()))
                    && ruled(lines[i + 1..].iter().find(|l| !l.trim().is_empty()))
            }
        }
    })
}

/// A question its agent asks that Auto Approve may answer Yes to: one
/// asking leave to go on ("Do you want to proceed?", "Do you want to make
/// this edit to …?", Codex's "Would you like to run the following
/// command?") with its first answer, "1. Yes", chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Which question it is: the dialog's text, its digits aside (Claude
    /// Code's "will automatically block this request in 1:50" counts down
    /// under it), so the same question isn't taken for a new one.
    pub key: u64,
    /// Its question ("Do you want to proceed?").
    pub question: String,
    /// How many answers below the mark its yes for this once stands: 0
    /// where Return says it, as in nearly every question. Claude Code's
    /// read outside the working directories has a kept yes first, under
    /// the mark, and "Yes, but ask again next time" fourth: Down three
    /// times, and Return only once the mark is seen to stand on it.
    pub down: usize,
}

/// The question on `screen` that Auto Approve may answer, if one is: the
/// last line asking "Do you want to …?" or "Would you like to …?", its
/// first answer after it "1. Yes" with the chooser (❯, ›) on it.
/// Antigravity's too: a question ("Run this command?") with its first
/// answer, "> 1. Yes, run command", right under it and "↑/↓ Navigate"
/// under its answers. (Return there is yes for this once: never its
/// "always allow", which are answers 2 and 3.)
/// Not a plan waiting to be approved (Claude Code's "No, keep planning"):
/// that answer is the person's.
pub fn approvable(screen: &str) -> Option<Ask> {
    let lines: Vec<&str> = screen.lines().collect();
    // (Antigravity's chooser is a plain ">", which is also where a
    // message is typed: taken for a chooser only on a screen that has
    // its line of keys.)
    let navigated = screen.contains("↑/↓ Navigate");
    let marks: &[char] = if navigated { &['❯', '›', '>'] } else { &['❯', '›'] };
    let q = lines.iter().enumerate().rposition(|(i, l)| {
        let t = l.trim().to_lowercase();
        let first_under = || lines[i + 1..].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim().strip_prefix('>').is_some_and(|rest| rest.trim_start().starts_with("1. ")));
        // ("Allow this read outside the working directories?": Claude Code, 2.1.33x.)
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || t.starts_with("allow this ") || (navigated && first_under()))
    })?;
    let rest = &lines[q + 1..];
    // Its first answer (Codex shows the command between): "1. …", chosen.
    let at = rest.iter().position(|l| l.trim().trim_start_matches(marks).trim_start().starts_with("1. "))?;
    // (Its own: no other question between; but for the reason Codex gives
    // for a command, which it's told to put as a question, "Reason: Do you
    // want to …?": Auto Approve never answered Codex.)
    if rest[..at].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    if rest.iter().any(|l| l.to_lowercase().contains("keep planning")) {
        return None;
    }
    // Its answers, each with whether the mark stands on it: "N. …"
    // lines, a line under one that isn't numbered being the rest of it.
    let mut answers: Vec<(bool, String)> = Vec::new();
    for line in &rest[at..] {
        let t = line.trim();
        let (marked, unmarked) = match marks.iter().find_map(|mark| t.strip_prefix(*mark)) {
            Some(after) => (true, after.trim_start()),
            None => (false, t),
        };
        let numbered = unmarked.split_once(". ").or_else(|| unmarked.split_once(".\t")).filter(|(n, _)| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()));
        match (numbered, answers.last_mut()) {
            (Some((_, label)), _) => answers.push((marked, label.trim().to_owned())),
            // (The line of keys under them, or the end of them.)
            (None, _) if t.is_empty() || t.contains("Esc to cancel") || t.contains("↑/↓") || t.to_lowercase().starts_with("press enter") => break,
            (None, Some(last)) => {
                last.1.push(' ');
                last.1.push_str(t);
            }
            (None, None) => break,
        }
    }
    // The mark stands on one of them: where the agent put it (the
    // first), or on the yes for this once.
    let mut marked = answers.iter().enumerate().filter(|(_, (m, _))| *m).map(|(i, _)| i);
    let (Some(at_mark), None) = (marked.next(), marked.next()) else { return None };
    let once: Vec<usize> = answers.iter().enumerate().filter(|(_, (_, label))| classify_choice_kind(label) == ChoiceKind::Once).map(|(i, _)| i).collect();
    let down = match once[..] {
        // (As ever: the first answer is the yes, and the mark is on it.)
        _ if at_mark == 0 && once.first() == Some(&0) => 0,
        // One yes for this once, further down: the mark where the agent
        // put it (to be moved there), or on it already. Anywhere else,
        // someone moved it: theirs.
        [only] if at_mark == only => 0,
        [only] if at_mark == 0 => only,
        _ => return None,
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &lines[q.saturating_sub(12)..] {
        // Only its words, the digits of a clock counting down aside: not
        // the marks a program animates as it waits (Claude Code's "●"
        // before the tool call it asks about blinks, so the question
        // never read the same twice: Auto Approve answered only once it
        // stopped, half a minute on).
        l.chars().filter(|c| c.is_alphanumeric()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>().hash(&mut h);
    }
    Some(Ask { key: h.finish(), question: lines[q].trim().to_owned(), down })
}

/// The permission question an agent asks, whatever its shape, as Auto
/// Approve may answer it (Ben, 2026-11-26, of a read outside the working
/// directories that never counted down: "when it's on it should auto
/// approve everything after a 30s countdown"): any question
/// [`parse_approval`] reads (each agent's, its answers by what they
/// mean), answered with the yes that keeps nothing: the one for this
/// once, wherever it stands; where a question has none, the one for this
/// conversation alone. Never a yes that is kept in settings or switches
/// the agent's mode, and never a plan to choose a mode for (Ben, the same
/// day, of the two left unanswered: "i want folder trust auto approved.
/// plans should not be though").
///
/// A folder to trust is answered too, with its yes ([`trust_ask`]): the
/// agent can't begin without it, and whoever turned Auto Approve on for
/// a pane has said yes to what it does there.
///
/// The mark must stand where the agent put it (the first answer) or on
/// that yes already: anywhere else, someone moved it and it is theirs.
/// [`Ask::down`] says how far below the mark the yes is.
pub fn approvable_for(agent: Agent, screen: &str) -> Option<Ask> {
    if let Some(trust) = parse_folder_trust(screen) {
        return trust_ask(&trust);
    }
    let asked = parse_approval(agent, screen)?;
    let of_kind = |kind: ChoiceKind| -> Vec<usize> { asked.choices.iter().filter(|c| c.kind == kind).map(|c| c.index).collect() };
    let (once, session) = (of_kind(ChoiceKind::Once), of_kind(ChoiceKind::AllowSession));
    let yes = match (&once[..], &session[..]) {
        ([only], _) => *only,
        // (More than one reads as a plain yes: the first answer, where it is one, as ever.)
        ([first, ..], _) if *first == 0 => 0,
        ([], [only]) => *only,
        _ => return None,
    };
    let at_mark = asked.selected_index?;
    let down = match at_mark {
        m if m == yes => 0,
        0 => yes,
        _ => return None,
    };
    Some(Ask { key: asked.key, question: asked.question, down })
}

/// A folder an agent asks to trust before it begins, as Auto Approve
/// answers it: with its yes ("Yes, I trust this folder"; Codex's "Yes,
/// continue"; "Yes, add it" for a folder added), wherever it stands.
/// Claude Code puts the mark on "No, exit" above it, Antigravity on the
/// yes (both read from the programs, 2026-11-26). As for any question:
/// a mark must be seen, on the first answer or on the yes; anywhere
/// else it was moved, and is theirs.
fn trust_ask(trust: &FolderTrustPrompt) -> Option<Ask> {
    use std::hash::{Hash, Hasher};
    if !trust.marked {
        return None;
    }
    let down = match trust.selected_index {
        at if at == trust.trust_index => 0,
        0 => trust.trust_index,
        _ => return None,
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("trust", &trust.folder, trust.is_add_dir).hash(&mut h);
    Some(Ask { key: h.finish(), question: trust.question.clone(), down })
}

/// What the screen shows, for telling a change: its text with the
/// clocks that count by themselves taken out (Codex's "Pursuing goal
/// (17h 52m)", whose time goes on every minute whatever it's doing: Ben,
/// 2026-11-06).
pub fn fingerprint(screen: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for line in screen.lines() {
        match line.find("Pursuing goal (") {
            Some(at) => {
                let rest = &line[at + "Pursuing goal (".len()..];
                let after = rest.find(')').map_or("", |e| &rest[e + 1..]);
                (&line[..at], after).hash(&mut h);
            }
            None => line.hash(&mut h),
        }
    }
    h.finish()
}

/// A feedback survey an agent draws over its own last lines, which
/// takes a digit as its answer (Claude Code's "How is Claude doing this
/// session? … 1: Bad 2: Fine 3: Good 0: Dismiss"; Antigravity's "How's
/// the CLI experience so far? … [1] Good [2] Fine [3] Bad [0] Skip").
/// Never a question for whoever looks after the agent: it once hid a
/// permission prompt under it for 25 minutes, and a message beginning
/// with a digit would answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    pub question: String,
    /// The key that sets it aside without answering it.
    pub dismiss: &'static str,
}

/// The feedback survey showing on an agent's screen, if one is: its
/// question and its choices as the agent draws them, on two lines one
/// after the other at the screen's foot, each from its line's start. (A
/// message that quotes the survey in its prose isn't one.)
pub fn parse_survey(agent: Agent, screen: &str) -> Option<Survey> {
    let (asks, first, last) = match agent {
        Agent::Claude => ("How is Claude doing this session?", "1: Bad", "0: Dismiss"),
        Agent::Antigravity => ("How's the CLI experience so far?", "[1] Good", "[0] Skip"),
        Agent::Codex => return None,
    };
    let foot: Vec<&str> = screen.lines().rev().filter(|l| !l.trim().is_empty()).take(12).collect();
    foot.windows(2).find_map(|pair| {
        // (`pair`: a line and the one above it, the foot being read upwards.)
        let choices = pair[0].split_whitespace().collect::<Vec<_>>().join(" ");
        let question = pair[1].trim().trim_start_matches(['●', '•', '*']).trim();
        (choices.starts_with(first) && choices.ends_with(last) && question.starts_with(asks)).then(|| Survey { question: question.to_owned(), dismiss: "0" })
    })
}

/// Claude Code's offer to send a feedback report it has drafted: one
/// line of three keys, "1 to review · 2 to send · 0 to dismiss", the
/// last thing above its field. Not a survey and never set aside for
/// anyone: the ShiftUp lead's item 48 (2026-11-27), with Ben's rule
/// there: "no feedback report is ever sent, and nothing answers that box
/// by itself". While it shows, a message beginning "2 …" would send the
/// report, so nothing is typed into that screen and whoever looks after
/// the agent is told. Its line as drawn, if it shows: the three keys in
/// their order and nothing else on the line, at the screen's foot. (Read
/// from the words the lead quoted, not yet from a capture of the box:
/// where it is drawn otherwise it isn't seen, which is today's state.)
pub fn feedback_draft(agent: Agent, screen: &str) -> Option<String> {
    if agent != Agent::Claude {
        return None;
    }
    // (The last thing above its field: the line under it, blanks aside,
    // is the rule over the field, or the field itself. The same words
    // printed in what it wrote, a bullet's line or a line of a list
    // further up, are not the box: taken for it, every message to a
    // healthy agent was refused, and the `0` it was told to press was
    // typed into its field. A code review, 2026-11-27.)
    let lines: Vec<&str> = screen.lines().collect();
    let rule = |l: &str| l.trim().chars().filter(|c| *c == '─').count() >= 20;
    let field = |l: &str| {
        let t = l.trim_start();
        t.starts_with('❯') || t.starts_with("│ >") || t.starts_with("╭")
    };
    let from = lines.len().saturating_sub(40);
    (from..lines.len()).rev().find_map(|i| {
        let words = lines[i].split_whitespace().collect::<Vec<_>>().join(" ");
        let keys = words.trim_start_matches(['│', ' ']).trim_end_matches(['│', ' ']);
        let rest = keys.strip_prefix("1 to review")?.strip_suffix("0 to dismiss")?;
        // (Between them: the middle key and what divides the three, nothing more.)
        let middle = rest.trim_matches(|c: char| !c.is_alphanumeric());
        let over_the_field = lines[i + 1..].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| rule(l) || field(l));
        (middle == "2 to send" && over_the_field).then(|| keys.to_owned())
    })
}

/// Semantic kind of a choice in a tool approval dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChoiceKind {
    /// Approve this single invocation only ("1. Yes", "1. Yes, proceed", "1. Yes, run command").
    Once,
    /// Allow for the rest of this session/conversation/project without persisting globally.
    AllowSession,
    /// Persist allowance / "don't ask again" across future turns or sessions.
    DontAskAgain,
    /// Switch the agent's execution/permission mode (e.g. "switch to auto mode").
    SwitchMode,
    /// Deny / cancel this once ("No", "No, cancel", "No, and tell ...",
    /// "No, and ask again next time").
    No,
    /// A no that is kept ("No, and block reads", "No, and don't ask
    /// again"): it refuses the like from now on, as `DontAskAgain`
    /// allows it. Never what "no, this once" means.
    NoAlways,
    /// Any other choice.
    Other,
}

impl ChoiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::AllowSession => "allow_session",
            Self::DontAskAgain => "dont_ask_again",
            Self::SwitchMode => "switch_mode",
            Self::No => "no",
            Self::NoAlways => "no_always",
            Self::Other => "other",
        }
    }
}

/// One parsed option inside an approval chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalChoice {
    /// 0-based position in the displayed list.
    pub index: usize,
    /// Leading item number (`1`, `2`, …) if shown.
    pub number: Option<u32>,
    /// Cleaned label text (without leading cursor mark or item number).
    pub label: String,
    /// True if the cursor mark (`❯`, `›`, `>`) currently rests on this choice.
    pub selected: bool,
    /// Semantic kind of the choice so callers can pick `"once"` or `"no"` safely by meaning.
    pub kind: ChoiceKind,
}

/// Structured details of a live tool-approval dialog on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub key: u64,
    pub question: String,
    pub tool: Option<String>,
    pub command: Option<String>,
    /// Set when the terminal folded long output (e.g. agy's `"⋯ 28 lines hidden"`).
    pub folded_lines: Option<usize>,
    pub choices: Vec<ApprovalChoice>,
    pub selected_index: Option<usize>,
}

/// Startup folder-trust safety dialog (Claude Code workspace/`--add-dir` trust or Codex directory trust).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTrustPrompt {
    pub folder: Option<String>,
    /// True when Claude Code is asking about a secondary `--add-dir` directory rather than the primary workspace.
    pub is_add_dir: bool,
    /// 0-based index of the currently highlighted choice (`❯` / `›`).
    pub selected_index: usize,
    /// 0-based index of `"Yes, I trust this folder"` / `"Yes, add it"` / `"Yes, continue"`.
    pub trust_index: usize,
    /// True when the cursor is already on the trust choice.
    pub selected_trust: bool,
    /// A mark was seen on one of its choices (without one,
    /// `selected_index` is the first by default and nothing is known of
    /// where Return would land).
    pub marked: bool,
    /// The question as it is said to the person where it counts down
    /// ("Trust /var/tmp/new?").
    pub question: String,
}

/// Usage or rate-limit notice parsed from an agent's screen footer/status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageInfo {
    pub line: String,
    pub percent: Option<u8>,
    pub limit_reached: bool,
}

/// Normalizes Unicode apostrophes/quotes, strips zero-width characters, and collapses whitespace
/// so choice matching handles typographic punctuation like Claude Code's `don’t ask again` (U+2019).
fn normalize_choice_text(label: &str) -> String {
    let replaced: String = label
        .chars()
        .filter(|c| !matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}'))
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' | '\u{2032}' | '`' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{00A0}' => ' ',
            other => other.to_ascii_lowercase(),
        })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strips a single trailing parenthetical shortcut hint such as `(y)`,
/// `(1)`, `(esc)`, `(shift+tab)`: a key and nothing else. Any short
/// word in brackets used to go: "Yes (always)" lost its "(always)" and
/// read as a yes for this once (a code review, 2026-11-27).
fn strip_trailing_shortcut(s: &str) -> &str {
    let t = s.trim();
    if let Some(open) = t.rfind('(')
        && t.ends_with(')')
    {
        let inside = &t[open + 1..t.len() - 1];
        let key = |k: &str| (k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric())) || matches!(k, "esc" | "escape" | "enter" | "return" | "tab" | "space" | "shift" | "ctrl" | "alt" | "cmd");
        if !inside.is_empty() && inside.split('+').all(key) {
            return t[..open].trim_end();
        }
    }
    t
}

/// A choice's label less what it quotes: the spans in quotes taken out
/// ('…', "…", and `…`, which `normalize_choice_text` has made '…'). An
/// apostrophe inside a word ("don't") opens nothing: a quote opens at
/// the label's start or after a space or a bracket, and closes before
/// a space, the end or punctuation. What a label quotes is a command
/// or a path the agent chose, not the CLI's wording, and may say
/// anything.
fn own_words(lower: &str) -> String {
    let chars: Vec<char> = lower.chars().collect();
    let closes = |j: usize, c: char| chars[j] == c && (j + 1 == chars.len() || chars[j + 1].is_whitespace() || matches!(chars[j + 1], ',' | '.' | ';' | ':' | ')' | ']' | '?'));
    let mut out = String::with_capacity(lower.len());
    // (A quote with nothing to close it from here has nothing to close it
    // further on either: not looked for twice, so a label of many
    // unclosed quotes is still read once through.)
    let mut unclosed = [false; 2];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let which = usize::from(c == '"');
        let opens = matches!(c, '\'' | '"') && !unclosed[which] && (i == 0 || chars[i - 1].is_whitespace() || matches!(chars[i - 1], '(' | '[' | ':'));
        if opens {
            match (i + 1..chars.len()).find(|&j| closes(j, c)) {
                Some(end) => {
                    out.push(' ');
                    i = end + 1;
                    continue;
                }
                None => unclosed[which] = true,
            }
        }
        out.push(c);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a choice does, from its label. From the CLI's own words alone
/// (`own_words`): a label quotes the command or the path it would
/// allow, and words in those moved a choice from one kind to another.
/// Antigravity's "always allow in this conversation for commands that
/// start with 'cat settings.json'" read as a grant kept in the
/// settings; Claude Code's "always allow access to" a folder whose
/// name had "this session" in it read as one for this session, which a
/// caller asking for the session alone would then have taken, and it
/// is kept for good (a code review, 2026-11-27). A yes for this once
/// is only ever one of a list of whole labels, quoted words or not.
pub fn classify_choice_kind(label: &str) -> ChoiceKind {
    let whole = normalize_choice_text(label);
    if whole.is_empty() {
        return ChoiceKind::Other;
    }
    // The CLIs' own sentences around what they quote, told by how they
    // begin and end, before anything inside them is read: what a label
    // quotes may have quotes of its own (`python3 -c '` and a script:
    // the outer quote then "closes" at the inner one, and the script's
    // words were read as the CLI's), and a path may hold any words
    // ("this session notes/": a folder kept for the project read as
    // allowed for the session).
    if whole.starts_with("yes, and always allow in this conversation for commands that start with ") {
        return ChoiceKind::AllowSession;
    }
    if whole.starts_with("yes, and always allow for commands that start with ") || (whole.starts_with("yes, and always allow access to ") && whole.ends_with(" from this project")) {
        return ChoiceKind::DontAskAgain;
    }
    let lower = own_words(&whole);
    if lower.contains("switch to auto")
        || lower.contains("auto-accept")
        || lower.contains("auto mode")
        || lower.contains("shift+tab")
        || lower.contains("bypass permission")
        || lower.contains("full-auto")
    {
        return ChoiceKind::SwitchMode;
    }
    // A no that is kept, before anything else a "no" could be taken for.
    let is_no =
        lower == "no" || lower.starts_with("no,") || lower.starts_with("no ") || lower.starts_with("no(") || lower.starts_with("deny") || lower.starts_with("reject") || lower.starts_with("never");
    if is_no
        && (lower.contains("block")
            || lower.contains("don't ask")
            || lower.contains("dont ask")
            || lower.contains("do not ask")
            || lower.contains("always")
            || lower.contains("never")
            || lower.contains("remember"))
    {
        return ChoiceKind::NoAlways;
    }
    if lower.contains("persist to settings")
        || lower.contains("settings.json")
        || lower.contains("keep allowing")
        || lower.contains("don't ask")
        || lower.contains("dont ask")
        || lower.contains("do not ask")
        || lower.contains("remember")
        || lower.contains("all future")
        || lower.contains("for future")
    {
        return ChoiceKind::DontAskAgain;
    }
    // (For this session or conversation only where the CLI says so
    // before it names any path: a path is not in quotes, and its own
    // words are not the CLI's.)
    let head = lower.split(' ').take_while(|word| !word.contains('/') && !word.starts_with('~')).collect::<Vec<_>>().join(" ");
    if lower.contains("always allow") || lower.contains("always approve") || lower.contains("always run") {
        if head.contains("this conversation") || head.contains("this session") {
            return ChoiceKind::AllowSession;
        }
        return ChoiceKind::DontAskAgain;
    }
    if lower.contains("allow all")
        || lower.contains("allow reading")
        || lower.contains("allow edits")
        || lower.contains("during this session")
        || lower.contains("in this session")
        || lower.contains("for this session")
        || lower.contains("this conversation")
        || lower.contains("rest of this session")
        || lower.contains("for commands that start with")
        || lower.contains("for commands in")
    {
        return ChoiceKind::AllowSession;
    }
    if lower == "no" || lower.starts_with("no,") || lower.starts_with("no ") || lower.starts_with("no(") || lower.contains("cancel") || lower.starts_with("deny") || lower.starts_with("reject") {
        return ChoiceKind::No;
    }
    // Strict allowlist for single-use `Once`: NEVER classify unknown `"Yes, and ..."` or
    // standing-permission phrasing as `Once`.
    // (Of the whole label: one that quotes anything is not on the list.)
    let clean = strip_trailing_shortcut(&whole).trim_end_matches('.');
    if matches!(
        clean,
        "yes"
            | "y"
            | "yes, proceed"
            | "yes, continue"
            | "yes, run"
            | "yes, run command"
            | "yes, run this command"
            | "yes, run tool"
            | "yes, run once"
            | "yes, once"
            | "yes, allow once"
            | "yes, allow this once"
            | "yes, approve once"
            | "yes, this time"
            // (Claude Code's read outside the working directories: a yes for this read alone.)
            | "yes, but ask again next time"
            | "yes, and ask again next time"
            | "yes, allow"
            | "yes, allow this"
            | "yes, allow this tool"
            | "yes, allow this command"
            | "allow once"
            | "approve once"
            | "run once"
            | "run command"
            | "proceed"
    ) {
        return ChoiceKind::Once;
    }
    ChoiceKind::Other
}

/// Detects Claude Code's startup folder-trust / `--add-dir` trust dialog or Codex's
/// directory-trust safety dialog and reports exact cursor & trust indices.
pub fn parse_folder_trust(screen: &str) -> Option<FolderTrustPrompt> {
    let lower = normalize_choice_text(screen);
    let is_add_dir = lower.contains("adding a new directory") && (lower.contains("yes, add") || lower.contains("is this a directory you created or one you trust"));
    let is_codex_trust = lower.contains("do you trust the contents of this directory") && lower.contains("yes, continue");
    let is_folder_trust = is_add_dir
        || is_codex_trust
        || lower.contains("trust this folder")
        || lower.contains("is this a project you created or one you trust")
        || lower.contains("is this a directory you created or one you trust");
    if !is_folder_trust {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let mut folder: Option<String> = None;
    for (i, l) in lines.iter().enumerate() {
        let lt = l.to_ascii_lowercase();
        if lt.contains("accessing workspace") || lt.contains("adding a new directory") {
            for next in &lines[i + 1..] {
                let t = next.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with('/') || t.starts_with('~') {
                    folder = Some(t.to_owned());
                }
                break;
            }
        } else if is_codex_trust
            && folder.is_none()
            && let Some(pos) = lt.find("you are in ")
        {
            let rest = l[pos + "you are in ".len()..].trim().trim_end_matches('.');
            if rest.starts_with('/') || rest.starts_with('~') {
                folder = Some(rest.to_owned());
            }
        }
    }
    let marks = ['❯', '›', '>'];
    let mut choices: Vec<(bool, String)> = Vec::new();
    for l in &lines {
        let trimmed = l.trim();
        let (marked, rest) = match marks.iter().find_map(|m| trimmed.strip_prefix(*m)) {
            Some(r) => (true, r.trim_start()),
            None => (false, trimmed),
        };
        let rest_clean = rest.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map(|(_, r)| r.trim_start()).unwrap_or(rest);
        let rl = normalize_choice_text(rest_clean);
        if rl.starts_with("yes, i trust")
            || rl.starts_with("yes, trust")
            || rl.starts_with("yes, add")
            || (is_codex_trust && rl.starts_with("yes, continue"))
            || rl.starts_with("no, exit")
            || rl.starts_with("no, quit")
            || rl.starts_with("no, i don't trust")
        {
            choices.push((marked, rest_clean.to_owned()));
        }
    }
    let trust_index = choices.iter().position(|(_, c)| c.to_ascii_lowercase().starts_with("yes"))?;
    let marked = choices.iter().any(|(m, _)| *m);
    let selected_index = choices.iter().position(|(m, _)| *m).unwrap_or(0);
    let question = match &folder {
        Some(folder) if is_add_dir => format!("Add and trust {folder}?"),
        Some(folder) => format!("Trust {folder}?"),
        None => "Trust this folder?".to_owned(),
    };
    Some(FolderTrustPrompt { folder, is_add_dir, selected_index, trust_index, selected_trust: selected_index == trust_index, marked, question })
}

/// Parses a structured tool-approval prompt across Claude Code, Codex, and Antigravity (`agy`).
/// Unlike `approvable()`, returns choices even when the cursor is not on choice 1, and labels
/// each choice semantically (`Once`, `AllowSession`, `DontAskAgain`, `SwitchMode`, `No`, `Other`).
pub fn parse_approval(agent: Agent, screen: &str) -> Option<ApprovalPrompt> {
    if parse_folder_trust(screen).is_some() {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let navigated = screen.contains("↑/↓ Navigate") || agent == Agent::Antigravity;
    let marks: &[char] = if navigated { &['❯', '›', '>'] } else { &['❯', '›'] };
    let parse_numbered = |raw: &str| -> Option<(bool, u32, String)> {
        let t = raw.trim();
        if t.is_empty() {
            return None;
        }
        let (selected, rest) = match marks.iter().find_map(|m| t.strip_prefix(*m)) {
            Some(r) => (true, r.trim_start()),
            None => (false, t),
        };
        let (num_s, label) = rest.split_once(". ")?;
        if num_s.is_empty() || !num_s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let num = num_s.parse::<u32>().ok()?;
        Some((selected, num, label.trim().to_owned()))
    };
    // The same, with the column its number stands at: a chooser's
    // answers stand in one column, marked or not ("> 1." and "  2.",
    // " ❯ 1." and "   2."), and what a choice quotes of a command does
    // not ("3. Read the log" at the line's start, inside a script an
    // "always allow" choice shows): only a line at the answers' own
    // column is an answer. Before its number: spaces, and one mark at
    // most.
    let numbered_at = |raw: &str| -> Option<(bool, u32, String, usize)> {
        let lead: Vec<char> = raw.chars().take_while(|c| !c.is_ascii_digit()).collect();
        if !lead.iter().all(|c| c.is_whitespace() || marks.contains(c)) || lead.iter().filter(|c| marks.contains(c)).count() > 1 {
            return None;
        }
        let (selected, num, label) = parse_numbered(raw)?;
        Some((selected, num, label, lead.len()))
    };
    // What ends a chooser: the line under its answers.
    let footer_marker = |l: &str| {
        let tl = l.trim().to_ascii_lowercase();
        tl.starts_with("esc to cancel")
            || tl.starts_with("press enter to confirm")
            || tl.starts_with("press enter to continue")
            || tl.starts_with("↑/↓ navigate")
            || tl.starts_with("enter to select")
            || tl.starts_with("tab to amend")
            || tl.starts_with("ctrl+")
    };

    let is_known_question = |q: &str| -> bool {
        let t = q.trim().to_ascii_lowercase();
        t.ends_with('?') && (t.starts_with("do you want to ") || t.starts_with("would you like to ") || t.starts_with("run this command") || t.starts_with("allow "))
    };

    // Match single-line OR 2-line wrapped approval question headers (e.g. long edit file paths).
    let (q_start_idx, q_idx, question_text) = lines.iter().enumerate().rev().find_map(|(i, l)| {
        let trimmed = l.trim();
        if !trimmed.ends_with('?') {
            return None;
        }
        let first_under = || lines[i + 1..].iter().find(|nl| !nl.trim().is_empty()).and_then(|nl| parse_numbered(nl)).is_some_and(|(_, num, _)| num == 1);
        if is_known_question(trimmed) || (navigated && first_under()) {
            return Some((i, i, trimmed.to_owned()));
        }
        // Try joining with the preceding non-empty line for wrapped questions (`Do you want to make this edit to` / `path/to/file.rs?`).
        if let Some((pi, prev)) = lines[..i].iter().enumerate().rev().find(|(_, pl)| !pl.trim().is_empty()) {
            let joined = format!("{} {}", prev.trim(), trimmed);
            if is_known_question(&joined) {
                return Some((pi, i, joined));
            }
        }
        None
    })?;

    // The question found is the last one on the screen. Where an earlier
    // one stands above it with a marked answer between them, the last is
    // one an answer quotes (a transcript of a chooser, in a command an
    // "always allow" answer shows), and the CLI's own cursor is up there,
    // on an answer of the real question: this box would be read as the
    // quoted one, its quoted mark on its quoted Yes, and Return pressed
    // on whatever the real mark stands on. Not read at all. (A code
    // review, 2026-11-27.)
    if let Some(earlier) = lines[..q_start_idx].iter().rposition(|l| is_known_question(l.trim()))
        && lines[earlier + 1..q_start_idx].iter().any(|l| numbered_at(l).is_some_and(|(selected, ..)| selected))
    {
        return None;
    }
    let rest = &lines[q_idx + 1..];
    // Where the chooser ends: its foot, the last such lines read (a run
    // of them: Antigravity draws two). Looked for from the bottom: a
    // line that only looks like one, quoted in an answer's label
    // ("Ctrl+G opens the editor", in a script an answer shows), ended
    // the chooser where it stood, the answers under it and its No lost
    // (a code review, 2026-11-27).
    let foot = rest.iter().rposition(|l| footer_marker(l)).map(|mut at| {
        while let Some(before) = rest[..at].iter().rposition(|l| !l.trim().is_empty()) {
            if !footer_marker(rest[before]) {
                break;
            }
            at = before;
        }
        at
    });
    let region = &rest[..foot.unwrap_or(rest.len())];
    // Every numbered line between the question and the foot, with its
    // column; and the one that carries the mark, which says which
    // column the answers stand in. One mark, or the box isn't read: a
    // second marked number is a line an answer quotes (a blockquote,
    // "> 3. …", where `>` is the mark), and which of the two the CLI's
    // own cursor is can't be told. (The mark used to be looked for
    // from a first answer down, and the *last* "1." with a mark under
    // it began the chooser: a list an answer quoted, "1. …" at the
    // answers' column with the mark moved to No, was read as a chooser
    // of that line and No. The same review.)
    let numbered: Vec<(usize, bool, u32, String, usize)> = region.iter().enumerate().filter_map(|(i, l)| numbered_at(l).map(|(selected, num, label, col)| (i, selected, num, label, col))).collect();
    let mut marked = numbered.iter().filter(|n| n.1);
    let (Some(mark), None) = (marked.next(), marked.next()) else { return None };
    let choice_col = mark.4;
    // The answers: the numbered lines at that column, all of them from
    // the question to the foot. They count up from 1 (a number may be
    // left out: Antigravity's 1, 2, 4), each once: a number seen twice
    // or out of order there is a list quoted inside an answer (or in
    // the command shown above them), indented as they are, and which
    // lines are answers can't be told. Not read as a question at all:
    // half its answers would be worse.
    let answers: Vec<&(usize, bool, u32, String, usize)> = numbered.iter().filter(|n| n.4 == choice_col).collect();
    if answers.first().map(|a| a.2) != Some(1) || !answers.windows(2).all(|w| w[1].2 > w[0].2 && w[1].2 <= w[0].2 + 3) {
        return None;
    }
    let first_choice_rel = answers[0].0;
    if rest[..first_choice_rel].iter().map(|l| l.trim()).any(|l| l.ends_with('?') && !l.starts_with("Reason:")) {
        return None;
    }
    if rest.iter().any(|l| l.to_ascii_lowercase().contains("keep planning")) {
        return None;
    }

    // Each answer's label: its line, and what stands under it to the
    // next answer. A label may quote the command it would allow, and a
    // command of several lines is quoted with its line ends, blank
    // lines and numbered lines of its own (at another column) among
    // them: all of it is the label's. (A box whose "always allow"
    // choices quoted `python3 -c '` and a script was read as two
    // choices, its "No" never seen, and `olo agent approve --no` found
    // no "No" to choose: a minder's report, 2026-11-27.) The last
    // answer's goes on as a wrapped label does, to a blank line or a
    // rule (three lines at most where no foot was found). One pass over the rows (each answer once:
    // every line under an answer used to look ahead for the next).
    let is_rule_or_chrome =
        |t: &str| t.starts_with('─') || t.starts_with('╌') || t.starts_with('═') || t.starts_with('━') || t.starts_with('❯') || t.starts_with('›') || t.starts_with("> ") || t.starts_with("? for ");
    let mut choices: Vec<ApprovalChoice> = Vec::with_capacity(answers.len());
    for (index, a) in answers.iter().enumerate() {
        let mut label = a.3.clone();
        let under = region[a.0 + 1..answers.get(index + 1).map_or(region.len(), |next| next.0)].iter().map(|l| l.trim());
        let mut add = |t: &str| {
            label.push(' ');
            label.push_str(t);
        };
        if index + 1 < answers.len() {
            under.filter(|t| !t.is_empty()).for_each(&mut add);
        } else if foot.is_some() {
            // (To the foot, where one was found: a last answer wrapped
            // over more lines than three lost its end, and "Yes, but ask
            // again next time" cut short is no yes for this once.)
            under.take_while(|t| !t.is_empty() && !is_rule_or_chrome(t)).for_each(&mut add);
        } else {
            under.take_while(|t| !t.is_empty() && !is_rule_or_chrome(t)).take(3).for_each(&mut add);
        }
        choices.push(ApprovalChoice { index, number: Some(a.2), kind: classify_choice_kind(&label), label, selected: a.1 });
    }
    if choices.len() < 2 {
        return None;
    }
    // Must actually look like a yes/no approval dialog (at least one Once or No choice).
    if !choices.iter().any(|c| matches!(c.kind, ChoiceKind::Once | ChoiceKind::No)) {
        return None;
    }
    // The answer the mark stands on (one, by the above).
    let selected_index = choices.iter().position(|c| c.selected)?;

    // Extract tool name, full command/arguments, and folded-line indicator from the dialog body above `q_start_idx`
    // (and between `q_idx` and `first_choice_rel` for Codex).
    let dialog_start = lines[..q_start_idx]
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.starts_with("────────") && t.chars().all(|c| c == '─')
        })
        .unwrap_or_else(|| q_start_idx.saturating_sub(24));
    let header_slice = &lines[dialog_start..q_start_idx];
    let between_slice = &rest[..first_choice_rel];

    let mut tool: Option<String> = None;
    let mut cmd_lines: Vec<String> = Vec::new();
    let mut folded_lines: Option<usize> = None;

    // A line that says lines are folded away, and how many: the mark
    // (an ellipsis) and its count, alone on their line, "⋯ 28 lines
    // hidden" or, as Antigravity writes it, "⋯ (19 lines hidden)"; the
    // count was read only without the brackets, and a folded command was
    // told as a whole one (seen on two boxes, 2026-11-27). Never words
    // inside a command: `python3 -c 'print("lines hidden")'` is whole.
    let parse_folded = |t: &str| -> Option<usize> {
        let said = t.trim().strip_prefix('⋯').or_else(|| t.trim().strip_prefix('…'))?.trim();
        let said = said.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(said);
        let (count, words) = said.split_once(' ')?;
        matches!(words.trim().to_ascii_lowercase().as_str(), "lines hidden" | "line hidden" | "more lines hidden").then(|| count.parse::<usize>().ok()).flatten()
    };

    // Detect dashed command block (Claude Code 2.1.331: `╌╌╌╌` ... `╌╌╌╌`).
    let is_dash = |l: &str| {
        let t = l.trim();
        t.len() >= 12 && t.chars().all(|c| c == '╌' || c == '─')
    };
    let dash_positions: Vec<usize> = header_slice.iter().enumerate().filter_map(|(i, l)| is_dash(l).then_some(i)).collect();
    if dash_positions.len() >= 2 {
        let a = dash_positions[dash_positions.len() - 2];
        let b = dash_positions[dash_positions.len() - 1];
        for l in &header_slice[a + 1..b] {
            let t = l.trim().trim_start_matches('│').trim();
            if let Some(n) = parse_folded(t) {
                folded_lines = Some(n);
            } else if !t.is_empty() {
                cmd_lines.push(t.to_owned());
            }
        }
    }

    // Check `Requesting permission for:` (Antigravity) and tool headers (`Bash command`, `Edit file`, etc.).
    let mut in_req = false;
    for l in header_slice {
        let t = l.trim();
        if let Some(n) = parse_folded(t) {
            folded_lines = Some(n);
        }
        let tl = t.to_ascii_lowercase();
        if tool.is_none() {
            if tl == "bash command" || tl == "command" || tl.starts_with("bash(") {
                tool = Some("Bash".into());
            } else if tl.starts_with("edit file") || tl.starts_with("edit(") {
                tool = Some("Edit".into());
            } else if tl.starts_with("write file") || tl.starts_with("create file") || tl.starts_with("write(") {
                tool = Some("Write".into());
            } else if tl.starts_with("read file") || tl.starts_with("read(") {
                tool = Some("Read".into());
            } else if let Some(after) = t.strip_prefix("● ").and_then(|s| s.split('(').next()) {
                let name = after.trim();
                if !name.is_empty() && name.len() <= 40 && !name.contains(' ') {
                    tool = Some(name.to_owned());
                }
            }
        }
        if tl.starts_with("requesting permission for:") {
            in_req = true;
            let after = t["Requesting permission for:".len()..].trim();
            if !after.is_empty() && cmd_lines.is_empty() {
                cmd_lines.push(after.to_owned());
            }
            continue;
        }
        if in_req && cmd_lines.is_empty() && !t.is_empty() && !is_dash(l) {
            cmd_lines.push(t.to_owned());
        } else if in_req && !t.is_empty() && !is_dash(l) && parse_folded(t).is_none() {
            cmd_lines.push(t.to_owned());
        }
    }

    // Codex puts `$ command` between the question and `1. Yes`.
    for l in between_slice {
        let t = l.trim();
        if let Some(n) = parse_folded(t) {
            folded_lines = Some(n);
        }
        if let Some(c) = t.strip_prefix("$ ") {
            if tool.is_none() {
                tool = Some("Bash".into());
            }
            cmd_lines.push(c.trim().to_owned());
        } else if !cmd_lines.is_empty() && !t.is_empty() && !t.starts_with("Environment:") && !t.starts_with("Reason:") {
            cmd_lines.push(t.to_owned());
        }
    }

    // Claude Code older format (no dashed box, indented command directly under `Bash command`).
    if cmd_lines.is_empty() {
        let mut saw_tool_hdr = false;
        for l in header_slice {
            let t = l.trim();
            let tl = t.to_ascii_lowercase();
            if tl == "bash command" || tl == "edit file" || tl == "write file" {
                saw_tool_hdr = true;
                continue;
            }
            if saw_tool_hdr && !t.is_empty() && !tl.starts_with("tip:") && !t.starts_with('⚠') && !is_dash(l) {
                cmd_lines.push(t.to_owned());
                break;
            }
        }
    }

    if tool.is_none() {
        let ql = question_text.to_ascii_lowercase();
        if ql.contains("command") {
            tool = Some("Bash".into());
        } else if ql.contains("edit") {
            tool = Some("Edit".into());
        } else if ql.contains("write") || ql.contains("create") {
            tool = Some("Write".into());
        }
    }

    let command = (!cmd_lines.is_empty()).then(|| cmd_lines.join("\n"));

    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in &lines[q_start_idx.saturating_sub(12)..] {
        l.chars().filter(|c| c.is_alphanumeric()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>().hash(&mut h);
    }

    Some(ApprovalPrompt { key: h.finish(), question: question_text, tool, command, folded_lines, choices, selected_index: Some(selected_index) })
}

/// Detects whether a genuine interactive question / non-approval chooser is active on screen
/// (never fires on transient `"esc to cancel"`, idle prompt lines, or prose mentioning `"enter to confirm"`).
pub fn parse_question(agent: Agent, screen: &str) -> Option<String> {
    if let Some(trust) = parse_folder_trust(screen) {
        return Some(match trust.folder {
            Some(f) => format!("Trust folder {f}?"),
            None => "Trust this folder?".into(),
        });
    }
    if parse_approval(agent, screen).is_some() {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let non_empty: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let bottom8 = non_empty[non_empty.len().saturating_sub(8)..].join("\n").to_ascii_lowercase();
    let navigated = bottom8.contains("↑/↓ navigate");
    let has_chooser_footer = navigated || bottom8.contains("enter to select") || bottom8.contains("enter to confirm") || bottom8.contains("press enter to continue");
    if !has_chooser_footer {
        return None;
    }
    // Evaluate prompt lines against the WHOLE screen and exclude them so Claude's own `❯`
    // prompt never counts as a chooser cursor mark.
    let prompt_idxs = prompt_line_indices(agent, &lines);
    let dialog_top = lines.len().saturating_sub(18);
    let has_mark = lines.iter().enumerate().skip(dialog_top).any(|(i, l)| {
        if prompt_idxs.contains(&i) {
            return false;
        }
        let t = l.trim_start();
        t.starts_with('❯') || t.starts_with('›') || (navigated && t.starts_with("> "))
    });
    if !has_mark {
        return None;
    }
    lines[dialog_top..].iter().rev().map(|l| l.trim()).find(|l| l.ends_with('?')).map(str::to_owned).or_else(|| Some("waiting for choice".into()))
}

/// What an agent that says it failed has on its screen of why: the
/// last line among its last sixty that names an error of its model's
/// service ("API Error: 529 …", as Claude Code writes it, whatever it
/// stands in: a retry's count, a tool's result), without its bullet.
/// For the *words* of a failure the agent has itself declared, where
/// nothing else has them: the service gave its last line on the screen
/// as the error, which was the last thing it had been doing (the
/// ShiftUp lead's item 49, 2026-11-27: "every Claude child's turn ended
/// as `error` with its last working sentence as the error's text").
/// Never what makes a screen a failure: that is [`parse_api_error`]'s,
/// which is strict about where such a line may stand.
pub fn error_line(screen: &str) -> Option<String> {
    // (Of this turn: no further up than the person's last message, as
    // it is drawn. And a line that *is* the error, not one that speaks
    // of it: what a tool printed, "the fixture expects API Error: 401",
    // is not why the turn failed. A code review, 2026-11-27.)
    let said = |l: &str| {
        let t = l.trim_start();
        ['❯', '›'].iter().any(|m| t.strip_prefix(*m).is_some_and(|rest| !rest.trim().is_empty())) || t.strip_prefix("> ").is_some_and(|rest| !rest.trim().is_empty())
    };
    screen.lines().rev().filter(|l| !l.trim().is_empty()).take(60).take_while(|l| !said(l)).find_map(|l| {
        let words = l.trim().trim_start_matches(['●', '•', '⎿', '└', '│', '*', ' ']).trim();
        words.get(..9).is_some_and(|head| head.eq_ignore_ascii_case("api error")).then(|| words.to_owned())
    })
}

/// Detects an API / stream mid-response error that ended the CURRENT turn without a normal Stop hook
/// (e.g. Claude Code's `"API Error: Connection lost mid-response"` or Codex's `"■ ..."`).
/// Stops scanning at prior turn boundaries so a stale error from an earlier turn never poisons
/// a later healthy turn.
pub fn parse_api_error(agent: Agent, screen: &str) -> Option<String> {
    if classify(agent, screen) == State::Working {
        return None;
    }
    let lines: Vec<&str> = screen.lines().collect();
    let prompt_idxs = prompt_line_indices(agent, &lines);
    let mut seen_non_chrome = 0usize;
    for (idx, raw) in lines.iter().enumerate().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Skip horizontal rules & prompt input box / footer chrome.
        if line.chars().all(|c| matches!(c, '─' | '╌' | '═' | '━' | '╭' | '╮' | '╰' | '╯')) {
            continue;
        }
        // If we cross back to a non-empty user turn line (`› ...`, `❯ ...`, `> ...`) that is NOT
        // the live empty prompt box / placeholder, any error above it belonged to an earlier turn!
        let is_user_turn_line = (line.starts_with('❯') || line.starts_with('›') || line.starts_with("> ")) && {
            let body = line.trim_start_matches(['❯', '›', '>', ' ', '\u{a0}']).trim();
            let bl = body.to_ascii_lowercase();
            !body.is_empty() && !bl.starts_with("ask ") && !bl.starts_with("try \"") && !bl.starts_with("accept-edits mode") && !bl.starts_with("auto mode") && !bl.starts_with("plan mode")
        };
        if is_user_turn_line {
            break;
        }
        if prompt_idxs.contains(&idx) {
            continue;
        }
        let low_raw = line.to_ascii_lowercase();
        if low_raw.contains("? for shortcuts") || low_raw.contains("← for agents") || low_raw.contains("shift+tab to cycle") || low_raw.starts_with("gpt-") {
            continue;
        }
        // Codex's turn-completion summary or a non-error assistant reply means the current turn succeeded!
        if agent == Agent::Codex && (low_raw.contains("worked for ") || (line.starts_with("• ") && !low_raw.contains("error") && !low_raw.contains("failed") && !low_raw.contains("interrupted"))) {
            break;
        }
        seen_non_chrome += 1;
        if seen_non_chrome > 12 {
            break;
        }
        if let Some(after_sq) = line.strip_prefix("■ ").map(str::trim).filter(|s| !s.is_empty()) {
            // Skip Codex's expected user-denial notice ("Conversation interrupted - tell the model what to do differently")
            // if a turn isn't failed; wait: when an API/stream error actually happens, Codex prints "■ stream disconnected" / "■ unexpected status".
            // Keep real errors (including stream/API failures), and still report genuine unrecovered interrupts only when it's the current turn's tail!
            return Some(after_sq.to_owned());
        }
        let l = line.trim_start_matches(['⎿', '●', '•', '✗', '✕', '⚠', ' ']).trim();
        let low = l.to_ascii_lowercase();
        if l.starts_with('>') || l.starts_with('❯') || l.starts_with('›') || l.starts_with('+') || l.starts_with('-') {
            continue;
        }
        if low.starts_with("api error:")
            || low.starts_with("api error (")
            || low.contains("connection lost mid-response")
            || low.contains("stream disconnected")
            || low.contains("overloaded_error")
            || (low.starts_with("error:") && low.contains("timed out"))
        {
            return Some(l.to_owned());
        }
    }
    None
}

/// Parses usage percentage and rate-limit banners from an agent's live screen footer/status bar.
pub fn parse_usage(screen: &str) -> Option<UsageInfo> {
    let tail: Vec<&str> = screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(6).collect();
    for line in tail {
        let low = line.to_ascii_lowercase();
        let is_limit_hit = low.contains("you've hit your limit")
            || low.contains("hit your usage limit")
            || low.contains("usage limit reached")
            || low.contains("rate limit reached")
            || low.contains("weekly limit reached")
            || low.contains("session limit reached")
            || low.contains("out of extra usage");
        let is_usage_pct =
            (low.contains("you've used ") || low.contains("used ")) && low.contains('%') && (low.contains("limit") || low.contains("weekly") || low.contains("session") || low.contains("quota"));
        if is_limit_hit || is_usage_pct {
            let percent = line.split('%').next().and_then(|before| before.rsplit(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse::<u8>().ok()).filter(|&n| n <= 100);
            let limit_reached = is_limit_hit || percent == Some(100);
            return Some(UsageInfo { line: line.to_owned(), percent, limit_reached });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(identify("2.1.311", "/home/u/.local/share/claude/versions/2.1.311", &args(&["claude"])), Some(Agent::Claude));
        assert_eq!(identify("node", "/usr/bin/node", &args(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"])), Some(Agent::Claude));
        assert_eq!(identify("codex", "/usr/local/bin/codex", &args(&["codex"])), Some(Agent::Codex));
        assert_eq!(identify("node", "/usr/bin/node", &args(&["node", "/usr/local/bin/codex", "--full-auto"])), Some(Agent::Codex));
        assert_eq!(identify("vim", "/usr/bin/vim", &args(&["vim", "claude.md"])), None);
        // Codex's server for every session (as `ps` has it, codex-cli
        // 0.170), and what isn't one: a window's own Codex, a run of
        // `codex exec`, Claude Code.
        let packaged = "/home/u/.codex/packages/app-server-daemon/0.170.0/codex";
        assert!(shared_server("codex", packaged, &args(&[packaged, "app-server", "--listen", "unix://", "--analytics-default-enabled"])));
        assert!(shared_server("codex", packaged, &args(&[packaged, "app-server", "daemon", "pid-update-loop"])));
        assert!(!shared_server("codex", "/usr/local/bin/codex", &args(&["codex", "--full-auto"])));
        assert!(!shared_server("codex", "/usr/local/bin/codex", &args(&["codex", "exec", "-m", "x", "look at app-server"])));
        assert!(!shared_server("claude", "/home/u/.local/share/claude/versions/2.1.311", &args(&["claude", "app-server"])));
    }

    #[test]
    fn codex_goal_clock_isnt_a_change() {
        let at = |t: &str| format!("• Did the thing.\n\n  Pursuing goal ({t}) · ship the browser\n› \n");
        assert_eq!(fingerprint(&at("17h 52m")), fingerprint(&at("17h 53m")));
        assert_ne!(fingerprint(&at("17h 52m")), fingerprint("• Did the thing.\n\n  Pursuing goal (17h 52m) · ship the browser\n› more\n"));
        assert_ne!(fingerprint("✻ Pondering… (12s)"), fingerprint("✻ Pondering… (13s)"));
    }

    /// What Auto Approve answers: leave to go on, "1. Yes" chosen (Claude
    /// Code's, as Ben's screenshot of 2026-11-09 has it, and Codex's); the
    /// same question as it counts down; not a plan, nor another answer
    /// chosen, nor a question that isn't a dialog's.
    #[test]
    fn what_auto_approve_answers() {
        let claude = |left: &str| {
            format!(
                " Bash command\n   git commit -q -m \"$MSG\" 2>&1 | tail -15\n   Commit the mechanical rename\n Dangerous rm operation not ruled out\n ⚠ Claude Code will automatically block this request in {left}, to avoid holding up an unattended session\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n\n Esc to cancel · Tab to amend\n"
            )
        };
        let a = approvable(&claude("1:50")).expect("Claude Code's");
        assert_eq!(a.question, "Do you want to proceed?");
        assert_eq!(approvable(&claude("1:49")).map(|b| b.key), Some(a.key));
        // Claude Code 2.1.322's, as Terminal read it (the fake model's
        // command): the "●" before the tool call blinks while it asks, the
        // same question either way.
        let real = |bullet: &str| {
            format!(
                "❯ approve-test\n\n{bullet} Mark the test approved\n  ⎿  $ touch /var/tmp/kig/work/claude-approved\n\n────────────────────────────────\n Bash command\n Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below\n Mark the test approved\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n touch /var/tmp/kig/work/claude-approved\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and always allow access to /var/tmp/kig/work from this project\n   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n   4. No\n\n Esc to cancel · Tab to amend\n"
            )
        };
        let on = approvable(&real("●")).expect("Claude Code 2.1.322's");
        assert_eq!(approvable(&real(" ")).map(|b| b.key), Some(on.key));
        // Codex 0.167's, as Terminal read it: its reason for the command
        // put as a question, as Codex tells its model to.
        let codex = "• Running touch /var/tmp/kig/work/codex-approved\n\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: Do you want to mark the test approved?\n\n  $ touch /var/tmp/kig/work/codex-approved\n\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `touch /var/tmp/kig/\n     work/codex-approved` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        assert_eq!(approvable(codex).map(|a| a.question), Some("Would you like to run the following command?".into()));
        assert_eq!(classify(Agent::Codex, codex), State::Blocked);
        assert!(approvable(" Edit file\n src/main.rs\n Do you want to make this edit to main.rs?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n").is_some());
        assert!(approvable("Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n  2. No (esc)\n").is_some());
        assert_ne!(approvable(&claude("1:50").replace("tail -15", "tail -16x")).map(|b| b.key), Some(a.key));
        // The person moved to No: theirs.
        assert!(approvable(" Do you want to proceed?\n   1. Yes\n ❯ 2. No\n").is_none());
        // A plan: theirs.
        assert!(approvable(" Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n").is_none());
        // Words in the conversation, not a dialog.
        assert!(approvable("● Do you want to proceed?\n\n╭────╮\n│ >  │\n╰────╯\n").is_none());
        // A choice that isn't yes or no.
        assert!(approvable(" Which database?\n ❯ 1. Postgres\n   2. SQLite\n").is_none());
        // Another question's answers under words that ask.
        assert!(approvable("Do you want to keep the old API?\n\n Which one should go?\n ❯ 1. Yes, the old\n   2. The new\n").is_none());
    }

    /// Keep It Going types only at the agent's own prompt (Claude Code
    /// 2.1.322's and Codex 0.167's as Terminal read them): not into a
    /// chooser, whose Return would choose.
    #[test]
    fn at_its_prompt() {
        let claude = "● Ran it.\n\n✻ Cogitated for 0s · done 7:59 PM\n                     ◐ medium · /effort\n────────────────────────────────\n❯\u{a0}\n────────────────────────────────\n  ⏸ manual mode on · ? for shortcuts\n";
        assert!(at_prompt(Agent::Claude, claude));
        assert!(at_prompt(Agent::Claude, "● Done: the tests pass.\n\n╭────────╮\n│ >      │\n╰────────╯\n  ? for shortcuts\n"));
        let auto_mode = "────────────────────────────────\n Make auto mode your default permission mode?\n\n   Auto mode lets Claude handle permission prompts automatically.\n\n   ❯ Yes, set auto mode as my default permission mode\n     No, keep manual mode\n";
        assert!(!at_prompt(Agent::Claude, auto_mode));
        let codex = "• Ran it.\n\n  Worked for 7s • 8:09 PM\n\n\n› Ask Codex to do anything\n\n  gpt-test default · /var/tmp/kig/work\n  ← for agents · ? for shortcuts\n";
        assert!(at_prompt(Agent::Codex, codex));
        assert!(!at_prompt(Agent::Codex, "  Would you like to run the following command?\n\n  $ touch x\n\n› 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n"));
    }

    #[test]
    fn states() {
        let working = "● Reading files\n\n✻ Pondering… (12s · ↓ 300 tokens · esc to interrupt)\n\n╭────╮\n│ >  │\n╰────╯\n";
        assert_eq!(classify(Agent::Claude, working), State::Working);
        let asking = " Bash command\n   rm -rf build\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n";
        assert_eq!(classify(Agent::Claude, asking), State::Blocked);
        let idle = "● Done: the tests pass.\n\n╭────────╮\n│ >      │\n╰────────╯\n  ? for shortcuts\n";
        assert_eq!(classify(Agent::Claude, idle), State::Idle);
        // (Codex 0.172 at work, as it draws it: its status line over its field, the field's two lines under.)
        let codex_field = |held: &str| {
            format!("• Explored\n  └ Read review.md\n\n• Working (19m 43s • esc to interrupt)\n\n› {held}\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n")
        };
        assert_eq!(classify(Agent::Codex, &codex_field("Ask Codex to do anything")), State::Working);
        // A long message waiting in its field (nine lines of it) pushes the status line out of the
        // screen's last six lines: still at work.
        let long = (1..=9).map(|i| format!("line {i} of what the lead sent while it works")).collect::<Vec<_>>().join("\n  ");
        assert_eq!(classify(Agent::Codex, &codex_field(&long)), State::Working, "a long message in its field");
        // At rest with the same field: no status line over it. And words that only mention the line,
        // in what it wrote, are not the line.
        let rested = codex_field(&long).replace("• Working (19m 43s • esc to interrupt)\n", "");
        assert_eq!(classify(Agent::Codex, &rested), State::Idle);
        let mentions = rested.replace("  └ Read review.md", "  └ the service looks for \"esc to interrupt\" in the last six lines; Working (1m • esc to interrupt) is the line");
        assert_eq!(classify(Agent::Codex, &mentions), State::Idle, "its own words about the line");
        // (A code review's cases, 2026-11-27.) A message in its field that begins like a choice is still
        // its field, and the status line over it is seen; so is one with a quoted mark in it.
        assert_eq!(classify(Agent::Codex, &codex_field(&format!("2. Verify the result\n  {long}"))), State::Working, "a numbered message in its field");
        assert_eq!(classify(Agent::Codex, &codex_field(&format!("{long}\n  › as it wrote\n  and more"))), State::Working, "a mark quoted in its field");
        // An answer that speaks of the hint, in a bullet's line, over a long message in its field: at rest.
        // And the hint's words typed into its field: at rest.
        let speaks = rested.replace("  └ Read review.md", "  └ Read review.md\n\n• The hint reads (esc to interrupt)\n• Worked for 7s");
        assert_eq!(classify(Agent::Codex, &speaks), State::Idle, "an answer that speaks of the hint");
        let short = "• Done.\n\n› tell me what (esc to interrupt) means\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n";
        assert_eq!(classify(Agent::Codex, short), State::Idle, "the hint's words in its field");
        // Its status line with more after its clock (a count of what it runs) is its status line.
        assert_eq!(classify(Agent::Codex, &codex_field("Ask Codex to do anything").replace("esc to interrupt)", "esc to interrupt) · 1 background terminal running")), State::Working);
        // A status line of a turn before, over a rule drawn since: not this turn's.
        let before = rested.replace("  └ Read review.md", "• Working (2s • esc to interrupt)\n──────────────────────\n• Done.");
        assert_eq!(classify(Agent::Codex, &before), State::Idle, "a status line above a rule");
        // Codex 0.172's own screens, as a second investigator took them from the real program (2026-11-27).
        // A turn over, with what was sent while it worked shown above as it was sent: at rest, whatever
        // status line of the turn before stands further up.
        let over = "• Working (9s • esc to interrupt)\n\n• First answer.\n\n  Worked for 16s • 1:04 PM\n\n› QUEUED_LINE_1: Please also check this after finishing.\n  QUEUED_LINE_2: Second line of queued note.\n  QUEUED_LINE_3: Third line.\n\n• QUEUED_FOLLOWUP_ANSWERED.\n\n  Worked for <1s • 1:04 PM\n\n› Ask Codex to do anything\n\n  gpt-5.4-codex high · /var/tmp/work · Run the full phased turn.\n  ← for agents · ? for shortcuts                    ⚠ 1 warning · f2 to view\n";
        assert_eq!(classify(Agent::Codex, over), State::Idle, "a turn over");
        // While it runs a command: its own words before its clock, and a count of what it runs after.
        let running = "• I will check the diff first.\n\n• Checking the diff first, then running the verification command. (6s • esc to interrupt) · 1 background terminal runni…\n\n› Ask Codex to do anything\n\n  gpt-5.4-codex high · /var/tmp/work · Run the full phased turn.\n  ← for agents · ? for shortcuts\n";
        assert_eq!(classify(Agent::Codex, running), State::Working, "its status line as it runs a command");
        // While its answer comes, it draws no status line at all: by its screen it is at its prompt.
        // (What says its turn is on then is its conversation's file: `olo_rex_link::agents::state::turn_on`.)
        let streaming = "› Run the full phased turn.\n\n• Line 1 of commentary streaming slowly.\n  Line 2 of commentary streaming slowly.\n\n› Ask Codex to do anything\n\n  gpt-5.4-codex high · /var/tmp/work · Run the full phased turn.\n  ← for agents · ? for shortcuts\n";
        assert_eq!(classify(Agent::Codex, streaming), State::Idle, "an answer coming: no word of it on its screen");
        // (The confirming review's, 2026-11-27.) Codex's own mark begins no status line: typed at a
        // finished Codex, the line's very words are the person's.
        let typed = "• Done.\n\n  Worked for 3s • 1:04 PM\n\n› Explain Working (4s • esc to interrupt)\n\n  gpt-5.4-codex high · /var/tmp/work\n  ← for agents · ? for shortcuts\n";
        assert_eq!(classify(Agent::Codex, typed), State::Idle, "the status line's words typed into its field");
        // The queue's hint is its footer, not a line of a message in the field.
        let hint_typed = "• Done.\n\n  Worked for 3s • 1:04 PM\n\n› What does this mean:\n  tab to queue message\n\n  gpt-5.4-codex high · /var/tmp/work\n  ← for agents · ? for shortcuts\n";
        assert_eq!(classify(Agent::Codex, hint_typed), State::Idle, "the queue's hint typed into its field");
        let hint = "• Explored\n\n• Working (9s • esc to interrupt)\n\n› LINE_1: follow-up\n  LINE_2\n  LINE_3\n  LINE_4\n  LINE_5\n  LINE_6\n  LINE_7\n  LINE_8\n  LINE_9\n  LINE_10\n  LINE_11\n  LINE_12\n  LINE_13\n  LINE_14\n  LINE_15\n  LINE_16\n  LINE_17\n\n  gpt-5.4-codex high · /var/tmp/work\n  tab to queue message\n";
        assert_eq!(classify(Agent::Codex, hint), State::Working, "its footer while something waits to be queued");
        // A message in its field that begins like two choices is its field still (no chooser's words under it).
        assert_eq!(classify(Agent::Codex, &codex_field(&format!("2. Verify the result\n  3. Repeat\n  {long}"))), State::Working, "two numbered lines in its field");
        // A list of choices with its mark on one is no field, whatever stands over it.
        let chooser = "• Working (2s • esc to interrupt)\n\n  Would you like to run the following command?\n\n  $ touch x\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again (a)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        assert_ne!(classify(Agent::Codex, chooser), State::Working, "a question under a status line of before");
        let codex = "• Working (4s • esc to interrupt)\n▌ \n";
        assert_eq!(classify(Agent::Codex, codex), State::Working);
        let codex_ask = "Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n";
        assert_eq!(classify(Agent::Codex, codex_ask), State::Blocked);
    }

    /// Claude Code 2.1.331's screens (2026-11-24, from a run of it on a
    /// specimen task, its rules shortened; nothing of the account in them):
    /// asking leave for a command, its third answer one that would switch
    /// the person's mode (never Auto Approve's: Return is the first); at its
    /// prompt, its mode in the footer; at work, its field shown under what
    /// it does; and asking whether to trust a folder (the person's alone).
    #[test]
    fn claude_code_2_1_331() {
        let rule = "────────────────────────────────────────";
        let dashed = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";
        let asking = format!(
            "❯ Run exactly this one command and nothing else: cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n  Copying a.txt to b.txt\n  ⎿  $ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{rule}\n Bash command\n Tip: auto mode handles these prompts for you — choose \"switch to auto mode\" below\n Copy a.txt to b.txt\n{dashed}\n │ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{dashed}\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and always allow access to /var/tmp/s from this project\n   3. Yes, and switch to auto mode · auto mode handles these prompts for you\n   4. No\n Esc to cancel · Tab to amend\n"
        );
        assert_eq!(classify(Agent::Claude, &asking), State::Blocked);
        assert_eq!(approvable(&asking).map(|a| a.question), Some("Do you want to proceed?".into()));
        let idle = format!("  Ran 1 shell command\n● DONE\n✻ Crunched for 2s · done 1:47 PM\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · ? for shortcuts\n");
        assert_eq!(classify(Agent::Claude, &idle), State::Idle);
        assert!(at_prompt(Agent::Claude, &idle));
        assert_eq!(approvable(&idle), None);
        let working = format!("❯ Run exactly this one command\n  Copying a.txt to b.txt\n✻ Copying… (3s)\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · esc to interrupt · ← for agents\n");
        assert_eq!(classify(Agent::Claude, &working), State::Working);
        let trust = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/s\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team).\n Claude Code'll be able to read, edit, and execute files here.\n Security guide\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        assert_eq!(classify(Agent::Claude, &trust), State::Blocked);
        assert_eq!(approvable(&trust), None, "the person's");
        assert!(!at_prompt(Agent::Claude, &trust));
    }

    /// Screens captured from the agents themselves, read as Olo reads them:
    /// each file in `OLO_SCREENS` (`:` between), named for its agent
    /// (`agy-…`, `claude-…`, `codex-…`), its state, whether Keep It Going
    /// would type there and what Auto Approve would answer. For checking
    /// what an agent's update draws against this (`tools/dev/agents peek
    /// NAME 40 > agy-asking.txt` of a specimen asked to run a command it
    /// needs leave for): `OLO_SCREENS=… cargo test -p olo-agent-screen
    /// read_screens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn read_screens() {
        let Some(files) = std::env::var_os("OLO_SCREENS") else { return };
        for file in std::env::split_paths(&files) {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let agent = if name.starts_with("agy") {
                Agent::Antigravity
            } else if name.starts_with("codex") {
                Agent::Codex
            } else {
                Agent::Claude
            };
            let screen = std::fs::read_to_string(&file).unwrap();
            println!("{name}: {:?}; Keep It Going would type: {}; Auto Approve would answer: {:?}", classify(agent, &screen), at_prompt(agent, &screen), approvable(&screen).map(|a| a.question));
        }
    }

    /// Antigravity's screens (agy 1.5.15, recorded from one run of it;
    /// its banner, which names an account, left out): at its prompt,
    /// working, and asking leave to run a command.
    const AGY_RULE: &str = "──────────────────────────────────────────────────────────────────────";
    fn agy(under: &str) -> String {
        format!("> Do what the brief says.\n● Read(/var/tmp/x/brief.md) (ctrl+o to expand)\n▸ Thought for 8s, 541 tokens\n● Bash(cd /var/tmp/x && touch one.txt)\n{under}")
    }
    /// An Antigravity box whose "always allow" choices quote the command
    /// they would allow, a `python3 -c '` and a script: the quote keeps
    /// the script's line ends, so each of those choices is several lines
    /// on the screen, with a blank line where the script has one, or
    /// more lines than a wrapped label ever has. All four choices are
    /// read, the "No" among them. (The first is the pane builder's box of
    /// 2026-11-27 as it stood; with a blank line in the script it was
    /// read as two choices, and `olo agent approve --no` found no "No".)
    #[test]
    fn a_choice_that_quotes_a_command_of_several_lines_hides_no_choice() {
        // (`marked`: which answer the mark stands on, as it does after `approve` has moved it there.)
        let boxed = |quoted: &str, marked: usize| {
            let m = |n: usize| if n == marked { ">" } else { " " };
            agy(&format!(
                "● Bash(/usr/bin/python3 -c ')\nCommand\n{AGY_RULE}\nRequesting permission for:\n   /usr/bin/python3 -c '\n   import subprocess\n   pairs = [\n       (\"clip.rs\", [\"crates/screen-pane/src/clip.rs\"]),\n   ⋯ (19 lines hidden)\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation for commands that start with '/usr/bin/python3 -c '\n{quoted}'\n{} 3. Yes, and always allow for commands that start with '/usr/bin/python3 -c '\n{quoted}' (Persist to settings.json)\n{} 4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel                    accept-edits · Gemini 4 Neon · xhigh\n",
                m(1),
                m(2),
                m(3),
                m(4)
            ))
        };
        for (what, quoted) in [
            ("as it stood", "import subprocess\npairs = [\n    (\"clip.rs\", [\"crates/scree..."),
            ("a blank line in the script", "import subprocess\n\npairs = [\n    (\"clip.rs\", [\"crat..."),
            ("two blank lines", "import subprocess\n\n\npairs = [..."),
            ("more lines than a label wraps to", "a=1\nb=2\nc=3\nd=4\ne=5\nprint(a..."),
            // (A list in the script, numbered on from the answer it stands in, at the line's start:
            // not an answer, which stands at the answers' own column.)
            ("a numbered list in the script", "cat <<EOF\n\n3. Read the log\n4. Write it up\nEOF..."),
            ("a line of the script that begins as the mark does", "cat > out <<EOF\n> quoted\n\n> more\nEOF..."),
        ] {
            // Wherever the mark stands: on the first answer as the box comes, and on each other as
            // `approve` moves it before it presses Return (it reads the box again with the mark there;
            // read as no question then, nothing was pressed).
            for marked in 1..=4 {
                let ap = parse_approval(Agent::Antigravity, &boxed(quoted, marked)).unwrap_or_else(|| panic!("{what}, the mark on answer {marked}: no box read"));
                let kinds: Vec<ChoiceKind> = ap.choices.iter().map(|c| c.kind).collect();
                assert_eq!(
                    kinds,
                    [ChoiceKind::Once, ChoiceKind::AllowSession, ChoiceKind::DontAskAgain, ChoiceKind::No],
                    "{what}, the mark on answer {marked}: {:?}",
                    ap.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>()
                );
                assert_eq!((ap.selected_index, ap.choices[3].number, ap.choices[3].label.as_str()), (Some(marked - 1), Some(4), "No, cancel"), "{what}, the mark on answer {marked}");
                assert_eq!(ap.tool.as_deref(), Some("Bash"), "{what}");
                assert_eq!(ap.folded_lines, Some(19), "{what}: that the command is folded is said");
            }
        }
        // (What ends the choices still ends them: a blank line with no choice after it, then other words,
        // numbered or not.)
        for after in ["  3 tests ran before this.", "3. tests ran before this."] {
            let ended = agy(&format!(
                "● Bash(cargo test)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cargo test\nRun this command?\n> 1. Yes, run command\n  2. No, cancel\n\n{after}\n  ↑/↓ Navigate\nesc to cancel\n"
            ));
            let ap = parse_approval(Agent::Antigravity, &ended).expect("a box of two");
            assert_eq!(ap.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Yes, run command", "No, cancel"], "{after}");
        }
        // An answer's number seen twice at the answers' own column: which lines are answers can't be told.
        // Not read as a question at all (half its answers, with no "No" among them, was worse).
        let twice = agy(&format!(
            "● Bash(sh -c ')\nCommand\n{AGY_RULE}\nRequesting permission for:\n   sh -c '…'\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'sh -c '\n\n  3. a line of the script, indented as the answers are\n...'\n  3. Yes, and always allow for commands that start with 'sh' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate\nesc to cancel\n"
        ));
        assert!(parse_approval(Agent::Antigravity, &twice).is_none());
        // The same wherever the mark stands, and wherever the list is: a list an answer quotes, numbered
        // from 1 at the answers' own column, never begins the chooser. (With the mark moved to No, the
        // quoted "1." was taken for the first answer and the box read as that line and No: a code review,
        // 2026-11-27.) And a quoted line that carries the mark's own sign before a number ("> 3. …", a
        // blockquote) is a second mark: which is the cursor can't be told.
        for (what, quoted) in [("a list from 1 at the answers' column", "cat <<EOF\n  1. Gather\n  2. Sort\nEOF..."), ("a blockquote with a number", "cat <<EOF\n> 3. as they said\nEOF...")] {
            for marked in 1..=4 {
                assert!(parse_approval(Agent::Antigravity, &boxed(quoted, marked)).is_none(), "{what}, the mark on answer {marked}: read as a question");
            }
        }
        // A line of a quoted script that begins as a chooser's foot does ("Ctrl+G …") is not the foot:
        // that is the last such lines of the box. All four answers are read, wherever the mark stands
        // (it ended the chooser at answer 2: with the mark on 1 the "No" was lost, on "No" nothing read).
        for marked in 1..=4 {
            let ap = parse_approval(Agent::Antigravity, &boxed("echo keys\nCtrl+G opens the editor\nEsc to cancel it\necho done...", marked))
                .unwrap_or_else(|| panic!("a foot's words in the script, the mark on answer {marked}: no box read"));
            assert_eq!(ap.choices.iter().map(|c| c.kind).collect::<Vec<_>>(), [ChoiceKind::Once, ChoiceKind::AllowSession, ChoiceKind::DontAskAgain, ChoiceKind::No], "the mark on answer {marked}");
            assert_eq!((ap.selected_index, ap.choices[3].label.as_str()), (Some(marked - 1), "No, cancel"));
        }
    }

    /// A chooser an answer quotes is never the chooser: where the last
    /// question on the screen has an earlier one above it with a marked
    /// answer between them, the box is not read. (It was read as the
    /// quoted one, its quoted mark on its quoted "Yes", while the CLI's
    /// own cursor stood on an answer of the real question above: Return
    /// then takes that answer, whatever it is.) And a last answer
    /// wrapped over many lines is read whole to the foot.
    #[test]
    fn a_chooser_quoted_in_an_answer_is_not_the_chooser() {
        for marked in 1..=2 {
            let m = |n: usize| if n == marked { ">" } else { " " };
            let quoting = agy(&format!(
                "● Bash(cat <<EOF)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cat <<EOF …\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation for commands that start with 'cat <<EOF\nRun this command?\n> 1. Yes, run command\n  2. No, cancel\nEOF'\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel\n",
                m(1),
                m(2)
            ));
            assert!(parse_approval(Agent::Antigravity, &quoting).is_none(), "the real mark on answer {marked}: the quoted chooser was read");
        }
        // (The same box with nothing quoted is read.)
        let plain = agy(&format!("● Bash(ls)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   ls\nRun this command?\n> 1. Yes, run command\n  2. No, cancel\n  ↑/↓ Navigate\nesc to cancel\n"));
        assert_eq!(parse_approval(Agent::Antigravity, &plain).expect("a box of two").choices.len(), 2);
        // A last answer in a narrow pane, over five lines: whole, and so still the yes for this once.
        let narrow = agy(&format!(
            "● Read(a.png)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   a.png\nRun this command?\n> 1. No, cancel\n  2. Yes, but\n     ask\n     again\n     next\n     time\n  ↑/↓ Navigate\nesc to cancel\n"
        ));
        let ap = parse_approval(Agent::Antigravity, &narrow).expect("a box of two");
        assert_eq!((ap.choices[1].label.as_str(), ap.choices[1].kind), ("Yes, but ask again next time", ChoiceKind::Once));
    }

    /// What a choice does is read from the CLI's own words, never from
    /// what it quotes (a command, a path): words there moved a choice
    /// from "this session" to "kept for good" and back.
    #[test]
    fn what_a_choice_quotes_does_not_say_what_it_does() {
        use ChoiceKind::*;
        for (label, kind) in [
            // Antigravity's two, as they are.
            ("Yes, and always allow in this conversation for commands that start with 'rm one.txt'", AllowSession),
            ("Yes, and always allow for commands that start with 'rm one.txt' (Persist to settings.json)", DontAskAgain),
            // Its session choice, quoting a command that names the settings, or the other choice's own words.
            ("Yes, and always allow in this conversation for commands that start with 'cat settings.json'", AllowSession),
            ("Yes, and always allow in this conversation for commands that start with 'echo (Persist to settings.json)'", AllowSession),
            // A grant kept for good, quoting words of a session: still kept for good.
            ("Yes, and always allow for commands that start with 'echo in this conversation' (Persist to settings.json)", DontAskAgain),
            ("Yes, and don't ask again for `echo this session` commands", DontAskAgain),
            ("Yes, and don't ask again for commands that start with 'git status'", DontAskAgain),
            // Claude Code's folder, kept in the project's settings: its path is not in quotes, and says what it likes.
            ("Yes, and always allow access to tmp/ from this project", DontAskAgain),
            ("Yes, and always allow access to notes/for this session/ from this project", DontAskAgain),
            ("Yes, and always allow access to ~/this conversation/ from this project", DontAskAgain),
            // (Its words before the path's first slash, the folder's name having spaces.)
            ("Yes, and always allow access to this session notes/ from this project", DontAskAgain),
            // What it quotes has a quote of its own (`python3 -c '` and a script): the script's words are not the CLI's.
            ("Yes, and always allow in this conversation for commands that start with 'python3 -c ' import json print(open(\"settings.json\").read())'", AllowSession),
            // Only a key in brackets is a shortcut: a word there is the CLI's, and no yes for this once.
            ("Yes (always)", Other),
            ("Yes (y)", Once),
            ("Yes, allow all edits during this session (shift+tab)", SwitchMode),
            // A no that quotes a yes; a yes that quotes: not on the list of yeses for this once.
            ("No, and tell Codex what to do differently (esc)", No),
            ("Yes, run 'ls'", Other),
            ("Yes", Once),
            ("Yes, proceed (y)", Once),
            ("Yes, run command", Once),
        ] {
            assert_eq!(classify_choice_kind(label), kind, "{label}");
        }
        // (An apostrophe inside a word opens nothing.)
        assert_eq!(own_words("yes, and don't ask again for commands that start with 'git status'"), "yes, and don't ask again for commands that start with");
        assert_eq!(own_words("don't run 'a' or \"b c\" (ever)"), "don't run or (ever)");
        assert_eq!(own_words("an unclosed 'quote stays"), "an unclosed 'quote stays");
    }

    /// What reading a box costs (a pane's screen is read at each change
    /// of it): `jato test --release -p olo-agent-screen -- --ignored
    /// the_cost --nocapture`, the least of twenty rounds a box. A plain
    /// box of 16 lines; one of 216, whose two "always allow" answers
    /// each quote a script of a hundred lines. (2026-11-27, when every
    /// line under an answer stopped looking ahead for the next answer:
    /// the long box 809 µs before and 78 µs after, the plain one 13.8
    /// µs and 12.6 µs.)
    /// What telling a Codex at rest from one at work costs, on a screen of
    /// thirty lines read at each change of it (`jato test --release -p
    /// olo-agent-screen -- --ignored the_cost_of_telling --nocapture`),
    /// the same screen and the same classifier before and after the look
    /// over its field for its status line (2026-11-27): 32.7 µs a
    /// reading before (the hint's words looked for in the last six
    /// lines), 20.2 µs after, each the least of forty rounds in a build
    /// of its own: no cost that can be measured (the difference is the
    /// two builds').
    #[test]
    #[ignore]
    fn the_cost_of_telling_a_codex_at_rest() {
        let screen = format!(
            "{}\n› Ask Codex to do anything\n\n  GPT-6.2-Astra max · /var/tmp/x · Context 52% left\n  ← for agents · ? for shortcuts\n",
            (0..26).map(|i| format!("• Ran step {i} of the work, and said so at some length")).collect::<Vec<_>>().join("\n")
        );
        assert_eq!(classify(Agent::Codex, &screen), State::Idle);
        let least = (0..40)
            .map(|_| {
                let t = std::time::Instant::now();
                for _ in 0..500 {
                    std::hint::black_box(classify(Agent::Codex, std::hint::black_box(&screen)));
                }
                t.elapsed() / 500
            })
            .min()
            .unwrap();
        println!("Codex at rest ({} lines): {least:?} a reading", screen.lines().count());
    }

    #[test]
    #[ignore]
    fn the_cost_of_reading_a_box() {
        let plain = agy_asking();
        let script = (0..100).map(|i| format!("step_{i} = {i}")).collect::<Vec<_>>().join("\n");
        let long = agy(&format!(
            "● Bash(python3 -c ')\nCommand\n{AGY_RULE}\nRequesting permission for:\n   python3 -c '…'\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'python3 -c '\n{script}'\n  3. Yes, and always allow for commands that start with 'python3 -c '\n{script}' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend\nesc to cancel\n"
        ));
        for (what, screen, answers) in [("a plain box", &plain, None), ("two answers quoting a hundred lines each", &long, Some(4))] {
            let ap = parse_approval(Agent::Antigravity, screen).expect(what);
            if let Some(n) = answers {
                assert_eq!(ap.choices.len(), n, "{what}");
            }
            let least = (0..20)
                .map(|_| {
                    let t = std::time::Instant::now();
                    for _ in 0..200 {
                        std::hint::black_box(parse_approval(Agent::Antigravity, std::hint::black_box(screen)));
                    }
                    t.elapsed() / 200
                })
                .min()
                .unwrap();
            println!("{what} ({} lines): {least:?} a reading", screen.lines().count());
        }
    }

    /// That a command is folded is said by its own mark alone, in each
    /// of the forms it is drawn in.
    #[test]
    fn only_its_own_mark_says_a_command_is_folded() {
        // Words in a command are not the mark that says lines are folded: a command shown whole is whole.
        let whole = agy(&format!(
            "● Bash(python3 -c 'print(\"3 lines hidden\")')\nCommand\n{AGY_RULE}\nRequesting permission for:\n   python3 -c 'print(\"3 lines hidden\")'\nRun this command?\n> 1. Yes, run command\n  2. No, cancel\n  ↑/↓ Navigate\nesc to cancel\n"
        ));
        let ap = parse_approval(Agent::Antigravity, &whole).expect("a box of two");
        assert_eq!((ap.folded_lines, ap.command.as_deref()), (None, Some("python3 -c 'print(\"3 lines hidden\")'")));
        for (said, n) in [("⋯ 28 lines hidden", 28), ("⋯ (19 lines hidden)", 19), ("⋯ (1 line hidden)", 1)] {
            let folded = agy(&format!(
                "● Bash(cargo test)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cargo test --workspace\n   {said}\nRun this command?\n> 1. Yes, run command\n  2. No, cancel\n  ↑/↓ Navigate\nesc to cancel\n"
            ));
            assert_eq!(parse_approval(Agent::Antigravity, &folded).expect("a box of two").folded_lines, Some(n), "{said}");
        }
    }

    fn agy_idle() -> String {
        agy(&format!("  DONE\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"))
    }
    fn agy_working() -> String {
        agy(&format!("⣽  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\nesc to cancel\n"))
    }
    fn agy_asking() -> String {
        agy(&format!(
            "● Bash(cd /var/tmp/x && rm one.txt) (ctrl+o to expand)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cd /var/tmp/x && rm one.txt\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'rm one.txt'\n  3. Yes, and always allow for commands that start with 'rm one.txt' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel\n"
        ))
    }

    #[test]
    fn antigravity_is_told_by_its_program_and_read_by_its_own_words() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(identify("agy", "/home/u/.local/bin/agy", &args(&["agy"])), Some(Agent::Antigravity));
        assert_eq!(identify("agy", "/home/u/.local/bin/agy", &args(&["agy", "--model", "gemini-4-neon-high"])), Some(Agent::Antigravity));
        // Its words: "esc to cancel" under the field is its working (a
        // question's of Claude Code's), "↑/↓ Navigate" its asking.
        assert_eq!(classify(Agent::Antigravity, &agy_idle()), State::Idle);
        assert_eq!(classify(Agent::Antigravity, &agy_working()), State::Working);
        assert_eq!(classify(Agent::Antigravity, &agy_asking()), State::Blocked);
        // (Working with a message queued: no "esc to cancel", its spinner
        // and the queue's line say so; Keep It Going mustn't type then.)
        let queued = agy(&format!("⣯  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\nPress up to edit queued messages\n"));
        assert_eq!(classify(Agent::Antigravity, &queued), State::Working);
        assert!(!at_prompt(Agent::Antigravity, &queued));
        let spinner_only = agy(&format!("⠙  Generating...\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"));
        assert_eq!(classify(Agent::Antigravity, &spinner_only), State::Working);
        // (A screen given untrimmed, its blank lines after: still
        // working. Braille's blank, U+2800, is a space, no spinner.)
        assert_eq!(classify(Agent::Antigravity, &format!("{spinner_only}\n\n\n\n\n\n\n\n\n\n")), State::Working);
        let blank = agy(&format!("\u{2800}  notes\n{AGY_RULE}\n> Accept-edits mode: file edits auto-approved (shift+tab to cycle)\n{AGY_RULE}\n? for shortcuts\n"));
        assert_eq!(classify(Agent::Antigravity, &blank), State::Idle);
        // (Braille art far above, in a banner, isn't a spinner.)
        let banner = format!("⣿⣿⣿ agy\n{}\n\n\n\n\n\n\n\n{}", "⣿⣿⣿⣿", agy_idle());
        assert_eq!(classify(Agent::Antigravity, &banner), State::Idle);
        // Keep It Going types at its prompt, and nowhere else: not into
        // a question (whose chosen answer begins with ">" too), nor
        // while it works.
        assert!(at_prompt(Agent::Antigravity, &agy_idle()));
        assert!(!at_prompt(Agent::Antigravity, &agy_working()));
        assert!(!at_prompt(Agent::Antigravity, &agy_asking()));
        // Auto Approve: its question, with "1. Yes, run command" chosen
        // (Return: this once). Not with another answer chosen; and the
        // same question read twice is one question.
        let ask = approvable(&agy_asking()).expect("Antigravity's");
        assert_eq!(ask.question, "Run this command?");
        assert_eq!(approvable(&agy_asking()).map(|a| a.key), Some(ask.key));
        let moved = agy_asking().replace("> 1. Yes, run command\n  2. Yes, and", "  1. Yes, run command\n> 2. Yes, and");
        assert_eq!(approvable(&moved), None, "the chooser on an answer that always allows");
        assert_eq!(approvable(&agy_idle()), None);
        assert_eq!(approvable(&agy_working()), None);
        // (A line a person typed that ends in a question isn't one of
        // its questions: no first answer under it.)
        assert_eq!(approvable(&agy(&format!("{AGY_RULE}\n> shall we go on?\n{AGY_RULE}\n? for shortcuts\n"))), None);
    }

    /// What reading agy's screen costs (`perf stat` around this, release,
    /// for a change to `classify`: Rex reads each agent's screen after
    /// every pause in its output).
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn classify_cost() {
        let screens = [agy_idle(), agy_working(), agy_asking()];
        let mut working = 0;
        for i in 0..200_000 {
            if classify(Agent::Antigravity, std::hint::black_box(&screens[i % 3])) == State::Working {
                working += 1;
            }
        }
        assert_eq!(working, 66_667);
    }

    #[test]
    fn parses_folder_trust_structured_approvals_api_errors_and_usage() {
        let rule = "────────────────────────────────────────";
        let dashed = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";
        // 1. Folder trust prompt (No, exit preselected)
        let trust = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/worktree-1\n Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        let tp = parse_folder_trust(&trust).expect("folder trust detected");
        assert_eq!(tp.folder.as_deref(), Some("/var/tmp/worktree-1"));
        assert_eq!((tp.selected_index, tp.trust_index, tp.selected_trust), (0, 1, false));
        assert!(parse_approval(Agent::Claude, &trust).is_none(), "trust is never a tool approval");

        // 2. Claude Code structured approval with semantic choice kinds (even if cursor is on choice 2)
        let claude_ask = format!(
            "{rule}\n Bash command\n Copy a.txt\n{dashed}\n │ cp /var/tmp/s/a.txt /var/tmp/s/b.txt\n{dashed}\n Do you want to proceed?\n   1. Yes\n ❯ 2. Yes, and always allow access to /var/tmp/s from this project\n   3. Yes, and switch to auto mode\n   4. No\n Esc to cancel · Tab to amend\n"
        );
        let ap = parse_approval(Agent::Claude, &claude_ask).expect("claude approval");
        assert_eq!(ap.tool.as_deref(), Some("Bash"));
        assert_eq!(ap.command.as_deref(), Some("cp /var/tmp/s/a.txt /var/tmp/s/b.txt"));
        assert_eq!(ap.selected_index, Some(1));
        assert_eq!(ap.choices.len(), 4);
        assert_eq!(ap.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap.choices[1].kind, ChoiceKind::DontAskAgain);
        assert_eq!(ap.choices[2].kind, ChoiceKind::SwitchMode);
        assert_eq!(ap.choices[3].kind, ChoiceKind::No);

        // 3. Antigravity approval with folded lines
        let agy_ask = agy(&format!(
            "● Bash(cargo test) (ctrl+o to expand)\nCommand\n{AGY_RULE}\nRequesting permission for:\n   cargo test --workspace\n   ⋯ 28 lines hidden\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with 'cargo'\n  3. Yes, and always allow for commands that start with 'cargo' (Persist to settings.json)\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command\nesc to cancel\n"
        ));
        let ap_agy = parse_approval(Agent::Antigravity, &agy_ask).expect("agy approval");
        assert_eq!(ap_agy.tool.as_deref(), Some("Bash"));
        assert_eq!(ap_agy.folded_lines, Some(28));
        assert_eq!(ap_agy.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap_agy.choices[1].kind, ChoiceKind::AllowSession);
        assert_eq!(ap_agy.choices[2].kind, ChoiceKind::DontAskAgain);
        assert_eq!(ap_agy.choices[3].kind, ChoiceKind::No);

        // 4. API Error mid-response detection
        let api_err = format!("● Working on patch\n  ⎿  API Error: Connection lost mid-response\n{rule}\n❯ \n{rule}\n");
        assert_eq!(parse_api_error(Agent::Claude, &api_err).as_deref(), Some("API Error: Connection lost mid-response"));
        // Not while actively generating
        let api_err_working = format!("API Error: old\n✻ Pondering… (esc to interrupt)\n");
        assert!(parse_api_error(Agent::Claude, &api_err_working).is_none());

        // 5. Usage percentage & limit reached
        let usage_scr = format!("{rule}\n❯ \n{rule}\n  You've used 87% of your weekly limit · resets Fri\n");
        let u = parse_usage(&usage_scr).expect("usage parsed");
        assert_eq!(u.percent, Some(87));
        assert!(!u.limit_reached);
        let hit_scr = format!("{rule}\n❯ \n{rule}\n  You've hit your limit · resets 5pm\n");
        let uh = parse_usage(&hit_scr).expect("limit hit parsed");
        assert!(uh.limit_reached);
    }

    #[test]
    fn fuzz_approval_choice_classification_never_mislabels_persistent_as_once() {
        // (A small generator of its own: the same cases every run.)
        struct Rng(u64);
        impl Rng {
            fn next_u64(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.0 = x;
                x
            }
            fn range(&mut self, n: usize) -> usize {
                (self.next_u64() as usize) % n.max(1)
            }
            fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
                &slice[self.range(slice.len())]
            }
        }
        let mut rng = Rng(0x010_5AFE_A990_0001);

        // Real-world traps reported in production + adversarial variants across all 3 harnesses.
        let persistent_or_session_traps = [
            "Yes, and don\u{2019}t ask again for: env",
            "Yes, and don't ask again for: cargo test",
            "Yes, and don\u{2018}t ask again for commands in /home/user/repo",
            "Yes, and dont ask again for `git status`",
            "Yes, and do not ask again for this command",
            "Yes, and always allow in this conversation for commands that start with 'env'",
            "Yes, and always allow for commands that start with 'env' (Persist to settings.json)",
            "Yes, allow reading from /workspace/project during this session",
            "Yes, allow all edits during this session (shift+tab)",
            "Yes, switch to auto mode",
            "Yes, and remember this choice for future turns",
            "Yes, and trust all future runs of this tool",
            "Yes, and auto-accept similar commands",
            "Yes, always run without asking",
            "Yes, allow for this session",
            "Yes, and something totally new and unknown that grants extra scope",
        ];
        for trap in persistent_or_session_traps {
            let k = classify_choice_kind(trap);
            assert_ne!(k, ChoiceKind::Once, "SECURITY VIOLATION: persistent/unknown choice `{trap}` was classified as `Once`!");
        }

        // Valid single-use choices across Claude Code, Codex, and Antigravity.
        let valid_once = ["Yes", "Yes (y)", "Yes, proceed (y)", "Yes, run command", "Yes, run this command", "Yes, allow once", "Allow once"];
        for ok in valid_once {
            assert_eq!(classify_choice_kind(ok), ChoiceKind::Once, "valid single-use choice `{ok}` should be `Once`");
        }

        // Wrapped multi-line Claude/Codex/Agy approval dialogs across 250 randomized layouts.
        let prefixes = ["❯ ", "› ", "> "];
        let indents = ["  ", "   ", "    ", "\t"];
        let apostrophes = ["'", "\u{2019}", "\u{2018}", "`"];
        let cmds = ["env", "cargo test -p api && echo 'ok'", "rm -rf build && make", "git status --short"];
        for i in 0..250 {
            let is_agy = i % 3 == 2;
            let agent = match i % 3 {
                0 => Agent::Claude,
                1 => Agent::Codex,
                _ => Agent::Antigravity,
            };
            let mark = if is_agy { "> " } else { *rng.pick(&prefixes[..2]) };
            let apos = *rng.pick(&apostrophes);
            let indent = *rng.pick(&indents);
            let cmd = *rng.pick(&cmds);
            let selected_idx = rng.range(3);
            let c1 = if is_agy {
                "Yes, run command"
            } else if i % 3 == 1 {
                "Yes, proceed (y)"
            } else {
                "Yes"
            };
            // Deliberately wrap choice 2 onto a continuation line with 2..4 spaces!
            let c2_a = "Yes, and";
            let c2_b = format!("don{apos}t ask again for: {cmd}");
            let c3 = if is_agy { "No, cancel" } else { "No, and tell agent what to do differently (esc)" };

            let m = |idx: usize| if idx == selected_idx { mark } else { "  " };
            let footer = if is_agy {
                "  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command"
            } else if i % 3 == 1 {
                "  Press enter to confirm or esc to cancel"
            } else {
                " Esc to cancel · Tab to amend"
            };
            let question = if is_agy {
                "Run this command?"
            } else if i % 3 == 1 {
                "Would you like to run the following command?"
            } else {
                "Do you want to proceed?"
            };
            let body = if is_agy {
                format!("Command\n────────────────────\nRequesting permission for:\n   {cmd}\n{question}\n")
            } else if i % 3 == 1 {
                format!("{question}\n\n  $ {cmd}\n\n")
            } else {
                format!("Bash command\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n  {cmd}\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n{question}\n")
            };
            let screen = format!("{body}{}1. {c1}\n{}2. {c2_a}\n{indent}{c2_b}\n{}3. {c3}\n{footer}\n", m(0), m(1), m(2));
            let ap = parse_approval(agent, &screen).unwrap_or_else(|| panic!("case {i} failed to parse:\n{screen}"));
            assert_eq!(ap.selected_index, Some(selected_idx), "case {i} wrong selected_index");
            assert_eq!(ap.choices.len(), 3, "case {i} wrong choice count: {:?}", ap.choices);
            assert_eq!(ap.choices[0].kind, ChoiceKind::Once, "case {i} choice 0");
            assert_ne!(ap.choices[1].kind, ChoiceKind::Once, "case {i} choice 1 MUST NOT be Once: {:?}", ap.choices[1]);
            assert_eq!(ap.choices[1].kind, ChoiceKind::DontAskAgain, "case {i} choice 1 should be DontAskAgain: {:?}", ap.choices[1]);
            assert_eq!(ap.choices[2].kind, ChoiceKind::No, "case {i} choice 2");
            assert!(ap.command.as_deref().is_some_and(|c| c.contains(cmd)), "case {i} command missing: {:?}", ap.command);
        }

        // Folder-trust AND `--add-dir` ("Adding a new directory:") prompts must NEVER be misclassified as tool approvals.
        let add_dir_screen = "────────────────────\n Adding a new directory:\n /var/tmp/extra-dir\n This session hasn't worked here before. Is this a directory you created or one you trust?\n ❯ No, exit\n   Yes, add it\n Enter to confirm · Esc to cancel\n";
        let trust = parse_folder_trust(add_dir_screen).expect("must detect --add-dir trust prompt");
        assert_eq!(trust.folder.as_deref(), Some("/var/tmp/extra-dir"));
        assert_eq!(trust.selected_index, 0);
        assert_eq!(trust.trust_index, 1);
        assert!(!trust.selected_trust);
        assert!(parse_approval(Agent::Claude, add_dir_screen).is_none());
    }

    /// A folder to trust is answered with its yes (Ben, 2026-11-26: "i
    /// want folder trust auto approved. plans should not be though"),
    /// as each agent draws the question (Claude Code 2.1.332 and
    /// Antigravity read from the programs that day, each started in a
    /// new folder; Codex's from its own test above).
    #[test]
    fn auto_approve_trusts_a_folder_and_never_approves_a_plan() {
        let rule = "─".repeat(60);
        // Claude Code: the mark starts on "No, exit"; the yes is one down.
        let claude = format!(
            "{rule}\n Accessing workspace:\n /var/tmp/olo-agents-try/trust/claude-new\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open\n source project, or work from your team). If not, take a moment to review what's in this folder first.\n Claude Code'll be able to read, edit, and execute files here.\n Security guide\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel\n"
        );
        let ask = approvable_for(Agent::Claude, &claude).expect("a folder to trust is answered");
        assert_eq!((ask.down, ask.question.as_str()), (1, "Trust /var/tmp/olo-agents-try/trust/claude-new?"));
        assert_eq!(classify(Agent::Claude, &claude), State::Blocked);
        // The mark moved down to the yes: Return is right, and it is the same question still.
        let on_yes = claude.replace(" ❯ No, exit\n   Yes, I trust this folder", "   No, exit\n ❯ Yes, I trust this folder");
        let there = approvable_for(Agent::Claude, &on_yes).unwrap();
        assert_eq!((there.down, there.key), (0, ask.key));
        // No mark seen (a screen half drawn): nothing is known of where Return would land.
        let unmarked = claude.replace(" ❯ No, exit", "   No, exit");
        assert!(approvable_for(Agent::Claude, &unmarked).is_none());
        // Antigravity: the mark is on the yes from the start.
        let agy = "Accessing workspace:\n/var/tmp/olo-agents-try/trust/agy-new\nDo you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.\n> Yes, I trust this folder\n  No, exit\n  ↑/↓ Navigate · enter Confirm\n\n   auto · Gemini 4 Neon · xhigh\n";
        let ask = approvable_for(Agent::Antigravity, agy).expect("Antigravity's folder to trust");
        assert_eq!((ask.down, ask.question.as_str()), (0, "Trust /var/tmp/olo-agents-try/trust/agy-new?"));
        assert_eq!(classify(Agent::Antigravity, agy), State::Blocked);
        // Someone moved its mark to the no: theirs.
        let on_no = agy.replace("> Yes, I trust this folder\n  No, exit", "  Yes, I trust this folder\n> No, exit");
        assert!(approvable_for(Agent::Antigravity, &on_no).is_none());
        // Codex: "Yes, continue" under the mark.
        let codex = "> You are in /var/tmp/new\n\n  Do you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n\n› 1. Yes, continue\n  2. No, quit\n\n  Press enter to continue\n";
        assert_eq!(approvable_for(Agent::Codex, codex).map(|a| (a.down, a.question)), Some((0, "Trust /var/tmp/new?".to_owned())));
        // A folder added to a running Claude Code.
        let added = format!("{rule}\n Adding a new directory:\n /var/tmp/other\n Is this a directory you created or one you trust?\n ❯ 1. Yes, add it\n   2. No\n Enter to confirm · Esc to cancel\n");
        assert_eq!(approvable_for(Agent::Claude, &added).map(|a| (a.down, a.question)), Some((0, "Add and trust /var/tmp/other?".to_owned())));
        // A plan waiting on a mode is never answered, whatever is on.
        let plan = " Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n";
        assert!(approvable_for(Agent::Claude, plan).is_none());
    }

    /// Claude Code's read outside the working directories, as 2.1.332
    /// draws it (Ben's picture of a pane where Auto Approve never counted
    /// down, 2026-11-26): Auto Approve may answer it, with its yes for
    /// this once, three below the mark; and Return is right only once
    /// the mark stands there.
    #[test]
    fn auto_approve_answers_a_read_outside_with_its_yes_for_this_once() {
        let dotted = "┆".repeat(40);
        let dialog = |marked: usize| {
            let mark = |i: usize| if i == marked { "❯" } else { " " };
            format!(
                "● Reading the picture\n{rule}\n Read outside the working directories\n{dotted}\n Read(/var/tmp/olo-agents-try/shots/light-2x.png)\n{dotted}\n Auto mode and the sandbox read outside the working directories without asking. Yes or No\n answers for this read; the rest is for later reads. Block: the file tools refuse reads outside\n the working directories in every project. To undo, remove\n permissions.blockReadsOutsideWorkingDirectories from your user settings; file tools follow\n next session, sandboxed commands at once.\n\n Allow this read outside the working directories?\n {} 1. Yes, and keep allowing any reads outside the working directories\n {} 2. No, and block reads outside the working directories from now on\n {} 3. No, and ask again next time\n {} 4. Yes, but ask again next time\n\n Esc to cancel · Tab to amend\n",
                mark(0),
                mark(1),
                mark(2),
                mark(3),
                rule = "─".repeat(40),
            )
        };
        let asked = approvable(&dialog(0)).expect("a question Auto Approve may answer");
        assert_eq!(asked.question, "Allow this read outside the working directories?");
        assert_eq!(asked.down, 3, "its yes for this once is the fourth: never Return on the first, which is kept");
        // The mark moved there: Return now, and it is the same question.
        let there = approvable(&dialog(3)).expect("the same question");
        assert_eq!((there.down, there.key), (0, asked.key));
        // The mark anywhere else is somebody's doing: theirs.
        assert!(approvable(&dialog(1)).is_none());
        assert!(approvable(&dialog(2)).is_none());
        // And as the service reads it: one of each kind.
        let parsed = parse_approval(Agent::Claude, &dialog(0)).expect("read as a permission question");
        assert_eq!(parsed.choices.iter().map(|c| c.kind).collect::<Vec<_>>(), [ChoiceKind::DontAskAgain, ChoiceKind::NoAlways, ChoiceKind::No, ChoiceKind::Once]);
        assert_eq!(classify(Agent::Claude, &dialog(0)), State::Blocked);
        // The questions it always answered are answered as before: Return.
        let plain = "● Bash(touch one.txt)\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for this command\n   3. No, and tell Claude what to do differently (esc)\n";
        assert_eq!(approvable(plain).expect("as before").down, 0);
        // (Its mark moved off the first: the person's.)
        assert!(approvable(&plain.replace(" ❯ 1. Yes", "   1. Yes").replace("   2. Yes, and", " ❯ 2. Yes, and")).is_none());

        // Every agent's question, by the one reader (`approvable_for`).
        let for_claude = approvable_for(Agent::Claude, &dialog(0)).expect("Claude Code's read outside");
        assert_eq!((for_claude.down, for_claude.key), (3, asked.key), "the same question, the same key");
        assert_eq!(approvable_for(Agent::Claude, &dialog(3)).map(|a| a.down), Some(0));
        assert!(approvable_for(Agent::Claude, &dialog(2)).is_none(), "the mark on a no: the person's");
        assert_eq!(approvable_for(Agent::Claude, plain).map(|a| a.down), Some(0));
        let agy = |marked: usize| {
            let mark = |i: usize| if i == marked { ">" } else { " " };
            format!(
                "Command\n────────────────────\nRequesting permission for:\n   rm -f /var/tmp/x\nRun this command?\n{} 1. Yes, run command\n{} 2. Yes, and always allow in this conversation\n{} 3. No, cancel\n  ↑/↓ Navigate\n",
                mark(0),
                mark(1),
                mark(2)
            )
        };
        assert_eq!(approvable_for(Agent::Antigravity, &agy(0)).map(|a| a.down), Some(0));
        assert!(approvable_for(Agent::Antigravity, &agy(1)).is_none(), "moved to the yes that is kept for the conversation: the person's");
        let codex = "  Would you like to run the following command?\n\n  Reason: Do you want to allow listing the folder?\n\n  $ ls -la\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `ls` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        assert_eq!(approvable_for(Agent::Codex, codex).map(|a| a.down), Some(0));
        // A question with no yes for this once: the yes for this conversation, never the kept one.
        let session_only = "Command\n────────────────────\nRequesting permission for:\n   curl example.com\nRun this command?\n> 1. Yes, and always allow for commands that start with 'curl' (Persist to settings.json)\n  2. Yes, and always allow in this conversation\n  3. No, cancel\n  ↑/↓ Navigate\n";
        assert_eq!(approvable_for(Agent::Antigravity, session_only).map(|a| a.down), Some(1));
        // A plan to choose a mode for is never answered (a folder to trust is: its own test).
        let trust = "────────────────────\n Accessing workspace:\n /var/tmp/new\n Quick safety check: Is this a project you created or one you trust?\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n Enter to confirm · Esc to cancel\n";
        assert_eq!(approvable_for(Agent::Claude, trust).map(|a| a.down), Some(0));
        let plan = " Would you like to proceed?\n ❯ 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n";
        assert!(approvable_for(Agent::Claude, plan).is_none());
    }

    /// Claude Code's question about a read outside the working
    /// directories (2.1.33x; ShiftUp's launcher met it live, 2026-11-25):
    /// two yeses and two noes, one of each kept and one for this once.
    #[test]
    fn a_read_outside_has_one_yes_and_one_no_for_this_once() {
        let screen = " Read(/var/tmp/other/notes.md)\n Allow this read?\n \u{276f} 1. Yes, and keep allowing any reads outside the working directories\n   2. No, and block reads\n   3. No, and ask again next time\n   4. Yes, but ask again next time\n Esc to cancel\n";
        let kinds: Vec<ChoiceKind> = ["Yes, and keep allowing any reads outside the working directories", "No, and block reads", "No, and ask again next time", "Yes, but ask again next time"]
            .into_iter()
            .map(classify_choice_kind)
            .collect();
        assert_eq!(kinds, [ChoiceKind::DontAskAgain, ChoiceKind::NoAlways, ChoiceKind::No, ChoiceKind::Once]);
        let asked = parse_approval(Agent::Claude, screen).expect("the question is read");
        assert_eq!(asked.choices.iter().map(|c| c.kind).collect::<Vec<_>>(), kinds);
        assert_eq!(asked.selected_index, Some(0), "the mark stands on the one that is kept: Return alone would keep it");
        assert_eq!(asked.choices.iter().filter(|c| c.kind == ChoiceKind::Once).count(), 1);
        assert_eq!(asked.choices.iter().filter(|c| c.kind == ChoiceKind::No).count(), 1);
        // A kept no in other words; and a plain no is still a no.
        assert_eq!(classify_choice_kind("No, and don\u{2019}t ask again"), ChoiceKind::NoAlways);
        assert_eq!(classify_choice_kind("No, and tell Claude what to do differently (esc)"), ChoiceKind::No);
        assert_eq!(classify_choice_kind("No, cancel"), ChoiceKind::No);
        // Antigravity's yes for this conversation is neither once nor kept in its settings.
        assert_eq!(classify_choice_kind("Yes, and always allow in this conversation"), ChoiceKind::AllowSession);
        assert_eq!(classify_choice_kind("Yes, and always allow for commands that start with 'env' (Persist to settings.json)"), ChoiceKind::DontAskAgain);
    }

    #[test]
    fn a_feedback_survey_is_seen_and_a_message_that_quotes_it_is_not() {
        let rule = "─".repeat(40);
        // Claude Code's, as it draws it over its prompt (2.1.33x).
        let claude = format!("● Done.\n{rule}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n{rule}\n❯ \n{rule}");
        let seen = parse_survey(Agent::Claude, &claude).expect("Claude Code's survey");
        assert_eq!(seen.dismiss, "0");
        assert!(seen.question.starts_with("How is Claude doing"));
        // Antigravity's, drawn over a permission prompt's last lines.
        let agy =
            "● Bash(for i in 1 2 3; do cargo test …)\n How's the CLI experience so far? Help us improve:\n [1] Good  [2] Fine  [3] Bad  [0] Skip\nesc to cancel · accept-edits · Gemini 4 Neon · xhigh";
        assert_eq!(parse_survey(Agent::Antigravity, agy).expect("Antigravity's survey").dismiss, "0");
        // Each is its own agent's; Codex has none.
        assert!(parse_survey(Agent::Antigravity, &claude).is_none());
        assert!(parse_survey(Agent::Claude, agy).is_none());
        assert!(parse_survey(Agent::Codex, &claude).is_none());
        // A permission prompt isn't a survey, nor a working screen.
        assert!(parse_survey(Agent::Antigravity, "Run this command?\n> 1. Yes, run command\n  4. No, cancel\n  ↑/↓ Navigate").is_none());
        assert!(parse_survey(Agent::Claude, "✻ Working… (esc to interrupt)\n❯ ").is_none());
        // A message that quotes it in a sentence isn't one, nor one that
        // has only its choices, nor one scrolled far up the screen.
        assert!(parse_survey(Agent::Claude, "● The survey reads \"How is Claude doing this session?\" with 1: Bad … 0: Dismiss under it.\n❯ ").is_none());
        assert!(parse_survey(Agent::Claude, "● Ratings:\n  1: Bad    2: Fine   3: Good   0: Dismiss\n❯ ").is_none());
        let far = format!("{claude}\n{}", (0..14).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
        assert!(parse_survey(Agent::Claude, &far).is_none());
    }

    /// The words of a failure, where an agent that says it failed has them on its screen.
    #[test]
    fn a_failed_agents_screen_says_why() {
        let rule = "─".repeat(40);
        let failed = format!(
            "● Read(src/main.rs)\n  ⎿  API Error: 503 upstream connect error or disconnect/reset before headers · Retrying in 8 seconds… (attempt 10/10)\n● I was reading the parser when it stopped.\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n"
        );
        assert_eq!(error_line(&failed).as_deref(), Some("API Error: 503 upstream connect error or disconnect/reset before headers · Retrying in 8 seconds… (attempt 10/10)"));
        assert_eq!(error_line("● Done.\n❯ \n"), None);
        // An error of the turn before (over the person's next message) is not this turn's; nor is a line
        // that only speaks of one.
        let before = format!("  ⎿  API Error: 401 Unauthorized\n❯ try again\n● Reading the parser.\n{rule}\n❯ \n{rule}\n");
        assert_eq!(error_line(&before), None);
        let quoted = format!("❯ go on\n  ⎿  API Error: 529 Overloaded\n  ⎿  the fixture expects API Error: 401\n{rule}\n❯ \n{rule}\n");
        assert_eq!(error_line(&quoted).as_deref(), Some("API Error: 529 Overloaded"));
    }

    /// Claude Code's drafted feedback report (the ShiftUp lead's item 48):
    /// seen where its three keys are a line of their own at the foot, and
    /// nowhere else.
    #[test]
    fn a_drafted_feedback_report_is_seen_and_only_that() {
        let rule = "─".repeat(40);
        let boxed = format!("● Done.\n\n  1 to review · 2 to send · 0 to dismiss\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        assert_eq!(feedback_draft(Agent::Claude, &boxed).as_deref(), Some("1 to review · 2 to send · 0 to dismiss"));
        // Drawn with another divider, or in a frame: the same three keys.
        assert!(feedback_draft(Agent::Claude, &boxed.replace(" · ", "  |  ")).is_some());
        assert!(feedback_draft(Agent::Claude, &boxed.replace("  1 to review · 2 to send · 0 to dismiss", "│ 1 to review · 2 to send · 0 to dismiss │")).is_some());
        // While it shows the agent is not "at its prompt": nothing that types by that word types into it.
        assert!(!at_prompt(Agent::Claude, &boxed) && at_prompt(Agent::Claude, &boxed.replace("  1 to review · 2 to send · 0 to dismiss\n", "")));
        // The same words in what it wrote (a bullet's line; a line with more of its answer under it) are
        // not the box, though they stand near the foot.
        let printed = format!("● The box says:\n● 1 to review · 2 to send · 0 to dismiss\n✻ Crunched for 2s\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        assert!(feedback_draft(Agent::Claude, &printed).is_none() && at_prompt(Agent::Claude, &printed));
        let listed = format!("  1 to review · 2 to send · 0 to dismiss\n  and then it goes away.\n{rule}\n❯ \n{rule}\n");
        assert!(feedback_draft(Agent::Claude, &listed).is_none());
        // Over a field with several lines typed in it: still the box.
        let typed = format!("● Done.\n\n  1 to review · 2 to send · 0 to dismiss\n{rule}\n❯ a first line\n  a second\n  a third\n{rule}\n  ? for shortcuts\n");
        assert!(feedback_draft(Agent::Claude, &typed).is_some());
        // Words that quote it, a line with more on it, a line with a key missing: not the box.
        assert!(feedback_draft(Agent::Claude, &boxed.replace("  1 to review", "● The box says \"1 to review")).is_none());
        assert!(feedback_draft(Agent::Claude, &boxed.replace("0 to dismiss", "0 to dismiss, it says")).is_none());
        assert!(feedback_draft(Agent::Claude, &boxed.replace(" · 2 to send", "")).is_none());
        assert!(feedback_draft(Agent::Claude, &boxed.replace("2 to send", "2 to send it all, 3 to keep")).is_none());
        // Far above the foot (scrolled-by output): not the box. Another agent's screen: never.
        let above = format!("  1 to review · 2 to send · 0 to dismiss\n{}\n❯ \n", (0..14).map(|i| format!("● line {i}")).collect::<Vec<_>>().join("\n"));
        assert!(feedback_draft(Agent::Claude, &above).is_none());
        assert!(feedback_draft(Agent::Codex, &boxed).is_none());
        assert!(feedback_draft(Agent::Antigravity, &boxed).is_none());
        // And it is no survey: nothing sets it aside.
        assert!(parse_survey(Agent::Claude, &boxed).is_none());
    }

    #[test]
    fn audit_hardening_parsers_edge_cases() {
        // 1. Zero-width characters & NBSP inside "don't ask again" must NEVER be `Once`.
        assert_eq!(classify_choice_kind("Yes, and don\u{200B}\u{2019}t\u{00A0}ask again for: env"), ChoiceKind::DontAskAgain);
        assert_eq!(classify_choice_kind("Yes, and something brand new"), ChoiceKind::Other);

        // 2. Column-0 wrapped choice continuations (real `agy` narrow terminal wrapping!) & skipped numbers `1, 2, 4`.
        let agy_col0_wrap = "Command\n────────────────────\nRequesting permission for:\n   echo DENIED_CMD_agy\nRun this command?\n> 1. Yes, run command\n  2. Yes, and always allow in this conversation for commands that start with\n'echo DENIED_CMD_agy'\n  4. No, cancel\n  ↑/↓ Navigate · tab Amend\nesc to cancel\n";
        let ap = parse_approval(Agent::Antigravity, agy_col0_wrap).expect("col-0 wrapped agy approval");
        assert_eq!(ap.choices.len(), 3);
        assert_eq!(ap.choices[0].kind, ChoiceKind::Once);
        assert_eq!(ap.choices[1].kind, ChoiceKind::AllowSession);
        assert_eq!(ap.choices[2].kind, ChoiceKind::No);
        assert_eq!(ap.choices[2].number, Some(4));

        // 3. 2-line wrapped Claude edit question.
        let wrapped_q = "Edit file\n────────────────────\nDo you want to make this edit to\ncrates/rex-link/src/agent.rs?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n Esc to cancel\n";
        let ap_wq = parse_approval(Agent::Claude, wrapped_q).expect("2-line wrapped edit question");
        assert_eq!(ap_wq.question, "Do you want to make this edit to crates/rex-link/src/agent.rs?");
        assert_eq!(ap_wq.choices.len(), 3);

        // 4. Codex directory trust prompt detected as FolderTrustPrompt (not ApprovalPrompt, not Working).
        let codex_trust = "You are in /var/tmp/repo\n\nDo you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n\n› 1. Yes, continue\n  2. No, quit\n\nPress enter to continue\n";
        let ct = parse_folder_trust(codex_trust).expect("codex trust");
        assert_eq!(ct.folder.as_deref(), Some("/var/tmp/repo"));
        assert!(!ct.is_add_dir);
        assert!(ct.selected_trust);
        assert!(parse_approval(Agent::Codex, codex_trust).is_none());
        assert_eq!(classify(Agent::Codex, codex_trust), State::Blocked);

        // 5. Ordinary prose containing "Do you want to", "esc to cancel", "enter to confirm", "(y/n)"
        //    must NEVER falsely classify as Blocked or Working or parse_question!
        let claude_prose =
            "● Do you want to keep the old API? Use (y/n) or press Enter to confirm; esc to cancel.\n\n────────────────────────────────\n❯ \n────────────────────────────────\n  ? for shortcuts\n";
        assert_eq!(classify(Agent::Claude, claude_prose), State::Idle);
        assert_eq!(parse_question(Agent::Claude, claude_prose), None);

        let agy_prose = "● Docs mention esc to cancel and ↑/↓ Navigate in old examples.\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n────────────────────────────────\n> Accept-edits mode\n────────────────────────────────\n? for shortcuts\n";
        assert_eq!(classify(Agent::Antigravity, agy_prose), State::Idle);

        // 6. Stale Codex `■ Conversation interrupted` from a prior turn must NEVER poison a later healthy turn!
        let codex_stale_err = "■ Conversation interrupted - tell the model what to do differently.\n\n› Follow-up task\n\n• TURN2_FOLLOWUP_codex_OK\n\n  Worked for 1s\n\n› Ask Codex to do anything\n  gpt-5.4-codex · /var/tmp/work\n";
        assert_eq!(parse_api_error(Agent::Codex, codex_stale_err), None);
        // Active error at tail of current turn IS still detected!
        let codex_live_err = "› Run broken request\n■ stream disconnected before completion: 529 overloaded\n\n› Ask Codex to do anything\n  gpt-5.4-codex · /var/tmp/work\n";
        assert_eq!(parse_api_error(Agent::Codex, codex_live_err).as_deref(), Some("stream disconnected before completion: 529 overloaded"));

        // 7. `parse_usage` ignores ordinary output lines more than 6 lines above the bottom.
        let fake_usage = "Test output: used 30% of the session quota\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n";
        assert!(parse_usage(fake_usage).is_none());
    }
}
