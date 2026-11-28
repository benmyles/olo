//! Agent Screens (`olo-screen`): screens of their own for agents, which
//! the person watches and can take over. With a command (below) it is the
//! agents' tool; without one it opens the window that shows the screens.
//! See `screens.rs` for what a screen is and HACKING.md, "Agent Screens".

mod atspi;
mod find;
mod sidebar;
mod viewer;

pub(crate) use olo_screen_pane::{export, history, moving, record, screens};

use std::{path::PathBuf, time::Duration};

use olo_kit::{commands, instance};
use olo_protocol::{Frame, ImageFormat, Reply, Request, SyntheticEvent, WindowInfo};
use serde::Serialize;
use serde_json::json;

const APP_ID: &str = "olo.screen";

const USAGE: &str = "\
usage: olo-screen                         open Agent Screens (watch, take control)
       olo-screen watch [[HOST:]NAME]     the same, showing NAME (or HOST:NAME on a Rex host)
       olo-screen start [NAME] [--size WxH] [--scale N] [--sound]
                                             start a screen (default: agent, 1440x900, 1x, silent),
                                             or start a stopped or archived one again (as it was,
                                             unless given); prints it as JSON
       olo-screen list [--all [--json]] [--host HOST [--json]]
                                             local screens as JSON; --all includes connected Rex
                                             hosts; --host HOST lists one Rex host's screens
       olo-screen rename NAME [TITLE]     give a screen a name to show by (none: its own)
       olo-screen stop NAME               end a screen and everything on it; the screen is kept
                                             (stopped: its history, Chrome profile, passwords)
       olo-screen archive NAME            a stopped screen out of the way (listed as archived)
       olo-screen unarchive NAME          back among the stopped ones
       olo-screen delete NAME             a screen and everything it kept, to the Trash
       olo-screen record-video NAME [--seconds N] [--fps N] [--window ID] [--region X,Y,W,H]
                               [--show-clicks] [--show-keys] [--sound computer|microphone|both|none] [--as-gif]
                          [--no-pointer] [--pointer-size K] [--smooth-pointer] [--zoom-follow] [--background]
                          [--camera ID] [--encoder auto|cpu|nvidia] [--codec h264|hevc|av1] [--to PATH]
                                             a video of the screen (or one window, or part), into
                                             ~/Videos unless told; prints its path (JSON) at once
       olo-screen mark-video NAME TITLE   this moment of the video marked (a chapter)
       olo-screen caption-video NAME TEXT [--seconds N] | --clear
                                          a caption in the video (never on the screen), low in
                                          the middle, for N seconds or till the next
       olo-screen zoom-video NAME X,Y,W,H|out
                                          the video zoomed to that part of the screen (points),
                                          gliding there and held; out: back to the whole
       olo-screen pause-video NAME        nothing recorded till resume-video (the pause cut out)
       olo-screen resume-video NAME
       olo-screen stop-video NAME         the video finished; prints where it went
       olo-screen video-frame VIDEO [OUT.png] [--at SECONDS]
                                             the picture a video shows at a time (0: its first),
                                             to check one recorded
       olo-screen history NAME [--json] [--kind KIND] [--search TEXT] [--last N] [--clear]
                                             what was done on it: each action, what it matched, how it went
       olo-screen history NAME --look N [OUT.png]
                                             the screen as it was when action N (from 1) ended
       olo-screen history NAME --export OUT.mp4 [--part SPEC] [--from N] [--to N] [--zoom 1|2|3] [--encoder auto|cpu|nvidia|vaapi]
                                             as a video: its actions (N to N, from 1), the time
                                             between them skipped; each pixel zoomed (small
                                             screens easier to read), as far as H.264 allows
       olo-screen stats [NAME] [--json]   when each screen was started and last used, how long it
                                             ran and was in use, its actions by kind, its programs
       olo-screen env NAME                its environment, for a shell (eval \"$(…)\")
       olo-screen run NAME [--quick] [--] PROGRAM [ARGS…]
                                             start a program on it; prints its pid and, under
                                             after, the screen once it shows (a window opens or
                                             the focused one changes; at most 5 s)
       olo-screen take NAME|--new --window ID…
                                             the person's windows (ids on their display) onto the
                                             screen (--new: a new one), their programs running on
                                             as they go; only when the person asks
       olo-screen give NAME [--window ID…] [--workspace N] [--display OUTPUT]
                                             the screen's windows (default: all) to the person's
                                             display, onto its workspace N (from 1; default: the
                                             one showing) of OUTPUT (default: the active display)
       olo-screen look NAME [OUT.png] [--window ID] [--crop X,Y,W,H] [--no-pointer]
                                             a picture of the screen (or one window, or part);
                                             without OUT, a new file among the screen's pictures
                                             (the newest 20 kept); prints its path
       olo-screen windows NAME            its windows (id, app, title, focused, frame, content,
                                             popups: open menus' frames), as JSON
       olo-screen tree NAME [--window ID] [--json] [--role R] [--contains TEXT]
                          [--visible-only] [--depth N] [--max-len N] [--cells]
                                             what Olo apps' windows show: buttons, fields,
                                             text, lists (row N/M), open menus, with where they
                                             are on the screen
       olo-screen menus NAME [--window ID]
                                             the menus of a window's app (default: the focused one)
       olo-screen invoke NAME COMMAND [--window ID]
                                             choose a menu command (as `menus` lists it,
                                             or by its label)
       olo-screen review NAME [--window ID] [--json]
                                             the layout review of Olo apps' windows as drawn
                                             now: errors (blurry, seams), then what almost
                                             lines up or sits off-centre, to look at closely
       olo-screen focus|close|minimize|zoom NAME [--window ID]
       olo-screen resize NAME WxH [--window ID]    its content's size
       olo-screen place NAME X Y [WxH] [--window ID]
                                             its frame's top left (and content's size)
       olo-screen click NAME X Y [--right|--middle] [--double]   (or X,Y, or --at X,Y)
       olo-screen click NAME FIND… [--right|--middle] [--double]
                                             the middle of what FIND matches (below)
       olo-screen set NAME FIND… TEXT     click into a field, replace its text
       olo-screen select NAME FIND… ITEM  open a dropdown, choose ITEM from its menu
                                             (a page's <select>: by its words, from its options)
       olo-screen wait NAME [FIND…] [--gone] [--idle] [--timeout MS]
                                             until it shows (or goes, or the windows stop changing)
       olo-screen wait NAME --free [--timeout MS]
                                             until the person hands the screen back (Take Control)
       olo-screen scroll-to NAME FIND…|WORDS [--at X,Y]
                                             scroll the list (under X,Y) until it shows
       olo-screen move NAME X Y
       olo-screen down NAME X Y [--right] | up NAME [--right]
       olo-screen drag NAME X1 Y1 X2 Y2 [--steps N] [--duration MS]
                                             press, move there in N steps (20) over MS (300), let go
       olo-screen scroll NAME DX DY [--no-end]       (or --dx N --dy N; --at X,Y for where)
                                             by pixels, as on a touchpad (positive: down/right);
                                             the fingers lift after, unless --no-end
       olo-screen wheel NAME NOTCHES [H-NOTCHES]
                                             mouse-wheel notches (positive: down/right), one at a time
       olo-screen key NAME CHORD…         e.g. Cmd+S, Ctrl+Shift+T, Enter, Esc, Tab, Down
       olo-screen type NAME TEXT          type text (what a US keyboard types)

While the person controls a screen (Agent Screens' Take Control), every
action on it is refused, doing nothing, until they hand it back: reading
it (windows, tree, look) still answers.

FIND is --name NAME (exact, else part of it; a value counts too), --role ROLE,
--match ROLE:TEXT, --text TEXT (in the name or value), --index N (from 0, when
several match) and --window ID.

Actions print JSON: what they matched and, under after, how the screen is
once it has settled (nothing changed for a quarter of a second and no page
loading; at most 2 s, or 8 s while a page loads): the focused window and
element; settled (false: still changing when it was read); picture, a PNG
of the whole screen then; windows; and tree, the focused window's as
`tree --visible-only` gives it (its first 150 lines). So you act on what is
there now. --quick (anywhere after NAME; for run, before PROGRAM) or
OLO_SCREEN_QUICK=1, for a run of quick actions: the focus alone, at once
(move, down, up, drag, scroll and wheel then print nothing).

A moved program keeps running: an Olo app re-executes there with its
state (one window of several by a copy of the app, where it can), any
other program's connection is replayed there with all its windows. The
person's programs on a screen go back to their display when it stops.

Coordinates are the screen's logical pixels, from its top left; Cmd is ⌘
(apps see it as Ctrl). Programs run on a screen with WAYLAND_DISPLAY and
OLO_SOCKET set to it; a screen's own Deskbar and desktop are there too.";

fn fail(message: impl std::fmt::Display) -> i32 {
    eprintln!("olo-screen: {message}");
    1
}

/// Every `--window ID` given, each with its id: one without (the last
/// word, or another option after it) is an error, never "every window"
/// (`give NAME --window` gave every window to the desktop: Astra's
/// review, 2026-11-28).
fn window_ids(args: &[String]) -> Result<Vec<u64>, String> {
    let mut ids = Vec::new();
    for (at, a) in args.iter().enumerate() {
        if a != "--window" {
            continue;
        }
        let id = args.get(at + 1).filter(|v| !v.starts_with("--")).ok_or("--window needs a window's id after it (`windows` lists them)")?;
        ids.push(id.parse::<u64>().map_err(|_| format!("{id} isn't a window's id"))?);
    }
    Ok(ids)
}

/// Why nothing moved.
fn stayed_why(o: &moving::Outcome) -> String {
    match o.stayed.first() {
        Some(s) => format!("nothing moved: {} {}", if s.app_id.is_empty() { "the window" } else { s.app_id.as_str() }, s.why),
        None => "nothing moved".into(),
    }
}

/// JSON payload printed by `olo-screen take`, including `"pid"` and `"started"`
/// so bridged `take --new` callers (and Rex's desktop owner tracker) always
/// observe the newly started screen's exact session generation (`agyrev-da27b4f6e1` #3).
fn take_result_json(s: &screens::Screen, outcome: &moving::Outcome) -> serde_json::Value {
    json!({
        "name": s.name,
        "pid": s.pid,
        "started": s.started,
        "went": outcome.went,
        "stayed": outcome.stayed,
    })
}

fn number(s: Option<&String>, what: &str) -> Result<f64, String> {
    s.ok_or_else(|| format!("missing {what}"))?.parse().map_err(|_| format!("{what} must be a number"))
}

/// The value after `flag`, if given.
fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1))
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn button(args: &[String]) -> u32 {
    if has(args, "--right") {
        screens::RIGHT
    } else if has(args, "--middle") {
        screens::MIDDLE
    } else {
        screens::LEFT
    }
}

thread_local! {
    /// What the command printed last (its answer), for its history.
    static ANSWER: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
}

/// Normalize `--quick` out of command arguments identically in `cli()` and
/// `run_command()` so `olo-screen click --quick qa …` still derives `named = "qa"`
/// for both history recording and direct CLI owner attribution (Neon #9).
fn strip_quick_flag(command: &str, raw_rest: &[String]) -> (Vec<String>, bool) {
    let mut rest: Vec<String> = raw_rest.to_vec();
    let mut quick = std::env::var("OLO_SCREEN_QUICK").is_ok_and(|v| !v.is_empty() && v != "0");
    if command == "run" {
        if rest.get(1).map(String::as_str) == Some("--quick") {
            rest.remove(1);
            quick = true;
        }
    } else if history::ACTIONS.contains(&command) || command == "maximize" {
        let n = rest.len();
        rest.retain(|a| a != "--quick");
        quick |= rest.len() != n;
    }
    (rest, quick)
}

/// Extract screen session `(pid, started)` ONLY from replies whose JSON
/// describes the screen session itself (`start` and `take`), NEVER from
/// `run` / `record-video` whose `"pid"` is the launched child program (Astra #9).
fn answer_screen_gen(command: &str, answer: Option<&serde_json::Value>) -> (Option<i32>, Option<u64>) {
    if !matches!(command, "start" | "take") {
        return (None, None);
    }
    let pid = answer.and_then(|v| v["pid"].as_i64()).and_then(|p| i32::try_from(p).ok());
    let started = answer.and_then(|v| v["started"].as_u64());
    (pid, started)
}

/// Read Rex pane attribution environment variables consistently with
/// `crates/rex-link/src/screens.rs`: `REX_BLOCK`, `REX_SESSION`, and
/// `REX_SERVER_SOCKET` (stripping optional `unix://` prefix — Astra #12).
fn read_direct_rex_env(get: impl Fn(&str) -> Option<String>) -> (Option<String>, String, Option<String>) {
    let block = get("REX_BLOCK").filter(|s| !s.trim().is_empty());
    let session = get("REX_SESSION").unwrap_or_default();
    let socket = get("REX_SERVER_SOCKET").filter(|s| !s.trim().is_empty()).map(|s| s.strip_prefix("unix://").unwrap_or(&s).to_owned());
    (block, session, socket)
}

/// Run a command; its exit code, or None for the window. An action on a
/// screen goes in that screen's history (`history`), with what it
/// answered and how it went.
fn cli(args: &[String]) -> Option<Result<(), String>> {
    let command = args.first()?.clone();
    let (norm_rest, _) = strip_quick_flag(&command, &args[1..]);
    let named = norm_rest.first().filter(|a| !a.starts_with("--")).cloned();
    let is_start = command == "start" || (command == "take" && has(&norm_rest, "--new"));
    let is_stop_or_delete = matches!(command.as_str(), "stop" | "delete");
    // Neon #6: match `rex-link`/`screens_sock` — `rename` never transfers ownership.
    let is_mutating =
        is_start || matches!(command.as_str(), "maximize" | "take") || (history::ACTIONS.contains(&command.as_str()) && !matches!(command.as_str(), "stop" | "delete" | "archive" | "unarchive"));
    // Capture the runtime session generation BEFORE executing a mutating or
    // stop/delete command so a delayed completion can verify under lock that
    // the same generation is still present (Astra #10).
    let pre_gen = if (is_mutating || is_stop_or_delete) && !is_start { named.as_deref().filter(|n| screens::valid_name(n)).and_then(|n| read_runtime_screen_gen_in(&screens::dir(), n)) } else { None };

    let at = history::now_ms();
    ANSWER.with(|a| a.borrow_mut().take());
    let result = run_command(args)?;
    let answer = ANSWER.with(|a| a.borrow_mut().take()).map(for_history);
    // The screen: as named, or the one `start` / `take --new` chose.
    let name = if matches!(command.as_str(), "start" | "take") { answer.as_ref().and_then(|v| v["name"].as_str().map(str::to_owned)).or(named.clone()) } else { named.clone() };
    if result.is_ok()
        && let Some(ref target_name) = name
        && screens::valid_name(target_name)
    {
        let (ans_pid, ans_started) = answer_screen_gen(&command, answer.as_ref());
        let expected_gen = if is_start { ans_pid.map(|pid| ScreenGen { pid, started: ans_started, start_ticks: proc_start_ticks_of(pid) }) } else { pre_gen };
        if let Err(e) = record_direct_cli_owner(&command, &norm_rest, target_name, expected_gen) {
            eprintln!("olo-screen: warning: couldn't update Rex owner metadata for {target_name:?}: {e}");
        }
    }
    if history::ACTIONS.contains(&command.as_str())
        && let Some(name) = name
    {
        let args = if named.is_some() { norm_rest[1..].to_vec() } else { norm_rest.clone() };
        let action = history::Action { at, ms: history::now_ms().saturating_sub(at), kind: command, args, result: answer, error: result.as_ref().err().cloned() };
        history::record(&name, &action);
        // (Its frames kept too, if what kept them has gone.)
        if !matches!(action.kind.as_str(), "start" | "stop" | "delete" | "archive") {
            screens::ensure_recorder(&name);
        }
    }
    Some(result)
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
struct DirectOwnerPane {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_key: Option<String>,
    session_id: String,
    block_id: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
struct DirectScreenOwner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_key: Option<String>,
    session_id: String,
    block_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_by: Option<DirectOwnerPane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    began: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    screen_pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    screen_started: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    screen_start_ticks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rex_socket: Option<String>,
    at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenGen {
    pid: i32,
    started: Option<u64>,
    start_ticks: Option<u64>,
}

const DIRECT_OWNER_LOCK_TIMEOUT: Duration = Duration::from_millis(500);
static DIRECT_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn proc_start_ticks_of(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

fn direct_euid() -> libc::uid_t {
    // SAFETY: `geteuid()` has no preconditions.
    unsafe { libc::geteuid() }
}

fn direct_c_name(name: &str) -> std::io::Result<std::ffi::CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid filename {name:?}")));
    }
    std::ffi::CString::new(name).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in filename"))
}

/// Verify that every existing resolved ancestor of `dir` is either root-owned
/// without world/group write (unless sticky) or owned by our UID without world
/// write — allowing standard Linux symlink ancestors like `/home -> /var/home`
/// and `/var/run -> /run` while rejecting other-UID or world-writable ancestors
/// (Astra #20 / Neon #5, matching `apps/rex/src/screens_sock.rs::verify_safe_ancestors`).
fn direct_verify_safe_ancestors(dir: &std::path::Path, uid: libc::uid_t) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let mut anc = dir.parent();
    while let Some(p) = anc {
        if p.as_os_str().is_empty() || p == std::path::Path::new("/") {
            break;
        }
        match std::fs::metadata(p) {
            Ok(meta) => {
                if !meta.file_type().is_dir() {
                    return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("non-directory ancestor {}", p.display())));
                }
                let mode = meta.mode();
                let auid = meta.uid();
                let safe_root = auid == 0 && ((mode & 0o022) == 0 || (mode & libc::S_ISVTX) != 0);
                let safe_self = auid == uid && (mode & 0o002) == 0;
                if !safe_root && !safe_self {
                    return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("unsafe ancestor directory {}", p.display())));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        anc = p.parent();
    }
    Ok(())
}

fn direct_open_verified_owned_dir(dir: &std::path::Path, create_if_missing: bool) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::os::unix::fs::DirBuilderExt as _;
    let uid = direct_euid();
    direct_verify_safe_ancestors(dir, uid)?;
    // Walk from the closest existing trusted ancestor using single-component
    // `openat(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` + `fstat` so intermediate
    // components cannot be swapped with symlinks or foreign directories (Astra #20).
    let mut existing = dir.to_path_buf();
    let mut missing_tail: Vec<std::ffi::OsString> = Vec::new();
    while existing.parent().is_some() && std::fs::metadata(&existing).is_err() {
        let Some(fname) = existing.file_name() else { break };
        missing_tail.push(fname.to_os_string());
        if !existing.pop() {
            break;
        }
    }
    if !missing_tail.is_empty() && !create_if_missing {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("directory {} not found", dir.display())));
    }
    if !missing_tail.is_empty() {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
    }
    // Canonicalize the parent (following trusted system symlinks like `/home -> /var/home`),
    // then open and verify each component under `/` or the canonical parent with `O_NOFOLLOW`.
    let parent = dir.parent().unwrap_or_else(|| std::path::Path::new("/"));
    let leaf = dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid dir {}", dir.display())))?;
    let canon_parent = std::fs::canonicalize(parent)?;
    let mut cur_fd = {
        let root_c = std::ffi::CString::new("/").unwrap();
        let raw = unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        unsafe { OwnedFd::from_raw_fd(raw) }
    };
    for comp in canon_parent.components() {
        let std::path::Component::Normal(seg) = comp else { continue };
        let seg_str = seg.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-utf8 path component"))?;
        let cseg = direct_c_name(seg_str)?;
        let raw = unsafe { libc::openat(cur_fd.as_raw_fd(), cseg.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let next_fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(next_fd.as_raw_fd(), &mut st) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let safe_root = st.st_uid == 0 && ((st.st_mode & 0o022) == 0 || (st.st_mode & libc::S_ISVTX) != 0);
        let safe_self = st.st_uid == uid && (st.st_mode & 0o002) == 0;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || (!safe_root && !safe_self) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("unsafe ancestor component {seg_str:?}")));
        }
        cur_fd = next_fd;
    }
    let cleaf = direct_c_name(leaf)?;
    // SAFETY: NUL-terminated component opened with O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC.
    let raw = unsafe { libc::openat(cur_fd.as_raw_fd(), cleaf.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid || (st.st_mode & 0o002) != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("unsafe directory {}", dir.display())));
    }
    Ok(fd)
}

fn direct_open_verified_subdir_at(parent_fd: &std::os::fd::OwnedFd, name: &str, create_if_missing: bool) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    let uid = direct_euid();
    let cname = direct_c_name(name)?;
    if create_if_missing {
        // SAFETY: valid dirfd and single component name.
        let rc = unsafe { libc::mkdirat(parent_fd.as_raw_fd(), cname.as_ptr(), 0o700) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(err);
            }
        }
    }
    // SAFETY: valid dirfd and single component name; refuses symlinks atomically.
    let raw = unsafe { libc::openat(parent_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid || (st.st_mode & 0o002) != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("unsafe screen directory {name:?}")));
    }
    Ok(fd)
}

fn direct_read_bounded_regular_at(dir_fd: &std::os::fd::OwnedFd, file_name: &str, max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::fs::MetadataExt as _;
    let cname = direct_c_name(file_name).ok()?;
    // SAFETY: valid dirfd and single component name.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    // SAFETY: freshly opened owned descriptor.
    let f = unsafe { std::fs::File::from_raw_fd(raw) };
    let meta = f.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 || meta.uid() != direct_euid() {
        return None;
    }
    let mut buf = Vec::with_capacity((meta.len() as usize).min(64 << 10));
    f.take(max_bytes as u64 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= max_bytes).then_some(buf)
}

