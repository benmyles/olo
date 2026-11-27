//! The agents service of a host (`olo-rex-link agent serve`): one
//! program for the account's Rex server, started by the first `olo
//! agent` command (or by an Olo desktop as it connects) and ended when
//! nothing is left for it to do.
//!
//! It owns the children of the server's tabs. It starts each as a tab
//! of the server's (`session.new_window`), writes down whose child it
//! is (the registry, a file of the account's), reads what it shows from
//! the server itself, types into it, and closes it; and it keeps the
//! numbered log of what happens to each (`watch --since`). Nothing of
//! it is the Olo desktop's: with no desktop connected every command
//! works as with one, and a desktop that connects is told everything
//! (`follow`) and shows it.
//!
//! **At rest it does nothing.** It waits on its socket, on the server's
//! connection and on each live child's stream, and on no clock. A
//! child's output is read as a sign only (that something was written,
//! and how much): after a pause in it ([`QUIET`]; [`AT_MOST`] where it
//! never pauses) the child's screen is asked of the server once and
//! read (`olo_agent_screen`), and what changed of its state is written
//! down once it has held a quarter of a second ([`CONFIRM`]).
//!
//! **It can be killed at any moment.** The registry and the log are
//! files, each written whole or line by line; a new service reads them,
//! asks the server which tabs are still there, and goes on: a child
//! whose tab is gone has exited, one that is there is watched again. A
//! command whose service went away starts another and asks again.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// When, for the service's log: UTC, to the second ("2026-11-27
/// 01:45:06Z"), from seconds since 1970 (the days made a date by the
/// calendar's own arithmetic: no time library is here for one line).
pub(super) fn stamp_of(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// A line of the service's log (its standard error, which whoever
/// started it gave a file), begun with when: at a handover of four
/// askings in eighteen seconds (2026-11-27) the log said what had
/// happened, in order, and not when, and nobody could say how long
/// nothing had answered between two of them.
macro_rules! logged {
    ($($arg:tt)*) => {
        eprintln!("{} {}", stamp_of(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())), format_args!($($arg)*))
    };
}

use olo_rex_link::agents::state;
use olo_rex_link::agents::wire::{AgentView, Frame, PROTO, Paths, Request, Response};
use olo_rex_link::agents::{Activity, NativeSubagent, ScreenClassification, SubagentEvent, SubagentEventLog, SubagentLiveState, SubagentRegistry, classify_screen_rich, now_unix_ms, to_unix_ms};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc};

use super::rex::{Event, Rex, RexErr, Said};

/// The registry's name for this host: the service knows one.
pub const HOST: &str = "";
/// How long a child's output must pause before its screen is read.
pub const QUIET: Duration = Duration::from_millis(120);
/// And the longest that is waited for such a pause while it goes on
/// writing (a spinner beside a question never pauses).
pub const AT_MOST: Duration = Duration::from_millis(1000);
/// How long a state must hold before it is an event.
pub const CONFIRM: Duration = Duration::from_millis(250);
/// With no child alive and nobody asking, the service ends after this.
const IDLE_EXIT: Duration = Duration::from_secs(600);
/// The longest request taken (a prompt or a file sent whole).
const REQUEST_MOST: usize = 8 << 20;

/// A session of the server, as last seen.
#[derive(Default)]
pub struct Sess {
    pub windows: HashSet<String>,
    /// Each block's window.
    pub blocks: HashMap<String, String>,
    pub active: Option<String>,
    /// Whether nothing has been said to change since it was read.
    fresh: bool,
    /// Whether this connection follows it (its blocks' events, their
    /// streams).
    attached: bool,
}

/// A reading of a child's screen: what it showed, and what that means.
#[derive(Clone)]
pub struct Reading {
    out_seq: u64,
    activity: Activity,
    pub cls: ScreenClassification,
    pub screen: String,
    /// What the child had been given and not yet been seen at when this
    /// reading *began* (`Expected::cursor`): a reading takes up only
    /// that ([`Looked::expected`]).
    expected: Option<u64>,
}

/// What is kept of a child while it lives (nothing of it is written
/// down: a new service finds it out again).
pub struct Live {
    /// Counts what its program wrote: a reading is of one count.
    out_seq: u64,
    pub last_output_at: u64,
    pub last_state_change_at: u64,
    read: Option<Reading>,
    /// What its program says of itself (OSC 7501), by the record's name.
    statuses: Vec<(String, Activity, Option<String>)>,
    /// It has said what it is doing itself at some time (an OSC 7501
    /// record come, or gone): a program that speaks so says "idle" by
    /// taking its record away, and then has none. Whether it *ever*
    /// spoke is what tells a Codex whose own word is to be believed
    /// from one that gives none (`state::turn_on`): by the records it
    /// has now, a Codex at rest after it was killed and started again
    /// "gave none", and its rollout, left saying a task was started,
    /// made it "at work" for ten seconds more (the live suite under
    /// load, 2026-11-27: "resumed-after-kill Codex at rest must NEVER
    /// be stuck working").
    reported: bool,
    /// Counts what it has said of itself, each word as it came: a
    /// snapshot of its records asked for meanwhile is older than a word
    /// that came while it was asked (`learn`).
    status_seq: u64,
    /// Counts what it has been given ([`state::Expected::seq`]).
    given: u64,
    /// A rest of its is kept back and will be looked at again (one
    /// timer, not one for each look: `transition`'s confirmation).
    relook: bool,
    pub exit_code: Option<i32>,
    /// The terminal's own process (the shell the server started).
    pub pid: Option<i32>,
    rows: u16,
    pub last_state: Option<SubagentLiveState>,
    last_key: Option<String>,
    check_gen: u64,
    /// Seen at work since it started (or since it was last typed to):
    /// really at work, not the word for an empty tab.
    seen_working: bool,
    /// What it was given and has not yet been seen at work on
    /// (`state::Expected`): a rest meanwhile is the rest before that
    /// turn.
    expects: Option<olo_rex_link::agents::state::Expected>,
    /// The cursor of what the turn it is in, or last ended, answers.
    pub answers: Option<u64>,
    /// Its rest is kept back until then (`state::Rest::Kept`): read
    /// again at whatever it shows meanwhile, and when that time comes.
    held_until: Option<u64>,
    /// Asks its watcher to read the screen now.
    poke: Arc<Notify>,
    task: Option<tokio::task::AbortHandle>,
    /// What followers were last told of it.
    told: Option<(SubagentLiveState, Option<String>)>,
    /// A feedback survey on its screen wouldn't go when set aside: left
    /// alone until it has gone by itself.
    survey_stays: bool,
    /// Claude Code's offer to send a feedback report it drafted stands
    /// on its screen, and whoever looks after the child has been told
    /// of this showing of it (`olo_agent_screen::feedback_draft`).
    draft_told: bool,
    /// The approval box that stands (`state::box_hash`) and its number
    /// among this child's boxes; none while none stands or once it was
    /// answered here. See [`Service::box_key`].
    box_now: Option<(u64, u64)>,
    boxes: u64,
    /// Latest confirmed state event recorded for this live child, kept
    /// independently of the 2,000-event log ring so a standing question
    /// is never lost when other host events evict the ring.
    pub latest_event: Option<SubagentEvent>,
}

impl Live {
    fn new() -> Self {
        let now = now_unix_ms();
        Self {
            out_seq: 0,
            last_output_at: now,
            last_state_change_at: now,
            read: None,
            statuses: Vec::new(),
            reported: false,
            status_seq: 0,
            given: 0,
            relook: false,
            exit_code: None,
            pid: None,
            rows: 0,
            last_state: None,
            last_key: None,
            check_gen: 0,
            seen_working: false,
            expects: None,
            answers: None,
            held_until: None,
            poke: Arc::new(Notify::new()),
            task: None,
            told: None,
            survey_stays: false,
            draft_told: false,
            box_now: None,
            boxes: 0,
            latest_event: None,
        }
    }

    fn activity(&self) -> (Activity, Option<String>) {
        self.statuses.iter().max_by_key(|(_, a, _)| a.rank()).map(|(_, a, msg)| (*a, msg.clone())).unwrap_or_default()
    }
}

/// What a minder was last told of an agent's standing question or
/// approval: the exact box instance key (`bN-hhhhhhhh`) for approvals,
/// or question key (`q:...` / `tx:waiting`) for non-approval questions,
/// bound to the agent's `rules_gen` when it was delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToldGrant {
    pub token: String,
    pub is_approval: bool,
    pub rules_gen: u64,
}

pub struct State {
    pub reg: SubagentRegistry,
    pub log: SubagentEventLog,
    pub live: HashMap<String, Live>,
    pub sessions: HashMap<String, Sess>,
    /// Connections being answered now.
    clients: usize,
    /// Of them, those asking something that is done once and isn't a
    /// wait (not a `watch`, not a `follow`): what the service finishes
    /// before it ends.
    doing: usize,
    /// Since when nothing has been asked and no child lives.
    idle_since: Option<Instant>,
    /// Minders stopped by the one who may stop them (its agent's parent,
    /// or the service once its agent has ended), scoped strictly to the
    /// active `stop_target_list` call and removed when it finishes:
    /// `None` suppresses notices for all workers (empty/detached teardown);
    /// `Some(workers)` suppresses notices only for workers being stopped
    /// alongside this minder, never for unexpected survivors.
    pub let_go: HashMap<String, Option<HashSet<String>>>,
    /// Shared minders atomically marked as closing (empty-timer expiry,
    /// failed-start cleanup, or ancestor `--tree` stop): excluded from
    /// minder selection and assignment commit.
    pub closing_minders: HashSet<String>,
    /// Shared minders that hit a confirmed `Error` state (screen or
    /// transcript): excluded from reuse until confirmed recovery.
    pub unhealthy_minders: HashSet<String>,
    /// Empty-period generation per shared minder: bumped whenever a
    /// minder gains an assignment/reservation or becomes empty, so an
    /// earlier empty timer never shortens a later grace period.
    pub empty_gen: HashMap<String, u64>,
    /// Pre-allocated child IDs currently mid-start (before registration).
    pub reserved_ids: HashSet<String>,
    /// Reserved live names `(session_id, asker_win, lowercase_name)`
    /// while a `start` is in flight so concurrent starts and auto-named
    /// minders never collide.
    pub pending_names: HashSet<(String, String, String)>,
    /// By `(minder_id, agent_id)`: the exact box instance or question of
    /// that agent this minder was last told of (a watch's answer, a
    /// status), bound to `rules_gen`. A minder answers that question and
    /// no other, and never under stale rules.
    pub told: HashMap<(String, String), ToldGrant>,
    /// By `agent_id`: monotonically bumped on every rules update or
    /// ownership change so decisions made under earlier rules are
    /// invalidated immediately (including in-flight approvals).
    pub rules_gen: HashMap<String, u64>,
    /// By `(minder_id, agent_id)`: required `(rules_gen, rules_path)` whose
    /// `minding` (`rules`) revision has not yet been delivered to the minder.
    /// Neither `status` nor an approval-only `watch` may clear this or mint a
    /// `ToldGrant` until the minder actually receives the `minding` revision
    /// (synthesised if the 2,000-event log ring evicted the original event).
    pub pending_rules_rev: HashMap<(String, String), (u64, String)>,
    /// By `(minder_id, agent_id)`: the last state event of `agent_id`
    /// that this minder has been told (its number in the log). What
    /// stands and is later than that, it is told; nothing twice.
    pub seen: HashMap<(String, String), u64>,
    /// By `(minder_id, agent_id)`: the last `minding` roster/rules event
    /// about `agent_id` delivered to `minder_id`. Keyed per subject so a
    /// targeted watch on B never swallows an unread rules event for A.
    pub minding_seen: HashMap<(String, String), u64>,
    /// By `minder_id`: exited agents whose exit has not yet been reported
    /// via `escalate`. Early shutdown after a report waits until all
    /// pending exits for this minder have been reported.
    pub pending_exit_reports: HashMap<String, HashSet<String>>,
    /// By shared `minder_id`: number of agents currently mid-launch
    /// under this minder (so concurrent `start --minder` bursts honor
    /// [`minds_most`] without races).
    pub pending_minds: HashMap<String, usize>,
    /// By child: its list of what is not allowed without asking
    /// (`leave::LeaveList`), where its parent gave one: everything else
    /// it asks leave for, the service itself allows, once. None kept
    /// for a child that has none (looked for once, in its folder).
    pub leave: HashMap<String, Option<Arc<super::leave::LeaveList>>>,
}

/// How long a minder has, once the agent it minded has ended, to tell
/// the parent what it did before it is stopped (its report ends it
/// sooner: `escalate`).
pub const MINDER_REPORTS: Duration = Duration::from_secs(180);

/// How long an agent at rest has to take up a message sent to it
/// before its rest is told all the same (thirty seconds; a test's own
/// time for it: `OLO_REX_AGENTS_TAKEN_UP_MS`).
pub fn taken_up() -> Duration {
    std::env::var("OLO_REX_AGENTS_TAKEN_UP_MS").ok().and_then(|v| v.parse().ok()).map_or(Duration::from_secs(30), Duration::from_millis)
}

/// [`MINDER_REPORTS`], or what a test of it set in its place
/// (`OLO_REX_AGENTS_MINDER_REPORTS_MS`).
pub fn minder_reports() -> Duration {
    std::env::var("OLO_REX_AGENTS_MINDER_REPORTS_MS").ok().and_then(|v| v.parse().ok()).map_or(MINDER_REPORTS, Duration::from_millis)
}

