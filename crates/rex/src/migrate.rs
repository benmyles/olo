//! An account moved from one host to another (Ben, 2026-11-25: "i want the
//! ability to migrate a rex user from one host to another, and it should
//! copy all the user's home dir files including agent
//! settings/logs/auth/state etc and just work seamlessly … machine to
//! machine copy (don't go via the user's desktop) … start back up the
//! agent sessions on the new host including custom terminal panes … and
//! so for the user the action session should then move to the new server
//! in the rex client"; "it must also support the case where the source is
//! running eg ubuntu and the dest is eg fedora").
//!
//! Built a piece at a time, each usable on its own. What each host says
//! of itself for a move ([`Facts`], from Olo's helper there: `move look`),
//! each pane's programs ([`Pane`], `move look-pane`), and the command line
//! an agent comes back with on the destination ([`resume_line`]). Read as
//! the account, changing nothing.

use serde::Deserialize;

/// The helper's command on a host for a move's look at it (its build
/// `helper_id`, as `helper::file` names it).
pub fn look_command(helper_id: &str) -> String {
    format!("{} move look", crate::helper::file(helper_id))
}

/// The helper's command for each pane's programs, by the pids Rex's report
/// gives.
pub fn look_pane_command(helper_id: &str, pids: &[u32]) -> String {
    let mut c = format!("{} move look-pane", crate::helper::file(helper_id));
    for p in pids {
        c.push_str(&format!(" {p}"));
    }
    c
}

/// What a host says of itself for a move (the helper's `move look`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Facts {
    /// `/etc/os-release`'s `ID` ("ubuntu", "fedora"), `ID_LIKE` and
    /// `PRETTY_NAME`.
    pub system: String,
    pub system_like: String,
    pub system_name: String,
    /// `uname -m`: a program built for one machine doesn't run on another.
    pub machine: String,
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    /// The account's home: where it differs, paths in what moves are
    /// rewritten. Its login shell.
    pub home: String,
    pub shell: String,
    /// Room on the home's filesystem (bytes, inodes: the account's), and
    /// on `/var/tmp`'s.
    pub free: u64,
    pub free_inodes: u64,
    pub var_tmp_free: u64,
    pub var_tmp_inodes: u64,
    /// The account is an administrator there (in `wheel`, `sudo` or
    /// `admin`): told by its groups, sudo never tried to look (a refused
    /// sudo may be logged, or mailed to the host's administrators).
    pub admin: bool,
    pub groups: Vec<String>,
    /// The programs a move may use that are on the path there.
    pub tools: Vec<String>,
    /// SELinux is on: what is copied is labelled again (`restorecon`).
    pub selinux: bool,
    /// The account's lingering is on (its user manager runs logged out).
    pub linger: bool,
    /// The account's own systemd units (its `~/.config/systemd/user`: not
    /// the system's user units) and those enabled.
    pub units: Vec<String>,
    pub enabled: Vec<String>,
    /// The TCP ports its programs listen on.
    pub listening: Vec<u16>,
    /// Its other logins (SSH sessions but the look's own).
    pub other_logins: u32,
    /// Filesystems mounted below the home (the copy doesn't go into them).
    pub mounts: Vec<String>,
    /// A tmux or screen server of its (their sessions end with the move).
    pub tmux: bool,
    pub screen: bool,
    /// What tells this host and home from another (the same account on the
    /// same machine is no move).
    pub machine_id: String,
    pub boot_id: String,
    pub home_dev: Option<u64>,
    pub home_ino: Option<u64>,
    /// The port its SSH server answered the look on (a forwarded port the
    /// app reaches it by isn't what another host would use).
    pub ssh_port: Option<u16>,
    /// Its clock (seconds since the epoch), to compare.
    pub now: u64,
    pub helper: String,
}

impl Facts {
    /// What `move look` printed.
    pub fn read(output: &str) -> Result<Facts, String> {
        let f: Facts = serde_json::from_str(output.trim()).map_err(|e| format!("the host's look didn't read: {e}"))?;
        if f.user.is_empty() || f.home.is_empty() {
            return Err("the host didn't say who it is (no account or home)".into());
        }
        Ok(f)
    }

    /// Whether program `name` is on the path there.
    pub fn has(&self, name: &str) -> bool {
        self.tools.iter().any(|h| h == name)
    }

    /// Whether a program built on `other`'s machine runs here.
    pub fn same_machine(&self, other: &Facts) -> bool {
        self.machine == other.machine
    }

    /// Whether `other` is this same account and home (a move to itself).
    pub fn same_home(&self, other: &Facts) -> bool {
        self.machine_id == other.machine_id && self.boot_id == other.boot_id && self.uid == other.uid && self.home_dev == other.home_dev && self.home_ino == other.home_ino
    }
}

/// A pane's process as the helper's `move look-pane` says it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Program {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Its whole command line, the program first.
    pub argv: Vec<String>,
    pub cwd: String,
    /// When it started, in clock ticks after boot.
    pub start: u64,
    /// The agents' session files it holds open.
    pub open: Vec<String>,
}

/// The agent in a pane and the session it is in, as the helper found them
/// (`how`: "open", the file it holds; "written", Claude Code's transcript
/// written since it started; "argv", its command line's; "guess", the
/// newest of its folder; "none").
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PaneAgent {
    pub agent: String,
    pub pid: u32,
    pub argv: Vec<String>,
    pub cwd: String,
    pub session: Option<String>,
    pub how: String,
}

/// A pane: its process (as Rex's report gives it) and everything under it,
/// parents first; the agent there, if one is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Pane {
    pub pid: u32,
    pub tree: Vec<Program>,
    pub agent: Option<PaneAgent>,
}

impl Pane {
    /// What `move look-pane` printed.
    pub fn read_all(output: &str) -> Result<Vec<Pane>, String> {
        serde_json::from_str(output.trim()).map_err(|e| format!("the panes' look didn't read: {e}"))
    }

    /// The agent's own session the pane holds open, anywhere in its tree:
    /// Codex's rollout's id (`rollout-STAMP-ID.jsonl`), agy's
    /// conversation's (`ID.db`, its log beside it).
    pub fn open_session(&self) -> Option<String> {
        self.tree.iter().flat_map(|p| p.open.iter()).find_map(|f| {
            let name = f.rsplit('/').next()?;
            for log in [".db", ".db-wal", ".db-shm"] {
                if let Some(stem) = name.strip_suffix(log) {
                    return Some(stem.to_owned());
                }
            }
            let stem = name.strip_suffix(".jsonl")?;
            // (The id is a UUID: its last five dash-separated parts.)
            let parts: Vec<&str> = stem.split('-').collect();
            (parts.len() >= 5).then(|| parts[parts.len() - 5..].join("-"))
        })
    }
}

/// What a move never copies, from the home's root (`/` anchors a pattern
/// there): what the destination keeps as its own (its `authorized_keys`),
/// Olo's and Rex's own per-host state (each host is given its own: the
/// helper for its machine, Rex's server, the Token Pool's program and its
/// vault, which moves by the pool's own hand-back, never copied), what
/// is the move's own, and what can't move or is the machine's (sockets'
/// folders, locks, editors' servers for one machine, containers' storage
/// that a user can't copy). `.cache` too unless asked (a cache, rebuilt;
/// another system's caches don't fit). Another system or machine: Codex's
/// daemon's packages (it fetches its own).
pub fn excluded(from: &Facts, to: &Facts, bring_cache: bool) -> Vec<String> {
    let mut out: Vec<String> = [
        "/.ssh/authorized_keys",
        "/.ssh/authorized_keys2",
        "/.local/share/olo/rex-link",
        "/.local/state/olo-rex/move",
        "/.local/state/olo-rex/moved",
        "/.local/state/olo-rex/agent.sock",
        "/.local/state/rex",
        "/.local/share/olo/pool",
        "/.config/olo/pool.*",
        "/.config/olo/pool-ca*",
        "/.config/olo/pool-clients.json",
        "/.local/state/olo/pool-*",
        "/.config/systemd/user/olo-pool.service",
        "/.config/systemd/user/rex.service",
        "/.config/systemd/user/rex.socket",
        "/.config/systemd/user/*.wants/olo-pool.service",
        "/.config/systemd/user/*.wants/rex.service",
        "/.config/systemd/user/*.wants/rex.socket",
        "/.cache/olo/rex-sync",
        "/.codex/app-server-daemon",
        "/.codex/app-server-control",
        "/.claude/ide",
        "/.gnupg/*.lock",
        "/.local/state/nvim/swap",
        "/.local/share/Trash",
        "/.local/share/containers/storage",
        "/.local/share/docker",
        "/.vscode-server",
        "/.cursor-server",
    ]
    .map(String::from)
    .to_vec();
    if !bring_cache {
        out.push("/.cache".into());
    }
    if from.home != to.home {
        // (Codex's database names its sessions by path: made again there
        // from the rewritten sessions, and told of them.)
        out.push("/.codex/state_*.sqlite*".into());
        out.push("/.zcompdump*".into());
    }
    if from.system != to.system || !from.same_machine(to) {
        out.push("/.codex/packages/app-server-daemon".into());
    }
    out
}