fn direct_write_file_atomic_at(dir_fd: &std::os::fd::OwnedFd, file_name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::sync::atomic::Ordering;
    let target_c = direct_c_name(file_name)?;
    let seq = DIRECT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.tmp.{}.{seq}", std::process::id());
    let tmp_c = direct_c_name(&tmp_name)?;
    // SAFETY: valid dirfd and single component name; O_CREAT | O_EXCL | O_NOFOLLOW guarantees exclusive creation.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: exclusively created fd owned by us.
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    let write_res = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    if let Err(e) = write_res {
        // SAFETY: we exclusively created `tmp_c`.
        unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(e);
    }
    // SAFETY: valid dirfd and single component names.
    if unsafe { libc::renameat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), dir_fd.as_raw_fd(), target_c.as_ptr()) } != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err);
    }
    Ok(())
}

fn direct_unlink_at_ignore_enoent(dir_fd: &std::os::fd::OwnedFd, file_name: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    let cname = direct_c_name(file_name)?;
    // SAFETY: valid dirfd and single component name.
    if unsafe { libc::unlinkat(dir_fd.as_raw_fd(), cname.as_ptr(), 0) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    Ok(())
}

fn direct_acquire_lock_at(dir_fd: &std::os::fd::OwnedFd, lock_name: &str, timeout: Duration) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    let uid = direct_euid();
    let cname = direct_c_name(lock_name)?;
    // SAFETY: valid dirfd and single component name; refuses symlinks.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFREG || st.st_uid != uid {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("unsafe lock file {lock_name:?}")));
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // SAFETY: valid open regular file descriptor.
        if unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(fd);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Err(err);
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("timed out waiting for lock {lock_name:?}")));
        }
        std::thread::sleep(Duration::from_millis(10).min(deadline - now));
    }
}

fn read_direct_owner_at(dir_fd: &std::os::fd::OwnedFd, file_name: &str) -> Option<DirectScreenOwner> {
    let buf = direct_read_bounded_regular_at(dir_fd, file_name, 64 * 1024)?;
    let o = serde_json::from_slice::<DirectScreenOwner>(&buf).ok()?;
    (!o.block_id.is_empty()).then_some(o)
}

#[cfg(test)]
fn read_direct_owner(path: &std::path::Path) -> Option<DirectScreenOwner> {
    let parent = path.parent()?;
    let fname = path.file_name()?.to_str()?;
    let dir_fd = direct_open_verified_owned_dir(parent, false).ok()?;
    read_direct_owner_at(&dir_fd, fname)
}

fn read_runtime_screen_gen_at(rt_fd: &std::os::fd::OwnedFd, name: &str) -> Option<ScreenGen> {
    #[derive(serde::Deserialize)]
    struct RtGen {
        name: String,
        pid: i32,
        #[serde(default)]
        started: u64,
        #[serde(default)]
        start_ticks: Option<u64>,
    }
    let buf = direct_read_bounded_regular_at(rt_fd, &format!("{name}.json"), 64 * 1024)?;
    let g = serde_json::from_slice::<RtGen>(&buf).ok()?;
    if g.name != name || g.pid <= 0 {
        return None;
    }
    let start_ticks = g.start_ticks.or_else(|| proc_start_ticks_of(g.pid));
    Some(ScreenGen { pid: g.pid, started: (g.started > 0).then_some(g.started), start_ticks })
}

fn read_runtime_screen_gen_in(rt_dir: &std::path::Path, name: &str) -> Option<ScreenGen> {
    let rt_fd = direct_open_verified_owned_dir(rt_dir, false).ok()?;
    read_runtime_screen_gen_at(&rt_fd, name)
}

fn screen_gens_match(expected: ScreenGen, actual: ScreenGen) -> bool {
    if expected.pid != actual.pid {
        return false;
    }
    if let (Some(e), Some(a)) = (expected.started, actual.started)
        && e != a
    {
        return false;
    }
    if let (Some(e), Some(a)) = (expected.start_ticks, actual.start_ticks)
        && e != a
    {
        return false;
    }
    true
}

fn owner_matches_gen(o: &DirectScreenOwner, g: ScreenGen) -> bool {
    if let Some(p) = o.screen_pid
        && p != g.pid
    {
        return false;
    }
    if let (Some(os), Some(gs)) = (o.screen_started, g.started)
        && os != gs
    {
        return false;
    }
    if let (Some(ot), Some(gt)) = (o.screen_start_ticks, g.start_ticks)
        && ot != gt
    {
        return false;
    }
    true
}

/// Record or clean up desktop screen ownership sidecars (`NAME.owner.json` in
/// runtime dir + `owner.json` in state dir) when `olo-screen` is invoked directly
/// inside a local Rex terminal pane (Astra #7–#12, #14, #24).
fn record_direct_cli_owner(command: &str, rest: &[String], name: &str, expected_gen: Option<ScreenGen>) -> std::io::Result<()> {
    let rt_dir = screens::dir();
    let state_root = history::dir(name).parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| olo_xdg::state_dir().join("screens"));
    record_direct_cli_owner_with_env(&rt_dir, &state_root, command, rest, name, expected_gen, DIRECT_OWNER_LOCK_TIMEOUT, |k| std::env::var(k).ok())
}

#[allow(clippy::too_many_arguments)]
fn record_direct_cli_owner_with_env(
    rt_dir: &std::path::Path,
    state_root: &std::path::Path,
    command: &str,
    rest: &[String],
    name: &str,
    expected_gen: Option<ScreenGen>,
    lock_timeout: Duration,
    get_env: impl Fn(&str) -> Option<String>,
) -> std::io::Result<()> {
    let is_start = command == "start" || (command == "take" && has(rest, "--new"));
    let is_stop_or_delete = matches!(command, "stop" | "delete");
    let is_mutating = is_start || matches!(command, "maximize" | "take") || (history::ACTIONS.contains(&command) && !matches!(command, "stop" | "delete" | "archive" | "unarchive"));
    if !is_mutating && !is_stop_or_delete {
        return Ok(());
    }
    if is_stop_or_delete {
        return unlink_direct_runtime_owner_in(rt_dir, state_root, name, expected_gen, lock_timeout);
    }
    let (env_block, env_session, env_socket) = read_direct_rex_env(get_env);
    // Astra #8: skip irrelevant mutating actions outside Rex BEFORE opening any lock.
    if !is_start && env_block.is_none() {
        return Ok(());
    }
    record_direct_owner_in(rt_dir, state_root, name, is_start, env_block.as_deref(), &env_session, env_socket.as_deref(), expected_gen, lock_timeout)
}