/// How long a conversation file's word that a turn is on is believed
/// of an agent that shows no life (`state::TURN_ON_MS`; a test's own:
/// `OLO_REX_AGENTS_TURN_ON_MS`).
fn turn_on_ms() -> u64 {
    std::env::var("OLO_REX_AGENTS_TURN_ON_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(state::TURN_ON_MS)
}

/// When a child's conversation file was last written (0: not known).
fn transcript_written_ms(entry: &NativeSubagent, info: Option<&Value>) -> u64 {
    info.and_then(|v| v["transcript_path"].as_str())
        .or(entry.transcript_path.as_deref())
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// How a child stands, by its screen (`cls`) settled with its
/// conversation's file (`info`: `effective_state`), and, where the screen
/// reads as at rest, by whether the file says a turn is on all the same
/// (`state::turn_on`: then it is at work). `output_ms`: when its screen
/// was last written. One rule for `list`, `status`, `watch` and the
/// events, so that none calls at rest what another calls at work.
pub(super) fn standing(
    entry: &NativeSubagent,
    cls: &ScreenClassification,
    info: Option<&Value>,
    alive: bool,
    (output_ms, reports, expects): (u64, bool, Option<state::Expected>),
) -> (SubagentLiveState, Option<String>, bool) {
    let turn_ms = info.and_then(|v| v["turn_started_ms"].as_u64());
    // (What the file says of a turn before the one it was given is not
    // of this turn: its end, an error it ended with. `of_the_turn_given`.)
    let its_turn = state::of_the_turn_given(turn_ms, expects);
    let tx = info.and_then(|v| v["transcript_state"].as_str()).filter(|_| its_turn);
    let tx_error = info.and_then(|v| v["transcript_error"].as_str()).filter(|_| its_turn);
    let (state, error, aborted) = olo_rex_link::agents::effective_state(cls, tx, tx_error, alive);
    // (The file's own times last, and only where they are asked for: a
    // file is looked up for nobody else.)
    if state == SubagentLiveState::Idle
        && !aborted
        && !reports
        && entry.cli == olo_rex_link::agents::AgentCli::Codex
        && tx == Some("working")
        && state::turn_on(entry.cli, tx, turn_ms, to_unix_ms(entry.started_at).max(0) as u64, transcript_written_ms(entry, info), output_ms, now_unix_ms(), turn_on_ms())
    {
        return (SubagentLiveState::Working, None, false);
    }
    (state, error, aborted)
}

/// Most live agents one shared minder minds before a second minder of
/// the same `CLI:MODEL:EFFORT` is started under the asking tab
/// (default 12; `OLO_REX_AGENTS_MINDS_MOST` for tests).
pub fn minds_most() -> usize {
    std::env::var("OLO_REX_AGENTS_MINDS_MOST").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n >= 1).unwrap_or(olo_rex_link::agents::minder::DEFAULT_MINDS_MOST)
}

/// Three-way tab presence returned by [`Service::tab_presence`].
pub(super) enum TabPresence {
    Present,
    Gone,
    Unknown(String),
}

/// A child as it stands, read now or as last read.
pub struct Looked {
    pub cls: ScreenClassification,
    pub screen: String,
    pub last_output_at: u64,
    pub last_state_change_at: u64,
    pub pid: Option<i32>,
    /// A feedback survey is drawn over it and wouldn't be set aside:
    /// what is read may be covered by it.
    pub survey: bool,
    /// What the child had been given and not yet been seen at when the
    /// reading began. A turn seen on in this reading takes up that and
    /// nothing given since: a reading begun while turn A was on, with a
    /// message B sent and noted as it ended, took B for begun, and A's
    /// rest and last words were then B's answer (a code review,
    /// 2026-11-27; the taking-up in `look` came with the status that
    /// reads a screen first, d796809c2).
    pub expected: Option<u64>,
    /// Whether its program says what it is doing itself (an OSC 7501
    /// record of its own). Where a Codex does, that is its turn's word
    /// and its conversation's file is not asked (`state::turn_on`).
    pub reports: bool,
    /// What it has been given and not yet been seen at, as that stands
    /// now (`state::of_the_turn_given`).
    pub expects: Option<state::Expected>,
}

pub struct Service {
    pub rex: Arc<Rex>,
    pub paths: Paths,
    st: Mutex<State>,
    /// Something a waiter may be waiting for happened: an event written,
    /// a question come or gone, an exit.
    pub changed: Notify,
    frames: broadcast::Sender<Frame>,
    /// `watch --match` and `--regex` in progress: only then is every
    /// reading of a screen a reason to wake the watchers.
    pub text_watchers: AtomicUsize,
    /// The registry or the log has something to write.
    save: Notify,
    /// The service has become idle, or stopped being so.
    idle: Notify,
    /// Set to end the service once what it has is written.
    retire: Notify,
    /// The helper whose service takes over when this one retires
    /// (`retire --to`, or the later build that asked): started by this
    /// service as it ends, with this service's lock ([`start_successor`]).
    successor: std::sync::Mutex<Option<PathBuf>>,
    /// Said in the log once: a later build asked while the service was
    /// held.
    said_held: std::sync::atomic::AtomicBool,
    /// Serializes find-or-start of a shared minder so concurrent
    /// `start --minder` requests reuse one minder and honor
    /// [`minds_most`].
    pub minder_gate: tokio::sync::Mutex<()>,
    /// Per-worker async locks serializing `mind` mutations (`--off`,
    /// `--rules`, `--with`) so concurrent moves on one worker never
    /// interleave with stale ownership.
    worker_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn exited(summary: String) -> ScreenClassification {
    ScreenClassification { state: SubagentLiveState::Exited, summary: Some(summary), approval: None, folder_trust: None, api_error: None, usage: None, recognized_question: false, in_turn: false }
}

fn starting() -> ScreenClassification {
    ScreenClassification {
        state: SubagentLiveState::Working,
        summary: Some("starting".into()),
        approval: None,
        folder_trust: None,
        api_error: None,
        usage: None,
        recognized_question: false,
        in_turn: false,
    }
}

/// What tells two readings of one state apart: the question asked, the
/// error said.
pub fn subkey(cls: &ScreenClassification) -> String {
    if let Some(ap) = &cls.approval {
        // (The box to the letter, `state::box_hash`: by the screen's
        // own key, digits aside, a box that followed one whose command
        // differed by a digit was the same question, and told to nobody.)
        format!("ap:{}", state::box_hash(ap))
    } else if let Some(tr) = &cls.folder_trust {
        format!("trust:{}:{}", tr.folder.as_deref().unwrap_or(""), tr.selected_index)
    } else if let Some(err) = &cls.api_error {
        format!("err:{err}")
    } else if cls.state == SubagentLiveState::WaitingQuestion {
        format!("q:{}", cls.summary.as_deref().unwrap_or(""))
    } else {
        String::new()
    }
}

pub fn tail_of(screen: &str, lines: usize) -> Vec<String> {
    screen.lines().rev().filter(|l| !l.trim().is_empty()).take(lines).map(str::to_owned).collect::<Vec<_>>().into_iter().rev().collect()
}

/// Whether the process that asks runs another file of the helper than
/// this service does, written later (a build put on the host since this
/// one started). The same file, or an earlier one (a watcher from before
/// an upgrade, still waiting), is not: the service is never handed back.
fn asker_is_a_later_build(pid: i32) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // (The helper itself, by its file's name, as it is built and as it
    // is put on a host: `olo-rex-link`, `olo-rex-link-ID-ARCH`. Another
    // program that speaks to the socket is no build of this one, however
    // new its file: a test's own request ended the service it was
    // asking, 2026-11-26.)
    let Ok(theirs_at) = std::fs::read_link(format!("/proc/{pid}/exe")) else { return false };
    let name = theirs_at.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.trim_end_matches(" (deleted)").starts_with("olo-rex-link") {
        return false;
    }
    // (And put beside this one: an upgrade, in the folder a host's
    // helpers are kept in. A build somewhere else is not one: the debug
    // build of a source tree, asked through from a test session, was
    // handed the live service of a host with agents at work, 2026-11-26.)
    let Ok(mine_at) = std::fs::read_link("/proc/self/exe") else { return false };
    if mine_at.parent() != theirs_at.parent() {
        return false;
    }
    // (Through /proc's own links: a file taken off the disk since, as an
    // upgrade's tidying does, is still read.)
    let (Ok(mine), Ok(theirs)) = (std::fs::metadata("/proc/self/exe"), std::fs::metadata(format!("/proc/{pid}/exe"))) else { return false };
    (mine.dev(), mine.ino()) != (theirs.dev(), theirs.ino()) && (theirs.mtime(), theirs.mtime_nsec()) > (mine.mtime(), mine.mtime_nsec())
}

impl Service {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn worker_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.worker_locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id.to_owned()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
    }

    pub fn bump_empty_gen_locked(st: &mut State, minder_id: &str) -> u64 {
        let g = st.empty_gen.entry(minder_id.to_owned()).or_insert(0);
        *g = g.wrapping_add(1);
        *g
    }

    /// The account's hold on this host's own pieces:
    /// `~/.local/state/olo/host-upgrade.hold`, beside the folder the
    /// services' own are in. `tools/dev/host-upgrade` keeps to it, and
    /// so does the service: the lead of the agents at work here asked
    /// to be told before the service is replaced, and a desktop that
    /// upgraded and connected replaced it unasked twenty minutes before
    /// the time it had been given (2026-11-26, 20:25 UTC: its helper,
    /// put beside this one, was "a later build" and was handed the
    /// service).
    fn hold_file(&self) -> PathBuf {
        self.paths.dir.parent().and_then(|p| p.parent()).unwrap_or(&self.paths.dir).join("host-upgrade.hold")
    }

    /// The hold's own words, while it is there: this service is then
    /// handed to no later build and retired by no one who doesn't say
    /// they know of it. (A service that is running: one that has ended
    /// by itself, with no child alive and nothing asked, is started
    /// again by whichever build asks next.)
    fn held(&self) -> Option<String> {
        let words = std::fs::read_to_string(self.hold_file()).ok()?;
        Some(words.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(240).collect())
    }

    // ---- The server's sessions ----

    /// Reads a session's windows and blocks from the server.
    pub async fn refresh(&self, session: &str) -> Result<(), RexErr> {
        let view = self.rex.call("session.view", json!({ "session_id": session })).await?;
        let mut st = self.lock();
        let sess = st.sessions.entry(session.to_owned()).or_default();
        sess.windows.clear();
        sess.blocks.clear();
        sess.active = view["active_window_id"].as_str().map(str::to_owned);
        for w in view["windows"].as_array().into_iter().flatten() {
            let Some(wid) = w["window_id"].as_str() else { continue };
            sess.windows.insert(wid.to_owned());
            for layer in w["layers"].as_array().into_iter().flatten() {
                for b in layer["blocks"].as_array().into_iter().flatten() {
                    if let Some(bid) = b["block_id"].as_str() {
                        sess.blocks.insert(bid.to_owned(), wid.to_owned());
                    }
                }
            }
        }
        sess.fresh = true;
        Ok(())
    }

    /// The session as the server has it now (read again only where
    /// something was said to change).
    pub async fn seen(&self, session: &str) -> Result<(), RexErr> {
        if self.lock().sessions.get(session).is_some_and(|s| s.fresh) {
            return Ok(());
        }
        self.refresh(session).await
    }

    pub async fn session_ids(&self) -> Result<Vec<String>, RexErr> {
        let list = self.rex.call("session.list", Value::Null).await?;
        Ok(list["sessions"].as_array().into_iter().flatten().filter_map(|s| s["session_id"].as_str().map(str::to_owned)).collect())
    }

    /// The session a block is in, looked for in every session.
    pub async fn session_of_block(&self, block: &str) -> Option<String> {
        let found = |st: &State| st.sessions.iter().find(|(_, s)| s.fresh && s.blocks.contains_key(block)).map(|(id, _)| id.clone());
        if let Some(id) = found(&self.lock()) {
            return Some(id);
        }
        for id in self.session_ids().await.ok()? {
            let _ = self.seen(&id).await;
        }
        found(&self.lock())
    }

    /// Follows a session (its blocks' events, and leave to open their
    /// streams), once.
    async fn attach(&self, session: &str) -> Result<(), RexErr> {
        if self.lock().sessions.get(session).is_some_and(|s| s.attached) {
            return Ok(());
        }
        self.rex.call("session.attach", json!({ "session_id": session })).await?;
        self.lock().sessions.entry(session.to_owned()).or_default().attached = true;
        Ok(())
    }

    // ---- A child's screen ----

    /// How a child stands. `fresh`: asked of the server now, whatever
    /// was read before; otherwise the last reading where nothing was
    /// written since.
    pub async fn look(&self, entry: &NativeSubagent, fresh: bool) -> Looked {
        let (reading, out_seq, activity, status_msg, rows, expected) = {
            let st = self.lock();
            let alive = entry.exited_at.is_none() && st.reg.by_window(HOST, &entry.session_id, &entry.window_id).is_some();
            let Some(live) = st.live.get(&entry.id).filter(|_| alive) else {
                // (Its program ended and its tab stays, or the tab is gone.)
                let code = st.live.get(&entry.id).and_then(|l| l.exit_code).or(entry.exit_code);
                let cls = match code {
                    Some(code) => exited(format!("exited (code {code})")),
                    None => exited("tab closed".into()),
                };
                let (out, chg) = st.live.get(&entry.id).map(|l| (l.last_output_at, l.last_state_change_at)).unwrap_or((0, 0));
                return Looked { cls, screen: String::new(), last_output_at: out, last_state_change_at: chg, pid: entry.last_pid, survey: false, expected: None, reports: false, expects: None };
            };
            if let Some(code) = live.exit_code {
                return Looked {
                    cls: exited(format!("exited (code {code})")),
                    screen: String::new(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey: false,
                    expected: None,
                    reports: false,
                    expects: None,
                };
            }
            let (activity, status_msg) = live.activity();
            if !fresh && let Some(read) = live.read.as_ref().filter(|r| r.out_seq == live.out_seq && r.activity == activity) {
                return Looked {
                    cls: read.cls.clone(),
                    screen: read.screen.clone(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey: live.survey_stays,
                    expected: read.expected,
                    reports: live.reported,
                    expects: live.expects,
                };
            }
            (live.read.clone(), live.out_seq, activity, status_msg, live.rows, live.expects.map(|x| x.seq))
        };
        let mut survey = false;
        let mut live_read = false;
        let (cls, screen) = match self.screen_under_survey(entry, usize::from(rows), &mut survey).await {
            Ok(screen) => {
                live_read = true;
                let mut cls = classify_screen_rich(entry.cli, activity, &screen, true);
                if cls.summary.is_none() {
                    cls.summary = status_msg;
                }
                (cls, screen)
            }
            // (Its tab is gone: the event that says so is on its way.)
            Err(e) if e.gone() => (exited("tab closed".into()), String::new()),
            // (The server didn't say: as it last stood, or starting.)
            Err(_) => reading.map(|r| (r.cls, r.screen)).unwrap_or_else(|| (starting(), String::new())),
        };
        let mut st = self.lock();
        let (mut out, mut chg, mut pid, mut reports, mut expects) = (0, 0, entry.last_pid, false, None);
        if let Some(live) = st.live.get_mut(&entry.id) {
            // (Kept as a reading only where the screen was read: what the
            // server couldn't say is answered by the last reading, and
            // that keeps the count and the giving it was made at. Stored
            // under this asking's, an old "at work" passed for a reading
            // of the screen after its turn's end, and nothing was told
            // of that end. A code review, 2026-11-27.)
            if live_read && cls.state != SubagentLiveState::Exited {
                live.read = Some(Reading { out_seq, activity, cls: cls.clone(), screen: screen.clone(), expected });
            }
            // Any live reading that sees a turn on (`cls.in_turn`) proves the
            // child took up what it was given, even if its turn finishes before
            // the watcher's `QUIET` debounce fires `transition()`. Clear the
            // cached transition dedupe state and invalidate any stale pre-turn
            // confirmation task so the turn's end is never swallowed against
            // the pre-turn rest.
            // Only what it had been given when this reading began: a
            // message noted since is another turn's, which a later
            // reading sees begin.
            if live_read
                && state::takes_up(cls.in_turn, expected, live.expects)
                && let Some(given) = live.expects.take()
            {
                live.answers = Some(given.cursor);
                live.seen_working = true;
                live.held_until = None;
                live.last_state = None;
                live.last_key = None;
                live.check_gen = live.check_gen.wrapping_add(1);
            }
            if live_read && cls.approval.is_none() {
                live.box_now = None;
            }
            live.survey_stays = survey;
            (out, chg, pid, reports, expects) = (live.last_output_at, live.last_state_change_at, live.pid.or(entry.last_pid), live.reported, live.expects);
        }
        // (And a screen that wasn't read takes up nothing.)
        Looked { cls, screen, last_output_at: out, last_state_change_at: chg, pid, survey, expected: expected.filter(|_| live_read), reports, expects }
    }

    /// A child's screen as the server has it, with a feedback survey
    /// drawn over it set aside first (its own "0: Dismiss", "[0] Skip":
    /// never a question for whoever looks after the agent, and a
    /// permission prompt may lie under it). Three times at most; one
    /// that stays is said (`survey`) and left alone until it has gone.
    async fn screen_under_survey(&self, entry: &NativeSubagent, rows: usize, survey: &mut bool) -> Result<String, RexErr> {
        let mut screen = self.rex.screen(&entry.session_id, &entry.block_id, rows).await?;
        let Some(agent) = entry.cli.screen_agent() else { return Ok(screen) };
        let stays = self.lock().live.get(&entry.id).is_some_and(|l| l.survey_stays);
        for _ in 0..3 {
            let Some(found) = olo_agent_screen::parse_survey(agent, &screen) else {
                *survey = false;
                return Ok(screen);
            };
            *survey = true;
            if stays {
                break;
            }
            self.rex.write(&entry.session_id, &entry.block_id, found.dismiss).await?;
            tokio::time::sleep(Duration::from_millis(300)).await;
            screen = self.rex.screen(&entry.session_id, &entry.block_id, rows).await?;
        }
        *survey = olo_agent_screen::parse_survey(agent, &screen).is_some();
        Ok(screen)
    }

    /// Where a child's list is kept: in its folder of the service's
    /// own, beside its minder's rules, so that a service started in this
    /// one's place still goes by it.
    fn leave_file(&self, id: &str) -> PathBuf {
        self.paths.dir.join("minders").join(id).join("approve-all-but.txt")
    }

    fn start_file(&self, id: &str) -> PathBuf {
        self.paths.dir.join("minders").join(id).join("started-with.json")
    }

    /// What a child was started with, kept in its own folder so that it
    /// can be started again as it was (`resume`): the arguments as they
    /// reached the service (a file named in them is its text by then: a
    /// minder's rules, a list), less its prompt, which is said once and
    /// is nobody's to keep. The owner's alone to read: an `--env` or a
    /// `--settings` may hold what is nobody else's.
    pub fn keep_start(&self, id: &str, raw: &Value) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let Some(args) = raw.get("args").and_then(Value::as_array) else { return };
        let args: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
        let (mut kept, mut after) = (Vec::with_capacity(args.len()), false);
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg {
                _ if after => kept.push(arg),
                "--" => {
                    after = true;
                    kept.push(arg);
                }
                "--prompt" | "-p" => {
                    it.next();
                }
                a if a.starts_with("--prompt=") => {}
                _ => kept.push(arg),
            }
        }
        let file = self.start_file(id);
        let written = std::fs::create_dir_all(file.parent().unwrap_or(&self.paths.dir))
            .and_then(|()| std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&file))
            .and_then(|mut f| f.write_all(json!({ "args": kept }).to_string().as_bytes()));
        if let Err(e) = written {
            logged!("olo agents: what {id} was started with couldn't be kept ({e}): it can't be started again by `resume`");
        }
    }

    /// What a child was started with (`keep_start`), where it was kept.
    pub fn kept_start(&self, id: &str) -> Option<Vec<String>> {
        let kept: Value = serde_json::from_str(&std::fs::read_to_string(self.start_file(id)).ok()?).ok()?;
        Some(kept["args"].as_array()?.iter().filter_map(|a| a.as_str().map(str::to_owned)).collect())
    }

    /// Gives the child `id` a list (`text`: a pattern a line) from now,
    /// in another's place if it had one: how many patterns. A line that
    /// is no pattern is refused, and nothing changes.
    pub fn set_leave(&self, id: &str, text: &str) -> Result<usize, String> {
        let list = super::leave::LeaveList::parse(text)?;
        let file = self.leave_file(id);
        std::fs::create_dir_all(file.parent().unwrap_or(&self.paths.dir)).and_then(|()| std::fs::write(&file, text)).map_err(|e| format!("{}: {e}", file.display()))?;
        let n = list.patterns().len();
        self.lock().leave.insert(id.to_owned(), Some(Arc::new(list)));
        Ok(n)
    }

    /// Takes the child's list away: everything it asks leave for is
    /// asked again. Whether it had one.
    pub fn clear_leave(&self, id: &str) -> bool {
        let had = std::fs::remove_file(self.leave_file(id)).is_ok();
        self.lock().leave.insert(id.to_owned(), None).flatten().is_some() || had
    }

    /// The child's list, if it has one.
    pub fn leave_of(&self, id: &str) -> Option<Arc<super::leave::LeaveList>> {
        if let Some(known) = self.lock().leave.get(id) {
            return known.clone();
        }
        // (Its file, read once: a list that can't be read as it was
        // written allows nothing, and says so in the log.)
        let list = std::fs::read_to_string(self.leave_file(id)).ok().and_then(|text| match super::leave::LeaveList::parse(&text) {
            Ok(list) => Some(Arc::new(list)),
            Err(e) => {
                logged!("olo agents: the list kept for {id} isn't one any more ({e}): everything it asks is asked");
                None
            }
        });
        self.lock().leave.insert(id.to_owned(), list.clone());
        list
    }

    /// What the service allowed by a child's list, kept with what its
    /// minder answered (`olo agent mind NAME`): the whole of what was
    /// asked.
    fn leave_given(&self, id: &str, asked: &str) {
        use std::io::Write as _;
        let line = json!({ "at_ms": now_unix_ms(), "did": "allowed", "said": asked, "by": "list" }).to_string();
        let dir = self.paths.dir.join("minders").join(id);
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("answers.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// The key of the approval box that stands on a child's screen
    /// (`approval.box`, and what `approve --box KEY` answers; the
    /// ShiftUp lead's item 40, 2026-11-27: "`approve` answers whatever
    /// stands when it arrives, so a yes judged for one box can land on
    /// the next"): its number among the child's boxes and what it is
    /// (`state::box_hash`), "b7-1f3a9c02". The same box read again has
    /// the same key; another, the next number, and so has the very
    /// same command asked again after it was answered (the box is put
    /// away when it is answered here, `box_gone`, and when the screen
    /// is read with none on it). Numbers begin again with the service:
    /// a key from before then answers nothing, which is the safe way to
    /// be wrong.
    pub fn box_key(&self, id: &str, ap: Option<&olo_agent_screen::ApprovalPrompt>) -> Option<String> {
        let mut st = self.lock();
        let live = st.live.get_mut(id)?;
        let Some(ap) = ap else {
            live.box_now = None;
            return None;
        };
        let what = state::box_hash(ap);
        let number = match live.box_now {
            Some((was, number)) if was == what => number,
            _ => {
                live.boxes += 1;
                live.box_now = Some((what, live.boxes));
                live.boxes
            }
        };
        Some(format!("b{number}-{:08x}", what as u32))
    }

    /// The box that stood was answered: whatever stands next is another.
    pub fn box_gone(&self, id: &str) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.box_now = None;
        }
    }

    /// An approval as it is told (`approval_to_json`), with its box's key.
    pub fn approval_json(&self, id: &str, ap: &olo_agent_screen::ApprovalPrompt, pending: &[Value]) -> Value {
        let mut told = olo_rex_link::agents::approval_to_json(ap, pending);
        told["box"] = json!(self.box_key(id, Some(ap)));
        told
    }

    /// A child was given work (a prompt at its start, a message sent and
    /// submitted while it rested): `cursor` is what its giver was
    /// answered with, and for `within` its rest is the rest before that
    /// turn (`state::Expected`).
    pub fn expect_work(&self, entry: &NativeSubagent, cursor: u64, within: Duration, transcript_known: bool) {
        // (How long its conversation's file is now, where it has one:
        // grown later, with its turn over, a turn was there unseen.)
        let transcript_len = transcript_known.then(|| entry.transcript_path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len()));
        if let Some(live) = self.lock().live.get_mut(&entry.id) {
            let now = now_unix_ms();
            // (A prompt at its start: its turn can have begun from the
            // moment the service asked for its tab, `started_at`. A
            // message: from now, before it is typed.)
            let turn_from_ms = if transcript_known { now } else { to_unix_ms(entry.started_at).max(0) as u64 };
            live.given += 1;
            live.expects = Some(state::Expected { cursor, given_ms: now, until_ms: now + within.as_millis() as u64, transcript_len, turn_from_ms, seq: live.given });
        }
    }

    /// Forgets what was last made of a child (after typing into it: the
    /// next thing it shows is an event even where it looks the same).
    pub fn reset_edge(&self, entry: &NativeSubagent, clear_seen_working: bool) {
        let mut st = self.lock();
        if let Some(live) = st.live.get_mut(&entry.id) {
            live.last_state = None;
            live.last_key = None;
            live.read = None;
            if clear_seen_working {
                live.seen_working = false;
            }
        }
        st.log.reset_child_dedupe(&entry.id);
    }

    /// Has a child's watcher read its screen now.
    pub fn poke(&self, id: &str) {
        if let Some(live) = self.lock().live.get(id) {
            live.poke.notify_one();
        }
    }

    // ---- What is read of the agents themselves (their transcripts, their processes) ----

    /// Each child's agent as this host has it: the conversation it is
    /// in, its last message whole, the tool call it waits on, what it
    /// runs. What was found of a conversation is kept.
    pub async fn inspect(self: &Arc<Self>, entries: &[NativeSubagent]) -> HashMap<String, Value> {
        let entries: Vec<&NativeSubagent> = entries.iter().filter(|e| e.cli.is_agent()).collect();
        if entries.is_empty() {
            return HashMap::new();
        }
        let items: Vec<crate::agent::InspectItemRequest> = {
            let st = self.lock();
            entries
                .iter()
                .map(|e| crate::agent::InspectItemRequest {
                    id: e.id.clone(),
                    cli: e.cli.binary().to_owned(),
                    // Never feed an exited record's recycled `last_pid` into live process discovery.
                    shell_pid: if e.exited_at.is_some() { 0 } else { st.live.get(&e.id).and_then(|l| l.pid).or(e.last_pid).unwrap_or(0) },
                    cwd: e.cwd.clone().unwrap_or_default(),
                    known_session_id: e.effective_session_id().map(str::to_owned),
                    known_transcript: e.transcript_path.clone(),
                    started_at: e.started_at,
                })
                .collect()
        };
        // (Files and `/proc`, read off the service's own thread.)
        let Ok(found) = tokio::task::spawn_blocking(move || crate::agent::inspect_items(&items)).await else { return HashMap::new() };
        let mut map = HashMap::new();
        let mut changed = false;
        {
            let mut st = self.lock();
            for item in found {
                let Ok(v) = serde_json::to_value(&item) else { continue };
                let Some(id) = v["id"].as_str().map(str::to_owned) else { continue };
                changed |= st.reg.update_discovered(&id, v["cli_session_id"].as_str(), v["session_confidence"].as_str(), v["transcript_path"].as_str(), v["pid"].as_i64().map(|p| p as i32));
                map.insert(id, v);
            }
        }
        if changed {
            self.save.notify_one();
        }
        map
    }

    pub fn pending_tools(info: Option<&Value>) -> Vec<Value> {
        if let Some(arr) = info.and_then(|v| v["pending_tools"].as_array()).filter(|a| !a.is_empty()) {
            return arr.clone();
        }
        info.map(|v| &v["pending_tool"]).filter(|v| !v.is_null()).map(|v| vec![v.clone()]).unwrap_or_default()
    }

    // ---- What happens to a child ----

    /// [`record`](Self::record) for what isn't a state of the child's:
    /// what its minder (or the child itself) has for the parent. Always
    /// written, as a new event.
    pub fn record_beside(&self, event: SubagentEvent) -> u64 {
        let seq = {
            let mut st = self.lock();
            let seq = st.log.push_beside(event.clone());
            let mut event = event;
            event.seq = seq;
            st.reg.event_seq = seq;
            if let Ok(mut line) = serde_json::to_string(&event) {
                use std::io::Write as _;
                line.push('\n');
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.paths.events) {
                    let _ = f.write_all(line.as_bytes());
                }
            }
            seq
        };
        self.save.notify_one();
        self.changed.notify_waiters();
        seq
    }

    /// Writes an event down and tells who waits: in that order. An event
    /// a watcher has been told is in the log's file already (one line
    /// appended, there whatever becomes of this process), so a service
    /// killed the moment after goes on from that event's number and
    /// never says it twice.
    pub fn record(&self, event: SubagentEvent) -> u64 {
        let seq = {
            let mut st = self.lock();
            let before = st.log.latest_seq();
            let seq = st.log.push(event.clone());
            if seq > before {
                let mut event = event;
                event.seq = seq;
                st.reg.event_seq = seq;
                if let Some(live) = st.live.get_mut(&event.id) {
                    live.latest_event = Some(event.clone());
                }
                // (Under the lock: the file's lines are in the log's order.)
                if let Ok(mut line) = serde_json::to_string(&event) {
                    use std::io::Write as _;
                    line.push('\n');
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.paths.events) {
                        let _ = f.write_all(line.as_bytes());
                    }
                }
            }
            seq
        };
        self.save.notify_one();
        self.changed.notify_waiters();
        seq
    }

    pub fn event_of(&self, entry: &NativeSubagent, state: SubagentLiveState, summary: Option<String>, dedupe_key: String) -> SubagentEvent {
        let ancestors = self.lock().reg.ancestor_windows(HOST, &entry.session_id, &entry.window_id);
        SubagentEvent {
            seq: 0,
            timestamp_ms: now_unix_ms(),
            host_key: HOST.to_owned(),
            session_id: entry.session_id.clone(),
            parent_window_id: entry.parent_window_id.clone(),
            ancestor_windows: ancestors,
            id: entry.id.clone(),
            name: entry.name.clone(),
            window_id: entry.window_id.clone(),
            block_id: entry.block_id.clone(),
            resume_id: entry.resume_id.clone(),
            cli_session_id: entry.effective_session_id().map(str::to_owned),
            event: state.as_str().to_owned(),
            state,
            summary,
            last_message: None,
            approval: None,
            error: None,
            tail: Vec::new(),
            dedupe_key,
            // (The turn it is of, where its child was seen to take one up.)
            answers: self.lock().live.get(&entry.id).and_then(|l| l.answers),
            minding: None,
        }
    }

    /// Tells followers how a child stands, where that differs from what
    /// they were last told. Reads the current registry entry and sends
    /// the frame under the state lock so a late screen read never puts a
    /// moved worker back under its old parent in Rex.
    fn tell(&self, id: &str, state: SubagentLiveState, summary: Option<String>) {
        // (What a working child last printed changes with every line: not said.)
        let summary = if state == SubagentLiveState::Working { None } else { summary };
        let mut st = self.lock();
        let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
        let Some(live) = st.live.get_mut(id) else { return };
        if live.told.as_ref().is_some_and(|(s, m)| *s == state && *m == summary) {
            return;
        }
        live.told = Some((state, summary.clone()));
        let _ = self.frames.send(Frame::Agent { agent: Box::new(AgentView { entry, state, summary }) });
    }

    /// The registry has something to write.
    pub fn saved(&self) {
        self.save.notify_one();
    }

    /// Says a thing to followers.
    pub fn said(&self, frame: Frame) {
        let _ = self.frames.send(frame);
    }

    /// Tells followers a child's record as it is now (it started, was
    /// found out more of, exited), serialized with state updates under
    /// the state lock so move frames and state frames cannot reorder.
    pub fn tell_entry(&self, id: &str) {
        let st = self.lock();
        let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
        let (state, summary) = match (entry.exited_at, st.live.get(id).and_then(|l| l.told.clone())) {
            (Some(_), _) => (SubagentLiveState::Exited, None),
            (None, Some((state, summary))) => (state, summary),
            (None, None) => (SubagentLiveState::Working, None),
        };
        let _ = self.frames.send(Frame::Agent { agent: Box::new(AgentView { entry, state, summary }) });
    }

    /// A child's program ended (`code`), or its tab closed (none).
    pub fn exit(self: &Arc<Self>, id: &str, code: Option<i32>) {
        let moved: Vec<String>;
        let orphaned_minded: Vec<String>;
        let entry = {
            let mut st = self.lock();
            if let (Some(live), Some(code)) = (st.live.get_mut(id), code) {
                live.exit_code = Some(code);
            }
            let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
            // (Its own children go under its parent: said to followers too.)
            moved = st
                .reg
                .entries
                .iter()
                .filter(|e| {
                    e.session_id == entry.session_id && e.exited_at.is_none() && (e.parent_window_id == entry.window_id || e.minded_by.as_deref() == Some(entry.id.as_str())) && e.id != entry.id
                })
                .map(|e| e.id.clone())
                .collect();
            orphaned_minded = st.reg.minded_by(&entry.id).into_iter().map(|e| e.id.clone()).collect();
            if !st.reg.mark_exited_with_code(HOST, &entry.session_id, &entry.window_id, code) {
                // (Said already; its tab closing after its program ended ends its watcher.)
                if code.is_none()
                    && let Some(live) = st.live.remove(id)
                    && let Some(task) = live.task
                {
                    task.abort();
                }
                return;
            }
            if entry.is_minder() {
                st.seen.retain(|(m, _), _| m != &entry.id);
                st.minding_seen.retain(|(m, _), _| m != &entry.id);
                st.told.retain(|(m, _), _| m != &entry.id);
                st.pending_rules_rev.retain(|(m, _), _| m != &entry.id);
                st.pending_exit_reports.remove(&entry.id);
                st.unhealthy_minders.remove(&entry.id);
                st.closing_minders.remove(&entry.id);
                st.empty_gen.remove(&entry.id);
            }
            if code.is_none()
                && let Some(live) = st.live.remove(id)
                && let Some(task) = live.task
            {
                task.abort();
            }
            entry
        };
        let (summary, error) = match code {
            Some(code) => (format!("exited (code {code})"), (code != 0).then(|| format!("process exited with code {code}"))),
            None => ("tab closed".to_owned(), None),
        };
        let mut event = self.event_of(&entry, SubagentLiveState::Exited, Some(summary), "exited".into());
        event.error = error;
        self.record(event);
        self.tell_entry(id);
        for child in &moved {
            self.tell_entry(child);
        }
        self.minding_ends(&entry, code, &orphaned_minded);
        self.idle.notify_one();
    }

    /// Releases every live agent still minded by a shared minder that
    /// hit a confirmed error (screen or transcript, without stopping its
    /// agents): marks the minder unhealthy so it is not reused, moves each
    /// live agent back under the minder's parent tab, clears `minded_by`,
    /// notifies followers, escalates to the lead, and arms the empty-minder
    /// shutdown timer so the errored minder does not occupy a tab forever.
    fn release_minded_on_error(self: &Arc<Self>, minder: &NativeSubagent, how: &str) {
        let released: Vec<String> = {
            let mut st = self.lock();
            st.unhealthy_minders.insert(minder.id.clone());
            let ids: Vec<String> = st.reg.minded_by(&minder.id).into_iter().map(|e| e.id.clone()).collect();
            for e in &mut st.reg.entries {
                if e.exited_at.is_none() && e.minded_by.as_deref() == Some(minder.id.as_str()) {
                    e.minded_by = None;
                    if e.parent_window_id == minder.window_id {
                        e.parent_window_id = minder.parent_window_id.clone();
                    }
                }
            }
            for id in &ids {
                *st.rules_gen.entry(id.clone()).or_insert(0) += 1;
            }
            st.seen.retain(|(m, _), _| m != &minder.id);
            st.minding_seen.retain(|(m, _), _| m != &minder.id);
            st.told.retain(|(m, _), _| m != &minder.id);
            st.pending_rules_rev.retain(|(m, _), _| m != &minder.id);
            st.pending_exit_reports.remove(&minder.id);
            ids
        };
        if !released.is_empty() {
            self.saved();
            for id in &released {
                self.tell_entry(id);
            }
            self.escalate_minder_ended(&released, how);
        }
        self.arm_empty_minder_if_idle(&minder.id, minder_reports());
    }

    /// Records an escalation on each still-live agent whose minder
    /// ended so the lead's watch is woken immediately and the agents'
    /// standing questions become the lead's again.
    pub(super) fn escalate_minder_ended(&self, agent_ids: &[String], how: &str) {
        for agent_id in agent_ids {
            let found = {
                let st = self.lock();
                st.reg.entries.iter().rev().find(|e| e.id == *agent_id && e.exited_at.is_none()).cloned().map(|it| {
                    let state = st.live.get(&it.id).and_then(|l| l.last_state).unwrap_or(SubagentLiveState::Working);
                    (it, state)
                })
            };
            if let Some((it, state)) = found {
                let msg = format!("Its minder ended ({how}): what it asks comes to you again.");
                let mut event = self.event_of(&it, state, Some(msg.clone()), String::new());
                event.event = olo_rex_link::agents::minder::ESCALATION.to_owned();
                event.last_message = Some(msg);
                self.record_beside(event);
            }
        }
    }

    /// Claude Code's offer to send a feedback report it has drafted ("1
    /// to review · 2 to send · 0 to dismiss") stands on a child's
    /// screen: whoever looks after the child is told, once for each
    /// showing of it, as an `escalation` on their watch (a minder's
    /// agent too: a minder may press no such key). The service answers
    /// it never, and nothing is typed into that screen while it shows
    /// (`Service::send`). The ShiftUp lead's item 48, 2026-11-27, with
    /// Ben's rule there: "no feedback report is ever sent, and nothing
    /// answers that box by itself".
    fn feedback_draft_seen(&self, entry: &NativeSubagent, screen: &str) {
        let Some(agent) = entry.cli.screen_agent() else { return };
        let shows = olo_agent_screen::feedback_draft(agent, screen);
        let state = {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(&entry.id) else { return };
            match (&shows, live.draft_told) {
                (None, _) => {
                    live.draft_told = false;
                    return;
                }
                (Some(_), true) => return,
                (Some(_), false) => live.draft_told = true,
            }
            live.last_state.unwrap_or(SubagentLiveState::Idle)
        };
        let who = entry.name.as_deref().unwrap_or(&entry.id);
        let msg = format!(
            "Feedback draft: Claude Code offers to send a feedback report it drafted ({}). Nothing answers it for you and nothing is typed into this child while it shows: `olo agent send {who} --key 0` sets it aside.",
            shows.unwrap_or_default()
        );
        let mut event = self.event_of(entry, state, Some(msg.clone()), String::new());
        event.event = olo_rex_link::agents::minder::ESCALATION.to_owned();
        event.last_message = Some(msg);
        event.tail = tail_of(screen, 10);
        self.record_beside(event);
    }

    /// What a child's end means for minding.
    /// - An older 1:1 minder whose agent still runs: the parent is told
    ///   (an escalation on that agent), and what the agent asks reaches
    ///   the parent's watch again.
    /// - A shared minder whose agents still run: each live agent has
    ///   already been moved back under the minder's parent tab and
    ///   unminded in [`exit`]; unless this minder was torn down with an
    ///   ancestor's `stop --tree` (`let_go`), the lead is told on each
    ///   live agent.
    /// - A minded agent that exited: if its minder is an older 1:1
    ///   minder, or a shared minder with no live agents left, schedule
    ///   the minder to end after [`minder_reports`] (unless it reports
    ///   sooner or another agent is given to it meanwhile).
    fn minding_ends(self: &Arc<Self>, ended: &NativeSubagent, code: Option<i32>, orphaned_minded: &[String]) {
        let how = || match code {
            Some(code) => format!("exited (code {code})"),
            None => "its tab was closed".to_owned(),
        };
        if let Some(minded) = ended.minds.as_deref() {
            if self.lock().let_go.contains_key(&ended.id) {
                return;
            }
            self.escalate_minder_ended(&[minded.to_owned()], &how());
            return;
        }
        if ended.is_shared_minder() {
            let to_escalate: Vec<String> = {
                let st = self.lock();
                match st.let_go.get(&ended.id) {
                    Some(None) => return,
                    Some(Some(stopping)) => orphaned_minded.iter().filter(|id| !stopping.contains(*id)).cloned().collect(),
                    None => orphaned_minded.to_vec(),
                }
            };
            if !to_escalate.is_empty() {
                self.escalate_minder_ended(&to_escalate, &how());
            }
            return;
        }
        let Some(minder) = self.lock().reg.minder_of(&ended.id).cloned() else { return };
        if minder.is_shared_minder() {
            {
                let mut st = self.lock();
                st.pending_exit_reports.entry(minder.id.clone()).or_default().insert(ended.id.clone());
            }
            self.saved();
            self.arm_empty_minder_if_idle(&minder.id, minder_reports());
            return;
        }
        self.end_minder_in(minder, minder_reports());
    }

    /// Arms a generation-checked empty-minder shutdown timer for shared
    /// minder `minder_id` if it currently has zero live assignments and
    /// zero pending reservations. Any earlier timer's generation is
    /// superseded so a subsequent empty period always gets its full grace
    /// time; on expiry, the minder is atomically marked in
    /// `closing_minders` before releasing the lock so no concurrent start
    /// or move can select it while Rex closes its tab.
    pub fn arm_empty_minder_if_idle(self: &Arc<Self>, minder_id: &str, after: Duration) {
        let armed = {
            let mut st = self.lock();
            let Some(m) = st.reg.entries.iter().rev().find(|e| e.id == minder_id && e.exited_at.is_none() && e.is_shared_minder()).cloned() else {
                return;
            };
            let idle = st.reg.minded_by(&m.id).is_empty() && st.pending_minds.get(&m.id).copied().unwrap_or(0) == 0 && !st.closing_minders.contains(&m.id);
            if !idle {
                return;
            }
            let epoch = Self::bump_empty_gen_locked(&mut st, &m.id);
            (m, epoch)
        };
        let (minder, epoch) = armed;
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let should_stop = {
                let mut st = service.lock();
                let still = st.reg.entries.iter().any(|e| e.id == minder.id && e.exited_at.is_none());
                let same_gen = st.empty_gen.get(&minder.id) == Some(&epoch);
                let idle = st.reg.minded_by(&minder.id).is_empty() && st.pending_minds.get(&minder.id).copied().unwrap_or(0) == 0;
                if still && same_gen && idle && !st.closing_minders.contains(&minder.id) {
                    st.closing_minders.insert(minder.id.clone());
                    true
                } else {
                    false
                }
            };
            if should_stop {
                let _ = service.stop_tabs_inner(&minder, true, false, false, true).await;
                service.lock().closing_minders.remove(&minder.id);
            }
        });
    }

    /// Stops a minder whose agents have ended, `after` that long:
    /// shared minders go through generation-checked [`arm_empty_minder_if_idle`];
    /// older 1:1 minders stop directly once their time is up.
    pub fn end_minder_in(self: &Arc<Self>, minder: NativeSubagent, after: Duration) {
        if minder.is_shared_minder() {
            self.arm_empty_minder_if_idle(&minder.id, after);
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let still = service.lock().reg.entries.iter().any(|e| e.id == minder.id && e.exited_at.is_none());
            if still {
                let _ = service.stop_tabs_inner(&minder, true, false, false, true).await;
            }
        });
    }

    /// Reads a child's screen and, where its state differs from what was
    /// last made of it, writes that down once it has held ([`CONFIRM`]):
    /// with its last message whole and the tool it waits on, from its
    /// transcript.
    async fn transition(self: &Arc<Self>, id: &str) {
        let Some(entry) = self.lock().reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
        let looked = self.look(&entry, false).await;
        let read_expecting = looked.expected;
        self.feedback_draft_seen(&entry, &looked.screen);
        let cls = looked.cls;
        // Its tab is gone (the server said so of its screen): its end is
        // said once, as an end is said (`exit`), whichever word of it
        // comes first: this, or the server's own event that the tab
        // closed. Never as a state that held.
        if cls.state == SubagentLiveState::Exited {
            if matches!(self.tab_presence(&entry).await, TabPresence::Gone) {
                self.exit(id, None);
            }
            return;
        }
        if self.text_watchers.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            self.changed.notify_waiters();
        }
        // (Which box stands, or that none does: a box come after one
        // that went is another, though it asks the very same. Key the
        // transition by `box:<key>` so a second identical box never matches
        // `live.last_key` from box 1.)
        let key = if let Some(bk) = self.box_key(id, cls.approval.as_ref()) { format!("box:{bk}") } else { subkey(&cls) };
        let started_ms = to_unix_ms(entry.started_at).max(0) as u64;
        let at_prompt = cls.summary.as_deref() == Some(state::AT_PROMPT);
        let mut look_again = None;
        let generation = {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(id) else { return };
            // Anything but rest says it has begun: at work, asking,
            // failed. Not the word for a tab with nothing drawn in it
            // yet, "starting": that counted as work, and whether a
            // child's first rest was then told hung on whether the very
            // first reading of its tab was of an empty one (told a
            // turn's end 0.6 s after its start, the tab's own shell on
            // the screen; or never told at all, and whoever waited for
            // it to be up waited out their time). What it was given, it
            // has now taken up: the turn that follows answers that.
            if cls.state != SubagentLiveState::Idle && !(cls.state == SubagentLiveState::Working && cls.summary.as_deref() == Some(state::STARTING)) {
                live.seen_working = true;
            }
            // What it was given, it has taken up once its own screen
            // says a turn is on (`ScreenClassification::in_turn`): the
            // turn that follows answers that. Not at anything else that
            // isn't rest. (It was: an agy started with a prompt is "at
            // work" by Rex's word for its pane while it loads, then
            // stands at its own empty prompt for a moment before it
            // takes the prompt up; that rest was told as the prompt's
            // turn's end, 1.3 s after the start, and `olo agent run`
            // returned done, with no message, and closed the tab on an
            // agent that had done nothing. The ShiftUp lead, 2026-11-27,
            // item 46: "we nearly read 'no findings' from it". A
            // folder's trust asked at the start did the same.)
            // (And only what it had been given when the reading this
            // goes by began: `Looked::expected`.)
            if state::takes_up(cls.in_turn, read_expecting, live.expects)
                && let Some(given) = live.expects.take()
            {
                live.answers = Some(given.cursor);
            }
            // At rest: is that to tell (`state::rest_now`)? Not while it
            // has been given work it hasn't been seen at, nor while what
            // is on its screen isn't yet its own.
            let kept = match cls.state {
                SubagentLiveState::Idle => match state::rest_now(now_unix_ms(), started_ms, live.seen_working, at_prompt, live.expects, None) {
                    state::Rest::Kept { until_ms } => Some(until_ms),
                    state::Rest::Told { .. } => None,
                },
                _ => None,
            };
            // (A rest kept back is read again at whatever the child
            // shows, the same or not, and when its time is up.)
            let was_kept = live.held_until.is_some();
            if kept.is_some() && live.held_until != kept {
                look_again = kept;
            }
            live.held_until = kept;
            // (The same as it last stood: nothing new to make of it.
            // Unless it has been given work it was never seen at: then
            // a rest that looks the same as the last may be the end of
            // a turn that came and went between two readings, which its
            // conversation's file says, below. Without this a message
            // taken up late and answered quickly was never told: found
            // by ten copies of a test at once, 2026-11-27.)
            if live.last_state == Some(cls.state) && live.last_key.as_deref() == Some(key.as_str()) && !was_kept && kept.is_none() && live.expects.is_none() {
                return;
            }
            live.last_state = Some(cls.state);
            live.last_key = Some(key.clone());
            live.last_state_change_at = now_unix_ms();
            live.check_gen = live.check_gen.wrapping_add(1);
            // (Kept back with work given: its conversation's file may
            // say the turn came and went unseen, which is read below.
            // Kept back as not yet drawn: nothing more to read.)
            if (kept.is_some() && live.expects.is_none()) || cls.state == SubagentLiveState::Working { None } else { Some(live.check_gen) }
        };
        if let Some(until_ms) = look_again {
            let (service, id) = (self.clone(), id.to_owned());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(until_ms.saturating_sub(now_unix_ms()) + 150)).await;
                service.poke(&id);
            });
        }
        // Followers are told what its screen says now that it differs
        // from what it said (it works, it asks): at once, before it is
        // settled with its transcript below. Not at every reading of a
        // screen that says the same: that took back what had been
        // settled (done, by its transcript, told as at rest again by
        // the next look at the same screen), and a follower that shows
        // the state it is told (the desktop's rows) would have said At
        // rest of a child that was done.
        // (But for a Codex that reads as at rest and isn't kept back: its
        // screen says so within a turn too, and what its conversation's
        // file says of that is read below, a quarter of a second on:
        // `state::turn_on`. Told there, as at rest or as at work.)
        if !(entry.cli == olo_rex_link::agents::AgentCli::Codex && cls.state == SubagentLiveState::Idle && generation.is_some()) {
            self.tell(id, cls.state, cls.summary.clone());
        }
        // (A question come or gone: who waits on that looks at once.)
        if cls.folder_trust.is_some() || cls.approval.is_some() || cls.state == SubagentLiveState::Exited || cls.state == SubagentLiveState::Working {
            self.changed.notify_waiters();
        }
        let Some(generation) = generation else { return };
        let (service, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(CONFIRM).await;
            let current = |service: &Service| service.lock().live.get(&id).is_some_and(|l| l.check_gen == generation);
            if !current(&service) {
                return;
            }
            let Some(entry) = service.lock().reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
            let again = service.look(&entry, true).await;
            if again.cls.state == SubagentLiveState::Exited {
                // (Gone meanwhile: its end is `exit`'s to say.)
                service.poke(&id);
                return;
            }
            let again_key = if let Some(bk) = service.box_key(&id, again.cls.approval.as_ref()) { format!("box:{bk}") } else { subkey(&again.cls) };
            if again.cls.state != cls.state || again_key != key {
                // (It didn't hold: what it shows now is read in its turn.)
                service.poke(&id);
                return;
            }
            let inspected = service.inspect(std::slice::from_ref(&entry)).await;
            // (What took a while to read may have been overtaken.)
            if !current(&service) {
                return;
            }
            let info = inspected.get(&id);
            let entry = service.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned().unwrap_or(entry);
            let last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
            let pending = Self::pending_tools(info);
            let approval = again.cls.approval.as_ref().map(|ap| service.approval_json(&id, ap, &pending));
            // (What its file says of a turn before the one it was given is
            // not of this turn: `state::of_the_turn_given`.)
            let given = service.lock().live.get(&id).and_then(|l| l.expects);
            let turn_ms = info.and_then(|v| v["turn_started_ms"].as_u64());
            let its_turn = state::of_the_turn_given(turn_ms, given);
            let tx = info.and_then(|v| v["transcript_state"].as_str()).filter(|_| its_turn);
            let (state, error, _) = olo_rex_link::agents::effective_state(&again.cls, tx, info.and_then(|v| v["transcript_error"].as_str()).filter(|_| its_turn), true);
            // At rest by its screen, and in a turn by its conversation's
            // file (`state::turn_on`): no rest to tell. Followers are
            // told it works, and its screen and file are read again in a
            // moment (nothing on the screen says when the file changes):
            // until the file says the turn is over, or the agent has
            // shown no life for so long that the file's word is no more
            // believed.
            if state == SubagentLiveState::Idle
                && !again.reports
                && entry.cli == olo_rex_link::agents::AgentCli::Codex
                && tx == Some("working")
                && state::turn_on(entry.cli, tx, turn_ms, to_unix_ms(entry.started_at).max(0) as u64, transcript_written_ms(&entry, info), again.last_output_at, now_unix_ms(), turn_on_ms())
            {
                {
                    let mut st = service.lock();
                    let Some(live) = st.live.get_mut(&id) else { return };
                    if live.check_gen != generation {
                        return;
                    }
                    // (Kept back: the next reading of the same screen is
                    // gone through again, not passed over as the same.)
                    live.held_until = Some(now_unix_ms() + state::TURN_ON_LOOK_MS);
                    live.seen_working = true;
                }
                service.tell(&id, SubagentLiveState::Working, None);
                tokio::time::sleep(Duration::from_millis(state::TURN_ON_LOOK_MS)).await;
                service.poke(&id);
                return;
            }
            // At rest, with what its conversation's file says: told, or
            // kept back still (`state::rest_now`).
            if matches!(state, SubagentLiveState::Idle | SubagentLiveState::Done) {
                let over = matches!(info.and_then(|v| v["transcript_state"].as_str()), Some("done" | "idle" | "waiting" | "aborted"));
                let written = info.and_then(|v| v["transcript_path"].as_str()).or(entry.transcript_path.as_deref()).and_then(|p| std::fs::metadata(p).ok()).map(|m| state::Written {
                    len: m.len(),
                    over,
                    at_ms: m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64),
                    said: last_message.is_some(),
                    turn_ms: info.and_then(|v| v["turn_started_ms"].as_u64()),
                });
                let mut st = service.lock();
                let Some(live) = st.live.get_mut(&id) else { return };
                match state::rest_now(now_unix_ms(), started_ms, live.seen_working, at_prompt, live.expects, written) {
                    state::Rest::Kept { until_ms } => {
                        live.held_until = Some(until_ms);
                        // Looked at again in a moment, and so on until it
                        // is told: what ends the wait is its
                        // conversation's file saying the turn is over,
                        // and nothing on a screen says when a file
                        // changes. With no such look, a turn that was
                        // over by its screen before its file said so (or
                        // whose rest was "done" by its own word, which
                        // armed no look at all) was told when its time
                        // ran out, two minutes on, or never (a code
                        // review, and the live suite under load,
                        // 2026-11-27). One timer a child.
                        if !std::mem::replace(&mut live.relook, true) {
                            let (service, id) = (service.clone(), id.clone());
                            let wait = until_ms.saturating_sub(now_unix_ms()).clamp(50, 1000);
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(wait)).await;
                                if let Some(live) = service.lock().live.get_mut(&id) {
                                    live.relook = false;
                                }
                                service.poke(&id);
                            });
                        }
                        return;
                    }
                    state::Rest::Told { answered } => {
                        if let Some(cursor) = answered {
                            (live.answers, live.expects, live.seen_working) = (Some(cursor), None, true);
                        }
                        live.held_until = None;
                    }
                }
            }
            // Leave it asks for, given by its list where it has one: the
            // service says yes, once, itself and at once, to anything no
            // pattern of the list names, and nobody is told there was a
            // question (`leave.rs`). What a pattern names, what couldn't
            // be read off the screen whole, and a box with no plain "yes,
            // this once" go to whoever answers for it, as before.
            // A command the agent has folded on its screen ("⋯ 19 lines
            // hidden"), or cut short, is never the list's to allow: it
            // judged the lines that showed, and `git push` on a hidden
            // line went through (a code review, 2026-11-27). Nor is it
            // allowed by the same command found whole in the agent's
            // conversation file, as it was for a day: which call there
            // is this box's can't be told for certain (two that begin
            // alike, one written late, another session's file), and the
            // tool is named otherwise there, so a pattern on `^Bash:`
            // missed. What isn't read whole is asked.
            let whole_asked = again.cls.approval.as_ref().and_then(|ap| {
                let cut = ap.folded_lines.is_some()
                    || ap.command.as_deref().is_some_and(|c| {
                        let t = c.trim_end();
                        t.ends_with('…') || t.ends_with("...")
                    });
                if cut {
                    return None;
                }
                super::leave::asked(ap.tool.as_deref(), ap.command.as_deref(), &ap.question)
            });
            if state == SubagentLiveState::WaitingApproval
                && let Some(list) = service.leave_of(&id)
                && let Some(ap) = &again.cls.approval
                && let Some(asked) = whole_asked
                && list.excepts(&asked).is_none()
                && ap.choices.iter().filter(|c| c.kind == olo_agent_screen::ChoiceKind::Once).count() == 1
            {
                let from = super::ops::Caller { session: Some(entry.session_id.clone()), window: None, window_from_block: false, block: None, child: None };
                // (For the box it judged: another that stands by the time
                // the answer is given is not answered by this.)
                let mut args = vec![json!(id.clone()), json!("--once")];
                if let Some(key) = service.box_key(&id, Some(ap)) {
                    args.extend([json!("--box"), json!(key)]);
                }
                args.push(json!("--json"));
                let answered = service.approve(&json!({ "args": args }), &from).await;
                if answered.ok {
                    service.leave_given(&id, &asked);
                    return;
                }
                logged!("olo agents: {id} asked leave its list allows and couldn't be answered ({}): it is asked as any other", answered.stderr.trim());
            }
            let dedupe = if state == SubagentLiveState::WaitingApproval
                && let Some(box_key) = approval.as_ref().and_then(|a| a["box"].as_str())
            {
                format!("box:{box_key}")
            } else if !key.is_empty() {
                key
            } else if let Some(message) = &last_message {
                use std::hash::{Hash as _, Hasher as _};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                message.hash(&mut h);
                format!("msg:{:x}", h.finish())
            } else {
                // (What it shows: the same after a service is started
                // again, so what was said of it isn't said twice; another
                // after another turn.)
                format!("scr:{:x}", olo_agent_screen::fingerprint(&again.screen))
            };
            let err_for_minder = error.clone();
            let sum_for_minder = again.cls.summary.clone();
            let mut event = service.event_of(&entry, state, again.cls.summary.clone(), dedupe);
            event.last_message = last_message;
            event.approval = approval;
            // (Where it says it failed and nothing has the words, its screen may.)
            event.error = error.or_else(|| (state == SubagentLiveState::Error).then(|| olo_agent_screen::error_line(&again.screen)).flatten());
            event.tail = tail_of(&again.screen, 10);
            service.tell(&id, state, again.cls.summary);
            service.record(event);
            // A shared minder that hits an API/transcript error (stuck at
            // prompt, no longer running its watch loop) must not strand its
            // agents: mark unhealthy, release them back under the lead, and
            // arm its empty timer. Confirmed recovery clears `unhealthy_minders`.
            if entry.is_shared_minder() {
                if state == SubagentLiveState::Error {
                    let how = err_for_minder.as_deref().or(sum_for_minder.as_deref()).unwrap_or("API error");
                    service.release_minded_on_error(&entry, how);
                } else {
                    service.lock().unhealthy_minders.remove(&entry.id);
                }
            }
        });
    }

    /// Watches a live child: its stream for output and its program's
    /// end, and its screen after each pause. Ends with the child.
    pub fn watch_child(self: &Arc<Self>, entry: &NativeSubagent) {
        let (service, entry) = (self.clone(), entry.clone());
        let poke = {
            let mut st = self.lock();
            let live = st.live.entry(entry.id.clone()).or_insert_with(Live::new);
            if let Some(old) = live.task.take() {
                old.abort();
            }
            live.poke.clone()
        };
        let id = entry.id.clone();
        let task = tokio::spawn(async move {
            // What the terminal's own process is, its size and what its
            // program says of itself so far.
            service.learn(&entry).await;
            let mut retry = Duration::from_millis(250);
            loop {
                let mut stream = match service.rex.stream(&entry.session_id, &entry.block_id).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        if e.gone() || matches!(service.tab_presence(&entry).await, TabPresence::Gone) {
                            service.exit(&entry.id, None);
                            return;
                        }
                        tokio::time::sleep(retry).await;
                        retry = (retry * 2).min(Duration::from_secs(30));
                        continue;
                    }
                };
                retry = Duration::from_millis(250);
                // (As it stands when the watching begins.)
                service.transition(&entry.id).await;
                // Since when something written is unread, and when it
                // was last written.
                let mut unread: Option<(tokio::time::Instant, tokio::time::Instant)> = None;
                loop {
                    let due = unread.map(|(first, last)| (last + QUIET).min(first + AT_MOST));
                    tokio::select! {
                        said = stream.next() => match said {
                            Said::Output(_) => {
                                let now = tokio::time::Instant::now();
                                unread = Some((unread.map_or(now, |(first, _)| first), now));
                                if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                    live.out_seq += 1;
                                    live.last_output_at = now_unix_ms();
                                }
                            }
                            Said::Exited(code) => {
                                service.exit(&entry.id, Some(code));
                                // (Its tab may stay: watched until it closes.)
                            }
                            Said::Resized { rows } => {
                                if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                    live.rows = rows;
                                    live.read = None;
                                }
                            }
                            Said::Closed => break,
                        },
                        _ = async { tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)).await }, if due.is_some() => {
                            unread = None;
                            service.transition(&entry.id).await;
                        }
                        _ = poke.notified() => {
                            service.transition(&entry.id).await;
                        }
                    }
                }
                // The stream ended: with the tab, or by itself.
                if matches!(service.tab_presence(&entry).await, TabPresence::Gone) {
                    service.exit(&entry.id, None);
                    return;
                }
                tokio::time::sleep(retry).await;
            }
        });
        if let Some(live) = self.lock().live.get_mut(&id) {
            live.task = Some(task.abort_handle());
        }
    }

    /// Three-way check for whether a child's tab still exists on the Rex server:
    /// distinguishes a confirmed `Gone` (absent from `session.view` or `e.gone()`)
    /// from an `Unknown` transient RPC error/timeout so two RPC failures never
    /// count as a closed tab.
    pub(super) async fn tab_presence(&self, entry: &NativeSubagent) -> TabPresence {
        match self.refresh(&entry.session_id).await {
            Ok(()) => {
                let there = self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.contains_key(&entry.block_id));
                if there { TabPresence::Present } else { TabPresence::Gone }
            }
            Err(e) if e.gone() => TabPresence::Gone,
            Err(e) => TabPresence::Unknown(e.to_string()),
        }
    }

    /// What a child's terminal is: its process, its size, what its
    /// program has said of itself.
    async fn learn(self: &Arc<Self>, entry: &NativeSubagent) {
        let said_before = self.lock().live.get(&entry.id).map_or(0, |l| l.status_seq);
        let (process, size, status) = tokio::join!(
            self.rex.terminal(&entry.session_id, &entry.block_id, "process", json!({})),
            self.rex.terminal(&entry.session_id, &entry.block_id, "size", json!({})),
            self.rex.terminal(&entry.session_id, &entry.block_id, "program_status", json!({})),
        );
        let exit_code = process.as_ref().ok().and_then(|p| p["last_exit"]["exit_code"].as_i64()).map(|c| c as i32);
        {
            let mut st = self.lock();
            let Some(live) = st.live.get_mut(&entry.id) else { return };
            if let Ok(p) = &process {
                live.pid = p["child"]["pid"].as_i64().or_else(|| p["foreground"]["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 0).or(live.pid);
            }
            if let Ok(s) = &size {
                live.rows = s["rows"].as_u64().unwrap_or(0) as u16;
            }
            // (Its records as they stood when asked. A word of its own
            // that came while the three were asked is newer than that:
            // "done", come between, was written over with the "working"
            // of the asking, and a still terminal stayed at work. A code
            // review, 2026-11-27.)
            if let Ok(s) = &status
                && live.status_seq == said_before
            {
                live.statuses = s["records"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        (r["id"].as_str().unwrap_or_default().to_owned(), Activity::of(r["state"].as_str().unwrap_or_default()), r["msg"].as_str().or_else(|| r["title"].as_str()).map(str::to_owned))
                    })
                    .collect();
                live.reported |= !live.statuses.is_empty();
            }
        }
        if let Some(code) = exit_code {
            self.exit(&entry.id, Some(code));
        }
    }

    // ---- What the server says unasked ----

    async fn on_event(self: &Arc<Self>, event: Event) {
        match event {
            Event::Session { session, kind, payload } => {
                let child_of = |st: &State, window: Option<&str>, block: Option<&str>| {
                    st.reg
                        .entries
                        .iter()
                        .rev()
                        .find(|e| e.session_id == session && st.live.contains_key(&e.id) && (window.is_some_and(|w| e.window_id == w) || block.is_some_and(|b| e.block_id == b)))
                        .map(|e| e.id.clone())
                };
                match kind.as_str() {
                    "window_closed" | "block_closed" => {
                        let id = {
                            let mut st = self.lock();
                            if let Some(sess) = st.sessions.get_mut(&session) {
                                sess.fresh = false;
                            }
                            child_of(&st, payload["window_id"].as_str(), payload["block_id"].as_str())
                        };
                        if let Some(id) = id {
                            self.exit(&id, None);
                        }
                    }
                    "window_created" | "block_created" | "session_view_changed" | "active_window_changed" => {
                        if let Some(sess) = self.lock().sessions.get_mut(&session) {
                            sess.fresh = false;
                        }
                    }
                    "block_event" => {
                        let name = payload["event"].as_str().or_else(|| payload["name"].as_str()).unwrap_or_default();
                        let Some(id) = child_of(&self.lock(), None, payload["block_id"].as_str()) else { return };
                        let inner = if payload["payload"].is_null() { &payload["data"] } else { &payload["payload"] };
                        match name {
                            "program_status_changed" => {
                                let record = &inner["record"];
                                let said = (
                                    inner["id"].as_str().unwrap_or_default().to_owned(),
                                    Activity::of(record["state"].as_str().unwrap_or_default()),
                                    record["msg"].as_str().or_else(|| record["title"].as_str()).map(str::to_owned),
                                );
                                let before = {
                                    let mut st = self.lock();
                                    let Some(live) = st.live.get_mut(&id) else { return };
                                    let before = live.activity().0;
                                    live.reported = true;
                                    live.status_seq += 1;
                                    match live.statuses.iter_mut().find(|(name, _, _)| *name == said.0) {
                                        Some(old) => *old = said,
                                        None => live.statuses.push(said),
                                    }
                                    (before, live.activity().0)
                                };
                                // (What it does now, not each word it says of it.)
                                if before.0 != before.1 {
                                    self.poke(&id);
                                }
                            }
                            "program_status_removed" => {
                                let gone: Vec<&str> = inner["ids"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                                if let Some(live) = self.lock().live.get_mut(&id) {
                                    live.statuses.retain(|(name, _, _)| !gone.contains(&name.as_str()));
                                    live.reported = true;
                                    live.status_seq += 1;
                                }
                                self.poke(&id);
                            }
                            "process_changed" => {
                                let Some(entry) = self.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
                                let service = self.clone();
                                tokio::spawn(async move {
                                    let Ok(p) = service.rex.terminal(&entry.session_id, &entry.block_id, "process", json!({})).await else { return };
                                    if let Some(live) = service.lock().live.get_mut(&entry.id) {
                                        live.pid = p["child"]["pid"].as_i64().or_else(|| p["foreground"]["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 0).or(live.pid);
                                    }
                                    match p["last_exit"]["exit_code"].as_i64() {
                                        Some(code) => service.exit(&entry.id, Some(code as i32)),
                                        None => service.poke(&entry.id),
                                    }
                                });
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::Server { kind, payload } => {
                if kind == "session_destroyed"
                    && let Some(session) = payload["session_id"].as_str()
                {
                    let ids: Vec<String> = {
                        let mut st = self.lock();
                        st.sessions.remove(session);
                        st.reg.entries.iter().filter(|e| e.session_id == session && e.exited_at.is_none()).map(|e| e.id.clone()).collect()
                    };
                    for id in ids {
                        self.exit(&id, None);
                    }
                }
            }
            Event::Closed(_) => {}
        }
    }

    /// What a new service does with what the last one wrote down: each
    /// child whose tab the server still has is watched again, and each
    /// whose tab is gone has exited (after a restart of the server every
    /// one has, and keeps its conversation's id to be taken up again).
    async fn reconcile(self: &Arc<Self>) {
        let sessions: HashSet<String> = self.session_ids().await.unwrap_or_default().into_iter().collect();
        let mine: Vec<NativeSubagent> = self.lock().reg.entries.iter().filter(|e| e.exited_at.is_none()).cloned().collect();
        let wanted: HashSet<&str> = mine.iter().map(|e| e.session_id.as_str()).filter(|s| sessions.contains(*s)).collect();
        for session in wanted {
            let _ = self.refresh(session).await;
        }
        for entry in mine {
            let there = self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.contains_key(&entry.block_id));
            if there && self.attach(&entry.session_id).await.is_ok() {
                // (One that a service before this rule left as its own: let go of now.)
                self.never_its_size(&entry).await;
                self.watch_child(&entry);
            } else {
                self.lock().live.insert(entry.id.clone(), Live::new());
                self.exit(&entry.id, None);
            }
        }
        // Re-arm the empty-minder timer for any live shared minder left
        // empty across a handover/restart so it never sits indefinitely.
        let empty_minders: Vec<String> = {
            let st = self.lock();
            st.reg.entries.iter().filter(|e| e.exited_at.is_none() && e.is_shared_minder() && st.reg.minded_by(&e.id).is_empty()).map(|e| e.id.clone()).collect()
        };
        for mid in empty_minders {
            self.arm_empty_minder_if_idle(&mid, minder_reports());
        }
    }

    /// A child's tab is never the service's to size. The Rex server
    /// makes whoever creates a block its owner if they are attached to
    /// its session then (rev 19: "If the calling client is attached to
    /// the session, it becomes the block's owner"), and an owner's size
    /// is the one that holds: a desktop that shows the tab asks for its
    /// own view's size and is not given it. The service is attached to
    /// a session from its first child there on, so its second child and
    /// every one after (and every one, after the service started again)
    /// was the service's, and stood at the size of the pane it was
    /// asked from for as long as it ran: shown in a shorter view its
    /// last rows, where an agent's prompt is, were cut off (Ben,
    /// 2026-11-27: "there seems to be a bug rendering some antigravity
    /// agy terminals that are launched as subagents where it doesn't
    /// show the full tui stuff like the typing line"; a 108 × 64 tab in
    /// a view of 56 rows; the second investigator proved the rule on a
    /// scratch server). The service has no view: it gives the block up
    /// and leaves the line for it (`session.disown_block`), which keeps
    /// the size the tab was made with until a desktop shows it. Nothing
    /// for a block it doesn't own; a server that knows no owners says
    /// no, and that is all right.
    async fn never_its_size(&self, entry: &NativeSubagent) {
        let _ = self.rex.call("session.disown_block", json!({ "session_id": entry.session_id, "block_ids": [entry.block_id] })).await;
    }

    /// A child begins: written down, watched, said to followers.
    pub async fn begin(self: &Arc<Self>, entry: NativeSubagent) -> Result<(), RexErr> {
        self.begin_attached(entry, None, None).await.map(|_| ())
    }

    /// Registers a newly started child, atomically validating any chosen
    /// shared minder and converting its pending reservation to a live
    /// assignment in the SAME lock hold (never double-counting load and
    /// never committing an assignment to a minder that exited, errored,
    /// or started closing while the tab was being created). If the minder
    /// became unavailable, the child is registered under `fallback_parent`
    /// unminded and the reason is returned in the second tuple element.
    pub async fn begin_attached(self: &Arc<Self>, mut entry: NativeSubagent, fallback_parent: Option<&str>, reserved_minder: Option<&str>) -> Result<(NativeSubagent, Option<String>), RexErr> {
        self.attach(&entry.session_id).await?;
        self.never_its_size(&entry).await;
        let mut minder_lost: Option<String> = None;
        {
            let mut st = self.lock();
            if let Some(mid) = reserved_minder
                && let Some(n) = st.pending_minds.get_mut(mid)
            {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.pending_minds.remove(mid);
                }
            }
            if let Some(mid) = entry.minded_by.clone() {
                let minder_ok = st.reg.entries.iter().rev().find(|e| e.id == mid && e.exited_at.is_none() && e.is_shared_minder()).filter(|m| {
                    !st.closing_minders.contains(&m.id)
                        && !st.unhealthy_minders.contains(&m.id)
                        && st.live.get(&m.id).and_then(|l| l.last_state) != Some(SubagentLiveState::Error)
                        && st.sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&m.window_id))
                });
                if let Some(m) = minder_ok {
                    entry.parent_window_id = m.window_id.clone();
                    Self::bump_empty_gen_locked(&mut st, &mid);
                } else {
                    entry.minded_by = None;
                    if let Some(lead) = fallback_parent {
                        entry.parent_window_id = lead.to_owned();
                    }
                    minder_lost = Some(format!("selected minder `{mid}` ended or became unavailable while the agent was starting"));
                }
            }
            st.reserved_ids.remove(&entry.id);
            st.reg.register(entry.clone());
            st.live.insert(entry.id.clone(), Live::new());
            st.idle_since = None;
        }
        // (Written down before anybody is told it started: a service
        // killed the moment after knows the tab for its child.)
        self.write_now().await;
        self.tell_entry(&entry.id);
        self.watch_child(&entry);
        Ok((entry, minder_lost))
    }

    // ---- What is written down ----

    /// Writes the registry whenever there is something of it to write:
    /// one writer, so what is written last is what is newest. (An
    /// event's line is written as it is recorded, `record`.)
    async fn writer(self: Arc<Self>) {
        loop {
            self.save.notified().await;
            self.write_now().await;
        }
    }

    async fn write_now(&self) {
        let (reg, minder_state) = {
            let st = self.lock();
            let ms = MinderStateDisk::from_state(&st);
            (st.reg.clone(), ms)
        };
        let paths = self.paths.clone();
        let _ = tokio::task::spawn_blocking(move || {
            reg.save_to(&paths.registry);
            minder_state.save_to(&paths.dir.join("minder-state.json"));
            // (The log's file kept to the last of it: read whole by the next service.)
            if std::fs::metadata(&paths.events).is_ok_and(|m| m.len() > 16 << 20)
                && let Ok(all) = std::fs::read_to_string(&paths.events)
            {
                let keep: Vec<&str> = all.lines().rev().take(2000).collect();
                let kept: String = keep.into_iter().rev().map(|l| format!("{l}\n")).collect();
                let tmp = paths.events.with_extension("jsonl.tmp");
                if std::fs::write(&tmp, kept).is_ok() {
                    let _ = std::fs::rename(&tmp, &paths.events);
                }
            }
        })
        .await;
    }

    // ---- Those that ask ----

    /// Builds a consistent `Frame::Snapshot` under a single lock acquisition
    /// (avoiding the self-deadlock when a temporary guard lived across `views()`).
    fn snapshot(&self) -> Frame {
        let st = self.lock();
        let cursor = st.log.latest_seq();
        let agents = st
            .reg
            .entries
            .iter()
            .map(|entry| {
                let (state, summary) = match (entry.exited_at, st.live.get(&entry.id).and_then(|l| l.told.clone())) {
                    (Some(_), _) => (SubagentLiveState::Exited, None),
                    (None, Some((state, summary))) => (state, summary),
                    (None, None) => (SubagentLiveState::Working, None),
                };
                AgentView { entry: entry.clone(), state, summary }
            })
            .collect();
        Frame::Snapshot { proto: PROTO, cursor, agents }
    }

    /// Children an earlier Olo's desktop kept (it was their keeper then):
    /// taken over once, those whose tabs are still there as live.
    async fn adopt(self: &Arc<Self>, obj: Option<&Value>) -> Response {
        let given: Vec<NativeSubagent> = obj.and_then(|v| v.get("agents")).and_then(|a| serde_json::from_value(a.clone()).ok()).unwrap_or_default();
        let mut taken = 0;
        for mut entry in given {
            entry.host_key = HOST.to_owned();
            // Imported records never carry minder privileges or assignments:
            // strip them unconditionally so `adopt` cannot forge a minder.
            entry.shared_minder = false;
            entry.minded_by = None;
            entry.minds = None;
            if entry.role.as_deref().is_some_and(|r| r.trim().eq_ignore_ascii_case("minder")) {
                entry.role = None;
            }
            if self.lock().reg.entries.iter().any(|e| e.id == entry.id || (e.session_id == entry.session_id && e.window_id == entry.window_id)) {
                continue;
            }
            let _ = self.seen(&entry.session_id).await;
            let there = self.lock().sessions.get(&entry.session_id).is_some_and(|s| s.windows.contains(&entry.window_id));
            taken += 1;
            if entry.exited_at.is_none() && there {
                if self.begin(entry).await.is_err() {
                    taken -= 1;
                }
            } else {
                if entry.exited_at.is_none() {
                    entry.exited_at = Some(now_unix_ms() as i64);
                }
                self.lock().reg.entries.push(entry.clone());
                self.save.notify_one();
                self.tell_entry(&entry.id);
            }
        }
        Response::ok(format!("Took over {taken} child record(s).\n"), json!({ "ok": true, "adopted": taken }))
    }

    /// One connection: a request, and its answer (or, for `follow`,
    /// everything and then each change until the asker goes).
    async fn serve_one(self: Arc<Self>, stream: UnixStream) {
        let peer_pid = stream.peer_cred().ok().and_then(|c| c.pid());
        // (The account's own alone: the socket's folder is too, and this says so again.)
        if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) {
            return;
        }
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read);
        let mut line = String::new();
        let mut limited = (&mut lines).take(REQUEST_MOST as u64);
        let said = |answer: &Response| serde_json::to_string(answer).unwrap_or_default() + "\n";
        if limited.read_line(&mut line).await.is_err() || !line.ends_with('\n') {
            if line.len() >= REQUEST_MOST {
                let _ = write
                    .write_all(
                        format!("\n{}", said(&Response::err("", format!("a request of more than {} MiB (a long prompt goes in a file the agent is told to read)", REQUEST_MOST >> 20), 2))).as_bytes(),
                    )
                    .await;
            }
            return;
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            let _ = write.write_all(format!("\n{}", said(&Response::err("", "a request that isn't one", 2))).as_bytes()).await;
            return;
        };
        // Authenticate and gate the caller BEFORE protocol/later-build handover,
        // `follow`, `adopt`, `retire`, `ping`, or `policy` so a restricted caller
        // (minder, exited child, or child calling `adopt`) can never trigger a
        // service handover or reach service-wide operations. For `policy`,
        // authenticate strictly by the querying peer's PID (never `req.block`,
        // which names the subject pane Rex is asking about).
        let caller = if req.op == "policy" { self.caller_by_peer(peer_pid) } else { self.caller(&req, peer_pid).await };
        if let Some(refused) = self.gate_caller(&req, &caller) {
            let wire = if req.op == "follow" { said(&refused) } else { format!("\n{}", said(&refused)) };
            let _ = write.write_all(wire.as_bytes()).await;
            let _ = write.shutdown().await;
            return;
        }
        // An asker of a later build: its own service takes over. This
        // one finishes what it is doing, writes down what it knows and
        // ends; the children are the server's and go on. Later by what
        // it speaks, or by its file: a helper put on the host after this
        // one was (an upgrade that speaks the same) is asked through at
        // once, and a service that had no such word ran the old build for
        // as long as anything kept it busy: on a host whose agents never
        // rest, for good (2026-11-26).
        if req.proto > PROTO || peer_pid.is_some_and(asker_is_a_later_build) {
            match self.held() {
                None => {
                    let _ = write.write_all(format!("\n{}", said(&Response { ok: false, stdout: String::new(), stderr: String::new(), code: 75, data: json!({ "handover": true }) })).as_bytes()).await;
                    // (The asker's own helper is the one that takes over:
                    // started by this service as it ends.)
                    if let Some(theirs) = peer_pid.and_then(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).ok()).filter(|exe| successor_ok(exe).is_ok()) {
                        *self.successor.lock().unwrap_or_else(|e| e.into_inner()) = Some(theirs);
                    }
                    self.retire.notify_one();
                    return;
                }
                // Held (`held`): this service stays the one that
                // serves. A later build that speaks as this one does is
                // answered by it like any asker; one that speaks what
                // this one doesn't is told why it can't be.
                Some(why) if req.proto > PROTO => {
                    let msg = format!(
                        "this host's agents service is held at the build it runs, and a later Olo's `olo agent` speaks what that build doesn't: it is answered once the hold is lifted ({}: {why})",
                        self.hold_file().display()
                    );
                    let _ = write.write_all(format!("\n{}", said(&Response::err(&req.op, msg, 69))).as_bytes()).await;
                    return;
                }
                Some(why) => {
                    if !self.said_held.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        logged!("a later build of the helper asked, and was answered by this one, not handed the service: held ({}: {why})", self.hold_file().display());
                    }
                }
            }
        }
        if req.op == "follow" {
            // (Asked for before the picture is taken: nothing between the two is missed.)
            let mut frames = self.frames.subscribe();
            let first = self.snapshot();
            if write.write_all((serde_json::to_string(&first).unwrap_or_default() + "\n").as_bytes()).await.is_err() {
                return;
            }
            let mut gone = String::new();
            loop {
                tokio::select! {
                    frame = frames.recv() => {
                        let frame = match frame {
                            Ok(frame) => frame,
                            // (Fallen behind: everything again, under one lock.)
                            Err(broadcast::error::RecvError::Lagged(_)) => self.snapshot(),
                            Err(broadcast::error::RecvError::Closed) => return,
                        };
                        if write.write_all((serde_json::to_string(&frame).unwrap_or_default() + "\n").as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    // (The follower went, or closed its side.)
                    n = lines.read_line(&mut gone) => if n.unwrap_or(0) == 0 { return } else { gone.clear() },
                }
            }
        }
        // Taken: said before anything is done (`wire::TAKEN`).
        if write.write_all(b"\n").await.is_err() {
            return;
        }
        let waits = matches!(req.op.as_str(), "watch" | "wait");
        if !waits {
            self.lock().doing += 1;
        }
        // (An asker that goes away while a watch is waited for is let
        // go: nobody waits for it any more. What is done once is done to
        // its end, asker or no asker.)
        let mut gone = String::new();
        let answer = if waits {
            tokio::select! {
                answer = self.answer(&req, &caller) => Some(answer),
                _ = async { while lines.read_line(&mut gone).await.unwrap_or(0) > 0 { gone.clear(); } } => None,
            }
        } else {
            let answer = self.answer(&req, &caller).await;
            self.lock().doing -= 1;
            self.idle.notify_one();
            Some(answer)
        };
        if let Some(answer) = answer {
            let _ = write.write_all(said(&answer).as_bytes()).await;
            let _ = write.shutdown().await;
        }
    }

    async fn answer(self: &Arc<Self>, req: &Request, caller: &super::ops::Caller) -> Response {
        match req.op.as_str() {
            // (`hands_over`: it starts its successor itself, `retire --to`.)
            "ping" => Response::ok(String::new(), json!({ "ok": true, "proto": PROTO, "pid": std::process::id(), "cursor": self.lock().log.latest_seq(), "hands_over": true })),
            // (Held: only one that says it knows of the hold ends this
            // service; `tools/dev/host-upgrade --even-if-held`.)
            "retire" if !req.args.iter().any(|a| a == "--even-if-held") && self.held().is_some() => Response::err(
                "retire",
                format!("this host's agents service is held at the build it runs ({}: {}); `retire --even-if-held` ends it all the same", self.hold_file().display(), self.held().unwrap_or_default()),
                69,
            ),
            "retire" => {
                // (`--to HELPER`: its successor, started by this service
                // itself as it ends, so no other build's can start in
                // between.)
                let to = req.args.iter().position(|a| a == "--to").map(|i| req.args.get(i + 1).map(PathBuf::from));
                if let Some(to) = to {
                    let Some(to) = to else { return Response::err("retire", "--to requires the helper that takes over (its whole path)", 2) };
                    if let Err(why) = successor_ok(&to) {
                        return Response::err("retire", format!("--to {}: {why}", to.display()), 2);
                    }
                    *self.successor.lock().unwrap_or_else(|e| e.into_inner()) = Some(to);
                }
                self.retire.notify_one();
                Response::ok(String::new(), json!({ "ok": true }))
            }
            "adopt" => self.adopt(req.obj.as_ref()).await,
            "policy" => self.policy(req).await,
            _ => self.agent_op(req, caller).await,
        }
    }

    /// Which child a process (or the pane it names) is, and what that
    /// child may ask of the desktop: what a desktop asks before it does
    /// something for a pane it doesn't know as a child. The process's
    /// ancestors say whose tab it runs in, whatever pane it names.
    async fn policy(self: &Arc<Self>, req: &Request) -> Response {
        let pid = req.obj.as_ref().and_then(|o| o["pid"].as_i64()).map(|p| p as i32).filter(|p| *p > 1);
        let child = self.caller(req, pid).await.child;
        Response::ok(String::new(), json!({ "ok": true, "child": child.as_ref().map(|c| &c.id), "desktop": child.as_ref().map(|c| c.desktop_policy.describe()) }))
    }
}

/// The lock a running service holds: the file is locked for as long as
/// its descriptor lives.
fn take_lock(paths: &Paths) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd as _;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&paths.lock).ok()?;
    // SAFETY: a lock on a descriptor this function owns.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(format!("{} isn't a folder of this account's", dir.display())));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Service-private minder delivery state persisted across handovers so a
/// running minder keeps its per-subject cursors, rules generations, and
/// pending exit reports without replaying already-delivered events.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct MinderStateDisk {
    #[serde(default)]
    seen: Vec<(String, String, u64)>,
    #[serde(default)]
    minding_seen: Vec<(String, String, u64)>,
    #[serde(default)]
    rules_gen: HashMap<String, u64>,
    #[serde(default)]
    pending_rules_rev: Vec<(String, String, u64, String)>,
    #[serde(default)]
    pending_exit_reports: HashMap<String, Vec<String>>,
}