/// Why a move from `from` to `to` can't be made, each a sentence for the
/// person (none: it can). `bytes` and `files`: what the source's home
/// holds that goes (its look's count).
pub fn refusals(from: &Facts, to: &Facts, bytes: u64, files: u64) -> Vec<String> {
    let mut why = Vec::new();
    if from.same_home(to) {
        why.push("That's the same account on the same machine.".to_owned());
    }
    let apart = from.now.abs_diff(to.now);
    if from.now > 0 && to.now > 0 && apart > 60 {
        why.push(format!("The two hosts' clocks are {apart} seconds apart: set them right first (the copy tells changed files by their times)."));
    }
    let needed = bytes + bytes / 10 + (1 << 30);
    if to.free < needed {
        why.push(format!("{} has {} free in its home; the move needs {}.", to.system_name_or_host(), size_words(to.free), size_words(needed)));
    }
    if to.free_inodes > 0 && to.free_inodes < files + 10_000 {
        why.push(format!("{} has room for {} more files in its home; the move brings {files}.", to.system_name_or_host(), to.free_inodes));
    }
    why
}

impl Facts {
    fn system_name_or_host(&self) -> String {
        if self.system_name.is_empty() { "The destination".to_owned() } else { format!("The destination ({})", self.system_name) }
    }
}

/// Bytes as the person reads them ("12.3 GB").
pub fn size_words(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..1_000_000 => format!("{:.0} KB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.0} MB", b / 1e6),
        _ => format!("{:.1} GB", b / 1e9),
    }
}

/// The units a move stops and disables on the source and enables on the
/// destination: the account's own (not Rex's or the pool's: each host is
/// given those as any host is).
pub fn own_units(facts: &Facts) -> Vec<String> {
    facts.units.iter().filter(|u| !(u.starts_with("rex.") || u.starts_with("olo-pool."))).cloned().collect()
}

/// The source stopped, in the order that keeps it stopped (as the
/// account, a POSIX shell's): its crontab saved in the move's folder and
/// emptied; its own timers, paths and sockets (they start services), then
/// its services, stopped and disabled, Rex's socket and server with them;
/// the agents asked to end and given ten seconds (their screens first:
/// Codex's live windows start its daemon again), then Codex's daemon by
/// its pid file; tmux and screen; rootless containers. Lingering stays
/// on (a detached second pass needs the account's systemd) and is turned
/// off after it. `agents`: the agents' pids, as the panes' look found them.
pub fn stop_script(from: &Facts, dir: &str, agents: &[u32]) -> String {
    let units = own_units(from);
    let starters: Vec<&String> = units.iter().filter(|u| u.ends_with(".timer") || u.ends_with(".path") || u.ends_with(".socket")).collect();
    let services: Vec<&String> = units.iter().filter(|u| u.ends_with(".service") || u.ends_with(".target")).collect();
    let words = |list: &[&String]| list.iter().map(|u| crate::provision::shell_word(u)).collect::<Vec<_>>().join(" ");
    let mut s = format!("d={}\nmkdir -p \"$d\"\n", crate::provision::shell_word(dir));
    if from.has("crontab") {
        s.push_str("if crontab -l > \"$d/crontab\" 2> /dev/null; then crontab -r; else rm -f \"$d/crontab\"; fi\n");
    }
    if from.has("systemctl") {
        s.push_str("export XDG_RUNTIME_DIR=\"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}\"\n");
        if !starters.is_empty() {
            let w = words(&starters);
            s.push_str(&format!("systemctl --user stop {w} 2> /dev/null; systemctl --user disable {w} 2> /dev/null\n"));
        }
        s.push_str("systemctl --user stop rex.socket 2> /dev/null; systemctl --user disable rex.socket 2> /dev/null\n");
        if !services.is_empty() {
            let w = words(&services);
            s.push_str(&format!("systemctl --user stop {w} 2> /dev/null; systemctl --user disable {w} 2> /dev/null\n"));
        }
    }
    if !agents.is_empty() {
        let pids = agents.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
        s.push_str(&format!("kill -TERM {pids} 2> /dev/null\nn=0; while [ $n -lt 100 ] && kill -0 {pids} 2> /dev/null; do sleep 0.1; n=$((n + 1)); done\n"));
    }
    s.push_str("p=\"$HOME/.codex/app-server-daemon/daemon.pid\"; [ -r \"$p\" ] && kill -TERM \"$(cat \"$p\")\" 2> /dev/null\n");
    if from.tmux {
        s.push_str("command -v tmux > /dev/null && tmux kill-server 2> /dev/null\n");
    }
    if from.screen {
        s.push_str("command -v screen > /dev/null && screen -ls 2> /dev/null | awk '/\\t/ {print $1}' | while read -r x; do screen -S \"$x\" -X quit; done\n");
    }
    if from.has("podman") {
        s.push_str("podman stop -a -t 10 > /dev/null 2>&1\n");
    }
    if from.has("systemctl") {
        s.push_str("systemctl --user stop rex.service 2> /dev/null; systemctl --user disable rex.service 2> /dev/null\n");
    }
    s.push_str("true\n");
    s
}

/// A split to make again: pane `of` (by its number: the window's first is
/// 0) split to make pane `new` beside it (`"right"`) or under it
/// (`"down"`), the split then set to `ratio`.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub of: usize,
    pub new: usize,
    pub direction: &'static str,
    pub ratio: f64,
}

/// A layer's panes as the source had them, made again from one pane: the
/// splits in the order to make them (each splits a pane already made, the
/// old one keeping the first side), and the source's block each new pane
/// stands for (by its number).
pub fn rebuild_splits(layout: &crate::model::LayoutNode) -> (Vec<Split>, Vec<Option<String>>) {
    fn walk(node: &crate::model::LayoutNode, me: usize, splits: &mut Vec<Split>, panes: &mut Vec<Option<String>>) {
        match &node.split {
            Some(split) => {
                let new = panes.len();
                panes.push(None);
                let direction = if split.direction.is_horizontal() { "right" } else { "down" };
                splits.push(Split { of: me, new, direction, ratio: split.ratio });
                walk(&split.before, me, splits, panes);
                walk(&split.after, new, splits, panes);
            }
            None => panes[me] = node.block_id.clone(),
        }
    }
    let (mut splits, mut panes) = (Vec::new(), vec![None]);
    walk(layout, 0, &mut splits, &mut panes);
    (splits, panes)
}

/// What a move tells the agent in one pane ([`message`]).
#[derive(Clone, Debug, Default)]
pub struct Told<'a> {
    /// The two hosts as Rex names them.
    pub from_label: &'a str,
    pub to_label: &'a str,
    pub from: Facts,
    pub to: Facts,
    /// When it was moved, in the person's words ("at 14:05").
    pub when: &'a str,
    /// The old home's path still leads to the new one (a link made).
    pub linked: bool,
    /// The agent was in the middle of a turn when the move stopped it.
    pub cut: bool,
    /// Its folder isn't on the destination (it was outside the home).
    pub folder_gone: Option<&'a str>,
    /// The account's folders in `/var/tmp` came too.
    pub var_tmp: bool,
    /// Ports its programs listened on, on the source.
    pub ports: &'a [u16],
}

/// A system's package manager, as the person would name it.
fn packages(f: &Facts) -> &'static str {
    let like = format!("{} {}", f.system, f.system_like);
    if like.contains("fedora") || like.contains("rhel") || like.contains("centos") {
        "dnf"
    } else if like.contains("debian") || like.contains("ubuntu") {
        "apt"
    } else {
        "its own package manager"
    }
}