fn unlink_direct_runtime_owner_in(rt_dir: &std::path::Path, state_root: &std::path::Path, name: &str, expected_gen: Option<ScreenGen>, lock_timeout: Duration) -> std::io::Result<()> {
    if !screens::valid_name(name) {
        return Ok(());
    }
    let rt_fd = match direct_open_verified_owned_dir(rt_dir, false) {
        Ok(fd) => fd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let _life_lock = direct_acquire_lock_at(&rt_fd, &format!("{name}.lifecycle.lock"), lock_timeout)?;
    let state_dir_fd = match direct_open_verified_owned_dir(state_root, false) {
        Ok(root_fd) => match direct_open_verified_subdir_at(&root_fd, name, false) {
            Ok(fd) => Some(fd),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let _owner_lock =
        if let Some(ref sfd) = state_dir_fd { direct_acquire_lock_at(sfd, ".owner.lock", lock_timeout)? } else { direct_acquire_lock_at(&rt_fd, &format!(".{name}.owner.lock"), lock_timeout)? };
    // Never unlink if a running descriptor is present (e.g. a replacement started concurrently).
    if read_runtime_screen_gen_at(&rt_fd, name).is_some() {
        return Ok(());
    }
    let rt_owner_name = format!("{name}.owner.json");
    if let Some(exp) = expected_gen
        && let Some(existing) = read_direct_owner_at(&rt_fd, &rt_owner_name)
        && !owner_matches_gen(&existing, exp)
    {
        return Ok(());
    }
    direct_unlink_at_ignore_enoent(&rt_fd, &rt_owner_name)
}

#[allow(clippy::too_many_arguments)]
fn record_direct_owner_in(
    rt_dir: &std::path::Path,
    state_root: &std::path::Path,
    name: &str,
    is_start: bool,
    env_block: Option<&str>,
    env_session: &str,
    env_socket: Option<&str>,
    expected_gen: Option<ScreenGen>,
    lock_timeout: Duration,
) -> std::io::Result<()> {
    if !screens::valid_name(name) {
        return Ok(());
    }
    let block_opt = env_block.filter(|b| !b.is_empty());
    // Astra #8: skip irrelevant non-start actions outside Rex before touching any lock.
    if !is_start && block_opt.is_none() {
        return Ok(());
    }

    let rt_fd = match direct_open_verified_owned_dir(rt_dir, is_start) {
        Ok(fd) => fd,
        Err(e) if !is_start && e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // Acquire lifecycle lock first (same lock ordering everywhere) to verify
    // runtime session generation atomically against Start/Stop (Astra #10).
    let _life_lock = direct_acquire_lock_at(&rt_fd, &format!("{name}.lifecycle.lock"), lock_timeout)?;

    let create_state = is_start && block_opt.is_some();
    let state_dir_fd = match direct_open_verified_owned_dir(state_root, create_state) {
        Ok(root_fd) => match direct_open_verified_subdir_at(&root_fd, name, create_state) {
            Ok(fd) => Some(fd),
            Err(e) if !create_state && e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        },
        Err(e) if !create_state && e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };

    let _owner_lock =
        if let Some(ref sfd) = state_dir_fd { direct_acquire_lock_at(sfd, ".owner.lock", lock_timeout)? } else { direct_acquire_lock_at(&rt_fd, &format!(".{name}.owner.lock"), lock_timeout)? };

    let rt_gen = read_runtime_screen_gen_at(&rt_fd, name);
    let rt_owner_name = format!("{name}.owner.json");

    // Astra #10: verify under both locks that the runtime generation still
    // matches what this invocation started or acted on before writing/unlinking.
    if is_start {
        if let Some(exp) = expected_gen {
            let Some(cur) = rt_gen else { return Ok(()) };
            if !screen_gens_match(exp, cur) {
                return Ok(());
            }
        }
    } else if let Some(exp) = expected_gen {
        let Some(cur) = rt_gen else { return Ok(()) };
        if !screen_gens_match(exp, cur) {
            return Ok(());
        }
    } else if rt_gen.is_some() {
        // Command acted on a stopped screen (`pre_gen = None`), but a new
        // running screen started concurrently: never clobber its owner.
        return Ok(());
    }

    let Some(block_id) = block_opt else {
        // Astra #11: verified standalone Start outside Rex clears BOTH runtime
        // `NAME.owner.json` AND persistent `owner.json` so stopping later cannot
        // resurrect the previous Rex pane's ownership.
        direct_unlink_at_ignore_enoent(&rt_fd, &rt_owner_name)?;
        if let Some(ref sfd) = state_dir_fd {
            direct_unlink_at_ignore_enoent(sfd, "owner.json")?;
        }
        return Ok(());
    };

    let need_rt = is_start || rt_gen.is_some();
    let need_st = is_start || state_dir_fd.is_some();
    if !need_rt && !need_st {
        return Ok(());
    }

    let rt_existing = read_direct_owner_at(&rt_fd, &rt_owner_name);
    let st_existing = state_dir_fd.as_ref().and_then(|sfd| read_direct_owner_at(sfd, "owner.json"));
    let existing = rt_existing.as_ref().or(st_existing.as_ref());

    let effective_gen = rt_gen.or(expected_gen);
    let screen_pid = effective_gen.map(|g| g.pid);
    let screen_started = effective_gen.and_then(|g| g.started);
    let screen_start_ticks = effective_gen.and_then(|g| g.start_ticks);

    let same_gen = |o: &DirectScreenOwner| effective_gen.is_none_or(|g| owner_matches_gen(o, g));
    let started_by = if is_start {
        Some(DirectOwnerPane { host_key: None, session_id: env_session.to_owned(), block_id: block_id.to_owned() })
    } else {
        existing.filter(|o| same_gen(o)).and_then(|o| o.started_by.clone())
    };

    let matches_target = |o: &DirectScreenOwner| {
        o.host_key.is_none()
            && o.session_id == env_session
            && o.block_id == block_id
            && o.rex_socket.as_deref() == env_socket
            && (!is_start || o.started_by == started_by)
            && (screen_pid.is_none() || o.screen_pid == screen_pid)
            && (screen_started.is_none() || o.screen_started == screen_started)
            && (screen_start_ticks.is_none() || o.screen_start_ticks == screen_start_ticks)
    };
    let rt_ok = !need_rt || rt_existing.as_ref().is_some_and(matches_target);
    let st_ok = !need_st || st_existing.as_ref().is_some_and(matches_target);
    let copies_agree = !need_rt || !need_st || rt_existing == st_existing;
    // Astra #24: skip rewriting unchanged sidecars so repeated clicks do not churn
    // files or wake Rex's watcher unnecessarily.
    if rt_ok && st_ok && copies_agree {
        return Ok(());
    }

    // If one authoritative copy already matches and the other merely drifted or
    // went missing, repair the missing/stale copy without advancing `at`.
    if let Some(auth) = rt_existing.as_ref().filter(|o| matches_target(o)).or_else(|| st_existing.as_ref().filter(|o| matches_target(o))) {
        let bytes = serde_json::to_vec_pretty(auth).map_err(std::io::Error::other)?;
        if need_rt && rt_existing.as_ref() != Some(auth) {
            direct_write_file_atomic_at(&rt_fd, &rt_owner_name, &bytes)?;
        }
        if need_st
            && let Some(ref sfd) = state_dir_fd
            && st_existing.as_ref() != Some(auth)
        {
            direct_write_file_atomic_at(sfd, "owner.json", &bytes)?;
        }
        return Ok(());
    }

    let owner = DirectScreenOwner {
        host_key: None,
        session_id: env_session.to_owned(),
        block_id: block_id.to_owned(),
        started_by,
        pid: None,
        began: None,
        screen_pid,
        screen_started,
        screen_start_ticks,
        rex_socket: env_socket.map(str::to_owned),
        at: history::now_ms(),
    };
    let bytes = serde_json::to_vec_pretty(&owner).map_err(std::io::Error::other)?;
    if need_rt {
        direct_write_file_atomic_at(&rt_fd, &rt_owner_name, &bytes)?;
    }
    if need_st && let Some(ref sfd) = state_dir_fd {
        direct_write_file_atomic_at(sfd, "owner.json", &bytes)?;
    }
    Ok(())
}

/// An action's answer as its history keeps it: what the screen was after
/// it, but not its picture, tree and windows (the frames are kept; they'd
/// fill the history, which is kept under 8 MB by dropping the oldest).
fn for_history(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(after) = v.get_mut("after").and_then(|a| a.as_object_mut()) {
        for k in ["picture", "tree", "windows"] {
            after.remove(k);
        }
    }
    v
}

/// The screen has settled once nothing it shows has changed for this long
/// (as `wait --idle` has it): a page's answer to a click a moment later
/// (300 ms in the test) is what an action says, not the page before it.
const QUIET: Duration = Duration::from_millis(250);
/// How long an action waits at most for the screen to settle before it
/// says how it is; while a page is loading (`UiNode::busy`), longer.
const SETTLE_AT_MOST: Duration = Duration::from_secs(2);
const LOADING_AT_MOST: Duration = Duration::from_secs(8);
/// How long `run` waits at most for its program to show.
const SHOW_AT_MOST: Duration = Duration::from_secs(5);
/// The most lines of the focused window's tree an action gives.
const AFTER_TREE_LINES: usize = 150;
/// The pictures kept for each screen (`picture_path`).
const PICTURES_KEPT: usize = 20;

/// How the screen is after an action (Ben, 2026-11-18: "i want the agent
/// to always know what the latest state is"), once it has stopped
/// changing: the focus, a picture of the whole screen, the focused
/// window's tree as `tree --visible-only` gives it, and the windows. So
/// an agent acts on what is there, never on what it last saw. Quick (the
/// focus alone, as soon as the app has taken it in), for a run of quick
/// actions: `--quick`, OLO_SCREEN_QUICK=1.
#[derive(Serialize)]
struct Afterwards {
    #[serde(flatten)]
    focus: find::After,
    /// For `run`: something changed within `SHOW_AT_MOST` (a window
    /// opened, the focused one changed); false, it shows nothing yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    shown: Option<bool>,
    /// It stopped changing within `SETTLE_AT_MOST` (else, how it was then).
    #[serde(skip_serializing_if = "Option::is_none")]
    settled: Option<bool>,
    /// The picture's path (a PNG among the screen's pictures).
    #[serde(skip_serializing_if = "Option::is_none")]
    picture: Option<String>,
    /// The screen's windows, front first: id, title, app, focused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    windows: Vec<String>,
    /// The focused window's tree, its first `AFTER_TREE_LINES` lines.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tree: Vec<String>,
    /// What couldn't be read or taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// What it took: settling (and, for `run`, showing) and the picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    ms: Option<serde_json::Value>,
}

impl Afterwards {
    fn focus(focus: find::After) -> Afterwards {
        Afterwards { focus, shown: None, settled: None, picture: None, windows: Vec::new(), tree: Vec::new(), error: None, ms: None }
    }
}

/// What is read of the screen while it settles: its windows, the focused
/// one's trees, and the two as one text to compare.
struct Seen {
    windows: Vec<WindowInfo>,
    trees: Vec<ScreenTree>,
    key: String,
    /// A window still to draw a size it was given.
    settling: bool,
    /// Something in it still coming (`UiNode::busy`: a page loading).
    busy: bool,
}

fn seen(s: &screens::Screen) -> Result<Seen, String> {
    let windows = s.windows()?;
    let trees = windows.iter().find(|w| w.focused).and_then(|w| trees(s, Some(w.id)).ok()).unwrap_or_default();
    let key = format!("{}{}", windows_key(&windows), outline(&trees, &Filter::default()));
    let settling = windows.iter().any(|w| w.settling);
    let busy = trees.iter().any(|t| t.nodes.iter().any(|n| n.busy));
    Ok(Seen { windows, trees, key, settling, busy })
}

/// The windows as a text that changes when one comes, goes, moves, is
/// retitled or takes the focus.
fn windows_key(windows: &[WindowInfo]) -> String {
    windows.iter().map(|w| format!("{} {:?} {} {} {:?}\n", w.id, w.title, w.focused, w.minimized, w.frame)).collect()
}

/// A window, as `after.windows` lists it.
fn window_line(w: &WindowInfo) -> String {
    let mut l = format!("{} {:?} ({})", w.id, w.title, w.app_id);
    for (on, word) in [(w.focused, "focused"), (w.minimized, "minimized")] {
        if on {
            l.push(' ');
            l.push_str(word);
        }
    }
    l
}

/// The screen once it has settled after an action (`Afterwards`); for
/// `run`, once its program has shown first (the windows or the focused
/// one's tree no longer as `shows_from`, `Seen::key`).
fn afterwards(s: &screens::Screen, quick: bool, shows_from: Option<&str>) -> Afterwards {
    if quick {
        return Afterwards::focus(find::after(s));
    }
    let began = std::time::Instant::now();
    let mut shown = None;
    if let Some(before) = shows_from {
        let mut changed = false;
        while !changed && began.elapsed() < SHOW_AT_MOST {
            std::thread::sleep(Duration::from_millis(100));
            changed = seen(s).is_ok_and(|now| now.key != before);
        }
        shown = Some(changed);
    }
    // The app takes it in over a frame or two; then read until nothing
    // has changed for `QUIET`, no window still to draw a size it was
    // given and no page still loading.
    let from = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(40));
    let mut last = seen(s);
    let mut changed = std::time::Instant::now();
    let mut settled = false;
    loop {
        let (busy, loading) = last.as_ref().map_or((false, false), |v| (v.settling || v.busy, v.busy));
        if last.is_ok() && !busy && changed.elapsed() >= QUIET {
            settled = true;
            break;
        }
        if from.elapsed() >= if loading { LOADING_AT_MOST } else { SETTLE_AT_MOST } {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        let now = seen(s);
        if !matches!((&last, &now), (Ok(a), Ok(b)) if a.key == b.key) {
            changed = std::time::Instant::now();
        }
        last = now;
    }
    let settling_ms = began.elapsed().as_millis() as u64;
    let taken = std::time::Instant::now();
    let picture = picture_path(&s.name, "after").and_then(|p| s.picture(&p, None, true, ImageFormat::Png).map(|()| p.display().to_string()));
    let picture_ms = taken.elapsed().as_millis() as u64;
    let mut a = match &last {
        Ok(seen) => {
            let mut a = Afterwards::focus(find::focus_of(&seen.windows, &seen.trees));
            a.windows = seen.windows.iter().map(window_line).collect();
            let filter = Filter { visible_only: true, max_len: Some(80), ..Default::default() };
            a.tree = outline(&seen.trees, &filter).lines().map(str::to_owned).collect();
            if a.tree.len() > AFTER_TREE_LINES {
                let more = a.tree.len() - AFTER_TREE_LINES;
                a.tree.truncate(AFTER_TREE_LINES);
                let id = seen.trees.first().map_or(0, |t| t.window);
                a.tree.push(format!("… {more} lines more: olo-screen tree {} --window {id} --visible-only", s.name));
            }
            a
        }
        Err(e) => {
            let mut a = Afterwards::focus(find::After { window: None, focused: None });
            a.error = Some(format!("the screen wasn't read: {e}"));
            a
        }
    };
    a.settled = Some(settled);
    a.shown = shown;
    match picture {
        Ok(p) => a.picture = Some(p),
        Err(e) => a.error = Some(a.error.map_or(String::new(), |x| format!("{x}; ")) + &format!("no picture: {e}")),
    }
    a.ms = Some(json!({ "settling": settling_ms, "picture": picture_ms }));
    a
}

/// Print, for an action that said nothing before, how the screen is
/// after it (`quick`: still nothing).
fn tell_after(s: &screens::Screen, quick: bool) -> Result<(), String> {
    if !quick {
        print_json(&json!({ "after": afterwards(s, false, None) }));
    }
    Ok(())
}

/// Where a picture of screen `name` goes when no file is given (`kind`:
/// "after" an action, a "look"): among its pictures, on disk beside its
/// history (the runtime folder is memory: looks piled up there), named by
/// when, the newest `PICTURES_KEPT` kept.
fn picture_path(name: &str, kind: &str) -> Result<PathBuf, String> {
    let dir = history::dir(name).join("pictures");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut kept: Vec<PathBuf> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect()).unwrap_or_default();
    if kept.len() >= PICTURES_KEPT {
        kept.sort();
        for p in &kept[..kept.len() + 1 - PICTURES_KEPT] {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(dir.join(format!("{}-{kind}.png", history::now_ms())))
}

/// Keep `v` as the command's answer, for its history.
fn answered(v: serde_json::Value) {
    ANSWER.with(|a| *a.borrow_mut() = Some(v));
}

/// What an action is told while the person controls screen `name`.
fn controlled_answer(name: &str) -> String {
    format!(
        "the person is controlling screen {name} (Agent Screens' Take Control): nothing was done. Wait until they hand it back (olo-screen wait {name} --free), then look at it again before going on: they may have changed what's on it."
    )
}

fn run_command(args: &[String]) -> Option<Result<(), String>> {
    let command = args.first()?.as_str();
    // `--quick` (anywhere after the name; for `run`, before the program),
    // or OLO_SCREEN_QUICK=1 for a run of them: an action answers with the
    // focus alone, at once, as before 2026-11-18, not with how the screen
    // is once settled (`afterwards`).
    let (rest_vec, quick) = strip_quick_flag(command, &args[1..]);
    let rest = &rest_vec[..];
    let name = || rest.first().cloned().ok_or_else(|| format!("{command} needs a screen's name"));
    // While the person has the screen, an agent's actions wait for them
    // (reading it still answers): two hands on one pointer made a mess.
    // Hold a shared `ActionLease` (`LOCK_SH`) across mutating input/window
    // commands so Take Control cannot interleave mid-drag.
    let _action_lease = if !matches!(command, "start" | "stop" | "take" | "give")
        && (history::ACTIONS.contains(&command) || command == "maximize")
        && let Some(n) = rest.first()
    {
        if screens::valid_name(n) {
            match screens::acquire_action_lease(n) {
                Ok(lease) => Some(lease),
                Err(_) => return Some(Err(controlled_answer(n))),
            }
        } else {
            None
        }
    } else {
        if command == "stop"
            && let Some(n) = rest.first()
            && screens::controlled(n)
        {
            return Some(Err(controlled_answer(n)));
        }
        None
    };
    let screen = || screens::get(&name()?);
    let input = |events: Vec<SyntheticEvent>| screen()?.input(events);
    Some(match command {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "watch" => {
            if rest.len() > 1 {
                return Some(Err("watch takes at most one [HOST:]NAME target".into()));
            }
            if let Some(target) = rest.first() {
                if target.starts_with('-') {
                    return Some(Err(format!("unknown watch option {target:?} (expected [HOST:]NAME)")));
                }
                if let Err(e) = validate_watch_target(target) {
                    return Some(Err(e));
                }
            }
            return None;
        }
        "start" => (|| {
            let given = rest.first().filter(|a| !a.starts_with("--")).cloned();
            if let Some(ref g) = given
                && !screens::valid_name(g)
            {
                return Err(format!("{g:?} can't name a screen (letters, digits, - and _)"));
            }
            // A stopped screen starts again as it was, unless told otherwise.
            let mut opts = given.as_deref().and_then(screens::kept).unwrap_or_default();
            if let Some(size) = option(rest, "--size") {
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                opts.width = w.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
                opts.height = h.parse().map_err(|_| "--size is WIDTHxHEIGHT")?;
            }
            if let Some(scale) = option(rest, "--scale") {
                opts.scale = scale.parse().map_err(|_| "--scale is a number")?;
            }
            opts.sound = opts.sound || has(rest, "--sound");
            let screen = screens::start(&given.unwrap_or_else(screens::free_name), &opts)?;
            print_json(&json!(screen));
            Ok(())
        })(),
        "list" => list_screens_cli(rest),
        "rename" => name().and_then(|n| history::set_title(&n, rest.get(1).map(String::as_str))),
        "stop" => (|| {
            let n = name()?;
            let (_, expected_started, expected_last_used) = screens::parse_expected_flags_strict(&rest[1..], false)?;
            screens::stop_if_expected(&n, expected_started, expected_last_used)
        })(),
        "archive" => name().and_then(|n| screens::archive(&n, true)),
        "unarchive" => name().and_then(|n| screens::archive(&n, false)),
        "delete" => (|| {
            let n = name()?;
            let (_, expected_started, expected_last_used) = screens::parse_expected_flags_strict(&rest[1..], false)?;
            if expected_started.is_some() || expected_last_used.is_some() {
                screens::delete_if_stopped_expected(&n, expected_started, expected_last_used)
            } else {
                screens::delete(&n)
            }
        })(),
        "env" => screen().map(|s| {
            for (k, v) in s.env() {
                println!("export {k}={}", shell_quote(&v));
            }
        }),
        "take" => (|| {
            let ids = window_ids(rest)?;
            if ids.is_empty() {
                return Err("take needs the windows to move (--window ID, as the person's display lists them)".into());
            }
            let person = moving::Place::person()?;
            let s = if has(rest, "--new") {
                let opts = screens::Options::default();
                screens::start(&screens::free_name(), &opts)?
            } else {
                screen()?
            };
            let outcome = moving::move_windows(&person, &ids, &moving::Place::screen(&s))?;
            let v = take_result_json(&s, &outcome);
            print_json(&v);
            if outcome.went.is_empty() { Err(stayed_why(&outcome)) } else { Ok(()) }
        })(),
        "give" => screen().and_then(|s| {
            let workspace = option(rest, "--workspace").map(|n| n.parse::<u32>().ok().filter(|n| *n >= 1).ok_or("--workspace is a number from 1")).transpose()?;
            let outcome = moving::give(&s, Some(window_ids(rest)?), workspace.map(|n| n - 1), option(rest, "--display").cloned())?;
            let v = json!({ "went": outcome.went, "stayed": outcome.stayed });
            print_json(&v);
            if outcome.went.is_empty() { Err(stayed_why(&outcome)) } else { Ok(()) }
        }),
        "run" => screen().and_then(|s| {
            // (What it shows is waited for: a window that opens, or the
            // focused one changing, as Chrome's given a page in a tab.)
            let before = (!quick).then(|| seen(&s).map(|v| v.key).unwrap_or_default());
            // (Past run's own options, as its history counts it.)
            let pid = s.run(history::run_argv(&rest[1..]))?;
            let mut v = json!({ "pid": pid });
            if let Some(before) = before {
                v["after"] = json!(afterwards(&s, false, Some(&before)));
            }
            print_json(&v);
            Ok(())
        }),
        // (Started with each screen: its frames for its history.)
        "record" => name().and_then(|n| record::run(&n)),
        // A video of the screen (olo-screenshot's recorder, on the screen's
        // display): started, its path said at once; `stop-video` finishes it.
        "record-video" => screen().and_then(|s| {
            let to = match option(rest, "--to") {
                Some(p) => std::path::PathBuf::from(p),
                None => video_path(&s.name),
            };
            let mut argv: Vec<String> = vec!["olo-screenshot".into(), "--record".into(), "--to".into(), to.display().to_string()];
            // (A window named without its id is an error, not the screen
            // recorded: Astra's review, 2026-11-28.)
            if let Some(id) = window_arg(rest)? {
                argv.extend(["--window".to_owned(), id.to_string()]);
            }
            for flag in ["--seconds", "--fps", "--encoder", "--codec", "--region", "--pointer-size", "--camera"] {
                if let Some(v) = option(rest, flag) {
                    argv.extend([flag.to_owned(), v.to_owned()]);
                }
            }
            for flag in ["--no-pointer", "--show-clicks", "--show-keys", "--as-gif", "--background", "--smooth-pointer", "--zoom-follow"] {
                if has(rest, flag) {
                    argv.push(flag.into());
                }
            }
            if let Some(v) = option(rest, "--sound") {
                argv.extend(["--sound".to_owned(), v.to_owned()]);
            }
            let pid = s.run(&argv)?;
            // Said once it's recording (its encoder open: NVENC takes a
            // couple of seconds), so what's done next is in it.
            let display = std::path::Path::new(&s.wayland_display).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                let g = olo_protocol::recording::going_on(&display).filter(|g| g.pid == pid);
                if g.as_ref().is_some_and(|g| g.recording) {
                    break;
                }
                if g.is_none() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    return Err("the recording didn't start (olo-screenshot's log on the screen says why)".into());
                }
                if std::time::Instant::now() > until {
                    return Err("the recording hasn't started after 15 seconds".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let v = json!({ "pid": pid, "path": to, "recording": true });
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            answered(v);
            Ok(())
        }),
        // (The recorder's --stop, --pause and --resume, on the screen's
        // display.)
        "stop-video" | "pause-video" | "resume-video" | "zoom-video" | "caption-video" | "mark-video" => screen().and_then(|s| {
            let program = olo_xdg::find_program("olo-screenshot").ok_or("olo-screenshot isn't installed")?;
            let mut cmd = std::process::Command::new(program);
            let flag = match command {
                "pause-video" => "--pause",
                "resume-video" => "--resume",
                "zoom-video" => "--zoom-to",
                "caption-video" => "--caption",
                "mark-video" => "--mark",
                _ => "--stop",
            };
            cmd.arg(flag).env_remove("DISPLAY");
            if command == "zoom-video" {
                cmd.arg(positionals(rest).get(1).ok_or("zoom-video NAME X,Y,W,H|out")?);
            }
            if command == "mark-video" {
                cmd.arg(positionals(rest).get(1).cloned().unwrap_or_default());
            }
            if command == "caption-video" {
                // (--clear: the caption taken down.)
                let text = if has(rest, "--clear") { String::new() } else { positionals(rest).get(1).cloned().ok_or("caption-video NAME TEXT [--seconds N] | --clear")? };
                cmd.arg(text);
                if let Some(v) = option(rest, "--seconds") {
                    cmd.args(["--seconds", v]);
                }
            }
            for (k, v) in s.env() {
                cmd.env(k, v);
            }
            let out = cmd.output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("olo-screenshot: ").to_owned());
            }
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            answered(v);
            Ok(())
        }),
        // A picture out of a video (one recorded, to check it), as Viewer
        // shows it there.
        "video-frame" => (|| {
            let p = positionals(rest);
            let video = p.first().ok_or("video-frame VIDEO [OUT.png] [--at SECONDS]")?;
            let video = std::path::absolute(PathBuf::from(video)).map_err(|e| e.to_string())?;
            let at: f64 = option(rest, "--at").map_or(Ok(0.0), |v| v.parse()).map_err(|_| "--at takes seconds")?;
            let out = match p.get(1) {
                Some(o) => std::path::absolute(PathBuf::from(o)).map_err(|e| e.to_string())?,
                None => {
                    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    video.with_file_name(format!("{stem} at {at} s.png"))
                }
            };
            let image = olo_preview::video::picture_at(&olo_preview::input::Input::file(&video), at)?;
            let [w, h] = image.size;
            let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().and_then(|mut wr| wr.write_image_data(&image.rgba)).map_err(|e| e.to_string())?;
            print_json(&json!({ "path": out, "at": at, "width": w, "height": h }));
            Ok(())
        })(),
        "history" => (|| {
            let n = name()?;
            if !screens::valid_name(&n) {
                return Err(format!("{n:?} can't name a screen (letters, digits, - and _)"));
            }
            // Scope all history flags strictly to arguments AFTER the screen name.
            let flags = &rest[1..];
            let has_expected_flag = flags.iter().any(|f| f == "--expected-started" || f == "--expected-last-used" || f.starts_with("--expected-"));
            if has(flags, "--clear") || has_expected_flag {
                let (saw_clear, expected_started, expected_last_used) = screens::parse_expected_flags_strict(flags, true)?;
                if !saw_clear {
                    return Err("--expected-started and --expected-last-used are only valid with `history NAME --clear`".into());
                }
                return screens::clear_history_if_stopped_expected(&n, expected_started, expected_last_used);
            }
            // The screen as it was when action N (from 1, as listed)
            // ended, as a picture.
            if let Some(idx_s) = option(flags, "--look") {
                let idx: usize = idx_s.parse().map_err(|_| "--look is an action's number, from 1")?;
                let all = history::read(&n);
                let a = all.get(idx.wrapping_sub(1)).ok_or_else(|| format!("there are {} actions", all.len()))?;
                let (when, frame) = record::frame_at(&record::index(&n), a.at.saturating_add(a.ms)).ok_or("no frames were kept from then")?;
                let out = match flags.iter().skip_while(|a| *a != "--look").nth(2).filter(|a| !a.starts_with("--")) {
                    Some(o) => std::path::absolute(PathBuf::from(o)).map_err(|e| e.to_string())?,
                    None => screens::dir().join(format!("{n}-history-{idx}.png")),
                };
                export::save_shot(&frame, 1, &out)?;
                print_json(&json!({ "path": out, "action": a, "frame_at": when }));
                return Ok(());
            }
            // As a video: an MP4, quiet stretches cut.
            if let Some(out) = option(flags, "--export") {
                let out = std::path::absolute(PathBuf::from(out)).map_err(|e| e.to_string())?;
                let mut shown = -1i32;
                // Actions by 0-based `--part SPEC` (e.g. `2-4,8`), or 1-based
                // `--from`/`--to`, or all.
                let at = |flag| option(flags, flag).map(|v| v.parse::<usize>().map_err(|_| format!("{flag} is an action's number, from 1"))).transpose();
                let (from, to) = (at("--from")?, at("--to")?);
                let count = history::read(&n).len();
                let part: Option<Vec<usize>> = if let Some(spec) = option(flags, "--part") {
                    Some(export::parse_part_spec(spec, count)?)
                } else if from.is_some() || to.is_some() {
                    Some((from.unwrap_or(1).max(1) - 1..to.unwrap_or(count).min(count)).collect())
                } else {
                    None
                };
                let zoom = option(flags, "--zoom").map(|z| z.trim_end_matches(['x', '×']).parse::<usize>().map_err(|_| "--zoom is 1, 2 or 3")).transpose()?.unwrap_or(1);
                let encoder = option(flags, "--encoder").map(|e| olo_video::Choice::parse(e).ok_or("--encoder is auto, cpu, nvidia or vaapi")).transpose()?.unwrap_or(olo_video::Choice::Auto);
                let made = export::export(&n, part.as_deref(), zoom, encoder, &out, &mut |f| {
                    let pct = (f * 100.0) as i32;
                    if pct / 10 != shown / 10 {
                        eprintln!("olo-screen: exporting… {pct}%");
                        shown = pct;
                    }
                    true
                })?;
                print_json(
                    &json!({ "path": out, "seconds": made.frames as f64 / 10.0, "frames": made.frames, "pictures_encoded": made.encoded, "encoder": made.encoder, "size": [made.size.0, made.size.1] }),
                );
                return Ok(());
            }
            let kind = option(flags, "--kind");
            let search = option(flags, "--search");
            let mut actions: Vec<history::Action> = history::read(&n).into_iter().filter(|a| kind.is_none_or(|k| &a.kind == k) && search.is_none_or(|t| a.mentions(t))).collect();
            if let Some(last) = option(flags, "--last") {
                let last: usize = last.parse().map_err(|_| "--last is a number")?;
                actions.drain(..actions.len().saturating_sub(last));
            }
            if has(flags, "--json") {
                print_json(&json!(actions));
            } else if actions.is_empty() {
                println!("Nothing done on {n} yet.");
            } else {
                for a in &actions {
                    println!("{}", a.line());
                }
            }
            Ok(())
        })(),
        "stats" => {
            let running = screens::list();
            let mut names = history::names();
            for s in &running {
                if !names.contains(&s.name) {
                    names.push(s.name.clone());
                }
            }
            if let Some(n) = rest.first().filter(|a| !a.starts_with("--")) {
                names.retain(|x| x == n);
                if names.is_empty() {
                    names.push(n.clone());
                }
            }
            let now = history::now_ms();
            let all: Vec<serde_json::Value> = names
                .iter()
                .map(|n| {
                    let up = running.iter().find(|s| &s.name == n);
                    let st = history::stats(n, &history::read(n), up.is_some(), now);
                    let windows = up.and_then(|s| s.windows().ok()).map(|w| w.len());
                    let mut v = json!(st);
                    v["running"] = json!(up.is_some());
                    v["windows"] = json!(windows);
                    v
                })
                .collect();
            if has(rest, "--json") {
                print_json(&json!(all));
            } else {
                for v in &all {
                    println!("{}", stats_text(v));
                }
            }
            Ok(())
        }
        "look" => screen().and_then(|s| {
            // Where to: as given, or a new file beside the screen's logs.
            let out = match positionals(rest).get(1) {
                Some(out) => std::path::absolute(PathBuf::from(out)).map_err(|e| e.to_string())?,
                None => picture_path(&s.name, "look")?,
            };
            let window = window_arg(rest)?;
            let pointer = !has(rest, "--no-pointer");
            match option(rest, "--crop") {
                None => s.picture(&out, window, pointer, ImageFormat::Png)?,
                Some(crop) => {
                    let n: Vec<f64> = crop.split(',').map(|v| v.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| "--crop is X,Y,W,H")?;
                    let [x, y, w, h] = n[..] else { return Err("--crop is X,Y,W,H".into()) };
                    crop_picture(&s, &out, window, pointer, (x, y, w, h))?;
                }
            }
            // (The path alone, or with --json as JSON, `{"path": …}`, as
            // the commands that answer JSON have it: an agent took one for
            // the other, 2026-11-24.)
            if has(rest, "--json") {
                println!("{}", json!({ "path": out.display().to_string() }));
            } else {
                println!("{}", out.display());
            }
            Ok(())
        }),
        "windows" => screen().and_then(|s| {
            println!("{}", serde_json::to_string_pretty(&json!(s.windows()?)).unwrap_or_default());
            Ok(())
        }),
        "tree" => screen().and_then(|s| {
            let only = window_arg(rest)?;
            let mut trees = trees(&s, only)?;
            let filter = Filter::from_args(rest)?;
            if has(rest, "--json") {
                for t in &mut trees {
                    let c = t.content;
                    let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
                    t.nodes.retain(|n| {
                        let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
                        depth.insert(n.id, d);
                        filter.keeps(n, d, c, &t.popups)
                    });
                }
                println!("{}", serde_json::to_string_pretty(&trees).unwrap_or_default());
            } else {
                print!("{}", outline(&trees, &filter));
            }
            Ok(())
        }),
        "menus" => screen().and_then(|s| {
            let named = window_arg(rest)?;
            let w = app_window(&s, named)?;
            let menus = menus_of(&w, named.is_some())?.ok_or_else(|| format!("{} didn't give its menus", w.app_id))?;
            println!("{}", serde_json::to_string_pretty(&menus).unwrap_or_default());
            Ok(())
        }),
        "invoke" => screen().and_then(|s| {
            // (Asked to press a control by its name, as `click` does: said
            // what each is for. "invoke needs a menu command" alone was
            // read ten times without the agent seeing why.)
            if ["--name", "--role", "--match", "--text"].iter().any(|f| has(rest, f)) {
                return Err("invoke chooses one of the app's menu commands, by its command (`menus` lists them: `invoke NAME save`); to press a button or another control by its name, use `click NAME --role Button --name …`".into());
            }
            let command = positionals(rest).get(1).cloned().ok_or("invoke needs a menu command (`menus` lists them: `invoke NAME save`)")?;
            let target = window_arg(rest)?;
            let w = app_window(&s, target)?;
            if invoke_in(&w, target.is_some(), &command)? {
                if !quick {
                    print_json(&json!({ "invoked": command, "after": afterwards(&s, false, None) }));
                }
                return Ok(());
            }
            // Why not, and what there is like it.
            let entries = menus_of(&w, target.is_some())?.map(|m| m.entries).unwrap_or_default();
            // Its label (or its command but for case), the one item so
            // named: that item, chosen. (Agents write what the menu says.)
            let named: Vec<&commands::Entry> = entries.iter().filter(|e| e.command != command && (find::plain(&e.label) == find::plain(&command) || find::plain(&e.command) == find::plain(&command))).collect();
            if let [e] = named[..]
                && e.enabled
                && invoke_in(&w, target.is_some(), &e.command)?
            {
                let mut v = json!({ "invoked": e.command, "label": e.label });
                if !quick {
                    v["after"] = json!(afterwards(&s, false, None));
                }
                print_json(&v);
                return Ok(());
            }
            if let Some(e) = entries.iter().find(|e| e.command == command).or(named.first().copied()) {
                return Err(format!("{} ▸ {} ({:?}) is off now: it can't be chosen as things are", e.path.join(" ▸ "), e.label, e.command));
            }
            let mut like: Vec<&commands::Entry> = entries.iter().filter(|e| e.enabled).collect();
            like.sort_by_key(|e| std::cmp::Reverse(find::likeness(&command, &e.label).max(find::likeness(&command, &e.command))));
            let shown: Vec<String> = like.iter().take(8).map(|e| format!("{:?} ({})", e.command, e.label)).collect();
            Err(format!("{} has no command {command:?}{}", w.app_id, if shown.is_empty() { " (it says no menus)".into() } else { format!("; the nearest it has: {} (`menus` has them all)", shown.join(", ")) }))
        }),
        "review" => screen().and_then(|s| {
            let only = window_arg(rest)?;
            // (Every window, a minimized one too, to tell a review's
            // window by: left out, an older app's review of its minimized
            // notes.txt was taken for the one showing; Astra's review,
            // 2026-11-28. What's shown, of those that show.)
            let every: Vec<WindowInfo> = s.windows()?;
            let all: Vec<&WindowInfo> = every.iter().filter(|w| w.content.is_some()).collect();
            let windows: Vec<&WindowInfo> = all.iter().copied().filter(|w| only.is_none_or(|id| w.id == id)).collect();
            if let Some(id) = only.filter(|_| windows.is_empty()) {
                return Err(format!("no window {id}"));
            }
            // One question per app, all at once.
            let mut pids: Vec<u32> = windows.iter().filter_map(|w| w.pid.map(|p| p as u32)).collect();
            pids.sort_unstable();
            pids.dedup();
            let asked: Vec<_> = pids.into_iter().map(|pid| (pid, std::thread::spawn(move || commands::review(pid)))).collect();
            // Each app's reviews of the windows asked about: an app's
            // other windows left out, one of the same title too; all of
            // its windows' when none was named.
            let mut reviews: Vec<commands::WindowReview> = Vec::new();
            for (pid, r) in asked {
                let answer = r.join().ok().flatten().unwrap_or_default();
                let siblings: Vec<&WindowInfo> = every.iter().filter(|w| w.pid == Some(pid as i32)).collect();
                match only {
                    Some(id) => {
                        if let Some(w) = siblings.iter().find(|w| w.id == id && w.content.is_some()) {
                            reviews.extend(review_of(w, answer, &siblings)?);
                        }
                    }
                    None => reviews.extend(answer.into_iter().filter(|r| r.layer.is_none() && all.iter().any(|w| w.pid == Some(pid as i32) && reviews_window(r, w)))),
                }
            }
            if has(rest, "--json") {
                println!("{}", serde_json::to_string_pretty(&reviews).unwrap_or_default());
            } else if reviews.is_empty() {
                println!("no Olo windows to review");
            } else {
                for r in &reviews {
                    println!("{} ({}×{} at {}×): {} findings", r.title, r.width, r.height, r.scale, r.findings.len());
                    for f in &r.findings {
                        println!("  - {f}");
                    }
                }
            }
            Ok(())
        }),
        "click" => (|| {
            let s = screen()?;
            let (x, y, matched) = match find::Query::from_args(rest)? {
                Some(q) => {
                    let f = find::find(&s, &q)?;
                    // (With no place to click, what a click on it does,
                    // invoked: Chrome's tabs, whose strip Chrome draws.)
                    if let Some(command) = f.command.clone().filter(|_| f.width < 1.0 || f.height < 1.0) {
                        let w = s.windows()?.into_iter().find(|w| w.id == f.window).ok_or_else(|| format!("its window {} has gone", f.window))?;
                        // (The command names its own target, a Chrome
                        // tab's: asked of the app as a whole; but where a
                        // window was named, only of that window, declined
                        // where it can't be told (a tab moved to another
                        // window took the click there: Astra's review,
                        // 2026-11-28).)
                        if !invoke_in(&w, q.window.is_some(), &command)? {
                            return Err(format!("the {} {:?} has no place to click, and {} didn't do {command:?}", f.role, f.name.as_deref().unwrap_or(""), w.app_id));
                        }
                        print_json(&json!({ "invoked": command, "matched": f, "after": afterwards(&s, quick, None) }));
                        return Ok(());
                    }
                    let f = into_view(&s, &q, f)?;
                    (f.center.0 as f64, f.center.1 as f64, Some(f))
                }
                None => {
                    let (x, y) = point(rest, "click")?;
                    (x, y, None)
                }
            };
            s.input(screens::click(x, y, button(rest), if has(rest, "--double") { 2 } else { 1 }))?;
            print_json(&json!({ "clicked": [x, y], "matched": matched, "after": afterwards(&s, quick, None) }));
            Ok(())
        })(),
        "move" => (|| {
            let (x, y) = point(rest, "move")?;
            input(vec![SyntheticEvent::Move { x, y }])?;
            tell_after(&screen()?, quick)
        })(),
        "down" => (|| {
            let (x, y) = point(rest, "down")?;
            input(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button: button(rest), pressed: true }])?;
            tell_after(&screen()?, quick)
        })(),
        "up" => (|| {
            input(vec![SyntheticEvent::Button { button: button(rest), pressed: false }])?;
            tell_after(&screen()?, quick)
        })(),
        "drag" => (|| {
            let n = |i: usize, w: &str| number(rest.get(i), w);
            let steps = option(rest, "--steps").map(|s| s.parse::<u32>().map_err(|_| "--steps is a number")).transpose()?.unwrap_or(20);
            let ms = option(rest, "--duration").map(|s| s.parse::<u64>().map_err(|_| "--duration is milliseconds")).transpose()?.unwrap_or(300);
            let s = screen()?;
            s.input_paced(screens::drag((n(1, "x1")?, n(2, "y1")?), (n(3, "x2")?, n(4, "y2")?), steps, Duration::from_millis(ms)))?;
            tell_after(&s, quick)
        })(),
        "scroll" => (|| {
            // DX DY, or --dx/--dy (either alone), and --at X,Y for where.
            let by = |flag: &str| option(rest, flag).map(|v| v.parse::<f64>().map_err(|_| format!("{flag} is a number of pixels"))).transpose();
            let (dx, dy) = match (by("--dx")?, by("--dy")?) {
                (None, None) => {
                    let p = positionals(rest);
                    let n = |i: usize| p.get(i).and_then(|a| a.parse::<f64>().ok());
                    match (n(1), n(2)) {
                        (Some(dx), Some(dy)) => (dx, dy),
                        _ => return Err("scroll takes DX DY in pixels (positive: right and down), or --dx N and --dy N; --at X,Y for where".into()),
                    }
                }
                (dx, dy) => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
            };
            let s = screen()?;
            if let Some(a) = option(rest, "--at") {
                let (x, y) = pair(a).ok_or("--at is X,Y")?;
                s.input(vec![SyntheticEvent::Move { x, y }])?;
            }
            s.input_paced(screens::scroll(dx, dy, !has(rest, "--no-end")))?;
            tell_after(&s, quick)
        })(),
        "wheel" => (|| {
            let dy = number(rest.get(1), "notches")? as i32;
            let dx = rest.get(2).filter(|a| !a.starts_with("--")).map(|_| number(rest.get(2), "horizontal notches")).transpose()?.unwrap_or(0.0) as i32;
            let s = screen()?;
            s.input_paced(screens::wheel(dx, dy))?;
            tell_after(&s, quick)
        })(),
        "key" => (|| {
            let chords = rest.get(1..).filter(|c| !c.is_empty()).ok_or("key needs a chord, such as Cmd+S")?;
            let mut events = Vec::new();
            for c in chords {
                events.extend(screens::chord(&screens::parse_chord(c)?));
            }
            let s = screen()?;
            s.input(events)?;
            print_json(&json!({ "after": afterwards(&s, quick, None) }));
            Ok(())
        })(),
        "type" => (|| {
            let s = screen()?;
            s.type_text(&rest.get(1..).filter(|t| !t.is_empty()).ok_or("type needs text")?.join(" "))?;
            let after = afterwards(&s, quick, None);
            warn_if_not_text(&after.focus);
            print_json(&json!({ "after": after }));
            Ok(())
        })(),
        "set" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("set needs the field: --name, --role or --match")?;
            let text = positionals(rest).get(1).cloned().ok_or("set needs the text to put in the field")?;
            let f = into_view(&s, &q, find::find(&s, &q)?)?;
            // Into the field (once more if the first click went astray:
            // the keys below would edit whatever has the focus).
            let into = |s: &screens::Screen| find::after(s).focused.is_some_and(|now| find::focus_in(&now, &f));
            let mut focused = false;
            for _ in 0..2 {
                s.input(screens::click(f.center.0 as f64, f.center.1 as f64, screens::LEFT, 1))?;
                focused = into(&s);
                if focused {
                    break;
                }
            }
            if !focused && find::takes_text(&f.role) {
                return Err(format!("clicked the {} {:?} twice but it didn't take the focus; nothing typed", f.role, f.name.as_deref().unwrap_or("")));
            }
            let mut events = screens::chord(&screens::parse_chord("Ctrl+A")?);
            events.extend(screens::chord(&screens::parse_chord("Delete")?));
            s.input(events)?;
            s.type_text(&text)?;
            let after = afterwards(&s, quick, None);
            warn_if_not_text(&after.focus);
            // (Read back: a page's own field can drop what is typed into it
            // and `set` said it was set, a sign-in's code field among them:
            // an agent's report, 2026-11-24. Once more as `type` does, a
            // click and the keys alone, which worked there; then said.)
            if let Some(why) = set_not_kept(&f, &text, after.focus.focused.as_ref()) {
                s.input(screens::click(f.center.0 as f64, f.center.1 as f64, screens::LEFT, 1))?;
                s.type_text(&text)?;
                let again = afterwards(&s, quick, None);
                if set_not_kept(&f, &text, again.focus.focused.as_ref()).is_some() {
                    return Err(why);
                }
                print_json(&json!({ "field": f, "after": again, "typed_again": true }));
                return Ok(());
            }
            print_json(&json!({ "field": f, "after": after }));
            Ok(())
        })(),
        "select" => (|| {
            let s = screen()?;
            let q = find::Query::from_args(rest)?.ok_or("select needs the dropdown: --name, --role or --match")?;
            let item = positionals(rest).get(1).cloned().ok_or("select needs the item to choose")?;
            let field = into_view(&s, &q, find::find(&s, &q)?)?;
            // A dropdown that says what it offers and draws its menu
            // itself (a web page's <select>: the browser's menu, whose
            // items are no things of their own on the screen): chosen by
            // the keyboard, by its words, and checked. (Waited for as
            // menu items, they never came: "its menu has no item (or
            // didn't open)", 31 times in the agents' histories, all in
            // Chrome.)
            if !field.options.is_empty() {
                let want = find::option_meant(&field.options, &item).map_err(|e| format!("{} {:?}: {e}", field.role, field.name.as_deref().unwrap_or("")))?.clone();
                if field.value.as_deref().map(find::plain) != Some(find::plain(&want)) {
                    // (Its words typed only where they reach it: the
                    // select, or its list, opened. Where the click left the
                    // keyboard elsewhere, they went to a search box, the
                    // page's "/" shortcut, another select, which they
                    // changed: an agent's report, 2026-11-24. Clicked once
                    // more, then said, nothing typed.)
                    let mut reach = false;
                    for _ in 0..2 {
                        s.input(screens::click(field.center.0 as f64, field.center.1 as f64, screens::LEFT, 1))?;
                        std::thread::sleep(Duration::from_millis(250));
                        if keys_reach(&field, find::after(&s).focused.as_ref()) {
                            reach = true;
                            break;
                        }
                        s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    }
                    if !reach {
                        let now = find::after(&s).focused.map(|n| format!("the {} {:?}", n.role, n.name.unwrap_or_default())).unwrap_or_else(|| "something else".into());
                        return Err(format!("clicked the {} {:?} twice but {now} kept the keyboard; nothing typed", field.role, field.name.as_deref().unwrap_or("")));
                    }
                    s.type_text(&want)?;
                    std::thread::sleep(Duration::from_millis(150));
                    s.input(screens::chord(&screens::parse_chord("Enter")?))?;
                }
                let after = afterwards(&s, quick, None);
                let now = find::find(&s, &find::Query { index: 0, ..q.clone() }).ok().filter(|n| n.window == field.window);
                let says = now.as_ref().and_then(|n| n.value.clone()).unwrap_or_default();
                if find::plain(&says) != find::plain(&want) {
                    s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    return Err(format!("chose {want:?}, but the {} says {says:?}", field.role));
                }
                print_json(&json!({ "field": field, "chose": want, "after": after }));
                return Ok(());
            }
            // (What shows of a menu's kind before it opens: a menu bar's
            // own, a page's lists. More than that is its menu, open.)
            const ITEMS: [&str; 3] = ["MenuItem", "MenuItemRadio", "ListBoxOption"];
            let items_in = |trees: &[ScreenTree]| -> Vec<String> {
                ITEMS.iter().flat_map(|role| find::matches(trees, &find::Query { role: Some((*role).into()), ..Default::default() })).filter_map(|f| f.name).collect()
            };
            let before = items_in(&trees(&s, None)?).len();
            s.input(screens::click(field.center.0 as f64, field.center.1 as f64, screens::LEFT, 1))?;
            // Its menu opens in a frame or two; then choose the item: a
            // menu's item, or (a page's own dropdown) an option of a list.
            // A menu that has opened without the item says so at once; one
            // that doesn't open is given three seconds.
            let until = std::time::Instant::now() + Duration::from_secs(3);
            let mut open_for = 0;
            let chosen = loop {
                let trees = trees(&s, None)?;
                let item_of = |role: &str| find::matches(&trees, &find::Query { name: Some(item.clone()), role: Some(role.into()), ..Default::default() }).into_iter().next();
                if let Some(f) = ["MenuItem", "MenuItemRadio", "MenuItemCheckBox", "ListBoxOption", "ListItem"].into_iter().find_map(item_of) {
                    break f;
                }
                let mut shown: Vec<String> = items_in(&trees).into_iter().map(|n| format!("{n:?}")).collect();
                open_for = if shown.len() > before { open_for + 1 } else { 0 };
                if open_for >= 3 || std::time::Instant::now() >= until {
                    // What its menu does show, to choose from.
                    shown.dedup();
                    shown.truncate(20);
                    s.input(screens::chord(&screens::parse_chord("Esc")?))?;
                    return Err(if shown.is_empty() { format!("its menu didn't open (no item {item:?} showed)") } else { format!("its menu has no item {item:?}; it has: {}", shown.join(", ")) });
                }
                std::thread::sleep(Duration::from_millis(120));
            };
            s.input(screens::click(chosen.center.0 as f64, chosen.center.1 as f64, screens::LEFT, 1))?;
            print_json(&json!({ "field": field, "chose": chosen, "after": afterwards(&s, quick, None) }));
            Ok(())
        })(),
        "wait" => (|| {
            let s = screen()?;
            // (Until the person hands it back, by default as long as
            // they like.)
            if has(rest, "--free") {
                let ms = option(rest, "--timeout").map(|t| t.parse::<u64>().map_err(|_| "--timeout is milliseconds")).transpose()?;
                let until = ms.map(|ms| std::time::Instant::now() + Duration::from_millis(ms));
                while screens::controlled(&s.name) {
                    if until.is_some_and(|u| std::time::Instant::now() >= u) {
                        return Err(format!("the person is still controlling screen {}", s.name));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                print_json(&json!({ "free": true }));
                return Ok(());
            }
            let ms = option(rest, "--timeout").map(|t| t.parse::<u64>().map_err(|_| "--timeout is milliseconds")).transpose()?.unwrap_or(10_000);
            let timeout = Duration::from_millis(ms);
            if has(rest, "--idle") {
                find::wait_idle(&s, window_arg(rest)?, timeout)?;
            }
            if let Some(q) = find::Query::from_args(rest)? {
                let found = find::wait_for(&s, &q, has(rest, "--gone"), timeout)?;
                print_json(&json!({ "found": found }));
            } else if !has(rest, "--idle") {
                return Err("wait for what? --idle, --free, --name, --role, --match or --text (with --gone, until it goes)".into());
            }
            Ok(())
        })(),
        "scroll-to" => (|| {
            let s = screen()?;
            // (The words alone, as `scroll-to NAME "Pricing"`: what's
            // looked for, anywhere in a name or a value. Seven of eight
            // refusals in the agents' histories were that.)
            let q = match find::Query::from_args(rest)? {
                Some(q) => q,
                None => match positionals(rest).get(1) {
                    Some(text) => find::Query { text: Some(text.clone()), window: window_arg(rest)?, ..Default::default() },
                    None => return Err("scroll-to needs what to find: the words, or --name, --role, --match or --text".into()),
                },
            };
            let at = option(rest, "--at")
                .map(|a| {
                    let (x, y) = a.split_once(',').ok_or("--at is X,Y")?;
                    Ok::<_, String>((x.trim().parse::<f64>().map_err(|_| "--at is X,Y")?, y.trim().parse::<f64>().map_err(|_| "--at is X,Y")?))
                })
                .transpose()?;
            let f = scroll_to(&s, &q, at)?;
            let mut v = json!({ "found": f });
            if !quick {
                v["after"] = json!(afterwards(&s, false, None));
            }
            print_json(&v);
            Ok(())
        })(),
        "focus" | "close" | "minimize" | "zoom" | "maximize" => (|| {
            let s = screen()?;
            let w = app_window(&s, window_arg(rest)?)?;
            let id = w.id;
            let request = match command {
                "focus" => Request::Activate { id },
                "close" => Request::Close { id },
                "minimize" => Request::Minimize { id },
                _ => Request::ToggleZoom { id },
            };
            ok(s.request(&request)?)?;
            print_json(&json!({ "window": id, "after": afterwards(&s, quick, None) }));
            Ok(())
        })(),
        "resize" | "place" => (|| {
            let s = screen()?;
            let w = app_window(&s, window_arg(rest)?)?;
            let (Some(frame), Some(content)) = (w.frame, w.content) else { return Err(format!("window {} isn't showing", w.id)) };
            let args = positionals(rest);
            let size = |a: Option<&String>| -> Result<(i32, i32), String> {
                let (w, h) = a.and_then(|a| a.split_once('x')).ok_or("the size is WIDTHxHEIGHT (of what the app draws)")?;
                Ok((w.parse().map_err(|_| "the size is WIDTHxHEIGHT")?, h.parse().map_err(|_| "the size is WIDTHxHEIGHT")?))
            };
            // Sizes are the content's (what the app draws); the frame adds the tab and border.
            let (extra_w, extra_h) = (frame.width - content.width, frame.height - content.height);
            let mut to = frame;
            if command == "resize" {
                let (cw, ch) = size(args.get(1))?;
                (to.width, to.height) = (cw + extra_w, ch + extra_h);
            } else {
                (to.x, to.y) = (number(args.get(1), "x")? as i32, number(args.get(2), "y")? as i32);
                if args.get(3).is_some() {
                    let (cw, ch) = size(args.get(3))?;
                    (to.width, to.height) = (cw + extra_w, ch + extra_h);
                }
            }
            ok(s.request(&Request::Place { id: w.id, frame: to })?)?;
            let after = if quick {
                std::thread::sleep(Duration::from_millis(150));
                None
            } else {
                Some(afterwards(&s, false, None))
            };
            let now = s.windows()?.into_iter().find(|x| x.id == w.id);
            let mut v = json!({ "window": now });
            if let Some(after) = after {
                v["after"] = json!(after);
            }
            print_json(&v);
            Ok(())
        })(),
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    })
}

/// The arguments that aren't flags (nor flags' values).
fn positionals(args: &[String]) -> Vec<String> {
    const VALUED: [&str; 17] = [
        "--at",
        "--name",
        "--role",
        "--match",
        "--text",
        "--index",
        "--window",
        "--timeout",
        "--steps",
        "--duration",
        "--crop",
        "--name-contains",
        "--contains",
        "--depth",
        "--max-len",
        "--dx",
        "--dy",
    ];
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if VALUED.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with("--") || a.parse::<f64>().is_ok() {
            out.push(a.clone());
        }
    }
    out
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    answered(v.clone());
}

/// A screen's stats (`history::Stats` and whether it runs) for people.
fn stats_text(v: &serde_json::Value) -> String {
    let ms = |k: &str| v[k].as_u64().unwrap_or(0);
    let dur = |ms: u64| {
        let s = ms / 1000;
        if s >= 3600 {
            format!("{} h {} min", s / 3600, s / 60 % 60)
        } else if s >= 60 {
            format!("{} min {} s", s / 60, s % 60)
        } else {
            format!("{s} s")
        }
    };
    let when = |k: &str| v[k].as_u64().map(history::clock).unwrap_or_else(|| "never".into());
    let kinds: Vec<String> = v["actions"].as_object().map(|o| o.iter().map(|(k, n)| format!("{k} {n}")).collect()).unwrap_or_default();
    let programs: Vec<String> = v["programs"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    let state = if v["running"].as_bool().unwrap_or(false) { format!("running, {} windows", v["windows"].as_u64().unwrap_or(0)) } else { "stopped".into() };
    format!(
        "{}: {state}\n  started {}, last used {}\n  ran {}, in use {}\n  actions: {}{}\n  programs: {}",
        v["name"].as_str().unwrap_or("?"),
        when("started"),
        when("last_used"),
        dur(ms("ran_ms")),
        dur(ms("active_ms")),
        if kinds.is_empty() { "none".into() } else { kinds.join(", ") },
        match v["failed"].as_u64() {
            Some(n) if n > 0 => format!(" ({n} failed)"),
            _ => String::new(),
        },
        if programs.is_empty() { "none".into() } else { programs.join(", ") },
    )
}

fn ok(reply: Reply) -> Result<(), String> {
    match reply {
        Reply::Error { message } => Err(message),
        _ => Ok(()),
    }
}

/// Typing that went somewhere that doesn't take text says so.
/// Whether keys typed now reach the dropdown `field` (a page's select): it
/// has the keyboard (itself, where it is), or its list, opened, does (an
/// option, a list, a menu). Anything else that has it would take them (a
/// search box, another select), and with nothing that has it, the page
/// does (its "/" shortcut: the page's whole document, which takes text,
/// lies over the select, so the same place alone isn't it).
fn keys_reach(field: &find::Found, now: Option<&find::Found>) -> bool {
    let Some(now) = now else { return false };
    let same_place = now.window == field.window && now.x < field.x + field.width && field.x < now.x + now.width && now.y < field.y + field.height && field.y < now.y + now.height;
    (same_place && now.role == field.role)
        || matches!(now.role.as_str(), "ListBoxOption" | "ListItem" | "List" | "ListBox" | "MenuItem" | "MenuItemRadio" | "Menu" | "MenuListOption" | "MenuListPopup")
}

/// Why `f` didn't keep `text` set into it, as the screen says after (`now`,
/// what has the keyboard): its value empty, or as it was before. None
/// where it did, or where that can't be told: no value said, a password's
/// (never shown), the keyboard moved on (a code's boxes, each taking one,
/// often of one name: so the same place, not the same name).
/// A value shown otherwise (a number grouped, a date formatted) is kept.
fn set_not_kept(f: &find::Found, text: &str, now: Option<&find::Found>) -> Option<String> {
    let now = now?;
    let same_place = now.window == f.window && now.x < f.x + f.width && f.x < now.x + now.width && now.y < f.y + f.height && f.y < now.y + now.height;
    if text.is_empty() || now.role == "PasswordInput" || !same_place {
        return None;
    }
    let value = now.value.as_deref()?;
    let before = f.value.as_deref().unwrap_or("");
    let dropped = value.trim().is_empty() || (value == before && before != text);
    dropped.then(|| {
        format!(
            "typed {text:?} into the {} {:?}, twice, but it holds {value:?}: the page didn't keep it (look at it, or try `click` then `key` for each character)",
            now.role,
            now.name.as_deref().unwrap_or("")
        )
    })
}

fn warn_if_not_text(after: &find::After) {
    match &after.focused {
        Some(f) if !find::takes_text(&f.role) => eprintln!("olo-screen: the keys went to a {} ({}), not a text field", f.role, f.name.as_deref().unwrap_or("unnamed")),
        None if after.window.is_none() => eprintln!("olo-screen: no window has the focus; the keys went nowhere"),
        _ => {}
    }
}

/// A picture of part of the screen: X,Y,W,H in the screen's logical pixels.
fn crop_picture(s: &screens::Screen, out: &std::path::Path, window: Option<u64>, pointer: bool, (x, y, w, h): (f64, f64, f64, f64)) -> Result<(), String> {
    let raw = screens::dir().join(format!("crop-{}.rgba", std::process::id()));
    s.picture(&raw, window, pointer, ImageFormat::Rgba)?;
    let bytes = std::fs::read(&raw).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&raw);
    let bytes = bytes?;
    let word = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let (Some(pw), Some(ph)) = (word(0), word(4)) else { return Err("the picture was cut short".into()) };
    let pixels = &bytes[8..];
    // A window's picture starts at the window; the screen's, at its corner.
    let origin = match window {
        Some(id) => s.windows()?.into_iter().find(|w| w.id == id).and_then(|w| w.frame).map_or((0.0, 0.0), |f| (f.x as f64, f.y as f64)),
        None => (0.0, 0.0),
    };
    let scale = if window.is_some() { s.scale } else { pw as f64 / s.width.max(1) as f64 };
    let px = |v: f64| (v * scale).round().max(0.0) as usize;
    let (x0, y0) = (px(x - origin.0).min(pw), px(y - origin.1).min(ph));
    let (x1, y1) = ((x0 + px(w)).min(pw), (y0 + px(h)).min(ph));
    if x1 <= x0 || y1 <= y0 {
        return Err("--crop is outside the picture".into());
    }
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for row in y0..y1 {
        data.extend_from_slice(pixels.get((row * pw + x0) * 4..(row * pw + x1) * 4).ok_or("the picture was cut short")?);
    }
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), (x1 - x0) as u32, (y1 - y0) as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&data)).map_err(|e| e.to_string())
}