impl MinderStateDisk {
    fn from_state(st: &State) -> Self {
        Self {
            seen: st.seen.iter().map(|((m, a), s)| (m.clone(), a.clone(), *s)).collect(),
            minding_seen: st.minding_seen.iter().map(|((m, a), s)| (m.clone(), a.clone(), *s)).collect(),
            rules_gen: st.rules_gen.clone(),
            pending_rules_rev: st.pending_rules_rev.iter().map(|((m, a), (g, p))| (m.clone(), a.clone(), *g, p.clone())).collect(),
            pending_exit_reports: st.pending_exit_reports.iter().map(|(m, s)| (m.clone(), s.iter().cloned().collect())).collect(),
        }
    }

    fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save_to(&self, path: &std::path::Path) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);
        let Ok(json) = serde_json::to_string(self) else { return };
        let tmp = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)
            && f.write_all(json.as_bytes()).is_ok()
            && f.sync_all().is_ok()
        {
            drop(f);
            let _ = std::fs::rename(&tmp, path);
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn load_events(paths: &Paths, start_seq: u64) -> SubagentEventLog {
    let events: Vec<SubagentEvent> = std::fs::read_to_string(&paths.events).map(|all| all.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default();
    SubagentEventLog::restored(events, start_seq)
}

/// Runs the service for the Rex server at `server` until it has nothing
/// left to do. 0: ended in order, or another is running; 69: the server
/// couldn't be reached.
/// The lock's descriptor, where the service before this one handed it
/// down ([`start_successor`]): named in the environment.
const LOCK_FD: &str = "OLO_REX_AGENTS_LOCK_FD";

/// Whether a file may be the helper whose service takes over: a
/// program of this account's own, named as a helper is.
fn successor_ok(exe: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let meta = std::fs::metadata(exe).map_err(|e| e.to_string())?;
    let named = exe.is_absolute() && exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("olo-rex-link"));
    if !named || !meta.is_file() || meta.permissions().mode() & 0o111 == 0 || meta.uid() != unsafe { libc::geteuid() } {
        return Err("not a helper of this account's own (a program named olo-rex-link…, by its whole path)".to_owned());
    }
    Ok(())
}

/// Starts the service that takes over from this one, as this one ends:
/// `exe agent --server SERVER serve`, in a session of its own, with
/// **this service's lock in its hand** (the lock's own descriptor, left
/// open across the exec in the child alone and named in [`LOCK_FD`]:
/// the lock is on the open file, so it is never free for another to
/// take) and its input a
/// pipe this service holds open until it has ended: the successor
/// begins when that closes. Without this a service that retired just
/// went, and the next one was whichever build asked first: on a host
/// with thirty watchers begun under earlier builds, a handover took two
/// minutes and six services, of three builds in turn (2026-11-27).
/// What is returned is that pipe's end: kept until this process ends.
fn start_successor(exe: &std::path::Path, server: &std::path::Path, paths: &Paths, lock: &std::fs::File) -> std::io::Result<std::process::ChildStdin> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::os::unix::process::CommandExt as _;
    let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&paths.log)?;
    let handed = lock.as_raw_fd();
    let mut command = std::process::Command::new(exe);
    command.args(["agent", "--server"]).arg(server).arg("serve").env(LOCK_FD, handed.to_string()).stdin(std::process::Stdio::piped()).stdout(log.try_clone()?).stderr(log).current_dir("/");
    // SAFETY: between fork and exec, two calls that are safe there: a
    // session of its own, and the lock's descriptor (the same number in
    // the child) no longer closed at the exec. In the child alone: no
    // other program this service might start is ever handed it.
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            if libc::fcntl(handed, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()?.stdin.take().ok_or_else(|| std::io::Error::other("its input couldn't be held"))
}