/// What each agent finds typed at its prompt on the destination, not sent
/// (the person reads it, may change it, sends it): one line (a newline
/// would send it), no `@` (it opens a mention's chooser), only what is
/// true of this move and this pane.
pub fn message(t: &Told) -> String {
    let clean = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>().replace('@', " at ");
    let host = |label: &str, f: &Facts| {
        let what = [f.system_name.as_str(), f.machine.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ");
        if what.is_empty() { clean(label) } else { format!("{} ({})", clean(label), clean(&what)) }
    };
    let mut m = format!("[Olo] This session was moved from {} to {}", host(t.from_label, &t.from), host(t.to_label, &t.to));
    if !t.when.is_empty() {
        m.push_str(&format!(" {}", clean(t.when)));
    }
    m.push('.');
    if t.from.home != t.to.home {
        m.push_str(&format!(" Your home moved from {} to {}{}.", clean(&t.from.home), clean(&t.to.home), if t.linked { " (the old path still leads here)" } else { "" }));
    }
    m.push_str(if t.var_tmp { " Everything in the home came with it, and your folders in /var/tmp; /tmp did not." } else { " Everything in the home came with it; /tmp and /var/tmp did not." });
    if t.cut {
        m.push_str(" Your last turn was cut off by the move: look at what it was doing before going on.");
    }
    if let Some(folder) = t.folder_gone {
        m.push_str(&format!(" Your folder {} isn't on this host: you are in the home.", clean(folder)));
    }
    let (was, now) = (packages(&t.from), packages(&t.to));
    if t.from.system != t.to.system {
        m.push_str(&format!(" System packages don't move ({was} there, {now} here): check the tools and libraries your work needs are installed."));
    }
    if !t.from.same_machine(&t.to) && !t.to.machine.is_empty() {
        m.push_str(&format!(" This is {} {} machine, not {}: rebuild anything native.", if t.to.machine.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }, t.to.machine, t.from.machine));
    }
    if !t.ports.is_empty() {
        let ports = t.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
        m.push_str(&format!(" Servers were listening there on {} {ports}: start again those you need.", if t.ports.len() == 1 { "port" } else { "ports" }));
    }
    m.push_str(" Rex's ids for this session and pane changed (use $REX_SESSION and $REX_BLOCK). Then carry on with your work.");
    m
}

/// A layer as the source had it: tiled or floating (its bounds, whether it
/// was minimized), its panes' splits ([`rebuild_splits`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct CapturedLayer {
    pub floating: bool,
    pub bounds: [f64; 4],
    pub minimized: bool,
    pub splits: Vec<CapturedSplit>,
    /// The source's block for each pane, by its number.
    pub panes: Vec<Option<String>>,
}

/// [`Split`], kept in a move's journal.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct CapturedSplit {
    pub of: usize,
    pub new: usize,
    pub direction: String,
    pub ratio: f64,
}

/// A window as the source had it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct CapturedWindow {
    pub label: String,
    pub layers: Vec<CapturedLayer>,
    pub focused: Option<String>,
    pub zoomed: Option<String>,
}

/// A pane as the source had it: its label, its top process and the one in
/// front (Rex's report), its folder, its last screen (to tell which of an
/// agent's sessions it shows).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct CapturedPane {
    pub block_id: String,
    pub label: String,
    pub pid: Option<u32>,
    pub foreground: Option<u32>,
    pub cwd: String,
    pub screen: String,
}

/// A session as the source had it, all a move needs to make it again.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct CapturedSession {
    pub label: String,
    pub windows: Vec<CapturedWindow>,
    pub active_window: Option<usize>,
    pub panes: Vec<CapturedPane>,
}

/// A session of the source's, read whole: its windows and layers and how
/// their panes are split, each pane's process and last screen.
pub async fn capture(client: &crate::control::ControlClient, session_id: &str) -> Result<CapturedSession, crate::error::RexError> {
    let view = client.session_view(session_id).await?.payload;
    let mut out = CapturedSession { label: view.label.clone(), ..CapturedSession::default() };
    out.active_window = view.active_window_id.as_ref().and_then(|a| view.windows.iter().position(|w| &w.window_id == a));
    for w in &view.windows {
        let mut window = CapturedWindow { label: w.label.clone(), focused: w.focused_block_id.clone(), zoomed: w.zoomed_block_id.clone(), ..CapturedWindow::default() };
        for layer in &w.layers {
            let (splits, panes) = match &layer.layout {
                Some(layout) => rebuild_splits(layout),
                None => (Vec::new(), layer.blocks.first().map(|b| vec![Some(b.block_id.clone())]).unwrap_or_default()),
            };
            window.layers.push(CapturedLayer {
                floating: layer.layer_kind() == crate::model::LayerKind::Floating,
                bounds: layer.bounds,
                minimized: layer.state.as_deref() == Some("minimized"),
                splits: splits.into_iter().map(|s| CapturedSplit { of: s.of, new: s.new, direction: s.direction.to_owned(), ratio: s.ratio }).collect(),
                panes,
            });
            for b in &layer.blocks {
                // (A terminal's; another kind of block, a forward's, isn't
                // made again.)
                if !b.flavor.is_empty() && !b.flavor.contains("terminal") {
                    continue;
                }
                let process = client.terminal_process(session_id, &b.block_id).await.ok().map(|r| r.payload);
                let screen = client.terminal_format(session_id, &b.block_id, "plain").await.map(|r| r.payload).unwrap_or_default();
                out.panes.push(CapturedPane {
                    block_id: b.block_id.clone(),
                    label: b.label.clone(),
                    pid: process.as_ref().and_then(|p| p.child.as_ref()).map(|c| c.pid),
                    foreground: process.as_ref().and_then(|p| p.foreground.as_ref()).map(|c| c.pid),
                    cwd: process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).and_then(|c| c.cwd.clone()).unwrap_or_default(),
                    screen,
                });
            }
        }
        out.windows.push(window);
    }
    Ok(out)
}

/// What one pane runs on the destination: a shell in `cwd` with
/// `command` typed and run (an agent's resume line, a program's command
/// line; nothing for a shell).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct PaneRun {
    pub cwd: String,
    pub command: Option<Vec<String>>,
}

/// A captured session made again on the destination, as a new session:
/// its windows in order with their labels, each layer's panes split as
/// they were with their ratios (floating layers where they were, minimized
/// ones minimized), labels, the pane in front and a zoomed one, the window
/// in front. Each pane opens a shell in its folder (`runs`, by the
/// source's block; a folder that isn't there, the home). What it answers:
/// the new session's id, and each source block's new block.
pub async fn rebuild(
    client: &crate::control::ControlClient,
    captured: &CapturedSession,
    runs: &std::collections::HashMap<String, PaneRun>,
) -> Result<(String, std::collections::HashMap<String, String>), crate::error::RexError> {
    use serde_json::{Value, json};
    let shell_in = |block: Option<&String>| -> Option<Value> {
        let run = block.and_then(|b| runs.get(b))?;
        (!run.cwd.is_empty()).then(|| json!({ "cwd": run.cwd }))
    };
    let first = |window: &CapturedWindow| window.layers.first().and_then(|l| l.panes.first().cloned().flatten());
    let mut made: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let created = client.session_create(Some(&captured.label), shell_in(captured.windows.first().and_then(first).as_ref())).await?.payload;
    let sid = created.get("session_id").and_then(Value::as_str).ok_or_else(|| crate::error::RexError::Protocol("session.create: no session_id".into()))?.to_owned();
    let mut window_ids = Vec::new();
    for (wi, window) in captured.windows.iter().enumerate() {
        // (The session came with its first window; each other is made.)
        let before: Vec<String> = client.session_view(&sid).await?.payload.windows.iter().map(|w| w.window_id.clone()).collect();
        if wi > 0 {
            client.session_new_window(&sid, Some(&window.label), shell_in(first(window).as_ref())).await?;
        }
        let view = client.session_view(&sid).await?.payload;
        let w = if wi == 0 { view.windows.first() } else { view.windows.iter().find(|w| !before.contains(&w.window_id)) }
            .ok_or_else(|| crate::error::RexError::Protocol("a window made isn't in the session".into()))?
            .clone();
        if !window.label.is_empty() {
            let _ = client.session_set_window_label(&sid, &w.window_id, &window.label).await;
        }
        window_ids.push(w.window_id.clone());
        for (li, layer) in window.layers.iter().enumerate() {
            // (The first layer's first pane came with the window; a
            // floating layer is made with its own.)
            let start = if li == 0 {
                w.layers.first().and_then(|l| l.blocks.first()).map(|b| b.block_id.clone())
            } else {
                let made_layer = client.session_new_layer(&sid, Some(&w.window_id), layer.bounds, shell_in(layer.panes.first().and_then(Option::as_ref))).await?.payload;
                made_layer["block_ids"][0].as_str().or_else(|| made_layer["block_id"].as_str()).map(str::to_owned)
            };
            let Some(start) = start else { continue };
            let mut blocks: Vec<Option<String>> = vec![None; layer.panes.len().max(1)];
            blocks[0] = Some(start);
            for split in &layer.splits {
                let Some(of) = blocks.get(split.of).cloned().flatten() else { continue };
                let answer = client.session_new_split(&sid, Some(&of), &split.direction, shell_in(layer.panes.get(split.new).and_then(Option::as_ref))).await?.payload;
                if let Some(new) = answer["block_ids"][0].as_str() {
                    if split.new < blocks.len() {
                        blocks[split.new] = Some(new.to_owned());
                    }
                }
                if let Some(split_id) = answer["split_ids"][0].as_str() {
                    let _ = client.session_resize_pane(&sid, split_id, split.ratio).await;
                }
            }
            for (source, new) in layer.panes.iter().zip(&blocks) {
                if let (Some(source), Some(new)) = (source, new) {
                    made.insert(source.clone(), new.clone());
                }
            }
            if layer.minimized && li > 0 {
                let view = client.session_view(&sid).await?.payload;
                if let Some(id) = view.windows.iter().find(|x| x.window_id == w.window_id).and_then(|x| x.layers.get(li)).map(|l| l.layer_id.clone()) {
                    let _ = client.session_set_layer_state(&sid, &id, "minimized", false).await;
                }
            }
        }
    }
    // Labels, then each window's pane in front and its zoom (last: a
    // split made after a zoom would undo it), then the window in front.
    for pane in &captured.panes {
        if let Some(new) = made.get(&pane.block_id)
            && !pane.label.is_empty()
        {
            let _ = client.session_set_block_label(&sid, new, &pane.label).await;
        }
    }
    for (window, id) in captured.windows.iter().zip(&window_ids) {
        if let Some(new) = window.focused.as_ref().and_then(|f| made.get(f)) {
            let _ = client.session_focus_block(&sid, new).await;
        }
        if let Some(new) = window.zoomed.as_ref().and_then(|z| made.get(z)) {
            let _ = client.session_zoom_block(&sid, Some(new), Some(true)).await;
        }
        let _ = id;
    }
    if let Some(id) = captured.active_window.and_then(|i| window_ids.get(i)) {
        let _ = client.session_focus_window(&sid, id).await;
    }
    Ok((sid, made))
}