/// Scroll the list under `at` (or the middle of the window) until `q`
/// shows whole: to the top first, then down a few notches at a time.
fn scroll_to(s: &screens::Screen, q: &find::Query, at: Option<(f64, f64)>) -> Result<find::Found, String> {
    let shown = |s: &screens::Screen| -> Result<(Option<find::Found>, String), String> {
        let trees = trees(s, q.window)?;
        Ok((find::matches(&trees, q).into_iter().nth(q.index), outline(&trees, &Filter::default())))
    };
    let whole = |f: &find::Found| f.role != "Row" || f.height >= 12.0;
    let first = shown(s)?.0;
    if let Some(f) = &first
        && whole(f)
        && in_view(s, f)?
    {
        return Ok(f.clone());
    }
    let (x, y) = match at {
        Some(p) => p,
        None => {
            let w = app_window(s, q.window)?;
            let c = w.content.ok_or("the window isn't showing")?;
            ((c.x + c.width / 2) as f64, (c.y + c.height / 2) as f64)
        }
    };
    s.input(vec![SyntheticEvent::Move { x, y }])?;
    // Straight to it, if it's there out of sight (a web page's tree has
    // the whole page); out from under what lies over it, if it's in sight
    // under a layer of its page's (a sticky header); else up to the top,
    // then down; until it shows or the list stops moving.
    let covered: [(i32, usize); 1];
    let plan: &[(i32, usize)] = match &first {
        Some(f) if f.covered.is_some() && in_sight(s, f)? => {
            covered = [(clear_of_cover(s, f)?, 12)];
            &covered
        }
        Some(f) if f.center.1 < view_of(s, f)?.1 => &[(-3, 1000)],
        Some(_) => &[(3, 1000)],
        None => &[(-10, 100), (3, 1000)],
    };
    let mut under = first.filter(|f| f.covered.is_some());
    for &(notches, rounds) in plan {
        let mut last = String::new();
        for _ in 0..rounds {
            s.input_paced(screens::wheel(0, notches))?;
            std::thread::sleep(Duration::from_millis(80));
            let (found, now) = shown(s)?;
            if let Some(f) = found {
                if whole(&f) && in_view(s, &f)? {
                    return settled(s, &shown, f);
                }
                under = f.covered.is_some().then_some(f);
            }
            if now == last {
                break;
            }
            last = now;
        }
    }
    // (In sight but under the page's own layer all along: a click would
    // reach that, and said it pressed what it never did, an agent's
    // report, 2026-11-24.)
    if let Some(f) = under {
        return Err(format!(
            "the {} {:?} is under the page's {}, and scrolling didn't bring it out: a click at its middle would reach that instead (close it, or scroll the page yourself)",
            f.role,
            f.name.as_deref().unwrap_or(""),
            f.covered.as_deref().unwrap_or("layer")
        ));
    }
    Err(format!("scrolled the whole list; nothing matches {}", q.describe()))
}

