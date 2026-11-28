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
    /// Whether a feedback survey covered this reading (preserved on cached reads).
    survey: bool,
    /// Monotonic sequence number of the live screen RPC that produced this reading.
    seq: u64,
}

/// A child started with `--trust`: its own folders' trust questions in
/// the fifteen seconds of its start are the service's to answer, and
/// nobody's to be told of meanwhile. Held from the moment its record is
/// made, before its screen is first read (the answering task comes to a
/// question a moment after the reading that first shows it: on a busy
/// host that reading had told it as a question, and `run --trust` came
/// back "question" for one answered a moment later. 2026-11-27).
pub(super) struct TrustAtStart {
    /// Its primary launch folder (`entry.cwd`), trimmed of trailing `/`.
    primary: Option<String>,
    /// Its launch `--add-dir` folders, trimmed of trailing `/`.
    add_dirs: Vec<String>,
    /// The questions whose Return was submitted or whose attempt ended
    /// ([`TrustAtStart::which`]): one still standing after that is a person's.
    answered: HashSet<Option<String>>,
    /// Keys are on their way just now.
    answering: bool,
    until: Instant,
}

/// Normalizes a directory path for startup `--trust` matching while preserving
/// the root directory `"/"` (never collapsing `"/"` to `""`).
pub(super) fn norm_dir(d: &str) -> Option<String> {
    if d.is_empty() {
        return None;
    }
    let trimmed = d.trim_end_matches('/');
    Some(if trimmed.is_empty() { "/".to_owned() } else { trimmed.to_owned() })
}

impl TrustAtStart {
    fn of(entry: &NativeSubagent, until: Instant) -> Self {
        let primary = entry.cwd.as_deref().and_then(norm_dir);
        let add_dirs = entry.add_dirs.iter().filter_map(|d| norm_dir(d)).collect();
        Self { primary, add_dirs, answered: HashSet::new(), answering: false, until }
    }

    /// Which question a reading shows: the one at the child's start
    /// (None: one of them, whether or not its folder's path is drawn
    /// yet. Read with no path and then with one, it was two questions
    /// and answered twice), or an added folder's (its path).
    pub(super) fn which(asked: &olo_agent_screen::FolderTrustPrompt) -> Option<String> {
        asked.is_add_dir.then(|| asked.folder.as_deref().and_then(norm_dir).unwrap_or_default())
    }

    /// The question is about one of the child's own launch folders.
    /// - An `--add-dir` prompt (`is_add_dir`) MUST show a drawn path matching
    ///   one of the child's launch folders (`add_dirs` or `primary`); a
    ///   pathless partial add-dir frame is never claimed.
    /// - A primary prompt (`!is_add_dir`) with a drawn path MUST match the
    ///   launch `primary` directory (never a foreign folder). A pathless
    ///   primary prompt requires `primary.is_some()` (and the trust worker
    ///   additionally verifies process-cwd binding before confirming).
    pub(super) fn owns_parts(primary: Option<&str>, add_dirs: &[String], asked: &olo_agent_screen::FolderTrustPrompt) -> bool {
        let p_norm = primary.and_then(norm_dir);
        if asked.is_add_dir {
            asked.folder.as_deref().and_then(norm_dir).is_some_and(|f| add_dirs.iter().filter_map(|a| norm_dir(a)).any(|a| a == f) || p_norm.as_deref() == Some(f.as_str()))
        } else {
            match asked.folder.as_deref().and_then(norm_dir) {
                Some(f) => p_norm.as_deref() == Some(f.as_str()),
                None => p_norm.is_some(),
            }
        }
    }

    pub(super) fn owns(&self, asked: &olo_agent_screen::FolderTrustPrompt) -> bool {
        Self::owns_parts(self.primary.as_deref(), &self.add_dirs, asked)
    }

    /// Partial `--add-dir` frame whose path has not drawn yet while this child
    /// has launch `--add-dir` directories and startup trust is still within its
    /// deadline: hold back delivery until the path draws or the deadline expires,
    /// without claiming or pressing Return on the pathless frame.
    fn pending_path(&self, asked: &olo_agent_screen::FolderTrustPrompt) -> bool {
        asked.is_add_dir && asked.folder.is_none() && !self.add_dirs.is_empty() && Instant::now() < self.until
    }

    /// This question is still the service's to answer, or its keys are
    /// on their way: not to be told. Always bounded by `self.until`.
    pub(super) fn on_its_way(&self, asked: &olo_agent_screen::FolderTrustPrompt) -> bool {
        if Instant::now() >= self.until {
            return false;
        }
        (self.owns(asked) && (self.answering || !self.answered.contains(&Self::which(asked)))) || self.pending_path(asked)
    }
}

/// Explicit state machine for feedback-survey dismissal on a child's screen.
/// Replaces loose `survey_pressed`/`survey_stays` flags so:
/// - Only the matching pre-submission owner token releases a `Claimed` slot.
/// - Transitioning to `Submitted` BEFORE `rex.write().await` ensures timeout or
///   cancellation never double-presses `0`.
/// - Partial survey frames (question without choices) NEVER clear an active state.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SurveyState {
    Idle,
    Claimed { token: u64 },
    Submitted { token: u64, since: Instant, submitted_at_seq: u64 },
    Stays,
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
    pub(super) given: u64,
    /// A rest of its is kept back and will be looked at again (one
    /// timer, not one for each look: `transition`'s confirmation).
    relook: bool,
    pub exit_code: Option<i32>,
    /// The terminal's own process (the shell the server started).
    pub pid: Option<i32>,
    rows: u16,
    pub last_state: Option<SubagentLiveState>,
    pub(super) last_key: Option<String>,
    check_gen: u64,
    /// Monotonic generation advanced whenever an answer is committed (`approve`
    /// or `send`): tags `latest_event` outside the 2,000-event log ring so
    /// pre-submission confirmations can never masquerade as post-answer events
    /// and ring eviction never duplicates a confirmed post-answer event.
    pub(super) event_gen: u64,
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
    /// Notified immediately by the stream reader on any PTY output, resize,
    /// stream connect, or exit for THIS child (zero stationary polling).
    output_notify: Arc<Notify>,
    /// Notified on per-child authority/box/trust/list/exit state changes so
    /// in-flight writers wake immediately without polling.
    state_notify: Arc<Notify>,
    /// Notified when an active survey dismissal (`Claimed`/`Submitted`) finishes
    /// or transitions to `Stays`, waking waiting `send` callers.
    survey_notify: Arc<Notify>,
    /// True while the child's Rex terminal stream is connected and delivering
    /// `output_notify` wakeups.
    stream_connected: bool,
    /// Monotonic counter incremented whenever `stream_connected` changes so
    /// wait loops immediately wake on mid-wait disconnect and arm fallback reads.
    stream_gen: u64,
    /// True once durable registration has succeeded (suppresses public follower
    /// `Frame::Agent` broadcasts, event recording, and premature `exit()` during
    /// pre-commit `begin_attached` persistence so a rolled-back child never leaks).
    pub(super) committed: bool,
    /// Deferred process exit code observed while `!committed` (replayed once
    /// `begin_attached` commits, discarded on rollback).
    pending_exit_code: Option<i32>,
    /// Deferred tab closure observed while `!committed` (replayed once
    /// `begin_attached` commits, discarded on rollback).
    pending_tab_closed: bool,
    /// True while a non-Working state transition awaits confirmation and has not
    /// yet recorded a confirmed event (prevents transient RPC failure or
    /// superseded confirmation reads from stranding a standing question).
    pub(super) unconfirmed: bool,
    /// `check_gen` of the currently sleeping confirmation task, if one is active,
    /// so blinking screens don't cancel a healthy in-flight confirmation while
    /// failed or superseded confirmations clear this to `None` and re-arm.
    confirm_in_flight: Option<u64>,
    /// Background tasks for this child (stream consumer + transition runner).
    tasks: Vec<tokio::task::AbortHandle>,
    /// What followers were last told of it.
    told: Option<(SubagentLiveState, Option<String>)>,
    /// Explicit feedback-survey dismissal state machine.
    survey: SurveyState,
    /// Monotonic owner token generator for `SurveyState::Claimed`.
    survey_seq: u64,
    /// Claude Code's offer to send a feedback report it drafted stands
    /// on its screen, and whoever looks after the child has been told
    /// of this showing of it (`olo_agent_screen::feedback_draft`).
    draft_told: bool,
    /// The approval box that stands (`state::box_hash`), its number among
    /// this child's boxes, and whether an answer has already been written
    /// for this showing (`BoxNow::answered`); none while none stands or
    /// once it was finished here. Said by the readings, in `look`.
    box_now: Option<BoxNow>,
    boxes: u64,
    /// Highest answered box number retired by a positive ordered `elsewhere`
    /// reading (so an `approve` dismissal wait always observes `BoxEnd::Gone`
    /// even if a concurrent reader saw `Working` and a blank frame followed).
    last_elsewhere_box: u64,
    /// True while `approve` is actively navigating choices (`delta != 0`):
    /// prevents a concurrent `look` that catches a transient mid-repaint
    /// frame from wiping `box_now`.
    navigating: bool,
    /// A box showing the list must not auto-answer: one whose Return
    /// didn't clearly dismiss (`BoxEnd::Blank`/`Stays`), or one that stood
    /// when this service took the child over. Kept for the entire showing
    /// with NO wall-clock expiry; cleared only by a genuinely different
    /// complete box or an ordered, positively `elsewhere` reading.
    list_skip: Option<ListSkip>,
    /// True for a child reconciled or adopted from a previous service until
    /// resolved on an ordered, conclusive live frame (standing box placed in
    /// `list_skip`, standing survey marked `Stays`, or positive `elsewhere`).
    /// Never derived from wall-clock `started_at < began_ms`.
    adopted_pending: bool,
    /// The readings of its screen, in the order their RPCs were begun, and
    /// the last of them accepted: which box stands is what the latest
    /// accepted reading says, never an out-of-order or failed read.
    pub(super) read_seq: u64,
    pub(super) accepted_seq: u64,
    /// Explicit handoff flag kept true after `box_finish`, trust release, or
    /// relinquished survey claim until a live ordered reread publishes the
    /// standing prompt or proves it gone (with bounded retry on failed reads).
    pending_handoff: bool,
    handoff_failures: u8,
    handoff_retry_armed: bool,
    /// One writer of answers into it ([`Service::answer_lock`]).
    answer_lock: Arc<tokio::sync::Mutex<()>>,
    /// Its own folders' trust is the service's to answer (`--trust`, the
    /// fifteen seconds of its start): [`TrustAtStart`].
    trust: Option<TrustAtStart>,
    /// Latest confirmed state event recorded for this live child, kept
    /// independently of the 2,000-event log ring so a standing question
    /// is never lost when other host events evict the ring.
    pub latest_event: Option<SubagentEvent>,
    /// The `event_gen` at which `latest_event` was confirmed and recorded.
    pub(super) latest_event_gen: u64,
    /// Adopted pre-handover approval box `(number, hash32)` restored from the
    /// event log in `reconcile()` so a standing box #2+ keeps its original
    /// content dedupe key (`box:b{n}-{hash}`) across handover while minting a
    /// fresh generation-specific `--box` key (Astra R2 #5).
    adopted_box: Option<(u64, u32)>,
    /// Consecutive standing-probe RPC failures and earliest backoff retry instant
    /// so a persistently unreadable pane never turns `watch` into a 250 ms spin
    /// loop or starves healthy siblings (Astra R2 #6 & #21).
    pub(super) standing_fails: u8,
    pub(super) standing_retry_after: Option<tokio::time::Instant>,
}

/// Generates a fresh non-zero per-process random 32-bit incarnation token
/// from `/dev/urandom` (falling back to PID + timestamp hash) so box keys can
/// never collide across two service processes even if an older helper drops
/// `service_gen` from `registry.json`.
fn new_service_incarnation() -> u32 {
    use std::io::Read as _;
    static FALLBACK: AtomicUsize = AtomicUsize::new(1);
    let mut buf = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(&mut buf).is_ok()
    {
        let v = u32::from_le_bytes(buf);
        if v != 0 {
            return v;
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let n = FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u64;
    let mixed = (std::process::id() as u64).wrapping_mul(0x9e37_79b9) ^ nanos ^ (nanos >> 32) ^ n.wrapping_mul(0x85eb_ca6b);
    (mixed as u32).max(1)
}

/// The approval box currently standing on a child's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoxNow {
    hash: u64,
    prompt_hash: u64,
    choice_count: usize,
    number: u64,
    service_gen: u64,
    incarnation: u32,
    answered: bool,
}

impl BoxNow {
    fn key(self) -> String {
        match (self.service_gen, self.incarnation) {
            (0, 0) => format!("b{}-{:08x}", self.number, self.hash as u32),
            (g, 0) => format!("b{}-g{}-{:08x}", self.number, g, self.hash as u32),
            (g, inc) => format!("b{}-g{}-{:08x}-{:08x}", self.number, g, inc, self.hash as u32),
        }
    }

    fn as_looked(self) -> LookedBox {
        LookedBox { key: self.key(), number: self.number, hash: self.hash, answered: self.answered }
    }
}

/// Exact approval-box instance reported by [`Looked::boxed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookedBox {
    pub key: String,
    pub number: u64,
    pub hash: u64,
    pub answered: bool,
}

/// A showing-scoped list suppression entry with no wall-clock expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListSkip {
    hash: u64,
    prompt_hash: u64,
}

impl ListSkip {
    fn from_box(b: BoxNow) -> Self {
        Self { hash: b.hash, prompt_hash: b.prompt_hash }
    }

    fn matches(self, ap: &olo_agent_screen::ApprovalPrompt) -> bool {
        let h = state::box_hash(ap);
        let ph = state::prompt_hash(ap);
        // Suppress the same submitted request across choice-count changes in
        // EITHER direction (e.g., 2->3 or 3->2 redraw) until positive dismissal
        // or a genuinely different prompt.
        h == self.hash || ph == self.prompt_hash
    }
}

/// How an answered box was seen to end ([`Service::box_finish`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxEnd {
    /// Another box in its place, its agent at work in its turn or at
    /// its own prompt, its tab gone, or the refusal reason field shown.
    Gone,
    /// No box on the screen for a moment and nothing to say where its
    /// agent is: gone, or a repaint that is slow (a busy host). The
    /// screen can't say which, so the very same box seen next, with
    /// nothing else seen first, is a person's and not the list's.
    Blank,
    /// It still stands, its two seconds up; or its Return wasn't seen
    /// to be written.
    Stays,
}

/// True when `cls` positively proves the agent is outside any approval,
/// folder-trust, approval fragment, or interactive question dialog — genuinely
/// running a turn or standing at its own chat prompt (never an empty/blank
/// frame, partial dialog repaint, or bare `esc to interrupt` footer).
pub(super) fn is_positively_elsewhere(cls: &ScreenClassification) -> bool {
    cls.approval.is_none()
        && cls.folder_trust.is_none()
        && !cls.approval_fragment
        && cls.state != SubagentLiveState::WaitingQuestion
        && (cls.in_turn || cls.summary.as_deref() == Some(state::AT_PROMPT))
}

/// The box `ap` is, where it is the one held as standing: its key, number,
/// hash and whether its answer has been written. Read-only.
fn peek_box(live: &Live, ap: Option<&olo_agent_screen::ApprovalPrompt>) -> Option<LookedBox> {
    let what = state::box_hash(ap?);
    live.box_now.filter(|b| b.hash == what).map(BoxNow::as_looked)
}

impl Live {
    /// Which box stands after a reading taken in its order (`ap`: the
    /// box it shows, or none), and that box's exact [`LookedBox`].
    /// Returns `(looked_box, retired_answered_elsewhere)`.
    ///
    /// - The same box keeps its number; a repaint of the same answered prompt
    ///   (`prompt_hash` match, regardless of whether choice count shrank or
    ///   grew 2->3 / 3->2 / 3->3) keeps the answered box intact until positive
    ///   dismissal (`elsewhere == true`) or a genuinely different prompt.
    /// - A genuinely different box gets the next number and clears `list_skip`.
    /// - On an ordered, positively `elsewhere` reading (`ap == None, elsewhere == true`),
    ///   `box_now` is ALWAYS retired (even during `navigating`, since positive
    ///   dismissal is proven!), recording `last_elsewhere_box` and clearing
    ///   `list_skip` so a genuine identical successor box gets a fresh number.
    /// - `adopted_pending` is resolved strictly here on ordered, conclusive
    ///   frames (never on blank/ambiguous repaints or wall-clock comparisons).
    fn box_after(&mut self, ap: Option<&olo_agent_screen::ApprovalPrompt>, elsewhere: bool, service_gen: u64, incarnation: u32) -> (Option<LookedBox>, bool) {
        let Some(ap) = ap else {
            let mut retired_answered = false;
            if elsewhere {
                if let Some(cur) = self.box_now.take()
                    && cur.answered
                {
                    self.last_elsewhere_box = self.last_elsewhere_box.max(cur.number);
                    retired_answered = true;
                }
                self.list_skip = None;
                self.adopted_box = None;
                if self.adopted_pending && self.survey == SurveyState::Idle {
                    self.adopted_pending = false;
                }
            } else if !self.navigating && !self.box_now.is_some_and(|b| b.answered) {
                self.box_now = None;
            }
            return (None, retired_answered);
        };
        let what = state::box_hash(ap);
        let ph = state::prompt_hash(ap);
        let choices = ap.choices.len();
        let adopted = self.adopted_pending;
        if self.list_skip.is_some_and(|skip| !skip.matches(ap)) {
            self.list_skip = None;
        }
        let cur = match self.box_now {
            Some(cur) if cur.hash == what => cur,
            Some(cur) if cur.answered && cur.prompt_hash == ph => cur,
            _ => {
                let number = match self.adopted_box.take() {
                    Some((ad_n, ad_hash)) if ad_hash == (what as u32) => {
                        self.boxes = self.boxes.max(ad_n);
                        ad_n
                    }
                    Some((ad_n, _)) => {
                        self.boxes = self.boxes.max(ad_n) + 1;
                        self.boxes
                    }
                    None => {
                        self.boxes += 1;
                        self.boxes
                    }
                };
                let cur = BoxNow { hash: what, prompt_hash: ph, choice_count: choices, number, service_gen, incarnation, answered: false };
                self.box_now = Some(cur);
                if adopted {
                    self.list_skip = Some(ListSkip::from_box(cur));
                }
                cur
            }
        };
        if adopted {
            self.adopted_pending = false;
        }
        (Some(cur.as_looked()), false)
    }

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
            event_gen: 0,
            seen_working: false,
            expects: None,
            answers: None,
            held_until: None,
            poke: Arc::new(Notify::new()),
            output_notify: Arc::new(Notify::new()),
            state_notify: Arc::new(Notify::new()),
            survey_notify: Arc::new(Notify::new()),
            stream_connected: false,
            stream_gen: 0,
            committed: true,
            pending_exit_code: None,
            pending_tab_closed: false,
            unconfirmed: false,
            confirm_in_flight: None,
            tasks: Vec::new(),
            told: None,
            survey: SurveyState::Idle,
            survey_seq: 0,
            draft_told: false,
            box_now: None,
            boxes: 0,
            last_elsewhere_box: 0,
            navigating: false,
            list_skip: None,
            adopted_pending: false,
            read_seq: 0,
            accepted_seq: 0,
            pending_handoff: false,
            handoff_failures: 0,
            handoff_retry_armed: false,
            answer_lock: Arc::new(tokio::sync::Mutex::new(())),
            trust: None,
            latest_event: None,
            latest_event_gen: 0,
            adopted_box: None,
            standing_fails: 0,
            standing_retry_after: None,
        }
    }

    pub(super) fn survey_active_locked(&self) -> bool {
        self.survey != SurveyState::Idle
    }

    fn activity(&self) -> (Activity, Option<String>) {
        self.statuses.iter().max_by_key(|(_, a, _)| a.rank()).map(|(_, a, msg)| (*a, msg.clone())).unwrap_or_default()
    }
}

/// What a minder was last told of an agent's standing question or
/// approval: the exact box instance key (`b{n}-g{gen}-{inc:08x}-{hash:08x}`)
/// for approvals, or question key (`q:...` / `tx:waiting`) for non-approval
/// questions, bound to the agent's `rules_gen` and reading sequence `read_seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToldGrant {
    pub token: String,
    pub is_approval: bool,
    pub rules_gen: u64,
    pub read_seq: u64,
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
    /// In-flight `watch` / `wait` requests currently executing so handover
    /// shutdown cancels and drains them before the final durable save.
    active_watches: usize,
    /// Set atomically when retirement/handover begins: checked under
    /// `self.lock()` right alongside `doing += 1` / `active_watches += 1`
    /// BEFORE writing `TAKEN` (`\n`) so late connections get `NotTaken`
    /// and retry cleanly on the successor.
    pub retiring: bool,
    /// Set atomically after active requests drain and background tasks are
    /// aborted, right before `stop_writer` and `final_save`, so no detached
    /// background task can mutate state or write to disk/PTY across the final
    /// durable handover snapshot.
    pub frozen: bool,
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
    /// Provisional entries staged in `reg.entries` while awaiting durable
    /// commit (`begin_attached` or `adopt`): excluded from unrelated registry
    /// snapshots and follower snapshots so another writer cannot publish an
    /// entry that later rolls back.
    provisional_ids: HashSet<String>,
    /// Live recovery entries retained in memory after a failed close (`!tab_closed`)
    /// whose initial save failed: addressable in memory by ID, yet still tracked
    /// as unsaved until a subsequent `write_with_commit` persists them so a
    /// duplicate `adopt` cannot skip persistence while `registry.json` is unwritable.
    unsaved_recovery_ids: HashSet<String>,
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
    /// By `(watcher_key, agent_id)`: the highest `WaitingApproval` /
    /// `WaitingQuestion` event seq of `agent_id` already delivered to a
    /// non-service-minder watcher (`tab-minder` or lead) via `watch`. Used so
    /// `watch --since` immediately catches any standing unanswered prompt whose
    /// event fell at or before `since` (e.g., two back-to-back starts or a
    /// cursor bumped by `status`) without ever spin-looping on an already-seen
    /// standing prompt.
    pub watcher_seen: HashMap<(String, String), u64>,
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
    /// A feedback survey is drawn over it (still dismissing or refusing to go):
    /// what is read may be covered by it. Preserved on cached reads too.
    pub survey: bool,
    /// True while another reader owns an active survey dismissal (`Claimed` or
    /// `Submitted`) so input-producing callers (`send`) wait or refuse cleanly.
    pub survey_pending: bool,
    /// The approval box this reading shows, as the service knows it:
    /// its key (`b{n}-g{gen}-{inc:08x}-{hash:08x}`), number, hash, and whether
    /// its answer has been written. None where it shows no box or one not held
    /// as standing.
    pub boxed: Option<LookedBox>,
    /// What the child had been given and not yet been seen at when the
    /// reading began.
    pub expected: Option<u64>,
    /// Whether its program says what it is doing itself (an OSC 7501
    /// record of its own).
    pub reports: bool,
    /// What it has been given and not yet been seen at, as that stands
    /// now (`state::of_the_turn_given`).
    pub expects: Option<state::Expected>,
    /// True ONLY when a fresh Rex screen RPC succeeded during this `look()` call.
    pub live: bool,
    /// True ONLY when `live == true` AND this reading's RPC sequence was strictly
    /// newer than `accepted_seq` (not superseded by a newer reading).
    /// Key-writing paths (`approve`, `trust`, `send`, `--reason`) require `live && ordered`.
    pub ordered: bool,
    /// True when a fresh screen RPC failed transiently (never mistaken for a cache hit
    /// or a genuine `Working("starting")` transition).
    pub rpc_failed: bool,
    /// Monotonic sequence number of the reading (RPC seq on live reads, cached
    /// `Reading::seq` on cache hits, 0 on exited or uncached RPC failures).
    pub seq: u64,
    /// Child `out_seq` sampled immediately BEFORE the screen RPC began (so
    /// input dispatchers can detect output arriving mid-RPC — Astra #10).
    pub pre_read_out_seq: u64,
}

impl Looked {
    pub fn is_fresh_ordered(&self) -> bool {
        self.live && self.ordered
    }
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
    /// One write of the registry at a time, each of what is newest when
    /// it begins: held from before its copy is taken until its file is
    /// in its place, by the thread that writes it (a write whose asker
    /// went away is finished all the same, and nothing overtakes it).
    /// Without it a write begun earlier landed after a later one: the
    /// writer's own, slow on a busy host, over a new child's
    /// (`begin_attached`), or over the last write of a service handing
    /// over, after its successor had been let go: the successor then
    /// knew nothing of the children started in between ("subagent
    /// `sa-…` not found", once in a suite under a load of 90; found by
    /// a second investigator with the write made slow on purpose,
    /// 2026-11-27).
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// The service has become idle, or stopped being so.
    idle: Notify,
    /// Set to end the service once what it has is written.
    retire: Notify,
    /// Level-triggered shutdown watch channel set to `true` when retirement
    /// begins so running `watch`/`wait`/`follow` handlers wake with ZERO lost
    /// wakeup even if retirement fires before entering `select!`.
    retiring_tx: tokio::sync::watch::Sender<bool>,
    retiring_rx: tokio::sync::watch::Receiver<bool>,
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
    /// Fresh per-process random incarnation token embedded in approval box
    /// keys so two service processes can never collide even if `service_gen`
    /// is reset or dropped by an older helper.
    pub(super) incarnation: u32,
    /// Serializes `adopt` migrations through durable commit so concurrent
    /// desktop migrations never observe unpersisted staged entries.
    adopt_lock: tokio::sync::Mutex<()>,
    /// Service-level background timer tasks (empty-minder / `end_minder_in`)
    /// aborted atomically at the freeze boundary.
    timers: Mutex<Vec<tokio::task::AbortHandle>>,
    /// Per-child async locks serializing `spawn_blocking` leave-list file
    /// publication and revocation so concurrent updates never reorder (Astra #25).
    leave_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Explicit `retire` callers waiting on the handover outcome (drained +
    /// saved vs aborted) outside `st.doing` accounting (Neon #1 / Astra #22).
    retire_waiters: Mutex<Vec<tokio::sync::oneshot::Sender<Result<(), String>>>>,
}

fn exited(summary: String) -> ScreenClassification {
    ScreenClassification {
        state: SubagentLiveState::Exited,
        summary: Some(summary),
        approval: None,
        folder_trust: None,
        api_error: None,
        usage: None,
        recognized_question: false,
        approval_fragment: false,
        in_turn: false,
    }
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
        approval_fragment: false,
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

/// Parses a generation-independent box dedupe key (`box:b{n}-{hash32_hex}`)
/// into `(number, hash32)` so an adopted standing box #2+ keeps its original
/// box number across handover (Astra R2 #5).
pub(super) fn parse_box_dedupe_key(dedupe: &str) -> Option<(u64, u32)> {
    let rest = dedupe.strip_prefix("box:b")?;
    let (n_str, h_str) = rest.split_once('-')?;
    let n = n_str.parse::<u64>().ok()?;
    let h = u32::from_str_radix(h_str, 16).ok()?;
    Some((n, h))
}

/// Stable question identity for a transcript-only `WaitingQuestion` combining
/// turn start time and pending tool call identity (or last message) so a
/// second transcript question B cannot validate an earlier saved event A
/// (Astra R2 #8).
pub(super) fn transcript_question_key(info: Option<&Value>, turn_ms: Option<u64>) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let pending = Service::pending_tools(info);
    if !pending.is_empty() {
        for p in &pending {
            p.to_string().hash(&mut h);
        }
    } else if let Some(pt) = info.and_then(|v| v["pending_tool"].as_str()) {
        pt.hash(&mut h);
    } else if let Some(msg) = info.and_then(|v| v["last_message"].as_str()) {
        msg.hash(&mut h);
    }
    format!("tx:q:{}:{:016x}", turn_ms.unwrap_or(0), h.finish())
}

/// Extracts the owning child ID from a child-scoped watcher key
/// (`child:<id>` or `child:<id>:<label>`).
pub(super) fn watcher_owner_child(w: &str) -> Option<&str> {
    w.strip_prefix("child:").map(|rest| rest.split(':').next().unwrap_or(rest))
}

/// True when `w` is subject to [`MAX_DETACHED_WATCHER_SEEN`] eviction: all
/// non-child detached keys (`watcher:*`, `win:*`, `parent:*`) AND labelled
/// child keys (`child:<id>:<label>`), so a live child cannot grow unbounded
/// state by inventing fresh `--watcher` labels (Astra R3 #18, Neon R3 #6).
pub(super) fn is_capped_watcher_key(w: &str) -> bool {
    w.strip_prefix("child:").is_none_or(|rest| rest.contains(':'))
}

/// Cap on detached and labelled `watcher_seen` entries so supervisors or live
/// children generating fresh `--watcher ID` labels cannot grow state without
/// bound across a long-lived child (Astra R2 #20, R3 #18; Neon R3 #6).
pub(super) const MAX_DETACHED_WATCHER_SEEN: usize = 512;

/// Records `(wk, agent_id) -> seq` in `st.watcher_seen` if `seq` strictly
/// advances the stored watermark, returning `Some(previous_value)` when
/// inserted (so callers can roll back on cancellation/save error). Enforces
/// [`MAX_DETACHED_WATCHER_SEEN`] on detached and labelled watcher keys.
pub(super) fn record_watcher_seen_locked(st: &mut State, wk: String, agent_id: String, seq: u64) -> Option<Option<u64>> {
    let pair = (wk, agent_id);
    let prev = st.watcher_seen.get(&pair).copied();
    if seq <= prev.unwrap_or(0) {
        return None;
    }
    let is_capped = is_capped_watcher_key(&pair.0);
    st.watcher_seen.insert(pair.clone(), seq);
    if is_capped {
        let capped_count = st.watcher_seen.keys().filter(|(w, _)| is_capped_watcher_key(w)).count();
        if capped_count > MAX_DETACHED_WATCHER_SEEN {
            let evict = st.watcher_seen.iter().filter(|(k, _)| is_capped_watcher_key(&k.0) && *k != &pair).min_by_key(|(_, v)| **v).map(|(k, _)| k.clone());
            if let Some(old) = evict {
                st.watcher_seen.remove(&old);
            }
        }
    }
    Some(prev)
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

    fn leave_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.leave_locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id.to_owned()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
    }

    fn notify_retire_waiters(&self, outcome: Result<(), String>) {
        let waiters: Vec<_> = self.retire_waiters.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for tx in waiters {
            let _ = tx.send(outcome.clone());
        }
    }