/// A move's own folder on a host, under the account's home (the copy
/// leaves it out; its key, its log, its status, the source's saved
/// crontab).
pub fn move_dir(home: &str, id: &str) -> String {
    format!("{}/.local/state/olo-rex/move/{id}", home.trim_end_matches('/'))
}

/// Runs `command` on a host as the account; what it wrote, if it ended
/// well.
async fn ran<R: crate::provision::Runner>(host: &mut R, command: &str) -> Result<String, String> {
    let ran = host.run(false, command, &mut |_| {}).await.map_err(|f| f.why)?;
    match ran.status {
        Some(0) => Ok(ran.output),
        _ => Err(ran.output.lines().last().unwrap_or("it ended badly").to_owned()),
    }
}

/// The move's key, made on the source (its public line).
pub async fn make_key<R: crate::provision::Runner>(source: &mut R, helper_id: &str, from: &Facts, id: &str) -> Result<String, String> {
    let out = ran(source, &format!("{} move key {}", crate::helper::file(helper_id), crate::provision::shell_word(&move_dir(&from.home, id)))).await?;
    out.lines().find(|l| l.starts_with("ssh-ed25519 ")).map(str::to_owned).ok_or_else(|| format!("the source's helper made no key: {out}"))
}

/// That key let into the destination's account (a day, only the helper's
/// receive).
pub async fn allow<R: crate::provision::Runner>(dest: &mut R, helper_id: &str, id: &str, public: &str) -> Result<(), String> {
    ran(dest, &format!("{} move allow {id} {}", crate::helper::file(helper_id), crate::provision::shell_word(public))).await.map(|_| ())
}

/// Where the source reaches the destination, and how (`send`'s and
/// `reach`'s words).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Way {
    pub user: String,
    pub host: String,
    pub port: u16,
    /// The destination's host key, as the app's own connection checked it.
    pub host_key: String,
}

impl Way {
    fn words(&self, there: &str) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!(
            "--to {} --host-key {} --there {}",
            crate::provision::shell_word(&format!("{}@{host}:{}", self.user, self.port)),
            crate::provision::shell_word(&self.host_key),
            crate::provision::shell_word(there)
        )
    }
}

/// The first of `ways` by which the source reaches the destination with
/// the move's key (nothing stopped yet); `Err` with each one's answer.
pub async fn reach<R: crate::provision::Runner>(source: &mut R, helper_id: &str, from: &Facts, to: &Facts, id: &str, ways: &[Way]) -> Result<Way, String> {
    let mut said = Vec::new();
    for way in ways {
        let command = format!("{} move reach {} {}", crate::helper::file(helper_id), crate::provision::shell_word(&move_dir(&from.home, id)), way.words(&to.home));
        let out = source.run(false, &command, &mut |_| {}).await.map_err(|f| f.why)?;
        let v: serde_json::Value = out.output.lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or_default();
        if v["ok"].as_bool() == Some(true) {
            return Ok(way.clone());
        }
        said.push(format!("{}:{}: {}", way.host, way.port, v["error"].as_str().unwrap_or(out.output.trim())));
    }
    Err(said.join("; "))
}

/// A pass of the copy started on the source, detached (it goes on with
/// the app gone); its pid.
#[allow(clippy::too_many_arguments)]
pub async fn start_pass<R: crate::provision::Runner>(source: &mut R, helper_id: &str, from: &Facts, to: &Facts, id: &str, way: &Way, pass: u32, excluded: &[String]) -> Result<u32, String> {
    let mut command = format!("{} move send {} {} --pass {pass} --detach", crate::helper::file(helper_id), crate::provision::shell_word(&move_dir(&from.home, id)), way.words(&to.home));
    for e in excluded {
        command.push_str(&format!(" --exclude {}", crate::provision::shell_word(e)));
    }
    let out = ran(source, &command).await?;
    let v: serde_json::Value = out.lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or_default();
    v["started"].as_u64().map(|p| p as u32).ok_or_else(|| format!("the copy didn't start: {}", out.trim()))
}

/// How the copy stands (its `status.json`, and whether it still runs).
pub async fn pass_status<R: crate::provision::Runner>(source: &mut R, helper_id: &str, from: &Facts, id: &str) -> Result<serde_json::Value, String> {
    let out = ran(source, &format!("{} move status {}", crate::helper::file(helper_id), crate::provision::shell_word(&move_dir(&from.home, id)))).await?;
    serde_json::from_str(out.trim()).map_err(|e| format!("the copy's status didn't read: {e}"))
}

/// The source stopped and kept stopped ([`stop_script`]), then everything
/// of the account but the helper's own line ended (`move stop`): what is
/// left, if anything (a process in `D`, one a root supervisor starts
/// again).
pub async fn stop_source<R: crate::provision::Runner>(source: &mut R, helper_id: &str, from: &Facts, id: &str, agents: &[u32]) -> Result<Vec<serde_json::Value>, String> {
    ran(source, &format!("sh -c {}", crate::provision::shell_word(&stop_script(from, &move_dir(&from.home, id), agents)))).await?;
    let out = source.run(false, &format!("{} move stop", crate::helper::file(helper_id)), &mut |_| {}).await.map_err(|f| f.why)?;
    let v: serde_json::Value = out.output.lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or_default();
    Ok(v["left"].as_array().cloned().unwrap_or_default())
}

/// The source's lingering turned off (after the second pass: a detached
/// pass needs the account's systemd), by the account where it may, else by
/// root through sudo where it is an administrator.
pub async fn linger_off<R: crate::provision::Runner>(source: &mut R, from: &Facts) -> Result<(), String> {
    if !from.linger {
        return Ok(());
    }
    let mine = source.run(false, "loginctl disable-linger", &mut |_| {}).await.map_err(|f| f.why)?;
    if mine.status == Some(0) {
        return Ok(());
    }
    if from.admin {
        let user = crate::provision::shell_word(&from.user);
        let root = source.run(true, &format!("loginctl disable-linger {user}"), &mut |_| {}).await.map_err(|f| f.why)?;
        if root.status == Some(0) {
            return Ok(());
        }
    }
    Err("its lingering couldn't be turned off (a reboot would start what was stopped)".into())
}

/// A home whose path changed made right on the destination (`move fix`):
/// what it rewrote, renamed, retargeted, and what it only names.
pub async fn fix_destination<R: crate::provision::Runner>(dest: &mut R, helper_id: &str, from: &Facts, to: &Facts, id: &str) -> Result<serde_json::Value, String> {
    if from.home == to.home {
        return Ok(serde_json::json!({}));
    }
    let once = format!("{}/fixed.json", move_dir(&to.home, id));
    let out = ran(
        dest,
        &format!(
            "mkdir -p {} && {} move fix --from {} --to {} --once {}",
            crate::provision::shell_word(&move_dir(&to.home, id)),
            crate::helper::file(helper_id),
            crate::provision::shell_word(&from.home),
            crate::provision::shell_word(&to.home),
            crate::provision::shell_word(&once)
        ),
    )
    .await?;
    serde_json::from_str(out.lines().last().unwrap_or_default()).map_err(|e| format!("the fix's report didn't read: {e}"))
}