type ShownLookup<'a> = dyn Fn(&screens::Screen) -> Result<(Option<find::Found>, String), String> + 'a;

/// `f` once it stops moving: a browser scrolls smoothly, so what the
/// wheel brought into view goes on gliding for a moment, and a click at
/// where it was read mid-glide missed it (`set` on a web form). Read again
/// every 50 ms until two reads agree, a second at most.
fn settled(s: &screens::Screen, shown: &ShownLookup<'_>, mut f: find::Found) -> Result<find::Found, String> {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        match shown(s)?.0 {
            Some(now) if (now.x, now.y, now.width, now.height) == (f.x, f.y, f.width, f.height) => return Ok(now),
            Some(now) => f = now,
            None => break,
        }
    }
    Ok(f)
}

/// Where a click lands on what shows: the part of the screen `f`'s window
/// shows its content in (x, y, right, bottom), else the whole screen.
fn view_of(s: &screens::Screen, f: &find::Found) -> Result<(f32, f32, f32, f32), String> {
    let screen = (0.0, 0.0, s.width as f32, s.height as f32);
    let content = s.windows()?.into_iter().find(|w| w.id == f.window).and_then(|w| w.content);
    Ok(content.map_or(screen, |c| (c.x.max(0) as f32, c.y.max(0) as f32, ((c.x + c.width) as f32).min(screen.2), ((c.y + c.height) as f32).min(screen.3))))
}

/// Whether a click at `f`'s middle lands on it: within what its window
/// shows, on the screen. (A web page's tree has the whole page: a field
/// below the fold was at y 1045 of a 900-point screen, clicked where
/// nothing is, and `set` failed seven times on 2026-11-13.)
fn in_view(s: &screens::Screen, f: &find::Found) -> Result<bool, String> {
    Ok(in_sight(s, f)? && f.covered.is_none())
}

/// Whether `f`'s middle is in its view, whatever lies over it.
fn in_sight(s: &screens::Screen, f: &find::Found) -> Result<bool, String> {
    let (x, y) = f.center;
    let (left, top, right, bottom) = view_of(s, f)?;
    Ok(x >= left && x < right && y >= top && y < bottom)
}

/// Which way to wheel `f`, under a layer of its page's own, to bring it
/// out: a notch up where it is in the view's upper half (under a sticky
/// header, it comes out below it), down in the lower (a banner at the
/// foot). A notch at a time: a header is a notch or two tall.
fn clear_of_cover(s: &screens::Screen, f: &find::Found) -> Result<i32, String> {
    let (_, top, _, bottom) = view_of(s, f)?;
    Ok(wheel_out(f.center.1, top, bottom))
}

/// A notch up for a middle at `y` in the upper half of a view from `top`
/// to `bottom`, else down: the way out from under what covers it.
fn wheel_out(y: f32, top: f32, bottom: f32) -> i32 {
    if y < (top + bottom) / 2.0 { -1 } else { 1 }
}

/// `f`, scrolled into view first if it's out of sight (`scroll_to`).
fn into_view(s: &screens::Screen, q: &find::Query, f: find::Found) -> Result<find::Found, String> {
    if in_view(s, &f)? { Ok(f) } else { scroll_to(s, q, None) }
}

/// "X,Y" as two numbers.
fn pair(a: &str) -> Option<(f64, f64)> {
    let (x, y) = a.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Where, as agents write it: `X Y`, `X,Y` or `--at X,Y` (the screen's
/// logical points).
fn point(rest: &[String], what: &str) -> Result<(f64, f64), String> {
    if let Some(a) = option(rest, "--at") {
        return pair(a).ok_or_else(|| "--at is X,Y".to_owned());
    }
    let p = positionals(rest);
    if let Some(xy) = p.get(1).and_then(|a| pair(a)) {
        return Ok(xy);
    }
    match (p.get(1).and_then(|a| a.parse().ok()), p.get(2).and_then(|a| a.parse().ok())) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("{what} takes where: X Y, X,Y or --at X,Y (the screen's logical points){}", if what == "click" { ", or what to click: --name, --role, --match or --text" } else { "" })),
    }
}

/// The window `--window` names, if it's given: given with no id after it
/// (last, or before another option), refused, never taken for not given
/// (the window in front would have been acted on: Astra's review,
/// 2026-11-28).
fn window_arg(args: &[String]) -> Result<Option<u64>, String> {
    // (Every one checked, and one at most: `--window 35 --window` acted.)
    match window_ids(args)?.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        _ => Err("--window once: this acts in one window".to_owned()),
    }
}

/// Why a command for window `w` alone can't be given: app_server doesn't
/// say its surface, so its app can't be told which of its windows.
fn untargetable(w: &WindowInfo) -> String {
    format!(
        "window {} ({}) can't be told from its app's others (app_server says no surface for it), so nothing is done in it alone; leave out --window to act in the app's window in front",
        w.id, w.app_id
    )
}

fn pid_of(w: &WindowInfo) -> Result<u32, String> {
    w.pid.map(|p| p as u32).ok_or_else(|| format!("window {} has no process", w.id))
}

/// Chooses menu command `command` in window `w` itself, by its surface
/// (as its app knows it): never in another window of its app. Asked of
/// the app as a whole, the window in front took it: `--window` named
/// one document and the other, focused, closed (QA, 2026-11-28).
/// `explicit`: the window was named (`--window`), and nothing is done
/// anywhere else: not where app_server says no surface for it, nor where
/// its program takes commands only as a whole (a served one's, Chrome's
/// and Slack's), each an error. Unnamed (the window in front, or a
/// command that names its own target, a Chrome tab's), those are asked
/// of the app as a whole.
fn invoke_in(w: &WindowInfo, explicit: bool, command: &str) -> Result<bool, String> {
    let pid = pid_of(w)?;
    // (Not named: the app's command, asked as it always was, of its window
    // in front: asked of one window first, an app from before such asks
    // never heard it at all; Astra's review, 2026-11-28.)
    if !explicit {
        return Ok(commands::invoke(pid, command));
    }
    let Some(surface) = w.surface else { return Err(untargetable(w)) };
    match commands::invoke_window(pid, surface, command) {
        commands::InWindow::Done(taken) => Ok(taken),
        commands::InWindow::Unscoped => Err(whole_program(w)),
        commands::InWindow::Unanswered => Err(older(w)),
    }
}

/// Window `w`'s own menus, named by `--window` (`explicit`: as
/// [`invoke_in`], its app's in front where not).
fn menus_of(w: &WindowInfo, explicit: bool) -> Result<Option<commands::Menus>, String> {
    let pid = pid_of(w)?;
    if !explicit {
        return Ok(commands::menus(pid));
    }
    let Some(surface) = w.surface else { return Err(untargetable(w)) };
    match commands::menus_window(pid, surface) {
        commands::InWindow::Done(menus) => Ok(Some(menus)),
        commands::InWindow::Unscoped => Err(whole_program(w)),
        commands::InWindow::Unanswered => Err(older(w)),
    }
}

/// Why a command for window `w` alone isn't done: its program's commands
/// are the whole program's (Chrome's, Slack's).
fn whole_program(w: &WindowInfo) -> String {
    format!("{} takes commands only as a whole, not in one of its windows; leave out --window", w.app_id)
}

/// Why a command for window `w` alone isn't done: its program didn't
/// answer an ask for one window (older than such asks, or gone), so
/// nothing was done in any of its windows.
fn older(w: &WindowInfo) -> String {
    format!("{} didn't answer for window {} alone (an older Olo, or it's gone), so nothing was done; leave out --window to act in its window in front", w.app_id, w.id)
}

/// Whether review `r`, from window `w`'s app, is `w`'s: by surface where
/// both say it (two windows of one title are two reviews, each its
/// window's: QA, 2026-11-28, two documents named notes.txt each got
/// both), by title where either doesn't.
fn reviews_window(r: &commands::WindowReview, w: &WindowInfo) -> bool {
    match (r.surface, w.surface) {
        (Some(a), Some(b)) => a == b,
        _ => r.title == w.title,
    }
}

/// The review of window `w` alone, of its app's (`reviews`), its app's
/// windows being `siblings` (`w` among them): the one of its surface;
/// where they don't say one (an older kit), by title, only where that
/// names one window and one review; else an error saying it can't be
/// told (the review of all its windows, unnamed, has them all). Two
/// documents titled notes.txt each got both (Astra's review, 2026-11-28).
fn review_of(w: &WindowInfo, reviews: Vec<commands::WindowReview>, siblings: &[&WindowInfo]) -> Result<Vec<commands::WindowReview>, String> {
    let (exact, rest): (Vec<_>, Vec<_>) = reviews.into_iter().filter(|r| r.layer.is_none()).partition(|r| r.surface.is_some() && r.surface == w.surface);
    if !exact.is_empty() {
        return Ok(exact);
    }
    let legacy: Vec<_> = rest.into_iter().filter(|r| (r.surface.is_none() || w.surface.is_none()) && r.title == w.title).collect();
    let titled = siblings.iter().filter(|o| o.title == w.title).count();
    match legacy.len() {
        0 => Ok(Vec::new()),
        1 if titled <= 1 => Ok(legacy),
        _ => {
            Err(format!("window {} can't be told from another window of {} titled {:?} (it says no surface for them: an older Olo); review without --window for all of them", w.id, w.app_id, w.title))
        }
    }
}

/// Window `id`; or the focused one, or the only one showing.
fn app_window(s: &screens::Screen, id: Option<u64>) -> Result<WindowInfo, String> {
    let windows = s.windows()?;
    let listed = |windows: &[WindowInfo]| windows.iter().map(|w| format!("\n  {} {:?} ({}){}", w.id, w.title, w.app_id, if w.minimized { " minimized" } else { "" })).collect::<String>();
    match id {
        Some(id) => windows.iter().find(|w| w.id == id).cloned().ok_or_else(|| format!("no window {id}; the windows are:{}", listed(&windows))),
        None => {
            if let Some(w) = windows.iter().find(|w| w.focused) {
                return Ok(w.clone());
            }
            let showing: Vec<&WindowInfo> = windows.iter().filter(|w| !w.minimized && w.frame.is_some()).collect();
            match showing.as_slice() {
                [only] => Ok((*only).clone()),
                [] => Err("no windows are showing".to_owned()),
                _ => Err(format!("no window has the focus; give --window ID:{}", listed(&windows))),
            }
        }
    }
}

/// One window's tree, its nodes placed on the screen.
#[derive(Serialize)]
struct ScreenTree {
    window: u64,
    app_id: String,
    title: String,
    focused: bool,
    /// Where its content is on the screen.
    content: Frame,
    /// In tree order, a node's children after it; x and y are the screen's.
    nodes: Vec<commands::UiNode>,
    /// Where its open menus and dropdowns are (their items are in
    /// `nodes`, under a `Menu` each).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    popups: Vec<Frame>,
}

/// A window's open menus and dropdowns, put under it. A program gives a
/// popup's tree as a window's of its own (a menu's with no title, a
/// submenu's titled by its item), and app_server lists a popup with its
/// window, not as one: so the trees left over (`rest`: the program's
/// that no window took yet; not one titled as another window of the
/// program, `windows`) are matched to the window's `popups` by size,
/// each one's nodes moved to its popup's place on the screen, numbered
/// after the window's, its root a `Menu` under the window's. (They were
/// dropped: an open menu's items, a dropdown's, weren't in `tree` and
/// couldn't be clicked by name, as the skill says they can.) The popups
/// taken, in order.
fn with_popups(nodes: &mut Vec<commands::UiNode>, popups: &[Frame], rest: &mut Vec<commands::WindowTree>, windows: &[String]) -> Vec<Frame> {
    let root = nodes.first().map(|n| n.id);
    let mut shown = Vec::new();
    for p in popups {
        let fits = |t: &commands::WindowTree| t.layer.is_none() && t.width as i32 == p.width && t.height as i32 == p.height && (t.title.is_empty() || !windows.contains(&t.title));
        let Some(i) = rest.iter().position(fits) else { continue };
        let menu = rest.remove(i);
        let after = nodes.iter().map(|n| n.id).max().map_or(0, |m| m + 1);
        for mut n in menu.nodes {
            n.id += after;
            n.x += p.x as f32;
            n.y += p.y as f32;
            match n.parent {
                Some(parent) => n.parent = Some(parent + after),
                None => {
                    n.parent = root;
                    n.role = "Menu".into();
                }
            }
            nodes.push(n);
        }
        shown.push(*p);
    }
    shown
}

/// What the windows on `s` (or just window `only`) show. Windows of apps
/// not built on the kit, or minimized, have none.
fn trees(s: &screens::Screen, only: Option<u64>) -> Result<Vec<ScreenTree>, String> {
    let all: Vec<WindowInfo> = s.windows()?;
    if let Some(id) = only.filter(|id| !all.iter().any(|w| w.id == *id)) {
        return Err(format!("no window {id}"));
    }
    // One question per app (of the windows asked about), all at once:
    // each can take a frame or two.
    let mut pids: Vec<u32> = all.iter().filter(|w| w.content.is_some() && only.is_none_or(|id| w.id == id)).filter_map(|w| w.pid.map(|p| p as u32)).collect();
    pids.sort_unstable();
    pids.dedup();
    // A program not on Olo's kit: its tree over AT-SPI (GTK, Qt).
    let asked: Vec<_> = pids.iter().map(|&pid| (pid, std::thread::spawn(move || commands::tree(pid).filter(|t| !t.is_empty()).or_else(|| atspi::trees(pid))))).collect();
    // Windows only: Deskbar and other layer surfaces have no window here.
    let mut answers: Vec<(u32, Vec<commands::WindowTree>)> =
        asked.into_iter().map(|(pid, t)| (pid, t.join().ok().flatten().unwrap_or_default().into_iter().filter(|t| t.layer.is_none()).collect())).collect();
    let taken = take_trees(&all, &mut answers, only)?;
    // (A popup's tree is never one titled as a window of its app's that
    // took no tree: only those, not every window's on the screen; a
    // submenu "Tools" was dropped for another program's window "Tools":
    // Astra's review, 2026-11-28.)
    let resolved: Vec<u64> = taken.iter().map(|(w, ..)| w.id).collect();
    let unresolved = |pid: Option<i32>| -> Vec<String> { all.iter().filter(|o| o.pid == pid && !resolved.contains(&o.id)).map(|o| o.title.clone()).collect() };
    // Then each window asked about has its open menus from what's left
    // of its app's: a sibling's tree is never one of them.
    let mut out = Vec::new();
    for (w, content, tree) in taken.into_iter().filter(|(w, ..)| only.is_none_or(|id| w.id == id)) {
        let titles = unresolved(w.pid);
        let Some((_, mine)) = answers.iter_mut().find(|(p, _)| Some(*p as i32) == w.pid) else { continue };
        let mut nodes: Vec<commands::UiNode> = tree
            .nodes
            .into_iter()
            .map(|mut n| {
                n.x += content.x as f32;
                n.y += content.y as f32;
                n
            })
            .collect();
        let popups = with_popups(&mut nodes, &w.popups, mine, &titles);
        out.push(ScreenTree { window: w.id, app_id: w.app_id.clone(), title: w.title.clone(), focused: w.focused, content, nodes, popups });
    }
    Ok(out)
}