/// The lock the service before this one handed down, where it did: the
/// descriptor [`LOCK_FD`] names, taken only if it is this service's own
/// lock file (anything else named there is left alone).
fn handed_lock(paths: &Paths) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::fs::MetadataExt as _;
    let fd: i32 = std::env::var(LOCK_FD).ok()?.parse().ok()?;
    // SAFETY: before the runtime has any other thread; nothing else reads the environment now.
    unsafe { std::env::remove_var(LOCK_FD) };
    let ours = std::fs::metadata(&paths.lock).ok()?;
    // SAFETY: `fstat` on a number; nothing is owned until it is seen to be the lock.
    let theirs = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        (libc::fstat(fd, &mut st) == 0).then_some(st)
    }?;
    if theirs.st_dev as u64 != ours.dev() || theirs.st_ino as u64 != ours.ino() {
        return None;
    }
    // SAFETY: the descriptor is this process's, handed down for this.
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        Some(std::fs::File::from_raw_fd(fd))
    }
}

/// `SIGTERM` and `SIGINT` held back from here on (asked before the
/// runtime starts, so every thread of it holds them back too), and a
/// descriptor they are read from instead, with who sent them
/// (`signalfd`). The service's log said "told to end" of a signal and
/// nothing of its sender; one night it was ended so by nobody that could
/// be found afterwards (2026-11-27, 05:01:08 UTC: every sender in Olo's
/// own code and in Rex's was ruled out, and the signal's own record of
/// its sender had been thrown away).
pub fn end_signals() -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd as _;
    // SAFETY: a signal set of this function's own, and two calls on it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return None;
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if fd < 0 {
            // (Not held back after all: they end it as they did.)
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            return None;
        }
        Some(std::os::fd::OwnedFd::from_raw_fd(fd))
    }
}