/// The crontab the source had, as its stop saved it (none: it had none).
pub async fn saved_crontab<R: crate::provision::Runner>(source: &mut R, from: &Facts, id: &str) -> Result<Option<String>, String> {
    let file = crate::provision::shell_word(&format!("{}/crontab", move_dir(&from.home, id)));
    let out = ran(source, &format!("[ -s {file} ] && cat {file} || true")).await?;
    Ok((!out.trim().is_empty()).then_some(out))
}

/// The account's own units that were enabled on the source enabled (and
/// started) on the destination; its crontab, as the source had it, made
/// there, the old home's paths in it the new home's (`crontab` must be
/// there: else it is kept in the move's folder, and Done says so).
pub async fn units_and_crontab<R: crate::provision::Runner>(dest: &mut R, from: &Facts, to: &Facts, id: &str, crontab: Option<&str>) -> Result<(), String> {
    let wanted: Vec<String> = from.enabled.iter().filter(|u| !(u.starts_with("rex.") || u.starts_with("olo-pool."))).cloned().collect();
    let mut s = String::from("export XDG_RUNTIME_DIR=\"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}\"\nsystemctl --user daemon-reload 2> /dev/null\n");
    if !wanted.is_empty() {
        let w = wanted.iter().map(|u| crate::provision::shell_word(u)).collect::<Vec<_>>().join(" ");
        s.push_str(&format!("systemctl --user enable --now {w} 2>&1 || true\n"));
    }
    if let Some(table) = crontab {
        let table = String::from_utf8_lossy(&crate::sessions::rewritten(table.as_bytes().to_vec(), &from.home, &to.home)).into_owned();
        let saved = format!("{}/crontab", move_dir(&to.home, id));
        // (A delimiter no line of the table is.)
        let mut end = "OLO_MOVE_CRONTAB".to_owned();
        while table.lines().any(|l| l == end) {
            end.push('_');
        }
        s.push_str(&format!("mkdir -p {}\ncat > {} <<'{end}'\n{}\n{end}\n", crate::provision::shell_word(&move_dir(&to.home, id)), crate::provision::shell_word(&saved), table.trim_end_matches('\n')));
        if to.has("crontab") {
            s.push_str(&format!("crontab {}\n", crate::provision::shell_word(&saved)));
        }
    }
    s.push_str("true\n");
    ran(dest, &format!("sh -c {}", crate::provision::shell_word(&s))).await.map(|_| ())
}

/// The move's key taken out of the destination, its folder off the source
/// (its record kept on this computer).
pub async fn clean_up<S: crate::provision::Runner, D: crate::provision::Runner>(source: &mut S, dest: &mut D, helper_id: &str, from: &Facts, id: &str) -> Result<(), String> {
    ran(dest, &format!("{} move disallow {id}", crate::helper::file(helper_id))).await?;
    ran(source, &format!("rm -rf {}", crate::provision::shell_word(&move_dir(&from.home, id)))).await.map(|_| ())
}

/// A program a pane runs that only shows something (a list followed, a
/// log, a monitor): run again on the destination as it was. Any other is
/// not (`git push` running when the source was looked at is never run
/// twice): its command line is left typed at the shell's prompt instead.
pub fn only_shows(argv: &[String]) -> bool {
    let name = argv.first().map(|a| a.rsplit('/').next().unwrap_or(a)).unwrap_or_default();
    matches!(name, "tail" | "less" | "more" | "watch" | "htop" | "top" | "btop" | "glances" | "nvtop" | "journalctl" | "dmesg" | "man" | "olo-worklist")
        || (name == "tail" && argv.iter().any(|a| a == "-f" || a == "-F"))
}

/// What each pane runs on the destination, by the source's block: in its
/// folder (the new home's), an agent resumed in its session with its own
/// flags, a program that only shows something run again, anything else
/// typed and left for the person; a shell, a shell. And the agents' pids
/// on the source (to stop them first). `path`: the old home's paths made
/// the new home's.
pub fn pane_runs(captured: &CapturedSession, panes: &[Pane], path: &dyn Fn(&str) -> String) -> (std::collections::HashMap<String, PaneRun>, Vec<u32>, std::collections::HashMap<String, bool>) {
    let mut runs = std::collections::HashMap::new();
    let mut agents = Vec::new();
    // (Whether each pane's command is to be run, not only typed.)
    let mut run_it = std::collections::HashMap::new();
    for cp in &captured.panes {
        let pane = cp.pid.and_then(|pid| panes.iter().find(|p| p.pid == pid));
        let cwd = path(&cp.cwd);
        let (command, run) = match pane.and_then(|p| p.agent.as_ref()) {
            Some(a) => {
                agents.push(a.pid);
                let line = match &a.session {
                    Some(session) => resume_line(&a.agent, &a.argv, session, path),
                    // (An agent not yet in a session of its own: started
                    // again as it was told, nothing to resume.)
                    None => Some(std::iter::once(a.agent.clone()).chain(a.argv.iter().skip(1).filter(|w| w.starts_with('-')).map(|w| path(w))).collect()),
                };
                (line, true)
            }
            None => {
                // (The program in front, if it isn't the pane's shell.)
                let front = pane.and_then(|p| p.tree.iter().find(|t| Some(t.pid) == cp.foreground && Some(t.pid) != cp.pid).or_else(|| p.tree.first().filter(|t| !is_shell(&t.name))));
                match front {
                    Some(t) if !is_shell(&t.name) => (Some(t.argv.iter().map(|a| path(a)).collect::<Vec<_>>()), only_shows(&t.argv)),
                    _ => (None, false),
                }
            }
        };
        run_it.insert(cp.block_id.clone(), run);
        runs.insert(cp.block_id.clone(), PaneRun { cwd, command });
    }
    (runs, agents, run_it)
}

fn is_shell(name: &str) -> bool {
    matches!(name.trim_start_matches('-'), "bash" | "zsh" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "xonsh")
}

/// A command line as a shell takes it: each word quoted where it must be.
pub fn command_text(words: &[String]) -> String {
    words.iter().map(|w| crate::provision::shell_word(w)).collect::<Vec<_>>().join(" ")
}

/// How an agent's flag takes its value, as its own program parses it
/// (Claude Code's commander, Codex's clap, agy's Go flags).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// Nothing (a switch).
    No,
    /// The next word.
    One,
    /// The next word if it isn't a flag.
    Maybe,
    /// Every next word till a flag (commander's `<values...>`).
    Many,
}

/// An agent's flags that take values, and those that put a session of
/// their own in place of the one resumed (dropped, with their values).
struct Flags {
    takes: &'static [(&'static str, Takes)],
    session: &'static [&'static str],
}

/// Claude Code's (`claude --help`, 2026-11).
const CLAUDE: Flags = Flags {
    takes: &[
        ("--add-dir", Takes::Many),
        ("--agent", Takes::One),
        ("--agents", Takes::One),
        ("--allowedTools", Takes::Many),
        ("--allowed-tools", Takes::Many),
        ("--append-system-prompt", Takes::One),
        ("--autocompact", Takes::One),
        ("--betas", Takes::Many),
        ("--cloud", Takes::Maybe),
        ("-d", Takes::Maybe),
        ("--debug", Takes::Maybe),
        ("--debug-file", Takes::One),
        ("--disallowedTools", Takes::Many),
        ("--disallowed-tools", Takes::Many),
        ("--effort", Takes::One),
        ("--environment", Takes::One),
        ("--fallback-model", Takes::One),
        ("--file", Takes::Many),
        ("--from-pr", Takes::Maybe),
        ("--input-format", Takes::One),
        ("--json-schema", Takes::One),
        ("--max-budget-usd", Takes::One),
        ("--max-turns", Takes::One),
        ("--mcp-config", Takes::Many),
        ("--model", Takes::One),
        ("-n", Takes::One),
        ("--name", Takes::One),
        ("--output-format", Takes::One),
        ("--permission-mode", Takes::One),
        ("--permission-prompts", Takes::One),
        ("--plugin-dir", Takes::One),
        ("--plugin-url", Takes::One),
        ("--prompt-suggestions", Takes::Maybe),
        ("--remote-control", Takes::Maybe),
        ("--remote-control-session-name-prefix", Takes::One),
        ("-r", Takes::Maybe),
        ("--resume", Takes::Maybe),
        ("--session-id", Takes::One),
        ("--setting-sources", Takes::One),
        ("--settings", Takes::One),
        ("--system-prompt", Takes::One),
        ("--system-prompt-snapshot", Takes::One),
    ],
    session: &["-r", "--resume", "--session-id", "-c", "--continue", "--fork-session", "--from-pr", "-p", "--print", "--bg", "--background", "--desktop", "--cloud", "--environment"],
};