/// Each window's tree out of its app's (`answers`): every window of
/// those apps takes its own, the ones not asked about too, before
/// anything's left over for a popup (asked of one window, its sibling's
/// tree, the size of its open menu, was taken for that menu: Astra's
/// review, 2026-11-28). The trees of their surfaces first, then by title
/// and size for those that don't say one, one way at a time. Every
/// window that took one, in app_server's order, each with its content and
/// tree (what's asked about is the caller's to pick: the others name
/// which trees are no popup's); an error where the window named (`only`)
/// can't be told from another.
fn take_trees<'a>(all: &'a [WindowInfo], answers: &mut [(u32, Vec<commands::WindowTree>)], only: Option<u64>) -> Result<Vec<(&'a WindowInfo, Frame, commands::WindowTree)>, String> {
    let mut taken: Vec<Option<commands::WindowTree>> = all.iter().map(|_| None).collect();
    // (One strength of match at a time, across every window: a weaker
    // match of one window never takes the tree a stronger one names for
    // another; by size, A took B's tree before B came to it by title:
    // Astra's review, 2026-11-28.)
    for way in 0..WAYS {
        for (i, w) in all.iter().enumerate() {
            let (Some(content), Some(pid)) = (w.content, w.pid) else { continue };
            if taken[i].is_some() {
                continue;
            }
            let Some((_, mine)) = answers.iter_mut().find(|(p, _)| *p == pid as u32) else { continue };
            let siblings: Vec<&WindowInfo> = all.iter().filter(|o| o.pid == w.pid).collect();
            let at = match tree_by(w, content, mine, &siblings, way) {
                TreeMatch::Exact(at) => at,
                TreeMatch::Fallback(at, unique) => {
                    if !unique && only == Some(w.id) {
                        return Err(format!(
                            "window {} can't be told from another window of {} by its title and size (it says no surface for them: an older Olo); leave out --window to read them all",
                            w.id, w.app_id
                        ));
                    }
                    at
                }
                TreeMatch::None => continue,
            };
            taken[i] = Some(mine.remove(at));
        }
    }
    Ok(all.iter().zip(taken).filter_map(|(w, t)| Some((w, w.content?, t?))).collect())
}

/// How a window's tree is found among its app's ([`tree_by`], one way
/// at a time across every window in [`take_trees`]).
#[derive(Debug, PartialEq)]
enum TreeMatch {
    /// The tree of its surface.
    Exact(usize),
    /// By title or size, where a tree or the window says no surface; and
    /// whether that names it alone (no other tree, nor another of its
    /// app's windows, as like it).
    Fallback(usize, bool),
    None,
}

/// How many ways a tree is found ([`tree_by`]), strongest first.
const WAYS: usize = 5;

/// Which of its app's trees (`mine`) is window `w`'s (its content at
/// `content`; its app's windows `siblings`, it among them), by one way
/// alone (`way`, strongest first). By its surface, where both say it:
/// two windows of one title and size are never taken for each other, and
/// a tree that says another surface is never `w`'s. Else by its title
/// and size; its title; its page's title (Chrome's, "Page - Google
/// Chrome"), as wide; its size. Where it isn't by surface, whether it
/// names `w` alone: no other tree, nor another of its app's windows,
/// that the same way takes as well (a page's title by every window whose
/// title starts with it at its width: two Chrome windows "Mail …" took
/// the one tree "Mail" each; Astra's and Neon's reviews, 2026-11-28).
fn tree_by(w: &WindowInfo, content: Frame, mine: &[commands::WindowTree], siblings: &[&WindowInfo], way: usize) -> TreeMatch {
    if way == 0 {
        return mine.iter().position(|t| t.surface.is_some() && t.surface == w.surface).map_or(TreeMatch::None, TreeMatch::Exact);
    }
    let sized = |t: &commands::WindowTree| t.width as i32 == content.width && t.height as i32 == content.height;
    let may_be = |t: &commands::WindowTree| t.surface.is_none() || w.surface.is_none() || t.surface == w.surface;
    let same_size = |o: &WindowInfo| o.content.is_some_and(|c| c.width == content.width && c.height == content.height);
    let others = |like: &dyn Fn(&WindowInfo) -> bool| siblings.iter().any(|o| o.id != w.id && like(o));
    let matches = |t: &commands::WindowTree| match way {
        1 => t.title == w.title && sized(t),
        2 => t.title == w.title,
        3 => !t.title.is_empty() && w.title.starts_with(&t.title) && t.width as i32 == content.width,
        _ => sized(t),
    };
    let found: Vec<usize> = mine.iter().enumerate().filter(|(_, t)| may_be(t) && matches(t)).map(|(i, _)| i).collect();
    let Some(&at) = found.first() else { return TreeMatch::None };
    let t = &mine[at];
    let like_another = match way {
        1 => others(&|o| o.title == w.title && same_size(o)),
        2 => others(&|o| o.title == w.title),
        3 => others(&|o| o.title.starts_with(&t.title) && o.content.is_some_and(|c| c.width == t.width as i32)),
        _ => others(&same_size),
    };
    TreeMatch::Fallback(at, !like_another && found.len() == 1)
}

/// What `tree` shows of the trees.
#[derive(Debug, Default)]
struct Filter {
    role: Option<String>,
    /// In the name or value (any case).
    contains: Option<String>,
    /// Only what can be seen: some area, inside its window's content.
    visible_only: bool,
    depth: Option<usize>,
    /// Names and values cut to this many characters.
    max_len: Option<usize>,
    /// A table's cells (a row is named by them already).
    cells: bool,
}

impl Filter {
    fn from_args(args: &[String]) -> Result<Filter, String> {
        let n = |flag: &str| option(args, flag).map(|v| v.parse::<usize>().map_err(|_| format!("{flag} is a number"))).transpose();
        Ok(Filter {
            role: option(args, "--role").cloned(),
            contains: option(args, "--name-contains").or(option(args, "--contains")).cloned(),
            visible_only: has(args, "--visible-only"),
            depth: n("--depth")?,
            max_len: n("--max-len")?,
            cells: has(args, "--cells"),
        })
    }

    fn narrows(&self) -> bool {
        self.role.is_some() || self.contains.is_some()
    }

    /// Keep `n` (at `depth`, in a window whose content is `c`)?
    fn keeps(&self, n: &commands::UiNode, depth: usize, c: Frame, popups: &[Frame]) -> bool {
        if self.depth.is_some_and(|d| depth > d) {
            return false;
        }
        if self.visible_only {
            // (A menu's items show wherever its popup is, the window's
            // content or not.)
            let within = |c: &Frame| n.x < (c.x + c.width) as f32 && n.y < (c.y + c.height) as f32 && n.x + n.width > c.x as f32 && n.y + n.height > c.y as f32;
            let inside = within(&c) || popups.iter().any(within);
            // (What has no place but is shown, done by a command, is kept:
            // Chrome's tabs.)
            if (n.width < 1.0 || n.height < 1.0 || !inside) && n.command.is_none() {
                return false;
            }
        }
        if self.role.as_ref().is_some_and(|r| !n.role.eq_ignore_ascii_case(r)) {
            return false;
        }
        if let Some(t) = &self.contains {
            let t = t.to_lowercase();
            if ![&n.name, &n.value].iter().any(|s| s.as_ref().is_some_and(|s| s.to_lowercase().contains(&t))) {
                return false;
            }
        }
        true
    }

    fn cut(&self, s: &str) -> String {
        match self.max_len {
            Some(max) if s.chars().count() > max => format!("{}…", s.chars().take(max).collect::<String>()),
            _ => s.to_owned(),
        }
    }
}

/// The trees as an indented outline, one node a line.
fn outline(trees: &[ScreenTree], filter: &Filter) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    for t in trees {
        let c = t.content;
        let _ = writeln!(o, "window {} {:?} ({}){} content {},{} {}x{}", t.window, t.title, t.app_id, if t.focused { " focused" } else { "" }, c.x, c.y, c.width, c.height);
        let mut depth: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        let mut last: Option<&commands::UiNode> = None;
        // A table's rows are reported each once, by name and place; their
        // cells (and the unnamed area behind them) only with --cells.
        let mut in_row: Option<(f32, f32)> = None;
        for n in &t.nodes {
            let d = n.parent.and_then(|p| depth.get(&p)).map_or(0, |d| d + 1);
            depth.insert(n.id, d);
            let repeat = last.is_some_and(|l| l.role == n.role && l.name == n.name && l.value == n.value && l.x == n.x && l.y == n.y && l.width == n.width);
            last = Some(n);
            if n.role == "Row" {
                in_row = Some((n.y, n.y + n.height));
            } else if !matches!(n.role.as_str(), "Cell" | "Unknown") {
                in_row = None;
            }
            if repeat || !filter.cells && in_row.is_some() && n.role != "Row" {
                continue;
            }
            // Containers without a name say nothing an agent needs, and a
            // text run repeats its field's value.
            // Nor does a row scrolled out of sight.
            if n.role == "Row" && (n.width == 0.0 || n.height == 0.0) {
                continue;
            }
            if n.role == "TextRun" || n.name.is_none() && n.value.is_none() && matches!(n.role.as_str(), "GenericContainer" | "Window" | "Pane" | "Group" | "ScrollView") {
                continue;
            }
            if !filter.keeps(n, d, c, &t.popups) {
                continue;
            }
            let indent = if filter.narrows() { 2 } else { 2 + 2 * d.min(8) };
            let _ = write!(o, "{:indent$}{}", "", n.role);
            if let Some(name) = &n.name {
                let _ = write!(o, " {:?}", filter.cut(name));
            }
            if let Some(value) = &n.value {
                let _ = write!(o, " value={:?}", filter.cut(value));
            }
            // (What a dropdown offers, where its items aren't things of
            // their own: what `select` can choose.)
            if !n.options.is_empty() {
                let shown: Vec<String> = n.options.iter().take(12).map(|o| format!("{:?}", filter.cut(o))).collect();
                let _ = write!(o, " options=[{}{}]", shown.join(", "), if n.options.len() > shown.len() { format!(", … {} in all", n.options.len()) } else { String::new() });
            }
            // (A node with no place says none: Chrome's tabs, done by a
            // command.)
            if n.width > 0.0 || n.height > 0.0 {
                let _ = write!(o, " at {:.0},{:.0} {:.0}x{:.0}", n.x, n.y, n.width, n.height);
            }
            if let (Some(row), Some(rows)) = (n.row, n.rows) {
                let _ = write!(o, " row {}/{rows}", row + 1);
            }
            for (on, word) in [
                (!n.enabled, "disabled"),
                (n.clickable && !matches!(n.role.as_str(), "Clickable" | "Button" | "Link" | "MenuItem" | "Row"), "clickable"),
                (n.focused, "focused"),
                (n.checked == Some(true), "checked"),
                (n.selected == Some(true), "selected"),
                (n.minimized, "minimized"),
                (n.busy, "busy"),
            ] {
                if on {
                    let _ = write!(o, " {word}");
                }
            }
            if let Some(over) = &n.covered {
                let _ = write!(o, " under={:?}", filter.cut(over));
            }
            o.push('\n');
        }
    }
    o
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn local_list_json() -> Vec<serde_json::Value> {
    let running = screens::list();
    screens::all()
        .into_iter()
        .map(|(name, state)| {
            let title = history::title(&name);
            let mut v = match running.iter().find(|s| s.name == name) {
                Some(s) => json!(s),
                None => {
                    let kept = screens::kept(&name).unwrap_or_default();
                    json!({ "name": name, "width": kept.width, "height": kept.height, "scale": kept.scale, "sound": kept.sound })
                }
            };
            v["state"] = json!(state);
            v["title"] = json!(title);
            v
        })
        .collect()
}

/// Validate a `watch` target (`NAME` or `HOST:NAME`, splitting on the LAST colon
/// so host labels containing colons like `lab:west:qa` work cleanly — Finding 32).
fn validate_watch_target(target: &str) -> Result<(), String> {
    if target.is_empty() {
        return Err("watch target cannot be empty".into());
    }
    if let Some((host, name)) = target.rsplit_once(':') {
        if host.trim().is_empty() {
            return Err(format!("invalid watch target {target:?}: missing host before ':'"));
        }
        if !screens::valid_name(name) {
            return Err(format!("invalid screen name {name:?} in {target:?} (letters, digits, - and _)"));
        }
        Ok(())
    } else if screens::valid_name(target) {
        Ok(())
    } else {
        Err(format!("{target:?} can't name a screen (letters, digits, - and _)"))
    }
}

/// Resolve a Rex host spec by exact `key` first, then exact `label`, then a
/// unique case-insensitive `label` match (Finding 32).
fn resolve_rex_host<'a>(hosts: &'a [olo_screen_pane::rex_sock::RexHost], spec: &str) -> Result<&'a olo_screen_pane::rex_sock::RexHost, String> {
    if let Some(h) = hosts.iter().find(|h| h.key == spec) {
        return Ok(h);
    }
    let exact_label: Vec<&olo_screen_pane::rex_sock::RexHost> = hosts.iter().filter(|h| h.label == spec).collect();
    if exact_label.len() == 1 {
        return Ok(exact_label[0]);
    }
    if exact_label.len() > 1 {
        return Err(format!("multiple Rex hosts are named {spec:?}; pass the host key instead"));
    }
    let ci: Vec<&olo_screen_pane::rex_sock::RexHost> = hosts.iter().filter(|h| h.label.eq_ignore_ascii_case(spec)).collect();
    match ci.as_slice() {
        [only] => Ok(only),
        [] => Err(format!("no Rex host named {spec:?}")),
        _ => Err(format!("host name {spec:?} is ambiguous; pass the exact spelling or host key")),
    }
}

/// Query the live, peer-authenticated Rex Screens socket (Finding 9: never
/// trust a stale on-disk state file as live authority for `list --all`/`--host`).
fn read_live_rex_state() -> Option<olo_screen_pane::rex_sock::RexScreensState> {
    olo_screen_pane::RexSockClient::new().list().ok()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ListCliArgs {
    all: bool,
    as_json: bool,
    host_filter: Option<String>,
}

fn parse_list_cli_args(rest: &[String]) -> Result<ListCliArgs, String> {
    let mut out = ListCliArgs::default();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--all" => {
                out.all = true;
                i += 1;
            }
            "--json" => {
                out.as_json = true;
                i += 1;
            }
            "--host" => {
                let Some(val) = rest.get(i + 1) else {
                    return Err("--host requires a host name or key".into());
                };
                if val.is_empty() || val.starts_with('-') {
                    return Err(format!("--host requires a host name or key (got {val:?})"));
                }
                if out.host_filter.is_some() {
                    return Err("--host may only be given once".into());
                }
                out.host_filter = Some(val.clone());
                i += 2;
            }
            other => return Err(format!("unknown list option {other:?}")),
        }
    }
    if out.all && out.host_filter.is_some() {
        return Err("--all and --host cannot be combined".into());
    }
    Ok(out)
}

fn list_screens_cli(rest: &[String]) -> Result<(), String> {
    let parsed = parse_list_cli_args(rest)?;
    if !parsed.all && parsed.host_filter.is_none() {
        let listed = local_list_json();
        println!("{}", serde_json::to_string_pretty(&listed).unwrap_or_default());
        return Ok(());
    }
    let rex_state = read_live_rex_state();
    if let Some(target_host) = parsed.host_filter.as_deref() {
        let Some(state) = rex_state else {
            return Err("Rex isn’t running or has no hosts published.".into());
        };
        let host = resolve_rex_host(&state.hosts, target_host)?;
        if parsed.as_json {
            println!("{}", serde_json::to_string_pretty(host).unwrap_or_default());
        } else {
            print_host_text(host, &state.hosts);
        }
        return Ok(());
    }
    let local = local_list_json();
    let hosts = rex_state.map(|s| s.hosts).unwrap_or_default();
    if parsed.as_json {
        println!("{}", serde_json::to_string_pretty(&json!({ "local": local, "hosts": hosts })).unwrap_or_default());
    } else {
        println!("This Computer");
        if local.is_empty() {
            println!("  No screens");
        } else {
            for s in &local {
                let name = s["name"].as_str().unwrap_or("");
                let st = s["state"].as_str().unwrap_or("");
                let w = s["width"].as_i64().unwrap_or(0);
                let h = s["height"].as_i64().unwrap_or(0);
                let title = s["title"].as_str().map(|t| format!("  ({})", olo_screen_pane::rex_sock::sanitize_display_text(t))).unwrap_or_default();
                println!("  {name:<20} {st:<10} {w}x{h}{title}");
            }
        }
        for host in &hosts {
            println!();
            print_host_text(host, &hosts);
        }
    }
    Ok(())
}

/// Format one Rex host's screens for text output, printing the stable `host.key`
/// in copyable `HOST:NAME` targets whenever `host.label` contains `:` or fails
/// to resolve uniquely back to this host (Final pass B.10 / Astra #32).
fn format_host_text(host: &olo_screen_pane::rex_sock::RexHost, all_hosts: &[olo_screen_pane::rex_sock::RexHost]) -> String {
    use std::fmt::Write;
    let sanitize = olo_screen_pane::rex_sock::sanitize_display_text;
    let label = sanitize(&host.label);
    let key = sanitize(&host.key);
    let label_resolves_here = !host.label.is_empty() && !host.label.contains(':') && resolve_rex_host(all_hosts, &host.label).ok().map(|h| h.key.as_str()) == Some(host.key.as_str());
    let target_prefix = if label_resolves_here { label.clone() } else { key.clone() };
    let mut out = String::new();
    let tag = match host.state {
        olo_screen_pane::rex_sock::RexHostConn::Connected if !host.has_olo_screens() => " (not set up)",
        olo_screen_pane::rex_sock::RexHostConn::Connected => "",
        olo_screen_pane::rex_sock::RexHostConn::Disconnected => " (disconnected)",
        olo_screen_pane::rex_sock::RexHostConn::Reconnecting => " (reconnecting)",
        olo_screen_pane::rex_sock::RexHostConn::Connecting => " (connecting)",
        olo_screen_pane::rex_sock::RexHostConn::Error => " (error)",
    };
    if target_prefix != label {
        let _ = writeln!(out, "{label} [{key}]{tag}");
    } else {
        let _ = writeln!(out, "{label}{tag}");
    }
    if !host.has_olo_screens() && host.state.is_connected() {
        let why = host.not_set_up_reason.as_deref().map(sanitize).filter(|s| !s.is_empty()).unwrap_or_else(|| "Not set up".into());
        let _ = writeln!(out, "  {why}");
        return out;
    }
    if host.screens.is_empty() {
        let _ = writeln!(out, "  No screens");
        return out;
    }
    for s in &host.screens {
        let st = match s.state {
            olo_screen_pane::rex_sock::HostScreenState::Running => "running",
            olo_screen_pane::rex_sock::HostScreenState::Stopped => "stopped",
            olo_screen_pane::rex_sock::HostScreenState::Archived => "archived",
        };
        let title = s.title.as_deref().map(sanitize).filter(|t| !t.is_empty()).map(|t| format!("  ({t})")).unwrap_or_default();
        let _ = writeln!(out, "  {target_prefix}:{:<16} {st:<10} {}x{}{title}", s.name, s.width, s.height);
    }
    out
}

fn print_host_text(host: &olo_screen_pane::rex_sock::RexHost, all_hosts: &[olo_screen_pane::rex_sock::RexHost]) {
    print!("{}", format_host_text(host, all_hosts));
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = cli(&args) {
        std::process::exit(match result {
            Ok(()) => 0,
            Err(e) => fail(e),
        });
    }
    let show = args.get(1).cloned();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        // (Re-executed by an upgrade, the same pid: what it said it
        // controlled before isn't so now.)
        screens::clear_controlled_by_me();
        let window = olo_kit::open_window(olo_kit::WindowOptions::new("Agent Screens", 1000.0, 640.0).min_size(380.0, 320.0).keep_size("main"), move |cx| viewer::Viewer::new(show, cx));
        // What the workers and other starts say goes to the window.
        olo_kit::message_handler(app, move |message, _| {
            window.messenger().send(message);
        });
    });
}