/// A signal as the log has it: which, and its sender (its pid, what it
/// is and where it runs, read at once: it may be gone a moment later).
fn signal_said(info: &libc::signalfd_siginfo) -> String {
    let which = match info.ssi_signo as i32 {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        _ => "a signal",
    };
    let pid = info.ssi_pid;
    if pid == 0 {
        return format!("{which} from the kernel (code {})", info.ssi_code);
    }
    let read = |what: &str| std::fs::read(format!("/proc/{pid}/{what}")).ok();
    let command = read("cmdline").map(|b| b.split(|c| *c == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join(" ")).filter(|c| !c.is_empty());
    let command = command.map_or_else(|| "a process that has gone".to_owned(), |c| c.chars().take(200).collect());
    let cgroup = read("cgroup").and_then(|b| String::from_utf8_lossy(&b).lines().find_map(|l| l.strip_prefix("0::").map(str::to_owned))).map(|c| format!(", in {c}")).unwrap_or_default();
    format!("{which} from pid {pid} (uid {}): {command}{cgroup}", info.ssi_uid)
}

pub async fn serve(server: PathBuf, paths: Paths, signals: Option<std::os::fd::OwnedFd>) -> i32 {
    if let Err(e) = private_dir(&paths.dir).and_then(|()| paths.socket.parent().map_or(Ok(()), private_dir)) {
        logged!("olo agents: {e}");
        return 1;
    }
    // One service at a time. Another's lock is waited for a moment: it
    // may be on its way out (handing over, or with nothing left to do),
    // and whoever started this one is waiting for a service to answer.
    // Handed its lock by the service before it (`start_successor`): it
    // is the one that serves next, and begins when that one has ended
    // (its input, a pipe that one holds open, closes), not before: two
    // would write the same record.
    let waited = Instant::now();
    let lock = if let Some(lock) = handed_lock(&paths) {
        let before = tokio::task::spawn_blocking(|| {
            use std::io::Read as _;
            let mut said = [0u8; 64];
            while std::io::stdin().read(&mut said).is_ok_and(|n| n > 0) {}
        });
        let _ = tokio::time::timeout(Duration::from_secs(60), before).await;
        lock
    } else {
        loop {
            if let Some(lock) = take_lock(&paths) {
                break lock;
            }
            // (It serves: this one has nothing to do.)
            if std::os::unix::net::UnixStream::connect(&paths.socket).is_ok() || waited.elapsed() > Duration::from_secs(20) {
                return 0;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let (rex, mut events) = match Rex::connect(&server).await {
        Ok(connected) => connected,
        Err(e) => {
            logged!("olo agents: the Rex server at {} couldn't be reached: {e}", server.display());
            return 69;
        }
    };
    // (Every session's windows coming and going; a child's own block's
    // events come with following its session.)
    if let Err(e) = rex.call("session.attach_all", json!({ "event_scope": "session" })).await {
        logged!("olo agents: the Rex server wouldn't say what happens in its sessions: {e}");
        return 69;
    }
    let reg = match SubagentRegistry::read_from(&paths.registry) {
        Ok(reg) => reg,
        Err(why) => {
            // (Said, and no service: whoever asks is told it can't be
            // asked, with this word, and is never told "no children".)
            logged!("olo agents: {why}");
            return 69;
        }
    };
    let log = load_events(&paths, reg.event_seq);
    let minder_disk = MinderStateDisk::load_from(&paths.dir.join("minder-state.json"));
    let _ = std::fs::remove_file(&paths.socket);
    let listener = match UnixListener::bind(&paths.socket) {
        Ok(listener) => listener,
        Err(e) => {
            logged!("olo agents: couldn't listen at {}: {e}", paths.socket.display());
            return 1;
        }
    };
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
    }
    // (When it began, and which build it is: what a handover is told by.)
    logged!("olo agents: serving (pid {}, {})", std::process::id(), std::env::current_exe().ok().and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default());
    let (frames, _) = broadcast::channel(1024);
    let service = Arc::new(Service {
        rex,
        paths: paths.clone(),
        st: Mutex::new(State {
            reg,
            log,
            live: HashMap::new(),
            sessions: HashMap::new(),
            clients: 0,
            doing: 0,
            idle_since: None,
            let_go: HashMap::new(),
            closing_minders: HashSet::new(),
            unhealthy_minders: HashSet::new(),
            empty_gen: HashMap::new(),
            reserved_ids: HashSet::new(),
            pending_names: HashSet::new(),
            told: HashMap::new(),
            rules_gen: minder_disk.rules_gen,
            pending_rules_rev: minder_disk.pending_rules_rev.into_iter().map(|(m, a, g, p)| ((m, a), (g, p))).collect(),
            seen: minder_disk.seen.into_iter().map(|(m, a, s)| ((m, a), s)).collect(),
            minding_seen: minder_disk.minding_seen.into_iter().map(|(m, a, s)| ((m, a), s)).collect(),
            pending_exit_reports: minder_disk.pending_exit_reports.into_iter().map(|(m, v)| (m, v.into_iter().collect())).collect(),
            pending_minds: HashMap::new(),
            leave: HashMap::new(),
        }),
        changed: Notify::new(),
        frames,
        text_watchers: AtomicUsize::new(0),
        save: Notify::new(),
        idle: Notify::new(),
        retire: Notify::new(),
        successor: std::sync::Mutex::new(None),
        said_held: std::sync::atomic::AtomicBool::new(false),
        minder_gate: tokio::sync::Mutex::new(()),
        worker_locks: Mutex::new(HashMap::new()),
    });
    tokio::spawn(service.clone().writer());
    service.reconcile().await;

    // What ends it: the server going, being asked to hand over, a
    // signal to end, or nothing left to do for a while.
    let (ended_tx, mut ended) = mpsc::unbounded_channel::<&'static str>();
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if let Event::Closed(why) = &event {
                    logged!("olo agents: the Rex server's connection ended: {why}");
                    let _ = ended.send("the Rex server went away");
                    return;
                }
                service.on_event(event).await;
            }
            let _ = ended.send("the Rex server went away");
        });
    }
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            service.retire.notified().await;
            let _ = ended.send("handed over");
        });
    }
    {
        let ended = ended_tx.clone();
        tokio::spawn(async move {
            // Read with its sender, where the signals are held back for
            // that (`end_signals`); as any program hears them otherwise.
            // SAFETY: the descriptor is this task's own from here to its
            // end (moved into it), and is closed only when what watches
            // it is dropped.
            if let Some(fd) = signals.and_then(|fd| unsafe { tokio::io::unix::AsyncFd::register(fd) }.ok()) {
                use std::os::fd::AsRawFd as _;
                loop {
                    let Ok(mut ready) = fd.readable().await else { return };
                    // SAFETY: one record of the kernel's own size, read into a zeroed one.
                    let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
                    let size = std::mem::size_of::<libc::signalfd_siginfo>();
                    let read = unsafe { libc::read(fd.get_ref().as_raw_fd(), (&raw mut info).cast(), size) };
                    if read == size as isize {
                        logged!("olo agents: told to end by {}", signal_said(&info));
                        let _ = ended.send("told to end");
                        return;
                    }
                    ready.clear_ready();
                }
            }
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else { return };
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            let _ = ended.send("told to end");
        });
    }
    {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            loop {
                let since = {
                    let mut st = service.lock();
                    // (A child that ended and left its tab open isn't work.)
                    let busy = st.clients > 0 || st.reg.entries.iter().any(|e| e.exited_at.is_none());
                    if busy {
                        st.idle_since = None;
                    } else if st.idle_since.is_none() {
                        st.idle_since = Some(Instant::now());
                    }
                    st.idle_since
                };
                match since {
                    // (Busy: nothing to wait for but the word that it no longer is.)
                    None => service.idle.notified().await,
                    Some(since) if since.elapsed() >= IDLE_EXIT => {
                        let _ = ended.send("nothing left to do");
                        return;
                    }
                    Some(since) => {
                        tokio::select! {
                            _ = tokio::time::sleep(IDLE_EXIT.saturating_sub(since.elapsed())) => {}
                            _ = service.idle.notified() => {}
                        }
                    }
                }
            }
        });
    }
    let why = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let service = service.clone();
                tokio::spawn(async move {
                    {
                        let mut st = service.lock();
                        st.clients += 1;
                        st.idle_since = None;
                    }
                    service.clone().serve_one(stream).await;
                    service.lock().clients -= 1;
                    service.idle.notify_one();
                });
            }
            why = ended.recv() => break why.unwrap_or("ended"),
        }
    };
    // No more is taken (its socket goes, so the next asker starts
    // another at once); what is being done is done to its end (a tab
    // being started is written down, a key being typed is typed), a
    // while at most; and what it knows is written before it goes.
    // Its successor, where it hands over to one: started now, this
    // service's lock in its hand, and waiting for this one's end.
    let heir = service.successor.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|_| why == "handed over");
    let _until_ended = heir.and_then(|exe| match start_successor(&exe, &server, &paths, &lock) {
        Ok(held) => {
            logged!("olo agents: its successor is started ({}): it has this service's lock, and serves once this one has ended", exe.display());
            Some(held)
        }
        Err(e) => {
            logged!("olo agents: its successor ({}) couldn't be started ({e}): whoever asks next starts a service", exe.display());
            None
        }
    });
    drop(listener);
    let _ = std::fs::remove_file(&paths.socket);
    let finishing = Instant::now();
    while service.lock().doing > 0 && finishing.elapsed() < Duration::from_secs(20) {
        tokio::select! {
            _ = service.idle.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    service.write_now().await;
    logged!("olo agents: ending ({why})");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log's time is the calendar's: the first second, a leap day,
    /// and the handover its want was found at.
    #[test]
    fn the_logs_time_is_the_calendars() {
        assert_eq!(stamp_of(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp_of(1_709_208_000), "2024-02-29 12:00:00Z");
        assert_eq!(stamp_of(1_795_743_906), "2026-11-27 01:45:06Z");
        assert_eq!(stamp_of(1_798_761_599), "2026-12-31 23:59:59Z");
    }
}