    /// The lock one writer of answers into a child holds (`approve`, the
    /// trust answer at its start). The child's own, gone with its record.
    pub fn answer_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.lock().live.get(id).map(|l| l.answer_lock.clone()).unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())))
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
    /// Requires the `windows` array to be present before replacing cached
    /// inventory so an incomplete/malformed reply never looks like `"gone"` (Astra #5).
    pub async fn refresh(&self, session: &str) -> Result<(), RexErr> {
        let view = self.rex.call("session.view", json!({ "session_id": session })).await?;
        let Some(windows) = view["windows"].as_array() else {
            return Err(RexErr::Api { code: "incomplete_reply".into(), message: "session.view reply missing `windows` array".into() });
        };
        let mut st = self.lock();
        let sess = st.sessions.entry(session.to_owned()).or_default();
        sess.windows.clear();
        sess.blocks.clear();
        sess.active = view["active_window_id"].as_str().map(str::to_owned);
        for w in windows {
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

    /// Lists live session IDs from Rex; requires the `sessions` array to be
    /// present so an incomplete reply never looks like zero live sessions (Astra #5).
    pub async fn session_ids(&self) -> Result<Vec<String>, RexErr> {
        let list = self.rex.call("session.list", Value::Null).await?;
        let Some(sessions) = list["sessions"].as_array() else {
            return Err(RexErr::Api { code: "incomplete_reply".into(), message: "session.list reply missing `sessions` array".into() });
        };
        Ok(sessions.iter().filter_map(|s| s["session_id"].as_str().map(str::to_owned)).collect())
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
        self.look_until(entry, fresh, None).await
    }

    /// [`Self::look`] optionally bounded by a monotonic wall-clock deadline
    /// (used by startup `--trust` so reads never outrun the 15 s budget).
    /// Fresh live `look_until` that retries briefly (up to 3×) ONLY when a
    /// concurrent reader superseded our sequence (`!res.rpc_failed && !res.ordered`),
    /// so a quiescent screen never spuriously fails `is_fresh_ordered()` due to
    /// a racing background `watch_child` read.
    pub(super) async fn look_ordered(&self, entry: &NativeSubagent, deadline: Option<tokio::time::Instant>) -> Looked {
        let mut last = self.look_until(entry, true, deadline).await;
        for _ in 0..3 {
            if last.is_fresh_ordered() || last.rpc_failed || last.cls.state == SubagentLiveState::Exited {
                break;
            }
            if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                break;
            }
            last = self.look_until(entry, true, deadline).await;
        }
        last
    }

    pub async fn look_until(&self, entry: &NativeSubagent, fresh: bool, deadline: Option<tokio::time::Instant>) -> Looked {
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
                return Looked {
                    cls,
                    screen: String::new(),
                    last_output_at: out,
                    last_state_change_at: chg,
                    pid: entry.last_pid,
                    survey: false,
                    survey_pending: false,
                    boxed: None,
                    expected: None,
                    reports: false,
                    expects: None,
                    live: false,
                    ordered: false,
                    rpc_failed: false,
                    seq: 0,
                    pre_read_out_seq: 0,
                };
            };
            if let Some(code) = live.exit_code {
                return Looked {
                    cls: exited(format!("exited (code {code})")),
                    screen: String::new(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey: false,
                    survey_pending: false,
                    boxed: None,
                    expected: None,
                    reports: false,
                    expects: None,
                    live: false,
                    ordered: false,
                    rpc_failed: false,
                    seq: 0,
                    pre_read_out_seq: live.out_seq,
                };
            }
            let (activity, status_msg) = live.activity();
            if !fresh && let Some(read) = live.read.as_ref().filter(|r| r.out_seq == live.out_seq && r.activity == activity) {
                let survey_pending = match live.survey {
                    SurveyState::Claimed { .. } => true,
                    SurveyState::Submitted { since, .. } => since.elapsed() < Self::SURVEY_GOES,
                    SurveyState::Idle | SurveyState::Stays => false,
                };
                let survey = read.survey || live.survey != SurveyState::Idle;
                return Looked {
                    cls: read.cls.clone(),
                    screen: read.screen.clone(),
                    last_output_at: live.last_output_at,
                    last_state_change_at: live.last_state_change_at,
                    pid: live.pid.or(entry.last_pid),
                    survey,
                    survey_pending,
                    boxed: peek_box(live, read.cls.approval.as_ref()),
                    expected: read.expected,
                    reports: live.reported,
                    expects: live.expects,
                    live: false,
                    ordered: false,
                    rpc_failed: false,
                    seq: read.seq,
                    pre_read_out_seq: read.out_seq,
                };
            }
            (live.read.clone(), live.out_seq, activity, status_msg, live.rows, live.expects.map(|x| x.seq))
        };
        let mut survey = false;
        let mut live_read = false;
        let mut rpc_failed = false;
        let mut my_seq = 0u64;
        let (cls, screen) = match self.screen_under_survey(entry, usize::from(rows), deadline, &mut survey).await {
            Ok((screen, seq)) => {
                live_read = true;
                my_seq = seq;
                let mut cls = classify_screen_rich(entry.cli, activity, &screen, true);
                if cls.summary.is_none() {
                    cls.summary = status_msg;
                }
                (cls, screen)
            }
            // (Its tab is gone: the event that says so is on its way.)
            Err(e) if e.gone() => (exited("tab closed".into()), String::new()),
            // (The server didn't say: as it last stood, or starting.)
            Err(_) => {
                rpc_failed = true;
                reading.map(|r| (r.cls, r.screen)).unwrap_or_else(|| (starting(), String::new()))
            }
        };
        let mut st = self.lock();
        let (mut out, mut chg, mut pid, mut reports, mut expects) = (0, 0, entry.last_pid, false, None);
        let mut boxed = None;
        let mut ordered = false;
        let mut survey_pending = false;
        let mut notify_state = None;
        let service_gen = st.reg.service_gen;
        if let Some(live) = st.live.get_mut(&entry.id) {
            // (Taken in the order the screen RPCs were begun: one begun before
            // the last accepted reading says nothing of what stands now.)
            ordered = live_read && my_seq > live.accepted_seq;
            if ordered {
                live.accepted_seq = my_seq;
                // Recompute the child's current `out_seq` at the time this ordered
                // reading is stored only if unchanged; keep the pre-RPC `out_seq`
                // so any output arriving during the RPC invalidates the cache.
                if cls.state != SubagentLiveState::Exited {
                    live.read = Some(Reading { out_seq, activity, cls: cls.clone(), screen: screen.clone(), expected, survey, seq: my_seq });
                }
            }
            // Only an ordered live reading can take up expected work and advance
            // `box_now` / `list_skip` / `adopted_pending`.
            if ordered
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
            let elsewhere = is_positively_elsewhere(&cls) && !survey;
            boxed = if ordered {
                let (b, retired_answered) = live.box_after(cls.approval.as_ref(), elsewhere, service_gen, self.incarnation);
                if retired_answered {
                    notify_state = Some(live.state_notify.clone());
                }
                b
            } else {
                peek_box(live, cls.approval.as_ref())
            };
            survey_pending = match live.survey {
                SurveyState::Claimed { .. } => true,
                SurveyState::Submitted { since, .. } => since.elapsed() < Self::SURVEY_GOES,
                SurveyState::Idle | SurveyState::Stays => false,
            };
            if !live_read && live.survey != SurveyState::Idle {
                survey = true;
            }
            (out, chg, pid, reports, expects) = (live.last_output_at, live.last_state_change_at, live.pid.or(entry.last_pid), live.reported, live.expects);
        }
        drop(st);
        if let Some(n) = notify_state {
            n.notify_waiters();
            self.changed.notify_waiters();
        }
        Looked {
            cls,
            screen,
            last_output_at: out,
            last_state_change_at: chg,
            pid,
            survey,
            survey_pending,
            boxed,
            expected: expected.filter(|_| ordered),
            reports,
            expects,
            live: live_read,
            ordered,
            rpc_failed,
            seq: my_seq,
            pre_read_out_seq: out_seq,
        }
    }

    /// Allocates the next monotonic reading sequence number for `id` immediately
    /// before starting a Rex `screen` RPC.
    fn next_read_seq(&self, id: &str) -> u64 {
        self.lock().live.get_mut(id).map_or(0, |l| {
            l.read_seq += 1;
            l.read_seq
        })
    }

    /// Current PTY output sequence number for `id`.
    pub(super) fn out_seq(&self, id: &str) -> u64 {
        self.lock().live.get(id).map_or(0, |l| l.out_seq)
    }

    /// Per-child notification handles `(output_notify, state_notify, survey_notify)`
    /// for event-driven waits with zero stationary polling.
    pub(super) fn child_notifies(&self, id: &str) -> Option<(Arc<Notify>, Arc<Notify>, Arc<Notify>)> {
        self.lock().live.get(id).map(|l| (l.output_notify.clone(), l.state_notify.clone(), l.survey_notify.clone()))
    }

    /// Wakes per-child state waiters (e.g. `approve` on revocation/list removal).
    pub(super) fn notify_child_state(&self, id: &str) {
        if let Some(n) = self.lock().live.get(id).map(|l| l.state_notify.clone()) {
            n.notify_waiters();
        }
    }

    /// How long a survey whose key was pressed is waited for before marking `Stays`.
    pub(super) const SURVEY_GOES: Duration = Duration::from_millis(900);

    /// True while the child's Rex terminal stream is connected and delivering
    /// `output_notify` wakeups (when false, wait loops fall back to 250 ms reads).
    pub(super) fn stream_connected(&self, id: &str) -> bool {
        self.lock().live.get(id).is_some_and(|l| l.stream_connected)
    }

    /// Monotonic counter bumped whenever the child's stream connects or
    /// disconnects so wait loops detect mid-wait disconnects immediately.
    pub(super) fn stream_gen(&self, id: &str) -> u64 {
        self.lock().live.get(id).map_or(0, |l| l.stream_gen)
    }

    /// True if an active survey dismissal (`Claimed` or unexpired `Submitted`)
    /// is currently running for `id`.
    pub(super) fn survey_pending(&self, id: &str) -> bool {
        self.lock().live.get(id).is_some_and(|l| match l.survey {
            SurveyState::Claimed { .. } => true,
            SurveyState::Submitted { since, .. } => since.elapsed() < Self::SURVEY_GOES,
            SurveyState::Idle | SurveyState::Stays => false,
        })
    }

    /// True if ANY feedback survey state (`Claimed`, `Submitted`, or `Stays`)
    /// is currently held for `id`.
    pub(super) fn survey_active(&self, id: &str) -> bool {
        self.lock().live.get(id).is_some_and(|l| l.survey != SurveyState::Idle)
    }

    /// Level-triggered retirement wait backed by `tokio::sync::watch` so
    /// `watch`/`wait`/`follow` never lose a retirement wakeup even if
    /// retirement fires before entering `select!`.
    async fn wait_retiring(&self) {
        let mut rx = self.retiring_rx.clone();
        if *rx.borrow_and_update() {
            return;
        }
        let _ = rx.changed().await;
    }

    async fn read_screen_once(&self, entry: &NativeSubagent, rows: usize, deadline: Option<tokio::time::Instant>) -> Result<(String, u64), RexErr> {
        let seq = self.next_read_seq(&entry.id);
        let screen = match deadline {
            Some(by) => {
                let rem = by.saturating_duration_since(tokio::time::Instant::now());
                if rem.is_zero() {
                    return Err(RexErr::Timeout);
                }
                self.rex.screen_within(&entry.session_id, &entry.block_id, rows, rem).await?
            }
            None => self.rex.screen(&entry.session_id, &entry.block_id, rows).await?,
        };
        Ok((screen, seq))
    }

    /// A child's screen as the server has it, with a feedback survey drawn over
    /// it set aside at most once per showing via [`SurveyState`].
    ///
    /// - Only the matching pre-submission owner token releases a `Claimed` slot
    ///   on Drop (waking the child watcher and waiting `send` callers).
    /// - Transitions to `Submitted` BEFORE `rex.write().await` so timeout or
    ///   cancellation treats delivery as uncertain and never presses `0` twice.
    /// - Partial survey frames (question without choices, or choices without
    ///   question: `has_survey_fragment`) NEVER clear an active state; only an
    ///   ordered, post-submission, non-blank non-fragment screen retires it.
    /// - Dismissal wait uses the child's `output_notify` + a single expiry timer
    ///   (zero stationary polling) and runs independently of `watch_child`'s
    ///   stream consumer.
    async fn screen_under_survey(&self, entry: &NativeSubagent, rows: usize, deadline: Option<tokio::time::Instant>, survey: &mut bool) -> Result<(String, u64), RexErr> {
        /// Releases ONLY the matching pre-submission `Claimed { token }` on Drop.
        struct SurveyClaim<'a> {
            service: &'a Service,
            id: &'a str,
            token: u64,
            submitted: bool,
        }
        impl Drop for SurveyClaim<'_> {
            fn drop(&mut self) {
                if self.submitted {
                    return;
                }
                let wake = {
                    let mut st = self.service.lock();
                    if let Some(live) = st.live.get_mut(self.id)
                        && live.survey == (SurveyState::Claimed { token: self.token })
                    {
                        live.survey = SurveyState::Idle;
                        live.pending_handoff = true;
                        live.handoff_failures = 0;
                        Some(live.survey_notify.clone())
                    } else {
                        None
                    }
                };
                if let Some(n) = wake {
                    n.notify_waiters();
                    self.service.poke(self.id);
                }
            }
        }

        let (mut screen, mut rpc_seq) = self.read_screen_once(entry, rows, deadline).await?;
        let Some(agent) = entry.cli.screen_agent() else { return Ok((screen, rpc_seq)) };
        loop {
            if olo_agent_screen::parse_survey(agent, &screen).is_none() {
                let weak_fragment = olo_agent_screen::has_survey_fragment(agent, &screen);
                let strong_fragment = olo_agent_screen::has_strong_survey_fragment(agent, &screen);
                let activity = self.lock().live.get(&entry.id).map_or(Activity::Idle, |l| l.activity().0);
                let cls_now = classify_screen_rich(entry.cli, activity, &screen, true);
                // Retire `Submitted`/`Stays` ONLY on positive evidence of a different
                // foreground state (running turn, prompt, approval, folder trust,
                // recognized question, or API error) — never on even a short partial
                // survey row (`1:`, `[0]`, `[1]`) or ambiguous non-empty fragment!
                let positive_other = !weak_fragment
                    && !cls_now.approval_fragment
                    && (is_positively_elsewhere(&cls_now) || cls_now.approval.is_some() || cls_now.folder_trust.is_some() || cls_now.recognized_question || cls_now.api_error.is_some());
                let wake = {
                    let mut st = self.lock();
                    if let Some(live) = st.live.get_mut(&entry.id) {
                        // Only retire `Submitted` or `Stays` on an ORDERED, positively
                        // different foreground reading begun strictly after submission.
                        // Never retire an active `Claimed` reservation from another reader!
                        let can_retire = rpc_seq > live.accepted_seq && positive_other;
                        match live.survey {
                            SurveyState::Submitted { submitted_at_seq, .. } if can_retire && rpc_seq > submitted_at_seq => {
                                live.survey = SurveyState::Idle;
                                Some(live.survey_notify.clone())
                            }
                            SurveyState::Stays if can_retire => {
                                live.survey = SurveyState::Idle;
                                Some(live.survey_notify.clone())
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                };
                if let Some(n) = wake {
                    n.notify_waiters();
                }
                // Astra #20: When already dismissing/staying (`survey != Idle`),
                // preserve `survey = true`; when `Idle`, require independent
                // `strong_fragment` evidence so an isolated `[1]` in ordinary
                // output never blocks normal `send`!
                let active_state = self.lock().live.get(&entry.id).is_some_and(|l| l.survey != SurveyState::Idle);
                *survey = strong_fragment || active_state;
                return Ok((screen, rpc_seq));
            }
            *survey = true;
            // (Never while a drafted feedback report is offered: the survey's
            // key is that box's key too, and nothing answers that box by itself.
            // Kept back, NOT marked Stays so it is still dismissed once the draft goes.)
            if olo_agent_screen::feedback_draft(agent, &screen).is_some() {
                break;
            }
            // Attempt to claim under the state lock.
            enum Decision {
                Claimed(u64),
                Done(Option<Arc<Notify>>),
            }
            let decision = {
                let mut st = self.lock();
                if st.frozen {
                    break;
                }
                let Some(live) = st.live.get_mut(&entry.id) else {
                    break;
                };
                // An adopted child's standing survey may already have had `0` pressed
                // by the predecessor service: mark `Stays` immediately, never press `0`.
                if live.adopted_pending {
                    live.survey = SurveyState::Stays;
                    Decision::Done(Some(live.survey_notify.clone()))
                } else {
                    match live.survey {
                        SurveyState::Stays => Decision::Done(None),
                        SurveyState::Claimed { .. } => Decision::Done(None),
                        SurveyState::Submitted { since, .. } => {
                            if since.elapsed() >= Self::SURVEY_GOES {
                                live.survey = SurveyState::Stays;
                                Decision::Done(Some(live.survey_notify.clone()))
                            } else {
                                Decision::Done(None)
                            }
                        }
                        SurveyState::Idle => {
                            live.survey_seq = live.survey_seq.wrapping_add(1);
                            let token = live.survey_seq;
                            live.survey = SurveyState::Claimed { token };
                            Decision::Claimed(token)
                        }
                    }
                }
            };
            let token = match decision {
                Decision::Claimed(token) => token,
                Decision::Done(wake) => {
                    if let Some(n) = wake {
                        n.notify_waiters();
                    }
                    break;
                }
            };
            let mut claim = SurveyClaim { service: self, id: &entry.id, token, submitted: false };
            // Verify fresh screen immediately before dispatching `0`.
            let (now_screen, verify_seq) = self.read_screen_once(entry, rows, deadline).await?;
            let found = olo_agent_screen::parse_survey(agent, &now_screen).filter(|_| olo_agent_screen::feedback_draft(agent, &now_screen).is_none());
            screen = now_screen;
            rpc_seq = verify_seq;
            let Some(found) = found else {
                drop(claim);
                continue;
            };
            // Transition to `Submitted` BEFORE `rex.write().await` so timeout or
            // cancellation never resets the slot and never presses `0` twice.
            // Also refuse to authorize input if this verify read was superseded or
            // if ownership no longer holds.
            let seq_before_write = self.out_seq(&entry.id);
            let committed = {
                let mut st = self.lock();
                if !st.frozen
                    && let Some(live) = st.live.get_mut(&entry.id)
                    && live.survey == (SurveyState::Claimed { token })
                    && !live.adopted_pending
                    && verify_seq > live.accepted_seq
                {
                    live.accepted_seq = verify_seq;
                    live.read_seq += 1;
                    let submitted_at_seq = live.read_seq;
                    live.survey = SurveyState::Submitted { token, since: Instant::now(), submitted_at_seq };
                    true
                } else {
                    false
                }
            };
            if !committed {
                drop(claim);
                break;
            }
            claim.submitted = true;
            let write_res = match deadline {
                Some(by) => {
                    let rem = by.saturating_duration_since(tokio::time::Instant::now());
                    if rem.is_zero() { Err(RexErr::Timeout) } else { self.rex.write_within(&entry.session_id, &entry.block_id, found.dismiss, rem).await }
                }
                None => self.rex.write(&entry.session_id, &entry.block_id, found.dismiss).await,
            };
            if let Err(e) = write_res {
                // Delivery is uncertain once dispatched: keep `Submitted` so no
                // second `0` is sent, and notify waiters so `send` doesn't hang.
                if let Some((_, _, sn)) = self.child_notifies(&entry.id) {
                    sn.notify_waiters();
                }
                return Err(e);
            }
            // Wait for dismissal via per-child `output_notify` + ONE expiry timer
            // (zero stationary polling when `stream_connected`; 250 ms fallback
            // only while disconnected). Track `survey_deadline` (900 ms) separately
            // from the caller's shorter `deadline` so a short caller deadline NEVER
            // prematurely marks a live survey `Stays`!
            let survey_deadline = tokio::time::Instant::now() + Self::SURVEY_GOES;
            let wait_until = deadline.map_or(survey_deadline, |by| survey_deadline.min(by));
            let notifies = self.child_notifies(&entry.id);
            let mut last_read_out = seq_before_write;
            let mut last_read_sgen = self.stream_gen(&entry.id);
            let mut final_checked = false;
            let mut fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
            loop {
                let cur_out = self.out_seq(&entry.id);
                let cur_sgen = self.stream_gen(&entry.id);
                let now_inst = tokio::time::Instant::now();
                let survey_expired = now_inst >= survey_deadline;
                let caller_expired = deadline.is_some_and(|by| now_inst >= by);
                if caller_expired && !survey_expired {
                    // Caller's short deadline elapsed before SURVEY_GOES: keep
                    // `Submitted` (do NOT prematurely mark `Stays`), wake waiters,
                    // and report Timeout.
                    if let Some((_, _, sn)) = notifies.as_ref() {
                        sn.notify_waiters();
                    }
                    return Err(RexErr::Timeout);
                }
                let fallback_ready = fallback_due.is_some_and(|d| now_inst >= d);
                // Always perform one final screen read at `survey_deadline` if the
                // last read still showed the survey (even if an early unrelated byte
                // triggered a read at 50 ms).
                if cur_out != last_read_out || cur_sgen != last_read_sgen || fallback_ready || (survey_expired && !final_checked) {
                    last_read_out = cur_out;
                    last_read_sgen = cur_sgen;
                    if survey_expired {
                        final_checked = true;
                    }
                    fallback_due = (!self.stream_connected(&entry.id)).then(|| tokio::time::Instant::now() + Duration::from_millis(250));
                    match self.read_screen_once(entry, rows, deadline).await {
                        Ok((s, sq)) => {
                            screen = s;
                            rpc_seq = sq;
                            if olo_agent_screen::parse_survey(agent, &screen).is_none() {
                                break;
                            }
                        }
                        Err(e) => {
                            if let Some((_, _, sn)) = notifies.as_ref() {
                                sn.notify_waiters();
                            }
                            return Err(e);
                        }
                    }
                }
                if survey_expired {
                    // Survey did not clear within `SURVEY_GOES`: mark `Stays`
                    // (only if still our `Submitted { token }`!) and wake waiters.
                    let wake = {
                        let mut st = self.lock();
                        if let Some(live) = st.live.get_mut(&entry.id)
                            && matches!(live.survey, SurveyState::Submitted { token: t, .. } if t == token)
                        {
                            live.survey = SurveyState::Stays;
                            Some(live.survey_notify.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(n) = wake {
                        n.notify_waiters();
                    }
                    break;
                }
                let sleep_to = fallback_due.map_or(wait_until, |f| f.min(wait_until));
                if let Some((ref out_n, ref st_n, _)) = notifies {
                    let out_wait = out_n.notified();
                    tokio::pin!(out_wait);
                    out_wait.as_mut().enable();
                    let st_wait = st_n.notified();
                    tokio::pin!(st_wait);
                    st_wait.as_mut().enable();
                    if self.out_seq(&entry.id) != last_read_out || self.stream_gen(&entry.id) != last_read_sgen {
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
                            if self.lock().live.get(&entry.id).is_none_or(|l| l.exit_code.is_some()) {
                                break;
                            }
                            if fallback_due.is_none() && !self.stream_connected(&entry.id) {
                                fallback_due = Some(tokio::time::Instant::now() + Duration::from_millis(250));
                            }
                        }
                        _ = tokio::time::sleep_until(sleep_to) => {}
                    }
                } else {
                    tokio::time::sleep_until(sleep_to).await;
                }
            }
            continue;
        }
        Ok((screen, rpc_seq))
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

    /// Atomically publishes a child's approval list via a private `0o600`
    /// sibling temp file + `sync_all()` + `fs::rename` on a blocking worker
    /// thread so a slow filesystem write never stalls the single-thread Tokio
    /// runtime, serialized per child via `leave_locks` (Astra #25).
    async fn write_leave_file_atomic(&self, id: &str, text: &str) -> std::io::Result<()> {
        let file = self.leave_file(id);
        let fallback_parent = self.paths.dir.clone();
        let text = text.to_owned();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            use std::sync::atomic::{AtomicU64, Ordering};
            static LEAVE_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

            let parent = file.parent().unwrap_or(&fallback_parent);
            std::fs::create_dir_all(parent)?;

            struct TempGuard {
                path: PathBuf,
                armed: bool,
            }
            impl Drop for TempGuard {
                fn drop(&mut self) {
                    if self.armed {
                        let _ = std::fs::remove_file(&self.path);
                    }
                }
            }

            let mut created = None;
            for _ in 0..64 {
                let n = LEAVE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
                let tmp = parent.join(format!(".approve-all-but.tmp.{}.{n}", std::process::id()));
                match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp) {
                    Ok(f) => {
                        created = Some((tmp, f));
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            let Some((tmp, mut f)) = created else {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "couldn't allocate unique leave temp file"));
            };
            let mut guard = TempGuard { path: tmp.clone(), armed: true };
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &file)?;
            guard.armed = false;
            Ok(())
        })
        .await
        .map_err(|e| std::io::Error::other(format!("leave writer task failed: {e}")))?
    }

    /// Removes any on-disk approval list for `id` on a blocking worker.
    /// Returns `Ok(true)` if removed, `Ok(false)` if already absent, or `Err`
    /// if an existing file could NOT be removed (Astra #14: never report
    /// revocation success while an old active file remains on disk!).
    async fn remove_leave_file_blocking(&self, id: &str) -> std::io::Result<bool> {
        let file = self.leave_file(id);
        tokio::task::spawn_blocking(move || match std::fs::remove_file(&file) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        })
        .await
        .map_err(|e| std::io::Error::other(format!("leave remove task failed: {e}")))?
    }

    /// Gives the child `id` a list (`text`: a pattern a line) from now,
    /// in another's place if it had one: how many patterns. A line that
    /// is no pattern is refused, and nothing changes.
    pub async fn set_leave(&self, id: &str, text: &str) -> Result<usize, String> {
        let list = super::leave::LeaveList::parse(text)?;
        let file = self.leave_file(id);
        let lock = self.leave_lock(id);
        let _guard = lock.lock().await;
        self.write_leave_file_atomic(id, text).await.map_err(|e| format!("{}: {e}", file.display()))?;
        let n = list.patterns().len();
        {
            let mut st = self.lock();
            st.leave.insert(id.to_owned(), Some(Arc::new(list)));
            if let Some(live) = st.live.get_mut(id) {
                // Invalidate any prior transition dedupe so a standing unanswered
                // approval box is immediately re-evaluated against the new list.
                live.last_state = None;
                live.last_key = None;
                live.read = None;
                live.pending_handoff = true;
                live.handoff_failures = 0;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
                live.check_gen = live.check_gen.wrapping_add(1);
            }
            st.log.reset_child_dedupe(id);
        }
        self.notify_child_state(id);
        self.changed.notify_waiters();
        self.poke(id);
        Ok(n)
    }

    /// Takes the child's list away durably: everything it asks leave for is
    /// asked again. Returns `Err` if an existing active file on disk cannot
    /// be neutralized (Astra #14 / #25).
    pub async fn clear_leave(&self, id: &str) -> Result<bool, String> {
        let file = self.leave_file(id);
        let lock = self.leave_lock(id);
        let _guard = lock.lock().await;
        let had = self.remove_leave_file_blocking(id).await.map_err(|e| format!("couldn't remove {}: {e}", file.display()))?;
        let was = self.lock().leave.insert(id.to_owned(), None).flatten().is_some() || had;
        if was {
            self.notify_child_state(id);
            self.changed.notify_waiters();
        }
        Ok(was)
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

    /// Atomically marks the standing approval box `standing` as answered
    /// under an already-held state lock, advancing both `check_gen` and
    /// `event_gen` and returning `(box_number, answer_event_gen)`.
    /// Fails if the box was already marked answered or was replaced.
    pub(super) fn box_mark_answered_locked(st: &mut State, id: &str, standing: &str) -> Result<(u64, u64), ()> {
        let live = st.live.get_mut(id).ok_or(())?;
        let cur = live.box_now.as_mut().ok_or(())?;
        if cur.key() != standing || cur.answered {
            return Err(());
        }
        cur.answered = true;
        live.check_gen = live.check_gen.wrapping_add(1);
        live.event_gen = live.event_gen.wrapping_add(1);
        Ok((cur.number, live.event_gen))
    }

    /// True when box `number` was already retired by an ordered, positively
    /// `elsewhere` reading (`Live::last_elsewhere_box >= number`).
    pub(super) fn box_was_retired_elsewhere(&self, id: &str, number: u64) -> bool {
        self.lock().live.get(id).is_some_and(|l| l.last_elsewhere_box >= number)
    }

    /// True if a reading showing an approval or folder-trust question must
    /// NOT be published or granted right now:
    /// - For an approval: either its box is already answered, or it is no
    ///   longer the exact unanswered `box_now` held for this child.
    /// - For a folder-trust prompt: startup `--trust` still owns it (or is
    ///   actively answering it, or waiting for its path within the deadline).
    pub(super) fn question_suppressed_locked(st: &State, id: &str, cls: &ScreenClassification, boxed: Option<&LookedBox>) -> bool {
        if cls.approval.is_some() {
            let live_ok = boxed.filter(|b| !b.answered).is_some_and(|b| st.live.get(id).and_then(|l| l.box_now).is_some_and(|cur| !cur.answered && cur.number == b.number && cur.hash == b.hash));
            if !live_ok {
                return true;
            }
        }
        if let Some(asked) = cls.folder_trust.as_ref()
            && st.live.get(id).and_then(|l| l.trust.as_ref()).is_some_and(|trust| trust.on_its_way(asked))
        {
            return true;
        }
        // Suppress partial approval fragments while an approval or startup
        // trust prompt is actively dismissing so a brief redraw never emits a
        // spurious `waiting_question` ("dialog redrawing").
        cls.approval_fragment && st.live.get(id).is_some_and(|l| l.box_now.is_some_and(|b| b.answered) || l.trust.as_ref().is_some_and(|t| t.answering))
    }

    /// True iff `box_now` is still the exact instance `box_key` and has not
    /// been answered (read-only: never mints a new box from a stale reading).
    pub(super) fn box_still_unanswered(&self, id: &str, box_key: &str) -> bool {
        self.lock().live.get(id).and_then(|l| l.box_now).is_some_and(|b| !b.answered && b.key() == box_key)
    }

    /// Sets or clears the mid-navigation flag so a concurrent `look`
    /// catching a transient repaint frame never wipes `box_now`.
    pub(super) fn set_navigating(&self, id: &str, navigating: bool) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.navigating = navigating;
        }
    }

    /// The keys of a folder's trust are on their way (`mark_answered` marks
    /// that folder's single attempt as spent). Records a dedupe boundary so
    /// any underlying chooser question confirms and publishes cleanly once
    /// trust finishes.
    pub(super) fn trust_taken(&self, id: &str, which: Option<String>, mark_answered: bool) {
        let mut st = self.lock();
        if st.frozen {
            return;
        }
        if let Some(live) = st.live.get_mut(id) {
            if let Some(trust) = live.trust.as_mut() {
                if mark_answered {
                    trust.answered.insert(which);
                }
                trust.answering = true;
            }
            live.read = None;
            live.last_state = None;
            live.last_key = None;
            live.check_gen = live.check_gen.wrapping_add(1);
        }
    }

    /// Its keys are no longer on their way: what still stands is read
    /// again, and told as any other question.
    pub(super) fn trust_keys_done(&self, id: &str) {
        {
            let mut st = self.lock();
            if st.frozen {
                return;
            }
            if let Some(live) = st.live.get_mut(id) {
                if let Some(trust) = live.trust.as_mut() {
                    trust.answering = false;
                }
                live.read = None;
                live.last_state = None;
                live.last_key = None;
                live.pending_handoff = true;
                live.handoff_failures = 0;
            }
            st.log.reset_child_dedupe(id);
        }
        self.notify_child_state(id);
        self.changed.notify_waiters();
        self.poke(id);
    }

    /// The answering of a child's folders at its start is over (answered,
    /// its time up, a question that isn't its own): what stands is read
    /// again and told.
    pub(super) fn trust_over(&self, id: &str) {
        {
            let mut st = self.lock();
            if st.frozen {
                return;
            }
            if let Some(live) = st.live.get_mut(id) {
                live.trust = None;
                live.read = None;
                live.last_state = None;
                live.last_key = None;
                live.pending_handoff = true;
                live.handoff_failures = 0;
            }
            st.log.reset_child_dedupe(id);
        }
        self.notify_child_state(id);
        self.changed.notify_waiters();
        self.poke(id);
    }

    /// Finishes an answered box once `approve`'s wait for it is over:
    /// the box is put away if it is still that one (`number`: never a
    /// newer one that took its place), and what was read of the screen
    /// with it, so that what stands next is read afresh and is another
    /// box. One that didn't go (`Stays`), or went with nothing to say
    /// where its agent is (`Blank`), is kept out of the list for the
    /// entire showing (`ListSkip`, no wall-clock expiry) unless a positive
    /// ordered `elsewhere` reading already retired this box (`last_elsewhere_box`).
    /// Also resets dedupe ONLY if no post-answer event (`latest_event_gen >= answer_event_gen`)
    /// has already been recorded — independent of the 2,000-event ring!
    pub fn box_finish(&self, id: &str, number: u64, end: BoxEnd, answer_event_gen: Option<u64>) {
        {
            let mut st = self.lock();
            let mut should_reset = false;
            if let Some(live) = st.live.get_mut(id) {
                if let Some(cur) = live.box_now
                    && cur.number == number
                {
                    if end != BoxEnd::Gone && live.last_elsewhere_box < number {
                        live.list_skip = Some(ListSkip::from_box(cur));
                    }
                    live.box_now = None;
                }
                live.read = None;
                live.pending_handoff = true;
                live.handoff_failures = 0;
                if answer_event_gen.is_none_or(|g| live.latest_event_gen < g) {
                    live.last_state = None;
                    live.last_key = None;
                    live.check_gen = live.check_gen.wrapping_add(1);
                    should_reset = true;
                }
            }
            if should_reset {
                st.log.reset_child_dedupe(id);
            }
        }
        self.notify_child_state(id);
        self.changed.notify_waiters();
        self.poke(id);
    }

    /// Conditionally clears `box_now` ONLY if it is still `expected_number`
    /// so a stale miss during navigation can never wipe a successor box.
    pub fn box_gone(&self, id: &str, expected_number: u64) {
        if let Some(live) = self.lock().live.get_mut(id)
            && live.box_now.is_some_and(|cur| cur.number == expected_number)
        {
            live.box_now = None;
            live.read = None;
        }
    }

    /// Checks `list_allows` under an already-held state lock (assuming
    /// `self.leave_of(id)` has loaded the child's leave list into `st.leave`).
    pub(super) fn list_allows_locked(st: &State, id: &str, ap: &olo_agent_screen::ApprovalPrompt) -> Option<String> {
        let cut = ap.folded_lines.is_some()
            || ap.command.as_deref().is_some_and(|c| {
                let t = c.trim_end();
                t.ends_with('…') || t.ends_with("...")
            });
        if cut {
            return None;
        }
        let asked = super::leave::asked(ap.tool.as_deref(), ap.command.as_deref(), &ap.question)?;
        let list = st.leave.get(id)?.as_ref()?;
        (list.excepts(&asked).is_none() && ap.choices.iter().filter(|c| c.kind == olo_agent_screen::ChoiceKind::Once).count() == 1).then_some(asked)
    }

    /// What a child asks, as its list is asked it, where the list allows
    /// it: the whole of the request (one cut short on the screen, or
    /// folded, is nobody's to allow unread), with one answer that is a
    /// yes for this once. Asked when the list judges a box, and again as
    /// the answer is about to be written: a list taken away meanwhile
    /// (`mind NAME --approve-none`) allows nothing.
    pub(super) fn list_allows(&self, id: &str, ap: &olo_agent_screen::ApprovalPrompt) -> Option<String> {
        let _ = self.leave_of(id)?;
        Self::list_allows_locked(&self.lock(), id, ap)
    }

    /// An approval as it is told (`approval_to_json`), labelled with the
    /// exact `box_key` captured on the reading that produced `ap` (never
    /// re-reading live `box_now` after `inspect`).
    pub fn approval_json_with_box(ap: &olo_agent_screen::ApprovalPrompt, pending: &[Value], box_key: Option<&str>) -> Value {
        let mut told = olo_rex_link::agents::approval_to_json(ap, pending);
        told["box"] = json!(box_key);
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
    /// Also advances `check_gen` and `event_gen` so in-flight pre-typing
    /// confirmation tasks never overwrite post-typing state.
    pub fn reset_edge(&self, entry: &NativeSubagent, clear_seen_working: bool) {
        let mut st = self.lock();
        if let Some(live) = st.live.get_mut(&entry.id) {
            live.last_state = None;
            live.last_key = None;
            live.read = None;
            live.check_gen = live.check_gen.wrapping_add(1);
            live.event_gen = live.event_gen.wrapping_add(1);
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
                if !st.frozen {
                    changed |= st.reg.update_discovered(&id, v["cli_session_id"].as_str(), v["session_confidence"].as_str(), v["transcript_path"].as_str(), v["pid"].as_i64().map(|p| p as i32));
                }
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
            if st.frozen {
                return st.log.latest_seq();
            }
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
            if st.frozen {
                return st.log.latest_seq();
            }
            let before = st.log.latest_seq();
            let seq = st.log.push(event.clone());
            if seq > before {
                let mut event = event;
                event.seq = seq;
                st.reg.event_seq = seq;
                if let Some(live) = st.live.get_mut(&event.id) {
                    live.latest_event = Some(event.clone());
                    live.latest_event_gen = live.event_gen;
                    live.pending_handoff = false;
                    live.handoff_failures = 0;
                    live.unconfirmed = false;
                    live.confirm_in_flight = None;
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

    /// Atomically validates that a confirmation task was not superseded
    /// (`check_gen == generation`, `event_gen == expected_event_gen`, and any
    /// approval/trust prompt is still unsuppressed) under the SAME lock that
    /// pushes and appends the event. Returns `Some(seq)` if recorded.
    fn record_confirmed(&self, id: &str, generation: u64, expected_event_gen: u64, cls: &ScreenClassification, boxed: Option<&LookedBox>, event: SubagentEvent) -> Option<u64> {
        let seq = {
            let mut st = self.lock();
            if st.frozen {
                return None;
            }
            let live = st.live.get(id)?;
            if live.check_gen != generation || live.event_gen != expected_event_gen {
                return None;
            }
            if Self::question_suppressed_locked(&st, id, cls, boxed) {
                return None;
            }
            let before = st.log.latest_seq();
            let seq = st.log.push(event.clone());
            if seq > before {
                st.reg.event_seq = seq;
            }
            if let Some(live) = st.live.get_mut(id) {
                live.pending_handoff = false;
                live.handoff_failures = 0;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
                if seq > before {
                    let mut event = event;
                    event.seq = seq;
                    live.latest_event = Some(event.clone());
                    live.latest_event_gen = live.event_gen;
                    if let Ok(mut line) = serde_json::to_string(&event) {
                        use std::io::Write as _;
                        line.push('\n');
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.paths.events) {
                            let _ = f.write_all(line.as_bytes());
                        }
                    }
                } else if seq > 0 && live.latest_event.as_ref().is_none_or(|e| e.seq <= seq) {
                    // Duplicate confirmation (e.g., after handover when `live.latest_event`
                    // started as None): populate the live cache with the original `seq` and
                    // current verified payload (including this generation's box key) so a
                    // later ring rollover cannot evict this standing prompt (Astra R2 #3).
                    let mut event = event;
                    event.seq = seq;
                    live.latest_event = Some(event);
                    live.latest_event_gen = live.event_gen;
                }
            }
            seq
        };
        self.save.notify_one();
        self.changed.notify_waiters();
        Some(seq)
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
        // Suppress public follower frames while `begin_attached` is still
        // awaiting durable registry persistence so a rollback never leaks.
        if !live.committed {
            return;
        }
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
        if st.provisional_ids.contains(id) {
            return;
        }
        let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id).cloned() else { return };
        if entry.exited_at.is_none() && st.live.get(id).is_some_and(|l| !l.committed) {
            return;
        }
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
        let notifies: Option<(Arc<Notify>, Arc<Notify>, Arc<Notify>)>;
        let entry = {
            let mut st = self.lock();
            if st.frozen {
                return;
            }
            if let Some(live) = st.live.get_mut(id)
                && !live.committed
            {
                if let Some(code) = code {
                    live.pending_exit_code = Some(code);
                } else {
                    live.pending_tab_closed = true;
                }
                return;
            }
            notifies = st.live.get(id).map(|l| (l.output_notify.clone(), l.state_notify.clone(), l.survey_notify.clone()));
            if let Some(live) = st.live.get_mut(id) {
                if let Some(code) = code {
                    live.exit_code = Some(code);
                }
                live.pending_handoff = false;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
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
                // (Said already; its tab closing after its program ended ends its watchers.)
                if code.is_none()
                    && let Some(live) = st.live.remove(id)
                {
                    for task in live.tasks {
                        task.abort();
                    }
                }
                if let Some((out_n, st_n, sv_n)) = notifies {
                    out_n.notify_waiters();
                    st_n.notify_waiters();
                    sv_n.notify_waiters();
                }
                return;
            }
            st.watcher_seen.retain(|(w, a), _| a != id && watcher_owner_child(w) != Some(id));
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
            {
                for task in live.tasks {
                    task.abort();
                }
            }
            entry
        };
        if let Some((out_n, st_n, sv_n)) = notifies {
            out_n.notify_waiters();
            st_n.notify_waiters();
            sv_n.notify_waiters();
        }
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
            if st.frozen {
                return;
            }
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
                self.notify_child_state(id);
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
        // (By its id: a name may have a space in it, and "send worker
        // one --key 0" is another child and a word.)
        let msg = format!(
            "Feedback draft: Claude Code offers to send a feedback report it drafted ({}). Nothing answers it for you and nothing is typed into this child while it shows: `olo agent send {} --key 0` sets it aside.",
            shows.unwrap_or_default(),
            entry.id
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
            if st.frozen {
                return;
            }
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
        let handle = tokio::spawn(async move {
            tokio::time::sleep(after).await;
            struct TimerDoingGuard {
                service: Arc<Service>,
            }
            impl Drop for TimerDoingGuard {
                fn drop(&mut self) {
                    {
                        let mut st = self.service.lock();
                        st.doing = st.doing.saturating_sub(1);
                    }
                    self.service.idle.notify_one();
                }
            }
            let _doing = {
                let mut st = service.lock();
                if st.frozen || st.retiring {
                    return;
                }
                let still = st.reg.entries.iter().any(|e| e.id == minder.id && e.exited_at.is_none());
                let same_gen = st.empty_gen.get(&minder.id) == Some(&epoch);
                let idle = st.reg.minded_by(&minder.id).is_empty() && st.pending_minds.get(&minder.id).copied().unwrap_or(0) == 0;
                if still && same_gen && idle && !st.closing_minders.contains(&minder.id) {
                    st.closing_minders.insert(minder.id.clone());
                    st.doing += 1;
                    Some(TimerDoingGuard { service: service.clone() })
                } else {
                    None
                }
            };
            if _doing.is_some() {
                let _ = service.stop_tabs_inner(&minder, true, false, false, true).await;
                service.lock().closing_minders.remove(&minder.id);
            }
        })
        .abort_handle();
        let mut timers = self.timers.lock().unwrap_or_else(|e| e.into_inner());
        timers.retain(|h| !h.is_finished());
        timers.push(handle);
    }

    /// Re-arms shutdown timers for all currently empty live shared minders AND
    /// any live 1:1 legacy minders whose ward has already exited (used after
    /// reconcile AND when an aborted handover resumes — Astra #21 / Neon #1).
    fn rearm_empty_minders(self: &Arc<Self>) {
        let (empty_shared, ended_one_to_one): (Vec<String>, Vec<NativeSubagent>) = {
            let st = self.lock();
            let shared = st
                .reg
                .entries
                .iter()
                .filter(|e| {
                    e.exited_at.is_none() && e.is_shared_minder() && st.reg.minded_by(&e.id).is_empty() && st.pending_minds.get(&e.id).copied().unwrap_or(0) == 0 && !st.closing_minders.contains(&e.id)
                })
                .map(|e| e.id.clone())
                .collect();
            let one_to_one = st
                .reg
                .entries
                .iter()
                .filter(|e| e.exited_at.is_none() && !e.is_shared_minder() && e.minds.as_deref().is_some_and(|ward_id| !st.reg.entries.iter().any(|w| w.id == ward_id && w.exited_at.is_none())))
                .cloned()
                .collect();
            (shared, one_to_one)
        };
        for mid in empty_shared {
            self.arm_empty_minder_if_idle(&mid, minder_reports());
        }
        for m in ended_one_to_one {
            self.end_minder_in(m, minder_reports());
        }
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
        let handle = tokio::spawn(async move {
            tokio::time::sleep(after).await;
            struct TimerDoingGuard {
                service: Arc<Service>,
            }
            impl Drop for TimerDoingGuard {
                fn drop(&mut self) {
                    {
                        let mut st = self.service.lock();
                        st.doing = st.doing.saturating_sub(1);
                    }
                    self.service.idle.notify_one();
                }
            }
            let _doing = {
                let mut st = service.lock();
                if !st.frozen && !st.retiring && st.reg.entries.iter().any(|e| e.id == minder.id && e.exited_at.is_none()) {
                    st.doing += 1;
                    Some(TimerDoingGuard { service: service.clone() })
                } else {
                    None
                }
            };
            if _doing.is_some() {
                let _ = service.stop_tabs_inner(&minder, true, false, false, true).await;
            }
        })
        .abort_handle();
        let mut timers = self.timers.lock().unwrap_or_else(|e| e.into_inner());
        timers.retain(|h| !h.is_finished());
        timers.push(handle);
    }

    /// Schedules at most one bounded backoff retry for a child with
    /// `pending_handoff == true` or an unconfirmed standing transition whose
    /// screen reread failed transiently. After 5 failed retries, emits an
    /// `escalation` so the owner knows a standing prompt could not be re-read.
    fn schedule_handoff_retry(self: &Arc<Self>, entry: &NativeSubagent) {
        const BACKOFF_MS: [u64; 5] = [250, 500, 1000, 1500, 2000];
        let delay = {
            let mut st = self.lock();
            if st.frozen {
                return;
            }
            let Some(live) = st.live.get_mut(&entry.id) else { return };
            if (!live.pending_handoff && !live.unconfirmed) || live.handoff_retry_armed {
                return;
            }
            live.handoff_failures = live.handoff_failures.saturating_add(1);
            if (live.handoff_failures as usize) > BACKOFF_MS.len() {
                live.pending_handoff = false;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
                live.handoff_failures = 0;
                None
            } else {
                live.handoff_retry_armed = true;
                Some(Duration::from_millis(BACKOFF_MS[(live.handoff_failures - 1) as usize]))
            }
        };
        if let Some(wait) = delay {
            let (service, id) = (self.clone(), entry.id.clone());
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                if service.lock().frozen {
                    return;
                }
                if let Some(live) = service.lock().live.get_mut(&id) {
                    live.handoff_retry_armed = false;
                }
                service.poke(&id);
            });
        } else {
            let state = self.lock().live.get(&entry.id).and_then(|l| l.last_state).unwrap_or(SubagentLiveState::Working);
            let msg = "Screen reread failed repeatedly after answering or releasing a prompt: check the child's status to see if a question is still standing.".to_owned();
            let mut event = self.event_of(entry, state, Some(msg.clone()), String::new());
            event.event = olo_rex_link::agents::minder::ESCALATION.to_owned();
            event.last_message = Some(msg);
            self.record_beside(event);
        }
    }

    /// Reads a child's screen and, where its state differs from what was
    /// last made of it, writes that down once it has held ([`CONFIRM`]):
    /// with its last message whole and the tool it waits on, from its
    /// transcript.
    async fn transition(self: &Arc<Self>, id: &str) {
        let (entry, force_fresh) = {
            let st = self.lock();
            if st.frozen {
                return;
            }
            if st.live.get(id).is_some_and(|l| !l.committed) {
                return;
            }
            let Some(entry) = st.reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
            let force = st.live.get(id).is_some_and(|l| l.confirm_in_flight.is_none() && (l.pending_handoff || l.unconfirmed));
            (entry, force)
        };
        let looked = self.look(&entry, force_fresh).await;
        if self.lock().frozen {
            return;
        }
        if looked.rpc_failed {
            // Never treat a transient RPC failure as `Working("starting")` and
            // never silently drop a first quiet prompt, pending handoff, or
            // unconfirmed standing transition.
            if let Some(live) = self.lock().live.get_mut(id) {
                live.unconfirmed = true;
            }
            self.schedule_handoff_retry(&entry);
            return;
        }
        if looked.live && !looked.ordered {
            // Astra #18: Superseded by a newer concurrent live reading (e.g. a
            // racing `status` RPC that populated the cache without scheduling
            // confirmation). Arm bounded retry/poke whenever no confirmation is
            // in flight so a quiet standing prompt is NEVER stranded!
            let needs_retry = {
                let mut st = self.lock();
                if let Some(live) = st.live.get_mut(id)
                    && live.confirm_in_flight.is_none()
                {
                    live.unconfirmed = true;
                    true
                } else {
                    false
                }
            };
            if needs_retry {
                self.schedule_handoff_retry(&entry);
            }
            return;
        }
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
        // `live.last_key` from box 1. If this showing was already answered
        // and the child has not yet redrawn, do not re-trigger confirmation.)
        let (key, box_answered) = match (cls.approval.as_ref(), looked.boxed.as_ref()) {
            (Some(_), Some(lb)) => (format!("box:{}", lb.key), lb.answered),
            // (A box the service doesn't hold as standing: the reading
            // came back out of its order, or the box was finished since.
            // Read afresh.)
            (Some(_), None) => {
                if let Some(live) = self.lock().live.get_mut(id) {
                    live.read = None;
                }
                self.poke(id);
                return;
            }
            (None, _) => (subkey(&cls), false),
        };
        // (An answer on its way, a box's or a folder's trust: nothing to
        // tell of what still stands on the screen, and what was being
        // confirmed from before the answer is of before it. Recording this
        // suppression as a dedupe boundary (`last_state = None; last_key = None`)
        // ensures returning to an ordinary quiet chooser Q after `--trust`
        // finishes always confirms and publishes Q.)
        let trust_on_its_way = cls.folder_trust.as_ref().is_some_and(|asked| self.lock().live.get(id).and_then(|l| l.trust.as_ref()).is_some_and(|trust| trust.on_its_way(asked)));
        let frag_dismissing = cls.approval_fragment && self.lock().live.get(id).is_some_and(|l| l.box_now.is_some_and(|b| b.answered) || l.trust.as_ref().is_some_and(|t| t.answering));
        if box_answered || trust_on_its_way || frag_dismissing {
            if let Some(live) = self.lock().live.get_mut(id) {
                live.check_gen = live.check_gen.wrapping_add(1);
                live.last_state = None;
                live.last_key = None;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
            }
            if trust_on_its_way {
                self.changed.notify_waiters();
            }
            return;
        }
        let started_ms = to_unix_ms(entry.started_at).max(0) as u64;
        let at_prompt = cls.summary.as_deref() == Some(state::AT_PROMPT);
        let mut look_again = None;
        let confirm_token = {
            let mut st = self.lock();
            if st.frozen {
                return;
            }
            if cls.state == SubagentLiveState::Working
                && is_positively_elsewhere(&cls)
                && matches!(st.log.last_recorded_of(id), Some((SubagentLiveState::WaitingApproval | SubagentLiveState::WaitingQuestion, _)))
            {
                st.log.reset_child_dedupe(id);
            }
            let Some(live) = st.live.get_mut(id) else { return };
            // A confirmed positive `Working` reading proves any prior prompt
            // is gone, so a pending post-answer handoff is satisfied.
            if cls.state == SubagentLiveState::Working && is_positively_elsewhere(&cls) {
                live.pending_handoff = false;
                live.handoff_failures = 0;
                live.unconfirmed = false;
                live.confirm_in_flight = None;
            }
            if cls.state != SubagentLiveState::Idle && !(cls.state == SubagentLiveState::Working && cls.summary.as_deref() == Some(state::STARTING)) {
                live.seen_working = true;
            }
            if state::takes_up(cls.in_turn, read_expecting, live.expects)
                && let Some(given) = live.expects.take()
            {
                live.answers = Some(given.cursor);
            }
            let kept = match cls.state {
                SubagentLiveState::Idle => match state::rest_now(now_unix_ms(), started_ms, live.seen_working, at_prompt, live.expects, None) {
                    state::Rest::Kept { until_ms } => Some(until_ms),
                    state::Rest::Told { .. } => None,
                },
                _ => None,
            };
            let was_kept = live.held_until.is_some();
            if kept.is_some() && live.held_until != kept {
                look_again = kept;
            }
            live.held_until = kept;
            // (The same as it last stood: nothing new to make of it,
            // unless it has pending work or a pending handoff / failed
            // confirmation with NO active confirmation already in flight —
            // folding `pending_handoff` under `confirm_in_flight.is_none()`
            // ensures blinking screens cannot livelock an active confirmation.)
            let needs_confirm_retry = (live.unconfirmed || live.pending_handoff) && live.confirm_in_flight.is_none();
            if live.last_state == Some(cls.state) && live.last_key.as_deref() == Some(key.as_str()) && !was_kept && kept.is_none() && live.expects.is_none() && !needs_confirm_retry {
                return;
            }
            live.last_state = Some(cls.state);
            live.last_key = Some(key.clone());
            live.last_state_change_at = now_unix_ms();
            live.check_gen = live.check_gen.wrapping_add(1);
            if (kept.is_some() && live.expects.is_none()) || cls.state == SubagentLiveState::Working {
                live.unconfirmed = false;
                live.confirm_in_flight = None;
                None
            } else {
                live.unconfirmed = true;
                live.confirm_in_flight = Some(live.check_gen);
                Some((live.check_gen, live.event_gen))
            }
        };
        if let Some(until_ms) = look_again {
            let (service, id) = (self.clone(), id.to_owned());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(until_ms.saturating_sub(now_unix_ms()) + 150)).await;
                if !service.lock().frozen {
                    service.poke(&id);
                }
            });
        }
        if !(entry.cli == olo_rex_link::agents::AgentCli::Codex && cls.state == SubagentLiveState::Idle && confirm_token.is_some()) {
            self.tell(id, cls.state, cls.summary.clone());
        }
        // (A question come or gone: who waits on that looks at once.)
        if cls.folder_trust.is_some() || cls.approval.is_some() || cls.state == SubagentLiveState::Exited || cls.state == SubagentLiveState::Working {
            self.changed.notify_waiters();
        }
        let Some((generation, confirm_event_gen)) = confirm_token else { return };
        let (service, id_owned) = (self.clone(), id.to_owned());
        let task = tokio::spawn(async move {
            let id = id_owned;
            struct ConfirmGuard {
                service: Arc<Service>,
                id: String,
                generation: u64,
            }
            impl Drop for ConfirmGuard {
                fn drop(&mut self) {
                    if let Some(live) = self.service.lock().live.get_mut(&self.id)
                        && live.confirm_in_flight == Some(self.generation)
                    {
                        live.confirm_in_flight = None;
                    }
                }
            }
            let confirm_guard = ConfirmGuard { service: service.clone(), id: id.clone(), generation };

            tokio::time::sleep(CONFIRM).await;
            let current = |service: &Service| {
                let st = service.lock();
                !st.frozen && st.live.get(&id).is_some_and(|l| l.check_gen == generation && l.event_gen == confirm_event_gen)
            };
            if !current(&service) {
                return;
            }
            let Some(entry) = service.lock().reg.entries.iter().rev().find(|e| e.id == id && e.exited_at.is_none()).cloned() else { return };
            let again = service.look(&entry, true).await;
            if !current(&service) {
                return;
            }
            if again.rpc_failed {
                drop(confirm_guard);
                service.schedule_handoff_retry(&entry);
                return;
            }
            if !again.is_fresh_ordered() {
                drop(confirm_guard);
                service.poke(&id);
                return;
            }
            if again.cls.state == SubagentLiveState::Exited {
                // (Gone meanwhile: its end is `exit`'s to say.)
                drop(confirm_guard);
                service.poke(&id);
                return;
            }
            let (again_box, again_key, again_answered) = match (again.cls.approval.as_ref(), again.boxed.as_ref()) {
                (Some(_), Some(lb)) => (Some(lb.key.clone()), format!("box:{}", lb.key), lb.answered),
                (Some(_), None) => {
                    drop(confirm_guard);
                    service.poke(&id);
                    return;
                }
                (None, _) => (None, subkey(&again.cls), false),
            };
            if again_answered {
                if let Some(live) = service.lock().live.get_mut(&id) {
                    live.unconfirmed = false;
                }
                return;
            }
            if Self::question_suppressed_locked(&service.lock(), &id, &again.cls, again.boxed.as_ref()) {
                if let Some(live) = service.lock().live.get_mut(&id) {
                    live.unconfirmed = false;
                }
                return;
            }
            if again.cls.state != cls.state || again_key != key {
                // (It didn't hold: what it shows now is read in its turn.)
                drop(confirm_guard);
                service.poke(&id);
                return;
            }
            let inspected = service.inspect(std::slice::from_ref(&entry)).await;
            // (What took a while to read may have been overtaken.)
            if !current(&service) {
                return;
            }
            if Self::question_suppressed_locked(&service.lock(), &id, &again.cls, again.boxed.as_ref()) {
                if let Some(live) = service.lock().live.get_mut(&id) {
                    live.unconfirmed = false;
                }
                return;
            }
            let info = inspected.get(&id);
            let entry = service.lock().reg.entries.iter().rev().find(|e| e.id == id).cloned().unwrap_or(entry);
            let last_message = info.and_then(|v| v["last_message"].as_str()).map(str::to_owned);
            let pending = Self::pending_tools(info);
            // Label with the EXACT box key captured on `again`, never re-reading live `box_now`.
            let approval = again.cls.approval.as_ref().map(|ap| Self::approval_json_with_box(ap, &pending, again_box.as_deref()));
            let given = service.lock().live.get(&id).and_then(|l| l.expects);
            let turn_ms = info.and_then(|v| v["turn_started_ms"].as_u64());
            let its_turn = state::of_the_turn_given(turn_ms, given);
            let tx = info.and_then(|v| v["transcript_state"].as_str()).filter(|_| its_turn);
            let (state, error, _) = olo_rex_link::agents::effective_state(&again.cls, tx, info.and_then(|v| v["transcript_error"].as_str()).filter(|_| its_turn), true);
            if state == SubagentLiveState::Idle
                && !again.reports
                && entry.cli == olo_rex_link::agents::AgentCli::Codex
                && tx == Some("working")
                && state::turn_on(entry.cli, tx, turn_ms, to_unix_ms(entry.started_at).max(0) as u64, transcript_written_ms(&entry, info), again.last_output_at, now_unix_ms(), turn_on_ms())
            {
                {
                    let mut st = service.lock();
                    if st.frozen {
                        return;
                    }
                    let Some(live) = st.live.get_mut(&id) else { return };
                    if live.check_gen != generation || live.event_gen != confirm_event_gen {
                        return;
                    }
                    live.held_until = Some(now_unix_ms() + state::TURN_ON_LOOK_MS);
                    live.seen_working = true;
                }
                service.tell(&id, SubagentLiveState::Working, None);
                tokio::time::sleep(Duration::from_millis(state::TURN_ON_LOOK_MS)).await;
                if !service.lock().frozen {
                    drop(confirm_guard);
                    service.poke(&id);
                }
                return;
            }
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
                if st.frozen {
                    return;
                }
                let Some(live) = st.live.get_mut(&id) else { return };
                match state::rest_now(now_unix_ms(), started_ms, live.seen_working, at_prompt, live.expects, written) {
                    state::Rest::Kept { until_ms } => {
                        live.held_until = Some(until_ms);
                        if !std::mem::replace(&mut live.relook, true) {
                            let (service, id) = (service.clone(), id.clone());
                            let wait = until_ms.saturating_sub(now_unix_ms()).clamp(50, 1000);
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(wait)).await;
                                if service.lock().frozen {
                                    return;
                                }
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
            // Showing-scoped list suppression (no wall-clock expiry), and
            // require a clear foreground (no staying/pending survey overlay!).
            let list_skipped = again.cls.approval.as_ref().is_some_and(|ap| service.lock().live.get(&id).is_some_and(|l| l.list_skip.is_some_and(|skip| skip.matches(ap))));
            if state == SubagentLiveState::WaitingApproval
                && !list_skipped
                && !again.survey
                && !again.survey_pending
                && let Some(ap) = &again.cls.approval
                && let Some(box_key_for_list) = again_box.as_deref().filter(|bk| service.box_still_unanswered(&id, bk))
                && let Some(asked) = service.list_allows(&id, ap)
            {
                // Track in-flight `--by-list` approval under `st.doing` so
                // handover drain waits for it and retirement/freeze never lets
                // a detached confirmation task type into a PTY or write
                // `answers.jsonl` across the final snapshot.
                struct DoingGuard {
                    service: Arc<Service>,
                }
                impl Drop for DoingGuard {
                    fn drop(&mut self) {
                        {
                            let mut st = self.service.lock();
                            st.doing = st.doing.saturating_sub(1);
                        }
                        self.service.idle.notify_one();
                    }
                }
                let admitted = {
                    let mut st = service.lock();
                    if st.retiring || st.frozen {
                        false
                    } else {
                        st.doing += 1;
                        true
                    }
                };
                if !admitted {
                    return;
                }
                let _doing = DoingGuard { service: service.clone() };
                let from = super::ops::Caller { session: Some(entry.session_id.clone()), window: None, window_from_block: false, block: None, child: None };
                let args = vec![json!(id.clone()), json!("--once"), json!("--by-list"), json!("--box"), json!(box_key_for_list), json!("--json")];
                let answered = service.approve(&json!({ "args": args }), &from).await;
                if answered.ok {
                    if let Some(live) = service.lock().live.get_mut(&id) {
                        live.unconfirmed = false;
                        live.pending_handoff = false;
                    }
                    service.leave_given(&id, &asked);
                    return;
                }
                if answered.data["dismissed"] == false {
                    logged!("olo agents: {id} asked leave its list allows and couldn't be answered ({}): it is asked as any other", answered.stderr.trim());
                    return;
                }
                if !service.box_still_unanswered(&id, box_key_for_list) {
                    return;
                }
                logged!("olo agents: {id} asked leave its list allows and couldn't be answered ({}): it is asked as any other", answered.stderr.trim());
            }
            let dedupe = if state == SubagentLiveState::WaitingApproval
                && let Some(lb) = again.boxed.as_ref()
            {
                // Generation-independent dedupe (`b{n}-{hash}`) prevents a
                // service crash/restart from re-emitting an already-reported
                // standing box while `approval["box"]` (`b{n}-g{gen}-{hash}`)
                // still uniquely binds every `--box` / `ToldGrant` token to
                // this service generation!
                format!("box:b{}-{:08x}", lb.number, lb.hash as u32)
            } else if !key.is_empty() {
                key
            } else if state == SubagentLiveState::WaitingQuestion {
                transcript_question_key(info, turn_ms)
            } else if let Some(message) = &last_message {
                use std::hash::{Hash as _, Hasher as _};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                message.hash(&mut h);
                format!("msg:{:x}", h.finish())
            } else {
                format!("scr:{:x}", olo_agent_screen::fingerprint(&again.screen))
            };
            let err_for_minder = error.clone();
            let sum_for_minder = again.cls.summary.clone();
            let mut event = service.event_of(&entry, state, again.cls.summary.clone(), dedupe);
            event.last_message = last_message;
            event.approval = approval;
            event.error = super::ops::error_words(state, error, &again.screen);
            if state == SubagentLiveState::Error
                && let Some(why) = &event.error
            {
                event.summary = Some(why.clone());
            }
            event.tail = tail_of(&again.screen, 10);
            if service.record_confirmed(&id, generation, confirm_event_gen, &again.cls, again.boxed.as_ref(), event).is_some() {
                service.tell(&id, state, again.cls.summary);
                if entry.is_shared_minder() {
                    if state == SubagentLiveState::Error {
                        let how = err_for_minder.as_deref().or(sum_for_minder.as_deref()).unwrap_or("API error");
                        service.release_minded_on_error(&entry, how);
                    } else {
                        service.lock().unhealthy_minders.remove(&entry.id);
                    }
                }
            }
        });
        if let Some(live) = self.lock().live.get_mut(id) {
            live.tasks.retain(|h| !h.is_finished());
            live.tasks.push(task.abort_handle());
        } else {
            task.abort();
        }
    }

    /// Watches a live child using TWO independent cooperating tasks:
    /// 1. Stream reader: pumps `stream.next()` without EVER calling `transition()`
    ///    inline, incrementing `out_seq` and notifying `output_notify.notify_waiters()`
    ///    immediately so dismissal waits (`screen_under_survey`, `approve`,
    ///    `trust_at_start`, `send`) wake with zero polling and zero deadlock.
    /// 2. Transition runner: debounces output (`QUIET`/`AT_MOST`) and wakes on
    ///    `poke`, calling `transition()` independently of stream consumption.
    pub fn watch_child(self: &Arc<Self>, entry: &NativeSubagent) {
        let (service, entry) = (self.clone(), entry.clone());
        let (poke, output_notify) = {
            let mut st = self.lock();
            let live = st.live.entry(entry.id.clone()).or_insert_with(Live::new);
            for old in live.tasks.drain(..) {
                old.abort();
            }
            (live.poke.clone(), live.output_notify.clone())
        };
        let id = entry.id.clone();

        // Task 1: Stream reader (never blocks on `transition()`).
        let (svc_stream, ent_stream, out_n_stream, poke_stream) = (service.clone(), entry.clone(), output_notify.clone(), poke.clone());
        let stream_task = tokio::spawn(async move {
            svc_stream.learn(&ent_stream).await;
            let mut retry = Duration::from_millis(250);
            loop {
                // Astra #4: Ensure `session.attach` succeeded before opening the
                // block stream (and re-attach on reconnect after a transient
                // failure) so a child kept live during reconcile is never
                // permanently stranded without streams or block events!
                if !svc_stream.lock().sessions.get(&ent_stream.session_id).is_some_and(|s| s.attached)
                    && let Err(e) = svc_stream.attach(&ent_stream.session_id).await
                {
                    if e.gone() || matches!(svc_stream.tab_presence(&ent_stream).await, TabPresence::Gone) {
                        svc_stream.exit(&ent_stream.id, None);
                        return;
                    }
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(Duration::from_secs(30));
                    continue;
                }
                let mut stream = match svc_stream.rex.stream(&ent_stream.session_id, &ent_stream.block_id).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        if e.gone() || matches!(svc_stream.tab_presence(&ent_stream).await, TabPresence::Gone) {
                            svc_stream.exit(&ent_stream.id, None);
                            return;
                        }
                        if let Some(s) = svc_stream.lock().sessions.get_mut(&ent_stream.session_id) {
                            s.attached = false;
                        }
                        tokio::time::sleep(retry).await;
                        retry = (retry * 2).min(Duration::from_secs(30));
                        continue;
                    }
                };
                retry = Duration::from_millis(250);
                // Mark stream connected and wake all waiters + initial transition.
                if let Some(live) = svc_stream.lock().live.get_mut(&ent_stream.id) {
                    live.stream_connected = true;
                    live.stream_gen = live.stream_gen.wrapping_add(1);
                    live.out_seq += 1;
                }
                out_n_stream.notify_waiters();
                svc_stream.notify_child_state(&ent_stream.id);
                poke_stream.notify_one();
                loop {
                    match stream.next().await {
                        Said::Output(_) => {
                            if let Some(live) = svc_stream.lock().live.get_mut(&ent_stream.id) {
                                live.out_seq += 1;
                                live.last_output_at = now_unix_ms();
                            }
                            out_n_stream.notify_waiters();
                        }
                        Said::Exited(code) => {
                            svc_stream.exit(&ent_stream.id, Some(code));
                            // (Its tab may stay: watched until it closes.)
                        }
                        Said::Resized { rows } => {
                            if let Some(live) = svc_stream.lock().live.get_mut(&ent_stream.id) {
                                live.rows = rows;
                                live.read = None;
                                live.out_seq += 1;
                            }
                            out_n_stream.notify_waiters();
                        }
                        Said::Closed => {
                            if let Some(live) = svc_stream.lock().live.get_mut(&ent_stream.id) {
                                live.stream_connected = false;
                                live.stream_gen = live.stream_gen.wrapping_add(1);
                            }
                            // Wake all wait loops immediately on disconnect so
                            // they arm their 250 ms fallback instead of waiting
                            // indefinitely on a dead stream!
                            out_n_stream.notify_waiters();
                            svc_stream.notify_child_state(&ent_stream.id);
                            break;
                        }
                    }
                }
                if matches!(svc_stream.tab_presence(&ent_stream).await, TabPresence::Gone) {
                    svc_stream.exit(&ent_stream.id, None);
                    return;
                }
                tokio::time::sleep(retry).await;
            }
        });

        // Task 2: Transition runner (debounced output + immediate `poke`).
        let (svc_tx, id_tx, out_n_tx, poke_tx) = (service.clone(), id.clone(), output_notify, poke);
        let transition_task = tokio::spawn(async move {
            let mut last_seen = svc_tx.out_seq(&id_tx);
            let mut unread: Option<(tokio::time::Instant, tokio::time::Instant)> = None;
            loop {
                if !svc_tx.lock().live.contains_key(&id_tx) {
                    return;
                }
                let out_wait = out_n_tx.notified();
                tokio::pin!(out_wait);
                out_wait.as_mut().enable();
                let poke_wait = poke_tx.notified();
                tokio::pin!(poke_wait);
                poke_wait.as_mut().enable();
                let cur = svc_tx.out_seq(&id_tx);
                if cur != last_seen {
                    last_seen = cur;
                    let now = tokio::time::Instant::now();
                    unread = Some((unread.map_or(now, |(first, _)| first), now));
                    continue;
                }
                let due = unread.map(|(first, last)| (last + QUIET).min(first + AT_MOST));
                tokio::select! {
                    _ = out_wait => {}
                    _ = async { tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)).await }, if due.is_some() => {
                        unread = None;
                        svc_tx.transition(&id_tx).await;
                    }
                    _ = poke_wait => {
                        unread = None;
                        svc_tx.transition(&id_tx).await;
                    }
                }
            }
        });

        if let Some(live) = self.lock().live.get_mut(&id) {
            live.tasks.push(stream_task.abort_handle());
            live.tasks.push(transition_task.abort_handle());
        } else {
            stream_task.abort();
            transition_task.abort();
        }
    }

    /// Three-way check for whether a child's tab still exists on the Rex server:
    /// distinguishes a confirmed `Gone` (absent from `session.view` or `e.gone()`)
    /// from an `Unknown` transient RPC error/timeout so two RPC failures never
    /// count as a closed tab.
    pub(super) async fn tab_presence(&self, entry: &NativeSubagent) -> TabPresence {
        match self.refresh(&entry.session_id).await {
            Ok(()) => {
                // Astra #5: Verify the block's mapped window matches `entry.window_id`
                // (not just independent membership in `windows` and `blocks`).
                let there = self
                    .lock()
                    .sessions
                    .get(&entry.session_id)
                    .is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.get(&entry.block_id).map(String::as_str) == Some(entry.window_id.as_str()));
                if there { TabPresence::Present } else { TabPresence::Gone }
            }
            Err(e) if e.gone() => TabPresence::Gone,
            Err(RexErr::Api { code, .. }) if code == "bad_request" => match self.session_ids().await {
                Ok(ids) if !ids.iter().any(|s| s == &entry.session_id) => TabPresence::Gone,
                Ok(_) => TabPresence::Unknown("session.view rejected a listed session id".into()),
                Err(e2) => TabPresence::Unknown(e2.to_string()),
            },
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
        if self.lock().frozen {
            return;
        }
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
    /// whose tab is confirmed gone has exited (after a restart of the server
    /// every one has, and keeps its conversation's id to be taken up again).
    ///
    /// Uses three-way presence (`Present` / `Gone` / `Unknown`): a temporary
    /// Rex failure (`session_ids`, `refresh`, or `attach` with `!e.gone()`)
    /// NEVER permanently stamps running children as exited.
    async fn reconcile(self: &Arc<Self>) -> Result<(), String> {
        let mine: Vec<NativeSubagent> = self.lock().reg.entries.iter().filter(|e| e.exited_at.is_none()).cloned().collect();
        let sessions: HashSet<String> = {
            let mut last_err = None;
            let mut found = None;
            for attempt in 0..3 {
                match self.session_ids().await {
                    Ok(ids) => {
                        found = Some(ids.into_iter().collect());
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        if attempt < 2 {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                }
            }
            match found {
                Some(s) => s,
                None if mine.is_empty() => HashSet::new(),
                None => {
                    return Err(format!("couldn't verify live Rex sessions during reconcile: {}", last_err.map_or_else(|| "unknown error".into(), |e| e.to_string())));
                }
            }
        };
        let wanted: HashSet<String> = mine.iter().map(|e| e.session_id.clone()).filter(|s| sessions.contains(s)).collect();
        let mut unknown_sessions: HashSet<String> = HashSet::new();
        for session in &wanted {
            let mut ok = false;
            let mut gone = false;
            for attempt in 0..3 {
                match self.refresh(session).await {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(e) if e.gone() => {
                        gone = true;
                        break;
                    }
                    Err(_) if attempt < 2 => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    Err(_) => break,
                }
            }
            if !ok && !gone {
                unknown_sessions.insert(session.clone());
            }
        }
        for entry in mine {
            let session_exists = sessions.contains(&entry.session_id);
            let session_unknown = unknown_sessions.contains(&entry.session_id);
            let there = self
                .lock()
                .sessions
                .get(&entry.session_id)
                .is_some_and(|s| s.windows.contains(&entry.window_id) && s.blocks.get(&entry.block_id).map(String::as_str) == Some(entry.window_id.as_str()));
            let keep_live = if session_unknown {
                true
            } else if session_exists && there {
                match self.attach(&entry.session_id).await {
                    Ok(()) => true,
                    Err(e) if e.gone() => false,
                    Err(e) => {
                        logged!("olo agents: reconcile attach failed transiently for {} ({}): keeping live and retrying via watcher", entry.id, e);
                        true
                    }
                }
            } else {
                false
            };
            if keep_live {
                // (One that a service before this rule left as its own: let go of now.)
                self.never_its_size(&entry).await;
                // Explicitly mark reconciled children as `adopted_pending = true`
                // (never inferred from wall-clock `started_at < began_ms`) so any
                // standing approval box or survey is suppressed from auto-answering
                // until resolved on an ordered, conclusive live frame.
                // Also: for ANY live minded child whose last recorded state was
                // `WaitingApproval` (whether `seen >= last_seq` OR `seen < last_seq`!),
                // reset its dedupe and arm `pending_handoff` so the new service
                // records a fresh event with this service's box key instead of
                // leaving an unread pre-handover approval stranded by `still_stands`!
                // For `WaitingQuestion`, reset dedupe when `seen >= last_seq`.
                {
                    let mut st = self.lock();
                    let mut live = Live::new();
                    live.adopted_pending = true;
                    // Restore the last confirmed box number and content hash for an adopted
                    // child so a still-open box #2+ keeps its original `box:b{n}-{hash}`
                    // dedupe identity across handover while minting a fresh generation key
                    // (Astra R2 #5).
                    if let Some(last_ev) = st.log.last_state_of(&entry.id)
                        && last_ev.state == SubagentLiveState::WaitingApproval
                        && let Some((bn, bh)) = parse_box_dedupe_key(&last_ev.dedupe_key)
                    {
                        live.boxes = bn;
                        live.adopted_box = Some((bn, bh));
                    }
                    if let Some(minder_id) = st.reg.minder_of(&entry.id).map(|m| m.id.clone())
                        && let Some((last_state, last_seq)) = st.log.last_recorded_of(&entry.id)
                        && (last_state == SubagentLiveState::WaitingApproval
                            || (last_state == SubagentLiveState::WaitingQuestion && st.seen.get(&(minder_id, entry.id.clone())).copied().unwrap_or(0) >= last_seq))
                    {
                        st.log.reset_child_dedupe(&entry.id);
                        live.pending_handoff = true;
                    }
                    st.live.insert(entry.id.clone(), live);
                }
                self.watch_child(&entry);
            } else {
                self.lock().live.insert(entry.id.clone(), Live::new());
                self.exit(&entry.id, None);
            }
        }
        // Re-arm the empty-minder timer for any live shared minder left
        // empty across a handover/restart so it never sits indefinitely.
        self.rearm_empty_minders();
        Ok(())
    }

    async fn never_its_size(&self, entry: &NativeSubagent) {
        let _ = self.rex.call("session.disown_block", json!({ "session_id": entry.session_id, "block_ids": [entry.block_id] })).await;
    }

    /// A child begins: written down, watched, said to followers.
    pub async fn begin(self: &Arc<Self>, entry: NativeSubagent, adopted: bool) -> Result<(), String> {
        self.begin_attached(entry, None, None, None, adopted, None, None).await.map(|_| ())
    }

    /// Registers a newly started child, atomically validating any chosen
    /// shared minder, converting its pending reservation to a live assignment
    /// (disarming `pending_guard` under the SAME lock so a persistence failure
    /// never double-decrements `pending_minds`), installing any validated
    /// startup `--approve-all-but` list BEFORE starting `watch_child`, and
    /// spawning startup folder-trust BEFORE awaiting `write_now()`.
    ///
    /// Follower `Frame::Agent` broadcasts stay suppressed (`live.committed =
    /// false`) until `write_now()` commits `registry.json` atomically; on
    /// failure, rolls back memory + leave file, aborts tasks, verifies tab
    /// close, and re-arms empty-minder shutdown once.
    /// Helper to close a newly created tab on registration failure (2 attempts
    /// + three-way `tab_presence` confirmation). Returns `(tab_closed, warning)`.
    async fn rollback_close_tab(&self, entry: &NativeSubagent) -> (bool, Option<String>) {
        let mut tab_closed = false;
        let mut last_close_err = None;
        for attempt in 0..2 {
            match self.rex.call("session.close_window", json!({ "session_id": entry.session_id, "window_id": entry.window_id })).await {
                Ok(_) => {
                    tab_closed = true;
                    last_close_err = None;
                    break;
                }
                Err(ce) if ce.gone() => {
                    tab_closed = true;
                    last_close_err = None;
                    break;
                }
                Err(ce) => {
                    last_close_err = Some(ce);
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
        if !tab_closed {
            if matches!(self.tab_presence(entry).await, TabPresence::Gone) {
                tab_closed = true;
            } else if let Some(ce) = last_close_err {
                logged!("olo agents: rollback failed to close tab {} for {}: {ce}", entry.window_id, entry.id);
                return (false, Some(format!(" (also failed to close tab {}: {ce}; kept live record `{}` for recovery)", entry.window_id, entry.id)));
            }
        }
        (tab_closed, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn begin_attached(
        self: &Arc<Self>,
        mut entry: NativeSubagent,
        fallback_parent: Option<&str>,
        mut pending_guard: Option<(&str, &mut super::ops::PendingMindGuard)>,
        trust_until: Option<Instant>,
        adopted: bool,
        expect_work: Option<(u64, Duration)>,
        leave_text: Option<&str>,
    ) -> Result<(NativeSubagent, Option<String>, Option<Result<usize, String>>), String> {
        if let Err(e) = self.attach(&entry.session_id).await {
            if !adopted {
                let (tab_closed, close_warn) = self.rollback_close_tab(&entry).await;
                if !tab_closed {
                    // Neon #2 / Astra #13: Tab could not be closed: fall back
                    // cleanly to parent/lead (`minded_by = None`), disarm
                    // `pending_guard` (re-arming any reused idle minder), mark
                    // record in `unsaved_recovery_ids`, and schedule persistence!
                    entry.minded_by = None;
                    if let Some(lead) = fallback_parent {
                        entry.parent_window_id = lead.to_owned();
                    }
                    let rearm_empty_minder = {
                        let mut st = self.lock();
                        let consumed = if let Some((mid, guard)) = pending_guard.take() {
                            if let Some(n) = st.pending_minds.get_mut(mid) {
                                *n = n.saturating_sub(1);
                                if *n == 0 {
                                    st.pending_minds.remove(mid);
                                }
                            }
                            guard.disarm();
                            Some(mid.to_owned())
                        } else {
                            None
                        };
                        st.reserved_ids.remove(&entry.id);
                        st.unsaved_recovery_ids.insert(entry.id.clone());
                        st.reg.register(entry.clone());
                        st.live.insert(entry.id.clone(), Live::new());
                        consumed.filter(|mid| st.reg.minded_by(mid).is_empty() && st.pending_minds.get(mid).copied().unwrap_or(0) == 0)
                    };
                    self.watch_child(&entry);
                    self.saved();
                    self.tell_entry(&entry.id);
                    if let Some(mid) = rearm_empty_minder {
                        self.arm_empty_minder_if_idle(&mid, minder_reports());
                    }
                }
                let warn = close_warn.unwrap_or_default();
                return Err(format!("the new tab couldn't be watched: {e}{warn}"));
            }
            return Err(format!("the new tab couldn't be watched: {e}"));
        }
        self.never_its_size(&entry).await;
        // Atomically write any validated startup leave list on a blocking worker
        // BEFORE registration/watch_child starts. If installation fails, neutralize
        // any pre-existing file on disk so a restart can NEVER resurrect an old
        // allow-all file; if an existing file cannot be removed, fail registration!
        let mut leave_res: Option<Result<usize, String>> = None;
        let mut parsed_leave: Option<Arc<super::leave::LeaveList>> = None;
        let mut leave_disabled_on_error = false;
        let mut wrote_leave_file = false;
        if let Some(text) = leave_text {
            let lock = self.leave_lock(&entry.id);
            let _leave_guard = lock.lock().await;
            match super::leave::LeaveList::parse(text) {
                Ok(list) => {
                    let file = self.leave_file(&entry.id);
                    match self.write_leave_file_atomic(&entry.id, text).await {
                        Ok(()) => {
                            wrote_leave_file = true;
                            leave_res = Some(Ok(list.patterns().len()));
                            parsed_leave = Some(Arc::new(list));
                        }
                        Err(e) => {
                            if let Err(rm_err) = self.remove_leave_file_blocking(&entry.id).await {
                                let (_, close_warn) = if adopted { (true, None) } else { self.rollback_close_tab(&entry).await };
                                let warn = close_warn.unwrap_or_default();
                                return Err(format!("--approve-all-but failed ({e}) and couldn't neutralize existing list file ({rm_err}){warn}"));
                            }
                            leave_disabled_on_error = true;
                            leave_res = Some(Err(format!("{}: {e}", file.display())));
                        }
                    }
                }
                Err(e) => {
                    if let Err(rm_err) = self.remove_leave_file_blocking(&entry.id).await {
                        let (_, close_warn) = if adopted { (true, None) } else { self.rollback_close_tab(&entry).await };
                        let warn = close_warn.unwrap_or_default();
                        return Err(format!("--approve-all-but invalid ({e}) and couldn't neutralize existing list file ({rm_err}){warn}"));
                    }
                    leave_disabled_on_error = true;
                    leave_res = Some(Err(e));
                }
            }
        }
        let mut minder_lost: Option<String> = None;
        let mut consumed_minder: Option<String> = None;
        {
            let mut st = self.lock();
            if let Some((mid, guard)) = pending_guard.take() {
                if let Some(n) = st.pending_minds.get_mut(mid) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        st.pending_minds.remove(mid);
                    }
                }
                // Disarm the guard atomically under the SAME lock that consumes
                // its reservation so a subsequent `write_now()` rollback never
                // decrements another concurrent start's reservation!
                guard.disarm();
                consumed_minder = Some(mid.to_owned());
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
            st.provisional_ids.insert(entry.id.clone());
            st.reg.register(entry.clone());
            if let Some(list) = parsed_leave {
                st.leave.insert(entry.id.clone(), Some(list));
            } else if leave_disabled_on_error {
                // Explicitly cache disabled (`None`) so `leave_of` never re-reads
                // disk on a failed startup list write.
                st.leave.insert(entry.id.clone(), None);
            }
            let mut live = Live::new();
            live.committed = false;
            live.adopted_pending = adopted;
            live.trust = trust_until.map(|until| TrustAtStart::of(&entry, until));
            if let Some((cursor, within)) = expect_work {
                let now = now_unix_ms();
                let turn_from_ms = to_unix_ms(entry.started_at).max(0) as u64;
                live.given = 1;
                live.expects = Some(state::Expected { cursor, given_ms: now, until_ms: now + within.as_millis() as u64, transcript_len: None, turn_from_ms, seq: 1 });
            }
            st.live.insert(entry.id.clone(), live);
            st.idle_since = None;
        }
        // Start watching and startup folder-trust BEFORE awaiting durable write
        // so a slow background save never consumes the 15 s trust window.
        // Public follower frames remain suppressed until `live.committed = true`.
        self.watch_child(&entry);
        if let Some(until) = trust_until {
            self.spawn_trust_at_start(&entry, until);
        }
        // Written down before anybody is told it started: a service killed the
        // moment after knows the tab for its child. Commit ONLY this entry
        // alongside already-committed records (excluding unrelated provisional
        // entries). Roll back cleanly on failure, keeping an owned recovery
        // record if Rex refuses to close the live tab.
        if let Err(e) = self.write_now_committing(&[entry.id.as_str()]).await {
            logged!("olo agents: failed to persist registry for {}: {e}", entry.id);
            let (tab_closed, close_warning) = if adopted { (true, None) } else { self.rollback_close_tab(&entry).await };
            if tab_closed {
                {
                    let mut st = self.lock();
                    if let Some(live) = st.live.get_mut(&entry.id) {
                        for task in live.tasks.drain(..) {
                            task.abort();
                        }
                    }
                }
                if wrote_leave_file {
                    let _ = self.remove_leave_file_blocking(&entry.id).await;
                }
                let rearm_empty_minder = {
                    let mut st = self.lock();
                    st.provisional_ids.remove(&entry.id);
                    st.unsaved_recovery_ids.remove(&entry.id);
                    st.reg.entries.retain(|r| r.id != entry.id);
                    st.live.remove(&entry.id);
                    st.leave.remove(&entry.id);
                    if let Some(sess) = st.sessions.get_mut(&entry.session_id) {
                        sess.windows.remove(&entry.window_id);
                        sess.blocks.remove(&entry.block_id);
                    }
                    consumed_minder.filter(|mid| st.reg.minded_by(mid).is_empty() && st.pending_minds.get(mid).copied().unwrap_or(0) == 0)
                };
                if let Some(mid) = rearm_empty_minder {
                    self.arm_empty_minder_if_idle(&mid, minder_reports());
                }
            } else {
                // Neon #2 / Astra #13: Close failed and tab is still live:
                // detach from any minder (`minded_by = None`, reset parent to
                // `fallback_parent`) so `ops.rs::start`'s error cleanup safely
                // removes `rules_path` and cleans up any fresh empty minder,
                // mark as `unsaved_recovery_ids` until durably persisted, and
                // schedule background save!
                let (deferred_code, deferred_tab_closed, rearm_empty_minder) = {
                    let mut st = self.lock();
                    st.provisional_ids.remove(&entry.id);
                    st.unsaved_recovery_ids.insert(entry.id.clone());
                    if let Some(reg_entry) = st.reg.entries.iter_mut().rev().find(|r| r.id == entry.id) {
                        reg_entry.minded_by = None;
                        if let Some(lead) = fallback_parent {
                            reg_entry.parent_window_id = lead.to_owned();
                        }
                    }
                    let rearm = consumed_minder.filter(|mid| st.reg.minded_by(mid).is_empty() && st.pending_minds.get(mid).copied().unwrap_or(0) == 0);
                    let (dc, dtc) = if let Some(live) = st.live.get_mut(&entry.id) {
                        live.committed = true;
                        (live.pending_exit_code.take(), std::mem::take(&mut live.pending_tab_closed))
                    } else {
                        (None, false)
                    };
                    (dc, dtc, rearm)
                };
                self.saved();
                self.tell_entry(&entry.id);
                if let Some(mid) = rearm_empty_minder {
                    self.arm_empty_minder_if_idle(&mid, minder_reports());
                }
                if let Some(code) = deferred_code {
                    self.exit(&entry.id, Some(code));
                }
                if deferred_tab_closed {
                    self.exit(&entry.id, None);
                }
            }
            let warn = close_warning.unwrap_or_default();
            return Err(format!("couldn't persist subagent registry: {e}{warn}"));
        }
        // Note: `write_now_committing` already cleared `entry.id` from
        // `provisional_ids` under `state` mutex BEFORE releasing `write_lock` (Astra #1).
        let (deferred_code, deferred_tab_closed) = {
            let mut st = self.lock();
            if let Some(live) = st.live.get_mut(&entry.id) {
                live.committed = true;
                (live.pending_exit_code.take(), std::mem::take(&mut live.pending_tab_closed))
            } else {
                (None, false)
            }
        };
        self.tell_entry(&entry.id);
        if let Some(code) = deferred_code {
            self.exit(&entry.id, Some(code));
        }
        if deferred_tab_closed {
            self.exit(&entry.id, None);
        } else if deferred_code.is_none() {
            self.poke(&entry.id);
        }
        Ok((entry, minder_lost, leave_res))
    }

    /// Independent verification for a pathless primary folder-trust prompt:
    /// requires a verified current **foreground** PID (never falling back to
    /// outer launch shell `child.pid` or cached `live.pid`) and readable
    /// `/proc/{pid}/cwd` matching `primary`. Fails closed (`false`) on RPC
    /// error, missing foreground PID, or unreadable procfs cwd.
    async fn process_cwd_matches_primary(&self, entry: &NativeSubagent, primary: Option<&str>, deadline: tokio::time::Instant) -> bool {
        let Some(primary_norm) = primary.and_then(norm_dir) else { return false };
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if rem.is_zero() {
            return false;
        }
        let Ok(p) = self.rex.terminal_within(&entry.session_id, &entry.block_id, "process", json!({}), rem).await else {
            return false;
        };
        let Some(pid) = p["foreground"]["pid"].as_i64().and_then(|v| i32::try_from(v).ok()).filter(|v| *v > 0) else {
            return false;
        };
        let Ok(proc_cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            return false;
        };
        if norm_dir(&proc_cwd.to_string_lossy()).as_deref() == Some(primary_norm.as_str()) {
            return true;
        }
        let primary_canon = std::fs::canonicalize(&primary_norm).ok();
        let proc_canon = std::fs::canonicalize(&proc_cwd).ok();
        matches!((primary_canon, proc_canon), (Some(a), Some(b)) if a == b)
    }

    /// Startup `--trust` worker spawned immediately upon registration:
    /// - Strictly bounded by `until` (captured BEFORE `session.new_window`) on
    ///   all reads, locks, and key writes (`look_until`, `write_within`).
    /// - Requires `looked.is_fresh_ordered()` and re-checks `TrustAtStart::owns_parts`
    ///   AND intended folder on EVERY verifying frame (including after Down).
    /// - Verifies process cwd binding on pathless primary prompts (fail-closed)
    ///   and revalidates the prompt + selection if the screen changed during
    ///   the process query.
    /// - Waits across pathless `--add-dir` partial frames without claiming.
    /// - Separates `arrows_sent` from `answered` (Return submitted) and waits
    ///   through blank repaints for the full 3 s navigation budget.
    /// - Never exits early on blank `Working("starting")` between primary and
    ///   `--add-dir` prompts; exits early only on positive `in_turn` / `AT_PROMPT`
    ///   or once all launch folders are answered.
    /// - Zero stationary polling while `stream_connected` (250 ms fallback only
    ///   while disconnected).
    fn spawn_trust_at_start(self: &Arc<Self>, entry: &NativeSubagent, until: Instant) {
        let (service, entry) = (self.clone(), entry.clone());
        let id = entry.id.clone();
        let task = tokio::spawn(async move {
            let deadline = tokio::time::Instant::from_std(until);
            let primary: Option<String> = entry.cwd.as_deref().and_then(norm_dir);
            let add_dirs: Vec<String> = entry.add_dirs.iter().filter_map(|d| norm_dir(d)).collect();
            let which = TrustAtStart::which;
            let is_own = |trust: &olo_agent_screen::FolderTrustPrompt| TrustAtStart::owns_parts(primary.as_deref(), &add_dirs, trust);

            struct AtStart<'a>(&'a Service, &'a str);
            impl Drop for AtStart<'_> {
                fn drop(&mut self) {
                    self.0.trust_over(self.1);
                }
            }
            let _at_start = AtStart(&service, &entry.id);

            struct OnItsWay<'a>(&'a Service, &'a str);
            impl Drop for OnItsWay<'_> {
                fn drop(&mut self) {
                    self.0.trust_keys_done(self.1);
                }
            }

            let notifies = service.child_notifies(&entry.id);
            // Returns `Some(true)` if woken by fallback/disconnect (caller MUST
            // force a fresh live read), `Some(false)` on stream/state change, or
            // `None` when `until_inst` expired.
            let wait_for_change = |last_out: u64, last_sgen: u64, until_inst: tokio::time::Instant| {
                let svc = service.clone();
                let id_w = entry.id.clone();
                let notifies = notifies.clone();
                async move {
                    let now_inst = tokio::time::Instant::now();
                    if now_inst >= until_inst {
                        return None;
                    }
                    let disconnected = !svc.stream_connected(&id_w);
                    let fallback = disconnected.then(|| (now_inst + Duration::from_millis(250)).min(until_inst));
                    let sleep_to = fallback.unwrap_or(until_inst);
                    if let Some((out_n, st_n, _)) = notifies {
                        let out_wait = out_n.notified();
                        tokio::pin!(out_wait);
                        out_wait.as_mut().enable();
                        let st_wait = st_n.notified();
                        tokio::pin!(st_wait);
                        st_wait.as_mut().enable();
                        if svc.out_seq(&id_w) != last_out {
                            return Some(false);
                        }
                        if svc.stream_gen(&id_w) != last_sgen {
                            return Some(true);
                        }
                        tokio::select! {
                            _ = out_wait => Some(svc.stream_gen(&id_w) != last_sgen),
                            _ = st_wait => Some(svc.stream_gen(&id_w) != last_sgen),
                            _ = tokio::time::sleep_until(sleep_to) => (fallback.is_some() && tokio::time::Instant::now() < until_inst).then_some(true),
                        }
                    } else {
                        tokio::time::sleep_until(sleep_to).await;
                        (fallback.is_some() && tokio::time::Instant::now() < until_inst).then_some(true)
                    }
                }
            };

            let mut arrows_sent: HashSet<Option<String>> = HashSet::new();
            let mut answered: HashSet<Option<String>> = HashSet::new();
            let mut need_fresh = false;
            loop {
                if tokio::time::Instant::now() >= deadline || service.lock().frozen {
                    break;
                }
                let out_before = service.out_seq(&entry.id);
                let sgen_before = service.stream_gen(&entry.id);
                let looked = service.look_until(&entry, need_fresh, Some(deadline)).await;
                need_fresh = false;
                if looked.cls.state == SubagentLiveState::Exited || service.lock().frozen {
                    break;
                }
                if looked.rpc_failed || looked.survey || looked.survey_pending {
                    let Some(force) = wait_for_change(out_before, sgen_before, deadline).await else { break };
                    need_fresh = force;
                    continue;
                }
                if let Some(trust) = looked.cls.folder_trust {
                    // Partial `--add-dir` frame whose path hasn't drawn yet:
                    // wait on output without claiming or pressing Return.
                    if trust.is_add_dir && trust.folder.is_none() {
                        if add_dirs.is_empty() {
                            break;
                        }
                        let Some(force) = wait_for_change(out_before, sgen_before, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    if !is_own(&trust) {
                        break;
                    }
                    if answered.contains(&which(&trust)) {
                        let Some(force) = wait_for_change(out_before, sgen_before, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    if !trust.marked {
                        let Some(force) = wait_for_change(out_before, sgen_before, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    // Acquire per-child answer lock strictly within deadline.
                    let answers = service.answer_lock(&entry.id);
                    let _held = tokio::select! {
                        held = answers.lock_owned() => held,
                        _ = tokio::time::sleep_until(deadline) => break,
                    };
                    if tokio::time::Instant::now() >= deadline || service.lock().frozen {
                        break;
                    }
                    let out_locked = service.out_seq(&entry.id);
                    let sgen_locked = service.stream_gen(&entry.id);
                    let fresh = service.look_until(&entry, true, Some(deadline)).await;
                    if fresh.cls.state == SubagentLiveState::Exited || service.lock().frozen {
                        break;
                    }
                    if !fresh.is_fresh_ordered() || fresh.survey || fresh.survey_pending {
                        drop(_held);
                        let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    let Some(mut trust) = fresh.cls.folder_trust else {
                        drop(_held);
                        let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    };
                    if trust.is_add_dir && trust.folder.is_none() {
                        drop(_held);
                        if add_dirs.is_empty() {
                            break;
                        }
                        let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    if !is_own(&trust) {
                        break;
                    }
                    if !trust.is_add_dir && trust.folder.is_none() {
                        if !service.process_cwd_matches_primary(&entry, primary.as_deref(), deadline).await {
                            break;
                        }
                        // ALWAYS perform one fresh ordered screen recheck right
                        // after the process query (even when `!stream_connected`)
                        // so a prompt change or survey appearing during the RPC
                        // can never receive stale arrows or Return!
                        let recheck = service.look_until(&entry, true, Some(deadline)).await;
                        if recheck.cls.state == SubagentLiveState::Exited || service.lock().frozen {
                            break;
                        }
                        if !recheck.is_fresh_ordered() || recheck.survey || recheck.survey_pending {
                            drop(_held);
                            let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                            need_fresh = force;
                            continue;
                        }
                        let Some(rt) = recheck.cls.folder_trust else {
                            drop(_held);
                            let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                            need_fresh = force;
                            continue;
                        };
                        if !is_own(&rt) || which(&rt) != which(&trust) {
                            break;
                        }
                        trust = rt;
                    }
                    let key = which(&trust);
                    if answered.contains(&key) {
                        drop(_held);
                        let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    if !trust.marked {
                        drop(_held);
                        let Some(force) = wait_for_change(out_locked, sgen_locked, deadline).await else { break };
                        need_fresh = force;
                        continue;
                    }
                    if tokio::time::Instant::now() >= deadline || service.lock().frozen {
                        break;
                    }

                    // Mark keys on their way (do not mark Return answered until Return is committed).
                    let delta = trust.trust_index as isize - trust.selected_index as isize;
                    service.trust_taken(&entry.id, key.clone(), false);
                    let _on_its_way = OnItsWay(&service, &entry.id);
                    let same_target = |t: &olo_agent_screen::FolderTrustPrompt| is_own(t) && which(t) == key;

                    let mut landed = delta == 0;
                    if delta != 0 {
                        if arrows_sent.contains(&key) {
                            // Already sent arrows for this prompt once: do not walk again.
                            answered.insert(key.clone());
                            service.trust_taken(&entry.id, key.clone(), true);
                            drop(_on_its_way);
                            continue;
                        }
                        arrows_sent.insert(key.clone());
                        let arrow = if delta > 0 { "\x1b[B" } else { "\x1b[A" };
                        let mut out_before_arrows = service.out_seq(&entry.id);
                        let mut sgen_before_arrows = service.stream_gen(&entry.id);
                        let mut pressed = true;
                        for step in 0..delta.unsigned_abs() {
                            if service.survey_active(&entry.id) || service.lock().frozen {
                                pressed = false;
                                break;
                            }
                            // Astra #16: After an intervening sleep between arrows,
                            // re-verify the live foreground screen before pressing
                            // the next arrow so a survey or different dialog never
                            // receives startup-trust arrow keys!
                            if step > 0 {
                                let step_now = service.look_ordered(&entry, Some(deadline)).await;
                                if !step_now.is_fresh_ordered()
                                    || step_now.survey
                                    || step_now.survey_pending
                                    || service.survey_active(&entry.id)
                                    || !step_now.cls.folder_trust.as_ref().is_some_and(same_target)
                                {
                                    pressed = false;
                                    break;
                                }
                            }
                            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
                            if rem.is_zero() || service.lock().frozen || service.rex.write_within(&entry.session_id, &entry.block_id, arrow, rem).await.is_err() {
                                pressed = false;
                                break;
                            }
                            if step + 1 < delta.unsigned_abs() {
                                let sleep_to = (tokio::time::Instant::now() + Duration::from_millis(50)).min(deadline);
                                tokio::time::sleep_until(sleep_to).await;
                            }
                        }
                        if pressed {
                            let by = (tokio::time::Instant::now() + Duration::from_secs(3)).min(deadline);
                            loop {
                                if tokio::time::Instant::now() >= by || service.lock().frozen {
                                    break;
                                }
                                if service.out_seq(&entry.id) == out_before_arrows
                                    && service.stream_gen(&entry.id) == sgen_before_arrows
                                    && wait_for_change(out_before_arrows, sgen_before_arrows, by).await.is_none()
                                {
                                    break;
                                }
                                out_before_arrows = service.out_seq(&entry.id);
                                sgen_before_arrows = service.stream_gen(&entry.id);
                                let again = service.look_until(&entry, true, Some(by)).await;
                                if again.cls.state == SubagentLiveState::Exited || service.lock().frozen {
                                    break;
                                }
                                if !again.is_fresh_ordered() {
                                    continue;
                                }
                                if again.survey || again.survey_pending {
                                    // Survey overlay appeared on top of trust dialog: abort without Return!
                                    break;
                                }
                                match again.cls.folder_trust {
                                    Some(t) if !same_target(&t) => {
                                        // Out-of-scope or different folder appeared after Down: abort without Return!
                                        break;
                                    }
                                    Some(t) if !t.is_add_dir && t.folder.is_none() => {
                                        if !service.process_cwd_matches_primary(&entry, primary.as_deref(), by).await {
                                            break;
                                        }
                                        // End verification with a fresh ordered screen read right after
                                        // the process query (including when `!stream_connected`) so a
                                        // prompt or mark change during the RPC can never receive Return!
                                        let recheck = service.look_until(&entry, true, Some(by)).await;
                                        if recheck.cls.state == SubagentLiveState::Exited || service.lock().frozen || !recheck.is_fresh_ordered() || recheck.survey || recheck.survey_pending {
                                            break;
                                        }
                                        let Some(rt) = recheck.cls.folder_trust else { break };
                                        if !same_target(&rt) {
                                            break;
                                        }
                                        if rt.marked && rt.selected_index == rt.trust_index {
                                            landed = true;
                                            break;
                                        }
                                    }
                                    Some(t) if t.marked && t.selected_index == t.trust_index => {
                                        landed = true;
                                        break;
                                    }
                                    Some(_) => {}
                                    // Blank repaint: wait through full `by` budget unless agent is positively elsewhere or in an approval.
                                    None if is_positively_elsewhere(&again.cls) || again.cls.approval.is_some() => break,
                                    None => {}
                                }
                            }
                        }
                    }

                    // Single Return per prompt, strictly within deadline.
                    answered.insert(key.clone());
                    service.trust_taken(&entry.id, key.clone(), true);
                    let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if landed && !rem.is_zero() && !service.lock().frozen {
                        let mut out_before_ret = service.out_seq(&entry.id);
                        let mut sgen_before_ret = service.stream_gen(&entry.id);
                        if service.rex.write_within(&entry.session_id, &entry.block_id, "\r", rem).await.is_ok() {
                            let by = (tokio::time::Instant::now() + Duration::from_secs(3)).min(deadline);
                            let mut none_since: Option<tokio::time::Instant> = None;
                            loop {
                                if tokio::time::Instant::now() >= by || service.lock().frozen {
                                    break;
                                }
                                let blank_due = none_since.map(|s| (s + Duration::from_millis(150)).min(by));
                                let wake_at = blank_due.unwrap_or(by);
                                let cur_out = service.out_seq(&entry.id);
                                if cur_out == out_before_ret
                                    && service.stream_gen(&entry.id) == sgen_before_ret
                                    && !none_since.is_some_and(|s| s.elapsed() >= Duration::from_millis(150))
                                    && wait_for_change(out_before_ret, sgen_before_ret, wake_at).await.is_none()
                                    && !none_since.is_some_and(|s| s.elapsed() >= Duration::from_millis(150))
                                {
                                    break;
                                }
                                out_before_ret = service.out_seq(&entry.id);
                                sgen_before_ret = service.stream_gen(&entry.id);
                                let again = service.look_until(&entry, true, Some(by)).await;
                                if again.cls.state == SubagentLiveState::Exited {
                                    break;
                                }
                                if !again.is_fresh_ordered() {
                                    continue;
                                }
                                match again.cls.folder_trust {
                                    Some(t) if same_target(&t) => none_since = None,
                                    Some(_) => break,
                                    None if is_positively_elsewhere(&again.cls) || again.cls.approval.is_some() => break,
                                    None => match none_since {
                                        Some(since) if since.elapsed() >= Duration::from_millis(150) => break,
                                        Some(_) => {}
                                        None => none_since = Some(tokio::time::Instant::now()),
                                    },
                                }
                            }
                        }
                    }
                    drop(_on_its_way);
                    if add_dirs.is_empty() {
                        break;
                    }
                    continue;
                }
                // Do NOT terminate early on a blank frame (`Working("starting")`):
                // only exit early on positive evidence of a running turn or prompt!
                if !answered.is_empty() && is_positively_elsewhere(&looked.cls) {
                    break;
                }
                let Some(force) = wait_for_change(out_before, sgen_before, deadline).await else {
                    break;
                };
                need_fresh = force;
            }
        });
        if let Some(live) = self.lock().live.get_mut(&id) {
            live.tasks.push(task.abort_handle());
        } else {
            task.abort();
        }
    }

    // ---- What is written down ----

    /// Test-only delay/barrier/handover hooks are honored ONLY when
    /// `OLO_REX_AGENTS_DIR` is an absolute isolated path (matching `Paths::of`
    /// and `client::service_env_from`).
    pub(super) fn test_env_allowed() -> bool {
        olo_rex_link::agents::wire::is_isolated_agents_dir(std::env::var_os("OLO_REX_AGENTS_DIR").as_deref())
    }

    /// Writes the registry whenever there is something of it to write,
    /// until it is told to stop (a service at its end: its last write
    /// is `serve`'s own, after this task is over). One write at a time
    /// with the others that write it (`write_lock`), so what is written
    /// last is what is newest. (An event's line is written as it is
    /// recorded, `record`.)
    async fn writer(self: Arc<Self>, mut stop: tokio::sync::oneshot::Receiver<()>) {
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => return,
                _ = self.save.notified() => {
                    if let Err(e) = self.write(true).await {
                        logged!("olo agents: background registry save failed: {e}");
                    }
                }
            }
        }
    }

    pub(crate) async fn write_now_committing(self: &Arc<Self>, committing_ids: &[&str]) -> std::io::Result<()> {
        self.write_with_commit(false, committing_ids).await
    }

    pub(crate) async fn write_now(self: &Arc<Self>) -> std::io::Result<()> {
        self.write_with_commit(false, &[]).await
    }

    /// Compacts `events.jsonl` when larger than `threshold` bytes without
    /// losing concurrent appends:
    /// 1. Snapshot `st.log` in memory up to `snap_seq` (brief lock).
    /// 2. Write and `sync_all()` the snapshot off-lock to a private
    ///    `create_new(true).mode(0o600)` temp file (with collision retry and
    ///    unlink-only-if-created guard).
    /// 3. Catch up any concurrent appends off-lock (flushing and `sync_all()`
    ///    off-lock in up to 5 passes), holding `self.lock()` ONLY for the
    ///    microsecond tail-empty check and `fs::rename(&tmp, &paths.events)`
    ///    (zero `sync_all()` under `self.lock()`).
    fn compact_events_if_needed(&self, threshold: u64) -> std::io::Result<()> {
        self.compact_events_with_hook(threshold, || {})
    }

    fn compact_events_with_hook(&self, threshold: u64, mut after_snapshot: impl FnMut()) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COMPACT_SEQ: AtomicU64 = AtomicU64::new(1);

        if !std::fs::metadata(&self.paths.events).is_ok_and(|m| m.len() > threshold) {
            return Ok(());
        }
        let (mut snap_seq, snap_events) = self.lock().log.snapshot();

        struct TempGuard {
            path: PathBuf,
            armed: bool,
        }
        impl Drop for TempGuard {
            fn drop(&mut self) {
                if self.armed {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }

        let mut created = None;
        for _ in 0..64 {
            let n = COMPACT_SEQ.fetch_add(1, Ordering::Relaxed);
            let cand = self.paths.events.with_extension(format!("jsonl.tmp.{}.{n}", std::process::id()));
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&cand) {
                Ok(f) => {
                    created = Some((cand, f));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        let Some((tmp, f)) = created else {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "couldn't allocate unique compaction temp file"));
        };
        let mut guard = TempGuard { path: tmp.clone(), armed: true };
        let mut writer = std::io::BufWriter::new(f);
        for ev in &snap_events {
            let mut line = serde_json::to_string(ev).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            writer.write_all(line.as_bytes())?;
        }
        writer.flush()?;
        let mut f = writer.into_inner().map_err(|e| e.into_error())?;
        // Always `sync_all()` completely OFF the global state mutex so slow
        // storage never blocks the single-threaded Tokio reactor!
        f.sync_all()?;
        after_snapshot();

        // Catch up any concurrent appends off-lock, then perform only the
        // microsecond atomic `rename` under `self.lock()` once the tail is empty
        // (ZERO `sync_all()` while holding `self.lock()`).
        for _ in 0..5 {
            let tail = {
                let st = self.lock();
                if st.log.has_gap(snap_seq) {
                    return Ok(());
                }
                let tail = st.log.after_seq(snap_seq);
                if tail.is_empty() {
                    drop(f);
                    std::fs::rename(&tmp, &self.paths.events)?;
                    guard.armed = false;
                    return Ok(());
                }
                tail
            };
            for ev in &tail {
                let mut line = serde_json::to_string(ev).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                line.push('\n');
                f.write_all(line.as_bytes())?;
                snap_seq = snap_seq.max(ev.seq);
            }
            f.sync_all()?;
        }
        Ok(())
    }

    /// `the_writers`: the writer task's own write (which isolated tests may
    /// gate with `OLO_REX_AGENTS_TEST_WRITER_DELAY_MS` or
    /// `OLO_REX_AGENTS_TEST_WRITER_BARRIER` when `OLO_REX_AGENTS_DIR` is set).
    async fn write(self: &Arc<Self>, the_writers: bool) -> std::io::Result<()> {
        self.write_with_commit(the_writers, &[]).await
    }

    /// Snapshots committed registry state (plus any `committing_ids` explicitly
    /// being committed by this request, excluding all OTHER provisional entries
    /// so an unrelated writer can never publish a registration that later rolls
    /// back!) and persists `minder-state.json` then `registry.json`.
    async fn write_with_commit(self: &Arc<Self>, the_writers: bool, committing_ids: &[&str]) -> std::io::Result<()> {
        // (Its turn first, then the copy: what is written is what is
        // newest when the write before it has landed.)
        let held = self.write_lock.clone().lock_owned().await;
        let test_ok = the_writers && Self::test_env_allowed();
        let slow = if test_ok { std::env::var("OLO_REX_AGENTS_TEST_WRITER_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0).map(|ms| ms.min(5000)) } else { None };
        let barrier = if test_ok {
            std::env::var("OLO_REX_AGENTS_TEST_WRITER_BARRIER").ok().and_then(|v| {
                let (enter, release) = v.split_once(':')?;
                (!enter.is_empty() && !release.is_empty()).then(|| (PathBuf::from(enter), PathBuf::from(release)))
            })
        } else {
            None
        };
        let (reg, minder_state, persisted_recovery_ids) = {
            let st = self.lock();
            let ms = MinderStateDisk::from_state(&st);
            let mut reg = st.reg.clone();
            if !st.provisional_ids.is_empty() {
                reg.entries.retain(|e| !st.provisional_ids.contains(&e.id) || committing_ids.contains(&e.id.as_str()));
            }
            let rec_ids: Vec<String> = reg.entries.iter().filter(|e| st.unsaved_recovery_ids.contains(&e.id)).map(|e| e.id.clone()).collect();
            (reg, ms, rec_ids)
        };
        let commit_owned: Vec<String> = committing_ids.iter().map(|s| (*s).to_owned()).collect();
        let service = self.clone();
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let _held = held;
            if let Some((enter, release)) = barrier
                && !enter.exists()
            {
                let _ = std::fs::write(&enter, b"entered\n");
                let wait_until = Instant::now() + Duration::from_secs(15);
                while !release.exists() && Instant::now() < wait_until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !release.exists() {
                    let _ = std::fs::write(enter.with_extension("expired"), b"expired\n");
                }
            }
            if let Some(ms) = slow {
                std::thread::sleep(Duration::from_millis(ms));
            }
            // Write `minder-state.json` FIRST and publish `registry.json` LAST
            // so `registry.json`'s atomic rename is the registration commit
            // point: a failure on either save never leaves a rolled-back child
            // published in `registry.json` on disk!
            minder_state.save_to(&paths.dir.join("minder-state.json"))?;
            reg.save_to(&paths.registry)?;
            // Astra #1: Clear successfully committed IDs from `provisional_ids`
            // (and any persisted `unsaved_recovery_ids`) under the state mutex
            // BEFORE releasing `write_lock` (`_held`) so no queued writer B can
            // snapshot without A and erase A after A returns success!
            if !commit_owned.is_empty() || !persisted_recovery_ids.is_empty() {
                let mut st = service.lock();
                for id in &commit_owned {
                    st.provisional_ids.remove(id);
                    st.unsaved_recovery_ids.remove(id);
                }
                for id in &persisted_recovery_ids {
                    st.unsaved_recovery_ids.remove(id);
                }
            }
            if let Err(e) = service.compact_events_if_needed(16 << 20) {
                logged!("olo agents: events compaction failed: {e}");
            }
            Ok(())
        })
        .await
        .map_err(|e| std::io::Error::other(format!("writer task failed: {e}")))?
    }

    // ---- Those that ask ----

    /// Builds a consistent `Frame::Snapshot` under a single lock acquisition
    /// (avoiding the self-deadlock when a temporary guard lived across `views()`),
    /// excluding ALL provisional uncommitted entries (both live and ended — Astra #3).
    fn snapshot(&self) -> Frame {
        let st = self.lock();
        let cursor = st.log.latest_seq();
        let agents = st
            .reg
            .entries
            .iter()
            .filter(|entry| !st.provisional_ids.contains(&entry.id) && !(entry.exited_at.is_none() && st.live.get(&entry.id).is_some_and(|l| !l.committed)))
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

    /// Validates that an adopted child ID is a single normal filesystem
    /// component (rejecting empty, `/`, `\`, `..`, `.`, or NUL so list/rules
    /// path joins can never escape agentd's state directory — Astra Addl #1).
    fn is_safe_child_id(id: &str) -> bool {
        if id.is_empty() || id.len() > 128 || id.contains('\0') || id.contains('/') || id.contains('\\') {
            return false;
        }
        let mut comps = std::path::Path::new(id).components();
        matches!((comps.next(), comps.next()), (Some(std::path::Component::Normal(_)), None))
    }

    /// Children an earlier Olo's desktop kept (it was their keeper then):
    /// taken over once, those whose tabs are still there as live.
    ///
    /// - Rejects missing/malformed JSON payloads or unsafe path IDs (exit 2;
    ///   never acknowledges `"Took over 0"`).
    /// - Serializes concurrent migrations through commit via `adopt_lock`.
    /// - Validates batch-internal duplicates/conflicts AND checks in-flight
    ///   `provisional_ids`/`reserved_ids` and durable committed records
    ///   (flushing any `unsaved_recovery_ids` before acknowledging duplicates).
    /// - Uses three-way `tab_presence` (`Present` / `Gone` / `Unknown`) so a
    ///   temporary Rex error never stamps a live child as exited.
    /// - Fails WITHOUT `"Took over"` in stdout if ANY live registration or
    ///   durable ended-record persistence fails.
    async fn adopt(self: &Arc<Self>, obj: Option<&Value>) -> Response {
        let Some(agents_val) = obj.and_then(|v| v.get("agents")) else {
            return Response::err("adopt", "malformed adopt payload: missing `agents` array", 2);
        };
        if !agents_val.is_array() {
            return Response::err("adopt", "malformed adopt payload: `agents` must be an array", 2);
        }
        let given: Vec<NativeSubagent> = match serde_json::from_value(agents_val.clone()) {
            Ok(v) => v,
            Err(e) => return Response::err("adopt", format!("malformed adopt payload: {e}"), 2),
        };
        // Pre-validate every ID is a safe single path component AND check
        // batch-internal duplicates/conflicts BEFORE staging anything (Astra #3 / Addl #1).
        let mut deduped: Vec<NativeSubagent> = Vec::with_capacity(given.len());
        for entry in given {
            if !Self::is_safe_child_id(&entry.id) {
                return Response::err("adopt", format!("invalid adopted child id `{}`: must be a single path component", entry.id), 2);
            }
            if let Some(prev) = deduped.iter().find(|e| e.id == entry.id || (e.session_id == entry.session_id && e.window_id == entry.window_id)) {
                if prev.id == entry.id && prev.session_id == entry.session_id && prev.window_id == entry.window_id && prev.block_id == entry.block_id {
                    continue;
                }
                return Response::err(
                    "adopt",
                    format!("conflicting records inside adopt batch: `{}` (tab {}/{}) vs `{}` (tab {}/{})", prev.id, prev.session_id, prev.window_id, entry.id, entry.session_id, entry.window_id),
                    1,
                );
            }
            deduped.push(entry);
        }
        let _adopt_guard = self.adopt_lock.lock().await;
        let mut taken = 0usize;
        let mut ended_added: Vec<String> = Vec::new();
        for mut entry in deduped {
            entry.host_key = HOST.to_owned();
            // Imported records never carry minder privileges or assignments:
            // strip them unconditionally so `adopt` cannot forge a minder.
            entry.shared_minder = false;
            entry.minded_by = None;
            entry.minds = None;
            if entry.role.as_deref().is_some_and(|r| r.trim().eq_ignore_ascii_case("minder")) {
                entry.role = None;
            }
            // Check against in-flight starts (`reserved_ids`, other `provisional_ids`),
            // already-staged `ended_added`, and durable committed records (Astra #3 / #13 / Neon #6).
            enum DupOutcome {
                New,
                DurableDup,
                UnsavedRecoveryDup,
                Conflict(String),
            }
            let dup_check = {
                let st = self.lock();
                if st.reserved_ids.contains(&entry.id) {
                    DupOutcome::Conflict(format!("adopted entry `{}` conflicts with an in-flight start", entry.id))
                } else if let Some(existing) = st.reg.entries.iter().find(|e| e.id == entry.id || (e.session_id == entry.session_id && e.window_id == entry.window_id)) {
                    let exact = existing.id == entry.id && existing.session_id == entry.session_id && existing.window_id == entry.window_id && existing.block_id == entry.block_id;
                    let other_provisional = st.provisional_ids.contains(&existing.id) && !ended_added.contains(&existing.id);
                    if other_provisional {
                        DupOutcome::Conflict(format!("adopted entry `{}` conflicts with an uncommitted in-flight registration", entry.id))
                    } else if !exact {
                        DupOutcome::Conflict(format!(
                            "conflicting existing record `{}` (tab {}/{}) for adopted entry `{}` (tab {}/{})",
                            existing.id, existing.session_id, existing.window_id, entry.id, entry.session_id, entry.window_id
                        ))
                    } else if st.unsaved_recovery_ids.contains(&existing.id) {
                        DupOutcome::UnsavedRecoveryDup
                    } else {
                        DupOutcome::DurableDup
                    }
                } else {
                    DupOutcome::New
                }
            };
            match dup_check {
                DupOutcome::DurableDup => continue,
                DupOutcome::UnsavedRecoveryDup => {
                    // Astra #13: Never acknowledge a duplicate whose recovery
                    // record is not yet durably saved on disk!
                    if let Err(e) = self.write_now().await {
                        if !ended_added.is_empty() {
                            let mut st = self.lock();
                            st.reg.entries.retain(|r| !ended_added.contains(&r.id));
                            for id in &ended_added {
                                st.provisional_ids.remove(id);
                            }
                        }
                        return Response::err("adopt", format!("failed to persist existing recovery record `{}`: {e}", entry.id), 1);
                    }
                    continue;
                }
                DupOutcome::New => {}
                DupOutcome::Conflict(why) => {
                    if !ended_added.is_empty() {
                        let mut st = self.lock();
                        st.reg.entries.retain(|r| !ended_added.contains(&r.id));
                        for id in &ended_added {
                            st.provisional_ids.remove(id);
                        }
                    }
                    return Response::err("adopt", why, 1);
                }
            }
            if entry.exited_at.is_none() {
                match self.tab_presence(&entry).await {
                    TabPresence::Present => {
                        if let Err(e) = self.begin(entry, true).await {
                            if !ended_added.is_empty() {
                                let mut st = self.lock();
                                st.reg.entries.retain(|r| !ended_added.contains(&r.id));
                                for id in &ended_added {
                                    st.provisional_ids.remove(id);
                                }
                            }
                            return Response::err("adopt", format!("failed to persist adopted live child: {e}"), 1);
                        }
                        taken += 1;
                    }
                    TabPresence::Gone => {
                        entry.exited_at = Some(now_unix_ms() as i64);
                        let id = entry.id.clone();
                        {
                            let mut st = self.lock();
                            st.provisional_ids.insert(id.clone());
                            st.reg.entries.push(entry);
                        }
                        ended_added.push(id);
                        taken += 1;
                    }
                    TabPresence::Unknown(why) => {
                        if !ended_added.is_empty() {
                            let mut st = self.lock();
                            st.reg.entries.retain(|r| !ended_added.contains(&r.id));
                            for id in &ended_added {
                                st.provisional_ids.remove(id);
                            }
                        }
                        return Response::err("adopt", format!("couldn't verify tab presence for live child `{}`: {why}", entry.id), 1);
                    }
                }
            } else {
                let id = entry.id.clone();
                {
                    let mut st = self.lock();
                    st.provisional_ids.insert(id.clone());
                    st.reg.entries.push(entry);
                }
                ended_added.push(id);
                taken += 1;
            }
        }
        if !ended_added.is_empty() {
            let commit_refs: Vec<&str> = ended_added.iter().map(String::as_str).collect();
            if let Err(e) = self.write_now_committing(&commit_refs).await {
                let mut st = self.lock();
                st.reg.entries.retain(|r| !ended_added.contains(&r.id));
                for id in &ended_added {
                    st.provisional_ids.remove(id);
                }
                return Response::err("adopt", format!("failed to persist adopted child records: {e}"), 1);
            }
            // Note: `write_now_committing` already removed `ended_added` from
            // `provisional_ids` under `state` mutex BEFORE releasing `write_lock`.
            for id in &ended_added {
                self.tell_entry(id);
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
            if self.lock().retiring {
                return;
            }
            // (Asked for before the picture is taken: nothing between the two is missed.)
            let mut frames = self.frames.subscribe();
            let first = self.snapshot();
            if write.write_all((serde_json::to_string(&first).unwrap_or_default() + "\n").as_bytes()).await.is_err() {
                return;
            }
            let mut gone = String::new();
            loop {
                tokio::select! {
                    _ = self.wait_retiring() => return,
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
        // Check `retiring` atomically under `self.lock()` right alongside
        // incrementing `doing` / `active_watches` BEFORE writing `TAKEN` (`\n`),
        // so any connection accepted after retirement begins gets `NotTaken`
        // and retries cleanly on the successor.
        let waits = matches!(req.op.as_str(), "watch" | "wait");
        let is_retire = req.op == "retire";
        struct ActiveOpGuard<'a> {
            service: &'a Service,
            waits: bool,
            counted: bool,
        }
        impl Drop for ActiveOpGuard<'_> {
            fn drop(&mut self) {
                if !self.counted {
                    return;
                }
                {
                    let mut st = self.service.lock();
                    if self.waits {
                        st.active_watches = st.active_watches.saturating_sub(1);
                    } else {
                        st.doing = st.doing.saturating_sub(1);
                    }
                }
                self.service.idle.notify_one();
            }
        }
        {
            let mut st = self.lock();
            if st.retiring {
                return;
            }
            if waits {
                st.active_watches += 1;
            } else if !is_retire {
                st.doing += 1;
            }
        }
        let active_guard = ActiveOpGuard { service: &self, waits, counted: !is_retire };
        // Taken: said before anything is done (`wire::TAKEN`).
        if tokio::time::timeout(Duration::from_secs(5), write.write_all(b"\n")).await.map_or(true, |r| r.is_err()) {
            return;
        }
        // (An asker that goes away while a watch is waited for is let
        // go: nobody waits for it any more. What is done once is done to
        // its end, asker or no asker.)
        let mut gone = String::new();
        let answer = if waits {
            tokio::select! {
                answer = self.answer(&req, &caller) => Some(answer),
                _ = self.wait_retiring() => None,
                _ = async { while lines.read_line(&mut gone).await.unwrap_or(0) > 0 { gone.clear(); } } => None,
            }
        } else {
            Some(self.answer(&req, &caller).await)
        };
        // End mutation accounting (`st.doing` / `st.active_watches`) as soon as
        // the operation itself finishes, BEFORE writing the reply over the client
        // socket, and bound reply delivery so a stalled client reader can NEVER
        // block handover drain!
        drop(active_guard);
        if let Some(answer) = answer {
            let bytes = said(&answer);
            let _ = tokio::time::timeout(Duration::from_secs(10), async {
                let _ = write.write_all(bytes.as_bytes()).await;
                let _ = write.shutdown().await;
            })
            .await;
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
                // between.) Wait on the handover-outcome oneshot (outside
                // `st.doing` accounting) so callers receive a non-zero error
                // if handover aborts on drain timeout or failed save (Neon #1 / Astra #22)!
                let to = req.args.iter().position(|a| a == "--to").map(|i| req.args.get(i + 1).map(PathBuf::from));
                if let Some(to) = to {
                    let Some(to) = to else { return Response::err("retire", "--to requires the helper that takes over (its whole path)", 2) };
                    if let Err(why) = successor_ok(&to) {
                        return Response::err("retire", format!("--to {}: {why}", to.display()), 2);
                    }
                    *self.successor.lock().unwrap_or_else(|e| e.into_inner()) = Some(to);
                }
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.retire_waiters.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
                self.retire.notify_one();
                match tokio::time::timeout(Duration::from_secs(75), rx).await {
                    Ok(Ok(Ok(()))) => Response::ok(String::new(), json!({ "ok": true, "handed_over": true })),
                    Ok(Ok(Err(why))) => Response::err("retire", why, 1),
                    Ok(Err(_)) => Response::ok(String::new(), json!({ "ok": true })),
                    Err(_) => Response::err("retire", "timed out waiting for handover outcome", 1),
                }
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

/// Reaps abandoned temp files from interrupted previous saves/compactions
/// once this service holds the exclusive service lock.
fn reap_stale_temps(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for ent in entries.flatten() {
        let name = ent.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("events.jsonl.tmp.") || s.starts_with("registry.json.tmp.") || s.starts_with("minder-state.json.tmp.") {
            let _ = std::fs::remove_file(ent.path());
        }
    }
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
    watcher_seen: Vec<(String, String, u64)>,
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
            watcher_seen: st.watcher_seen.iter().map(|((w, a), s)| (w.clone(), a.clone(), *s)).collect(),
            rules_gen: st.rules_gen.clone(),
            pending_rules_rev: st.pending_rules_rev.iter().map(|((m, a), (g, p))| (m.clone(), a.clone(), *g, p.clone())).collect(),
            pending_exit_reports: st.pending_exit_reports.iter().map(|(m, s)| (m.clone(), s.iter().cloned().collect())).collect(),
        }
    }

    fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn max_seen_seq(&self) -> u64 {
        self.seen.iter().map(|(_, _, s)| *s).chain(self.minding_seen.iter().map(|(_, _, s)| *s)).chain(self.watcher_seen.iter().map(|(_, _, s)| *s)).max().unwrap_or(0)
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);

        struct TempGuard {
            path: PathBuf,
            armed: bool,
        }
        impl Drop for TempGuard {
            fn drop(&mut self) {
                if self.armed {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }

        let json = serde_json::to_string(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut created = None;
        for _ in 0..64 {
            let tmp = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp) {
                Ok(f) => {
                    created = Some((tmp, f));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        let Some((tmp, mut f)) = created else {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "couldn't allocate unique minder-state temp file"));
        };
        let mut guard = TempGuard { path: tmp.clone(), armed: true };
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        guard.armed = false;
        Ok(())
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

/// Exit code and lock/handover-pipe handles returned from [`serve`] so the
/// caller (`client.rs`) explicitly drops the Tokio runtime BEFORE releasing
/// the exclusive lock and closing the successor's handover pipe.
pub struct ServeExit {
    pub code: i32,
    _lock: Option<std::fs::File>,
    _successor_pipe: Option<std::process::ChildStdin>,
}

impl ServeExit {
    fn exit(code: i32) -> Self {
        Self { code, _lock: None, _successor_pipe: None }
    }
}

/// Waits asynchronously for actual EOF on the inherited handover pipe (stdin)
/// using `AsyncFd` within `wait_dur`. Returns `Err` on timeout or read error
/// without leaving a lingering blocking thread behind.
async fn wait_pipe_eof(wait_dur: Duration) -> Result<(), String> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    // SAFETY: dup stdin with CLOEXEC so we own an independent descriptor and
    // set O_NONBLOCK on the inherited handover pipe.
    let owned: OwnedFd = unsafe {
        let fd = libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if fd < 0 {
            return Err(format!("dup handover stdin failed: {}", std::io::Error::last_os_error()));
        }
        let owned = OwnedFd::from_raw_fd(fd);
        let flags = libc::fcntl(owned.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(owned.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) != 0 {
            return Err(format!("set O_NONBLOCK on handover stdin failed: {}", std::io::Error::last_os_error()));
        }
        owned
    };
    wait_fd_eof(owned, wait_dur).await
}

async fn wait_fd_eof(owned: std::os::fd::OwnedFd, wait_dur: Duration) -> Result<(), String> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: `owned` is an owned descriptor moved into `AsyncFd` and closed only on Drop.
    let async_fd = unsafe { tokio::io::unix::AsyncFd::register_with_interest(owned, tokio::io::Interest::READABLE) }.map_err(|e| format!("register handover pipe with reactor failed: {e}"))?;
    let deadline = tokio::time::Instant::now() + wait_dur;
    let mut buf = [0u8; 64];
    loop {
        let mut ready = match tokio::time::timeout_at(deadline, async_fd.readable()).await {
            Ok(Ok(guard)) => guard,
            Ok(Err(e)) => return Err(format!("handover pipe poll error: {e}")),
            Err(_) => return Err(format!("predecessor did not close handover pipe within {}ms", wait_dur.as_millis())),
        };
        loop {
            // SAFETY: valid owned fd and stack buffer.
            let n = unsafe { libc::read(async_fd.get_ref().as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n == 0 {
                return Ok(());
            }
            if n > 0 {
                continue;
            }
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(e) if e == libc::EAGAIN || e == libc::EWOULDBLOCK => {
                    ready.clear_ready();
                    break;
                }
                _ => return Err(format!("handover pipe read error: {err}")),
            }
        }
    }
}

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
/// take) and its input a pipe this service holds open until after its
/// runtime has dropped: the successor begins ONLY when that pipe reaches EOF.
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
/// (`signalfd`).
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

pub async fn serve(server: PathBuf, paths: Paths, signals: Option<std::os::fd::OwnedFd>) -> ServeExit {
    if let Err(e) = private_dir(&paths.dir).and_then(|()| paths.socket.parent().map_or(Ok(()), private_dir)) {
        logged!("olo agents: {e}");
        return ServeExit::exit(1);
    }
    // One service at a time. Handed its lock by the service before it
    // (`start_successor`): requires actual EOF on the inherited handover pipe
    // before touching registry or socket; exits non-zero on timeout/error.
    let waited = Instant::now();
    let lock = if let Some(lock) = handed_lock(&paths) {
        let wait_dur = if Service::test_env_allowed() {
            std::env::var("OLO_REX_AGENTS_HANDOVER_WAIT_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0).map(Duration::from_millis).unwrap_or(Duration::from_secs(60))
        } else {
            Duration::from_secs(60)
        };
        if let Err(why) = wait_pipe_eof(wait_dur).await {
            logged!("olo agents: handover aborted without starting: {why}");
            return ServeExit::exit(75);
        }
        lock
    } else {
        loop {
            if let Some(lock) = take_lock(&paths) {
                break lock;
            }
            // (It serves: this one has nothing to do. Wait budget exceeds the
            // retiring predecessor's drain + final-save window.)
            if std::os::unix::net::UnixStream::connect(&paths.socket).is_ok() || waited.elapsed() > Duration::from_secs(55) {
                return ServeExit::exit(0);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    // Exclusive service lock is held: reap any abandoned temp files left by
    // a previously killed compaction or save.
    reap_stale_temps(&paths.dir);
    let (rex, mut events) = match Rex::connect(&server).await {
        Ok(connected) => connected,
        Err(e) => {
            logged!("olo agents: the Rex server at {} couldn't be reached: {e}", server.display());
            return ServeExit::exit(69);
        }
    };
    if let Err(e) = rex.call("session.attach_all", json!({ "event_scope": "session" })).await {
        logged!("olo agents: the Rex server wouldn't say what happens in its sessions: {e}");
        return ServeExit::exit(69);
    }
    let mut reg = match SubagentRegistry::read_from(&paths.registry) {
        Ok(reg) => reg,
        Err(why) => {
            logged!("olo agents: {why}");
            return ServeExit::exit(69);
        }
    };
    // Advance monotonic service generation so approval box keys
    // (`b{n}-g{gen}-{inc:08x}-{hash:08x}`) can never collide across service
    // handovers or restarts even if the same child asks an identical prompt first.
    reg.service_gen = reg.service_gen.saturating_add(1);
    let minder_disk = MinderStateDisk::load_from(&paths.dir.join("minder-state.json"));
    let log = load_events(&paths, reg.event_seq.max(minder_disk.max_seen_seq()));
    let _ = std::fs::remove_file(&paths.socket);
    let mut listener = match UnixListener::bind(&paths.socket) {
        Ok(listener) => listener,
        Err(e) => {
            logged!("olo agents: couldn't listen at {}: {e}", paths.socket.display());
            return ServeExit::exit(1);
        }
    };
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
    }
    // Persist the incremented `service_gen` AFTER binding the listening socket
    // (minimizing the socket-down handover window) and BEFORE accepting any
    // client request. Fail startup cleanly if persisting `service_gen` fails
    // so a generation is never served without durable reservation (in addition,
    // `incarnation` embeds a fresh random per-process token in every box key).
    if paths.registry.exists()
        && let Err(e) = reg.save_to(&paths.registry)
    {
        logged!("olo agents: couldn't persist service generation {}: {e}", reg.service_gen);
        drop(listener);
        let _ = std::fs::remove_file(&paths.socket);
        return ServeExit::exit(1);
    }
    let incarnation = new_service_incarnation();
    logged!(
        "olo agents: serving (pid {}, {}, gen {})",
        std::process::id(),
        std::env::current_exe().ok().and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default(),
        reg.service_gen,
    );
    let (frames, _) = broadcast::channel(1024);
    let (retiring_tx, retiring_rx) = tokio::sync::watch::channel(false);
    let watcher_seen = {
        let live_ids: HashSet<&str> = reg.entries.iter().filter(|e| e.exited_at.is_none()).map(|e| e.id.as_str()).collect();
        let mut kept: HashMap<(String, String), u64> = minder_disk
            .watcher_seen
            .into_iter()
            .filter(|(w, a, _)| live_ids.contains(a.as_str()) && watcher_owner_child(w).is_none_or(|cid| live_ids.contains(cid)))
            .map(|(w, a, s)| ((w, a), s))
            .collect();
        while kept.keys().filter(|(w, _)| is_capped_watcher_key(w)).count() > MAX_DETACHED_WATCHER_SEEN {
            if let Some(old) = kept.iter().filter(|(k, _)| is_capped_watcher_key(&k.0)).min_by_key(|(_, v)| **v).map(|(k, _)| k.clone()) {
                kept.remove(&old);
            } else {
                break;
            }
        }
        kept
    };
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
            active_watches: 0,
            retiring: false,
            frozen: false,
            idle_since: None,
            let_go: HashMap::new(),
            closing_minders: HashSet::new(),
            unhealthy_minders: HashSet::new(),
            empty_gen: HashMap::new(),
            reserved_ids: HashSet::new(),
            provisional_ids: HashSet::new(),
            unsaved_recovery_ids: HashSet::new(),
            pending_names: HashSet::new(),
            told: HashMap::new(),
            rules_gen: minder_disk.rules_gen,
            pending_rules_rev: minder_disk.pending_rules_rev.into_iter().map(|(m, a, g, p)| ((m, a), (g, p))).collect(),
            seen: minder_disk.seen.into_iter().map(|(m, a, s)| ((m, a), s)).collect(),
            minding_seen: minder_disk.minding_seen.into_iter().map(|(m, a, s)| ((m, a), s)).collect(),
            watcher_seen,
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
        retiring_tx,
        retiring_rx,
        successor: std::sync::Mutex::new(None),
        said_held: std::sync::atomic::AtomicBool::new(false),
        minder_gate: tokio::sync::Mutex::new(()),
        worker_locks: Mutex::new(HashMap::new()),
        write_lock: Arc::new(tokio::sync::Mutex::new(())),
        incarnation,
        adopt_lock: tokio::sync::Mutex::new(()),
        timers: Mutex::new(Vec::new()),
        leave_locks: Mutex::new(HashMap::new()),
        retire_waiters: Mutex::new(Vec::new()),
    });
    let (stop_writer, writer_stops) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(service.clone().writer(writer_stops));
    if let Err(why) = service.reconcile().await {
        logged!("olo agents: reconcile failed without altering live records: {why}");
        drop(listener);
        let _ = std::fs::remove_file(&paths.socket);
        let _ = stop_writer.send(());
        let _ = writer.await;
        return ServeExit::exit(69);
    }

    let (ended_tx, mut ended) = mpsc::unbounded_channel::<&'static str>();
    let events_task = {
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
        })
    };
    let retire_task = {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            loop {
                service.retire.notified().await;
                let _ = ended.send("handed over");
            }
        })
    };
    let signal_task = {
        let ended = ended_tx.clone();
        tokio::spawn(async move {
            if let Some(fd) = signals.and_then(|fd| unsafe { tokio::io::unix::AsyncFd::register(fd) }.ok()) {
                use std::os::fd::AsRawFd as _;
                loop {
                    let Ok(mut ready) = fd.readable().await else { return };
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
        })
    };
    let idle_task = {
        let (service, ended) = (service.clone(), ended_tx.clone());
        tokio::spawn(async move {
            loop {
                let since = {
                    let mut st = service.lock();
                    let busy = st.clients > 0 || st.reg.entries.iter().any(|e| e.exited_at.is_none());
                    if busy {
                        st.idle_since = None;
                    } else if st.idle_since.is_none() {
                        st.idle_since = Some(Instant::now());
                    }
                    st.idle_since
                };
                match since {
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
        })
    };
    let handover_drain_limit = if Service::test_env_allowed() {
        std::env::var("OLO_REX_AGENTS_TEST_DRAIN_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0).map(Duration::from_millis).unwrap_or(Duration::from_secs(45))
    } else {
        Duration::from_secs(45)
    };
    // Neon #1 / Astra #12: On SIGTERM (`"told to end"`), allow 65 s so an
    // admitted `start` spanning two 30 s Rex RPCs (`new_window` + `attach`)
    // drains to a safe point rather than being abandoned!
    let term_drain_limit = Duration::from_secs(65);
    let why = 'serve: loop {
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
        let drain_limit = if why == "told to end" { term_drain_limit } else { handover_drain_limit };
        // Close admission atomically, drop listener + socket, cancel active
        // watches/followers via level-triggered `retiring_tx` (zero lost wakeup),
        // and drain ALL accepted requests + active watches before freezing.
        {
            let mut st = service.lock();
            st.retiring = true;
        }
        drop(listener);
        let _ = std::fs::remove_file(&paths.socket);
        let _ = service.retiring_tx.send(true);

        let finishing = Instant::now();
        loop {
            let idle_wait = service.idle.notified();
            tokio::pin!(idle_wait);
            idle_wait.as_mut().enable();
            let remaining = {
                let st = service.lock();
                st.doing + st.active_watches
            };
            if remaining == 0 || finishing.elapsed() >= drain_limit {
                break;
            }
            tokio::select! {
                _ = idle_wait => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        // Never abandon accepted mutations across a handover timeout: if
        // non-watch operations (`doing > 0`) are still running after the drain
        // deadline on `"handed over"`, check whether a fatal end reason
        // (`"told to end"` / `"the Rex server went away"`) arrived meanwhile;
        // if not, abort handover, re-bind the socket (while still holding the
        // exclusive `lock`), rearm eligible shared + 1:1 minder timers, report
        // error to explicit `retire` callers, and resume!
        let remaining_doing = service.lock().doing;
        if remaining_doing > 0 && why == "handed over" {
            let mut fatal_why: Option<&'static str> = None;
            while let Ok(queued) = ended.try_recv() {
                if matches!(queued, "told to end" | "the Rex server went away") {
                    fatal_why = Some(queued);
                }
            }
            if let Some(fatal) = fatal_why {
                logged!("olo agents: handover drain timed out with {remaining_doing} operation(s), and fatal `{fatal}` arrived; shutting down");
                *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                service.notify_retire_waiters(Err(format!("handover aborted: {fatal}")));
                break 'serve fatal;
            }
            let _ = std::fs::remove_file(&paths.socket);
            match UnixListener::bind(&paths.socket) {
                Ok(rebound) => {
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
                    }
                    *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    {
                        let mut st = service.lock();
                        st.retiring = false;
                    }
                    let _ = service.retiring_tx.send(false);
                    service.rearm_empty_minders();
                    let abort_msg = format!("handover aborted ({remaining_doing} accepted operation(s) still running after {}s drain); resuming service", drain_limit.as_secs());
                    logged!("olo agents: {abort_msg}");
                    service.notify_retire_waiters(Err(abort_msg));
                    listener = rebound;
                    continue 'serve;
                }
                Err(e) => {
                    logged!("olo agents: handover drain timed out with {remaining_doing} operation(s) running and socket rebind failed ({e}); shutting down without spawning successor");
                    *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    service.notify_retire_waiters(Err(format!("handover aborted and socket rebind failed: {e}")));
                    break 'serve "drain timeout";
                }
            }
        }
        // Neon #1 / Astra #11: For `"handed over"`, run recoverable preflight
        // `write_now()` (with one 100 ms retry) BEFORE setting `st.frozen = true`!
        // Because `st.retiring == true && st.doing == 0`, no client mutations
        // can run, while live background exits/events/watchers continue intact
        // so a failed preflight save + resume drops ZERO events, watchers, or
        // 1:1 minder timers!
        if why == "handed over" {
            let mut preflight = service.write_now().await;
            if preflight.is_err() {
                tokio::time::sleep(Duration::from_millis(100)).await;
                preflight = service.write_now().await;
            }
            let mut fatal_why: Option<&'static str> = None;
            while let Ok(queued) = ended.try_recv() {
                if matches!(queued, "told to end" | "the Rex server went away") {
                    fatal_why = Some(queued);
                }
            }
            if let Some(fatal) = fatal_why {
                logged!("olo agents: handover interrupted by fatal `{fatal}`; shutting down without spawning successor");
                *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                service.notify_retire_waiters(Err(format!("handover aborted: {fatal}")));
                break 'serve fatal;
            }
            if let Err(e) = preflight {
                let _ = std::fs::remove_file(&paths.socket);
                if let Ok(rebound) = UnixListener::bind(&paths.socket) {
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
                    }
                    *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    {
                        let mut st = service.lock();
                        st.retiring = false;
                    }
                    let _ = service.retiring_tx.send(false);
                    service.rearm_empty_minders();
                    let abort_msg = format!("handover preflight save failed ({e}); keeping incumbent service running with in-memory state intact");
                    logged!("olo agents: {abort_msg}");
                    service.notify_retire_waiters(Err(abort_msg));
                    listener = rebound;
                    continue 'serve;
                }
                logged!("olo agents: handover preflight save failed ({e}) and socket rebind failed; shutting down without spawning successor");
                *service.successor.lock().unwrap_or_else(|e| e.into_inner()) = None;
                service.notify_retire_waiters(Err(format!("handover preflight save failed: {e}")));
                break 'serve "final save failed";
            }
        }
        break 'serve why;
    };
    // Authoritative freeze boundary: ONLY reached once handover is committed
    // (preflight succeeded) or on shutdown. Mark `st.frozen = true`, abort Rex
    // event consumer, timers, and every child task, stop & join `writer`, and
    // run ONE authoritative post-freeze `write_now()` before spawning heir!
    {
        let mut st = service.lock();
        st.frozen = true;
        for live in st.live.values_mut() {
            for task in live.tasks.drain(..) {
                task.abort();
            }
        }
    }
    for t in service.timers.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        t.abort();
    }
    events_task.abort();
    retire_task.abort();
    signal_task.abort();
    idle_task.abort();
    tokio::task::yield_now().await;

    let _ = stop_writer.send(());
    let _ = writer.await;
    let final_save = service.write_now().await;
    if let Err(ref e) = final_save {
        logged!("olo agents: final save failed before ending ({why}): {e}");
    }
    // Astra #12: Check whether SIGTERM (`"told to end"`) arrived during the
    // final save before deciding whether to spawn the successor!
    let mut fatal_before_spawn: Option<&'static str> = None;
    while let Ok(queued) = ended.try_recv() {
        if matches!(queued, "told to end" | "the Rex server went away") {
            fatal_before_spawn = Some(queued);
        }
    }
    let effective_why = fatal_before_spawn.unwrap_or(why);
    let can_spawn = effective_why == "handed over" && final_save.is_ok();
    let heir = service.successor.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|_| can_spawn);
    let successor_pipe = heir.and_then(|exe| match start_successor(&exe, &server, &paths, &lock) {
        Ok(held) => {
            logged!("olo agents: its successor is started ({}): it has this service's lock, and serves once this one has ended", exe.display());
            Some(held)
        }
        Err(e) => {
            logged!("olo agents: its successor ({}) couldn't be started ({e}): whoever asks next starts a service", exe.display());
            None
        }
    });
    if can_spawn {
        service.notify_retire_waiters(Ok(()));
    } else {
        service.notify_retire_waiters(Err(format!("handover ended without successor ({effective_why})")));
    }
    // Let any waiting `retire` connection flush its outcome reply before exit.
    tokio::time::sleep(Duration::from_millis(20)).await;
    logged!("olo agents: ending ({effective_why})");
    ServeExit { code: i32::from(final_save.is_err()), _lock: Some(lock), _successor_pipe: successor_pipe }
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

#[cfg(test)]
mod box_tests {
    use super::*;
    use olo_agent_screen::{Agent, parse_approval};

    fn asking(command: &str) -> olo_agent_screen::ApprovalPrompt {
        let rule = "─".repeat(60);
        let dashed = "╌".repeat(60);
        let screen = format!(
            "● Bash({command})\n{rule}\n Bash command\n Make it so\n{dashed}\n │ {command}\n{dashed}\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don’t ask again for: {command}\n   3. No\n Esc to cancel · Tab to amend\n"
        );
        parse_approval(Agent::Claude, &screen).expect("a box")
    }

    #[test]
    fn a_box_is_what_the_latest_reading_says() {
        let (a, b) = (asking("ls"), asking("cargo test"));
        let mut live = Live::new();
        // A reading in its order: its box, numbered; read again, the same.
        let first = live.box_after(Some(&a), false, 1, 0x1111_2222).0.unwrap();
        assert!(first.key.starts_with("b1-g1-11112222-") && !first.answered);
        assert_eq!(live.box_after(Some(&a), false, 1, 0x1111_2222).0, Some(first.clone()));
        // One that only reads (begun before the last taken): its own box where that stands, none for another's, and nothing changed.
        assert_eq!(peek_box(&live, Some(&a)), Some(first.clone()));
        assert_eq!(peek_box(&live, Some(&b)), None);
        assert_eq!(live.box_now.map(|x| x.number), Some(1));
        // Its answer written: kept through a frame with no box on it, and still answered when seen again.
        live.box_now.as_mut().unwrap().answered = true;
        assert_eq!(live.box_after(None, false, 1, 0x1111_2222), (None, false));
        let seen_again = live.box_after(Some(&a), false, 1, 0x1111_2222).0.unwrap();
        assert_eq!((seen_again.key, seen_again.answered), (first.key.clone(), true), "a repaint's empty frame put an answered box away");
        // A12: Same answered request whose choice list grows (2 -> 3 rows) MUST stay attached to the answered box (never minting a fresh box!)
        let mut a_two = a.clone();
        a_two.choices.remove(1);
        let mut grow = Live::new();
        let g1 = grow.box_after(Some(&a_two), false, 1, 0x1111_2222).0.unwrap();
        grow.box_now.as_mut().unwrap().answered = true;
        let g1_three = grow.box_after(Some(&a), false, 1, 0x1111_2222).0.unwrap();
        assert_eq!((g1_three.key, g1_three.answered), (g1.key, true), "A12: 2->3 choice redraw of same submitted request must stay answered");
        assert!(ListSkip::from_box(grow.box_now.unwrap()).matches(&a), "A12: ListSkip must match 2->3 choice redraw of same request");
        // An ordered positive `elsewhere` reading retires the answered box (even if `navigating`!) so a genuine identical successor gets a fresh number.
        live.navigating = true;
        assert_eq!(live.box_after(None, true, 1, 0x1111_2222), (None, true));
        live.navigating = false;
        assert_eq!(live.last_elsewhere_box, 1);
        let successor = live.box_after(Some(&a), false, 1, 0x1111_2222).0.unwrap();
        assert!(successor.key.starts_with("b2-g1-11112222-") && !successor.answered, "identical box after positive elsewhere must get fresh number");
        // A8: Different incarnation token produces a distinct box key EVEN IF service_gen stayed identical (e.g. older helper dropped service_gen)!
        let mut after_handover = Live::new();
        let handover_box = after_handover.box_after(Some(&a), false, 1, 0x3333_4444).0.unwrap();
        assert!(handover_box.key.starts_with("b1-g1-33334444-"));
        assert_ne!(handover_box.key, first.key, "distinct per-process incarnation must never collide even with equal service_gen");
        // Another box in its place is another, unanswered.
        let second = live.box_after(Some(&b), false, 1, 0x1111_2222).0.unwrap();
        assert!(second.key.starts_with("b3-g1-11112222-") && !second.answered);
        // One not answered goes with the first frame that has none (the very same asked again after is another).
        assert_eq!(live.box_after(None, false, 1, 0x1111_2222), (None, false));
        assert!(live.box_now.is_none());
        assert!(live.box_after(Some(&b), false, 1, 0x1111_2222).0.unwrap().key.starts_with("b4-g1-11112222-"));
        // A box kept from the list stays so through frames with none (no wall-clock expiry!), and is let go by another box.
        live.list_skip = Some(ListSkip::from_box(live.box_now.unwrap()));
        live.box_after(None, false, 1, 0x1111_2222);
        live.box_after(Some(&b), false, 1, 0x1111_2222);
        assert!(live.list_skip.is_some(), "kept from the list through an empty frame");
        live.box_after(Some(&a), false, 1, 0x1111_2222);
        assert!(live.list_skip.is_none());
        // …and by the agent seen plainly somewhere else: the same asked after that is another box, and the list's.
        live.list_skip = Some(ListSkip::from_box(live.box_now.unwrap()));
        live.box_after(None, true, 1, 0x1111_2222);
        assert!(live.list_skip.is_none(), "kept from the list though the agent was seen at work since");
        // The first conclusive reading of a child taken over (`adopted_pending`): what stands then is kept from the list.
        let mut taken_over = Live::new();
        taken_over.adopted_pending = true;
        // Blank frame does NOT clear `adopted_pending`.
        taken_over.box_after(None, false, 1, 0x1111_2222);
        assert!(taken_over.adopted_pending, "blank frame must not resolve adoption");
        taken_over.box_after(Some(&a), false, 1, 0x1111_2222);
        assert!(!taken_over.adopted_pending);
        assert!(taken_over.list_skip.is_some_and(|s| s.matches(&a)));
    }

    // ---- Deterministic mock `rex.control.v1` harness for real RPC/concurrency tests ----

    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    type MockHandler = Arc<dyn Fn(usize, &str, &Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, (&'static str, String)>> + Send>> + Send + Sync>;

    struct MockRex {
        dir: PathBuf,
        service: Arc<Service>,
        _task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MockRex {
        fn drop(&mut self) {
            self._task.abort();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn asking_screen(command: &str) -> String {
        let rule = "─".repeat(60);
        let dashed = "╌".repeat(60);
        format!(
            "● Bash({command})\n{rule}\n Bash command\n Make it so\n{dashed}\n │ {command}\n{dashed}\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don’t ask again for: {command}\n   3. No\n Esc to cancel · Tab to amend\n"
        )
    }

    fn claude_survey_screen() -> String {
        let rule = "─".repeat(60);
        format!("● Done.\n{rule}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n{rule}\n❯ \n{rule}\n")
    }

    fn test_entry(id: &str, name: &str) -> NativeSubagent {
        NativeSubagent {
            id: id.to_owned(),
            name: Some(name.to_owned()),
            host_key: HOST.to_owned(),
            session_id: "sess-1".to_owned(),
            window_id: format!("win-{id}"),
            block_id: format!("blk-{id}"),
            parent_window_id: "win-lead".to_owned(),
            parent_block_id: Some("blk-lead".to_owned()),
            cli: olo_rex_link::agents::AgentCli::Claude,
            model: "sonnet".to_owned(),
            effort: "high".to_owned(),
            title: name.to_owned(),
            role: None,
            cwd: Some("/var/tmp".to_owned()),
            resume_id: None,
            session_confidence: None,
            cli_session_id: None,
            transcript_path: None,
            desktop_policy: olo_rex_link::agents::DesktopPolicy::None,
            stream_mode: false,
            started_at: now_unix_ms() as i64,
            exited_at: None,
            exit_code: None,
            last_pid: None,
            created_rev: 1,
            seen_live: true,
            exited_by_sync: false,
            add_dirs: Vec::new(),
            command: Vec::new(),
            minds: None,
            minded_by: None,
            shared_minder: false,
        }
    }

    async fn spawn_mock_service(tag: &str, handler: MockHandler) -> MockRex {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-mock-svc-{}-{}-{tag}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let sock = dir.join("rex.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = handler.clone();
                let calls = calls.clone();
                tokio::spawn(async move {
                    #[allow(clippy::result_large_err)]
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(
                        stream,
                        |req: &tokio_tungstenite::tungstenite::handshake::server::Request, mut resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                            if let Some(proto) = req.headers().get("Sec-WebSocket-Protocol").cloned() {
                                resp.headers_mut().insert("Sec-WebSocket-Protocol", proto);
                            }
                            Ok(resp)
                        },
                    )
                    .await
                    else {
                        return;
                    };
                    let (mut tx, mut rx) = ws.split();
                    // Handshake: expect `hello`, reply `welcome`.
                    let Some(Ok(Message::Text(_))) = rx.next().await else { return };
                    let welcome = json!({ "type": "welcome", "protocol": "rex.control.v1", "limits": { "max_message_bytes": 1 << 20 } });
                    if tx.send(Message::Text(welcome.to_string().into())).await.is_err() {
                        return;
                    }
                    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
                    let writer = tokio::spawn(async move {
                        while let Some(msg) = out_rx.recv().await {
                            if tx.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    });
                    while let Some(Ok(msg)) = rx.next().await {
                        let Message::Text(text) = msg else { continue };
                        let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
                        if req["type"] != "request" {
                            continue;
                        }
                        let id = req["id"].as_str().unwrap_or_default().to_owned();
                        let method = req["method"].as_str().unwrap_or_default().to_owned();
                        let payload = req["payload"].clone();
                        let idx = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let handler = handler.clone();
                        let out_tx = out_tx.clone();
                        tokio::spawn(async move {
                            let resp = match handler(idx, &method, &payload).await {
                                Ok(val) => json!({ "type": "response", "id": id, "ok": true, "payload": val }),
                                Err((code, message)) => json!({ "type": "response", "id": id, "ok": false, "error": { "code": code, "message": message } }),
                            };
                            let _ = out_tx.send(resp.to_string());
                        });
                    }
                    writer.abort();
                });
            }
        });

        let (rex, _events) = Rex::connect(&sock).await.expect("mock Rex connect");
        let adir = dir.join("agents");
        let paths = Paths { socket: adir.join("sock"), lock: adir.join("lock"), registry: adir.join("registry.json"), events: adir.join("events.jsonl"), log: adir.join("service.log"), dir: adir };
        let (frames, _) = broadcast::channel(64);
        let (retiring_tx, retiring_rx) = tokio::sync::watch::channel(false);
        let service = Arc::new(Service {
            rex,
            paths,
            st: Mutex::new(State {
                reg: SubagentRegistry::default(),
                log: SubagentEventLog::default(),
                live: HashMap::new(),
                sessions: HashMap::new(),
                clients: 0,
                doing: 0,
                active_watches: 0,
                retiring: false,
                frozen: false,
                idle_since: None,
                let_go: HashMap::new(),
                closing_minders: HashSet::new(),
                unhealthy_minders: HashSet::new(),
                empty_gen: HashMap::new(),
                reserved_ids: HashSet::new(),
                provisional_ids: HashSet::new(),
                unsaved_recovery_ids: HashSet::new(),
                pending_names: HashSet::new(),
                told: HashMap::new(),
                rules_gen: HashMap::new(),
                pending_rules_rev: HashMap::new(),
                seen: HashMap::new(),
                minding_seen: HashMap::new(),
                watcher_seen: HashMap::new(),
                pending_exit_reports: HashMap::new(),
                pending_minds: HashMap::new(),
                leave: HashMap::new(),
            }),
            changed: Notify::new(),
            frames,
            text_watchers: AtomicUsize::new(0),
            save: Notify::new(),
            idle: Notify::new(),
            retire: Notify::new(),
            retiring_tx,
            retiring_rx,
            successor: std::sync::Mutex::new(None),
            said_held: std::sync::atomic::AtomicBool::new(false),
            minder_gate: tokio::sync::Mutex::new(()),
            worker_locks: Mutex::new(HashMap::new()),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            incarnation: new_service_incarnation(),
            adopt_lock: tokio::sync::Mutex::new(()),
            timers: Mutex::new(Vec::new()),
            leave_locks: Mutex::new(HashMap::new()),
            retire_waiters: Mutex::new(Vec::new()),
        });
        MockRex { dir, service, _task: task }
    }

    /// Test Finding 1 (hardened in Round 3): Hold a REAL minder `status` RPC
    /// behind a bounded-timeout barrier while a second minder `status` request
    /// mints box B's grant, then release the slow `status` #1 and prove:
    /// - slow `status` #1 returns OLD_BOX_A with `approval.box == null` (does NOT
    ///   relabel OLD_BOX_A with NEW_BOX_B's key), AND
    /// - `st.told` RETAINS box B's newer grant untouched!
    #[tokio::test]
    async fn ordered_look_holds_slow_rpc_across_newer_read() {
        let format_n = Arc::new(AtomicUsize::new(0));
        let slow_entered = Arc::new(Notify::new());
        let release_slow = Arc::new(Notify::new());
        let (fn_c, se_c, rs_c) = (format_n.clone(), slow_entered.clone(), release_slow.clone());
        let mock = spawn_mock_service(
            "ordered-look",
            Arc::new(move |_idx, method, _payload| {
                let fn_c = fn_c.clone();
                let se_c = se_c.clone();
                let rs_c = rs_c.clone();
                let method = method.to_owned();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        let n = fn_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 0 {
                            se_c.notify_one();
                            let _ = tokio::time::timeout(Duration::from_secs(5), rs_c.notified()).await;
                            return Ok(json!({ "content": asking_screen("echo OLD_BOX_A") }));
                        }
                        return Ok(json!({ "content": asking_screen("echo NEW_BOX_B") }));
                    }
                    Ok(json!({}))
                })
            }),
        )
        .await;

        let mut minder = test_entry("sa-m-ord", "minder-ord");
        minder.shared_minder = true;
        let mut entry = test_entry("sa-ord", "worker");
        entry.minded_by = Some(minder.id.clone());
        {
            let mut st = mock.service.lock();
            st.reg.register(minder.clone());
            st.reg.register(entry.clone());
            st.live.insert(minder.id.clone(), Live::new());
            st.live.insert(entry.id.clone(), Live::new());
        }
        let make_minder_caller = || crate::agentd::ops::Caller {
            session: Some("sess-1".into()),
            window: Some(minder.window_id.clone()),
            window_from_block: true,
            block: Some(minder.block_id.clone()),
            child: Some(minder.clone()),
        };

        let svc1 = mock.service.clone();
        let mc1 = make_minder_caller();
        let slow_wait = slow_entered.notified();
        tokio::pin!(slow_wait);
        slow_wait.as_mut().enable();
        // Start REAL minder `status` #1 (which enters RPC #0 and blocks on the barrier).
        let slow_task = tokio::spawn(async move { svc1.agent_op(&Request { op: "status".into(), args: vec!["worker".into(), "--json".into()], ..Default::default() }, &mc1).await });
        tokio::time::timeout(Duration::from_secs(5), slow_wait).await.expect("slow status #1 must enter format RPC");

        // Bump `out_seq` so minder `status` #2 executes a live RPC #1 and mints box B's grant!
        mock.service.lock().live.get_mut(&entry.id).unwrap().out_seq += 1;
        let fast_resp = mock.service.agent_op(&Request { op: "status".into(), args: vec!["worker".into(), "--json".into()], ..Default::default() }, &make_minder_caller()).await;
        assert!(fast_resp.ok, "{fast_resp:?}");
        let fast_box_key = fast_resp.data["approval"]["box"].as_str().expect("newer box B key").to_owned();
        assert!(fast_box_key.starts_with("b1-"), "fast box key: {fast_box_key}");
        assert!(fast_resp.data["approval"]["command"].as_str().unwrap().contains("NEW_BOX_B"));
        let pair = (minder.id.clone(), entry.id.clone());
        assert_eq!(mock.service.lock().told.get(&pair).map(|g| g.token.as_str()), Some(fast_box_key.as_str()), "minder status #2 must mint box B's grant");

        // Release older `status` #1 -> returns OLD_BOX_A with `box: null` (NOT box B's key!)
        // and MUST NOT overwrite or clear box B's newer grant in `st.told`!
        release_slow.notify_one();
        let slow_resp = tokio::time::timeout(Duration::from_secs(5), slow_task).await.expect("slow status #1 must finish").unwrap();
        assert!(slow_resp.ok, "{slow_resp:?}");
        assert!(slow_resp.data["approval"]["command"].as_str().unwrap().contains("OLD_BOX_A"));
        assert!(slow_resp.data["approval"]["box"].is_null(), "stale OLD_BOX_A status must NOT inherit NEW_BOX_B's key: {slow_resp:?}");
        assert_eq!(mock.service.lock().live[&entry.id].box_now.map(|b| b.key()), Some(fast_box_key.clone()));
        assert_eq!(mock.service.lock().told.get(&pair).map(|g| g.token.clone()), Some(fast_box_key), "stale status #1 must NEVER overwrite or revoke box B's newer grant");
    }

    /// Findings 5 & 9 + concurrent `send` through survey:
    /// (a) Cancelled pre-submission `Claimed` reader releases slot without sending `0`.
    /// (b) Lost write ack / timeout after `Submitted` NEVER double-presses `0`.
    /// (c) Partial survey fragment NEVER clears `Submitted`/`Stays`.
    /// (d) Concurrent `send --key 2` refuses while survey is staying/pending (zero `"2"` written!).
    #[tokio::test]
    async fn survey_state_machine_cancel_lost_ack_partial_and_concurrent_send() {
        let screen = Arc::new(Mutex::new(claude_survey_screen()));
        let writes = Arc::new(Mutex::new(Vec::<String>::new()));
        let fail_write = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hold_verify = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let verify_entered = Arc::new(Notify::new());
        let format_count = Arc::new(AtomicUsize::new(0));

        let (sc_c, wr_c, fw_c, hv_c, ve_c, fc_c) = (screen.clone(), writes.clone(), fail_write.clone(), hold_verify.clone(), verify_entered.clone(), format_count.clone());
        let mock = spawn_mock_service(
            "survey-sm",
            Arc::new(move |_idx, method, payload| {
                let (sc_c, wr_c, fw_c, hv_c, ve_c, fc_c) = (sc_c.clone(), wr_c.clone(), fw_c.clone(), hv_c.clone(), ve_c.clone(), fc_c.clone());
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        let n = fc_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if n == 1 && hv_c.load(std::sync::atomic::Ordering::SeqCst) {
                            ve_c.notify_one();
                            // Hold verify read so caller cancels the task mid-Claimed!
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                        return Ok(json!({ "content": sc_c.lock().unwrap().clone() }));
                    }
                    if method == "com.superlogical.terminal.write" {
                        let data = payload["args"]["data"].as_str().unwrap_or_default().to_owned();
                        wr_c.lock().unwrap().push(data);
                        if fw_c.load(std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "simulated lost write ack".to_owned()));
                        }
                        return Ok(json!({}));
                    }
                    Ok(json!({}))
                })
            }),
        )
        .await;

        let entry = test_entry("sa-surv", "builder");
        {
            let mut st = mock.service.lock();
            st.reg.register(entry.clone());
            st.live.insert(entry.id.clone(), Live::new());
        }

        // (a) Abort reader while `Claimed` before `rex.write()` -> slot returns to `Idle`, zero writes!
        let ve_wait = verify_entered.notified();
        tokio::pin!(ve_wait);
        ve_wait.as_mut().enable();
        let svc1 = mock.service.clone();
        let ent1 = entry.clone();
        let aborted_reader = tokio::spawn(async move { svc1.look(&ent1, true).await });
        tokio::time::timeout(Duration::from_secs(5), ve_wait).await.expect("aborted reader must enter verify RPC");
        assert!(matches!(mock.service.lock().live[&entry.id].survey, SurveyState::Claimed { .. }));
        aborted_reader.abort();
        let _ = aborted_reader.await;
        assert_eq!(mock.service.lock().live[&entry.id].survey, SurveyState::Idle, "cancelled Claimed reader must release slot");
        assert!(writes.lock().unwrap().is_empty(), "aborted pre-write Claimed reader must send ZERO bytes");

        // (b) Lost write ack after transitioning to `Submitted` -> stays `Submitted`, second reader sends ZERO extra `0`!
        hold_verify.store(false, std::sync::atomic::Ordering::SeqCst);
        fail_write.store(true, std::sync::atomic::Ordering::SeqCst);
        let first_err = mock.service.look(&entry, true).await;
        assert!(first_err.rpc_failed);
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"], "first reader dispatched exactly one 0");
        assert!(matches!(mock.service.lock().live[&entry.id].survey, SurveyState::Submitted { .. }));

        // Second reader sees survey still up while `Submitted` -> MUST NOT send a second `0`!
        fail_write.store(false, std::sync::atomic::Ordering::SeqCst);
        let second = mock.service.look(&entry, true).await;
        assert!(second.survey);
        assert!(second.survey_pending);
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"], "second reader must NEVER press 0 again after lost write ack");

        // (c) Partial survey fragment (question without choices) NEVER clears `Submitted`!
        *screen.lock().unwrap() = "● How is Claude doing this session? (optional)\n".to_owned();
        let partial = mock.service.look(&entry, true).await;
        assert!(partial.survey, "partial survey fragment must preserve survey == true");
        assert!(matches!(mock.service.lock().live[&entry.id].survey, SurveyState::Submitted { .. }), "partial fragment must not retire Submitted");

        // (d) Concurrent `send --key 2` while survey stays refuses without sending `"2"`!
        *screen.lock().unwrap() = claude_survey_screen();
        mock.service.lock().live.get_mut(&entry.id).unwrap().survey = SurveyState::Stays;
        let caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: true, block: Some("blk-lead".into()), child: None };
        let resp = mock.service.agent_op(&Request { op: "send".into(), args: vec!["builder".into(), "--key".into(), "2".into()], ..Default::default() }, &caller).await;
        assert!(!resp.ok, "send --key 2 must be refused while survey is on screen: {resp:?}");
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"], "send --key 2 must NEVER write '2' into survey");
    }

    /// Finding 10/11: transient RPC failure with `pending_handoff == true` must NOT
    /// treat `Working("starting")` as cleared; it keeps `pending_handoff` and emits an
    /// escalation if retries exhaust.
    #[tokio::test]
    async fn failed_reread_keeps_pending_handoff_and_escalates_on_exhaustion() {
        let mock = spawn_mock_service(
            "failed-handoff",
            Arc::new(move |_idx, method, _payload| {
                let method = method.to_owned();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        return Err(("timeout", "transient screen RPC failure".to_owned()));
                    }
                    Ok(json!({}))
                })
            }),
        )
        .await;

        let entry = test_entry("sa-fail", "flaky");
        {
            let mut st = mock.service.lock();
            st.reg.register(entry.clone());
            let mut live = Live::new();
            live.pending_handoff = true;
            // Drive right to retry exhaustion (5 prior failures) to verify escalation immediately.
            live.handoff_failures = 5;
            st.live.insert(entry.id.clone(), live);
        }

        mock.service.transition(&entry.id).await;
        let st = mock.service.lock();
        assert!(!st.live[&entry.id].pending_handoff, "pending_handoff clears once escalation is emitted");
        assert!(st.live[&entry.id].last_state.is_none(), "failed RPC must never set last_state = Working");
        let esc = st.log.find_since(0, |e| e.event == olo_rex_link::agents::minder::ESCALATION);
        assert!(esc.is_some(), "exhausted handoff retries must emit an escalation event");
    }

    /// Finding 14: Stationary screen causes ZERO polling RPCs during `approve`
    /// unmoved navigation verify wait (1.5 s) and survey dismissal wait (900 ms).
    #[tokio::test]
    async fn stationary_screen_sends_zero_polling_rpcs() {
        let format_calls = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(Mutex::new(Vec::<String>::new()));
        let screen = Arc::new(Mutex::new(asking_screen("cargo test")));
        let (fc_c, wr_c, sc_c) = (format_calls.clone(), writes.clone(), screen.clone());
        let mock = spawn_mock_service(
            "zero-poll",
            Arc::new(move |_idx, method, payload| {
                let (fc_c, wr_c, sc_c) = (fc_c.clone(), wr_c.clone(), sc_c.clone());
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        fc_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        return Ok(json!({ "content": sc_c.lock().unwrap().clone() }));
                    }
                    if method == "com.superlogical.terminal.write" {
                        wr_c.lock().unwrap().push(payload["args"]["data"].as_str().unwrap_or_default().to_owned());
                        return Ok(json!({}));
                    }
                    Ok(json!({}))
                })
            }),
        )
        .await;

        let entry = test_entry("sa-still", "still");
        {
            let mut st = mock.service.lock();
            st.reg.register(entry.clone());
            let mut live = Live::new();
            live.stream_connected = true;
            st.live.insert(entry.id.clone(), live);
        }

        // Run `approve --no` on a stationary screen (cursor stays on choice 0, zero PTY output arrives).
        // Must take 1 initial `look` + 1 between-arrows foreground recheck (Astra #16), write
        // `\x1b[B` x 2, do ZERO polling RPCs during the 1.5 s wait, and abort cleanly WITHOUT Escape!
        let caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: true, block: Some("blk-lead".into()), child: None };
        let resp = mock.service.approve(&json!({ "args": ["still", "--no", "--json"] }), &caller).await;
        assert!(!resp.ok);
        assert!(resp.stderr.contains("without pressing Enter or Escape"), "err: {}", resp.stderr);
        assert_eq!(format_calls.load(std::sync::atomic::Ordering::SeqCst), 2, "stationary 1.5s verify wait must send ZERO polling screen RPCs (1 initial + 1 between arrows)");
        assert_eq!(writes.lock().unwrap().as_slice(), ["\x1b[B", "\x1b[B"], "must send ZERO Escape and ZERO Enter on unmoved timeout");

        // Now test stationary survey dismissal wait (900 ms): 1 initial read + 1 verify read + 1 final check at expiry (3 total, NOT 6+ polls).
        format_calls.store(0, std::sync::atomic::Ordering::SeqCst);
        writes.lock().unwrap().clear();
        *screen.lock().unwrap() = claude_survey_screen();
        let looked = mock.service.look(&entry, true).await;
        assert!(looked.survey);
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"]);
        assert!(format_calls.load(std::sync::atomic::Ordering::SeqCst) <= 3, "stationary survey wait made too many RPCs: {}", format_calls.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Addendum Items 2 & 3:
    /// (a) `compact_events_if_needed` preserves all events including those present in memory,
    ///     leaves zero temp files behind, and `reap_stale_temps` removes abandoned temp files.
    /// (b) `begin_attached` propagates `save_to` failure (read-only dir), rolls back `reg` &
    ///     `live`, aborts tasks, and closes the created Rex tab.
    #[tokio::test]
    async fn compaction_reap_and_begin_attached_save_failure_rollback() {
        let closed_windows = Arc::new(Mutex::new(Vec::<String>::new()));
        let cw_c = closed_windows.clone();
        let mock = spawn_mock_service(
            "persist",
            Arc::new(move |_idx, method, payload| {
                let cw_c = cw_c.clone();
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "session.close_window"
                        && let Some(w) = payload["window_id"].as_str()
                    {
                        cw_c.lock().unwrap().push(w.to_owned());
                    }
                    Ok(json!({ "session_id": "sess-1", "windows": [], "blocks": [] }))
                })
            }),
        )
        .await;

        // (a) Seed stale temp files + oversized events file and verify compaction + reaping.
        let stale_ev = mock.service.paths.dir.join("events.jsonl.tmp.99999.1");
        let stale_reg = mock.service.paths.dir.join("registry.json.tmp.99999.1");
        let stale_ms = mock.service.paths.dir.join("minder-state.json.tmp.99999.1");
        std::fs::write(&stale_ev, b"abandoned").unwrap();
        std::fs::write(&stale_reg, b"abandoned").unwrap();
        std::fs::write(&stale_ms, b"abandoned").unwrap();
        reap_stale_temps(&mock.service.paths.dir);
        assert!(!stale_ev.exists() && !stale_reg.exists() && !stale_ms.exists(), "reap_stale_temps must remove abandoned tmp files");

        let entry = test_entry("sa-ev", "ev");
        let mut ev1 = mock.service.event_of(&entry, SubagentLiveState::Idle, Some("first".into()), "k1".into());
        ev1.event = "idle".into();
        let s1 = mock.service.record(ev1);
        // Append `ev2` MID-COMPACTION (after initial snapshot + `sync_all()`, before catch-up + rename!)
        // so removing the catch-up loop would deterministically lose `s2`!
        let mut s2 = 0u64;
        mock.service
            .compact_events_with_hook(1, || {
                let mut ev2 = mock.service.event_of(&entry, SubagentLiveState::Done, Some("second".into()), "k2".into());
                ev2.event = "done".into();
                s2 = mock.service.record(ev2);
            })
            .expect("compaction succeeds");
        assert!(s2 > s1);
        let after = std::fs::read_to_string(&mock.service.paths.events).unwrap();
        let seqs: Vec<u64> = after.lines().filter_map(|l| serde_json::from_str::<SubagentEvent>(l).ok()).map(|e| e.seq).collect();
        assert_eq!(seqs, vec![s1, s2], "compacted events.jsonl must preserve mid-compaction appended event s2");
        let leftover_tmps: Vec<_> = std::fs::read_dir(&mock.service.paths.dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".tmp.")).collect();
        assert!(leftover_tmps.is_empty(), "no temp files left after compaction: {leftover_tmps:?}");

        // (b) Deterministic filesystem error (privilege-independent — works even as root!):
        //     place a directory at `registry.json` (the commit rename target, AFTER `minder-state.json`
        //     succeeds) AND subscribe a follower to verify ZERO leaked frames, ZERO disk entries,
        //     full memory rollback, and tab close!
        let mut follower_rx = mock.service.frames.subscribe();
        std::fs::create_dir(&mock.service.paths.registry).unwrap();
        let res = mock.service.begin_attached(entry.clone(), None, None, None, false, None, None).await;
        std::fs::remove_dir(&mock.service.paths.registry).unwrap();
        assert!(res.is_err(), "begin_attached must fail when registry rename fails");
        assert!(mock.service.lock().reg.entries.is_empty(), "failed begin_attached must roll back registry entry");
        assert!(mock.service.lock().live.is_empty(), "failed begin_attached must roll back live state");
        assert_eq!(closed_windows.lock().unwrap().as_slice(), [entry.window_id.as_str()], "failed begin_attached must close created tab");
        assert!(follower_rx.try_recv().is_err(), "rolled-back child must NEVER broadcast Frame::Agent to followers");

        // (c) Failure at the first save (`minder-state.json` is a directory) ALSO leaves `registry.json` untouched!
        closed_windows.lock().unwrap().clear();
        let ms_path = mock.service.paths.dir.join("minder-state.json");
        let _ = std::fs::remove_file(&ms_path);
        std::fs::create_dir(&ms_path).unwrap();
        let res2 = mock.service.begin_attached(entry.clone(), None, None, None, false, None, None).await;
        std::fs::remove_dir(&ms_path).unwrap();
        assert!(res2.is_err(), "begin_attached must fail when minder-state save fails");
        assert!(!mock.service.paths.registry.exists(), "failed minder-state save must NEVER publish child into registry.json");
        assert!(mock.service.lock().reg.entries.is_empty());
        assert_eq!(closed_windows.lock().unwrap().as_slice(), [entry.window_id.as_str()]);
        assert!(follower_rx.try_recv().is_err(), "rolled-back child must NEVER broadcast Frame::Agent to followers");
    }

    /// Addendum Item 1: `wait_fd_eof` on an inherited handover pipe whose write end stays open
    /// must time out with `Err` rather than proceeding while the predecessor is still running.
    #[tokio::test]
    async fn wait_pipe_eof_requires_actual_eof_and_errors_on_timeout() {
        use std::os::fd::FromRawFd as _;
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        let read_end = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) };
        let write_end = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) };
        // With write_end still open, must time out with Err!
        let err = wait_fd_eof(read_end, Duration::from_millis(120)).await;
        assert!(err.is_err(), "open handover pipe must fail on timeout, not proceed: {err:?}");
        drop(write_end);
        // With write_end closed, must succeed immediately with Ok(())!
        let mut fds2 = [0i32; 2];
        assert_eq!(unsafe { libc::pipe2(fds2.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        let read_end2 = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds2[0]) };
        let write_end2 = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds2[1]) };
        drop(write_end2);
        wait_fd_eof(read_end2, Duration::from_secs(2)).await.expect("closed handover pipe must return Ok(()) on EOF");
    }

    /// Round 2 regression suite covering Astra 1, 4, 6, 13, 15, 16, 17, 18, 19, 21, 26 + T1
    /// and Agy 1, 2, 3, 4, 5, 7, 9, 10, 11, 12 + Lead Addendum L1:
    /// - A1: `adopt` returns error (no `"Took over"`) on live/ended persistence failure and persists ended records durably.
    /// - A4 / N5 / A26: `process_cwd_matches_primary` fails closed on RPC/missing-PID/procfs errors; `norm_dir("/")` preserves root.
    /// - A6 / N7: `box_after` retains answered box across equal-count label truncation / `folded_lines` repaint.
    /// - A13 / N2 / N1 / N3: level-triggered `wait_retiring`, retiring minder `watch` returns `handover: true` without appending, `frozen` blocks mutations.
    /// - A15 / N4: failed `begin_attached` disarms guard atomically and never double-decrements `pending_minds`.
    /// - A16: `begin_attached` installs startup leave list BEFORE `watch_child` and cleans it on rollback; `set_leave` resets dedupe.
    /// - A17: failed confirmation on an ordinary first approval retries and records once recovered.
    /// - A18 / N10 / A21 / N9 / L1: concurrent survey blocks `send`, `--key 0` dismisses staying survey over approval, `send --verify` rejects `rpc_failed` and unsubmitted `Working` drafts.
    /// - A19 / N12: partial survey row never clears `Submitted`; short caller deadline never prematurely marks `Stays`.
    /// - N11: disconnected stream (`stream_connected == false`) recovers via 250 ms fallback.
    /// - T1: queued minder `approve` blocked on `answer_lock` cannot spend a future grant minted for box 2 while blocked.
    #[tokio::test]
    async fn round2_adopt_trust_box_pending_minds_freeze_and_handover_regressions() {
        // 1. A26: `norm_dir` and `TrustAtStart::owns_parts` preserve root directory `/`.
        assert_eq!(norm_dir("/"), Some("/".to_owned()));
        assert_eq!(norm_dir("///"), Some("/".to_owned()));
        assert_eq!(norm_dir("/var/tmp/"), Some("/var/tmp".to_owned()));
        let root_trust =
            olo_agent_screen::FolderTrustPrompt { folder: Some("/".into()), is_add_dir: false, selected_index: 1, trust_index: 1, selected_trust: true, marked: true, question: "Trust /?".into() };
        assert!(TrustAtStart::owns_parts(Some("/"), &[], &root_trust));
        let root_add = olo_agent_screen::FolderTrustPrompt { is_add_dir: true, ..root_trust.clone() };
        assert!(TrustAtStart::owns_parts(Some("/var/tmp"), &["/".into()], &root_add));

        // 2. A6 / N7: 3-choice answered box with truncated session-choice label keeps the same answered box (no b2).
        let full_ap = asking("cargo test --all");
        let mut trunc_ap = full_ap.clone();
        trunc_ap.choices[1].label = "Yes, and don't ask".into();
        let mut live_box = Live::new();
        let b1 = live_box.box_after(Some(&full_ap), false, 1, 0xaaaa_bbbb).0.unwrap();
        live_box.box_now.as_mut().unwrap().answered = true;
        let repainted = live_box.box_after(Some(&trunc_ap), false, 1, 0xaaaa_bbbb).0.unwrap();
        assert_eq!(repainted.key, b1.key, "same-count truncated repaint of answered box must keep box #1");
        assert!(repainted.answered, "repainted answered box must remain answered");

        // 3. A1 + A4/N5 + A15/N4 + A16 + N1 + N3 + A13/N2 on mock service.
        let proc_mode = Arc::new(AtomicUsize::new(0)); // 0=err, 1=no pid, 2=dead pid, 3=self pid
        let pm_c = proc_mode.clone();
        let mock = spawn_mock_service(
            "r2-core",
            Arc::new(move |_idx, method, payload| {
                let pm_c = pm_c.clone();
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.process" {
                        return match pm_c.load(std::sync::atomic::Ordering::SeqCst) {
                            0 => Err(("timeout", "process RPC failed".into())),
                            1 => Ok(json!({})),
                            2 => Ok(json!({ "foreground": { "pid": 999_999_999 } })),
                            _ => Ok(json!({ "foreground": { "pid": std::process::id() } })),
                        };
                    }
                    if method == "session.view" {
                        return Ok(json!({
                            "session_id": "sess-1",
                            "windows": [
                                { "window_id": "win-sa-live-adopt", "layers": [{ "blocks": [{ "block_id": "blk-sa-live-adopt" }] }] },
                                { "window_id": "win-sa-w1", "layers": [{ "blocks": [{ "block_id": "blk-sa-w1" }] }] }
                            ]
                        }));
                    }
                    let _ = payload;
                    Ok(json!({}))
                })
            }),
        )
        .await;

        // A4 / N5: `process_cwd_matches_primary` fails closed on RPC error, missing PID, and dead PID
        //          (even when cached `live.pid` is set to our own PID!).
        let cwd_entry = test_entry("sa-cwd", "cwd");
        let self_cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        {
            let mut st = mock.service.lock();
            st.reg.register(cwd_entry.clone());
            let mut l = Live::new();
            l.pid = Some(std::process::id() as i32);
            st.live.insert(cwd_entry.id.clone(), l);
        }
        let dl = tokio::time::Instant::now() + Duration::from_secs(2);
        proc_mode.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(!mock.service.process_cwd_matches_primary(&cwd_entry, Some(&self_cwd), dl).await, "RPC error must fail closed");
        proc_mode.store(1, std::sync::atomic::Ordering::SeqCst);
        assert!(!mock.service.process_cwd_matches_primary(&cwd_entry, Some(&self_cwd), dl).await, "missing PID must fail closed");
        proc_mode.store(2, std::sync::atomic::Ordering::SeqCst);
        assert!(!mock.service.process_cwd_matches_primary(&cwd_entry, Some(&self_cwd), dl).await, "unreadable /proc/pid/cwd must fail closed");
        proc_mode.store(3, std::sync::atomic::Ordering::SeqCst);
        assert!(mock.service.process_cwd_matches_primary(&cwd_entry, Some(&self_cwd), dl).await, "matching live /proc/pid/cwd must pass");

        // A1: `adopt` returns error (NO "Took over"!) when persistence fails, and writes durably on success.
        let live_adopt = test_entry("sa-live-adopt", "live-adopt");
        let mut ended_adopt = test_entry("sa-ended-adopt", "ended-adopt");
        ended_adopt.exited_at = Some(now_unix_ms() as i64);
        std::fs::create_dir(&mock.service.paths.registry).unwrap();
        let bad_live = mock.service.adopt(Some(&json!({ "agents": [live_adopt] }))).await;
        assert!(!bad_live.ok && !bad_live.stdout.contains("Took over"), "failed live adopt must not acknowledge: {bad_live:?}");
        let bad_ended = mock.service.adopt(Some(&json!({ "agents": [ended_adopt.clone()] }))).await;
        assert!(!bad_ended.ok && !bad_ended.stdout.contains("Took over"), "failed ended adopt must not acknowledge: {bad_ended:?}");
        assert!(!mock.service.lock().reg.entries.iter().any(|e| e.id == "sa-ended-adopt"), "failed ended adopt must roll back");
        std::fs::remove_dir(&mock.service.paths.registry).unwrap();
        let ok_ended = mock.service.adopt(Some(&json!({ "agents": [ended_adopt] }))).await;
        assert!(ok_ended.ok && ok_ended.stdout.contains("Took over 1"), "{ok_ended:?}");
        let reg_disk = SubagentRegistry::read_from(&mock.service.paths.registry).unwrap();
        assert!(reg_disk.entries.iter().any(|e| e.id == "sa-ended-adopt"), "ended adopt must be durably written before returning Ok");

        // A15 / N4 + A16: failed `begin_attached` never double-decrements `pending_minds` AND cleans up startup leave list.
        let mut minder = test_entry("sa-m1", "minder-1");
        minder.shared_minder = true;
        {
            let mut st = mock.service.lock();
            st.reg.register(minder.clone());
            st.live.insert(minder.id.clone(), Live::new());
            st.sessions.entry("sess-1".into()).or_default().windows.insert(minder.window_id.clone());
        }
        let mut g1 = crate::agentd::ops::PendingMindGuard::reserve_for_test(mock.service.clone(), &minder.id);
        let _g2 = crate::agentd::ops::PendingMindGuard::reserve_for_test(mock.service.clone(), &minder.id);
        assert_eq!(mock.service.lock().pending_minds.get(&minder.id).copied(), Some(2));
        let mut worker1 = test_entry("sa-w1", "w1");
        worker1.minded_by = Some(minder.id.clone());
        let _ = std::fs::remove_file(&mock.service.paths.registry);
        std::fs::create_dir(&mock.service.paths.registry).unwrap();
        let err = mock.service.begin_attached(worker1.clone(), Some("win-lead"), Some((&minder.id, &mut g1)), None, false, None, Some("\\bgit\\s+push\\b\n")).await;
        std::fs::remove_dir(&mock.service.paths.registry).unwrap();
        assert!(err.is_err());
        drop(g1);
        assert_eq!(mock.service.lock().pending_minds.get(&minder.id).copied(), Some(1), "failed begin_attached + guard drop must NOT double-decrement start #2's reservation");
        assert!(!mock.service.leave_file(&worker1.id).exists(), "failed begin_attached must remove startup leave file");

        // A16: successful `begin_attached` with `leave_text` installs leave list immediately.
        let ok_w = mock.service.begin_attached(worker1.clone(), Some("win-lead"), None, None, false, None, Some("\\bgit\\s+push\\b\n")).await.unwrap();
        assert_eq!(ok_w.2, Some(Ok(1)));
        assert!(mock.service.lock().leave.get(&worker1.id).and_then(|o| o.as_ref()).is_some());

        // N1: level-triggered `wait_retiring` returns immediately even when fired BEFORE awaiting!
        let _ = mock.service.retiring_tx.send(true);
        tokio::time::timeout(Duration::from_millis(100), mock.service.wait_retiring()).await.expect("level-triggered retiring watch must never lose wakeup");

        // N3: retiring minder `watch` with evicted `pending_rules_rev` returns `handover: true` and appends ZERO events!
        let seq_before = mock.service.lock().log.latest_seq();
        {
            let mut st = mock.service.lock();
            st.retiring = true;
            st.pending_rules_rev.insert((minder.id.clone(), worker1.id.clone()), (1, "/var/tmp/rules.md".into()));
        }
        let m_caller = crate::agentd::ops::Caller {
            session: Some("sess-1".into()),
            window: Some(minder.window_id.clone()),
            window_from_block: true,
            block: Some(minder.block_id.clone()),
            child: Some(minder.clone()),
        };
        let w_resp = mock.service.agent_op(&Request { op: "watch".into(), args: vec!["--all".into(), "--json".into()], ..Default::default() }, &m_caller).await;
        assert!(!w_resp.ok && w_resp.code == 75 && w_resp.data["handover"] == true, "retiring minder watch must return handover:true: {w_resp:?}");
        assert_eq!(mock.service.lock().log.latest_seq(), seq_before, "retiring minder watch must not append to event log");

        // A13 / N2: `frozen = true` prevents background `record`, `record_beside`, and `exit` mutations!
        assert!(mock.service.lock().reg.entries.iter().any(|e| e.id == worker1.id && e.exited_at.is_none()), "worker1 must be live before freeze");
        mock.service.lock().frozen = true;
        let mut ev = mock.service.event_of(&worker1, SubagentLiveState::Done, Some("post-freeze".into()), "post".into());
        ev.event = "done".into();
        assert_eq!(mock.service.record(ev.clone()), seq_before);
        assert_eq!(mock.service.record_beside(ev), seq_before);
        mock.service.exit(&worker1.id, Some(0));
        assert!(mock.service.lock().reg.entries.iter().any(|e| e.id == worker1.id && e.exited_at.is_none()), "frozen service must not mutate reg on background exit");
    }

    /// Round 2 regression suite for A17, A18, A19, A21/N9, N10, N11, N12, T1, and Lead Addendum L1.
    #[tokio::test]
    async fn round2_confirm_retry_survey_send_verify_disconnected_and_queued_grant_regressions() {
        let rule = "─".repeat(60);
        let idle_screen = format!("● Done.\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        let working_with_draft = format!("✻ Thinking… (2s)\n{rule}\n❯ Run the full test suite now\n  line 2\n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        let working_cleared = format!("❯ Run the full test suite now\n✻ Thinking… (3s)\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        let screen = Arc::new(Mutex::new(asking_screen("cargo test -p confirm")));
        let fail_format = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writes = Arc::new(Mutex::new(Vec::<String>::new()));
        let on_write_screen = Arc::new(Mutex::new(Option::<String>::None));

        let (sc_c, ff_c, wr_c, ow_c) = (screen.clone(), fail_format.clone(), writes.clone(), on_write_screen.clone());
        let mock = spawn_mock_service(
            "r2-io",
            Arc::new(move |_idx, method, payload| {
                let (sc_c, ff_c, wr_c, ow_c) = (sc_c.clone(), ff_c.clone(), wr_c.clone(), ow_c.clone());
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        if ff_c.load(std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "simulated transient format failure".into()));
                        }
                        return Ok(json!({ "content": sc_c.lock().unwrap().clone() }));
                    }
                    if method == "com.superlogical.terminal.write" {
                        let data = payload["args"]["data"].as_str().unwrap_or_default().to_owned();
                        wr_c.lock().unwrap().push(data);
                        if let Some(next) = ow_c.lock().unwrap().take() {
                            *sc_c.lock().unwrap() = next;
                        }
                        return Ok(json!({}));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;

        let entry = test_entry("sa-r2", "worker");
        let mut minder = test_entry("sa-m2", "minder-2");
        minder.shared_minder = true;
        let mut minded_entry = entry.clone();
        minded_entry.minded_by = Some(minder.id.clone());
        {
            let mut st = mock.service.lock();
            st.reg.register(minder.clone());
            st.reg.register(minded_entry.clone());
            st.live.insert(minder.id.clone(), Live::new());
            let mut l = Live::new();
            l.stream_connected = true;
            st.live.insert(entry.id.clone(), l);
        }
        let lead = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: true, block: Some("blk-lead".into()), child: None };
        let m_caller = crate::agentd::ops::Caller {
            session: Some("sess-1".into()),
            window: Some(minder.window_id.clone()),
            window_from_block: true,
            block: Some(minder.block_id.clone()),
            child: Some(minder.clone()),
        };

        // 1. A17: Failed confirmation RPC on an ordinary FIRST approval (`pending_handoff == false`)
        //    must keep `unconfirmed == true`, clear `confirm_in_flight`, arm retry, and confirm once recovered!
        mock.service.transition(&entry.id).await;
        fail_format.store(true, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(CONFIRM + Duration::from_millis(80)).await;
        {
            let st = mock.service.lock();
            assert!(st.live[&entry.id].unconfirmed, "failed confirmation must keep unconfirmed == true");
            assert!(st.live[&entry.id].confirm_in_flight.is_none(), "failed confirmation must clear confirm_in_flight");
            assert!(st.log.find_since(0, |e| e.state == SubagentLiveState::WaitingApproval).is_none());
        }
        fail_format.store(false, std::sync::atomic::Ordering::SeqCst);
        mock.service.transition(&entry.id).await;
        let rec_ok = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mock.service.lock().log.find_since(0, |e| e.state == SubagentLiveState::WaitingApproval).is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
        assert!(rec_ok.is_ok(), "recovered confirmation must record WaitingApproval");

        // 2. T1 (poll-driven, zero sleep!): Poll the queued minder `approve` future
        //    directly into `answer_lock` contention (proving it captured box 1's grant
        //    and is blocked on `answer_lock`) BEFORE minting box 2's grant via `minder status`!
        let b1 = mock.service.look(&minded_entry, true).await.boxed.unwrap();
        let pair = (minder.id.clone(), entry.id.clone());
        mock.service.lock().told.insert(pair.clone(), ToldGrant { token: b1.key.clone(), is_approval: true, rules_gen: 0, read_seq: 1 });
        let held_lock = mock.service.answer_lock(&entry.id).lock_owned().await;
        let svc_q = mock.service.clone();
        let mc_q = crate::agentd::ops::Caller {
            session: Some("sess-1".into()),
            window: Some(minder.window_id.clone()),
            window_from_block: true,
            block: Some(minder.block_id.clone()),
            child: Some(minder.clone()),
        };
        let mut queued_fut = Box::pin(async move { svc_q.approve(&json!({ "args": ["worker", "--once", "--json"] }), &mc_q).await });
        // Drive `queued_fut` synchronously until it blocks on `held_lock` (zero sleep!).
        {
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            assert!(matches!(queued_fut.as_mut().poll(&mut cx), std::task::Poll::Pending), "queued minder approve must capture b1's grant and park on held answer_lock");
        }
        // Switch screen to box 2 and actually MINT box 2's grant via minder `status` BEFORE releasing `answer_lock`!
        *screen.lock().unwrap() = asking_screen("rm -rf /danger");
        mock.service.lock().live.get_mut(&entry.id).unwrap().out_seq += 1;
        let st_resp = mock.service.agent_op(&Request { op: "status".into(), args: vec!["worker".into(), "--json".into()], ..Default::default() }, &m_caller).await;
        assert!(st_resp.ok);
        let b2_key = st_resp.data["approval"]["box"].as_str().unwrap().to_owned();
        assert_ne!(b2_key, b1.key);
        assert_eq!(mock.service.lock().told.get(&pair).map(|g| g.token.as_str()), Some(b2_key.as_str()), "box 2's grant must be live in st.told before releasing lock");
        writes.lock().unwrap().clear();
        drop(held_lock);
        let q_res = tokio::time::timeout(Duration::from_secs(5), queued_fut).await.expect("queued minder approve must finish once lock drops");
        assert!(!q_res.ok, "queued minder approve bound to box 1 must fail on box 2 even though box 2 has a valid grant: {q_res:?}");
        assert!(writes.lock().unwrap().is_empty(), "queued minder approve must write ZERO keys into box 2: {:?}", writes.lock().unwrap());

        // 3. N10: Non-minder `send worker --key 0` dismisses a staying survey EVEN when an approval sits behind it!
        let survey_over_approval = format!("{}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n", asking_screen("cargo test"));
        *screen.lock().unwrap() = survey_over_approval;
        mock.service.lock().live.get_mut(&entry.id).unwrap().survey = SurveyState::Stays;
        writes.lock().unwrap().clear();
        let dismiss_resp = mock.service.agent_op(&Request { op: "send".into(), args: vec!["worker".into(), "--key".into(), "0".into(), "--json".into()], ..Default::default() }, &lead).await;
        assert!(dismiss_resp.ok, "send --key 0 must dismiss staying survey even over approval: {dismiss_resp:?}");
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"]);

        // 4. A19 & N12: Half-drawn survey row (`"  1: Bad\n"` / `"  1:"`) NEVER clears `Submitted`;
        //    short caller deadline (80 ms < 900 ms) NEVER prematurely marks `Stays`!
        *screen.lock().unwrap() = claude_survey_screen();
        mock.service.lock().live.get_mut(&entry.id).unwrap().survey = SurveyState::Idle;
        writes.lock().unwrap().clear();
        let short_dl = tokio::time::Instant::now() + Duration::from_millis(80);
        let short_look = mock.service.look_until(&minded_entry, true, Some(short_dl)).await;
        assert!(short_look.rpc_failed, "short caller deadline must time out");
        assert!(matches!(mock.service.lock().live[&entry.id].survey, SurveyState::Submitted { .. }), "N12: short caller deadline must NOT prematurely mark Stays");
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"]);
        // Partial non-positive survey line `"  1:"` must NOT retire `Submitted`!
        *screen.lock().unwrap() = "  1:\n".to_owned();
        let _ = mock.service.look(&minded_entry, true).await;
        assert!(matches!(mock.service.lock().live[&entry.id].survey, SurveyState::Submitted { .. }), "A19: half-drawn line must NOT retire Submitted");
        // Full survey redraw after partial line must NOT send a second `0`!
        *screen.lock().unwrap() = claude_survey_screen();
        let _ = mock.service.look(&minded_entry, true).await;
        assert_eq!(writes.lock().unwrap().as_slice(), ["0"], "A19: must never double-dismiss survey");
        // Positive `idle` prompt DOES retire `Submitted`!
        *screen.lock().unwrap() = idle_screen.clone();
        let _ = mock.service.look(&minded_entry, true).await;
        assert_eq!(mock.service.lock().live[&entry.id].survey, SurveyState::Idle);

        // 5. A21 / N9 + L1: `send --enter --verify` MUST NOT report `delivered: true` on `Working`
        //    while the draft is still sitting unsubmitted (`still_typed`), AND retries Enter to deliver!
        *screen.lock().unwrap() = idle_screen.clone();
        writes.lock().unwrap().clear();
        // First write (paste + Enter) lands on `working_with_draft` (still unsubmitted!),
        // then retry Enter clears to `working_cleared`!
        let step_ref = Arc::new(AtomicUsize::new(0));
        let sc2 = screen.clone();
        let wc2 = working_cleared.clone();
        let wd2 = working_with_draft.clone();
        let wr2 = writes.clone();
        let mock_send = spawn_mock_service(
            "r2-send-verify",
            Arc::new(move |_idx, method, payload| {
                let (sc2, wc2, wd2, wr2, step_ref) = (sc2.clone(), wc2.clone(), wd2.clone(), wr2.clone(), step_ref.clone());
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        return Ok(json!({ "content": sc2.lock().unwrap().clone() }));
                    }
                    if method == "com.superlogical.terminal.write" {
                        let data = payload["args"]["data"].as_str().unwrap_or_default().to_owned();
                        wr2.lock().unwrap().push(data.clone());
                        if data == "\r" {
                            let n = step_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            *sc2.lock().unwrap() = if n == 0 { wd2 } else { wc2 };
                        }
                        return Ok(json!({}));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;
        {
            let mut st = mock_send.service.lock();
            st.reg.register(entry.clone());
            let mut l = Live::new();
            l.stream_connected = false; // Also exercises N11 disconnected-stream fallback!
            st.live.insert(entry.id.clone(), l);
        }
        *screen.lock().unwrap() = idle_screen.clone();
        let send_resp = mock_send
            .service
            .agent_op(
                &Request { op: "send".into(), args: vec!["worker".into(), "Run the full test suite now\nline 2".into(), "--enter".into(), "--verify".into(), "--json".into()], ..Default::default() },
                &lead,
            )
            .await;
        assert!(send_resp.ok && send_resp.data["delivered"] == true, "retry Enter should deliver once draft clears: {send_resp:?}");
        let enter_count = writes.lock().unwrap().iter().filter(|w| w.as_str() == "\r").count();
        assert_eq!(enter_count, 2, "L1: Working + still_typed draft must NOT mark delivered on first Enter; must retry Enter once! writes={:?}", writes.lock().unwrap());

        // 6. N11: Disconnected stream (`stream_connected == false`, zero `out_seq` bumps) still completes
        //    `approve --no` navigation + dismissal via the 250 ms fallback!
        let ask_no_moved = asking_screen("cargo clippy").replace(" ❯ 1. Yes\n   2. Yes", "   1. Yes\n   2. Yes").replace("   3. No", " ❯ 3. No");
        let sc_n11 = Arc::new(Mutex::new(asking_screen("cargo clippy")));
        let mock_n11 = spawn_mock_service(
            "r2-n11",
            Arc::new({
                let sc_n11 = sc_n11.clone();
                let ask_no_moved = ask_no_moved.clone();
                let idle_screen = idle_screen.clone();
                move |_idx, method, payload| {
                    let (sc_n11, ask_no_moved, idle_screen) = (sc_n11.clone(), ask_no_moved.clone(), idle_screen.clone());
                    let method = method.to_owned();
                    let payload = payload.clone();
                    Box::pin(async move {
                        if method == "com.superlogical.terminal.format" {
                            return Ok(json!({ "content": sc_n11.lock().unwrap().clone() }));
                        }
                        if method == "com.superlogical.terminal.write" {
                            match payload["args"]["data"].as_str() {
                                Some("\x1b[B") => *sc_n11.lock().unwrap() = ask_no_moved,
                                Some("\r") => *sc_n11.lock().unwrap() = idle_screen,
                                _ => {}
                            }
                            return Ok(json!({}));
                        }
                        Ok(json!({}))
                    })
                }
            }),
        )
        .await;
        {
            let mut st = mock_n11.service.lock();
            st.reg.register(entry.clone());
            let mut l = Live::new();
            l.stream_connected = false;
            st.live.insert(entry.id.clone(), l);
        }
        let n11_resp = mock_n11.service.approve(&json!({ "args": ["worker", "--no", "--json"] }), &lead).await;
        assert!(n11_resp.ok && n11_resp.data["dismissal"] == "gone", "N11: disconnected stream must recover via fallback: {n11_resp:?}");

        // 7. A21 / N9: `send --enter --verify` when verification RPC fails (`rpc_failed = true`)
        //    MUST NEVER mark `delivered = true` via fallback `starting()`!
        let fmt_n = Arc::new(AtomicUsize::new(0));
        let mock_rpcfail = spawn_mock_service(
            "r2-send-rpcfail",
            Arc::new({
                let fmt_n = fmt_n.clone();
                let idle_screen = idle_screen.clone();
                move |_idx, method, _payload| {
                    let fmt_n = fmt_n.clone();
                    let idle_screen = idle_screen.clone();
                    let method = method.to_owned();
                    Box::pin(async move {
                        if method == "com.superlogical.terminal.format" {
                            let n = fmt_n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            if n <= 1 {
                                return Ok(json!({ "content": idle_screen }));
                            }
                            return Err(("timeout", "simulated verify RPC failure".into()));
                        }
                        Ok(json!({ "modes": [] }))
                    })
                }
            }),
        )
        .await;
        {
            let mut st = mock_rpcfail.service.lock();
            st.reg.register(entry.clone());
            let mut l = Live::new();
            l.stream_connected = true;
            st.live.insert(entry.id.clone(), l);
        }
        let fail_verify = mock_rpcfail
            .service
            .agent_op(&Request { op: "send".into(), args: vec!["worker".into(), "Ping verify".into(), "--enter".into(), "--verify".into(), "--json".into()], ..Default::default() }, &lead)
            .await;
        assert!(fail_verify.ok, "{fail_verify:?}");
        assert_eq!(fail_verify.data["delivered"], false, "A21/N9: failed verify RPC must NEVER report delivered=true: {fail_verify:?}");

        // 8. A18: Real concurrent interleaving — hold `bracketed_paste` (`modes`) behind a barrier while
        //    a survey appears and another reader claims it (`Claimed` -> `Stays`); `send` MUST NOT write into the survey!
        let modes_entered = Arc::new(Notify::new());
        let modes_release = Arc::new(Notify::new());
        let sc_a18 = Arc::new(Mutex::new(idle_screen.clone()));
        let wr_a18 = Arc::new(Mutex::new(Vec::<String>::new()));
        let mock_a18 = spawn_mock_service(
            "r2-a18",
            Arc::new({
                let (me, mr, sc, wr) = (modes_entered.clone(), modes_release.clone(), sc_a18.clone(), wr_a18.clone());
                move |_idx, method, payload| {
                    let (me, mr, sc, wr) = (me.clone(), mr.clone(), sc.clone(), wr.clone());
                    let method = method.to_owned();
                    let payload = payload.clone();
                    Box::pin(async move {
                        if method == "com.superlogical.terminal.modes" {
                            me.notify_one();
                            mr.notified().await;
                            return Ok(json!({ "modes": [] }));
                        }
                        if method == "com.superlogical.terminal.format" {
                            return Ok(json!({ "content": sc.lock().unwrap().clone() }));
                        }
                        if method == "com.superlogical.terminal.write" {
                            wr.lock().unwrap().push(payload["args"]["data"].as_str().unwrap_or_default().to_owned());
                            return Ok(json!({}));
                        }
                        Ok(json!({}))
                    })
                }
            }),
        )
        .await;
        {
            let mut st = mock_a18.service.lock();
            st.reg.register(entry.clone());
            let mut l = Live::new();
            l.stream_connected = true;
            st.live.insert(entry.id.clone(), l);
        }
        let me_wait = modes_entered.notified();
        tokio::pin!(me_wait);
        me_wait.as_mut().enable();
        let svc_a18 = mock_a18.service.clone();
        let lead_a18 = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: true, block: Some("blk-lead".into()), child: None };
        let send_task = tokio::spawn(async move {
            svc_a18.agent_op(&Request { op: "send".into(), args: vec!["worker".into(), "2 please continue".into(), "--enter".into(), "--json".into()], ..Default::default() }, &lead_a18).await
        });
        tokio::time::timeout(Duration::from_secs(5), me_wait).await.expect("A18 send must reach modes RPC");
        // Survey appears while `send`'s `modes` RPC is in flight and stays!
        *sc_a18.lock().unwrap() = claude_survey_screen();
        mock_a18.service.lock().live.get_mut(&entry.id).unwrap().survey = SurveyState::Stays;
        modes_release.notify_one();
        let a18_resp = tokio::time::timeout(Duration::from_secs(5), send_task).await.expect("A18 send task must complete").unwrap();
        assert!(!a18_resp.ok, "A18: send must refuse when survey appears during modes RPC: {a18_resp:?}");
        assert!(wr_a18.lock().unwrap().is_empty(), "A18: send must write ZERO bytes into the survey: {:?}", wr_a18.lock().unwrap());
    }

    /// Round 3 regression suite covering A1/A5/A6, A2, A4/N1, A9/A16, A10/N8,
    /// A15, A22/A24/N4, A26, A29, A30/N5.
    #[tokio::test]
    async fn round3_review_regression_suite() {
        let rule = "─".repeat(60);
        let idle_screen = format!("● Done.\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        let working_with_draft = format!("✻ Thinking… (2s)\n{rule}\n❯ Check build\n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        let working_cleared = format!("❯ Check build\n✻ Thinking… (3s)\n{rule}\n❯ \n{rule}\n  ⏸ manual mode on · esc to interrupt\n");
        let screen = Arc::new(Mutex::new(idle_screen.clone()));
        let writes = Arc::new(Mutex::new(Vec::<String>::new()));
        let session_view_err = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let close_err = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let proc_child_only = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let enter_step = Arc::new(AtomicUsize::new(0));
        let switch_to_approval_on_format = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fail_first_format = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let (sc_c, wr_c, sv_c, cl_c, pc_c, es_c, sw_c, ff_c) =
            (screen.clone(), writes.clone(), session_view_err.clone(), close_err.clone(), proc_child_only.clone(), enter_step.clone(), switch_to_approval_on_format.clone(), fail_first_format.clone());
        let wd_c = working_with_draft.clone();
        let wc_c = working_cleared.clone();
        let mock = spawn_mock_service(
            "r3-all",
            Arc::new(move |_idx, method, payload| {
                let (sc_c, wr_c, sv_c, cl_c, pc_c, es_c, sw_c, ff_c, wd_c, wc_c) =
                    (sc_c.clone(), wr_c.clone(), sv_c.clone(), cl_c.clone(), pc_c.clone(), es_c.clone(), sw_c.clone(), ff_c.clone(), wd_c.clone(), wc_c.clone());
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "session.list" {
                        if sv_c.load(std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "transient session.list error".into()));
                        }
                        return Ok(json!({ "sessions": [{ "session_id": "sess-1" }] }));
                    }
                    if method == "session.view" {
                        if sv_c.load(std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "transient session.view error".into()));
                        }
                        return Ok(json!({
                            "session_id": "sess-1",
                            "windows": [
                                { "window_id": "win-sa-rec", "layers": [{ "blocks": [{ "block_id": "blk-sa-rec" }] }] },
                                { "window_id": "win-sa-live", "layers": [{ "blocks": [{ "block_id": "blk-sa-live" }] }] },
                                { "window_id": "win-sa-q3", "layers": [{ "blocks": [{ "block_id": "blk-sa-q3" }] }] }
                            ]
                        }));
                    }
                    if method == "session.close_window" {
                        if cl_c.load(std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "close_window refused".into()));
                        }
                        return Ok(json!({}));
                    }
                    if method == "com.superlogical.terminal.process" {
                        if pc_c.load(std::sync::atomic::Ordering::SeqCst) {
                            // Only outer shell `child.pid`, NO `foreground.pid`!
                            return Ok(json!({ "child": { "pid": std::process::id() } }));
                        }
                        return Ok(json!({ "foreground": { "pid": std::process::id() } }));
                    }
                    if method == "com.superlogical.terminal.format" {
                        if ff_c.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            return Err(("timeout", "first quiet transition RPC failed".into()));
                        }
                        let cur = sc_c.lock().unwrap().clone();
                        if sw_c.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            *sc_c.lock().unwrap() = asking_screen("rm -rf /appeared-during-inspect");
                        }
                        return Ok(json!({ "content": cur }));
                    }
                    if method == "com.superlogical.terminal.write" {
                        let data = payload["args"]["data"].as_str().unwrap_or_default().to_owned();
                        wr_c.lock().unwrap().push(data.clone());
                        if data == "\r" && es_c.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                            let n = es_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            *sc_c.lock().unwrap() = if n == 1 { wd_c } else { wc_c };
                        }
                        return Ok(json!({}));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;

        // 1. A1 / A5 / A6: Malformed adopt rejected (exit 2, no "Took over"); unknown tab presence
        //    on temporary Rex error fails adopt without stamping live child exited; reconcile on
        //    session.list failure returns Err leaving live records intact!
        let bad_null = mock.service.adopt(Some(&Value::Null)).await;
        assert!(!bad_null.ok && bad_null.code == 2 && !bad_null.stdout.contains("Took over"), "A1: null adopt must fail: {bad_null:?}");
        let bad_enum = mock.service.adopt(Some(&json!({ "agents": [{ "id": "sa-x", "cli": "unknown-future-cli" }] }))).await;
        assert!(!bad_enum.ok && bad_enum.code == 2 && !bad_enum.stdout.contains("Took over"), "A1: unknown CLI enum must fail: {bad_enum:?}");

        let live_child = test_entry("sa-live", "live-c");
        session_view_err.store(true, std::sync::atomic::Ordering::SeqCst);
        let unk_adopt = mock.service.adopt(Some(&json!({ "agents": [live_child.clone()] }))).await;
        assert!(!unk_adopt.ok && !unk_adopt.stdout.contains("Took over"), "A6: temporary Rex error must fail adopt without stamping exited: {unk_adopt:?}");
        assert!(mock.service.lock().reg.entries.is_empty());

        // Reconcile on transient session.list error MUST return Err and leave live record untouched!
        mock.service.lock().reg.register(live_child.clone());
        let rec_err = mock.service.reconcile().await;
        assert!(rec_err.is_err(), "A6: reconcile must fail cleanly on session.list error instead of marking live children exited");
        assert!(mock.service.lock().reg.entries.iter().any(|e| e.id == live_child.id && e.exited_at.is_none()));
        session_view_err.store(false, std::sync::atomic::Ordering::SeqCst);

        // A5: Conflicting duplicate ID (different window_id) MUST fail instead of silently skipping!
        let mut conflict = live_child.clone();
        conflict.window_id = "win-different".into();
        let conf_resp = mock.service.adopt(Some(&json!({ "agents": [conflict] }))).await;
        assert!(!conf_resp.ok && !conf_resp.stdout.contains("Took over"), "A5: conflicting duplicate ID must error: {conf_resp:?}");

        // 2. A10: `process_cwd_matches_primary` fails closed when ONLY `child.pid` is returned (no `foreground.pid`).
        let self_cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        let dl = tokio::time::Instant::now() + Duration::from_secs(2);
        proc_child_only.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!mock.service.process_cwd_matches_primary(&live_child, Some(&self_cwd), dl).await, "A10: child.pid without foreground.pid must fail closed");
        proc_child_only.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(mock.service.process_cwd_matches_primary(&live_child, Some(&self_cwd), dl).await, "A10: verified foreground.pid must pass");

        // 3. A9 / Astra #1 / Astra #3 / Neon #2 / A16:
        //    - Unrelated `write_now()` excludes provisional uncommitted entries.
        //    - Competing `write_now()` queued behind `write_now_committing` MUST NOT
        //      erase the committed entry when reopening the final registry (Astra #1)!
        //    - Follower `snapshot()` excludes provisional entries (Astra #3).
        //    - `adopt` rejects unsafe path IDs (`../escape`, `/abs`) with exit 2 and
        //      rejects batch-internal conflicting ended records (Astra #3 / Addl #1).
        //    - Rollback when `session.close_window` fails & tab stays `Present` retains
        //      an unminded recovery record (`minded_by == None`, `parent_window_id == fallback`),
        //      tracked in `unsaved_recovery_ids` so duplicate `adopt` fails while unwritable!
        let rec_entry = test_entry("sa-rec", "rec");
        {
            let mut st = mock.service.lock();
            st.provisional_ids.insert("sa-prov".into());
            st.reg.register(test_entry("sa-prov", "prov"));
        }
        if let Frame::Snapshot { agents, .. } = mock.service.snapshot() {
            assert!(!agents.iter().any(|a| a.entry.id == "sa-prov"), "Astra #3: follower snapshot must exclude provisional entries");
        }
        mock.service.write_now().await.unwrap();
        let disk_reg = SubagentRegistry::read_from(&mock.service.paths.registry).unwrap();
        assert!(!disk_reg.entries.iter().any(|e| e.id == "sa-prov"), "A9: unrelated write_now must exclude provisional uncommitted entries");
        // Astra #1: Queue a competing `write_now()` behind `write_now_committing(&["sa-prov"])`
        // and verify `sa-prov` survives in the reopened final registry on disk!
        let (r_commit, r_compete) = tokio::join!(mock.service.write_now_committing(&["sa-prov"]), mock.service.write_now());
        r_commit.unwrap();
        r_compete.unwrap();
        let disk_after_compete = SubagentRegistry::read_from(&mock.service.paths.registry).unwrap();
        assert!(disk_after_compete.entries.iter().any(|e| e.id == "sa-prov"), "Astra #1: competing write_now must NEVER erase committed sa-prov");
        mock.service.lock().reg.entries.retain(|e| e.id != "sa-prov");

        // Astra Addl #1 & #3: unsafe child IDs and batch-internal conflicts rejected.
        let mut bad_id = test_entry("../escape", "esc");
        bad_id.exited_at = Some(1);
        let bad_resp = mock.service.adopt(Some(&json!({ "agents": [bad_id] }))).await;
        assert_eq!(bad_resp.code, 2, "Astra Addl #1: path-traversal adopt ID must exit 2: {bad_resp:?}");
        let mut b1 = test_entry("sa-bdup", "b1");
        b1.exited_at = Some(1);
        b1.window_id = "win-b1".into();
        let mut b2 = b1.clone();
        b2.window_id = "win-b2".into();
        let bconf = mock.service.adopt(Some(&json!({ "agents": [b1, b2] }))).await;
        assert!(!bconf.ok && !bconf.stdout.contains("Took over"), "Astra #3/Neon #6: batch-internal conflicting ended IDs must fail: {bconf:?}");

        let mut rec_minded = rec_entry.clone();
        rec_minded.minded_by = Some("sa-m-rec".into());
        close_err.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::fs::remove_file(&mock.service.paths.registry);
        std::fs::create_dir(&mock.service.paths.registry).unwrap();
        let rec_res = mock.service.begin_attached(rec_minded.clone(), Some("win-fallback"), None, None, false, None, None).await;
        assert!(rec_res.is_err());
        // While registry is STILL unwritable, a duplicate `adopt` for `sa-rec` MUST fail (Astra #13)!
        let dup_while_unwritable = mock.service.adopt(Some(&json!({ "agents": [rec_entry.clone()] }))).await;
        assert!(!dup_while_unwritable.ok && !dup_while_unwritable.stdout.contains("Took over"), "Astra #13: duplicate adopt must not ack unsaved recovery record: {dup_while_unwritable:?}");
        std::fs::remove_dir(&mock.service.paths.registry).unwrap();
        close_err.store(false, std::sync::atomic::Ordering::SeqCst);
        {
            let st = mock.service.lock();
            let saved_rec = st.reg.entries.iter().find(|e| e.id == rec_entry.id && e.exited_at.is_none()).expect("A16: failed rollback close must retain owned recovery record");
            assert_eq!(saved_rec.minded_by, None, "Neon #2: recovery record must clear minded_by");
            assert_eq!(saved_rec.parent_window_id, "win-fallback", "Neon #2: recovery record must reset parent_window_id to fallback_parent");
            assert!(st.live.get(&rec_entry.id).is_some_and(|l| l.committed), "A16: recovery record must be committed in memory");
        }

        // 4. A2 / Astra #14: Startup leave list failure neutralizes any pre-existing
        //    empty file on disk so even after a restart (`st.leave.clear()`) `leave_of` is None!
        let leave_fail_entry = test_entry("sa-lfail", "lfail");
        let lf = mock.service.leave_file(&leave_fail_entry.id);
        std::fs::create_dir_all(lf.parent().unwrap()).unwrap();
        std::fs::write(&lf, b"").unwrap(); // Pre-existing empty file!
        // Passing an invalid regex causes startup list installation to fail, neutralize disk file, and cache disabled `None`.
        let (_, _, lres) = mock.service.begin_attached(leave_fail_entry.clone(), None, None, None, false, None, Some("[invalid-regex")).await.unwrap();
        assert!(matches!(lres, Some(Err(_))));
        assert!(!lf.exists(), "Astra #14: failed startup list must durably neutralize old empty file on disk");
        assert!(mock.service.leave_of(&leave_fail_entry.id).is_none(), "A2: failed startup list must cache explicit disabled None");
        mock.service.lock().leave.clear();
        assert!(mock.service.leave_of(&leave_fail_entry.id).is_none(), "Astra #14: disabled list must survive restart (cleared cache)");

        // 5. A4 / N1: Minder `send` steering an `Idle` worker when an approval appears during `inspect()`
        //    MUST reject with ZERO keys written!
        let mut minder = test_entry("sa-m3", "minder-3");
        minder.shared_minder = true;
        let mut worker = test_entry("sa-w3", "w3");
        worker.minded_by = Some(minder.id.clone());
        {
            let mut st = mock.service.lock();
            st.reg.register(minder.clone());
            st.reg.register(worker.clone());
            st.live.insert(minder.id.clone(), Live::new());
            let mut l = Live::new();
            l.stream_connected = true;
            st.live.insert(worker.id.clone(), l);
        }
        let m_caller = crate::agentd::ops::Caller {
            session: Some("sess-1".into()),
            window: Some(minder.window_id.clone()),
            window_from_block: true,
            block: Some(minder.block_id.clone()),
            child: Some(minder.clone()),
        };
        let lead = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: true, block: Some("blk-lead".into()), child: None };
        *screen.lock().unwrap() = idle_screen.clone();
        writes.lock().unwrap().clear();
        switch_to_approval_on_format.store(true, std::sync::atomic::Ordering::SeqCst);
        let m_send = mock.service.agent_op(&Request { op: "send".into(), args: vec!["w3".into(), "Keep going".into(), "--enter".into(), "--json".into()], ..Default::default() }, &m_caller).await;
        assert!(!m_send.ok, "A4/N1: minder send must reject when approval appears during inspect: {m_send:?}");
        assert!(writes.lock().unwrap().is_empty(), "A4/N1: minder send must write ZERO bytes into newly appeared approval: {:?}", writes.lock().unwrap());

        // 6. A15: `approve` MUST reject when a staying survey covers the foreground (ZERO keys written!).
        let survey_over_approval = format!("{}\n● How is Claude doing this session? (optional)\n  1: Bad    2: Fine   3: Good   0: Dismiss\n", asking_screen("cargo test"));
        *screen.lock().unwrap() = survey_over_approval;
        mock.service.lock().live.get_mut(&worker.id).unwrap().survey = SurveyState::Stays;
        writes.lock().unwrap().clear();
        let ap_surv = mock.service.approve(&json!({ "args": ["w3", "--once", "--json"] }), &lead).await;
        assert!(!ap_surv.ok, "A15: approve must refuse while staying survey covers foreground: {ap_surv:?}");
        assert!(writes.lock().unwrap().is_empty(), "A15: approve must write ZERO keys into survey overlay: {:?}", writes.lock().unwrap());
        mock.service.lock().live.get_mut(&worker.id).unwrap().survey = SurveyState::Idle;

        // 7. A26: Partial survey row `1:` sitting over an IDLE Claude chat field MUST preserve `Submitted`!
        *screen.lock().unwrap() = claude_survey_screen();
        writes.lock().unwrap().clear();
        let _ = mock.service.look_until(&worker, true, Some(tokio::time::Instant::now() + Duration::from_millis(80))).await;
        assert!(matches!(mock.service.lock().live[&worker.id].survey, SurveyState::Submitted { .. }));
        *screen.lock().unwrap() = format!("  1:\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        let _ = mock.service.look(&worker, true).await;
        assert!(matches!(mock.service.lock().live[&worker.id].survey, SurveyState::Submitted { .. }), "A26: partial `1:` over idle chat box must preserve Submitted");
        *screen.lock().unwrap() = idle_screen.clone();
        let _ = mock.service.look(&worker, true).await;
        assert_eq!(mock.service.lock().live[&worker.id].survey, SurveyState::Idle);

        // 8. N4 / A24:
        //    - Continuous spinner output ticks every 100 ms MUST NOT starve the 800 ms retry Enter in `send --enter --verify`!
        //    - `send --verify` WITHOUT `--enter` MUST send ZERO `\r`!
        *screen.lock().unwrap() = idle_screen.clone();
        writes.lock().unwrap().clear();
        let no_enter = mock.service.agent_op(&Request { op: "send".into(), args: vec!["w3".into(), "just stage this".into(), "--verify".into(), "--json".into()], ..Default::default() }, &lead).await;
        assert!(no_enter.ok, "{no_enter:?}");
        assert!(!writes.lock().unwrap().iter().any(|w| w == "\r"), "A24: send --verify without --enter must NEVER write Return: {:?}", writes.lock().unwrap());

        *screen.lock().unwrap() = idle_screen.clone();
        writes.lock().unwrap().clear();
        enter_step.store(1, std::sync::atomic::Ordering::SeqCst);
        let svc_tick = mock.service.clone();
        let wid_tick = worker.id.clone();
        let tick_task = tokio::spawn(async move {
            for _ in 0..25 {
                tokio::time::sleep(Duration::from_millis(80)).await;
                let notify = {
                    let mut st = svc_tick.lock();
                    if let Some(l) = st.live.get_mut(&wid_tick) {
                        l.out_seq += 1;
                        Some(l.output_notify.clone())
                    } else {
                        None
                    }
                };
                if let Some(n) = notify {
                    n.notify_waiters();
                }
            }
        });
        let n4_resp = mock
            .service
            .agent_op(&Request { op: "send".into(), args: vec!["w3".into(), "Check build".into(), "--enter".into(), "--verify".into(), "--json".into()], ..Default::default() }, &lead)
            .await;
        tick_task.abort();
        enter_step.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(n4_resp.ok && n4_resp.data["delivered"] == true, "N4: continuous spinner output must NOT starve 800ms retry Enter: {n4_resp:?}");
        assert_eq!(writes.lock().unwrap().iter().filter(|w| w.as_str() == "\r").count(), 2, "N4: must fire retry Enter once under continuous output");

        // 9. A30 / N5 / Neon #9 / Astra #18:
        //    - Start REAL `watch_child` on a quiet standing prompt whose FIRST format RPC fails:
        //      verify the scheduled 250 ms retry wakes `watch_child` and records `WaitingApproval`
        //      in `st.log` without any manual second `transition()` call!
        //    - Blinking redraw while `pending_handoff == true` and `confirm_in_flight.is_some()`
        //      does NOT cancel confirmation!
        let quiet = test_entry("sa-q3", "q3");
        {
            let mut st = mock.service.lock();
            st.reg.register(quiet.clone());
            st.live.insert(quiet.id.clone(), Live::new());
        }
        *screen.lock().unwrap() = asking_screen("cargo check");
        fail_first_format.store(true, std::sync::atomic::Ordering::SeqCst);
        mock.service.watch_child(&quiet);
        mock.service.poke(&quiet.id);
        let wait_ok = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mock.service.lock().log.last_recorded_of(&quiet.id).is_some_and(|(s, _)| s == SubagentLiveState::WaitingApproval) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
        assert!(wait_ok.is_ok(), "A30 / Astra #18: real watch_child scheduled retry must recover quiet prompt after first RPC failure");

        let quiet_blink = test_entry("sa-q3b", "q3b");
        {
            let mut st = mock.service.lock();
            st.reg.register(quiet_blink.clone());
            let mut l = Live::new();
            l.pending_handoff = true;
            st.live.insert(quiet_blink.id.clone(), l);
        }
        mock.service.transition(&quiet_blink.id).await;
        let gen_after_first = mock.service.lock().live[&quiet_blink.id].check_gen;
        let cif = mock.service.lock().live[&quiet_blink.id].confirm_in_flight;
        assert_eq!(cif, Some(gen_after_first));
        // Simulate a 150 ms blink calling `transition()` while confirmation is already in flight:
        mock.service.transition(&quiet_blink.id).await;
        assert_eq!(mock.service.lock().live[&quiet_blink.id].check_gen, gen_after_first, "N5: blink during pending_handoff must NOT cancel active confirm_in_flight");
    }

    /// Regression for the `watch --all --since CURSOR` standing-approval race:
    /// when a tab-minder or lead starts multiple children back-to-back (or bumps its
    /// cursor from `status`) and child A enters `waiting_approval` at or before `since`,
    /// `watch --all --since` must immediately catch A's standing approval without
    /// rewinding the cursor, must not spin-loop if A is left waiting, must catch child B
    /// next, and must catch a newer approval box on A later.
    #[tokio::test]
    async fn watch_since_catches_standing_approval_without_spin_loop() {
        let rule = "─".repeat(60);
        let working = format!("✻ Thinking… (2s)\n{rule}\n❯ \n{rule}\n  esc to interrupt\n");
        let screens: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::from([("blk-a".into(), asking_screen("rex whoami --json")), ("blk-b".into(), asking_screen("ls crates/pool"))])));
        let sc = screens.clone();
        let mock = spawn_mock_service(
            "watch-standing",
            Arc::new(move |_idx, method, payload| {
                let sc = sc.clone();
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        let blk = payload["block_id"].as_str().unwrap_or_default();
                        let text = sc.lock().unwrap().get(blk).cloned().unwrap_or_default();
                        return Ok(json!({ "content": text }));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;

        let mut minder = test_entry("sa-tabm", "minder");
        minder.window_id = "win-tabm".into();
        minder.block_id = "blk-tabm".into();
        minder.role = Some("tab-minder".into());

        let mut child_a = test_entry("sa-ca", "child-a");
        child_a.window_id = "win-ca".into();
        child_a.block_id = "blk-a".into();
        child_a.parent_window_id = "win-tabm".into();

        let mut child_b = test_entry("sa-cb", "child-b");
        child_b.window_id = "win-cb".into();
        child_b.block_id = "blk-b".into();
        child_b.parent_window_id = "win-tabm".into();

        {
            let mut st = mock.service.lock();
            st.sessions.insert(
                "sess-1".into(),
                Sess {
                    windows: HashSet::from(["win-lead".into(), "win-tabm".into(), "win-ca".into(), "win-cb".into()]),
                    blocks: HashMap::from([("blk-tabm".into(), "win-tabm".into()), ("blk-a".into(), "win-ca".into()), ("blk-b".into(), "win-cb".into())]),
                    active: Some("win-tabm".into()),
                    fresh: true,
                    attached: true,
                },
            );
            st.reg.register(minder.clone());
            st.reg.register(child_a.clone());
            st.reg.register(child_b.clone());
            st.live.insert(child_a.id.clone(), Live::new());
            st.live.insert(child_b.id.clone(), Live::new());
        }

        // Prime live box keys and record both standing approvals BEFORE the watcher's cursor.
        let la = mock.service.look(&child_a, false).await;
        let lb = mock.service.look(&child_b, false).await;
        let box_a = la.boxed.as_ref().expect("child_a box").key.clone();
        let box_b = lb.boxed.as_ref().expect("child_b box").key.clone();

        let mut ev_a = mock.service.event_of(&child_a, SubagentLiveState::WaitingApproval, Some("rex whoami --json".into()), format!("ap:{box_a}"));
        ev_a.approval = la.cls.approval.as_ref().map(|ap| Service::approval_json_with_box(ap, &[], Some(&box_a)));
        let seq_a = mock.service.record(ev_a);

        let mut ev_b = mock.service.event_of(&child_b, SubagentLiveState::WaitingApproval, Some("ls crates/pool".into()), format!("ap:{box_b}"));
        ev_b.approval = lb.cls.approval.as_ref().map(|ap| Service::approval_json_with_box(ap, &[], Some(&box_b)));
        let seq_b = mock.service.record(ev_b);

        // Advance the log past BOTH approvals (simulating another event or a status cursor bump).
        let other = mock.service.event_of(&minder, SubagentLiveState::Working, Some("watching".into()), "work".into());
        let later_cursor = mock.service.record(other);
        assert!(later_cursor > seq_b && seq_b > seq_a);

        let caller =
            crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-tabm".into()), window_from_block: true, block: Some("blk-tabm".into()), child: Some(minder.clone()) };

        // 1. First watch --all --since later_cursor catches child_a immediately, cursor stays >= later_cursor!
        let r1 = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), later_cursor.to_string(), "--on".into(), "approval,question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r1.ok, "expected standing approval for child_a: {r1:?}");
        assert_eq!(r1.data["id"], "sa-ca");
        assert_eq!(r1.data["event"], "waiting_approval");
        assert_eq!(r1.data["standing"], true);
        assert_eq!(r1.data["cursor"], later_cursor, "returned cursor must never rewind below since");

        // 2. Second watch --all --since later_cursor skips already-delivered child_a and catches child_b!
        let r2 = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), later_cursor.to_string(), "--on".into(), "approval,question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r2.ok, "expected standing approval for child_b: {r2:?}");
        assert_eq!(r2.data["id"], "sa-cb");
        assert_eq!(r2.data["standing"], true);
        assert_eq!(r2.data["cursor"], later_cursor);

        // 3. Third watch with both still waiting on the SAME boxes must NOT spin-loop (times out cleanly)!
        let r3 = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), later_cursor.to_string(), "--on".into(), "approval,question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(!r3.ok && r3.code == 124, "already-seen standing boxes must not spin-loop: {r3:?}");

        // 4. Child A clears box_a, then raises a brand-new box_a2 at seq_a2, and cursor jumps past it:
        //    watch --all --since must catch box_a2 immediately!
        screens.lock().unwrap().insert("blk-a".into(), working.clone());
        let _ = mock.service.look(&child_a, true).await;
        screens.lock().unwrap().insert("blk-a".into(), asking_screen("cargo test -p olo-rex"));
        let la2 = mock.service.look(&child_a, true).await;
        let box_a2 = la2.boxed.as_ref().expect("child_a second box").key.clone();
        assert_ne!(box_a2, box_a);
        let mut ev_a2 = mock.service.event_of(&child_a, SubagentLiveState::WaitingApproval, Some("cargo test -p olo-rex".into()), format!("ap:{box_a2}"));
        ev_a2.approval = la2.cls.approval.as_ref().map(|ap| Service::approval_json_with_box(ap, &[], Some(&box_a2)));
        let _seq_a2 = mock.service.record(ev_a2);
        let jump_cursor = mock.service.record(mock.service.event_of(&minder, SubagentLiveState::Working, Some("bump".into()), "work2".into()));

        let r4 = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), jump_cursor.to_string(), "--on".into(), "approval,question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r4.ok && r4.data["id"] == "sa-ca" && r4.data["standing"] == true && r4.data["cursor"] == jump_cursor, "newer standing box_a2 must be caught: {r4:?}");
    }

    /// Round 1 review regressions (Astra #1, #2, #4, #5, #6, #8, #9, #11, #12 & Neon #1–#7):
    /// handover box-key rotation with real `box:b{n}-{hash}` dedupe keys, busy-sibling
    /// non-starvation, cursorless->cursor transition, answered/working child clearing
    /// `watcher_seen`, `window_from_block == false` + `--watcher` + `--no-standing`,
    /// ring-eviction (`had_gap`) catch-up, `WaitingQuestion` + `of_the_turn_given` turn
    /// filtering, and `Service::exit` pruning of `watcher_seen`.
    #[tokio::test]
    async fn watch_standing_round2_handover_question_starvation_cursorless_and_cleanup() {
        let rule = "─".repeat(60);
        let working = format!("✻ Thinking… (2s)\n{rule}\n❯ \n{rule}\n  esc to interrupt\n");
        let idle = format!("● Done.\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        let screens: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::from([("blk-a".into(), asking_screen("cargo test -p handover...")), ("blk-b".into(), working.clone()), ("blk-q".into(), idle.clone())])));
        let rpc_counts: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let sc = screens.clone();
        let rc = rpc_counts.clone();
        let mock = spawn_mock_service(
            "watch-r2",
            Arc::new(move |_idx, method, payload| {
                let sc = sc.clone();
                let rc = rc.clone();
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        let blk = payload["block_id"].as_str().unwrap_or_default().to_owned();
                        *rc.lock().unwrap().entry(blk.clone()).or_insert(0) += 1;
                        let text = sc.lock().unwrap().get(&blk).cloned().unwrap_or_default();
                        return Ok(json!({ "content": text }));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;

        let mut minder = test_entry("sa-tm", "minder");
        minder.window_id = "win-tm".into();
        minder.block_id = "blk-tm".into();
        minder.role = Some("tab-minder".into());

        // Live Claude transcript with pending Bash tool_use so handover refresh re-inspects the
        // live transcript rather than blindly copying an unverified pre-handover payload (Astra R3 #1).
        let a_tx_path = mock.service.paths.dir.join("a-transcript.jsonl");
        std::fs::write(
            &a_tx_path,
            concat!(
                "{\"type\":\"user\",\"timestamp\":\"2026-11-28T05:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"run tests\"}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-11-28T05:00:01Z\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tu-a\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test -p handover -- --full-transcript-cmd\"}}]}}\n"
            ),
        )
        .unwrap();
        let mut child_a = test_entry("sa-a", "child-a");
        child_a.window_id = "win-a".into();
        child_a.block_id = "blk-a".into();
        child_a.parent_window_id = "win-tm".into();
        child_a.transcript_path = Some(a_tx_path.display().to_string());

        let mut child_b = test_entry("sa-b", "child-b");
        child_b.window_id = "win-b".into();
        child_b.block_id = "blk-b".into();
        child_b.parent_window_id = "win-tm".into();

        // Transcript-only WaitingQuestion child (real Claude user + assistant tool_use with stop_reason, Astra R2 #23).
        let tx_path = mock.service.paths.dir.join("q-transcript.jsonl");
        let old_ts = "2026-11-28T05:00:00Z";
        std::fs::write(
            &tx_path,
            format!(
                concat!(
                    "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"user\",\"content\":\"check port\"}}}}\n",
                    "{{\"type\":\"assistant\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"assistant\",\"stop_reason\":\"tool_use\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"tu1\",\"name\":\"AskUserQuestion\",\"input\":{{\"questions\":[{{\"question\":\"Which port?\"}}]}}}}]}}}}\n"
                ),
                ts = old_ts
            ),
        )
        .unwrap();
        let mut child_q = test_entry("sa-q", "child-q");
        child_q.cli = olo_rex_link::agents::AgentCli::Claude;
        child_q.window_id = "win-q".into();
        child_q.block_id = "blk-q".into();
        child_q.parent_window_id = "win-tm".into();
        child_q.transcript_path = Some(tx_path.display().to_string());

        {
            let mut st = mock.service.lock();
            st.sessions.insert(
                "sess-1".into(),
                Sess {
                    windows: HashSet::from(["win-lead".into(), "win-tm".into(), "win-a".into(), "win-b".into(), "win-q".into()]),
                    blocks: HashMap::from([("blk-tm".into(), "win-tm".into()), ("blk-a".into(), "win-a".into()), ("blk-b".into(), "win-b".into()), ("blk-q".into(), "win-q".into())]),
                    active: Some("win-lead".into()),
                    fresh: true,
                    attached: true,
                },
            );
            st.reg.register(minder.clone());
            st.reg.register(child_a.clone());
            st.reg.register(child_b.clone());
            st.reg.register(child_q.clone());
            st.live.insert(child_a.id.clone(), Live::new());
            st.live.insert(child_b.id.clone(), Live::new());
            st.live.insert(child_q.id.clone(), Live::new());
        }

        // 1. Handover box-key rotation (Astra #1 / Neon #1): record child_a with an OLD-gen
        //    box key (`b1-g0-oldinc-...`) but real generation-independent dedupe key `box:b1-{hash}`.
        let la = mock.service.look(&child_a, false).await;
        let lb_live = la.boxed.as_ref().expect("live box for child_a");
        let content_key = format!("box:b{}-{:08x}", lb_live.number, lb_live.hash as u32);
        let old_box_key = format!("b{}-g0-oldinc00-{:08x}", lb_live.number, lb_live.hash as u32);
        assert_ne!(old_box_key, lb_live.key);
        let mut ev_a = mock.service.event_of(&child_a, SubagentLiveState::WaitingApproval, Some("cargo test -p handover".into()), content_key);
        let mut ap_json = la.cls.approval.as_ref().map(|ap| Service::approval_json_with_box(ap, &[], Some(&old_box_key))).unwrap();
        // Inject a stale pre-handover command to prove cross-generation refresh ignores it and
        // re-inspects the live transcript to recover the real full command (Astra R3 #1):
        ap_json["command"] = json!("cargo test -p handover -- --stale-pre-handover");
        ap_json["command_from"] = json!("transcript");
        ap_json["arguments"] = json!({ "command": "cargo test -p handover -- --stale-pre-handover" });
        ev_a.approval = Some(ap_json);
        let seq_a = mock.service.record(ev_a);
        // Simulate restart where `live.latest_event` is empty so single-pass ring fallback runs too!
        mock.service.lock().live.get_mut(&child_a.id).unwrap().latest_event = None;

        // 2. Sibling B records a queued Done event AFTER `since_floor` (Astra #6 starvation check).
        let bump = mock.service.record(mock.service.event_of(&minder, SubagentLiveState::Working, Some("bump".into()), "b1".into()));
        let mut ev_b_done = mock.service.event_of(&child_b, SubagentLiveState::Done, Some("b done".into()), "done:1".into());
        ev_b_done.event = "done".into();
        let seq_b_done = mock.service.record(ev_b_done);
        assert!(seq_b_done > bump && bump > seq_a);

        let caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-tm".into()), window_from_block: true, block: Some("blk-tm".into()), child: Some(minder.clone()) };

        // `--no-standing` (used by cursorless `--follow`, Astra #12) skips standing child_a and returns B's queued event!
        let follow_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-lead".into()), window_from_block: false, block: None, child: None };
        let r_no_stand = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "--all".into(),
                        "--parent".into(),
                        "win-tm".into(),
                        "--since".into(),
                        bump.to_string(),
                        "--no-standing".into(),
                        "--on".into(),
                        "approval,done".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &follow_caller,
            )
            .await;
        assert!(r_no_stand.ok && r_no_stand.data["id"] == "sa-b", "--no-standing must not emit pre-cursor standing approval: {r_no_stand:?}");
        // Verify unverified `window_from_block == false` did NOT pollute `win:win-lead`!
        assert!(!mock.service.lock().watcher_seen.keys().any(|(w, _)| w == "win:win-lead"));

        // Normal watch from tab-minder MUST catch older standing child_a BEFORE newer queued child_b (Astra #6)
        // AND refresh `approval.box` to the current live generation's box key (Astra #1 / Neon #1)!
        let r_stand = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), bump.to_string(), "--on".into(), "approval,done".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r_stand.ok && r_stand.data["id"] == "sa-a" && r_stand.data["standing"] == true, "older standing approval must beat busy sibling and survive handover key rotation: {r_stand:?}");
        assert_eq!(r_stand.data["cursor"], bump);
        assert_eq!(r_stand.data["approval"]["box"], lb_live.key, "handover standing delivery must refresh approval.box to live box key");
        assert_eq!(r_stand.data["approval"]["command"], "cargo test -p handover -- --full-transcript-cmd", "box refresh must preserve transcript-recovered command");
        assert_eq!(r_stand.data["approval"]["command_from"], "transcript", "box refresh must preserve command_from");
        assert_eq!(r_stand.data["approval"]["arguments"]["command"], "cargo test -p handover -- --full-transcript-cmd", "box refresh must preserve arguments");

        // Next watch from `bump` now delivers sibling B's queued event cleanly!
        let r_sib = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), bump.to_string(), "--on".into(), "approval,done".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r_sib.ok && r_sib.data["id"] == "sa-b" && r_sib.data["cursor"] == seq_b_done, "queued sibling event must remain intact after standing delivery: {r_sib:?}");

        // 3. Cursorless watch -> cursor watch transition (Astra #11 / Neon #3): a fresh watcher
        //    running WITHOUT `--since` receives child_a once and continuing with `--since <cur>`
        //    must NOT duplicate it!
        let w2_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: None, window_from_block: false, block: None, child: None };
        let r_curless = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-a".into(), "--watcher".into(), "det-1".into(), "--on".into(), "approval".into(), "--timeout".into(), "2".into(), "--json".into()],
                    ..Default::default()
                },
                &w2_caller,
            )
            .await;
        assert!(r_curless.ok && r_curless.data["id"] == "sa-a", "cursorless watch must catch standing approval: {r_curless:?}");
        let cur_after = r_curless.data["cursor"].as_u64().unwrap();
        let r_cur_next = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "det-1".into(),
                        "--since".into(),
                        cur_after.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &w2_caller,
            )
            .await;
        assert!(!r_cur_next.ok && r_cur_next.code == 124, "continuing after cursorless watch must not duplicate standing approval: {r_cur_next:?}");

        // 4. Transcript-only WaitingQuestion + `of_the_turn_given` filter (Astra R1 #4 / R2 #23 / Neon R1 #2):
        //    First verify the SAME real Claude transcript fixture IS delivered for a current turn (no newer expects),
        //    then set `live.expects` to a NEWER send turn and verify a fresh watcher ignores & clears it!
        let q_info = mock.service.inspect(std::slice::from_ref(&child_q)).await;
        let q_turn_ms = q_info.get(&child_q.id).and_then(|v| v["turn_started_ms"].as_u64());
        assert!(q_turn_ms.is_some(), "real Claude user+assistant fixture must report turn_started_ms: {q_info:?}");
        assert_eq!(q_info.get(&child_q.id).and_then(|v| v["transcript_state"].as_str()), Some("waiting"), "fixture must parse as waiting: {q_info:?}");
        let q_key = transcript_question_key(q_info.get(&child_q.id), q_turn_ms);
        let mut ev_q = mock.service.event_of(&child_q, SubagentLiveState::WaitingQuestion, Some("Which port?".into()), q_key);
        ev_q.event = "waiting_question".into();
        let seq_q = mock.service.record(ev_q);
        {
            let mut st = mock.service.lock();
            let ev_q_stored = st.log.last_state_of(&child_q.id).cloned();
            st.live.get_mut(&child_q.id).unwrap().latest_event = ev_q_stored;
        }
        let q_bump = mock.service.record(mock.service.event_of(&minder, SubagentLiveState::Working, Some("qb".into()), "qb".into()));
        assert!(q_bump > seq_q);
        let r_q_live = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-q".into(),
                        "--watcher".into(),
                        "det-q-live".into(),
                        "--since".into(),
                        q_bump.to_string(),
                        "--on".into(),
                        "question".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &w2_caller,
            )
            .await;
        assert!(r_q_live.ok && r_q_live.data["id"] == "sa-q" && r_q_live.data["standing"] == true, "current-turn transcript question must be delivered: {r_q_live:?}");
        {
            let mut st = mock.service.lock();
            let lq = st.live.get_mut(&child_q.id).unwrap();
            let newer = q_turn_ms.unwrap() + 60_000;
            lq.expects = Some(olo_rex_link::agents::state::Expected { cursor: seq_q, given_ms: newer, until_ms: newer + 60_000, transcript_len: Some(0), turn_from_ms: newer, seq: 1 });
        }
        let r_q_stale = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-q".into(), "--since".into(), q_bump.to_string(), "--on".into(), "question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(!r_q_stale.ok && r_q_stale.code == 124, "stale pre-send transcript question must not fire after newer expects: {r_q_stale:?}");
        assert_eq!(
            mock.service.lock().watcher_seen.get(&("child:sa-tm".to_owned(), "sa-q".to_owned())).copied(),
            Some(seq_q),
            "positively cleared pre-send question must advance watcher_seen to avoid repeat inspect()"
        );

        // 5. Answered/Working child advances `watcher_seen` on first check so subsequent wakes do 0 RPCs (Neon #2):
        screens.lock().unwrap().insert("blk-a".into(), working.clone());
        let r_cleared = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "det-2".into(),
                        "--since".into(),
                        q_bump.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &w2_caller,
            )
            .await;
        assert!(!r_cleared.ok && r_cleared.code == 124);
        assert_eq!(mock.service.lock().watcher_seen.get(&("watcher:det-2".to_owned(), "sa-a".to_owned())).copied(), Some(seq_a));
        let rpcs_before = rpc_counts.lock().unwrap().get("blk-a").copied().unwrap_or(0);
        let _ = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "det-2".into(),
                        "--since".into(),
                        q_bump.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &w2_caller,
            )
            .await;
        assert_eq!(rpc_counts.lock().unwrap().get("blk-a").copied().unwrap_or(0), rpcs_before, "already-cleared working child must trigger zero follow-up RPCs");

        // 6. Ring eviction (`had_gap`) with unread live approval newer than `since` (Astra #2):
        screens.lock().unwrap().insert("blk-a".into(), asking_screen("cargo check --evict"));
        let la_ev = mock.service.look(&child_a, true).await;
        let box_ev = la_ev.boxed.as_ref().expect("evict box");
        let mut ev_evict = mock.service.event_of(&child_a, SubagentLiveState::WaitingApproval, Some("cargo check --evict".into()), format!("box:b{}-{:08x}", box_ev.number, box_ev.hash as u32));
        ev_evict.approval = la_ev.cls.approval.as_ref().map(|ap| Service::approval_json_with_box(ap, &[], Some(&box_ev.key)));
        let before_evict = mock.service.cursor();
        let seq_evict = mock.service.record(ev_evict);
        {
            let mut st = mock.service.lock();
            let ev_stored = st.log.last_state_of(&child_a.id).cloned();
            st.live.get_mut(&child_a.id).unwrap().latest_event = ev_stored;
            let outsider = test_entry("sa-other", "outsider");
            for i in 0..2050u32 {
                let mut e = SubagentEvent {
                    seq: 0,
                    timestamp_ms: 0,
                    host_key: HOST.into(),
                    session_id: "sess-other".into(),
                    parent_window_id: "win-other".into(),
                    ancestor_windows: Vec::new(),
                    id: outsider.id.clone(),
                    name: outsider.name.clone(),
                    window_id: outsider.window_id.clone(),
                    block_id: outsider.block_id.clone(),
                    resume_id: None,
                    cli_session_id: None,
                    event: "done".into(),
                    state: SubagentLiveState::Done,
                    summary: None,
                    last_message: None,
                    approval: None,
                    error: None,
                    tail: Vec::new(),
                    dedupe_key: format!("flood:{i}"),
                    answers: None,
                    minding: None,
                };
                let s = st.log.push(e.clone());
                e.seq = s;
                st.reg.event_seq = s;
            }
            assert!(st.log.has_gap(before_evict));
        }
        let r_gap = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), before_evict.to_string(), "--on".into(), "approval".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &caller,
            )
            .await;
        assert!(r_gap.ok && r_gap.data["id"] == "sa-a" && r_gap.data["gap"] == true && r_gap.data["cursor"] == seq_evict, "ring-evicted live approval must still be caught with gap=true: {r_gap:?}");

        // 7. Service::exit prunes watcher_seen for both exited child and exited child watcher (Neon #7):
        assert!(mock.service.lock().watcher_seen.keys().any(|(_, a)| a == "sa-a"));
        mock.service.exit("sa-a", Some(0));
        assert!(!mock.service.lock().watcher_seen.keys().any(|(_, a)| a == "sa-a"));
        assert!(mock.service.lock().watcher_seen.keys().any(|(w, _)| w == "child:sa-tm"));
        mock.service.exit("sa-tm", Some(0));
        assert!(!mock.service.lock().watcher_seen.keys().any(|(w, _)| w == "child:sa-tm"));
    }

    /// Round 2 review regressions (Astra R2 #1–#24 & Neon R2 #1–#9):
    /// - Neon #1: `self.changed` enabled before `look_ordered().await` never drops sibling wakeups
    /// - Astra #1 / Neon #4: cancelling watch while `write_now()` holds `write_lock` rolls back `watcher_seen`
    /// - Astra #3: duplicate `record_confirmed` populates `live.latest_event` so ring rollover keeps prompt
    /// - Astra #4: unread queued approval with predecessor box key refreshes `approval.box` in-place
    /// - Astra #5: `adopted_box` preserves box #2 number & dedupe across handover
    /// - Astra #6: hung first child does not starve healthy second child
    /// - Astra #7 & #8: unreadable/unknown transcript does not clear question; question B does not validate A
    /// - Astra #9 / Neon #3: concurrent `read_seq` bump retries within 250ms instead of stalling until timeout
    /// - Astra #10: queued prompt reassigned to a service minder during await is not delivered to ordinary watcher
    /// - Astra #11: cursorless watch on unrecorded prompt records & binds exact watermark
    /// - Astra #12/#14/#15 / Neon #6: `Request::standing_after` excludes pre-start prompts while catching post-start evicted prompts
    /// - Astra #13 / Neon #8: `--since 0` catches ring-evicted prompt at seq 1
    /// - Astra #16 / Neon #7: unverified focus-derived `scope_parent` without `--parent` never writes `parent:*`
    /// - Astra #20: child `--watcher` uses `child:<id>:<label>` (pruned on exit) + detached cap
    /// - Astra #21: persistent RPC error uses exponential backoff (no 250ms spin)
    #[tokio::test]
    async fn watch_standing_round3_regressions() {
        let rule = "─".repeat(60);
        let working = format!("✻ Thinking… (2s)\n{rule}\n❯ \n{rule}\n  esc to interrupt\n");
        let idle = format!("● Done.\n{rule}\n❯ \n{rule}\n  ? for shortcuts\n");
        let screens: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::from([("blk-a".into(), asking_screen("cargo test --box-a")), ("blk-b".into(), asking_screen("cargo test --box-b")), ("blk-q".into(), idle.clone())])));
        let delay_blocks: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let fail_blocks: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let rpc_counts: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        type ReadHook = Arc<Mutex<Option<(String, Arc<Notify>)>>>;
        let notify_on_read: ReadHook = Arc::new(Mutex::new(None));
        let sc = screens.clone();
        let db = delay_blocks.clone();
        let fb = fail_blocks.clone();
        let rc = rpc_counts.clone();
        let nr = notify_on_read.clone();
        let mock = spawn_mock_service(
            "watch-r3",
            Arc::new(move |_idx, method, payload| {
                let sc = sc.clone();
                let db = db.clone();
                let fb = fb.clone();
                let rc = rc.clone();
                let nr = nr.clone();
                let method = method.to_owned();
                let payload = payload.clone();
                Box::pin(async move {
                    if method == "com.superlogical.terminal.format" {
                        let blk = payload["block_id"].as_str().unwrap_or_default().to_owned();
                        *rc.lock().unwrap().entry(blk.clone()).or_insert(0) += 1;
                        if let Some((target, n)) = nr.lock().unwrap().clone()
                            && target == blk
                        {
                            n.notify_one();
                        }
                        let delay = db.lock().unwrap().get(&blk).copied().unwrap_or(0);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        if fb.lock().unwrap().contains(&blk) {
                            return Err(("RPC_ERROR", "simulated rpc failure".into()));
                        }
                        let text = sc.lock().unwrap().get(&blk).cloned().unwrap_or_default();
                        return Ok(json!({ "content": text }));
                    }
                    Ok(json!({ "modes": [] }))
                })
            }),
        )
        .await;

        let mut tm = test_entry("sa-tm", "tab-minder");
        tm.window_id = "win-tm".into();
        tm.block_id = "blk-tm".into();
        tm.role = Some("tab-minder".into());

        let mut ca = test_entry("sa-a", "child-a");
        ca.window_id = "win-a".into();
        ca.block_id = "blk-a".into();
        ca.parent_window_id = "win-tm".into();

        let mut cb = test_entry("sa-b", "child-b");
        cb.window_id = "win-b".into();
        cb.block_id = "blk-b".into();
        cb.parent_window_id = "win-tm".into();

        let tx_path = mock.service.paths.dir.join("r3-q.jsonl");
        let write_q = |question: &str| {
            std::fs::write(
                &tx_path,
                format!(
                    concat!(
                        "{{\"type\":\"user\",\"timestamp\":\"2026-11-28T05:10:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"ask\"}}}}\n",
                        "{{\"type\":\"assistant\",\"timestamp\":\"2026-11-28T05:10:01Z\",\"message\":{{\"role\":\"assistant\",\"stop_reason\":\"tool_use\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"tu-q\",\"name\":\"AskUserQuestion\",\"input\":{{\"questions\":[{{\"question\":\"{q}\"}}]}}}}]}}}}\n"
                    ),
                    q = question
                ),
            )
            .unwrap();
        };
        write_q("Question Alpha?");
        let mut cq = test_entry("sa-q", "child-q");
        cq.cli = olo_rex_link::agents::AgentCli::Claude;
        cq.window_id = "win-q".into();
        cq.block_id = "blk-q".into();
        cq.parent_window_id = "win-tm".into();
        cq.transcript_path = Some(tx_path.display().to_string());

        {
            let mut st = mock.service.lock();
            st.sessions.insert(
                "sess-1".into(),
                Sess {
                    windows: HashSet::from(["win-lead".into(), "win-tm".into(), "win-a".into(), "win-b".into(), "win-q".into()]),
                    blocks: HashMap::from([("blk-tm".into(), "win-tm".into()), ("blk-a".into(), "win-a".into()), ("blk-b".into(), "win-b".into()), ("blk-q".into(), "win-q".into())]),
                    active: Some("win-tm".into()),
                    fresh: true,
                    attached: true,
                },
            );
            st.reg.register(tm.clone());
            st.reg.register(ca.clone());
            st.reg.register(cb.clone());
            st.reg.register(cq.clone());
            st.live.insert(ca.id.clone(), Live::new());
            st.live.insert(cb.id.clone(), Live::new());
            st.live.insert(cq.id.clone(), Live::new());
        }

        let tm_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-tm".into()), window_from_block: true, block: Some("blk-tm".into()), child: Some(tm.clone()) };
        let det_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: None, window_from_block: false, block: None, child: None };

        // 1. Astra #5: adopted_box preserves box #2 identity across handover while minting a fresh generation key.
        {
            let mut adopted_live = Live::new();
            let sample_cls = classify_screen_rich(ca.cli, Activity::Idle, &asking_screen("cargo test --box-a"), true);
            let h = olo_rex_link::agents::state::box_hash(sample_cls.approval.as_ref().unwrap()) as u32;
            adopted_live.boxes = 2;
            adopted_live.adopted_box = Some((2, h));
            let (b_adopted, _) = adopted_live.box_after(sample_cls.approval.as_ref(), false, 2, 0xabcdef01);
            let b_adopted = b_adopted.expect("adopted box");
            assert_eq!(b_adopted.number, 2, "adopted standing box #2 must keep number 2 across handover");
            assert!(b_adopted.key.starts_with("b2-g2-abcdef01-"), "adopted box must mint fresh generation key: {}", b_adopted.key);
        }

        // 2. Astra #11: cursorless watch on an UNRECORDED standing prompt records it & sets exact watermark -> no duplicate on next `--since`.
        let r_curless = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-a".into(), "--watcher".into(), "curless-1".into(), "--on".into(), "approval".into(), "--timeout".into(), "2".into(), "--json".into()],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_curless.ok && r_curless.data["id"] == "sa-a", "cursorless watch must catch unrecorded approval: {r_curless:?}");
        let cur_c1 = r_curless.data["cursor"].as_u64().unwrap();
        let r_cur_dup = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "curless-1".into(),
                        "--since".into(),
                        cur_c1.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(!r_cur_dup.ok && r_cur_dup.code == 124, "continuing after cursorless watch of unrecorded prompt must not duplicate: {r_cur_dup:?}");

        // 3. Astra #16 / Neon #7: unverified focus-derived scope_parent (no --parent, window_from_block=false) never writes `parent:win-tm`.
        let focus_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-tm".into()), window_from_block: false, block: None, child: None };
        let r_focus = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), "0".into(), "--on".into(), "approval".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &focus_caller,
            )
            .await;
        assert!(r_focus.ok, "focus caller still gets queued approval: {r_focus:?}");
        assert!(!mock.service.lock().watcher_seen.keys().any(|(w, _)| w == "parent:win-tm"), "focus-derived scope_parent without --parent must not write watcher_seen");

        // 4. Astra #1 / Neon #4: cancelling watch while `write_now().await` holds `write_lock` rolls back `watcher_seen`.
        let bump_1 = mock.service.record(mock.service.event_of(&tm, SubagentLiveState::Working, Some("b1".into()), "r3-b1".into()));
        let held_wl = mock.service.write_lock.clone().lock_owned().await;
        let svc_c = mock.service.clone();
        let det_c = det_caller.clone();
        let handle = tokio::spawn(async move {
            svc_c
                .agent_op(
                    &Request {
                        op: "watch".into(),
                        args: vec![
                            "sa-a".into(),
                            "--watcher".into(),
                            "rollback-1".into(),
                            "--since".into(),
                            bump_1.to_string(),
                            "--on".into(),
                            "approval".into(),
                            "--timeout".into(),
                            "5".into(),
                            "--json".into(),
                        ],
                        ..Default::default()
                    },
                    &det_c,
                )
                .await
        });
        // Wait briefly until watch has acquired & inserted watcher_seen and is blocked on write_lock:
        for _ in 0..50 {
            if mock.service.lock().watcher_seen.contains_key(&("watcher:rollback-1".into(), "sa-a".into())) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(mock.service.lock().watcher_seen.contains_key(&("watcher:rollback-1".into(), "sa-a".into())));
        handle.abort();
        let _ = handle.await;
        drop(held_wl);
        assert!(!mock.service.lock().watcher_seen.contains_key(&("watcher:rollback-1".into(), "sa-a".into())), "aborting watch during write_now() must roll back watcher_seen");
        // Next watch from same watcher MUST still receive the standing prompt!
        let r_after_rb = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "rollback-1".into(),
                        "--since".into(),
                        bump_1.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "2".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_after_rb.ok && r_after_rb.data["id"] == "sa-a" && r_after_rb.data["standing"] == true, "prompt must remain deliverable after rollback: {r_after_rb:?}");

        // 5. Astra R2 #4 & R3 #1: unread queued approval with stale predecessor box key refreshes
        //    `approval.box` AND rebuilds from live screen + live transcript instead of blindly
        //    carrying an unverified pre-handover command across generations.
        let lb_b = mock.service.look(&cb, false).await;
        let box_b = lb_b.boxed.as_ref().expect("box_b");
        let old_b_key = format!("b{}-g0-oldinc99-{:08x}", box_b.number, box_b.hash as u32);
        let mut ev_b = mock.service.event_of(&cb, SubagentLiveState::WaitingApproval, Some("cargo test --box-b".into()), format!("box:b{}-{:08x}", box_b.number, box_b.hash as u32));
        let mut ap_b = Service::approval_json_with_box(lb_b.cls.approval.as_ref().unwrap(), &[], Some(&old_b_key));
        ap_b["command"] = json!("cargo test --box-b --stale-pre-handover");
        ap_b["command_from"] = json!("transcript");
        ev_b.approval = Some(ap_b);
        let pre_b = mock.service.cursor();
        let seq_b = mock.service.record(ev_b);
        {
            let mut st = mock.service.lock();
            let ev_b_stored = st.log.last_state_of(&cb.id).cloned();
            st.live.get_mut(&cb.id).unwrap().latest_event = ev_b_stored;
        }
        let r_queued_ref = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-b".into(),
                        "--watcher".into(),
                        "qref-1".into(),
                        "--since".into(),
                        pre_b.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "2".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_queued_ref.ok && r_queued_ref.data["cursor"] == seq_b, "queued approval must deliver: {r_queued_ref:?}");
        assert_eq!(r_queued_ref.data["approval"]["box"], box_b.key, "queued stale-generation approval must refresh box key in-place");
        assert_eq!(r_queued_ref.data["approval"]["command"], "cargo test --box-b", "stale-gen refresh must not trust unverified pre-handover command (Astra R3 #1)");
        assert_eq!(r_queued_ref.data["approval"]["command_from"], "screen");

        // 6. Astra #6: hung first child (3.5s delay > 2s probe cap) does NOT starve healthy second child B!
        let bump_2 = mock.service.record(mock.service.event_of(&tm, SubagentLiveState::Working, Some("b2".into()), "r3-b2".into()));
        delay_blocks.lock().unwrap().insert("blk-a".into(), 3500);
        let t_start = Instant::now();
        let r_no_starve = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["--all".into(), "--since".into(), bump_2.to_string(), "--on".into(), "approval".into(), "--timeout".into(), "5".into(), "--json".into()],
                    ..Default::default()
                },
                &tm_caller,
            )
            .await;
        delay_blocks.lock().unwrap().remove("blk-a");
        assert!(r_no_starve.ok && r_no_starve.data["id"] == "sa-b", "healthy child B must be delivered even when older child A hangs: {r_no_starve:?}");
        assert!(t_start.elapsed() < Duration::from_secs(4), "bounded probe must finish well under watch timeout");

        // 7. Neon #1: `self.changed` wakeup firing DURING slow `look_ordered().await` wakes watch immediately!
        screens.lock().unwrap().insert("blk-a".into(), working.clone());
        let read_started = Arc::new(Notify::new());
        *notify_on_read.lock().unwrap() = Some(("blk-a".into(), read_started.clone()));
        delay_blocks.lock().unwrap().insert("blk-a".into(), 350);
        // Clear backoff on sa-a so it gets probed:
        {
            let mut st = mock.service.lock();
            let la = st.live.get_mut("sa-a").unwrap();
            la.standing_fails = 0;
            la.standing_retry_after = None;
        }
        let svc_n1 = mock.service.clone();
        let det_n1 = det_caller.clone();
        let w_n1 = tokio::spawn(async move {
            svc_n1
                .agent_op(
                    &Request {
                        op: "watch".into(),
                        args: vec![
                            "sa-a".into(),
                            "sa-b".into(),
                            "--watcher".into(),
                            "neon1".into(),
                            "--since".into(),
                            bump_2.to_string(),
                            "--on".into(),
                            "approval,done".into(),
                            "--timeout".into(),
                            "4".into(),
                            "--json".into(),
                        ],
                        ..Default::default()
                    },
                    &det_n1,
                )
                .await
        });
        read_started.notified().await;
        // Record sibling B's Done event WHILE watch is awaiting `look_ordered(&ca)`!
        let mut ev_b_done = mock.service.event_of(&cb, SubagentLiveState::Done, Some("b done mid-probe".into()), "r3-done-mid".into());
        ev_b_done.event = "done".into();
        let seq_b_mid = mock.service.record(ev_b_done);
        let r_n1 = w_n1.await.unwrap();
        *notify_on_read.lock().unwrap() = None;
        delay_blocks.lock().unwrap().remove("blk-a");
        assert!(r_n1.ok && r_n1.data["id"] == "sa-b" && r_n1.data["cursor"] == seq_b_mid, "sibling wakeup during look_ordered().await must not be lost (Neon #1): {r_n1:?}");

        // 8. Astra #7 & #8: transient missing transcript does NOT clear `tx:q:` question, and question B does NOT validate A!
        let qi1 = mock.service.inspect(std::slice::from_ref(&cq)).await;
        let q_tms = qi1.get(&cq.id).and_then(|v| v["turn_started_ms"].as_u64());
        let qk_a = transcript_question_key(qi1.get(&cq.id), q_tms);
        let mut ev_qa = mock.service.event_of(&cq, SubagentLiveState::WaitingQuestion, Some("Question Alpha?".into()), qk_a);
        ev_qa.event = "waiting_question".into();
        let _seq_qa = mock.service.record(ev_qa);
        {
            let mut st = mock.service.lock();
            let ev_stored = st.log.last_state_of(&cq.id).cloned();
            st.live.get_mut(&cq.id).unwrap().latest_event = ev_stored;
        }
        let qb = mock.service.record(mock.service.event_of(&tm, SubagentLiveState::Working, Some("qb".into()), "r3-qb".into()));
        // Temporarily hide transcript file -> inspect returns unknown; must NOT clear watcher_seen!
        let hidden = mock.service.paths.dir.join("r3-q.hidden");
        std::fs::rename(&tx_path, &hidden).unwrap();
        let r_q_miss = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-q".into(), "--watcher".into(), "q7-1".into(), "--since".into(), qb.to_string(), "--on".into(), "question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(!r_q_miss.ok && r_q_miss.code == 124);
        assert!(!mock.service.lock().watcher_seen.contains_key(&("watcher:q7-1".into(), "sa-q".into())), "transient missing transcript must NOT clear watcher_seen (Astra #7)");
        assert!(
            mock.service.lock().live.get("sa-q").unwrap().standing_fails >= 2,
            "missing transcript must accumulate standing_fails backoff instead of resetting in look_until (Astra R3 #10 / Neon R3 #1)"
        );
        // Restore transcript -> same question Alpha IS now delivered!
        std::fs::rename(&hidden, &tx_path).unwrap();
        {
            let mut st = mock.service.lock();
            let lq = st.live.get_mut("sa-q").unwrap();
            lq.standing_fails = 0;
            lq.standing_retry_after = None;
        }
        let r_q_ok = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-q".into(), "--watcher".into(), "q7-1".into(), "--since".into(), qb.to_string(), "--on".into(), "question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_q_ok.ok && r_q_ok.data["id"] == "sa-q" && r_q_ok.data["summary"] == "Question Alpha?", "restored transcript must deliver Alpha: {r_q_ok:?}");
        // Replace transcript with DIFFERENT Question Beta -> fresh watcher must NOT receive saved Alpha text (Astra #8)!
        write_q("Question Beta?");
        let r_q_beta = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-q".into(), "--watcher".into(), "q8-1".into(), "--since".into(), qb.to_string(), "--on".into(), "question".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(!r_q_beta.ok && r_q_beta.code == 124, "Question Beta in transcript must NOT validate saved Alpha event (Astra #8): {r_q_beta:?}");

        // 9. Astra #3 + #13 / Neon #8 + #12/#14/#15: duplicate record_confirmed populates live.latest_event,
        //    survives 2050-event ring rollover, is caught even with `--since 0`, AND `standing_after`
        //    excludes pre-start prompts while catching post-start ring-evicted prompts!
        screens.lock().unwrap().insert("blk-a".into(), asking_screen("cargo test --evict-r3"));
        let la_r3 = mock.service.look(&ca, true).await;
        let box_r3 = la_r3.boxed.as_ref().expect("box_r3");
        let dedupe_r3 = format!("box:b{}-{:08x}", box_r3.number, box_r3.hash as u32);
        let mut ev_r3 = mock.service.event_of(&ca, SubagentLiveState::WaitingApproval, Some("cargo test --evict-r3".into()), dedupe_r3);
        ev_r3.approval = Some(Service::approval_json_with_box(la_r3.cls.approval.as_ref().unwrap(), &[], Some(&box_r3.key)));
        let pre_r3 = mock.service.cursor();
        let seq_r3 = mock.service.record(ev_r3.clone());
        // Simulate handover clearing `live.latest_event`, then duplicate `record_confirmed` repopulating it (Astra #3):
        let (cg, eg) = {
            let mut st = mock.service.lock();
            let la = st.live.get_mut("sa-a").unwrap();
            la.latest_event = None;
            (la.check_gen, la.event_gen)
        };
        assert_eq!(mock.service.record_confirmed("sa-a", cg, eg, &la_r3.cls, Some(box_r3), ev_r3), Some(seq_r3));
        assert_eq!(mock.service.lock().live.get("sa-a").unwrap().latest_event.as_ref().map(|e| e.seq), Some(seq_r3), "duplicate record_confirmed must populate live.latest_event (Astra #3)");
        // Flood ring with 2050 unrelated events so seq_r3 is evicted:
        {
            let mut st = mock.service.lock();
            let outsider = test_entry("sa-out", "outsider");
            for i in 0..2050u32 {
                let e = SubagentEvent {
                    seq: 0,
                    timestamp_ms: 0,
                    host_key: HOST.into(),
                    session_id: "sess-o".into(),
                    parent_window_id: "win-o".into(),
                    ancestor_windows: Vec::new(),
                    id: outsider.id.clone(),
                    name: outsider.name.clone(),
                    window_id: outsider.window_id.clone(),
                    block_id: outsider.block_id.clone(),
                    resume_id: None,
                    cli_session_id: None,
                    event: "done".into(),
                    state: SubagentLiveState::Done,
                    summary: None,
                    last_message: None,
                    approval: None,
                    error: None,
                    tail: Vec::new(),
                    dedupe_key: format!("r3-flood:{i}"),
                    answers: None,
                    minding: None,
                };
                let s = st.log.push(e);
                st.reg.event_seq = s;
            }
            assert!(st.log.oldest_seq() > seq_r3);
        }
        // `--since 0` MUST still catch ring-evicted seq_r3 (Astra #13 / Neon #8)!
        let r_since0 = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec!["sa-a".into(), "--watcher".into(), "since0".into(), "--since".into(), "0".into(), "--on".into(), "approval".into(), "--timeout".into(), "1".into(), "--json".into()],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_since0.ok && r_since0.data["id"] == "sa-a" && r_since0.data["cursor"] == seq_r3, "--since 0 must catch ring-evicted live approval: {r_since0:?}");
        // `standing_after: Some(seq_r3)` excludes seq_r3 (send --wait / follow_watch pre-start, Astra #12/#14):
        let r_excl = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "sa-excl".into(),
                        "--since".into(),
                        seq_r3.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    standing_after: Some(seq_r3),
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(!r_excl.ok && r_excl.code == 124, "standing_after == seq_r3 must exclude pre-start prompt: {r_excl:?}");
        // `standing_after: Some(pre_r3)` STILL catches post-start ring-evicted seq_r3 (Astra #14)!
        let r_post = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "sa-post".into(),
                        "--since".into(),
                        pre_r3.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    standing_after: Some(pre_r3),
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(r_post.ok && r_post.data["id"] == "sa-a" && r_post.data["cursor"] == seq_r3, "standing_after < seq_r3 must still catch post-start ring-evicted prompt (Astra #14): {r_post:?}");

        // 10. Astra #20: child caller with `--watcher` keys under `child:<id>:<label>` (pruned on exit) & detached cap enforced!
        let r_child_w = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "lbl1".into(),
                        "--since".into(),
                        pre_r3.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "1".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &tm_caller,
            )
            .await;
        assert!(r_child_w.ok);
        assert!(mock.service.lock().watcher_seen.contains_key(&("child:sa-tm:lbl1".into(), "sa-a".into())));
        {
            let mut st = mock.service.lock();
            for i in 0..(MAX_DETACHED_WATCHER_SEEN + 20) {
                record_watcher_seen_locked(&mut st, format!("watcher:cap-{i}"), "sa-a".into(), (i as u64) + 1);
            }
            let detached = st.watcher_seen.keys().filter(|(w, _)| !w.starts_with("child:")).count();
            assert!(detached <= MAX_DETACHED_WATCHER_SEEN, "detached watcher_seen must stay bounded at {MAX_DETACHED_WATCHER_SEEN}, got {detached}");
        }
        mock.service.exit("sa-tm", Some(0));
        assert!(!mock.service.lock().watcher_seen.keys().any(|(w, _)| watcher_owner_child(w) == Some("sa-tm")), "exiting child must prune child:ID:label entries");

        // 11. Astra #21: persistent RPC error backs off exponentially instead of spinning 4x/sec!
        fail_blocks.lock().unwrap().insert("blk-a".into());
        let before_fail_rpcs = rpc_counts.lock().unwrap().get("blk-a").copied().unwrap_or(0);
        let r_fail = mock
            .service
            .agent_op(
                &Request {
                    op: "watch".into(),
                    args: vec![
                        "sa-a".into(),
                        "--watcher".into(),
                        "backoff-1".into(),
                        "--since".into(),
                        pre_r3.to_string(),
                        "--on".into(),
                        "approval".into(),
                        "--timeout".into(),
                        "2".into(),
                        "--json".into(),
                    ],
                    ..Default::default()
                },
                &det_caller,
            )
            .await;
        assert!(!r_fail.ok && r_fail.code == 124);
        let fail_rpcs = rpc_counts.lock().unwrap().get("blk-a").copied().unwrap_or(0) - before_fail_rpcs;
        assert!(fail_rpcs <= 4, "persistent RPC error must back off exponentially (<= 4 reads in 2s, got {fail_rpcs})");
        assert!(mock.service.lock().live.get("sa-a").unwrap().standing_fails >= 2);
        fail_blocks.lock().unwrap().remove("blk-a");
        {
            let mut st = mock.service.lock();
            let la = st.live.get_mut("sa-a").unwrap();
            la.standing_fails = 0;
            la.standing_retry_after = None;
        }

        // 12. Round 3 polish regressions:
        //  - Astra #18 / Neon #6: labelled `child:<id>:<label>` keys are also bounded at MAX_DETACHED_WATCHER_SEEN
        //  - Astra #21: empty `--parent ""` is rejected
        //  - Astra #3: `write_now()` timeout returns `since_floor` cursor (not `self.cursor()`)
        //  - Astra #13 / Neon #3: `republish_standing_question` emits `tx:q:` and `box:b{n}-{hash32}` dedupe keys
        //  - Astra #2 / Neon #5: `status` mints `tx:q:` and service-minder `send` rejects a stale `tx:q:` grant
        {
            let mut st = mock.service.lock();
            for i in 0..(MAX_DETACHED_WATCHER_SEEN + 20) {
                record_watcher_seen_locked(&mut st, format!("child:sa-b:lbl-{i}"), "sa-a".into(), (i as u64) + 1);
            }
            let capped = st.watcher_seen.keys().filter(|(w, _)| is_capped_watcher_key(w)).count();
            assert!(capped <= MAX_DETACHED_WATCHER_SEEN, "labelled child:ID:label keys must stay bounded at {MAX_DETACHED_WATCHER_SEEN}, got {capped}");
        }
        let r_empty_p = mock.service.agent_op(&Request { op: "watch".into(), args: vec!["--parent".into(), "".into(), "--json".into()], ..Default::default() }, &det_caller).await;
        assert!(!r_empty_p.ok && r_empty_p.code == 2, "empty --parent must be rejected (Astra R3 #21): {r_empty_p:?}");

        let floor_w = mock.service.cursor();
        {
            let held_w2 = mock.service.write_lock.clone().lock_owned().await;
            let svc_w = mock.service.clone();
            let det_w = det_caller.clone();
            let h_wto = tokio::spawn(async move {
                svc_w
                    .agent_op(
                        &Request {
                            op: "watch".into(),
                            args: vec![
                                "sa-a".into(),
                                "--watcher".into(),
                                "wto-1".into(),
                                "--since".into(),
                                floor_w.to_string(),
                                "--on".into(),
                                "approval".into(),
                                "--timeout".into(),
                                "1".into(),
                                "--json".into(),
                            ],
                            ..Default::default()
                        },
                        &det_w,
                    )
                    .await
            });
            for _ in 0..50 {
                if mock.service.lock().watcher_seen.contains_key(&("watcher:wto-1".into(), "sa-a".into())) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(mock.service.lock().watcher_seen.contains_key(&("watcher:wto-1".into(), "sa-a".into())), "watch must be blocked in write_now()");
            // Record a newer event WHILE watch is blocked inside `write_now()`:
            let seq_newer = mock.service.record(mock.service.event_of(&cb, SubagentLiveState::Working, Some("newer".into()), "r3-newer".into()));
            assert!(seq_newer > floor_w);
            let r_wto = h_wto.await.unwrap();
            drop(held_w2);
            assert!(!r_wto.ok && r_wto.code == 124, "blocked write_now must time out cleanly: {r_wto:?}");
            assert_eq!(r_wto.data["cursor"], floor_w, "write_now timeout must return since_floor so newer events are not skipped (Astra R3 #3)");
        }

        // Republish standing approval & transcript question (`mind --off`, Astra R3 #13 / Neon R3 #3):
        mock.service.republish_standing_question(&cb).await;
        let rep_b = mock.service.lock().log.last_state_of("sa-b").cloned().expect("republished sa-b");
        assert!(rep_b.dedupe_key.starts_with("box:b"), "republished approval must use box:b{{n}}-{{hash32}} dedupe key: {}", rep_b.dedupe_key);
        mock.service.republish_standing_question(&cq).await;
        let rep_q = mock.service.lock().log.last_state_of("sa-q").cloned().expect("republished sa-q");
        assert!(rep_q.dedupe_key.starts_with("tx:q:"), "republished transcript question must use tx:q: dedupe key: {}", rep_q.dedupe_key);

        // Service-minder exact `tx:q:` gate (Astra R3 #2 / Neon R3 #5):
        let mut sm = test_entry("sa-sm", "svc-minder");
        sm.window_id = "win-sm".into();
        sm.block_id = "blk-sm".into();
        sm.minds = Some("sa-q".into());
        {
            let mut st = mock.service.lock();
            st.reg.register(sm.clone());
            let mut cq_m = cq.clone();
            cq_m.minded_by = Some(sm.id.clone());
            st.reg.register(cq_m);
            st.told.insert((sm.id.clone(), cq.id.clone()), ToldGrant { token: "tx:q:stale-question-token".into(), rules_gen: 0, is_approval: false, read_seq: 0 });
        }
        let sm_caller = crate::agentd::ops::Caller { session: Some("sess-1".into()), window: Some("win-sm".into()), window_from_block: true, block: Some("blk-sm".into()), child: Some(sm.clone()) };
        let r_stale_send = mock.service.agent_op(&Request { op: "send".into(), args: vec!["sa-q".into(), "answer".into(), "--json".into()], ..Default::default() }, &sm_caller).await;
        assert!(!r_stale_send.ok && r_stale_send.stderr.contains("isn't what you were last told"), "mismatched tx:q: grant must be rejected: {r_stale_send:?}");
        let r_sm_status = mock.service.agent_op(&Request { op: "status".into(), args: vec!["sa-q".into(), "--json".into()], ..Default::default() }, &sm_caller).await;
        assert!(r_sm_status.ok, "minder status must succeed: {r_sm_status:?}");
        let minted_tok = mock.service.lock().told.get(&(sm.id.clone(), cq.id.clone())).map(|g| g.token.clone());
        assert_eq!(minted_tok.as_deref(), Some(rep_q.dedupe_key.as_str()), "status must mint exact transcript_question_key for service minder");
    }
}