/// Where a video of screen `name` goes unless told: the Videos folder,
/// named for the screen and when.
fn video_path(name: &str) -> std::path::PathBuf {
    let dir = olo_xdg::user_dir("VIDEOS").unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos"));
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r fills `tm` from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    dir.join(format!("Agent Screen {name} {:04}-{:02}-{:02} at {:02}.{:02}.{:02}.mp4", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window `w`'s tree among its app's by the strongest way that finds
    /// one (as [`take_trees`] has it for a window alone).
    fn tree_of(w: &WindowInfo, content: Frame, mine: &[commands::WindowTree], siblings: &[&WindowInfo]) -> TreeMatch {
        (0..WAYS).map(|way| tree_by(w, content, mine, siblings, way)).find(|m| *m != TreeMatch::None).unwrap_or(TreeMatch::None)
    }

    /// A field read back after `set`: what it holds empty, or as before,
    /// is a field that didn't keep it; formatted, kept; and nothing said
    /// where it can't be told (a password, no value, the keyboard gone on).
    #[test]
    fn a_set_field_is_read_back() {
        let field = |role: &str, value: Option<&str>, x: f32| find::Found {
            window: 1,
            role: role.into(),
            name: Some("Enter code".into()),
            value: value.map(Into::into),
            x,
            y: 100.0,
            width: 200.0,
            height: 30.0,
            center: (x + 100.0, 115.0),
            focused: true,
            row: None,
            rows: None,
            options: Vec::new(),
            covered: None,
            command: None,
        };
        let f = field("TextInput", Some(""), 10.0);
        assert!(set_not_kept(&f, "123456", Some(&field("TextInput", Some(""), 10.0))).is_some(), "empty after");
        assert!(set_not_kept(&field("TextInput", Some("old"), 10.0), "new", Some(&field("TextInput", Some("old"), 10.0))).is_some(), "as before");
        assert!(set_not_kept(&f, "123456", Some(&field("TextInput", Some("123 456"), 10.0))).is_none(), "formatted: kept");
        assert!(set_not_kept(&f, "secret", Some(&field("PasswordInput", Some(""), 10.0))).is_none(), "a password: can't be told");
        assert!(set_not_kept(&f, "123456", Some(&field("TextInput", None, 10.0))).is_none(), "no value said");
        assert!(set_not_kept(&f, "1", Some(&field("TextInput", Some(""), 400.0))).is_none(), "the keyboard moved on");
        assert!(set_not_kept(&f, "123456", None).is_none());
        // A select's words reach it, or its list, opened; not a search box
        // or another select that has the keyboard.
        let select = |name: &str, x: f32| find::Found { role: "ComboBox".into(), name: Some(name.into()), ..field("ComboBox", None, x) };
        let sel = select("Country", 10.0);
        assert!(keys_reach(&sel, Some(&select("Country", 10.0))));
        assert!(keys_reach(&sel, Some(&find::Found { role: "ListBoxOption".into(), ..field("ListBoxOption", None, 300.0) })));
        assert!(!keys_reach(&sel, None), "nothing has it: the page takes them");
        assert!(!keys_reach(&sel, Some(&select("Language", 300.0))), "another select");
        assert!(!keys_reach(&sel, Some(&find::Found { role: "SearchInput".into(), ..field("SearchInput", None, 300.0) })), "a search box");
        assert!(!keys_reach(&sel, Some(&find::Found { role: "Document".into(), ..field("Document", None, 0.0) })), "the page");
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// An action's history keeps how the screen was after it, but not
    /// its picture, tree and windows (the frames are kept).
    #[test]
    fn the_history_leaves_the_picture_and_tree_out() {
        let v = json!({ "clicked": [1, 2], "after": { "window": { "id": 1 }, "settled": true, "picture": "/p.png", "tree": ["window 1"], "windows": ["1 \"A\" (a) focused"], "ms": { "settling": 300, "picture": 5 } } });
        assert_eq!(for_history(v), json!({ "clicked": [1, 2], "after": { "window": { "id": 1 }, "settled": true, "ms": { "settling": 300, "picture": 5 } } }));
        assert_eq!(for_history(json!({ "pid": 4 })), json!({ "pid": 4 }));
    }

    /// Two windows of one app with one title (two documents named
    /// notes.txt, from two folders): each window's review and tree is its
    /// own, told apart by surface; by title where the app or app_server
    /// doesn't say one.
    #[test]
    fn two_windows_of_one_title_are_told_apart() {
        let window = |id: u64, surface: Option<u32>| -> WindowInfo {
            serde_json::from_value(json!({ "id": id, "app_id": "olo.styled-edit", "title": "notes.txt", "pid": 10, "workspace": 0, "minimized": false, "focused": id == 2, "surface": surface,
                "content": { "x": 0, "y": 0, "width": 600, "height": 400 } }))
            .unwrap()
        };
        let review =
            |surface: Option<u32>| commands::WindowReview { title: "notes.txt".into(), width: 600, height: 400, scale: 1.0, dark: false, layer: None, surface, resizing: false, findings: Vec::new() };
        let (a, b) = (window(1, Some(5)), window(2, Some(7)));
        assert!(reviews_window(&review(Some(5)), &a) && !reviews_window(&review(Some(5)), &b));
        assert!(reviews_window(&review(Some(7)), &b) && !reviews_window(&review(Some(7)), &a));
        // (Where either doesn't say: by title, as before.)
        assert!(reviews_window(&review(None), &a) && reviews_window(&review(None), &b));
        assert!(reviews_window(&review(Some(5)), &window(3, None)));
        // Trees: the same, in whatever order the app answers.
        let tree = |surface: Option<u32>, name: &str| commands::WindowTree {
            title: "notes.txt".into(),
            width: 600,
            height: 400,
            surface,
            nodes: vec![commands::UiNode { id: 1, role: "Window".into(), name: Some(name.into()), ..Default::default() }],
            ..Default::default()
        };
        let content = Frame { x: 0, y: 0, width: 600, height: 400 };
        let mine = vec![tree(Some(7), "second"), tree(Some(5), "first")];
        let both = [&a, &b];
        assert_eq!(tree_of(&a, content, &mine, &both), TreeMatch::Exact(1));
        assert_eq!(tree_of(&b, content, &mine, &both), TreeMatch::Exact(0));
        // A tree that says another window's surface is never this one's,
        // though its title and size are the same.
        assert_eq!(tree_of(&a, content, &[tree(Some(7), "second")], &both), TreeMatch::None);
        // Where a tree or the window says no surface (an older kit): by
        // title, said alone only where nothing else is like it.
        assert_eq!(tree_of(&a, content, &[tree(None, "unsaid")], &[&a]), TreeMatch::Fallback(0, true));
        assert_eq!(tree_of(&a, content, &[tree(None, "unsaid")], &both), TreeMatch::Fallback(0, false), "a sibling of its title and size");
        let c = window(3, None);
        assert_eq!(tree_of(&c, content, &[tree(None, "x"), tree(None, "y")], &[&c]), TreeMatch::Fallback(0, false), "two trees as like it");
    }

    /// A window named (`--window`) is never taken for another: given
    /// with no id, refused; a review of it, its own (by surface; by title
    /// only where that names one window and one review); a command or
    /// menus for it, never done elsewhere where its surface is unknown.
    #[test]
    fn a_window_named_is_the_one_acted_on() {
        assert_eq!(window_arg(&args(&["qa", "close", "--window"])).unwrap_err(), "--window needs a window's id after it (`windows` lists them)");
        assert!(window_arg(&args(&["qa", "--window", "--json"])).is_err());
        assert_eq!(window_arg(&args(&["qa", "--window", "35"])), Ok(Some(35)));
        assert_eq!(window_arg(&args(&["qa"])), Ok(None));
        let window = |id: u64, title: &str, surface: Option<u32>| -> WindowInfo {
            serde_json::from_value(json!({ "id": id, "app_id": "olo.styled-edit", "title": title, "pid": 10, "workspace": 0, "minimized": false, "focused": false, "surface": surface })).unwrap()
        };
        let review = |title: &str, surface: Option<u32>| commands::WindowReview {
            title: title.into(),
            width: 600,
            height: 400,
            scale: 1.0,
            dark: false,
            layer: None,
            surface,
            resizing: false,
            findings: Vec::new(),
        };
        let (a, b) = (window(1, "notes.txt", Some(5)), window(2, "notes.txt", Some(7)));
        let got = review_of(&a, vec![review("notes.txt", Some(7)), review("notes.txt", Some(5))], &[&a, &b]).unwrap();
        assert_eq!(got.iter().map(|r| r.surface).collect::<Vec<_>>(), [Some(5)]);
        // An older kit's reviews, no surface: two of one title can't be told.
        assert!(review_of(&a, vec![review("notes.txt", None), review("notes.txt", None)], &[&a, &b]).is_err());
        assert!(review_of(&a, vec![review("notes.txt", None)], &[&a, &b]).is_err(), "one review, two windows of its title");
        let c = window(3, "todo.txt", Some(9));
        assert_eq!(review_of(&c, vec![review("todo.txt", None), review("notes.txt", None)], &[&a, &b, &c]).unwrap().len(), 1, "one of its title: it");
        // Its surface unknown to app_server: a command for it alone isn't
        // given to its app's window in front.
        let unknown = window(4, "notes.txt", None);
        assert!(invoke_in(&unknown, true, "close").unwrap_err().contains("can't be told"));
        assert!(menus_of(&unknown, true).is_err());
    }

    /// Every window of the app takes its own tree before any is left for
    /// a popup: asked of one window, its sibling's tree (the size of its
    /// open menu) is never taken for that menu, whichever answers first.
    #[test]
    fn a_siblings_tree_is_never_a_popup() {
        let window = |id: u64, title: &str, surface: u32, popup: bool| -> WindowInfo {
            let popups = if popup { json!([{ "x": 100, "y": 100, "width": 200, "height": 60 }]) } else { json!([]) };
            serde_json::from_value(json!({ "id": id, "app_id": "olo.app", "title": title, "pid": 10, "workspace": 0, "minimized": false, "focused": id == 1, "surface": surface,
                "content": { "x": 0, "y": 0, "width": 600, "height": 400 }, "popups": popups }))
            .unwrap()
        };
        let tree = |title: &str, w: u32, h: u32, surface: Option<u32>| commands::WindowTree { title: title.into(), width: w, height: h, surface, ..Default::default() };
        // (B is as big as A's open menu.)
        let all = [window(1, "A", 5, true), window(2, "B", 7, false)];
        let all = [all[0].clone(), WindowInfo { content: Some(Frame { x: 700, y: 0, width: 200, height: 60 }), ..all[1].clone() }];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let pool = [tree("A", 600, 400, Some(5)), tree("B", 200, 60, Some(7)), tree("", 200, 60, Some(11))];
            let mut answers = vec![(10u32, order.iter().map(|&i| pool[i].clone()).collect::<Vec<_>>())];
            let taken = take_trees(&all, &mut answers, Some(1)).unwrap();
            let ids: Vec<(u64, &str)> = taken.iter().map(|(w, _, t)| (w.id, t.title.as_str())).collect();
            assert_eq!(ids, [(1, "A"), (2, "B")], "each window its own tree, order {order:?}");
            // What's left for A's menu: its menu, never B.
            let left: Vec<&str> = answers[0].1.iter().map(|t| t.title.as_str()).collect();
            assert_eq!(left, [""], "order {order:?}");
        }
    }

    /// One strength of match at a time across an app's windows: A, first
    /// in app_server's order, never takes B's tree by size before B takes
    /// it by its title; and a page's title that two windows start with,
    /// at its width, names neither alone.
    #[test]
    fn a_weaker_match_never_takes_a_siblings_tree() {
        let window = |id: u64, title: &str, width: i32| -> WindowInfo {
            serde_json::from_value(json!({ "id": id, "app_id": "olo.app", "title": title, "pid": 10, "workspace": 0, "minimized": false, "focused": false,
                "content": { "x": 0, "y": 0, "width": width, "height": 400 } }))
            .unwrap()
        };
        let tree = |title: &str| commands::WindowTree { title: title.into(), width: 600, height: 400, ..Default::default() };
        let all = [window(1, "Inbox", 600), window(2, "Drafts", 600)];
        let mut answers = vec![(10u32, vec![tree("Drafts")])];
        let taken = take_trees(&all, &mut answers, Some(2)).unwrap();
        let ids: Vec<(u64, &str)> = taken.iter().map(|(w, _, t)| (w.id, t.title.as_str())).collect();
        assert_eq!(ids, [(2, "Drafts")], "Drafts by its title, not Inbox by its size");
        // Two Chrome windows whose titles start "Mail", one tree "Mail".
        let all = [window(3, "Mail - Inbox - Google Chrome", 600), window(4, "Mail - Google Chrome", 600)];
        let siblings: Vec<&WindowInfo> = all.iter().collect();
        let content = all[0].content.unwrap();
        assert_eq!(tree_by(&all[0], content, &[tree("Mail")], &siblings, 3), TreeMatch::Fallback(0, false), "names neither alone");
        let mut answers = vec![(10u32, vec![tree("Mail")])];
        assert!(take_trees(&all, &mut answers, Some(3)).is_err(), "asked of one, it can't be told");
    }

    /// A popup's tree is never one titled as a window of its app's that
    /// took no tree, but a window of another program's title doesn't
    /// keep a submenu out.
    #[test]
    fn a_popup_is_kept_from_its_own_apps_windows_only() {
        let menu = |title: &str| commands::WindowTree { title: title.into(), width: 200, height: 60, ..Default::default() };
        let popup = [Frame { x: 10, y: 10, width: 200, height: 60 }];
        let mut nodes = vec![commands::UiNode { id: 1, ..Default::default() }];
        let mut rest = vec![menu("Tools")];
        assert_eq!(with_popups(&mut nodes, &popup, &mut rest, &[]).len(), 1, "another program's \"Tools\" keeps nothing out");
        let mut rest = vec![menu("Tools")];
        assert!(with_popups(&mut nodes, &popup, &mut rest, &["Tools".to_owned()]).is_empty(), "its own window \"Tools\", which took no tree, is no popup");
    }

    /// Every `--window` has its id: one without is an error, never every
    /// window; a command for one window takes it once.
    #[test]
    fn every_window_named_has_its_id() {
        assert_eq!(window_ids(&args(&["qa", "--window", "3", "--window", "4"])), Ok(vec![3, 4]));
        assert!(window_ids(&args(&["qa", "--window"])).is_err(), "give qa --window: not every window");
        assert!(window_ids(&args(&["qa", "--window", "3", "--window"])).is_err());
        assert_eq!(window_ids(&args(&["qa"])), Ok(vec![]));
        assert!(window_arg(&args(&["qa", "close", "--window", "35", "--window"])).is_err(), "checked every time");
        assert!(window_arg(&args(&["qa", "close", "--window", "35", "--window", "36"])).is_err(), "one window");
        assert_eq!(window_arg(&args(&["qa", "close", "--window", "35"])), Ok(Some(35)));
    }

    /// The windows an action lists: id, title, app, and how each is.
    #[test]
    fn a_window_in_a_line() {
        let w = |focused: bool, minimized: bool| -> WindowInfo {
            serde_json::from_value(json!({ "id": 3, "app_id": "olo.tracker", "title": "Home", "pid": null, "workspace": 0, "minimized": minimized, "focused": focused })).unwrap()
        };
        assert_eq!(window_line(&w(true, false)), "3 \"Home\" (olo.tracker) focused");
        assert_eq!(window_line(&w(false, true)), "3 \"Home\" (olo.tracker) minimized");
    }

    /// An open menu's items are in its window's tree: the program's tree
    /// with no title, taken by its popup's size, moved to the popup's
    /// place, under a `Menu` under the window; a tree no popup fits is
    /// left.
    #[test]
    fn an_open_menu_is_under_its_window() {
        let node = |id, parent, role: &str, name: Option<&str>, x, y| commands::UiNode {
            id,
            parent,
            role: role.into(),
            name: name.map(str::to_owned),
            x,
            y,
            width: 100.0,
            height: 20.0,
            enabled: true,
            ..Default::default()
        };
        let tree = |title: &str, w, h, nodes| commands::WindowTree { title: title.into(), width: w, height: h, active: false, layer: None, surface: None, path: None, nodes };
        let mut nodes = vec![node(0, None, "Window", Some("Notes"), 50.0, 60.0), node(1, Some(0), "Button", Some("Save"), 60.0, 70.0)];
        let mut rest = vec![
            tree("", 300, 90, vec![node(0, None, "Window", None, 0.0, 0.0), node(1, Some(0), "MenuItem", Some("Other"), 4.0, 4.0)]),
            tree("", 200, 60, vec![node(0, None, "Window", None, 0.0, 0.0), node(1, Some(0), "MenuItem", Some("Copy"), 4.0, 4.0), node(2, Some(0), "MenuItem", Some("Paste"), 4.0, 24.0)]),
            // (Another window of the program, the size of a popup: a window's.)
            tree("Letter", 120, 40, vec![node(0, None, "Window", Some("Letter"), 0.0, 0.0)]),
            // (A submenu: titled by its item.)
            tree("Paste", 120, 40, vec![node(0, None, "Window", Some("Paste"), 0.0, 0.0), node(1, Some(0), "MenuItem", Some("As Plain Text"), 4.0, 4.0)]),
        ];
        let popups = [Frame { x: 400, y: 300, width: 200, height: 60 }, Frame { x: 0, y: 0, width: 77, height: 77 }, Frame { x: 598, y: 320, width: 120, height: 40 }];
        let shown = with_popups(&mut nodes, &popups, &mut rest, &["Notes".to_owned(), "Letter".to_owned()]);
        assert_eq!(shown, vec![popups[0], popups[2]]);
        assert_eq!(rest.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["", "Letter"], "the tree no popup fits stays, and another window's");
        assert_eq!(
            nodes.split_off(5).iter().map(|n| (n.id, n.parent, n.role.as_str(), n.name.as_deref(), n.x, n.y)).collect::<Vec<_>>(),
            vec![(5, Some(0), "Menu", Some("Paste"), 598.0, 320.0), (6, Some(5), "MenuItem", Some("As Plain Text"), 602.0, 324.0)]
        );
        let said: Vec<_> = nodes.iter().map(|n| (n.id, n.parent, n.role.as_str(), n.name.as_deref(), n.x, n.y)).collect();
        assert_eq!(
            said,
            vec![
                (0, None, "Window", Some("Notes"), 50.0, 60.0),
                (1, Some(0), "Button", Some("Save"), 60.0, 70.0),
                (2, Some(0), "Menu", None, 400.0, 300.0),
                (3, Some(2), "MenuItem", Some("Copy"), 404.0, 304.0),
                (4, Some(2), "MenuItem", Some("Paste"), 404.0, 324.0),
            ]
        );
        // Found by name, and listed in the outline under the window;
        // seen with --visible-only though outside the window's content.
        let t = ScreenTree { window: 7, app_id: "olo.notes".into(), title: "Notes".into(), focused: true, content: Frame { x: 50, y: 60, width: 300, height: 200 }, nodes, popups: shown };
        let found = find::matches(std::slice::from_ref(&t), &find::Query { role: Some("MenuItem".into()), name: Some("Paste".into()), ..Default::default() });
        assert_eq!(found.len(), 1);
        let o = outline(std::slice::from_ref(&t), &Filter { visible_only: true, ..Default::default() });
        assert!(o.contains("Menu at 400,300 100x20") && o.contains("MenuItem \"Copy\" at 404,304"), "{o}");
    }

    /// A page's control under its sticky header: marked in `tree`'s line
    /// with what covers it, carried by what is found (so `click` scrolls it
    /// out first, a notch up under a header at the top, down under a
    /// banner at the foot), and not in view while it is under it.
    #[test]
    fn a_control_under_a_pages_layer() {
        let node = |covered: Option<&str>| commands::UiNode {
            id: 1,
            role: "Button".into(),
            name: Some("Save".into()),
            x: 100.0,
            y: 40.0,
            width: 80.0,
            height: 24.0,
            enabled: true,
            clickable: true,
            covered: covered.map(Into::into),
            ..Default::default()
        };
        let t = |covered| ScreenTree {
            window: 7,
            app_id: "olo.chrome".into(),
            title: "Shop".into(),
            focused: true,
            content: Frame { x: 0, y: 0, width: 800, height: 600 },
            nodes: vec![node(covered)],
            popups: Vec::new(),
        };
        let o = outline(&[t(Some("sticky Banner"))], &Filter::default());
        assert!(o.contains("Button \"Save\" at 100,40 80x24 under=\"sticky Banner\""), "{o}");
        assert!(!outline(&[t(None)], &Filter::default()).contains("under="));
        let found = find::matches(&[t(Some("sticky Banner"))], &find::Query { name: Some("Save".into()), ..Default::default() });
        assert_eq!(found[0].covered.as_deref(), Some("sticky Banner"));
        // Out from under: up in the view's upper half, down in its lower.
        assert_eq!(wheel_out(52.0, 0.0, 600.0), -1);
        assert_eq!(wheel_out(580.0, 0.0, 600.0), 1);
    }

    /// Chrome's tabs: no place of their own (Chrome draws its strip), each
    /// with the command that shows it. Found by name and role as anything
    /// is, so `click` can send that; said in `tree` with no place, kept
    /// with --visible-only.
    #[test]
    fn a_tab_with_no_place_is_done_by_its_command() {
        let tab = |id, name: &str, url: &str, on: bool| commands::UiNode {
            id,
            parent: Some(1),
            role: "Tab".into(),
            name: Some(name.into()),
            value: Some(url.into()),
            enabled: true,
            clickable: true,
            selected: Some(on),
            command: Some(format!("tab {}", 40 + id)),
            ..Default::default()
        };
        let window = commands::UiNode { id: 1, role: "Window".into(), name: Some("Docs".into()), width: 800.0, height: 600.0, enabled: true, ..Default::default() };
        let t = ScreenTree {
            window: 7,
            app_id: "google-chrome".into(),
            title: "Docs - Google Chrome".into(),
            focused: true,
            content: Frame { x: 0, y: 0, width: 800, height: 600 },
            nodes: vec![window, tab(2, "Docs", "https://docs.example/", true), tab(3, "Mail", "https://mail.example/", false)],
            popups: Vec::new(),
        };
        let found = find::matches(std::slice::from_ref(&t), &find::Query { role: Some("Tab".into()), name: Some("Mail".into()), ..Default::default() });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].command.as_deref(), Some("tab 43"));
        let o = outline(std::slice::from_ref(&t), &Filter::default());
        assert!(o.contains("Tab \"Mail\" value=\"https://mail.example/\" clickable\n"), "{o}");
        assert!(o.contains("Tab \"Docs\" value=\"https://docs.example/\" clickable selected\n"), "{o}");
        let seen = outline(std::slice::from_ref(&t), &Filter { visible_only: true, ..Default::default() });
        assert!(seen.contains("Tab \"Mail\""), "{seen}");
    }

    /// Where, as agents write it: `X Y`, `X,Y`, `--at X,Y`; anything else
    /// says how (it was "x must be a number" ten times on one screen).
    #[test]
    fn points_as_agents_write_them() {
        assert_eq!(point(&args(&["s", "600", "500"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "600,500"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "--at", "600, 500", "--double"]), "click"), Ok((600.0, 500.0)));
        assert_eq!(point(&args(&["s", "--right", "-5", "7.5"]), "click"), Ok((-5.0, 7.5)));
        assert!(point(&args(&["s", "--at", "600"]), "click").unwrap_err().contains("X,Y"));
        assert!(point(&args(&["s"]), "move").unwrap_err().contains("--at X,Y"));
        // (--dx and --dy take values: not where.)
        assert_eq!(positionals(&args(&["s", "--dx", "3", "--dy", "900"])), args(&["s"]));
    }

    #[test]
    fn review2_f42_start_and_history_reject_invalid_and_option_names_before_fs() {
        // Invalid/path-shaped/single-dash names must be rejected before
        // reading kept options or clearing history, never touching disk.
        for bad in ["../escape", "a/b", "..", "-bad"] {
            let err = cli(&args(&["start", bad])).expect("start returns Some").unwrap_err();
            assert!(err.contains("can't name a screen"), "start {bad:?} -> {err}");
        }
        for bad in ["--clear", "../escape", "-bad"] {
            let err = cli(&args(&["history", bad])).expect("history returns Some").unwrap_err();
            assert!(err.contains("can't name a screen"), "history {bad:?} -> {err}");
        }
        let err = cli(&args(&["history", "../escape", "--clear"])).expect("history returns Some").unwrap_err();
        assert!(err.contains("can't name a screen"), "history ../escape --clear -> {err}");
    }

    #[test]
    fn step6_watch_rejects_flag_and_accepts_host_target() {
        assert!(cli(&args(&["watch", "dev-box:review"])).is_none(), "watch HOST:NAME hands off to window");
        assert!(cli(&args(&["watch", "lab:west:qa"])).is_none(), "watch HOST_WITH_COLON:NAME splits on last colon");
        assert!(cli(&args(&["watch", "agent"])).is_none(), "watch NAME hands off to window");
        let err = cli(&args(&["watch", "--bogus"])).expect("watch --bogus fails cleanly").unwrap_err();
        assert!(err.contains("unknown watch option"), "{err}");
        // Finding 32: reject malformed targets and extra operands.
        assert!(cli(&args(&["watch", "a", "b"])).expect("extra operand fails").is_err());
        assert!(cli(&args(&["watch", ":qa"])).expect("missing host fails").is_err());
        assert!(cli(&args(&["watch", "dev-box:../bad"])).expect("invalid screen fails").is_err());
    }

    #[test]
    fn round3_f15_f32_cli_list_arity_host_resolution_and_sanitization() {
        use olo_screen_pane::rex_sock::{HostScreenState, RexHost, RexHostConn, RexScreen};
        // Finding 32: bare `--host` without value fails instead of returning local JSON.
        assert!(parse_list_cli_args(&args(&["--host"])).is_err());
        assert!(parse_list_cli_args(&args(&["--host", "--json"])).is_err());
        assert!(parse_list_cli_args(&args(&["--all", "--host", "dev"])).is_err());
        assert!(parse_list_cli_args(&args(&["--bogus"])).is_err());
        assert_eq!(parse_list_cli_args(&args(&["--host", "dev-box", "--json"])).unwrap(), ListCliArgs { all: false, as_json: true, host_filter: Some("dev-box".into()) });

        // Exact label beats earlier case-insensitive match; ambiguity rejected.
        let mk = |key: &str, label: &str| RexHost {
            key: key.into(),
            label: label.into(),
            state: RexHostConn::Connected,
            error: None,
            disconnected_at: None,
            installed: Some("1.0".into()),
            committed: None,
            default_on_host: true,
            not_set_up_reason: None,
            can_control: Some(true),
            control_disabled_reason: None,
            capabilities: Vec::new(),
            screens: Vec::new(),
        };
        let hosts = vec![mk("k1", "Build"), mk("k2", "build")];
        assert_eq!(resolve_rex_host(&hosts, "build").unwrap().key, "k2", "exact label must beat earlier case-insensitive match");
        assert_eq!(resolve_rex_host(&hosts, "k1").unwrap().key, "k1", "exact key match");
        assert!(resolve_rex_host(&hosts, "BUILD").is_err(), "ambiguous case-insensitive match must fail");

        // Finding 15: text-mode output sanitizes terminal escape/control sequences.
        let mut hostile = mk("k3", "dev\x1b[31m-box\n");
        hostile.screens.push(RexScreen {
            name: "qa".into(),
            title: Some("Check\x1b]0;pwn\x07\tout".into()),
            state: HostScreenState::Running,
            width: 1440,
            height: 900,
            scale: 1.0,
            sound: false,
            pid: None,
            started: None,
            last_used: None,
            actions: None,
            controlled: false,
            owner: None,
            started_by: None,
            controlled_by: None,
        });
        let text = format_host_text(&hostile, std::slice::from_ref(&hostile));
        assert!(!text.contains('\x1b') && !text.contains('\x07'), "control chars must be sanitized: {text:?}");
    }

    #[test]
    fn final_pass_b3_b10_strict_guard_cli_and_colliding_host_key_listing() {
        use olo_screen_pane::rex_sock::{HostScreenState, RexHost, RexHostConn, RexScreen};

        // B.3 (#2–#4): malformed/duplicate/unknown guard flags MUST fail cleanly before
        // dispatching unguarded delete/stop/history!
        for bad in [
            &["delete", "qa", "--expected-started=0"][..],
            &["delete", "qa", "--expected-started", "1", "--expected-started", "2"],
            &["delete", "qa", "--expected-started", "abc"],
            &["delete", "qa", "--bogus"],
            &["stop", "qa", "--expected-started=0"],
            &["stop", "qa", "--expected-last-used"],
            &["history", "qa", "--expected-started", "0"],
            &["history", "qa", "--clear", "--expected-started=0"],
            &["history", "qa", "--clear", "--clear"],
        ] {
            let err = cli(&args(bad)).expect("strict guard error returns Some").unwrap_err();
            assert!(!err.is_empty(), "expected error for {bad:?}");
        }

        // B.10 (#32): host 1 has key="lab" (label="Lab Primary"), host 2 has key="k2" (label="lab");
        // host 2's text listing MUST print `k2:qa` (never `lab:qa`, which resolves to host 1!).
        let mk_screen = |name: &str| RexScreen {
            name: name.into(),
            title: None,
            state: HostScreenState::Running,
            width: 1280,
            height: 800,
            scale: 1.0,
            sound: false,
            pid: None,
            started: None,
            last_used: None,
            actions: None,
            controlled: false,
            owner: None,
            started_by: None,
            controlled_by: None,
        };
        let mk_host = |key: &str, label: &str| RexHost {
            key: key.into(),
            label: label.into(),
            state: RexHostConn::Connected,
            error: None,
            disconnected_at: None,
            installed: Some("1.0".into()),
            committed: None,
            default_on_host: true,
            not_set_up_reason: None,
            can_control: Some(true),
            control_disabled_reason: None,
            capabilities: Vec::new(),
            screens: vec![mk_screen("qa")],
        };
        let all = vec![mk_host("lab", "Lab Primary"), mk_host("k2", "lab"), mk_host("a1", "prod"), mk_host("a2", "prod")];
        let h2_text = format_host_text(&all[1], &all);
        assert!(h2_text.contains("k2:qa"), "colliding label must print stable host key: {h2_text}");
        assert!(!h2_text.contains("  lab:qa"), "must not print lab:qa for host k2: {h2_text}");
        let prod1_text = format_host_text(&all[2], &all);
        let prod2_text = format_host_text(&all[3], &all);
        assert!(prod1_text.contains("a1:qa") && prod2_text.contains("a2:qa"), "duplicate labels must print stable keys");
    }

    #[test]
    fn agy_da27_f3_take_result_json_includes_pid_and_started_for_rex_owner_tracking() {
        let s = screens::Screen {
            name: "agent".into(),
            wayland_display: "wayland-test".into(),
            olo_socket: PathBuf::from("/var/tmp/olo-test.sock"),
            pid: 4242,
            width: 1280,
            height: 800,
            scale: 1.0,
            sound: false,
            started: 1_750_000_123,
            bus: None,
            a11y: None,
        };
        let outcome = moving::Outcome { went: vec![moving::Went { pid: 100, now: 100, app_id: "olo.styled-edit".into(), windows: vec![7], how: "kit", note: None }], stayed: Vec::new() };
        let v = take_result_json(&s, &outcome);
        assert_eq!(v["name"], "agent");
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["started"], 1_750_000_123u64);
        assert_eq!(v["went"].as_array().map(Vec::len), Some(1));
        assert_eq!(v["stayed"], json!([]));
    }

    #[test]
    fn astra_r2_f7_to_f12_f24_neon_f9_direct_local_cli_owner_attribution() {
        let tmp = std::path::PathBuf::from(format!("/var/tmp/olo-screen-owner-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rt_dir = tmp.join("rt");
        let state_root = tmp.join("state");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let my_pid = std::process::id() as i32;
        let my_ticks = proc_start_ticks_of(my_pid);
        std::fs::write(rt_dir.join("qa.json"), format!(r#"{{"name":"qa","pid":{my_pid},"started":1750000100}}"#)).unwrap();

        // Neon #9: `--quick` stripped from any arg position for non-run actions.
        let (stripped, quick) = strip_quick_flag("click", &args(&["--quick", "qa", "100", "200"]));
        assert!(quick && stripped.first().map(String::as_str) == Some("qa"), "got {stripped:?}");
        // A run's history has its program first, `--quick` or not (its
        // stats count both as one program).
        let (stripped, quick) = strip_quick_flag("run", &args(&["qa", "--quick", "env", "--quick"]));
        assert!(quick && stripped == args(&["qa", "env", "--quick"]), "got {stripped:?}");
        assert_eq!(history::run_program(&stripped[1..]), Some("env"));
        // What's started and what's counted are one: `--` ends run's
        // options, and a program named "-report" is that program.
        let (stripped, _) = strip_quick_flag("run", &args(&["qa", "--quick", "--", "-report", "now"]));
        assert_eq!(history::run_argv(&stripped[1..]), args(&["-report", "now"]));
        assert_eq!(history::run_program(&stripped[1..]), Some("-report"));

        // Astra #9: `run` child PID must NOT be extracted as screen session PID; `start`/`take` MUST.
        assert_eq!(answer_screen_gen("run", Some(&json!({"pid": 999999}))), (None, None));
        assert_eq!(answer_screen_gen("record-video", Some(&json!({"pid": 999999}))), (None, None));
        assert_eq!(answer_screen_gen("start", Some(&json!({"name": "qa", "pid": my_pid, "started": 1750000100u64}))), (Some(my_pid), Some(1750000100)));
        assert_eq!(answer_screen_gen("take", Some(&json!({"name": "qa", "pid": my_pid, "started": 1750000100u64}))), (Some(my_pid), Some(1750000100)));

        // Astra #12: read REX_SERVER_SOCKET (stripping unix://) and ignore REX_SOCKET.
        let env_lead = |k: &str| match k {
            "REX_BLOCK" => Some("blk-lead".into()),
            "REX_SESSION" => Some("sess-1".into()),
            "REX_SOCKET" => Some("/wrong/rex-socket.sock".into()),
            "REX_SERVER_SOCKET" => Some("unix:///run/user/1000/rex/server.sock".into()),
            _ => None,
        };
        let gen1 = ScreenGen { pid: my_pid, started: Some(1_750_000_100), start_ticks: my_ticks };
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "start", &args(&["qa"]), "qa", Some(gen1), Duration::from_millis(200), env_lead).unwrap();
        let rt_o = read_direct_owner(&rt_dir.join("qa.owner.json")).expect("runtime owner written");
        let st_o = read_direct_owner(&state_root.join("qa/owner.json")).expect("state owner written");
        assert_eq!(rt_o, st_o);
        assert_eq!(rt_o.session_id, "sess-1");
        assert_eq!(rt_o.block_id, "blk-lead");
        assert_eq!(rt_o.rex_socket.as_deref(), Some("/run/user/1000/rex/server.sock"));
        assert_eq!(rt_o.screen_pid, Some(my_pid));
        assert_eq!(rt_o.screen_started, Some(1_750_000_100));
        assert_eq!(rt_o.screen_start_ticks, my_ticks);
        assert_eq!(rt_o.started_by, Some(DirectOwnerPane { host_key: None, session_id: "sess-1".into(), block_id: "blk-lead".into() }));

        // Astra #24: repeated click from same pane skips rewriting unchanged sidecars (timestamp unchanged).
        let at_before = rt_o.at;
        std::thread::sleep(Duration::from_millis(5));
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "click", &args(&["qa", "10", "20"]), "qa", Some(gen1), Duration::from_millis(200), env_lead).unwrap();
        let rt_same = read_direct_owner(&rt_dir.join("qa.owner.json")).unwrap();
        assert_eq!(rt_same.at, at_before, "unchanged sidecar must not be rewritten");

        // Subsequent action from child pane hands over `block_id` while preserving `started_by`.
        let env_child = |k: &str| match k {
            "REX_BLOCK" => Some("blk-child".into()),
            "REX_SESSION" => Some("sess-1".into()),
            "REX_SERVER_SOCKET" => Some("/run/user/1000/rex/server.sock".into()),
            _ => None,
        };
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "click", &args(&["qa", "10", "20"]), "qa", Some(gen1), Duration::from_millis(200), env_child).unwrap();
        let after = read_direct_owner(&rt_dir.join("qa.owner.json")).expect("updated runtime owner");
        assert_eq!(after.block_id, "blk-child");
        assert_eq!(after.started_by, Some(DirectOwnerPane { host_key: None, session_id: "sess-1".into(), block_id: "blk-lead".into() }));

        // Astra #10: delayed click from Gen 1 after Gen 2 started MUST NOT clobber Gen 2's owner.
        std::fs::write(rt_dir.join("qa.json"), format!(r#"{{"name":"qa","pid":{my_pid},"started":1750000200}}"#)).unwrap();
        let gen2 = ScreenGen { pid: my_pid, started: Some(1_750_000_200), start_ticks: my_ticks };
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "start", &args(&["qa"]), "qa", Some(gen2), Duration::from_millis(200), env_child).unwrap();
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "click", &args(&["qa", "10", "20"]), "qa", Some(gen1), Duration::from_millis(200), env_lead).unwrap();
        let gen2_owner = read_direct_owner(&rt_dir.join("qa.owner.json")).unwrap();
        assert_eq!(gen2_owner.block_id, "blk-child", "delayed Gen 1 click must not overwrite Gen 2");
        assert_eq!(gen2_owner.screen_started, Some(1_750_000_200));

        // Astra #11: verified standalone Start outside Rex clears BOTH runtime AND persistent owner.json.
        std::fs::write(rt_dir.join("qa.json"), format!(r#"{{"name":"qa","pid":{my_pid},"started":1750000300}}"#)).unwrap();
        let gen3 = ScreenGen { pid: my_pid, started: Some(1_750_000_300), start_ticks: my_ticks };
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "start", &args(&["qa"]), "qa", Some(gen3), Duration::from_millis(200), |_| None).unwrap();
        assert!(!rt_dir.join("qa.owner.json").exists(), "standalone start must clear runtime owner");
        assert!(!state_root.join("qa/owner.json").exists(), "standalone start must clear persistent owner so stopping cannot resurrect old pane");

        // Astra #8: bounded lock timeout returns TimedOut instead of hanging, AND non-start outside Rex skips lock completely!
        let rt_fd = direct_open_verified_owned_dir(&rt_dir, false).unwrap();
        let held_lock = direct_acquire_lock_at(&rt_fd, "qa.lifecycle.lock", Duration::from_millis(100)).unwrap();
        // Outside Rex click succeeds immediately without touching the lock:
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "click", &args(&["qa", "1", "1"]), "qa", Some(gen3), Duration::from_millis(30), |_| None).unwrap();
        // Inside Rex click times out promptly:
        let err = record_direct_cli_owner_with_env(&rt_dir, &state_root, "click", &args(&["qa", "1", "1"]), "qa", Some(gen3), Duration::from_millis(40), env_lead).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        drop(held_lock);

        // Astra #7: symlinked per-screen state directory is refused and nothing is written outside state_root.
        let escape_dir = tmp.join("escape");
        std::fs::create_dir_all(&escape_dir).unwrap();
        std::os::unix::fs::symlink(&escape_dir, state_root.join("evil")).unwrap();
        std::fs::write(rt_dir.join("evil.json"), format!(r#"{{"name":"evil","pid":{my_pid},"started":1750000400}}"#)).unwrap();
        let gen_evil = ScreenGen { pid: my_pid, started: Some(1_750_000_400), start_ticks: my_ticks };
        let sym_err = record_direct_cli_owner_with_env(&rt_dir, &state_root, "start", &args(&["evil"]), "evil", Some(gen_evil), Duration::from_millis(100), env_lead).unwrap_err();
        assert!(!escape_dir.join("owner.json").exists(), "must never write through symlinked screen dir (err={sym_err})");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn r3_polish_astra_f20_neon_f5_f6_symlink_ancestor_world_writable_and_rename_no_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = std::path::PathBuf::from(format!("/var/tmp/olo-screen-r3-polish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 1. Astra #20 / Neon #5: symlinked ancestor (modeling Fedora Atomic `/home -> /var/home`)
        //    to a safe owned target succeeds, while a world-writable non-sticky ancestor is rejected!
        let real_home = tmp.join("var_home/alice");
        std::fs::create_dir_all(real_home.join("screens")).unwrap();
        let link_home = tmp.join("home");
        std::os::unix::fs::symlink(tmp.join("var_home"), &link_home).unwrap();
        let via_symlink = link_home.join("alice/screens");
        direct_open_verified_owned_dir(&via_symlink, false).expect("symlinked safe ancestor like /home -> /var/home must succeed");

        let ww_parent = tmp.join("world_writable");
        std::fs::create_dir_all(ww_parent.join("child")).unwrap();
        std::fs::set_permissions(&ww_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let ww_err = direct_open_verified_owned_dir(&ww_parent.join("child"), false).unwrap_err();
        assert_eq!(ww_err.kind(), std::io::ErrorKind::PermissionDenied, "world-writable non-sticky ancestor must be rejected (err={ww_err})");
        std::fs::set_permissions(&ww_parent, std::fs::Permissions::from_mode(0o700)).unwrap();

        // 2. Neon #6: `rename` is NOT a mutating ownership-transfer command — neither running
        //    nor stopped screens may have `owner.json` created or overwritten on `rename`.
        let rt_dir = tmp.join("rt");
        let state_root = tmp.join("state");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(state_root.join("stopped-qa")).unwrap();
        let my_pid = std::process::id() as i32;
        let my_ticks = proc_start_ticks_of(my_pid);
        std::fs::write(rt_dir.join("running-qa.json"), format!(r#"{{"name":"running-qa","pid":{my_pid},"started":1750000100}}"#)).unwrap();
        std::fs::write(state_root.join("stopped-qa/screen.json"), r#"{"width":800,"height":600,"scale":1.0}"#).unwrap();
        let env_other = |k: &str| match k {
            "REX_BLOCK" => Some("blk-renamer".into()),
            "REX_SESSION" => Some("sess-2".into()),
            _ => None,
        };
        let screen_gen = ScreenGen { pid: my_pid, started: Some(1_750_000_100), start_ticks: my_ticks };
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "rename", &args(&["running-qa", "New Title"]), "running-qa", Some(screen_gen), Duration::from_millis(100), env_other).unwrap();
        assert!(!rt_dir.join("running-qa.owner.json").exists(), "rename on running screen must not write runtime owner");
        assert!(!state_root.join("running-qa/owner.json").exists(), "rename on running screen must not write state owner");
        record_direct_cli_owner_with_env(&rt_dir, &state_root, "rename", &args(&["stopped-qa", "New Title"]), "stopped-qa", None, Duration::from_millis(100), env_other).unwrap();
        assert!(!state_root.join("stopped-qa/owner.json").exists(), "rename on stopped screen must not write state owner");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