/// Codex's (`codex resume --help`, 2026-11: its own options and the
/// interactive ones it shares).
const CODEX: Flags = Flags {
    takes: &[
        ("-c", Takes::One),
        ("--config", Takes::One),
        ("--enable", Takes::One),
        ("--disable", Takes::One),
        ("--remote", Takes::One),
        ("--remote-auth-token-env", Takes::One),
        ("-i", Takes::Many),
        ("--image", Takes::Many),
        ("-m", Takes::One),
        ("--model", Takes::One),
        ("--local-provider", Takes::One),
        ("-p", Takes::One),
        ("--profile", Takes::One),
        ("-s", Takes::One),
        ("--sandbox", Takes::One),
        ("-C", Takes::One),
        ("--cd", Takes::One),
        ("--add-dir", Takes::One),
        ("-a", Takes::One),
        ("--ask-for-approval", Takes::One),
    ],
    session: &["--last", "--all", "--include-non-interactive", "-i", "--image"],
};

/// agy's (`agy --help`, 2026-11; Go's flags: `-x` and `--x` alike).
const AGY: Flags = Flags {
    takes: &[
        ("--add-dir", Takes::One),
        ("--agent", Takes::One),
        ("--conversation", Takes::One),
        ("--resume", Takes::One),
        ("-r", Takes::One),
        ("--effort", Takes::One),
        ("--input-format", Takes::One),
        ("--json-schema", Takes::One),
        ("--log-file", Takes::One),
        ("--mode", Takes::One),
        ("--model", Takes::One),
        ("--output-format", Takes::One),
        ("--print-timeout", Takes::One),
        ("--project", Takes::One),
        ("--settings", Takes::One),
        ("-i", Takes::One),
        ("--prompt-interactive", Takes::One),
    ],
    session: &["--conversation", "--resume", "-r", "-c", "--continue", "-i", "--prompt-interactive", "-p", "--print", "--prompt", "--new-project", "--print-timeout"],
};

/// The command line an agent's pane runs again on the destination: the
/// agent by its name (found on the path there), its own flags as they were
/// (what it was told it may do: permissions, model, effort, folders, with
/// `path` applied to each value, a moved home's paths rewritten), and the
/// session it was in, resumed. Never what starts a session of its own: an
/// old `--resume` or `--continue`, a session id, a first prompt (it would
/// be sent again), a way of running it that isn't its screen (`--print`).
/// `None` for a program that isn't one of the three.
pub fn resume_line(agent: &str, argv: &[String], session: &str, path: &dyn Fn(&str) -> String) -> Option<Vec<String>> {
    let flags = match agent {
        "claude" => &CLAUDE,
        "codex" => &CODEX,
        "agy" => &AGY,
        _ => return None,
    };
    // (Go's flags take `-x` as `--x`; the others' long flags are `--x`.)
    let canonical = |f: &str| -> String { if agent == "agy" && f.starts_with('-') && !f.starts_with("--") && f.len() > 2 { format!("-{f}") } else { f.to_owned() } };
    let takes = |f: &str| flags.takes.iter().find(|(n, _)| *n == f).map_or(Takes::No, |(_, t)| *t);
    let is_flag = |w: &str| w.starts_with('-') && w.len() > 1;
    let mut kept: Vec<String> = Vec::new();
    // (The program itself, and Codex's subcommand, aren't flags.)
    let mut words = argv.iter().skip(1).peekable();
    if agent == "codex" && words.peek().is_some_and(|w| *w == "resume") {
        words.next();
    }
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if !is_flag(word) {
            // (A first prompt, or Codex's `resume ID`'s id: not again.)
            continue;
        }
        let (name, inline) = match word.split_once('=') {
            Some((n, v)) if n.starts_with("--") || agent == "agy" => (canonical(n), Some(v.to_owned())),
            _ => (canonical(word), None),
        };
        let mut values: Vec<String> = Vec::new();
        if inline.is_none() {
            match takes(&name) {
                Takes::No => {}
                Takes::One => values.extend(words.next().cloned()),
                Takes::Maybe => {
                    if words.peek().is_some_and(|w| !is_flag(w)) {
                        values.extend(words.next().cloned());
                    }
                }
                Takes::Many => {
                    while words.peek().is_some_and(|w| !is_flag(w)) {
                        values.extend(words.next().cloned());
                    }
                }
            }
        }
        if flags.session.contains(&name.as_str()) {
            continue;
        }
        match inline {
            Some(v) => kept.push(format!("{word_name}={}", path(&v), word_name = word.split_once('=').map_or(word.as_str(), |(n, _)| n))),
            None => {
                kept.push(word.clone());
                kept.extend(values.iter().map(|v| path(v)));
            }
        }
    }
    let mut line = vec![agent.to_owned()];
    match agent {
        "claude" => {
            line.extend(kept);
            line.extend(["--resume".to_owned(), session.to_owned()]);
        }
        "codex" => {
            line.push("resume".to_owned());
            line.extend(kept);
            line.push(session.to_owned());
        }
        _ => {
            line.extend(kept);
            line.extend(["--conversation".to_owned(), session.to_owned()]);
        }
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = r#"{"system":"ubuntu","system_like":"debian","system_name":"Ubuntu 26.04.1 LTS","machine":"x86_64","user":"ubuntu","uid":1000,"gid":1000,"home":"/home/ubuntu","shell":"/bin/bash","free":52428800000,"free_inodes":3000000,"var_tmp_free":1,"var_tmp_inodes":1,"admin":true,"groups":["ubuntu","sudo"],"tools":["crontab","sudo","git","bash","systemctl","tmux"],"selinux":false,"linger":true,"units":["backup.service","backup.timer","rex.socket"],"enabled":["backup.timer","rex.socket"],"listening":[8080],"other_logins":1,"mounts":[],"tmux":true,"screen":false,"machine_id":"aa","boot_id":"b1","home_dev":2049,"home_ino":131073,"ssh_port":22,"now":1795600000,"helper":"1"}"#;
    const FEDORA: &str = r#"{"system":"fedora","system_name":"Fedora Linux 44 (Cloud Edition)","machine":"aarch64","user":"fedora","uid":1000,"gid":1000,"home":"/home/fedora","shell":"/bin/bash","free":1073741824,"admin":true,"tools":["sudo","bash"],"selinux":true,"machine_id":"cc","boot_id":"b2","ssh_port":2222,"now":1795600003}"#;

    /// Each host's look is read whole (what an older helper doesn't say is
    /// its default); what a move needs to know of a pair (another system,
    /// another machine, another home, the same home) follows from it.
    #[test]
    fn a_hosts_look_is_read() {
        let (from, to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        assert_eq!((from.system.as_str(), from.home.as_str(), from.free), ("ubuntu", "/home/ubuntu", 52_428_800_000));
        assert!(from.has("crontab") && !to.has("crontab") && to.has("sudo"));
        assert_eq!((from.units.len(), from.enabled.as_slice()), (3, ["backup.timer".to_string(), "rex.socket".to_string()].as_slice()));
        assert_eq!((from.listening.as_slice(), from.other_logins, from.tmux), ([8080u16].as_slice(), 1, true));
        assert!(from.admin && to.admin && to.selinux && !from.selinux && from.linger && !to.linger);
        assert_eq!((to.ssh_port, to.mounts.len(), to.system_like.as_str()), (Some(2222), 0, ""));
        assert!(!from.same_machine(&to), "x86_64 to aarch64");
        assert!(!from.same_home(&to) && from.same_home(&from));
        assert!(Facts::read("").is_err() && Facts::read(r#"{"user":"a"}"#).is_err());
    }

    /// A pane's tree as the helper says it: its programs whole, and the
    /// session found open anywhere in it, Codex's by its rollout's id,
    /// agy's by its conversation's (or that conversation's log).
    #[test]
    fn a_panes_tree_is_read() {
        let said = r#"[{"pid":41,"tree":[{"pid":41,"ppid":1,"name":"bash","argv":["bash","-l"],"cwd":"/home/ubuntu/proj-b","start":9000,"open":[]},{"pid":42,"ppid":41,"name":"codex","argv":["codex","--yolo"],"cwd":"/home/ubuntu/proj-b","start":9100,"open":["/home/ubuntu/.codex/sessions/2026/11/25/rollout-2026-11-25T01-02-03-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl"]}]},{"pid":43,"tree":[{"pid":43,"ppid":1,"name":"agy","argv":["agy"],"open":["/home/ubuntu/.gemini/antigravity-cli/conversations/3f2a7c1e-0000-4000-8000-123456789abc.db-wal"]}]},{"pid":44,"tree":[]}]"#;
        let panes = Pane::read_all(said).unwrap();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].tree[1].argv, ["codex", "--yolo"]);
        assert_eq!(panes[0].open_session().as_deref(), Some("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"));
        assert_eq!(panes[1].open_session().as_deref(), Some("3f2a7c1e-0000-4000-8000-123456789abc"));
        assert_eq!(panes[2].open_session(), None);
        assert_eq!(look_pane_command("abc", &[41, 43]), r#""$HOME"/.local/share/olo/rex-link/olo-rex-link-abc-$(uname -m) move look-pane 41 43"#);
    }

    /// Each agent comes back with its own flags and the session it was in:
    /// never an old resume or continue, a session id, a first prompt, or a
    /// way of running it that isn't its screen; a moved home's paths
    /// rewritten in its flags' values; the program by its name.
    #[test]
    fn an_agent_comes_back_in_its_session_with_its_flags() {
        let a = |s: &str| s.split(' ').map(|w| w.replace('_', " ")).collect::<Vec<String>>();
        let path = |p: &str| p.replace("/home/ubuntu", "/home/fedora");
        let line = |agent: &str, argv: &str| resume_line(agent, &a(argv), "SID", &path).map(|l| l.join(" "));
        assert_eq!(line("claude", "claude --dangerously-skip-permissions"), Some("claude --dangerously-skip-permissions --resume SID".into()));
        assert_eq!(line("claude", "/home/ubuntu/.local/bin/claude --resume OLD --model opus fix_the_bug"), Some("claude --model opus --resume SID".into()));
        assert_eq!(line("claude", "claude -c --permission-mode acceptEdits"), Some("claude --permission-mode acceptEdits --resume SID".into()));
        assert_eq!(line("claude", "claude --add-dir /home/ubuntu/a /home/ubuntu/b --session-id X"), Some("claude --add-dir /home/fedora/a /home/fedora/b --resume SID".into()));
        assert_eq!(line("claude", "claude --model=sonnet -r"), Some("claude --model=sonnet --resume SID".into()));
        assert_eq!(line("claude", "claude -p say_hi"), Some("claude --resume SID".into()));
        assert_eq!(line("codex", "codex --yolo"), Some("codex resume --yolo SID".into()));
        assert_eq!(line("codex", "codex resume OLD -m gpt-6 -C /home/ubuntu/proj"), Some("codex resume -m gpt-6 -C /home/fedora/proj SID".into()));
        assert_eq!(line("codex", "codex resume --last --dangerously-bypass-approvals-and-sandbox"), Some("codex resume --dangerously-bypass-approvals-and-sandbox SID".into()));
        assert_eq!(line("codex", "codex -c model=\"o3\" write_tests"), Some("codex resume -c model=\"o3\" SID".into()));
        assert_eq!(line("agy", "agy --model gemini-4-neon-high -mode auto"), Some("agy --model gemini-4-neon-high -mode auto --conversation SID".into()));
        assert_eq!(line("agy", "agy -r OLD --dangerously-skip-permissions -i do_it"), Some("agy --dangerously-skip-permissions --conversation SID".into()));
        assert_eq!(line("agy", "agy --add-dir=/home/ubuntu/x -c"), Some("agy --add-dir=/home/fedora/x --conversation SID".into()));
        assert_eq!(line("vim", "vim x"), None);
    }

    /// What never moves, from the home's root: the destination's own keys,
    /// each host's own Olo and Rex state, the move's own; `.cache` unless
    /// asked; Codex's database where the home's path changes, its daemon's
    /// packages where the system or machine does.
    #[test]
    fn what_never_moves() {
        let (from, to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        let ex = excluded(&from, &to, false);
        for must in [
            "/.ssh/authorized_keys",
            "/.local/share/olo/rex-link",
            "/.local/state/olo-rex/move",
            "/.local/share/olo/pool",
            "/.config/olo/pool.*",
            "/.cache",
            "/.codex/state_*.sqlite*",
            "/.codex/packages/app-server-daemon",
        ] {
            assert!(ex.iter().any(|e| e == must), "{must} left out");
        }
        assert!(ex.iter().all(|e| e.starts_with('/')), "each from the home's root");
        let same = excluded(&from, &from, true);
        assert!(!same.iter().any(|e| e == "/.cache" || e == "/.codex/state_*.sqlite*" || e == "/.codex/packages/app-server-daemon"));
    }

    /// A move is refused for the same home, clocks apart, or too little room
    /// (bytes and files), each said as a sentence.
    #[test]
    fn a_move_is_refused_when_it_cant_work() {
        let (from, mut to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        to.free = 100_000_000_000;
        to.free_inodes = 5_000_000;
        assert!(refusals(&from, &to, 20_000_000_000, 300_000).is_empty());
        let no_room = refusals(&from, &to, 95_000_000_000, 300_000);
        assert_eq!(no_room, ["The destination (Fedora Linux 44 (Cloud Edition)) has 100.0 GB free in its home; the move needs 105.6 GB."]);
        to.free_inodes = 200_000;
        assert_eq!(refusals(&from, &to, 1, 300_000).len(), 1);
        to.free_inodes = 0;
        to.now = from.now + 3600;
        assert!(refusals(&from, &to, 1, 1)[0].starts_with("The two hosts' clocks are 3600 seconds apart"));
        assert_eq!(refusals(&from, &from, 1, 1)[0], "That's the same account on the same machine.");
    }

    /// The source is stopped so it stays stopped: crontab saved and emptied,
    /// the account's own starters before its services (Rex's socket among
    /// them, Rex's server last), the agents before Codex's daemon, tmux;
    /// never the system's user units, never lingering.
    #[test]
    fn the_source_is_stopped_in_order() {
        let from = Facts::read(UBUNTU).unwrap();
        let s = stop_script(&from, "/home/ubuntu/.local/state/olo-rex/move/m1", &[801, 802]);
        let at = |needle: &str| s.find(needle).unwrap_or_else(|| panic!("{needle} in\n{s}"));
        assert!(at("crontab -l > \"$d/crontab\"") < at("systemctl --user stop backup.timer"));
        assert!(at("systemctl --user stop backup.timer") < at("systemctl --user stop backup.service"));
        assert!(at("stop rex.socket") < at("kill -TERM 801 802"));
        assert!(at("kill -TERM 801 802") < at("daemon.pid"));
        assert!(at("tmux kill-server") < at("stop rex.service"));
        assert!(!s.contains("dbus") && !s.contains("linger") && !s.contains("olo-pool"), "{s}");
    }

    /// A window's panes made again as they were: a pane split right, its
    /// right one split down, each split's ratio set as it was, and which
    /// source block each new pane stands for.
    #[test]
    fn a_windows_panes_are_made_again() {
        let layout: crate::model::LayoutNode = serde_json::from_value(serde_json::json!({
            "split": {
                "split_id": "s1", "direction": "horizontal", "ratio": 0.6,
                "before": {"block_id": "shell"},
                "after": {"split": {
                    "split_id": "s2", "direction": "vertical", "ratio": 0.3,
                    "before": {"split": {"split_id": "s3", "direction": "horizontal", "ratio": 0.5, "before": {"block_id": "claude"}, "after": {"block_id": "codex"}}},
                    "after": {"block_id": "todo"}
                }}
            }
        }))
        .unwrap();
        let (splits, panes) = rebuild_splits(&layout);
        assert_eq!(splits, [Split { of: 0, new: 1, direction: "right", ratio: 0.6 }, Split { of: 1, new: 2, direction: "down", ratio: 0.3 }, Split { of: 1, new: 3, direction: "right", ratio: 0.5 },]);
        assert_eq!(panes, [Some("shell".into()), Some("claude".into()), Some("todo".into()), Some("codex".into())]);
        let one: crate::model::LayoutNode = serde_json::from_value(serde_json::json!({"block_id": "only"})).unwrap();
        assert_eq!(rebuild_splits(&one), (vec![], vec![Some("only".into())]));
    }

    /// The message is one line, no `@`, and says only what is true: another
    /// system and machine, the home's new path, a cut turn, ports.
    #[test]
    fn the_message_says_what_is_true() {
        let (from, to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        let told = Told { from_label: "old-box", to_label: "new@box", from: from.clone(), to: to.clone(), when: "at 14:05", linked: true, cut: true, ports: &[8080, 5173], ..Told::default() };
        let m = message(&told);
        assert!(!m.contains('\n') && !m.contains('\r') && !m.contains('@'), "{m}");
        assert!(m.starts_with("[Olo] This session was moved from old-box (Ubuntu 26.04.1 LTS, x86_64) to new at box (Fedora Linux 44 (Cloud Edition), aarch64) at 14:05."), "{m}");
        for part in [
            "Your home moved from /home/ubuntu to /home/fedora (the old path still leads here).",
            "/tmp and /var/tmp did not",
            "Your last turn was cut off",
            "(apt there, dnf here)",
            "This is an aarch64 machine, not x86_64",
            "ports 8080, 5173",
            "$REX_SESSION",
        ] {
            assert!(m.contains(part), "{part} in {m}");
        }
        let same = message(&Told { from_label: "a", to_label: "b", from: from.clone(), to: from.clone(), var_tmp: true, ..Told::default() });
        assert!(!same.contains("home moved") && !same.contains("packages") && !same.contains("machine") && !same.contains("cut off") && !same.contains("port"), "{same}");
        assert!(same.contains("and your folders in /var/tmp"));
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// A host that writes down each command and answers from a script:
    /// the first answer whose words are in the command, else nothing, ended
    /// well.
    struct Scripted {
        ran: Vec<(bool, String)>,
        answers: Vec<(&'static str, u32, String)>,
    }

    impl crate::provision::Runner for Scripted {
        async fn run(&mut self, root: bool, command: &str, _: &mut (dyn FnMut(&str) + Send)) -> Result<crate::provision::Ran, crate::provision::Failed> {
            self.ran.push((root, command.to_owned()));
            let (status, output) = self.answers.iter().find(|(w, _, _)| command.contains(w)).map(|(_, s, o)| (*s, o.clone())).unwrap_or((0, String::new()));
            Ok(crate::provision::Ran { status: Some(status), output })
        }
    }

    /// The move's steps on its hosts, each as the helper's command with its
    /// words quoted whole, its answer read: the key's line, the first way
    /// that reaches, a pass started detached (its excludes passed on), the
    /// source stopped then drained, lingering off by root where the account
    /// can't, the crontab carried with the new home's paths, and the key and
    /// folder taken away after.
    #[test]
    fn the_moves_steps_run_on_its_hosts() {
        let (from, to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        let mut src = Scripted {
            ran: vec![],
            answers: vec![
                ("move key", 0, "ssh-ed25519 AAAAkey olo-move-m1\n".into()),
                ("move reach", 1, r#"{"ok":false,"error":"connection refused"}"#.into()),
                ("move send", 0, "{\"started\":4242}\n".into()),
                ("move stop", 0, r#"{"ended":3,"left":[]}"#.into()),
                ("disable-linger ubuntu", 0, String::new()),
                ("loginctl disable-linger", 1, "Access denied\n".into()),
            ],
        };
        let mut dst = Scripted { ran: vec![], answers: vec![] };
        let public = block(make_key(&mut src, "abc", &from, "m1")).unwrap();
        assert_eq!(public, "ssh-ed25519 AAAAkey olo-move-m1");
        block(allow(&mut dst, "abc", "m1", &public)).unwrap();
        assert!(dst.ran[0].1.ends_with("move allow m1 'ssh-ed25519 AAAAkey olo-move-m1'"), "{:?}", dst.ran);
        let way = Way { user: "fedora".into(), host: "dest".into(), port: 22, host_key: "ssh-ed25519 HOSTKEY".into() };
        let refused = block(reach(&mut src, "abc", &from, &to, "m1", std::slice::from_ref(&way))).unwrap_err();
        assert_eq!(refused, "dest:22: connection refused");
        let pid = block(start_pass(&mut src, "abc", &from, &to, "m1", &way, 1, &["/.cache".to_owned(), "/a b".to_owned()])).unwrap();
        assert_eq!(pid, 4242);
        let send = &src.ran.iter().find(|(_, c)| c.contains("move send")).unwrap().1;
        assert!(
            send.contains(
                "move send /home/ubuntu/.local/state/olo-rex/move/m1 --to fedora@dest:22 --host-key 'ssh-ed25519 HOSTKEY' --there /home/fedora --pass 1 --detach --exclude /.cache --exclude '/a b'"
            ),
            "{send}"
        );
        let left = block(stop_source(&mut src, "abc", &from, "m1", &[801])).unwrap();
        assert!(left.is_empty());
        let n = src.ran.len();
        assert!(src.ran[n - 2].1.starts_with("sh -c ") && src.ran[n - 2].1.contains("kill -TERM 801") && src.ran[n - 1].1.ends_with("move stop"));
        block(linger_off(&mut src, &from)).unwrap();
        assert_eq!(src.ran.last().unwrap(), &(true, "loginctl disable-linger ubuntu".to_owned()), "by root once the account was refused");
        block(units_and_crontab(&mut dst, &from, &to, "m1", Some("0 * * * * /home/ubuntu/bin/backup\n"))).unwrap();
        let units = &dst.ran.last().unwrap().1;
        assert!(units.contains("enable --now backup.timer") && !units.contains("rex.socket"), "{units}");
        assert!(units.contains("0 * * * * /home/fedora/bin/backup") && units.contains("OLO_MOVE_CRONTAB"), "{units}");
        assert!(!units.contains("crontab /home"), "the destination has no crontab program: kept in the move's folder");
        block(clean_up(&mut src, &mut dst, "abc", &from, "m1")).unwrap();
        assert!(dst.ran.last().unwrap().1.ends_with("move disallow m1"));
        assert_eq!(src.ran.last().unwrap().1, "rm -rf /home/ubuntu/.local/state/olo-rex/move/m1");
    }

    /// Each pane comes back running what it ran: an agent resumed in its
    /// session with its flags (its home's paths the new home's), a viewer
    /// run again, another program only typed, a shell a shell; the agents'
    /// pids kept to stop them first.
    #[test]
    fn each_pane_comes_back_running_what_it_ran() {
        let captured = CapturedSession {
            panes: vec![
                CapturedPane { block_id: "b1".into(), pid: Some(10), foreground: Some(11), cwd: "/home/ubuntu/proj-a".into(), ..CapturedPane::default() },
                CapturedPane { block_id: "b2".into(), pid: Some(20), foreground: Some(20), cwd: "/home/ubuntu".into(), ..CapturedPane::default() },
                CapturedPane { block_id: "b3".into(), pid: Some(30), foreground: Some(31), cwd: "/home/ubuntu/proj-b".into(), ..CapturedPane::default() },
                CapturedPane { block_id: "b4".into(), pid: Some(40), foreground: Some(40), cwd: "/home/ubuntu".into(), ..CapturedPane::default() },
            ],
            ..CapturedSession::default()
        };
        let prog = |pid: u32, argv: &[&str]| Program { pid, name: argv[0].rsplit('/').next().unwrap().into(), argv: argv.iter().map(|a| a.to_string()).collect(), ..Program::default() };
        let panes = vec![
            Pane {
                pid: 10,
                tree: vec![prog(10, &["bash"]), prog(11, &["claude", "--dangerously-skip-permissions"])],
                agent: Some(PaneAgent {
                    agent: "claude".into(),
                    pid: 11,
                    argv: vec!["claude".into(), "--dangerously-skip-permissions".into()],
                    session: Some("S1".into()),
                    how: "written".into(),
                    ..PaneAgent::default()
                }),
            },
            Pane { pid: 20, tree: vec![prog(20, &["tail", "-f", "/home/ubuntu/todo.txt"])], agent: None },
            Pane { pid: 30, tree: vec![prog(30, &["bash"]), prog(31, &["git", "push"])], agent: None },
            Pane { pid: 40, tree: vec![prog(40, &["bash", "-l"])], agent: None },
        ];
        let path = |p: &str| p.replace("/home/ubuntu", "/home/fedora");
        let (runs, agents, run_it) = pane_runs(&captured, &panes, &path);
        assert_eq!(agents, [11]);
        assert_eq!(runs["b1"], PaneRun { cwd: "/home/fedora/proj-a".into(), command: Some(["claude", "--dangerously-skip-permissions", "--resume", "S1"].map(String::from).to_vec()) });
        assert_eq!(runs["b2"].command.as_deref(), Some(["tail", "-f", "/home/fedora/todo.txt"].map(String::from).as_slice()));
        assert_eq!((run_it["b1"], run_it["b2"], run_it["b3"]), (true, true, false), "a viewer runs again; git push is only typed");
        assert_eq!(runs["b3"].command.as_deref(), Some(["git", "push"].map(String::from).as_slice()));
        assert_eq!(runs["b4"].command, None);
        assert_eq!(command_text(&["tail".into(), "-f".into(), "/home/fedora/my todo.txt".into()]), "tail -f '/home/fedora/my todo.txt'");
    }
}
